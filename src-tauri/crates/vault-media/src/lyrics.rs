//! Local subtitle parsing. Timings are normalized before any document is published.
use crate::MediaError;
use vault_domain::{LyricLine, LyricsDocument};

fn stamp(input: &str) -> Option<u64> {
    let normalized = input.trim().replace(',', ".");
    let parts: Vec<_> = normalized.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let seconds: f64 = parts.last()?.parse().ok()?;
    if !seconds.is_finite() || !(0.0..60.0).contains(&seconds) {
        return None;
    }
    let minutes: u64 = parts[parts.len() - 2].parse().ok()?;
    let hours: u64 = if parts.len() == 3 {
        parts[0].parse().ok()?
    } else {
        0
    };
    if minutes > 10080 || (parts.len() == 3 && minutes >= 60) || hours > 168 {
        return None;
    }
    Some(hours * 3_600_000 + minutes * 60_000 + (seconds * 1000.0).round() as u64)
}
pub fn parse_lyrics(text: &str, format: &str) -> Result<LyricsDocument, MediaError> {
    if text.len() > 128 * 1024 || text.contains('\0') {
        return Err(MediaError::InvalidAudio);
    }
    let normalized = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines = Vec::new();
    match format {
        "plain" | "txt" => lines.extend(normalized.lines().map(|text| LyricLine {
            start_ms: None,
            end_ms: None,
            text: text.into(),
        })),
        "lrc" => {
            let mut offset = 0i64;
            for line in normalized.lines() {
                if let Some(value) = line
                    .strip_prefix("[offset:")
                    .and_then(|s| s.strip_suffix(']'))
                {
                    offset = value.parse().map_err(|_| MediaError::InvalidAudio)?;
                    if offset.unsigned_abs() > 86_400_000 {
                        return Err(MediaError::InvalidAudio);
                    }
                    continue;
                }
                let mut rest = line;
                let mut times = Vec::new();
                while let Some(value) = rest.strip_prefix('[') {
                    let (tag, tail) = value.split_once(']').ok_or(MediaError::InvalidAudio)?;
                    if let Some(time) = stamp(tag) {
                        times.push(time);
                    } else if tag.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        return Err(MediaError::InvalidAudio);
                    }
                    rest = tail;
                }
                if times.is_empty() && !rest.trim().is_empty() {
                    return Err(MediaError::InvalidAudio);
                }
                for time in times {
                    lines.push(LyricLine {
                        start_ms: Some(time),
                        end_ms: None,
                        text: rest.into(),
                    });
                }
            }
            for line in &mut lines {
                line.start_ms = line.start_ms.map(|n| n.saturating_add_signed(offset));
            }
            lines.sort_by_key(|line| line.start_ms);
        }
        "srt" | "vtt" => {
            for block in normalized.split("\n\n") {
                let block = block.trim();
                if block.is_empty() {
                    continue;
                }
                let mut rows = block.lines();
                let Some(first) = rows.next() else {
                    continue;
                };
                if first.starts_with("WEBVTT")
                    || first.starts_with("NOTE")
                    || first.starts_with("STYLE")
                    || first.starts_with("REGION")
                {
                    continue;
                }
                let timing = if first.contains("-->") {
                    first
                } else {
                    rows.next().ok_or(MediaError::InvalidAudio)?
                };
                let (start, end) = timing.split_once("-->").ok_or(MediaError::InvalidAudio)?;
                let start = stamp(start).ok_or(MediaError::InvalidAudio)?;
                let end = stamp(
                    end.split_whitespace()
                        .next()
                        .ok_or(MediaError::InvalidAudio)?,
                )
                .ok_or(MediaError::InvalidAudio)?;
                lines.push(LyricLine {
                    start_ms: Some(start),
                    end_ms: Some(end),
                    text: rows.collect::<Vec<_>>().join("\n"),
                });
            }
        }
        _ => return Err(MediaError::InvalidAudio),
    }
    let document = LyricsDocument {
        version: 1,
        language: None,
        lines,
    };
    if !document.valid() {
        return Err(MediaError::InvalidAudio);
    }
    Ok(document)
}
pub fn lyrics_text(document: &LyricsDocument) -> String {
    document
        .lines
        .iter()
        .map(|line| match line.start_ms {
            Some(n) => format!(
                "[{:02}:{:02}.{:03}]{}",
                n / 60000,
                n / 1000 % 60,
                n % 1000,
                line.text
            ),
            None => line.text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
pub fn embedded_lyrics(path: &std::path::Path) -> Result<Option<LyricsDocument>, MediaError> {
    use lofty::{file::TaggedFileExt, probe::Probe, tag::ItemKey};
    let tagged = Probe::open(path)
        .map_err(|_| MediaError::InvalidAudio)?
        .guess_file_type()
        .map_err(|_| MediaError::InvalidAudio)?
        .read()
        .map_err(|_| MediaError::InvalidAudio)?;
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(None);
    };
    let Some(text) = tag
        .get_string(ItemKey::UnsyncLyrics)
        .or_else(|| tag.get_string(ItemKey::Lyrics))
    else {
        return Ok(None);
    };
    let format = if text.lines().any(|s| {
        s.starts_with('[')
            && s.get(1..3)
                .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
    }) {
        "lrc"
    } else {
        "plain"
    };
    parse_lyrics(text, format).map(Some)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn lrc_repeated_times_and_offset_are_normalized() {
        let d = parse_lyrics(
            "[ar:Artist]\n[offset:-250]\n[00:02.50][00:01.00]Hello",
            "lrc",
        )
        .unwrap();
        assert_eq!(d.lines[0].start_ms, Some(750));
        assert_eq!(d.lines[1].start_ms, Some(2250));
        assert!(parse_lyrics("[00:99]bad", "lrc").is_err());
    }
    #[test]
    fn subtitles_validate_order_and_intervals_without_rendering_markup() {
        let d = parse_lyrics(
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 align:start\n<b>hello</b>",
            "vtt",
        )
        .unwrap();
        assert_eq!(d.lines[0].text, "<b>hello</b>");
        assert!(parse_lyrics("1\n00:00:03,000 --> 00:00:02,000\nbad", "srt").is_err());
        assert!(parse_lyrics(&"a".repeat(128 * 1024 + 1), "plain").is_err());
    }
}
