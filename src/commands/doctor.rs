//! `virt doctor` — diagnose host setup: KVM, QEMU, firmware, VMs.

use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::qemu::arch::Arch;
use crate::vmdir::VmDir;
use anyhow::Result;

pub fn run() -> Result<()> {
    let host = Arch::host();

    println!("Host arch:    {host}");
    check_accel();
    check_qemu(host);
    check_firmware(host);
    check_virtiofsd();
    check_vms();
    Ok(())
}

fn check_accel() {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata("/dev/kvm") {
        Ok(m) => {
            if m.permissions().mode() & 0o006 != 0 {
                println!("KVM:          ok (/dev/kvm present and usable)");
            } else {
                println!("KVM:          NOT USABLE — /dev/kvm exists but this user lacks access");
                println!("              fix: sudo usermod -aG kvm $USER, then re-login");
            }
        }
        Err(_) => {
            println!("KVM:          MISSING — VMs will run under TCG emulation (slow)");
            println!("              load the kvm module and create /dev/kvm");
        }
    }
}

fn check_qemu(host: Arch) {
    let mine = host.qemu_binary();
    if let Some(path) = find_in_path(mine) {
        println!("QEMU:         ok ({})", path.display());
    } else {
        println!("QEMU:         MISSING ({mine} not on PATH — install the qemu-system package)");
    }
    // Cross-arch report: the other arch's binary, if installed, enables
    // TCG-emulated guests of that arch.
    let other = match host {
        Arch::X86_64 => Arch::Aarch64,
        Arch::Aarch64 => Arch::X86_64,
    };
    let other_bin = other.qemu_binary();
    match find_in_path(other_bin) {
        Some(path) => println!(
            "Cross-arch:   available ({} at {} — {other} guests via TCG)",
            other_bin,
            path.display()
        ),
        None => println!("Cross-arch:   {other_bin} not installed (only {host} guests possible)"),
    }
}

fn check_firmware(host: Arch) {
    match host.resolve_firmware() {
        Some(fw) => println!("UEFI:         ok ({} + {})", fw.code, fw.vars_template),
        None => {
            println!("UEFI:         MISSING — no firmware found for {host}; looked for:");
            for c in host.firmware_candidates() {
                println!("              {}", c.code);
            }
            println!("              install the edk2/OVMF (or AAVMF) package");
        }
    }
}

fn check_virtiofsd() {
    match find_in_path("virtiofsd") {
        Some(path) => println!("virtiofsd:    ok ({}) — --share supported", path.display()),
        None => println!("virtiofsd:    not installed (--share will be unavailable)"),
    }
}

fn check_vms() {
    let dirs = VmDir::all();
    if dirs.is_empty() {
        println!("VMs:          none under {}", VmDir::base().display());
        return;
    }
    println!("VMs:          {}", VmDir::base().display());
    for dir in &dirs {
        let state = match VmConfig::load(&dir.config_path()) {
            Ok(c) => {
                let running = if VmLock::is_locked(dir) {
                    "running"
                } else {
                    "stopped"
                };
                format!(
                    "{running}, {}, {} MB, {}",
                    c.cpus,
                    c.memory_mb,
                    c.network_display()
                )
            }
            Err(_) => "corrupt config".to_string(),
        };
        println!("  {:<20} {state}", dir.name);
    }
}

fn find_in_path(bin: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        };
        write!(f, "{s}")
    }
}
