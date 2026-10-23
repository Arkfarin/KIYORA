//! Advisory matching only: approximate candidates are never automatically merged.
use vault_domain::Metadata;
fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub fn metadata_similar(left: &Metadata, right: &Metadata) -> bool {
    let title = normalized(&left.title);
    let performer = normalized(&left.performer);
    !title.is_empty()
        && !performer.is_empty()
        && title == normalized(&right.title)
        && performer == normalized(&right.performer)
        && left.duration_ms > 0
        && right.duration_ms > 0
        && left.duration_ms.abs_diff(right.duration_ms) <= 2000
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approximate_match_requires_named_artist_title_and_close_duration() {
        let a = Metadata {
            title: "  A   Song ".into(),
            performer: " Artist ".into(),
            duration_ms: 180_000,
            ..Default::default()
        };
        let mut b = Metadata {
            title: "a song".into(),
            performer: "artist".into(),
            duration_ms: 181_500,
            ..Default::default()
        };
        assert!(metadata_similar(&a, &b));
        b.duration_ms = 190_000;
        assert!(!metadata_similar(&a, &b));
        b.duration_ms = 180_000;
        b.performer.clear();
        assert!(!metadata_similar(&a, &b));
        b.performer = "artist".into();
        b.title = "another".into();
        assert!(!metadata_similar(&a, &b));
    }
}
