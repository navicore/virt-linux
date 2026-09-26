//! Guest-arch dispatch: QEMU binary, machine type, CPU, video, firmware.
//!
//! One binary supports x86_64 and aarch64 hosts and guests. Same-arch
//! guests run under KVM at full speed; cross-arch guests fall back to
//! TCG emulation (slow, but virt-macos cannot do it at all).

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

/// A (code, vars-template) firmware pair as shipped by distros.
#[derive(Debug)]
pub struct FirmwareCandidate {
    /// Read-only pflash unit 0 image.
    pub code: &'static str,
    /// Template copied to the per-VM NVRAM on first EFI boot.
    pub vars_template: &'static str,
}

impl Arch {
    pub fn host() -> Self {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }

    pub fn qemu_binary(self) -> &'static str {
        match self {
            Arch::X86_64 => "qemu-system-x86_64",
            Arch::Aarch64 => "qemu-system-aarch64",
        }
    }

    pub fn machine(self) -> &'static str {
        match self {
            Arch::X86_64 => "q35",
            Arch::Aarch64 => "virt",
        }
    }

    /// virtio-vga on x86; virtio-gpu-pci on aarch64 (no VGA legacy).
    pub fn video_device(self) -> &'static str {
        match self {
            Arch::X86_64 => "virtio-vga",
            Arch::Aarch64 => "virtio-gpu-pci",
        }
    }

    /// CPU model for the given accelerator.
    pub fn cpu(self, accel: Accel) -> &'static str {
        match (self, accel) {
            (_, Accel::Kvm) => "host",
            (Arch::X86_64, Accel::Tcg) => "max",
            (Arch::Aarch64, Accel::Tcg) => "max",
        }
    }

    /// Distros ship UEFI firmware under different paths; `virt doctor`
    /// and first-boot setup resolve the first pair that exists.
    pub fn firmware_candidates(self) -> &'static [FirmwareCandidate] {
        match self {
            Arch::X86_64 => &[
                // Debian/Ubuntu (ovmf package)
                FirmwareCandidate {
                    code: "/usr/share/OVMF/OVMF_CODE_4M.fd",
                    vars_template: "/usr/share/OVMF/OVMF_VARS_4M.fd",
                },
                FirmwareCandidate {
                    code: "/usr/share/OVMF/OVMF_CODE.fd",
                    vars_template: "/usr/share/OVMF/OVMF_VARS.fd",
                },
                // Fedora (edk2-ovmf)
                FirmwareCandidate {
                    code: "/usr/share/edk2/ovmf/OVMF_CODE.fd",
                    vars_template: "/usr/share/edk2/ovmf/OVMF_VARS.fd",
                },
                // Arch (edk2-shell? no — edk2-ovmf package layout)
                FirmwareCandidate {
                    code: "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
                    vars_template: "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
                },
            ],
            Arch::Aarch64 => &[
                // Debian/Ubuntu (qemu-efi-aarch64)
                FirmwareCandidate {
                    code: "/usr/share/AAVMF/AAVMF_CODE.fd",
                    vars_template: "/usr/share/AAVMF/AAVMF_VARS.fd",
                },
                // Fedora (edk2-aarch64)
                FirmwareCandidate {
                    code: "/usr/share/edk2/aarch64/QEMU_EFI.pflash",
                    vars_template: "/usr/share/edk2/aarch64/vars-template-pflash.raw",
                },
                // Arch (edk2-aarch64)
                FirmwareCandidate {
                    code: "/usr/share/edk2/aarch64/QEMU_EFI.fd",
                    vars_template: "/usr/share/edk2/aarch64/QEMU_VARS.fd",
                },
            ],
        }
    }

    /// First firmware candidate whose code and vars template both exist.
    pub fn resolve_firmware(self) -> Option<&'static FirmwareCandidate> {
        self.firmware_candidates()
            .iter()
            .find(|c| Path::new(c.code).exists() && Path::new(c.vars_template).exists())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    Kvm,
    Tcg,
}

impl Accel {
    /// KVM when /dev/kvm can actually be opened by this user (QEMU's own
    /// requirement), else TCG emulation. Stat-mode bit tests are wrong
    /// here: a 0660 root:kvm device is usable via group membership that
    /// never shows in the mode bits.
    pub fn detect() -> Self {
        use std::fs::OpenOptions;
        match OpenOptions::new().read(true).write(true).open("/dev/kvm") {
            Ok(_) => Accel::Kvm,
            Err(_) => Accel::Tcg,
        }
    }

    pub fn qemu_arg(self) -> &'static str {
        match self {
            Accel::Kvm => "kvm",
            Accel::Tcg => "tcg,thread=multi",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_dispatch_basics() {
        assert_eq!(Arch::X86_64.qemu_binary(), "qemu-system-x86_64");
        assert_eq!(Arch::Aarch64.qemu_binary(), "qemu-system-aarch64");
        assert_eq!(Arch::X86_64.machine(), "q35");
        assert_eq!(Arch::Aarch64.machine(), "virt");
        assert_eq!(Arch::X86_64.cpu(Accel::Kvm), "host");
        assert_eq!(Arch::Aarch64.cpu(Accel::Tcg), "max");
    }
}
