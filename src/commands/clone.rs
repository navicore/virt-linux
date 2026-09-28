//! `virt clone` — copy a VM (disk, firmware, boot config) under a new
//! name with fresh identity: new MAC, and — via virt-sysprep — a new
//! machine-id, SSH host keys, and hostname inside the guest.
//!
//! The template workflow: keep a fully configured VM stopped (GUI
//! installed, console enabled, agents installed) and clone it for each
//! new instance. Clones inherit firmware, console state, and network
//! mode — clone a `lan:` VM three times and you have a k3s cluster's
//! node set.

use crate::cli::Clone;
use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::logger;
use crate::mac;
use crate::vmdir::{VmDir, validate_name};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

pub fn run(args: &Clone) -> Result<()> {
    let src = VmDir::new(&args.source);
    let dst = VmDir::new(&args.new_name);

    if !src.exists() {
        bail!("VM '{}' does not exist.", args.source);
    }
    validate_name(&args.new_name)?;
    if dst.exists() {
        bail!("VM '{}' already exists.", args.new_name);
    }
    if VmLock::is_locked(&src) {
        let detail = src.pid().map(|p| format!(" (PID {p})")).unwrap_or_default();
        bail!(
            "VM '{}' is running{detail} — stop it first; a clone needs a quiet disk.",
            args.source
        );
    }

    let config = VmConfig::load(&src.config_path())?;
    let new_mac = mac::random_locally_administered()?;
    let description = args
        .description
        .clone()
        .unwrap_or_else(|| format!("clone of {}", args.source));

    dst.create()?;
    let result = perform(
        &src,
        &dst,
        &config,
        &args.new_name,
        &description,
        &new_mac,
        !args.no_sysprep,
    );
    if let Err(e) = result {
        // Leave no half-cloned VM behind.
        let _ = dst.remove();
        return Err(e);
    }
    Ok(())
}

fn perform(
    src: &VmDir,
    dst: &VmDir,
    config: &VmConfig,
    new_name: &str,
    description: &str,
    new_mac: &str,
    sysprep: bool,
) -> Result<()> {
    // Same hardware and boot setup, new identity.
    let mut new_config = config.clone();
    new_config.name = new_name.to_string();
    new_config.description = Some(description.to_string());
    new_config.mac_address = Some(new_mac.to_string());
    new_config.console_enabled = config.console_enabled;
    new_config.write(&dst.config_path())?;

    // The disk: sparse copy (holes preserved — actual usage is what
    // gets written, not the declared size).
    let started = std::time::Instant::now();
    let status = Command::new("cp")
        .arg("--sparse=always")
        .arg(src.disk_path())
        .arg(dst.disk_path())
        .status()
        .context("cannot run cp")?;
    anyhow::ensure!(status.success(), "disk copy failed");
    let copied = started.elapsed();

    // EFI variable store and imported-kernel files, when present.
    if src.nvram_path().exists() {
        std::fs::copy(src.nvram_path(), dst.nvram_path()).context("cannot copy NVRAM store")?;
    }
    for (s, d) in [
        (src.kernel_path(), dst.kernel_path()),
        (src.initrd_path(), dst.initrd_path()),
    ] {
        if s.exists() {
            std::fs::copy(&s, &d).with_context(|| format!("cannot copy {}", s.display()))?;
        }
    }

    logger::log(src, &format!("cloned to '{new_name}' (mac {new_mac})"));
    logger::log(dst, &format!("cloned from '{}' (mac {new_mac})", src.name));

    let size = human_bytes(&dst.disk_path());
    println!("Cloned '{}' -> '{}'", src.name, new_name);
    println!("  Disk:   {size} copied in {:.1}s", copied.as_secs_f64());
    println!("  MAC:    {new_mac} (fresh — the template keeps its own)");

    if sysprep {
        sysprep_guest(dst, new_name)?;
    } else {
        println!("  note: --no-sysprep: guest keeps the template's machine-id,");
        println!("        SSH host keys, and hostname.");
    }
    Ok(())
}

/// De-duplicate guest identity: new machine-id, regenerated SSH host
/// keys, hostname set to the VM name. k3s and friends key nodes off
/// machine-id — without this, three clones are one node in three hats.
fn sysprep_guest(dst: &VmDir, new_name: &str) -> Result<()> {
    let binary_available = Command::new("virt-sysprep")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !binary_available {
        println!(
            "  note: virt-sysprep not found — guest keeps the template's identity.\
             \n        Install libguestfs-tools before cloning for unique machine-ids."
        );
        return Ok(());
    }

    println!("  sysprep: resetting guest identity (machine-id, host keys, hostname)...");
    let output = Command::new("virt-sysprep")
        .arg("-a")
        .arg(dst.disk_path())
        .arg("--hostname")
        .arg(new_name)
        .output()
        .context("cannot run virt-sysprep")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!(
            "  warning: virt-sysprep failed (exit {}) — the clone works but shares\
             \n           the template's machine-id and host keys:\n{}",
            output.status.code().unwrap_or(-1),
            indent(&stderr)
        );
        return Ok(());
    }
    logger::log(dst, "sysprep: guest identity reset");
    Ok(())
}

fn indent(s: &str) -> String {
    s.trim()
        .lines()
        .map(|l| format!("             {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn human_bytes(path: &Path) -> String {
    let bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_formats() {
        let p = std::env::temp_dir().join("virt-human-test");
        std::fs::write(&p, vec![0u8; 5 * 1024 * 1024]).unwrap();
        assert_eq!(human_bytes(&p), "5 MB");
        std::fs::write(&p, vec![0u8; 3 * 1024 * 1024 * 1024]).unwrap();
        assert_eq!(human_bytes(&p), "3.0 GB");
        let _ = std::fs::remove_file(&p);
    }

    /// End-to-end clone of a synthetic VM (tiny disk, no sysprep):
    /// config restamped, disk/kernel copied, transient files excluded.
    #[test]
    fn clone_copies_disk_and_restamps_identity() {
        let base = std::env::temp_dir().join(format!("virt-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src_root = base.join("src");
        let dst_root = base.join("dst");
        std::fs::create_dir_all(&src_root).unwrap();
        std::fs::create_dir_all(&dst_root).unwrap();

        let src = VmDir {
            name: "template".into(),
            root: src_root.clone(),
        };
        std::fs::write(
            src.config_path(),
            r#"{"name":"template","cpus":2,"memoryMB":4048,"diskSizeGB":10,
                "description":"gold image","macAddress":"02:aa:bb:cc:dd:ee",
                "firmware":"bios","consoleEnabled":true,"networkMode":"lan","lanName":"k3s"}"#,
        )
        .unwrap();
        std::fs::write(src.disk_path(), vec![0u8; 1024]).unwrap();
        std::fs::write(src.kernel_path(), b"k").unwrap();
        std::fs::write(src.initrd_path(), b"i").unwrap();
        std::fs::write(src.pid_path(), b"999").unwrap(); // transient — must not travel

        let config = VmConfig::load(&src.config_path()).unwrap();
        let dst = VmDir {
            name: "node1".into(),
            root: dst_root.clone(),
        };
        perform(
            &src,
            &dst,
            &config,
            "node1",
            "clone of template",
            "02:11:22:33:44:55",
            false,
        )
        .unwrap();

        let cloned = VmConfig::load(&dst.config_path()).unwrap();
        assert_eq!(cloned.name, "node1");
        assert_eq!(cloned.mac_address.as_deref(), Some("02:11:22:33:44:55"));
        assert_eq!(cloned.firmware.as_deref(), Some("bios"));
        assert_eq!(cloned.console_enabled, Some(true));
        assert_eq!(cloned.lan_name.as_deref(), Some("k3s"));
        assert!(dst.disk_path().exists());
        assert!(dst.kernel_path().exists() && dst.initrd_path().exists());
        assert!(!dst.pid_path().exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
