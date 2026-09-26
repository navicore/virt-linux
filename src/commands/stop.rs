//! `virt stop` — gracefully stop a running VM from another terminal.
//!
//! The flock is authoritative: a held lock always means a live
//! supervisor. The PID file only tells us *where* to send the signal,
//! and is only trusted once we know the lock is held. SIGINT puts the
//! supervisor into its ACPI-powerdown-then-kill ladder; if even that
//! doesn't finish in time, we SIGKILL the supervisor (PDEATHSIG
//! cascades to QEMU) and repair the terminal it left in raw mode.

use crate::cli::Stop;
use crate::lock::VmLock;
use crate::logger;
use crate::tty;
use crate::vmdir::VmDir;
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

pub fn run(args: &Stop) -> Result<()> {
    let dir = VmDir::new(&args.name);

    if !dir.exists() {
        bail!("VM '{}' does not exist.", args.name);
    }

    if !VmLock::is_locked(&dir) {
        if dir.pid_path().exists() {
            let _ = std::fs::remove_file(dir.pid_path());
            bail!(
                "VM '{}' is not running (stale PID file removed).",
                args.name
            );
        }
        bail!("VM '{}' is not running.", args.name);
    }

    let pid = read_pid(&dir)?;

    println!(
        "Sending shutdown signal to VM '{}' (PID {pid})...",
        args.name
    );
    logger::log(&dir, &format!("virt stop: SIGINT -> pid {pid}"));

    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if let Some(libc::ESRCH) = err.raw_os_error() {
            println!("VM '{}' stopped.", args.name);
            return Ok(());
        }
        bail!("Failed to signal PID {pid}: {err}");
    }

    if wait_for_exit(pid, Duration::from_secs(15)) {
        println!("VM '{}' stopped.", args.name);
        logger::log(&dir, "stopped gracefully");
        return Ok(());
    }

    force_kill(&dir, &args.name, pid)
}

fn read_pid(dir: &VmDir) -> Result<u32> {
    dir.pid().ok_or_else(|| {
        anyhow::anyhow!(
            "VM '{}' is running but its PID file is missing or corrupt. Find it with: ps aux | grep virt",
            dir.name
        )
    })
}

fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn force_kill(dir: &VmDir, name: &str, pid: u32) -> Result<()> {
    println!("VM did not stop gracefully, force killing...");
    logger::log(dir, &format!("virt stop: SIGKILL -> pid {pid}"));

    // The killed supervisor can't restore its terminal from raw mode;
    // capture the tty first and repair it after.
    let tty_path = tty::tty_of_pid(pid);

    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    std::thread::sleep(Duration::from_secs(1));

    if let Some(tty_path) = &tty_path {
        if tty::restore_sane(tty_path) {
            eprintln!("Terminal restored (the VM held it in raw mode when killed).");
        }
    }

    if process_alive(pid) {
        bail!("Failed to stop VM '{name}' (PID {pid}).");
    }

    let _ = std::fs::remove_file(dir.pid_path());
    println!("VM '{name}' killed.");
    logger::log(dir, "force killed by virt stop");
    Ok(())
}
