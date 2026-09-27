//! `virt console-enable` — give an installed guest a serial console
//! by editing its bootloader configuration offline, from the host.
//!
//! Installers don't persist serial-console arguments when the install
//! ran graphically (anaconda saw a VGA primary console and wrote
//! default kernel args). This closes that gap without any guest-side
//! steps: libguestfs boots its own tiny appliance around the stopped
//! VM's disk and runs the distro's own tooling (`grubby`) inside it —
//! no root on the host, no daemon, XFS/ext4 alike.
//!
//! Run automatically once on the first `virt start` of an installed
//! VM (see `maybe_auto_enable`); also available by hand.

use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::logger;
use crate::vmdir::VmDir;
use anyhow::{Context, Result, bail};
use std::process::Command;

pub fn run(name: &str) -> Result<()> {
    let dir = VmDir::new(name);
    if !dir.exists() {
        bail!("VM '{name}' does not exist.");
    }
    if VmLock::is_locked(&dir) {
        bail!("VM '{name}' is running — stop it first (the disk must be offline).");
    }
    let config = VmConfig::load(&dir.config_path())?;
    enable(&dir, &config)?;
    Ok(())
}

/// The actual offline edit. Returns Ok(true) if it changed the disk,
/// Ok(false) if the console was already enabled.
pub(crate) fn enable(dir: &VmDir, config: &VmConfig) -> Result<bool> {
    if config.console_enabled == Some(true) {
        return Ok(false);
    }
    ensure_virt_customize()?;

    eprintln!(
        "Enabling serial console in the guest bootloader (one-time offline edit via libguestfs)..."
    );
    let output = Command::new("virt-customize")
        .arg("-a")
        .arg(dir.disk_path())
        .arg("--run-command")
        .arg("grubby --update-kernel=ALL --args=console=ttyS0")
        .output()
        .context("cannot run virt-customize")?;

    if !output.status.success() {
        // libguestfs writes progress to stderr; surface it on failure.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut hint = String::new();
        if !readable_host_kernel() {
            hint = "\n\nthis host's /boot/vmlinuz-* is not readable by this user, so \
                 libguestfs cannot build its appliance. One-time fix:\n  \
                 sudo chmod a+r /boot/vmlinuz-*"
                .to_string();
        }
        bail!(
            "virt-customize failed (exit {}):\n{}{}",
            output.status.code().unwrap_or(-1),
            stderr.trim(),
            hint
        );
    }

    let mut stamped = config.clone();
    stamped.console_enabled = Some(true);
    stamped.write(&dir.config_path())?;
    logger::log(dir, "console enabled (grubby via virt-customize)");
    eprintln!("Serial console enabled — 'virt start' will show kernel output and a login prompt.");
    Ok(true)
}

fn ensure_virt_customize() -> Result<()> {
    let ok = Command::new("virt-customize")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    anyhow::ensure!(
        ok,
        "virt-customize not found — install libguestfs-tools \
         (see 'virt doctor'); or configure the guest manually: \
         grubby --update-kernel=ALL --args=console=ttyS0"
    );
    Ok(())
}

/// supermin needs to read a host kernel to build libguestfs' appliance;
/// distros with locked-down /boot (mode 0600) break it for non-root.
fn readable_host_kernel() -> bool {
    let Ok(entries) = std::fs::read_dir("/boot") else {
        return false;
    };
    entries.flatten().any(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        name.starts_with("vmlinuz") && std::fs::File::open(e.path()).is_ok()
    })
}

/// Called from `virt start`: for an installed VM that has not yet had
/// its console enabled, try the offline edit before booting. Missing
/// tooling downgrades to a hint, never blocks the boot.
pub(crate) fn maybe_auto_enable(dir: &VmDir, config: &VmConfig) {
    // Direct-kernel boots already carry console=ttyS0 on the host-built
    // command line — nothing to enable in the guest.
    if dir.kernel_path().exists() {
        return;
    }
    if config.console_enabled == Some(true) {
        return;
    }
    if VmLock::is_locked(dir) {
        return;
    }
    match enable(dir, config) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("note: automatic console setup skipped: {e:#}");
            eprintln!("  the guest may boot silently; see the README's first-boot section.");
        }
    }
}
