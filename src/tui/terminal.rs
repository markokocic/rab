use std::io::{self, Write};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEvent};

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
// Background stdin reader — reads crossterm events on a dedicated thread
// and forwards them through a channel so crossterm parser bugs (e.g. hanging
// on partial escape sequences) cannot freeze the main event loop.
// =============================================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

static EVENT_TX: LazyLock<Mutex<Option<mpsc::Sender<TerminalEvent>>>> =
    LazyLock::new(|| Mutex::new(None));

static EVENT_RX: LazyLock<Mutex<Option<mpsc::Receiver<TerminalEvent>>>> =
    LazyLock::new(|| Mutex::new(None));

static STDIN_THREAD_HANDLE: LazyLock<Mutex<Option<std::thread::JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(None));

static STDIN_RUNNING: AtomicBool = AtomicBool::new(false);

/// Start the background stdin reader thread.
/// Must be called once after raw mode is enabled.
pub fn start_stdin_reader() {
    let (tx, rx) = mpsc::channel();
    *EVENT_TX.lock().unwrap() = Some(tx.clone());
    *EVENT_RX.lock().unwrap() = Some(rx);
    STDIN_RUNNING.store(true, Ordering::SeqCst);

    let handle = std::thread::spawn(move || {
        // Use poll() with a timeout so we can check the stop flag regularly.
        // 100ms interval is short enough for responsive shutdown but long
        // enough to not waste CPU.
        while STDIN_RUNNING.load(Ordering::SeqCst) {
            match event::poll(std::time::Duration::from_millis(100)) {
                Ok(true) => {
                    // Data available — read it
                    match event::read() {
                        Ok(Event::Key(key)) => {
                            let _ = tx.send(TerminalEvent::Key(key));
                        }
                        Ok(Event::Paste(content)) => {
                            let _ = tx.send(TerminalEvent::Paste(content));
                        }
                        Ok(Event::Resize(w, h)) => {
                            let _ = tx.send(TerminalEvent::Resize(w, h));
                        }
                        Ok(_) => {}
                        Err(_) => {
                            // Stdin error — terminal likely closed. Exit thread.
                            break;
                        }
                    }
                }
                Ok(false) => {
                    // Timeout — loop back and check stop flag
                }
                Err(_) => {
                    // poll error — exit
                    break;
                }
            }
        }
    });

    *STDIN_THREAD_HANDLE.lock().unwrap() = Some(handle);
}

/// Initiate stopping the background stdin reader thread.
/// Sets the running flag and drops sender/receiver.
/// The thread will exit on the next event::read() that completes after
/// the flag is cleared. Call `join_stdin_reader()` to wait for exit.
pub fn stop_stdin_reader() {
    STDIN_RUNNING.store(false, Ordering::SeqCst);
    let mut guard = EVENT_TX.lock().unwrap();
    *guard = None;
    drop(guard);
    let mut rx_guard = EVENT_RX.lock().unwrap();
    *rx_guard = None;
}

/// Wait for the stdin reader thread to exit.
pub fn join_stdin_reader() {
    let mut guard = STDIN_THREAD_HANDLE.lock().unwrap();
    if let Some(handle) = guard.take() {
        let _ = handle.join();
    }
}

/// Drain all pending events from the stdin reader channel.
pub fn drain_stdin_events() {
    let rx_guard = EVENT_RX.lock().unwrap();
    if let Some(rx) = rx_guard.as_ref() {
        while rx.try_recv().is_ok() {}
    }
}

/// Poll for any terminal event (key, paste, resize) with a timeout.
/// Blocks the calling thread until either an event arrives or the timeout
/// expires. Uses `recv_timeout` internally so zero CPU is consumed while
/// waiting.
///
/// The receiver is taken out of the mutex before the blocking call so
/// `EVENT_RX` is never held during the wait — other code (e.g.
/// `stop_stdin_reader`) can always lock it without contention.
pub fn poll_terminal_event(timeout: Option<Duration>) -> io::Result<Option<TerminalEvent>> {
    use mpsc::RecvTimeoutError;
    // Take the receiver out so we can block without holding EVENT_RX.
    let rx_opt = EVENT_RX.lock().unwrap().take();
    let rx = match rx_opt.as_ref() {
        Some(rx) => rx,
        None => return Ok(None),
    };
    let (event, keep) = match timeout {
        Some(dur) => match rx.recv_timeout(dur) {
            Ok(event) => (Some(event), true),
            Err(RecvTimeoutError::Timeout) => (None, true),
            Err(RecvTimeoutError::Disconnected) => (None, false),
        },
        None => match rx.recv() {
            Ok(event) => (Some(event), true),
            Err(_) => (None, false),
        },
    };
    // Drop the reference so we can move rx_opt
    let _ = rx;
    if keep {
        // Channel still alive — put receiver back
        *EVENT_RX.lock().unwrap() = rx_opt;
    }
    Ok(event)
}

pub fn poll_key_event(timeout: Option<Duration>) -> io::Result<Option<KeyEvent>> {
    match poll_terminal_event(timeout)? {
        Some(TerminalEvent::Key(key)) => Ok(Some(key)),
        _ => Ok(None),
    }
}

pub fn read_key_event() -> io::Result<KeyEvent> {
    loop {
        let rx_guard = EVENT_RX.lock().unwrap();
        if let Some(rx) = rx_guard.as_ref() {
            match rx.recv() {
                Ok(TerminalEvent::Key(key)) => return Ok(key),
                Ok(_) => continue,
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "stdin reader died",
                    ));
                }
            }
        }
        drop(rx_guard);
        std::thread::sleep(Duration::from_millis(10));
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
        // Kitty keyboard protocol disabled — it can cause crossterm's event
        // parser to hang on partial escape sequences, freezing the main loop.
        // self.enable_kitty_protocol(writer)?;
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
