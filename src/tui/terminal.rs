use std::io::{self, Write};
use std::sync::LazyLock;
use std::sync::Mutex;
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
        while STDIN_RUNNING.load(Ordering::SeqCst) {
            match event::poll(std::time::Duration::from_millis(100)) {
                Ok(true) => match event::read() {
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
                        break;
                    }
                },
                Ok(false) => {}
                Err(_) => {
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
/// Non-blocking read from the background stdin reader channel.
pub fn try_recv_terminal_event() -> Option<TerminalEvent> {
    use std::sync::mpsc::TryRecvError;
    // Lock the mutex for the entire operation so the receiver is never
    // removed while other code (e.g. stop_stdin_reader) might access it.
    let mut guard = EVENT_RX.lock().unwrap();
    let rx = guard.as_ref()?;
    match rx.try_recv() {
        Ok(event) => Some(event),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            // Channel disconnected — stdin reader thread exited.
            // Drop the receiver so a new channel can be created.
            *guard = None;
            None
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
        if self.kitty_active {
            return Ok(());
        }
        // Push flags: DISAMBIGUATE_ESCAPE_CODES (1) | REPORT_EVENT_TYPES (2) | REPORT_ALTERNATE_KEYS (4)
        // This enables:
        //   - Disambiguate escape codes (all keys use CSI-u format)
        //   - Report event types (press/repeat/release)
        //   - Report alternate keys (shifted key according to keyboard layout)
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
            // Pop one level of keyboard enhancement flags
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
        // Pop Kitty keyboard protocol so trailing release events don't leak
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
