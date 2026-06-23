use std::sync::atomic::{AtomicBool, Ordering};

static BUSY: AtomicBool = AtomicBool::new(false);

/// Write all bytes to fd, retrying on short write (async-signal-safe).
unsafe fn write_all(fd: libc::c_int, buf: &[u8]) {
    let mut off = 0;
    while off < buf.len() {
        // SAFETY: raw libc write is async-signal-safe
        let n = unsafe {
            libc::write(
                fd,
                buf[off..].as_ptr() as *const libc::c_void,
                buf.len() - off,
            )
        };
        if n <= 0 {
            break;
        }
        off += n as usize;
    }
}

extern "C" fn handler(_sig: libc::c_int) {
    if BUSY.swap(true, Ordering::Relaxed) {
        return;
    }

    let path = c"/tmp/rab-freeze.txt";
    // SAFETY: open is async-signal-safe for creating dump files
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
            0o644,
        )
    };
    if fd < 0 {
        BUSY.store(false, Ordering::Relaxed);
        return;
    }

    // SAFETY: write_all uses only async-signal-safe libc::write
    unsafe {
        write_all(fd, b"=== rab freeze dump ===\n");
    }
    unsafe {
        write_all(fd, b"--- Backtrace (stuck thread) ---\n");
    }

    // Capture backtrace using backtrace crate (designed for signal handlers).
    backtrace::trace(|frame| {
        let ip = frame.ip();
        let mut printed = false;
        backtrace::resolve(ip, |symbol| {
            if printed {
                return;
            }
            printed = true;
            let name = match symbol.name() {
                Some(n) => n.to_string(),
                None => String::new(),
            };
            let file = match symbol
                .filename()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
            {
                Some(s) => s.to_string(),
                None => String::new(),
            };
            let line = symbol.lineno().unwrap_or(0);
            let msg = format!("  {name}  ({file}:{line})\n");
            // SAFETY: backtrace crate guarantees async-signal-safety for resolve
            unsafe {
                write_all(fd, msg.as_bytes());
            }
        });
        if !printed {
            let msg = format!("  <unknown>  ({ip:?})\n");
            // SAFETY: backtrace crate guarantees async-signal-safety
            unsafe {
                write_all(fd, msg.as_bytes());
            }
        }
        true
    });

    // SAFETY: close is async-signal-safe
    unsafe {
        write_all(fd, b"\n--- end ---\n");
    }
    unsafe {
        libc::close(fd);
    }
    BUSY.store(false, Ordering::Relaxed);
}

/// Initialize freeze dump handlers. Call once at program startup.
pub fn init() {
    let _ = std::fs::write(
        "/tmp/rab-freeze.txt",
        "rab freeze dump handler ready — send `pkill -USR1 rab` to dump\n",
    );
    // SAFETY: signal registration is only done once at startup, no race
    unsafe {
        let ptr = handler as *const () as usize;
        libc::signal(libc::SIGUSR1, ptr as libc::sighandler_t);
        libc::signal(libc::SIGQUIT, ptr as libc::sighandler_t);
    }
}
