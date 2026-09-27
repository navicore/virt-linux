//! Installer-kernel extraction and direct boot of ISO installers.
//!
//! The fix for the silent first boot: distro installers (anaconda,
//! debian-installer) persist install-time kernel arguments into the
//! installed system's bootloader. By extracting the installer's own
//! kernel/initrd from the ISO and booting them directly with
//! `console=ttyS0` appended, `virt install` produces an installed
//! system whose serial console works with zero guest-side steps.
//!
//! Extraction shells out to `bsdtar` (libarchive-tools) — small,
//! ubiquitous, no daemon; `virt doctor` checks for it. ISOs without a
//! recognizable installer layout fall back to the plain EFI cdrom
//! boot.

use crate::iso;
use crate::qemu::arch::Arch;
use crate::vmdir::VmDir;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct InstallerBoot {
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub append: String,
}

/// (kernel-path, initrd-path) candidates inside the ISO, per family.
fn layout_candidates(arch: Arch) -> &'static [(&'static str, &'static str)] {
    match arch {
        // RHEL family: Rocky, RHEL, Fedora, Alma, CentOS.
        Arch::X86_64 => &[
            ("images/pxeboot/vmlinuz", "images/pxeboot/initrd.img"),
            ("install.amd/vmlinuz", "install.amd/initrd.gz"),
        ],
        Arch::Aarch64 => &[
            ("images/pxeboot/vmlinuz", "images/pxeboot/initrd.img"),
            ("install.arm64/vmlinuz", "install.arm64/initrd.gz"),
        ],
    }
}

/// Extract the installer kernel + initrd from `iso` into the VM dir
/// and build the kernel command line. Ok(None) = layout not
/// recognized (caller falls back to the EFI cdrom boot).
pub fn prepare(iso: &Path, arch: Arch, dir: &VmDir) -> Result<Option<InstallerBoot>> {
    if !bsdtar_available() {
        eprintln!(
            "  warning: bsdtar not found — cannot inject the serial console into the\n\
             \x20            installer. Install libarchive-tools for automatic console setup;"
        );
        eprintln!("  warning: otherwise the installed VM needs manual console setup (see README).");
        return Ok(None);
    }

    let kernel_path = dir.installer_kernel();
    let initrd_path = dir.installer_initrd();

    for &(krel, irel) in layout_candidates(arch) {
        let kernel = extract_file(iso, krel)?;
        let Some(kernel) = kernel else {
            continue;
        };
        let initrd = match extract_file(iso, irel)? {
            Some(initrd) => initrd,
            None => continue,
        };
        std::fs::write(&kernel_path, kernel)
            .with_context(|| format!("cannot write {}", kernel_path.display()))?;
        std::fs::write(&initrd_path, initrd)
            .with_context(|| format!("cannot write {}", initrd_path.display()))?;

        // RHEL-family installers need stage2 pointed at the CD, by the
        // ISO's volume label; Debian-family installers discover the CD
        // themselves. Console ordering matters: the kernel logs to
        // BOTH, but the LAST console becomes /dev/console — putting
        // tty0 last keeps anaconda's GUI on the SPICE display, while
        // ttyS0 stays registered (and is persisted into the installed
        // system, where systemd auto-spawns its serial getty).
        let rhel_layout = krel.starts_with("images/pxeboot");
        let append = if rhel_layout {
            let label = iso::volume_label(iso).context(
                "cannot read ISO volume label — needed for the installer's stage2 lookup",
            )?;
            format!(
                "console=ttyS0 console=tty0 inst.graphical inst.noreboot inst.stage2=hd:LABEL={label}"
            )
        } else {
            "console=ttyS0 console=tty0".to_string()
        };
        return Ok(Some(InstallerBoot {
            kernel: kernel_path,
            initrd: initrd_path,
            append,
        }));
    }
    Ok(None)
}

fn bsdtar_available() -> bool {
    Command::new("bsdtar")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `bsdtar -xOf <iso> <member>` → file bytes, or None if absent.
fn extract_file(iso: &Path, member: &str) -> Result<Option<Vec<u8>>> {
    let output = Command::new("bsdtar")
        .arg("-xOf")
        .arg(iso)
        .arg(member)
        .output()
        .with_context(|| format!("cannot run bsdtar for {member}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_cover_rhel_and_debian_families() {
        assert!(
            layout_candidates(Arch::X86_64)
                .iter()
                .any(|(k, _)| *k == "images/pxeboot/vmlinuz")
        );
        assert!(
            layout_candidates(Arch::X86_64)
                .iter()
                .any(|(k, _)| *k == "install.amd/vmlinuz")
        );
        assert!(
            layout_candidates(Arch::Aarch64)
                .iter()
                .any(|(k, _)| *k == "install.arm64/vmlinuz")
        );
    }
}
