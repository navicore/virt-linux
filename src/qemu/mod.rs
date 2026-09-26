//! QEMU command-line construction.
//!
//! The argv builder is a pure function of (VmConfig, VmDir, arch, accel,
//! boot, display, share) — no I/O, no host probing. Host-dependent
//! choices (accel, firmware paths) are resolved by the caller and
//! passed in, which is what makes this module the unit-test centerpiece
//! of the port: the QEMU contract is pinned by tests the same way
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
    /// Host directory shared via virtiofs (tag `share`). Requires the
    /// virtiofsd daemon (spawned by the supervisor before QEMU) and a
    /// shared-memory backend for vhost-user.
    pub share: Option<&'a Path>,
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

        // vhost-user (virtiofs) needs shared guest memory: a memfd
        // backend bound to a NUMA node. Only added when sharing.
        if self.share.is_some() {
            a.push("-object".into());
            a.push(format!(
                "memory-backend-memfd,id=mem,size={}M,share=on",
                self.config.memory_mb
            ));
            a.push("-numa".into());
            a.push("node,memdev=mem".into());
        }

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

        if self.share.is_some() {
            // The virtiofs share: same tag ("share") and guest-side
            // mount instructions as virt-macos's virtiofs device.
            a.push("-chardev".into());
            a.push(format!(
                "socket,id=char0,path={}",
                pb(&self.dir.virtiofsd_socket())
            ));
            a.push("-device".into());
            a.push("vhost-user-fs-pci,chardev=char0,tag=share".into());
        }

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
                // disable-ticketing: QEMU challenges clients by default,
                // and with no password set the challenge can never be
                // answered. The socket is already private to the VM dir
                // (filesystem permissions), so no auth is needed.
                a.push(format!(
                    "unix=on,addr={},disable-ticketing",
                    pb(&self.dir.spice_socket())
                ));
                // HID for the install window (arch-uniform virtio input).
                a.push("-device".into());
                a.push("virtio-keyboard-pci".into());
                a.push("-device".into());
                a.push("virtio-tablet-pci".into());
            }
        }

        a.push("-name".into());
        a.push(p(&self.config.name));
        // No -pidfile here: the SUPERVISOR writes vm.pid with its own
        // PID (virt-macos parity — `virt stop` signals the supervisor,
        // not QEMU; QEMU dies with the supervisor via PDEATHSIG).
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
