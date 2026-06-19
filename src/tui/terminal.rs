use crossterm::{
    cursor,
    event::{self, Event, KeyEvent},
    execute,
    terminal::{self, ClearType},
};
use std::io::{self, Write};
use std::time::Duration;

/// Terminal wrapper providing raw mode, event polling, and cursor control.
pub struct Terminal {
    prev_raw_mode: bool,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            prev_raw_mode: false,
        }
    }

    /// Enter raw mode and start input event channel.
    pub fn enter_raw_mode(&mut self) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        self.prev_raw_mode = true;
        Ok(())
    }

    /// Leave raw mode and restore terminal.
    pub fn leave_raw_mode(&mut self) -> io::Result<()> {
        if self.prev_raw_mode {
            terminal::disable_raw_mode()?;
            self.prev_raw_mode = false;
        }
        Ok(())
    }

    /// Show the terminal cursor.
    pub fn show_cursor(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, cursor::Show)
    }

    /// Hide the terminal cursor.
    pub fn hide_cursor(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, cursor::Hide)
    }

    /// Move cursor to a specific position (1-based row, 1-based col).
    pub fn move_cursor_to(writer: &mut impl Write, row: u16, col: u16) -> io::Result<()> {
        execute!(writer, cursor::MoveTo(col, row))
    }

    /// Clear the current line.
    pub fn clear_line(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, terminal::Clear(ClearType::CurrentLine))
    }

    /// Clear the entire screen.
    pub fn clear_screen(writer: &mut impl Write) -> io::Result<()> {
        execute!(writer, terminal::Clear(ClearType::All))
    }

    /// Get the current terminal size.
    pub fn size() -> io::Result<(u16, u16)> {
        terminal::size()
    }

    /// Write raw bytes to stdout.
    pub fn write(writer: &mut impl Write, data: &str) -> io::Result<()> {
        write!(writer, "{}", data)?;
        writer.flush()
    }

    /// Begin synchronized output mode.
    pub fn begin_sync(writer: &mut impl Write) -> io::Result<()> {
        write!(writer, "\x1b[?2026h")?;
        writer.flush()
    }

    /// End synchronized output mode.
    pub fn end_sync(writer: &mut impl Write) -> io::Result<()> {
        write!(writer, "\x1b[?2026l")?;
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
        let _ = self.leave_raw_mode();
    }
}

/// Poll for a keyboard event with an optional timeout.
/// Returns `None` if timeout expires, `Some(KeyEvent)` if a key is pressed.
pub fn poll_key_event(timeout: Option<Duration>) -> io::Result<Option<KeyEvent>> {
    if event::poll(timeout.unwrap_or(Duration::from_secs(0)))? {
        match event::read()? {
            Event::Key(key) => Ok(Some(key)),
            Event::Resize(_, _) => {
                // Resize events are consumed but we return None
                // The caller should check terminal size separately
                Ok(None)
            }
            _ => Ok(None),
        }
    } else {
        Ok(None)
    }
}

/// Block waiting for a keyboard event.
pub fn read_key_event() -> io::Result<KeyEvent> {
    loop {
        match event::read()? {
            Event::Key(key) => return Ok(key),
            _ => continue,
        }
    }
}
