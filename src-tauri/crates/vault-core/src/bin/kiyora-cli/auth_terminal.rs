//! Terminal-only presentation. No TDLib requests or token generation here.
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers},
    terminal,
};
use futures_util::StreamExt;
use qrcode::{Color, QrCode};
use std::io::{self, BufRead, IsTerminal, Write};
use vault_core::{ApiError, ErrorCode};
use zeroize::Zeroizing;

pub(super) enum InputEvent {
    Line(Zeroizing<String>),
    Cancel,
    Resize,
}

pub(super) trait AuthUi {
    fn message(&mut self, text: &str) -> Result<(), ApiError>;
    fn prompt(&mut self, text: &str) -> Result<(), ApiError>;
    fn qr(&mut self, link: &str) -> Result<(), ApiError>;
    fn clear_qr(&mut self) -> Result<(), ApiError>;
    async fn input(&mut self) -> Result<InputEvent, ApiError>;
}

pub(super) struct Terminal {
    events: Option<EventStream>,
    lines: Option<tokio::sync::mpsc::Receiver<Zeroizing<String>>>,
    buffer: Zeroizing<String>,
    ansi: bool,
    qr_visible: bool,
    alternate: bool,
}
fn io_error(_: io::Error) -> ApiError {
    ApiError::new(ErrorCode::Storage)
}
impl Terminal {
    pub fn new() -> Result<Self, ApiError> {
        let tty = io::stdin().is_terminal();
        if tty {
            terminal::enable_raw_mode().map_err(io_error)?;
        }
        let lines = if tty {
            None
        } else {
            // A plain thread (not spawn_blocking) cannot hold Tokio shutdown hostage
            // when stdin is a pipe with no newline. Only a single reader is created.
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            std::thread::spawn(move || {
                let mut stdin = io::stdin().lock();
                loop {
                    let mut line = Zeroizing::new(String::new());
                    match stdin.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    while line.ends_with(['\r', '\n']) {
                        line.pop();
                    }
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
            });
            Some(rx)
        };
        Ok(Self {
            events: tty.then(EventStream::new),
            lines,
            buffer: Zeroizing::new(String::new()),
            ansi: io::stdout().is_terminal() && std::env::var("TERM").is_ok_and(|s| s != "dumb"),
            qr_visible: false,
            alternate: false,
        })
    }
    fn write(&self, text: &str) -> Result<(), ApiError> {
        let mut out = io::stdout().lock();
        // Raw mode disables the terminal's newline translation.
        if self.events.is_some() {
            out.write_all(text.replace('\n', "\r\n").as_bytes())
                .map_err(io_error)?;
        } else {
            out.write_all(text.as_bytes()).map_err(io_error)?;
        }
        out.flush().map_err(io_error)
    }
    fn size(&self) -> (usize, usize) {
        terminal::size()
            .map(|(w, h)| (usize::from(w), usize::from(h)))
            .unwrap_or((80, 24))
    }
}
impl AuthUi for Terminal {
    fn message(&mut self, text: &str) -> Result<(), ApiError> {
        self.write(&format!("{text}\n"))
    }
    fn prompt(&mut self, text: &str) -> Result<(), ApiError> {
        self.buffer.clear();
        self.write(text)
    }
    fn qr(&mut self, link: &str) -> Result<(), ApiError> {
        self.clear_qr()?;
        // Do not turn redirected stdout into a credential log (even as a QR).
        if !io::stdout().is_terminal() {
            self.message(
                "QR login requires terminal output. Run auth qr without redirecting stdout.",
            )?;
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let (width, height) = self.size();
        let Some(qr) = render_qr(link, width, height.saturating_sub(3))? else {
            self.message("Terminal too small for this QR. Enlarge it; the current code will appear automatically.")?;
            return Ok(());
        };
        if self.ansi {
            self.write("\x1b[?1049h\x1b[2J\x1b[H")?;
            self.alternate = true;
        }
        self.message("Telegram: Settings > Devices > Link Device")?;
        // Explicit black/white, independent of the user's terminal theme.
        // The renderer draws LIGHT modules as foreground blocks.
        if self.ansi {
            self.write("\x1b[97;40m")?;
        }
        self.write(&qr)?;
        if self.ansi {
            self.write("\x1b[0m")?;
        }
        self.message("Waiting… Ctrl+C or /cancel to cancel.")?;
        self.qr_visible = true;
        Ok(())
    }
    fn clear_qr(&mut self) -> Result<(), ApiError> {
        if self.alternate {
            self.write("\x1b[0m\x1b[2J\x1b[H\x1b[?1049l")?;
            self.alternate = false;
        } else if self.qr_visible {
            // TERM=dumb has no cursor addressing. Push the old frame out of the
            // viewport and explicitly invalidate it; scrollback cannot be erased.
            self.write(&"\n".repeat(self.size().1))?;
            self.message("Previous QR is no longer current.")?;
        }
        self.qr_visible = false;
        Ok(())
    }
    async fn input(&mut self) -> Result<InputEvent, ApiError> {
        if let Some(lines) = &mut self.lines {
            return Ok(lines
                .recv()
                .await
                .map(InputEvent::Line)
                .unwrap_or(InputEvent::Cancel));
        }
        loop {
            let event = self
                .events
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::Internal))?
                .next()
                .await
                .ok_or_else(|| ApiError::new(ErrorCode::Cancelled))?
                .map_err(io_error)?;
            match event {
                Event::Resize(_, _) => return Ok(InputEvent::Resize),
                Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(InputEvent::Cancel);
                    }
                    KeyCode::Enter => {
                        let line =
                            std::mem::replace(&mut self.buffer, Zeroizing::new(String::new()));
                        if !self.qr_visible {
                            self.write("\n")?;
                        }
                        return Ok(InputEvent::Line(line));
                    }
                    KeyCode::Backspace => {
                        self.buffer.pop();
                    }
                    KeyCode::Char(c) if !c.is_control() && self.buffer.len() < 4096 => {
                        self.buffer.push(c)
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.clear_qr();
        if self.events.is_some() {
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// Four light modules on every edge. Half blocks fit two QR rows in one cell;
/// full blocks use two columns per module when both dimensions permit it.
/// Never scales down modules or wraps a QR. Input bytes go to the encoder unchanged.
pub(super) fn render_qr(
    link: &str,
    columns: usize,
    rows: usize,
) -> Result<Option<String>, ApiError> {
    if link.is_empty() {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let qr = QrCode::new(link.as_bytes()).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    let side = qr.width() + 8;
    if columns < side + 1 || rows < side.div_ceil(2) {
        return Ok(None);
    }
    let full = columns > side * 2 && rows >= side;
    let light = |x: usize, y: usize| {
        x < 4 || y < 4 || x >= side - 4 || y >= side - 4 || qr[(x - 4, y - 4)] == Color::Light
    };
    let mut output = String::new();
    for y in (0..side).step_by(if full { 1 } else { 2 }) {
        for x in 0..side {
            if full {
                output.push_str(if light(x, y) { "██" } else { "  " });
            } else {
                output.push(match (light(x, y), light(x, y + 1)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
        }
        output.push('\n');
    }
    Ok(Some(output))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn terminal_qr_decodes_exact_tdlib_link_at_both_sizes_with_quiet_zone() {
        let link = "tg://login?token=A_B-abcdefghijklmnopqrstuvwxyz012345678901234";
        for (columns, rows, full) in [(80, 21, false), (160, 70, true)] {
            let text = render_qr(link, columns, rows).unwrap().unwrap();
            assert!(!text.contains(link));
            let cells = text
                .lines()
                .map(|line| line.chars().collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let width = cells[0].len();
            assert!(width < columns);
            assert!(cells.len() <= rows);
            let height = cells.len() * 2;
            let white = |x: usize, y: usize| match cells[y / 2][x] {
                '█' => true,
                ' ' => false,
                '▀' => y.is_multiple_of(2),
                '▄' => !y.is_multiple_of(2),
                _ => panic!("unexpected glyph"),
            };
            let zone = if full { 8 } else { 4 };
            for y in 0..height {
                for x in 0..width {
                    if x < zone || x >= width - zone || y < zone || y >= height - zone {
                        assert!(white(x, y), "quiet zone must be light");
                    }
                }
            }
            let mut image =
                rqrr::PreparedImage::prepare_from_bitmap(width * 6, height * 6, |x, y| {
                    !white(x / 6, y / 6)
                });
            let grids = image.detect_grids();
            assert_eq!(grids.len(), 1);
            assert_eq!(grids[0].decode().unwrap().1, link);
        }
        assert!(render_qr(link, 20, 24).unwrap().is_none());
        assert!(render_qr(link, 80, 10).unwrap().is_none());
        assert!(render_qr("", 80, 24).is_err());
    }
}
