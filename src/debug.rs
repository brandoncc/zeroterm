//! Debug logging module for Zeroterm
//!
//! Writes timestamped debug logs to ~/.config/zeroterm/debug.log when enabled.
//! Since the TUI uses raw terminal mode, we log to a file instead of stderr.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

use crate::config;

static DEBUG_STATE: OnceLock<Mutex<DebugState>> = OnceLock::new();

struct DebugState {
    enabled: bool,
    file: Option<File>,
    start_time: Instant,
}

/// Initializes the debug logging system.
/// Call this once at startup with the debug flag state.
pub fn init(enabled: bool) {
    let file = if enabled {
        config::config_dir()
            .ok()
            .and_then(|dir| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("debug.log"))
                    .ok()
            })
            .map(|mut f| {
                // Write a separator for this session
                let _ = writeln!(
                    f,
                    "\n========== Session started at {} ==========",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                );
                f
            })
    } else {
        None
    };

    let _ = DEBUG_STATE.set(Mutex::new(DebugState {
        enabled,
        file,
        start_time: Instant::now(),
    }));
}

/// Logs a debug message with timestamp.
/// Does nothing if debug mode is not enabled.
pub fn log(message: &str) {
    if let Some(state) = DEBUG_STATE.get()
        && let Ok(mut guard) = state.lock()
        && guard.enabled
    {
        let elapsed = guard.start_time.elapsed();
        if let Some(ref mut file) = guard.file {
            let _ = writeln!(file, "[{:>8.3}s] {}", elapsed.as_secs_f64(), message);
            let _ = file.flush();
        }
    }
}

/// Logs a formatted debug message.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        $crate::debug::log(&format!($($arg)*))
    };
}
