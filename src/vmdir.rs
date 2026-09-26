//! Per-VM on-disk layout: `~/.virt/vms/<name>/` — the same base path
//! and layout as virt-macos, so disks, kernels, and configs move
//! between the two tools with plain copies (config.json schemas
//! overlap; disk.raw is plain raw either way).
//!
//! Everything a VM owns lives in one directory — config, disk, NVRAM,
//! runtime sockets (QMP, SPICE, guest agent), pid/lock/log files — so
//! `virt delete` is `rm -rf` and nothing else on the host is stateful.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VmDir {
    pub name: String,
    pub root: PathBuf,
}

impl VmDir {
    pub fn base() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        Path::new(&home).join(".virt").join("vms")
    }

    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            root: Self::base().join(name),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    pub fn disk_path(&self) -> PathBuf {
        self.root.join("disk.raw")
    }

    /// EFI variable store (edk2 pflash unit 1).
    pub fn nvram_path(&self) -> PathBuf {
        self.root.join("nvram.bin")
    }

    pub fn pid_path(&self) -> PathBuf {
        self.root.join("vm.pid")
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join("vm.lock")
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("vm.log")
    }

    /// Direct-kernel-boot files (see `virt kernel-import`).
    pub fn kernel_path(&self) -> PathBuf {
        self.root.join("kernel")
    }

    pub fn initrd_path(&self) -> PathBuf {
        self.root.join("initrd")
    }

    /// QMP control socket, owned by the QEMU supervisor.
    pub fn qmp_socket(&self) -> PathBuf {
        self.root.join("qmp.sock")
    }

    /// SPICE display socket for GUI (`virt install`) mode.
    pub fn spice_socket(&self) -> PathBuf {
        self.root.join("spice.sock")
    }

    /// Guest-agent channel (qemu-guest-agent over virtio-serial).
    pub fn qga_socket(&self) -> PathBuf {
        self.root.join("qga.sock")
    }

    /// virtiofsd vhost-user socket (the `--share` backend).
    pub fn virtiofsd_socket(&self) -> PathBuf {
        self.root.join("virtiofsd.sock")
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    pub fn create(&self) -> Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("cannot create {}", self.root.display()))
    }

    pub fn remove(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.root)
            .with_context(|| format!("cannot remove {}", self.root.display()))
    }

    /// All VM directories under the base dir, sorted by name.
    pub fn all() -> Vec<VmDir> {
        let base = Self::base();
        let Ok(entries) = std::fs::read_dir(&base) else {
            return Vec::new();
        };
        let mut dirs: Vec<VmDir> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| VmDir {
                name: e.file_name().to_string_lossy().into_owned(),
                root: e.path(),
            })
            .filter(|d| !d.name.starts_with('.'))
            .collect();
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        dirs
    }

    /// PID recorded by the supervisor, if present and parseable.
    /// Advisory only — the flock on `vm.lock` is the authority.
    pub fn pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.pid_path())
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

/// A VM name becomes a directory name — keep it safe.
pub fn validate_name(name: &str) -> Result<()> {
    anyhow::ensure!(!name.is_empty(), "VM name must not be empty");
    anyhow::ensure!(name.len() <= 64, "VM name must be at most 64 characters");
    anyhow::ensure!(
        !name.starts_with('.'),
        "VM name must not start with '.' (hidden directory)"
    );
    anyhow::ensure!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
        "VM name may contain only letters, digits, '-', '_', '.'"
    );
    Ok(())
}
