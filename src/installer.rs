//! GUI supervision (`virt install`) — SPICE display + remote-viewer.
//!
//! The virt-macos InstallerApp semantics, translated: closing the
//! viewer window is vetted into a graceful guest shutdown (closing
//! mid-write is a power cut); when the guest stops on its own (OS
//! install finished, in-guest poweroff), the session ends and the
//! viewer is dismissed.

use crate::config::VmConfig;
use crate::logger;
use crate::qemu::arch::{Accel, Arch};
use crate::qemu::{Display, QemuSpec};
use crate::supervisor::{
    StopLadder, clean_stale_runtime_files, dismiss_child, finish_session, install_signal_handlers,
    resolve_efi_boot, shutdown_requested, spawn_qemu, spawn_virtiofsd,
};
use crate::vmdir::VmDir;
use anyhow::{Context, Result, bail};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

pub fn run_gui(
    config: &VmConfig,
    dir: &VmDir,
    iso: Option<&Path>,
    viewer: bool,
    share: Option<&Path>,
) -> Result<()> {
    clean_stale_runtime_files(dir);

    let mut vfsd = match share {
        Some(share) => Some(spawn_virtiofsd(dir, share)?),
        None => None,
    };

    // Installs (and GUI re-entry) always use real EFI — installers
    // and GRUB need firmware, mirroring virt-macos's Install.
    let boot = resolve_efi_boot(dir, Arch::host())?;
    let spec = QemuSpec {
        config,
        dir,
        arch: Arch::host(),
        accel: Accel::detect(),
        boot,
        display: Display::Spice,
        iso,
        share,
    };
    let argv = spec.argv();

    logger::log(
        dir,
        &format!(
            "starting gui (iso={}, accel={})",
            iso.map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".into()),
            if matches!(spec.accel, Accel::Kvm) {
                "kvm"
            } else {
                "tcg"
            }
        ),
    );
    std::fs::write(dir.pid_path(), std::process::id().to_string())
        .context("cannot write PID file")?;

    let mut child = spawn_qemu(&argv)?;
    install_signal_handlers();
    logger::log(dir, "started");

    let uri = format!("spice+unix://{}", dir.spice_socket().display());
    let mut viewer_child = if viewer {
        Some(spawn_viewer(&uri)?)
    } else {
        eprintln!("  SPICE: {uri}");
        eprintln!("  (connect manually: remote-viewer '{uri}')");
        None
    };

    let mut ladder = StopLadder::new();
    let mut viewer_exit_noted = false;
    let loop_result = loop {
        if let Some(status) = child.try_wait()? {
            eprintln!("VM stopped.");
            dismiss_viewer(viewer_child.as_mut());
            finish_session(dir, status.code());
            break Ok(());
        }
        // Window closed → request graceful shutdown (InstallerApp's
        // windowShouldClose veto). A crashed viewer is
        // indistinguishable and lands here too — acceptable: the VM
        // is an install context, not a long-running service.
        if let Some(vc) = viewer_child.as_mut() {
            if vc.try_wait()?.is_some() && !viewer_exit_noted {
                viewer_exit_noted = true;
                eprintln!("Viewer window closed — requesting guest shutdown...");
                logger::log(dir, "viewer closed; shutdown requested");
            }
        }
        if shutdown_requested() || viewer_exit_noted {
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

/// Spawn remote-viewer against the VM's SPICE unix socket. It must die
/// with the supervisor (PDEATHSIG SIGTERM lets GTK clean up) but NOT
/// be setsid'd — it needs the user's display session.
fn spawn_viewer(uri: &str) -> Result<Child> {
    let mut cmd = Command::new("remote-viewer");
    cmd.arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // SAFETY: prctl touches only the child's own process state.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(
                libc::PR_SET_PDEATHSIG,
                libc::SIGTERM as libc::c_ulong,
                0,
                0,
                0,
            );
            Ok(())
        });
    }
    match cmd.spawn() {
        Ok(child) => Ok(child),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                bail!(
                    "remote-viewer not found — install virt-viewer, \
                     or pass --no-viewer and connect manually"
                );
            }
            Err(e).context("cannot spawn remote-viewer")
        }
    }
}

/// The guest stopped on its own (install finished, in-guest poweroff):
/// dismiss the viewer window.
fn dismiss_viewer(viewer: Option<&mut Child>) {
    if let Some(viewer) = viewer {
        dismiss_child(viewer);
    }
}
