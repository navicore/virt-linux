//! Terminal guard for the headless console.
//!
//! QEMU's `-serial stdio` puts the tty in raw mode and restores it on
//! clean exit — so the supervisor only *saves* the original termios
//! and restores it on the way out. That covers the paths QEMU cannot:
//! SIGKILL of the child (force stop) and supervisor death mid-run.
//! The supervisor deliberately does not set raw mode itself; if it
//! did, QEMU would save the already-raw state and "restore" raw mode
//! on exit, wedging the terminal.

use std::fs::File;
use std::process::{Command, Stdio};

pub struct TtyGuard {
    saved: Option<libc::termios>,
}

impl TtyGuard {
    /// Save fd 0's termios if it is a terminal.
    pub fn capture() -> Self {
        if unsafe { libc::isatty(0) } != 1 {
            return Self { saved: None };
        }
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::tcgetattr(0, &mut termios) };
        Self {
            saved: if rc == 0 { Some(termios) } else { None },
        }
    }
}

impl Drop for TtyGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved {
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        }
    }
}

/// The tty device a process's stdin is attached to, via /proc —
/// used by `virt stop` to repair a terminal left raw by a force
/// kill. (virt-macos shells out to `ps -o tty=`; /proc is the
/// Linux-native equivalent.)
pub fn tty_of_pid(pid: u32) -> Option<String> {
    let link = format!("/proc/{pid}/fd/0");
    let target = std::fs::read_link(link).ok()?;
    let target = target.to_string_lossy().into_owned();
    if target.starts_with("/dev/") {
        Some(target)
    } else {
        None
    }
}

/// `stty sane` on a tty owned by another process.
pub fn restore_sane(tty_path: &str) -> bool {
    let Ok(input) = File::open(tty_path) else {
        return false;
    };
    Command::new("stty")
        .arg("sane")
        .stdin(Stdio::from(input))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_is_harmless_without_tty() {
        // Under the test harness stdin may or may not be a tty; both
        // paths must construct and drop without incident.
        let guard = TtyGuard::capture();
        assert_eq!(guard.saved.is_some(), unsafe { libc::isatty(0) } == 1);
    }

    #[test]
    fn tty_of_pid_resolves_self_or_none() {
        // fd 0 of this test process is a pipe or tty; /proc resolution
        // must not error either way.
        let _ = tty_of_pid(std::process::id());
    }
}
