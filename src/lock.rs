//! Race-free mutual exclusion for a running VM.
//!
//! A running `virt start` holds an exclusive flock on `vm.lock` for its
//! entire lifetime. The kernel releases the lock when the process dies
//! (even on SIGKILL), so a held lock always means a live VM — unlike a
//! PID file, which can go stale or point at a recycled PID.

use crate::vmdir::VmDir;
use anyhow::{Context, Result};
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

pub struct VmLock {
    /// Dropping the File closes the fd, which releases the flock.
    _file: File,
}

impl VmLock {
    /// Acquire the exclusive lock for a VM, or error if another virt
    /// process already holds it.
    pub fn acquire(dir: &VmDir) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // The lock file is a persistent marker, not scratch data —
            // never truncate (a stale holder's bytes may be examined).
            .truncate(false)
            .open(dir.lock_path())
            .with_context(|| format!("cannot open {}", dir.lock_path().display()))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let pid = dir.pid().map(|p| format!(" (PID {p})")).unwrap_or_default();
            anyhow::bail!("VM '{}' is already running{}.", dir.name, pid);
        }
        Ok(Self { _file: file })
    }

    /// True if some process currently holds the VM's lock.
    pub fn is_locked(dir: &VmDir) -> bool {
        let Ok(file) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.lock_path())
        else {
            return false;
        };
        let fd = file.as_raw_fd();
        if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return true;
        }
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        false
    }
}
