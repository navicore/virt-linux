//! `virt doctor` — diagnose host setup: KVM, QEMU, firmware, VMs.

use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::qemu::arch::Arch;
use crate::vmdir::VmDir;
use anyhow::Result;

/// Distro-specific package names, from /etc/os-release, so doctor's
/// hints are copy-pasteable. Canonical table lives in README.md.
struct PkgNames {
    manager: &'static str,
    qemu: [&'static str; 2], // [x86_64, aarch64] package names
    firmware: [&'static str; 2],
    virt_viewer: &'static str,
    virtiofsd: &'static str,
}

fn detect_pkgs() -> PkgNames {
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let id = release
        .lines()
        .filter_map(|l| l.strip_prefix("ID_LIKE=").or_else(|| l.strip_prefix("ID=")))
        .map(|v| v.trim_matches('"').to_lowercase())
        .collect::<String>();
    if id.contains("debian") || id.contains("ubuntu") || id.contains("pop") {
        PkgNames {
            manager: "apt install",
            qemu: ["qemu-system-x86", "qemu-system-arm"],
            firmware: ["ovmf", "qemu-efi-aarch64"],
            virt_viewer: "virt-viewer",
            virtiofsd: "virtiofsd",
        }
    } else if id.contains("fedora") {
        PkgNames {
            manager: "dnf install",
            qemu: ["qemu-kvm", "qemu-system-arm"],
            firmware: ["edk2-ovmf", "edk2-aarch64"],
            virt_viewer: "virt-viewer",
            virtiofsd: "virtiofsd",
        }
    } else if id.contains("arch") {
        PkgNames {
            manager: "pacman -S",
            qemu: ["qemu-system-x86_64", "qemu-system-aarch64"],
            firmware: ["edk2-ovmf", "edk2-aarch64"],
            virt_viewer: "virt-viewer",
            virtiofsd: "virtiofsd",
        }
    } else {
        PkgNames {
            manager: "install",
            qemu: ["qemu-system-x86_64", "qemu-system-aarch64"],
            firmware: ["edk2-ovmf", "edk2-aarch64"],
            virt_viewer: "virt-viewer",
            virtiofsd: "virtiofsd",
        }
    }
}

pub fn run() -> Result<()> {
    let host = Arch::host();
    let pkgs = detect_pkgs();

    println!("Host arch:    {host}");
    check_accel();
    check_qemu(host, &pkgs);
    check_firmware(host, &pkgs);
    check_virtiofsd(&pkgs);
    check_remote_viewer(&pkgs);
    check_vms();
    Ok(())
}

fn check_accel() {
    // The test is "can this user open it" — QEMU's own requirement —
    // not the mode bits (0660 root:kvm is fine via group membership).
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
    {
        Ok(_) => println!("KVM:          ok (/dev/kvm present and usable)"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("KVM:          MISSING — VMs will run under TCG emulation (slow)");
            println!("              load the kvm module and create /dev/kvm");
        }
        Err(_) => {
            println!("KVM:          NOT USABLE — /dev/kvm exists but this user lacks access");
            println!("              fix: sudo usermod -aG kvm $USER, then re-login");
        }
    }
}

fn check_qemu(host: Arch, pkgs: &PkgNames) {
    let mine = host.qemu_binary();
    let mine_pkg = match host {
        Arch::X86_64 => pkgs.qemu[0],
        Arch::Aarch64 => pkgs.qemu[1],
    };
    if let Some(path) = find_in_path(mine) {
        println!("QEMU:         ok ({})", path.display());
    } else {
        println!("QEMU:         MISSING — {} {}", pkgs.manager, mine_pkg);
    }
    // Cross-arch report: the other arch's binary, if installed, enables
    // TCG-emulated guests of that arch.
    let other = match host {
        Arch::X86_64 => Arch::Aarch64,
        Arch::Aarch64 => Arch::X86_64,
    };
    let other_bin = other.qemu_binary();
    let other_pkg = match other {
        Arch::X86_64 => pkgs.qemu[0],
        Arch::Aarch64 => pkgs.qemu[1],
    };
    match find_in_path(other_bin) {
        Some(path) => println!(
            "Cross-arch:   available ({} at {} — {other} guests via TCG)",
            other_bin,
            path.display()
        ),
        None => println!(
            "Cross-arch:   not installed — optional, for {other} guests via TCG ({} {})",
            pkgs.manager, other_pkg
        ),
    }
}

fn check_firmware(host: Arch, pkgs: &PkgNames) {
    let fw_pkg = match host {
        Arch::X86_64 => pkgs.firmware[0],
        Arch::Aarch64 => pkgs.firmware[1],
    };
    match host.resolve_firmware() {
        Some(fw) => println!("UEFI:         ok ({} + {})", fw.code, fw.vars_template),
        None => {
            println!("UEFI:         MISSING — {} {}", pkgs.manager, fw_pkg);
            println!("              looked for:");
            for c in host.firmware_candidates() {
                println!("                {}", c.code);
            }
        }
    }
}

fn check_virtiofsd(pkgs: &PkgNames) {
    match find_in_path("virtiofsd") {
        Some(path) => println!("virtiofsd:    ok ({}) — --share supported", path.display()),
        None => println!(
            "virtiofsd:    not installed — {} {} (needed for --share, M4)",
            pkgs.manager, pkgs.virtiofsd
        ),
    }
}

fn check_remote_viewer(pkgs: &PkgNames) {
    match find_in_path("remote-viewer") {
        Some(path) => println!(
            "remote-viewer: ok ({}) — GUI installs supported",
            path.display()
        ),
        None => println!(
            "remote-viewer: not installed — {} {} (needed for 'virt install' GUI, M3)",
            pkgs.manager, pkgs.virt_viewer
        ),
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
