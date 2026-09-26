//! Headless VM supervisor — `virt start`'s runtime.
//!
//! Spawns QEMU with inherited stdio (the guest serial console IS this
//! terminal's byte stream) in its own session, then translates
//! termination signals into QMP ACPI powerdown requests with a
//! 10-second grace window before SIGKILL — the same ladder as
//! virt-macos's VMInstance.

use crate::config::VmConfig;
use crate::logger;
use crate::qemu::arch::{Accel, Arch};
use crate::qemu::{Boot, Display, QemuSpec};
use crate::qmp::Qmp;
use crate::tty::TtyGuard;
use crate::vmdir::VmDir;
use anyhow::{Context, Result};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the signal handlers; read by the supervision loop.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// SIGINT (from `virt stop` or Ctrl-C on a cooked tty), SIGTERM, and
/// SIGHUP (terminal closed) all route to a graceful guest shutdown.
fn install_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        sa.sa_flags = 0;
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// Drop artifacts a previous run may have left behind. The lock file
/// is deliberately untouched — we hold it.
fn clean_stale_runtime_files(dir: &VmDir) {
    for path in [
        dir.qmp_socket(),
        dir.qga_socket(),
        dir.spice_socket(),
        dir.pid_path(),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// Choose the boot mode: direct kernel when both files are present,
/// EFI otherwise. A single file is a broken import — warn and fall
/// back to EFI, exactly like virt-macos's Start.
fn resolve_boot(dir: &VmDir, config: &VmConfig) -> Result<Boot> {
    let kernel = dir.kernel_path();
    let initrd = dir.initrd_path();
    if kernel.exists() && initrd.exists() {
        return Ok(Boot::Kernel {
            kernel,
            initrd,
            root_device: config
                .root_device
                .clone()
                .unwrap_or_else(|| "/dev/vda2".into()),
            extra_args: config.extra_kernel_args.clone(),
        });
    }
    if kernel.exists() != initrd.exists() {
        eprintln!("  warning: kernel and initrd must both be present; falling back to EFI.");
    }
    let arch = Arch::host();
    let firmware = arch.resolve_firmware().ok_or_else(|| {
        anyhow::anyhow!(
            "no UEFI firmware found for {arch} — run 'virt-linux doctor' for the package to install"
        )
    })?;
    // First EFI boot: give the VM its own variable store from the
    // distro template (the virt-macos VZEFIVariableStore equivalent).
    if !dir.nvram_path().exists() {
        std::fs::copy(firmware.vars_template, dir.nvram_path())
            .with_context(|| format!("cannot seed NVRAM from {}", firmware.vars_template))?;
    }
    Ok(Boot::Efi {
        firmware_code: firmware.code.into(),
    })
}

pub fn run_headless(config: &VmConfig, dir: &VmDir) -> Result<()> {
    let _tty = TtyGuard::capture();
    clean_stale_runtime_files(dir);

    let boot = resolve_boot(dir, config)?;
    let spec = QemuSpec {
        config,
        dir,
        arch: Arch::host(),
        accel: Accel::detect(),
        boot,
        display: Display::Headless,
        iso: None,
    };
    let argv = spec.argv();

    logger::log(
        dir,
        &format!(
            "starting headless (cpus={}, memory={} MB, accel={})",
            config.cpus,
            config.memory_mb,
            if matches!(spec.accel, Accel::Kvm) {
                "kvm"
            } else {
                "tcg"
            }
        ),
    );

    // The PID file records the SUPERVISOR (this process), matching
    // virt-macos: `virt stop` signals it, and the flock it holds is
    // the authoritative running-state marker.
    std::fs::write(dir.pid_path(), std::process::id().to_string())
        .context("cannot write PID file")?;

    let mut child = {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        // SAFETY: setsid/prctl only touch the child's own process
        // state post-fork, pre-exec; nothing here touches parent
        // memory that isn't send/sync-safe plain libc calls.
        unsafe {
            cmd.pre_exec(|| {
                // Own session: terminal-generated signals reach only
                // the supervisor, which translates them to QMP.
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // Die with the supervisor however it dies — even
                // SIGKILL — so a force stop can never orphan a VM.
                libc::prctl(
                    libc::PR_SET_PDEATHSIG,
                    libc::SIGKILL as libc::c_ulong,
                    0,
                    0,
                    0,
                );
                Ok(())
            });
        }
        cmd.spawn()
            .with_context(|| format!("cannot spawn {}", argv[0]))?
    };

    install_signal_handlers();
    eprintln!(
        "VM running. Console output appears once the guest boots.\n\
         Use 'virt-linux stop {}' (another terminal) to shut down.",
        config.name
    );
    logger::log(dir, "started");

    let mut powerdown_sent_at: Option<Instant> = None;
    let mut deadline: Option<Instant> = None;
    let mut killed = false;

    loop {
        if let Some(status) = child.try_wait()? {
            logger::log(
                dir,
                &format!("guest stopped (exit {})", status.code().unwrap_or(-1)),
            );
            eprintln!("VM stopped.");
            break;
        }

        if SHUTDOWN.load(Ordering::SeqCst) && deadline.is_none() {
            eprintln!("Shutdown requested, waiting up to 10 seconds...");
            logger::log(dir, "shutdown requested");
            deadline = Some(Instant::now() + Duration::from_secs(10));
        }

        if let Some(deadline) = deadline {
            // Re-press the ACPI power button every 2s: a press during
            // early boot can be dropped before the guest's ACPI
            // handler is up (virt-macos retries for the same reason).
            let due = powerdown_sent_at
                .map(|at| at.elapsed() >= Duration::from_secs(2))
                .unwrap_or(true);
            if due && Instant::now() < deadline {
                if let Some(mut qmp) = connect_qmp_with_retry(dir) {
                    if qmp.system_powerdown().is_ok() {
                        powerdown_sent_at = Some(Instant::now());
                    }
                }
            }
            if !killed && Instant::now() >= deadline {
                eprintln!("Force stopping VM...");
                logger::log(dir, "force stopping — guest did not shut down within 10s");
                child.kill().ok();
                killed = true;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = std::fs::remove_file(dir.pid_path());
    // QEMU unlinks its server sockets on clean exit, but not when
    // SIGKILLed — tidy them here so the VM dir reflects a stopped VM.
    for sock in [dir.qmp_socket(), dir.qga_socket(), dir.spice_socket()] {
        let _ = std::fs::remove_file(sock);
    }
    logger::log(dir, "session ended");
    Ok(())
}

/// The QMP socket appears milliseconds after the child spawns; retry
/// briefly so a very early stop request isn't silently lost.
fn connect_qmp_with_retry(dir: &VmDir) -> Option<Qmp> {
    for _ in 0..20 {
        if let Ok(qmp) = Qmp::connect(&dir.qmp_socket()) {
            return Some(qmp);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Direct kernel boot marker for `virt start`'s banner (mirrors
/// Start.swift's messaging).
pub fn boot_banner(dir: &VmDir) -> String {
    if dir.kernel_path().exists() && dir.initrd_path().exists() {
        "direct kernel (console=ttyS0)".to_string()
    } else if dir.kernel_path().exists() != dir.initrd_path().exists() {
        "EFI/GRUB (partial kernel import — fell back)".to_string()
    } else {
        "EFI/GRUB (silent until the guest configures console=ttyS0)".to_string()
    }
}
