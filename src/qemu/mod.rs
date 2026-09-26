//! QEMU command-line construction.
//!
//! The argv builder is a pure function of (VmConfig, VmDir, arch, accel,
//! boot, display) — no I/O, no host probing. Host-dependent choices
//! (accel, firmware paths) are resolved by the caller and passed in,
//! which is what makes this module the unit-test centerpiece of the
//! port: the QEMU contract is pinned by tests the same way
//! VMConfiguration.swift pins the VZ contract.

pub mod arch;

use crate::config::VmConfig;
use crate::vmdir::VmDir;
use arch::{Accel, Arch};
use std::path::{Path, PathBuf};

/// How the guest boots. EFI for installs; direct kernel for fast
/// headless boots (`virt kernel-import`).
#[derive(Debug, Clone)]
pub enum Boot {
    /// Edk2 firmware via pflash. `firmware_code` is the distro's
    /// read-only image; per-VM NVRAM lives at `VmDir::nvram_path`.
    Efi { firmware_code: PathBuf },
    Kernel {
        kernel: PathBuf,
        initrd: PathBuf,
        /// e.g. "/dev/vda2"
        root_device: String,
        extra_args: Option<String>,
    },
}

/// Headless (`virt start`) vs GUI install window (`virt install`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// Serial console on stdio; no display server needed.
    Headless,
    /// SPICE over a unix socket; `virt install` spawns remote-viewer.
    Spice,
}

#[derive(Debug)]
pub struct QemuSpec<'a> {
    pub config: &'a VmConfig,
    pub dir: &'a VmDir,
    pub arch: Arch,
    pub accel: Accel,
    pub boot: Boot,
    pub display: Display,
    /// ISO attached (read-only, cdrom) for OS installs.
    pub iso: Option<&'a Path>,
}

impl QemuSpec<'_> {
    /// Full QEMU argv, element per argument (no shell quoting).
    pub fn argv(&self) -> Vec<String> {
        let mut a: Vec<String> = Vec::new();
        let p = |s: &str| s.to_string();
        let pb = |s: &Path| s.display().to_string();

        a.push(p(self.arch.qemu_binary()));
        a.push("-machine".into());
        a.push(p(self.arch.machine()));
        a.push("-accel".into());
        a.push(p(self.accel.qemu_arg()));
        a.push("-cpu".into());
        a.push(p(self.arch.cpu(self.accel)));
        a.push("-smp".into());
        a.push(self.config.cpus.to_string());
        a.push("-m".into());
        a.push(format!("{}M", self.config.memory_mb));

        match &self.boot {
            Boot::Efi { firmware_code } => {
                a.push("-drive".into());
                a.push(format!(
                    "if=pflash,format=raw,unit=0,readonly=on,file={}",
                    pb(firmware_code)
                ));
                a.push("-drive".into());
                a.push(format!(
                    "if=pflash,format=raw,unit=1,file={}",
                    pb(&self.dir.nvram_path())
                ));
            }
            Boot::Kernel {
                kernel,
                initrd,
                root_device,
                extra_args,
            } => {
                // QEMU's serial console is the 16550 UART, not virtio
                // hvc0 — hence ttyS0 in the command line.
                let mut cmdline = format!("console=ttyS0 root={root_device} ro");
                if let Some(extra) = extra_args {
                    cmdline.push(' ');
                    cmdline.push_str(extra);
                }
                a.push("-kernel".into());
                a.push(pb(kernel));
                a.push("-initrd".into());
                a.push(pb(initrd));
                a.push("-append".into());
                a.push(cmdline);
            }
        }

        // Main disk: raw image on virtio-blk.
        a.push("-drive".into());
        a.push(format!(
            "if=none,id=disk0,format=raw,file={}",
            pb(&self.dir.disk_path())
        ));
        a.push("-device".into());
        a.push("virtio-blk-pci,drive=disk0".into());

        if let Some(iso) = self.iso {
            a.push("-drive".into());
            a.push(format!("file={},media=cdrom,readonly=on", pb(iso)));
        }

        // Entropy source (mirrors VZVirtioEntropyDeviceConfiguration).
        a.push("-device".into());
        a.push("virtio-rng-pci".into());

        // Video device always present — EFI and GRUB need a framebuffer
        // even in headless mode (rendered nowhere, like virt-macos).
        a.push("-device".into());
        a.push(p(self.arch.video_device()));

        self.push_network(&mut a);
        self.push_agent_channel(&mut a);

        // QMP control socket — how `virt stop` reaches a running VM.
        a.push("-qmp".into());
        a.push(format!(
            "unix:{},server=on,nowait",
            pb(&self.dir.qmp_socket())
        ));

        match self.display {
            Display::Headless => {
                a.push("-display".into());
                a.push("none".into());
                // Guest serial console wired to the supervisor's stdio.
                a.push("-serial".into());
                a.push("stdio".into());
            }
            Display::Spice => {
                a.push("-spice".into());
                a.push(format!("unix=on,addr={}", pb(&self.dir.spice_socket())));
                // HID for the install window (arch-uniform virtio input).
                a.push("-device".into());
                a.push("virtio-keyboard-pci".into());
                a.push("-device".into());
                a.push("virtio-tablet-pci".into());
            }
        }

        a.push("-name".into());
        a.push(p(&self.config.name));
        // QEMU writes its own PID into the per-VM file — same contract
        // as virt-macos's PID file.
        a.push("-pidfile".into());
        a.push(pb(&self.dir.pid_path()));
        a
    }

    fn push_network(&self, a: &mut Vec<String>) {
        let mac = self
            .config
            .mac_address
            .clone()
            .unwrap_or_else(|| derived_mac(&self.config.name));
        match self.config.network_mode.as_deref() {
            Some("bridge") => {
                let br = self.config.bridge_interface.as_deref().unwrap_or("br0");
                a.push("-netdev".into());
                a.push(format!("bridge,id=net0,br={br}"));
                a.push("-device".into());
                a.push(format!("virtio-net-pci,netdev=net0,mac={mac}"));
            }
            Some("lan") => {
                // Dual-NIC cluster model: eth0 is a private per-VM slirp
                // (internet, no VM identity — every guest is 10.0.2.15),
                // eth1 is the shared L2 segment where k3s nodes live.
                let lan = self.config.lan_name.as_deref().unwrap_or("default");
                let endpoint = lan_multicast_endpoint(lan);
                let lan_mac = adjacent_mac(&mac);
                a.push("-netdev".into());
                a.push("user,id=net0".into());
                a.push("-device".into());
                a.push(format!("virtio-net-pci,netdev=net0,mac={mac}"));
                a.push("-netdev".into());
                a.push(format!("socket,id=lan0,mcast={endpoint}"));
                a.push("-device".into());
                a.push(format!("virtio-net-pci,netdev=lan0,mac={lan_mac}"));
            }
            // "nat" and legacy configs without a mode.
            _ => {
                a.push("-netdev".into());
                a.push("user,id=net0".into());
                a.push("-device".into());
                a.push(format!("virtio-net-pci,netdev=net0,mac={mac}"));
            }
        }
    }

    fn push_agent_channel(&self, a: &mut Vec<String>) {
        // qemu-guest-agent channel: gives the host guest-truth (IPs via
        // guest-network-get-interfaces) and richer shutdown later.
        a.push("-chardev".into());
        a.push(format!(
            "socket,path={},server=on,wait=off,id=qga0",
            self.dir.qga_socket().display()
        ));
        a.push("-device".into());
        a.push("virtio-serial-pci,id=vser0".into());
        a.push("-device".into());
        a.push("virtserialport,bus=vser0.0,chardev=qga0,name=org.qemu.guest_agent.0".into());
    }
}

/// Deterministic locally-administered MAC for configs that predate
/// persisted MACs. TODO(parity): assign-and-save like virt-macos does
/// on first boot instead of deriving.
fn derived_mac(name: &str) -> String {
    let h = fnv1a64(name.as_bytes());
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}",
        (h >> 24) as u8,
        (h >> 16) as u8,
        (h >> 8) as u8,
        h as u8
    )
}

/// Second NIC MAC: last octet of the primary +1 (with wrap), so a lan
/// VM's two interfaces never collide on one LAN segment.
fn adjacent_mac(mac: &str) -> String {
    let mut parts: Vec<String> = mac.split(':').map(|s| s.to_string()).collect();
    let last = parts.last_mut().expect("mac has octets");
    let octet = u8::from_str_radix(last, 0x10).unwrap_or(0);
    *last = format!("{:02x}", octet.wrapping_add(1));
    parts.join(":")
}

/// Multicast endpoint shared by every VM in a lan. All QEMUs on the
/// host that join the same group share one L2 segment — no root, no
/// bridge, no daemon. Deterministic per name; collisions between two
/// different lan names are possible but unlikely (64-bit FNV → 239.x
/// + 14-bit port) and harmless for a single-host test fabric.
pub fn lan_multicast_endpoint(lan: &str) -> String {
    let h = fnv1a64(lan.as_bytes());
    let b2 = ((h >> 8) & 0xFF) as u8;
    let b3 = (h & 0xFF) as u8;
    let port = 30_000 + ((h >> 16) & 0x3FFF) as u32;
    format!("239.255.{b2}.{b3}:{port}")
}

/// FNV-1a 64-bit — tiny, stable across Rust versions (std's
/// DefaultHasher makes no cross-version stability guarantee).
fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> VmConfig {
        VmConfig {
            name: "milford".into(),
            cpus: 4,
            memory_mb: 8192,
            disk_size_gb: 100,
            description: Some("medium size vm with rocky 10 os".into()),
            mac_address: Some("02:11:22:33:44:55".into()),
            root_device: None,
            extra_kernel_args: None,
            network_mode: None,
            bridge_interface: None,
            lan_name: None,
        }
    }

    fn dir() -> VmDir {
        VmDir::new("milford")
    }

    fn spec<'a>(cfg: &'a VmConfig, d: &'a VmDir, arch: Arch, boot: Boot) -> QemuSpec<'a> {
        QemuSpec {
            config: cfg,
            dir: d,
            arch,
            accel: Accel::Kvm,
            boot,
            display: Display::Headless,
            iso: None,
        }
    }

    /// Value following a flag argument, e.g. arg(args, "-m") -> "8192M".
    fn arg<'a>(args: &'a [String], flag: &str) -> &'a str {
        let i = args
            .iter()
            .position(|x| x == flag)
            .unwrap_or_else(|| panic!("flag {flag} missing: {args:?}"));
        args.get(i + 1).map(String::as_str).unwrap_or("")
    }

    fn has(args: &[String], needle: &str) -> bool {
        args.iter().any(|x| x.contains(needle))
    }

    #[test]
    fn headless_x86_64_kvm_nat_efi() {
        let cfg = config();
        let d = dir();
        let s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Efi {
                firmware_code: "/usr/share/OVMF/OVMF_CODE_4M.fd".into(),
            },
        );
        let a = s.argv();
        assert_eq!(a[0], "qemu-system-x86_64");
        assert_eq!(arg(&a, "-machine"), "q35");
        assert_eq!(arg(&a, "-accel"), "kvm");
        assert_eq!(arg(&a, "-cpu"), "host");
        assert_eq!(arg(&a, "-smp"), "4");
        assert_eq!(arg(&a, "-m"), "8192M");
        assert!(has(
            &a,
            "if=pflash,format=raw,unit=0,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd"
        ));
        assert!(has(
            &a,
            &format!("unit=1,file={}", d.nvram_path().display())
        ));
        assert!(has(&a, &format!("file={}", d.disk_path().display())));
        assert!(has(&a, "virtio-net-pci,netdev=net0,mac=02:11:22:33:44:55"));
        assert!(has(&a, "virtio-rng-pci"));
        assert!(has(&a, "virtio-vga"));
        assert_eq!(arg(&a, "-serial"), "stdio");
        assert_eq!(arg(&a, "-display"), "none");
        assert!(has(&a, &format!("unix:{}", d.qmp_socket().display())));
        assert!(has(&a, &d.pid_path().display().to_string()));
    }

    #[test]
    fn aarch64_tcg_machine_virt() {
        let cfg = config();
        let d = dir();
        let mut s = spec(
            &cfg,
            &d,
            Arch::Aarch64,
            Boot::Efi {
                firmware_code: "/usr/share/AAVMF/AAVMF_CODE.fd".into(),
            },
        );
        s.accel = Accel::Tcg;
        let a = s.argv();
        assert_eq!(a[0], "qemu-system-aarch64");
        assert_eq!(arg(&a, "-machine"), "virt");
        assert_eq!(arg(&a, "-accel"), "tcg,thread=multi");
        assert_eq!(arg(&a, "-cpu"), "max");
        assert!(has(&a, "virtio-gpu-pci"));
    }

    #[test]
    fn direct_kernel_boot_uses_tty_s0() {
        let cfg = config();
        let d = dir();
        let s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Kernel {
                kernel: d.kernel_path(),
                initrd: d.initrd_path(),
                root_device: "/dev/vda2".into(),
                extra_args: Some("quiet".into()),
            },
        );
        let a = s.argv();
        assert_eq!(arg(&a, "-kernel"), d.kernel_path().display().to_string());
        assert_eq!(arg(&a, "-initrd"), d.initrd_path().display().to_string());
        assert_eq!(arg(&a, "-append"), "console=ttyS0 root=/dev/vda2 ro quiet");
        // No pflash drives on the kernel-boot path.
        assert!(!has(&a, "pflash"));
    }

    #[test]
    fn lan_mode_gets_dual_nic_and_stable_mcast() {
        let mut cfg = config();
        cfg.network_mode = Some("lan".into());
        cfg.lan_name = Some("k3s".into());
        let d = dir();
        let s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Efi {
                firmware_code: "/x".into(),
            },
        );
        let a = s.argv();
        // eth0: private slirp; eth1: shared cluster segment.
        assert!(has(&a, "user,id=net0"));
        assert!(has(
            &a,
            &format!("socket,id=lan0,mcast={}", lan_multicast_endpoint("k3s"))
        ));
        // Second NIC MAC must differ from the primary.
        assert!(has(&a, "netdev=lan0,mac=02:11:22:33:44:56"));
        // Deterministic: same name → same endpoint, different name → not.
        assert_eq!(lan_multicast_endpoint("k3s"), lan_multicast_endpoint("k3s"));
        assert_ne!(
            lan_multicast_endpoint("k3s"),
            lan_multicast_endpoint("other")
        );
    }

    #[test]
    fn gui_mode_spice_iso_and_input() {
        let cfg = config();
        let d = dir();
        let mut s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Efi {
                firmware_code: "/x".into(),
            },
        );
        s.display = Display::Spice;
        s.iso = Some(Path::new("/iso/debian-13-amd64-netinst.iso"));
        let a = s.argv();
        assert!(has(&a, "media=cdrom,readonly=on"));
        assert!(has(
            &a,
            &format!("unix=on,addr={}", d.spice_socket().display())
        ));
        assert!(has(&a, "virtio-keyboard-pci"));
        assert!(has(&a, "virtio-tablet-pci"));
        // No serial-console wiring in GUI mode (exact-arg check: the
        // guest-agent channel legitimately contains "-serial").
        assert!(!a.iter().any(|x| x == "-serial"));
    }

    #[test]
    fn bridge_mode_uses_bridge_netdev() {
        let mut cfg = config();
        cfg.network_mode = Some("bridge".into());
        cfg.bridge_interface = Some("br0".into());
        let d = dir();
        let s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Efi {
                firmware_code: "/x".into(),
            },
        );
        let a = s.argv();
        assert!(has(&a, "bridge,id=net0,br=br0"));
        assert!(!has(&a, "socket,id=lan0"));
    }

    #[test]
    fn guest_agent_channel_always_present() {
        let cfg = config();
        let d = dir();
        let s = spec(
            &cfg,
            &d,
            Arch::X86_64,
            Boot::Efi {
                firmware_code: "/x".into(),
            },
        );
        let a = s.argv();
        assert!(has(&a, "name=org.qemu.guest_agent.0"));
        assert!(has(&a, &format!("path={}", d.qga_socket().display())));
    }

    #[test]
    fn adjacent_mac_wraps() {
        assert_eq!(adjacent_mac("02:aa:bb:cc:dd:ff"), "02:aa:bb:cc:dd:00");
    }
}
