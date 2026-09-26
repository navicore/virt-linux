//! VM supervision — QEMU process lifecycle shared by headless and GUI
//! modes (`virt start` / `virt install`).
//!
//! QEMU is spawned with inherited stdio in its own session, so
//! terminal-generated signals reach only the supervisor, which
//! translates them into QMP ACPI powerdown requests with a 10-second
//! grace window before SIGKILL — the same ladder as virt-macos's
//! VMInstance.

use crate::config::VmConfig;
use crate::logger;
use crate::qemu::arch::{Accel, Arch};
use crate::qemu::{Boot, Display, QemuSpec};
use crate::qmp::Qmp;
use crate::tty::TtyGuard;
use crate::vmdir::VmDir;
use anyhow::{Context, Result, bail};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by the signal handlers; read by the supervision loops.
pub(crate) static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// SIGINT (from `virt stop`, or Ctrl-C on a cooked tty), SIGTERM, and
/// SIGHUP (terminal closed) all route to a graceful guest shutdown.
pub(crate) fn install_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
        sa.sa_flags = 0;
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

pub(crate) fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Drop artifacts a previous run may have left behind. The lock file
/// is deliberately untouched — the caller holds it.
pub(crate) fn clean_stale_runtime_files(dir: &VmDir) {
    for path in [
        dir.qmp_socket(),
        dir.qga_socket(),
        dir.spice_socket(),
        dir.virtiofsd_socket(),
        dir.pid_path(),
    ] {
        let _ = std::fs::remove_file(path);
    }
}

/// Choose the boot mode: direct kernel when both files are present,
/// EFI otherwise. A single file is a broken import — warn and fall
/// back to EFI, exactly like virt-macos's Start.
pub(crate) fn resolve_boot(dir: &VmDir, config: &VmConfig) -> Result<Boot> {
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
    resolve_efi_boot(dir, Arch::host())
}

/// EFI boot for GUI installs (and the headless fallback): resolves
/// distro firmware and seeds per-VM NVRAM on first boot — the
/// virt-macos VZEFIVariableStore equivalent.
pub(crate) fn resolve_efi_boot(dir: &VmDir, arch: Arch) -> Result<Boot> {
    let firmware = arch.resolve_firmware().ok_or_else(|| {
        anyhow::anyhow!(
            "no UEFI firmware found for {arch} — run 'virt-linux doctor' for the package to install"
        )
    })?;
    if !dir.nvram_path().exists() {
        std::fs::copy(firmware.vars_template, dir.nvram_path())
            .with_context(|| format!("cannot seed NVRAM from {}", firmware.vars_template))?;
    }
    Ok(Boot::Efi {
        firmware_code: firmware.code.into(),
    })
}

/// Spawn QEMU: inherited stdio, own session, dies with the supervisor.
pub(crate) fn spawn_qemu(argv: &[String]) -> Result<Child> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // SAFETY: setsid/prctl only touch the child's own process state
    // post-fork, pre-exec — plain libc calls, no parent state.
    unsafe {
        cmd.pre_exec(|| {
            // Own session: terminal-generated signals reach only
            // the supervisor, which translates them to QMP.
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // Die with the supervisor however it dies — even SIGKILL —
            // so a force stop can never orphan a VM.
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
        .with_context(|| format!("cannot spawn {}", argv[0]))
}

/// The graceful-stop ladder: ACPI powerdown requests (re-pressed every
/// 2s — a press during early boot can be dropped before the guest's
/// ACPI handler is up, same reason virt-macos retries), then SIGKILL
/// once the 10-second grace window closes.
pub(crate) struct StopLadder {
    powerdown_at: Option<Instant>,
    deadline: Option<Instant>,
    killed: bool,
}

impl StopLadder {
    pub fn new() -> Self {
        Self {
            powerdown_at: None,
            deadline: None,
            killed: false,
        }
    }

    pub fn request(&mut self, dir: &VmDir) {
        if self.deadline.is_some() {
            return;
        }
        eprintln!("Shutdown requested, waiting up to 10 seconds...");
        logger::log(dir, "shutdown requested");
        self.deadline = Some(Instant::now() + Duration::from_secs(10));
    }

    pub fn tick(&mut self, dir: &VmDir, child: &mut Child) {
        let Some(deadline) = self.deadline else {
            return;
        };
        let due = self
            .powerdown_at
            .map(|at| at.elapsed() >= Duration::from_secs(2))
            .unwrap_or(true);
        if due && Instant::now() < deadline {
            if let Some(mut qmp) = connect_qmp_with_retry(dir) {
                if qmp.system_powerdown().is_ok() {
                    self.powerdown_at = Some(Instant::now());
                }
            }
        }
        if !self.killed && Instant::now() >= deadline {
            eprintln!("Force stopping VM...");
            logger::log(dir, "force stopping — guest did not shut down within 10s");
            child.kill().ok();
            self.killed = true;
        }
    }
}

/// Spawn the virtiofsd daemon for `--share` and wait for its socket.
/// QEMU connects to this socket as a vhost-user client, so the daemon
/// must be listening before QEMU starts. Dies with the supervisor
/// (PDEATHSIG SIGKILL, like QEMU).
pub(crate) fn spawn_virtiofsd(dir: &VmDir, share: &Path) -> Result<Child> {
    anyhow::ensure!(
        share.is_dir(),
        "Shared path is not a directory: {}",
        share.display()
    );
    let share = std::fs::canonicalize(share)
        .with_context(|| format!("cannot resolve {}", share.display()))?;
    let socket = dir.virtiofsd_socket();
    let _ = std::fs::remove_file(&socket);

    let mut cmd = Command::new("virtiofsd");
    cmd.arg("--socket-path")
        .arg(&socket)
        .arg("--shared-dir")
        .arg(&share)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    // SAFETY: prctl touches only the child's own process state.
    unsafe {
        cmd.pre_exec(|| {
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
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!(
                "virtiofsd not found — --share requires it \
                 (see 'virt-linux doctor' for the package)"
            );
        }
        Err(e) => return Err(e).context("cannot spawn virtiofsd"),
    };

    // The socket appearing means the daemon is ready to accept QEMU's
    // vhost-user connection. A child that dies first is a config error.
    for _ in 0..50 {
        if socket.exists() {
            return Ok(child);
        }
        if let Some(status) = child.try_wait()? {
            bail!("virtiofsd exited during startup (status {status})");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    bail!("virtiofsd did not create its socket within 5s");
}

/// Stop a helper child (virtiofsd, remote-viewer): SIGTERM, then
/// SIGKILL after a 2s grace.
pub(crate) fn dismiss_child(child: &mut Child) {
    let pid = child.id() as libc::pid_t;
    unsafe { libc::kill(pid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => return,
        }
    }
    child.kill().ok();
    let _ = child.wait();
}

/// The QMP socket appears milliseconds after the child spawns; retry
/// only while the socket does not yet exist (early boot). Once it
/// exists, a single attempt is made — bounded by the QMP read timeout,
/// so a wedged guest cannot stall the stop ladder.
pub(crate) fn connect_qmp_with_retry(dir: &VmDir) -> Option<Qmp> {
    for _ in 0..20 {
        if dir.qmp_socket().exists() {
            return Qmp::connect(&dir.qmp_socket()).ok();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Teardown common to every supervision mode.
pub(crate) fn finish_session(dir: &VmDir, exit: Option<i32>) {
    let _ = std::fs::remove_file(dir.pid_path());
    // QEMU unlinks its server sockets on clean exit, but not when
    // SIGKILLed — tidy them here so the VM dir reflects a stopped VM.
    for sock in [
        dir.qmp_socket(),
        dir.qga_socket(),
        dir.spice_socket(),
        dir.virtiofsd_socket(),
    ] {
        let _ = std::fs::remove_file(sock);
    }
    logger::log(dir, &format!("guest stopped (exit {})", exit.unwrap_or(-1)));
    logger::log(dir, "session ended");
}

/// Headless supervision (`virt start`).
pub fn run_headless(config: &VmConfig, dir: &VmDir, share: Option<&Path>) -> Result<()> {
    let _tty = TtyGuard::capture();
    clean_stale_runtime_files(dir);

    let mut vfsd = match share {
        Some(share) => Some(spawn_virtiofsd(dir, share)?),
        None => None,
    };

    let boot = resolve_boot(dir, config)?;
    let spec = QemuSpec {
        config,
        dir,
        arch: Arch::host(),
        accel: Accel::detect(),
        boot,
        display: Display::Headless,
        iso: None,
        share,
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

    let mut child = spawn_qemu(&argv)?;

    install_signal_handlers();
    eprintln!(
        "VM running. Console output appears once the guest boots.\n\
         Use 'virt-linux stop {}' (another terminal) to shut down.",
        config.name
    );
    logger::log(dir, "started");

    let mut ladder = StopLadder::new();
    let loop_result = loop {
        if let Some(status) = child.try_wait()? {
            eprintln!("VM stopped.");
            finish_session(dir, status.code());
            break Ok(());
        }
        if shutdown_requested() {
            ladder.request(dir);
        }
        ladder.tick(dir, &mut child);
        std::thread::sleep(Duration::from_millis(100));
    };
    if let Some(vfsd) = vfsd.as_mut() {
        dismiss_child(vfsd);
    }
    loop_result
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
