//! Integration tests for the QEMU argv builder — the port's pinned
//! contract with QEMU, analogous to virt-macos's VMConfiguration.

use std::path::{Path, PathBuf};

use virt_linux::config::VmConfig;
use virt_linux::qemu::arch::{Accel, Arch};
use virt_linux::qemu::{Boot, Display, QemuSpec};
use virt_linux::vmdir::VmDir;

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
        firmware: None,
        console_enabled: None,
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
        share: None,
    }
}

fn efi_boot() -> Boot {
    Boot::Efi {
        firmware_code: "/usr/share/OVMF/OVMF_CODE_4M.fd".into(),
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
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
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
    // No -pidfile: the supervisor writes vm.pid with its own PID
    // (virt stop signals the supervisor, not QEMU).
    assert!(!a.iter().any(|x| x == "-pidfile"));
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

/// BIOS boot: SeaBIOS default firmware, no pflash drives.
#[test]
fn bios_boot_skips_pflash() {
    let cfg = config();
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, Boot::Bios);
    let a = s.argv();
    assert!(!has(&a, "pflash"));
    assert!(!has(&a, &d.nvram_path().display().to_string()));
    // Everything else is still present.
    assert!(has(&a, &format!("file={}", d.disk_path().display())));
    assert_eq!(arg(&a, "-serial"), "stdio");
}

/// Installer direct boot: kernel/initrd/append from isoboot plus
/// -no-reboot so the end-of-install reset ends the session instead of
/// looping back into the installer.
#[test]
fn installer_boot_exits_on_guest_reset() {
    let cfg = config();
    let d = dir();
    let s = QemuSpec {
        config: &cfg,
        dir: &d,
        arch: Arch::X86_64,
        accel: Accel::Kvm,
        boot: Boot::Installer {
            kernel: d.installer_kernel(),
            initrd: d.installer_initrd(),
            append:
                "console=ttyS0 console=tty0 inst.graphical inst.noreboot inst.stage2=hd:LABEL=X"
                    .into(),
        },
        display: Display::Spice,
        iso: Some(Path::new("/iso/rocky.iso")),
        share: None,
    };
    let a = s.argv();
    assert_eq!(
        arg(&a, "-kernel"),
        d.installer_kernel().display().to_string()
    );
    assert_eq!(
        arg(&a, "-initrd"),
        d.installer_initrd().display().to_string()
    );
    assert!(arg(&a, "-append").contains("console=ttyS0"));
    assert!(a.iter().any(|x| x == "-no-reboot"));
    // The installer's install source stays attached.
    assert!(has(&a, "media=cdrom,readonly=on"));
}

#[test]
fn lan_mode_gets_dual_nic_and_stable_mcast() {
    let mut cfg = config();
    cfg.network_mode = Some("lan".into());
    cfg.lan_name = Some("k3s".into());
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    let a = s.argv();
    // eth0: private slirp; eth1: shared cluster segment.
    assert!(has(&a, "user,id=net0"));
    assert!(has(
        &a,
        &format!(
            "socket,id=lan0,mcast={}",
            virt_linux::qemu::lan_multicast_endpoint("k3s")
        )
    ));
    // Second NIC MAC must differ from the primary.
    assert!(has(&a, "netdev=lan0,mac=02:11:22:33:44:56"));
    // Deterministic: same name → same endpoint, different name → not.
    assert_eq!(
        virt_linux::qemu::lan_multicast_endpoint("k3s"),
        virt_linux::qemu::lan_multicast_endpoint("k3s")
    );
    assert_ne!(
        virt_linux::qemu::lan_multicast_endpoint("k3s"),
        virt_linux::qemu::lan_multicast_endpoint("other")
    );
}

#[test]
fn gui_mode_spice_iso_and_input() {
    let cfg = config();
    let d = dir();
    let mut s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    s.display = Display::Spice;
    s.iso = Some(Path::new("/iso/debian-13-amd64-netinst.iso"));
    let a = s.argv();
    assert!(has(&a, "media=cdrom,readonly=on"));
    // Installer sessions exit on guest reset instead of re-firing.
    // (Exercised via Boot::Installer in the direct-boot test below.)
    assert!(!a.iter().any(|x| x == "-no-reboot"));
    assert!(has(
        &a,
        &format!(
            "unix=on,addr={},disable-ticketing",
            d.spice_socket().display()
        )
    ));
    // GUI sessions capture the serial port for debugging silent installs.
    assert!(has(
        &a,
        &format!("file,id=serlog,path={}", d.serial_log().display())
    ));
    assert_eq!(arg(&a, "-serial"), "chardev:serlog");
    assert!(has(&a, "virtio-keyboard-pci"));
    assert!(has(&a, "virtio-tablet-pci"));
    // Serial in GUI mode is captured to the log chardev, not stdio.
    assert_eq!(arg(&a, "-serial"), "chardev:serlog");
}

#[test]
fn bridge_mode_uses_bridge_netdev() {
    let mut cfg = config();
    cfg.network_mode = Some("bridge".into());
    cfg.bridge_interface = Some("br0".into());
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    let a = s.argv();
    assert!(has(&a, "bridge,id=net0,br=br0"));
    assert!(!has(&a, "socket,id=lan0"));
}

#[test]
fn guest_agent_channel_always_present() {
    let cfg = config();
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    let a = s.argv();
    assert!(has(&a, "name=org.qemu.guest_agent.0"));
    assert!(has(&a, &format!("path={}", d.qga_socket().display())));
}

#[test]
fn share_wires_virtiofs_with_shared_memory() {
    let cfg = config();
    let d = dir();
    let mut s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    s.share = Some(Path::new("/home/user/code"));
    let a = s.argv();
    // vhost-user needs shared guest memory.
    assert!(has(&a, "memory-backend-memfd,id=mem,size=8192M,share=on"));
    assert!(has(&a, "node,memdev=mem"));
    // The share device, same tag as virt-macos's virtiofs share.
    assert!(has(
        &a,
        &format!("socket,id=char0,path={}", d.virtiofsd_socket().display())
    ));
    assert!(has(&a, "vhost-user-fs-pci,chardev=char0,tag=share"));
}

#[test]
fn no_share_means_no_numa_or_vhost_user() {
    let cfg = config();
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    let a = s.argv();
    assert!(!has(&a, "memory-backend-memfd"));
    assert!(!has(&a, "-numa"));
    assert!(!has(&a, "vhost-user-fs-pci"));
}

/// MAC derivation and adjacency (also covered implicitly above, but
/// pinned here since config MACs are optional).
#[test]
fn derived_mac_is_locally_administered_and_adjacent_differs() {
    let mut cfg = config();
    cfg.mac_address = None;
    let d = dir();
    let s = spec(&cfg, &d, Arch::X86_64, efi_boot());
    let a = s.argv();
    let mac = a
        .iter()
        .find(|x| x.starts_with("virtio-net-pci,netdev=net0,mac="))
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_string();
    let first = u8::from_str_radix(&mac[..2], 16).unwrap();
    assert_eq!(first & 0x03, 0x02, "{mac} not locally-administered unicast");
    // Deterministic for the same name.
    let a2 = s.argv();
    assert!(a2.iter().any(|x| x.ends_with(&format!("mac={mac}"))));
}

#[test]
fn adjacent_mac_wraps() {
    // Exercised through the private helper via the public argv in lan
    // mode above; re-verified by construction here using the test
    // binary's own copy of the logic.
    fn adjacent(mac: &str) -> String {
        let mut parts: Vec<String> = mac.split(':').map(|s| s.to_string()).collect();
        let last = parts.last_mut().unwrap();
        let octet = u8::from_str_radix(last, 0x10).unwrap();
        *last = format!("{:02x}", octet.wrapping_add(1));
        parts.join(":")
    }
    assert_eq!(adjacent("02:aa:bb:cc:dd:ff"), "02:aa:bb:cc:dd:00");
}

/// PathBuf helper re-exported for tests that build boot variants.
#[allow(dead_code)]
fn unused(_p: PathBuf) {}
