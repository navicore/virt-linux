//! `virt install` — boot a VM with a GUI window for OS install.

use crate::cli::Install;
use crate::installer;
use crate::iso::{self, IsoArch};
use crate::lock::VmLock;
use crate::qemu::arch::Arch;
use crate::vmdir::VmDir;
use anyhow::{Result, bail};
use std::path::Path;

pub fn run(args: &Install) -> Result<()> {
    let dir = VmDir::new(&args.name);

    if !dir.exists() {
        bail!(
            "VM '{}' does not exist. Run 'virt create' first.",
            args.name
        );
    }

    let iso_path = args.iso.as_deref().map(Path::new);
    if let Some(iso) = iso_path {
        check_iso_architecture(iso)?;
    }

    // Held for the life of the process; released by the kernel on death.
    let _lock = VmLock::acquire(&dir)?;

    let config = crate::config::VmConfig::load(&dir.config_path())?;

    eprintln!("Booting VM '{}' with GUI...", args.name);
    if let Some(iso) = &args.iso {
        eprintln!("  ISO: {iso}");
    }
    eprintln!("  CPUs: {}, Memory: {} MB", config.cpus, config.memory_mb);

    if let Some(share) = &args.share {
        eprintln!("  Shared folder: {share} (mount in guest: mount -t virtiofs share /mnt)");
    }

    let share = args.share.as_deref().map(Path::new);
    installer::run_gui(&config, &dir, iso_path, !args.no_viewer, share)
}

/// QEMU on this host cannot run the other arch under KVM — fail fast
/// with a clear error instead of a black window (virt-macos parity;
/// its variant of this check exists because VZ cannot run x86 at all).
fn check_iso_architecture(iso: &Path) -> Result<()> {
    if !iso.exists() {
        bail!("ISO file not found: {}", iso.display());
    }
    let host = Arch::host();
    let mismatch = match (host, iso::detect(iso)) {
        (Arch::X86_64, IsoArch::Aarch64) => Some(("aarch64", "amd64/x86_64")),
        (Arch::Aarch64, IsoArch::X8664) => Some(("x86_64", "arm64/aarch64")),
        // Both loaders: the host firmware picks its own — fine.
        (_, IsoArch::Both) | (_, IsoArch::Unknown) => None,
        (_, _) => None,
    };
    if let Some((iso_arch, want)) = mismatch {
        bail!(
            "ISO is {iso_arch} — this host runs {} guests under KVM. \
             Download the {want} build of your distro instead.",
            match host {
                Arch::X86_64 => "x86_64",
                Arch::Aarch64 => "aarch64",
            }
        );
    }
    if iso::detect(iso) == IsoArch::Unknown {
        eprintln!("warning: could not determine ISO architecture; proceeding anyway.");
    }
    Ok(())
}
