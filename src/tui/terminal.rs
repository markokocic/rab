use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent, KeyboardEnhancementFlags};

// =============================================================================
// TerminalEvent — events that can come from the terminal beyond just keys
// =============================================================================

/// Events from the terminal that the app loop should handle.
/// Matches pi's StdinBuffer which emits both `data` (key sequences) and `paste` events.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    Key(KeyEvent),
    Paste(String),
    Resize(u16, u16),
}

// =============================================================================
// Terminal trait — pi-compatible terminal interface
// =============================================================================

/// Terminal interface matching pi's `packages/tui/src/terminal.ts`.
/// All methods work with a `&mut dyn Write` for flexibility.
pub trait TerminalTrait {
    fn start(&mut self, writer: &mut dyn Write) -> io::Result<()>;
    fn stop(&mut self, writer: &mut dyn Write) -> io::Result<()>;
    fn drain_input(&mut self, max_ms: u64) -> io::Result<()>;
    fn write(&self, writer: &mut dyn Write, data: &str) -> io::Result<()>;
    fn size(&self) -> io::Result<(u16, u16)>;
    fn kitty_protocol_active(&self) -> bool;
    fn move_by(&self, writer: &mut dyn Write, lines: i32) -> io::Result<()>;
    fn hide_cursor(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn show_cursor(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn clear_line(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn clear_from_cursor(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn clear_screen(&self, writer: &mut dyn Write) -> io::Result<()>;
    fn set_title(&self, writer: &mut dyn Write, title: &str) -> io::Result<()>;
    fn set_progress(&self, writer: &mut dyn Write, active: bool) -> io::Result<()>;
    /// Enable/disable terminal color scheme change notifications (OSC 2031).
    /// When enabled, the terminal reports color scheme changes via
    /// `\x1b]10;rgb:RRRR/GGGG/BBBB\x07` sequences.
    fn set_color_scheme_notifications(
        &self,
        writer: &mut dyn Write,
        enabled: bool,
    ) -> io::Result<()>;
}

// =============================================================================
// Poll functions (kept as free functions for the event loop)
// =============================================================================

/// Poll for any terminal event (key, paste, resize).
/// Returns None on timeout or for unhandled event types.
pub fn poll_terminal_event(timeout: Option<Duration>) -> io::Result<Option<TerminalEvent>> {
    if event::poll(timeout.unwrap_or(Duration::ZERO))? {
        match event::read()? {
            Event::Key(key) => Ok(Some(TerminalEvent::Key(key))),
            Event::Paste(content) => Ok(Some(TerminalEvent::Paste(content))),
            Event::Resize(w, h) => Ok(Some(TerminalEvent::Resize(w, h))),
            _ => Ok(None),
        }
    } else {
        Ok(None)
    }
}

pub fn poll_key_event(timeout: Option<Duration>) -> io::Result<Option<KeyEvent>> {
    match poll_terminal_event(timeout)? {
        Some(TerminalEvent::Key(key)) => Ok(Some(key)),
        _ => Ok(None),
    }
}

pub fn read_key_event() -> io::Result<KeyEvent> {
    loop {
        match event::read()? {
            Event::Key(key) => return Ok(key),
            Event::Paste(_) => continue,
            Event::Resize(_, _) => continue,
            _ => continue,
        }
    }
}

// =============================================================================
// ProcessTerminal — crossterm-backed implementation
// =============================================================================

pub struct ProcessTerminal {
    was_raw: bool,
    kitty_active: bool,
}

impl ProcessTerminal {
    pub fn new() -> Self {
        Self {
            was_raw: false,
            kitty_active: false,
        }
    }

    fn enable_bracketed_paste(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[?2004h")?;
        writer.flush()
    }

    fn disable_bracketed_paste(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[?2004l")?;
        writer.flush()
    }

    fn enable_kitty_protocol(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS;
        write!(writer, "\x1b[>{}u", flags.bits())?;
        writer.flush()?;
        self.kitty_active = true;
        Ok(())
    }

    fn disable_kitty_protocol(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        if self.kitty_active {
            write!(writer, "\x1b[<u")?;
            writer.flush()?;
            self.kitty_active = false;
        }
        Ok(())
    }
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        if self.was_raw {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

impl TerminalTrait for ProcessTerminal {
    fn start(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        crossterm::terminal::enable_raw_mode()?;
        self.was_raw = true;
        self.enable_bracketed_paste(writer)?;
        self.enable_kitty_protocol(writer)?;
        // Refresh terminal dimensions
        let _ = crossterm::terminal::size();
        Ok(())
    }

    fn stop(&mut self, writer: &mut dyn Write) -> io::Result<()> {
        self.disable_kitty_protocol(writer)?;
        self.disable_bracketed_paste(writer)?;
        if self.was_raw {
            crossterm::terminal::disable_raw_mode()?;
            self.was_raw = false;
        }
        Ok(())
    }

    fn drain_input(&mut self, max_ms: u64) -> io::Result<()> {
        // Disable Kitty protocol so trailing release events don't leak
        let mut buf = Vec::new();
        self.disable_kitty_protocol(&mut buf)?;
        if !buf.is_empty() {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&buf)?;
            handle.flush()?;
        }

        let start = std::time::Instant::now();
        let mut last_data = start;
        loop {
            if start.elapsed().as_millis() as u64 >= max_ms {
                break;
            }
            if event::poll(Duration::from_millis(10))? {
                let _ = event::read()?;
                last_data = std::time::Instant::now();
            } else if last_data.elapsed().as_millis() > 50 {
                break;
            }
        }
        Ok(())
    }

    fn write(&self, writer: &mut dyn Write, data: &str) -> io::Result<()> {
        write!(writer, "{}", data)?;
        writer.flush()
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty_active
    }

    fn move_by(&self, writer: &mut dyn Write, lines: i32) -> io::Result<()> {
        if lines > 0 {
            write!(writer, "\x1b[{}B", lines)?;
        } else if lines < 0 {
            write!(writer, "\x1b[{}A", -lines)?;
        }
        writer.flush()
    }

    fn hide_cursor(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[?25l")?;
        writer.flush()
    }

    fn show_cursor(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[?25h")?;
        writer.flush()
    }

    fn clear_line(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[2K")?;
        writer.flush()
    }

    fn clear_from_cursor(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[J")?;
        writer.flush()
    }

    fn clear_screen(&self, writer: &mut dyn Write) -> io::Result<()> {
        write!(writer, "\x1b[2J\x1b[H")?;
        writer.flush()
    }

    fn set_title(&self, writer: &mut dyn Write, title: &str) -> io::Result<()> {
        write!(writer, "\x1b]0;{}\x07", title)?;
        writer.flush()
    }

    fn set_progress(&self, writer: &mut dyn Write, active: bool) -> io::Result<()> {
        if active {
            write!(writer, "\x1b]9;4;3\x07")?;
        } else {
            write!(writer, "\x1b]9;4;0;\x07")?;
        }
        writer.flush()
    }

    fn set_color_scheme_notifications(
        &self,
        writer: &mut dyn Write,
        enabled: bool,
    ) -> io::Result<()> {
        if enabled {
            write!(writer, "\x1b[?2031h")?;
        } else {
            write!(writer, "\x1b[?2031l")?;
        }
        writer.flush()
    }
}

// =============================================================================
// Legacy Terminal struct (backward compat) — uses crossterm execute! for Sized writers
// =============================================================================

use crossterm::{cursor, execute, terminal::ClearType};

pub struct Terminal {
    inner: ProcessTerminal,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            inner: ProcessTerminal::new(),
        }
    }

    pub fn enter_raw_mode(&mut self) -> io::Result<()> {
        let mut buf = Vec::new();
        self.inner.start(&mut buf)?;
        if !buf.is_empty() {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&buf)?;
            handle.flush()?;
        }
        Ok(())
    }

    pub fn leave_raw_mode(&mut self) -> io::Result<()> {
        let mut buf = Vec::new();
        self.inner.stop(&mut buf)?;
        if !buf.is_empty() {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(&buf)?;
            handle.flush()?;
        }
        Ok(())
    }

    pub fn show_cursor(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, cursor::Show)
    }

    pub fn hide_cursor(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, cursor::Hide)
    }

    pub fn move_cursor_to(writer: &mut impl Write, row: u16, col: u16) -> io::Result<()> {
        execute!(writer, cursor::MoveTo(col, row))
    }

    pub fn clear_line(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, crossterm::terminal::Clear(ClearType::CurrentLine))
    }

    pub fn clear_screen(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, crossterm::terminal::Clear(ClearType::All))
    }

    pub fn size() -> io::Result<(u16, u16)> {
        crossterm::terminal::size()
    }

    pub fn write(writer: &mut impl Write, data: &str) -> io::Result<()> {
        write!(writer, "{}", data)?;
        writer.flush()
    }

    pub fn begin_sync(writer: &mut impl Write) -> io::Result<()> {
        write!(writer, "\x1b[?2026h")?;
        writer.flush()
    }

    pub fn end_sync(writer: &mut impl Write) -> io::Result<()> {
        write!(writer, "\x1b[?2026l")?;
        writer.flush()
    }

    pub fn set_color_scheme_notifications(
        writer: &mut impl Write,
        enabled: bool,
    ) -> io::Result<()> {
        if enabled {
            write!(writer, "\x1b[?2031h")?;
        } else {
            write!(writer, "\x1b[?2031l")?;
        }
        writer.flush()
    }
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.inner.stop(&mut std::io::sink());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_terminal() {
        let term = ProcessTerminal::new();
        assert!(!term.kitty_protocol_active());
    }

    #[test]
    fn test_drain_input_timeout() {
        let mut term = ProcessTerminal::new();
        // drain_input may fail if no TTY is available in test env — that's ok
        let _ = term.drain_input(10);
    }
}
