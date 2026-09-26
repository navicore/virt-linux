//! Best-effort architecture detection for ISO images.
//!
//! Faithful port of virt-macos's ISOCheck: scans the raw image for EFI
//! boot-loader filenames in the ISO9660 directory records. Bootable
//! arm64 media must carry /EFI/BOOT/BOOTAA64.EFI; x86 media carries
//! BOOTX64.EFI.
//!
//! Only full loader filenames are trusted. Loose substrings ("aa64.efi")
//! false-positive on package text buried deep in DVD images: a Rocky
//! x86_64 DVD contains the string at offset ~10.16 GB (grub2-efi-aa64
//! RPM payload), which flipped the verdict to arm64 and booted a black
//! window. (Substring markers like "arm64-efi" are equally unreliable —
//! GRUB's `file` command embeds them as help text on every arch.)

use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsoArch {
    Aarch64,
    X8664,
    /// Multi-arch media carrying both loaders. virt-macos treats this
    /// as arm64 (VZ always boots BOOTAA64.EFI); on Linux the host
    /// firmware picks its own loader, so it is acceptable on any arch.
    Both,
    Unknown,
}

/// ISO9660 primary volume descriptor: sector 16 (2048-byte sectors),
/// volume-id field at offset 40, 32 bytes, space-padded.
const VD_SECTOR: u64 = 16;
const VD_ID_OFFSET: usize = 1;
const LABEL_OFFSET: usize = 40;
const LABEL_LEN: usize = 32;

/// The ISO's volume label, needed for RHEL-family installers to find
/// their stage2 on the CD (`inst.stage2=hd:LABEL=...`).
pub fn volume_label(path: &Path) -> Option<String> {
    use std::io::{Seek, SeekFrom};
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(VD_SECTOR * 2048)).ok()?;
    let mut sector = [0u8; 2048];
    file.read_exact(&mut sector).ok()?;
    if &sector[VD_ID_OFFSET..VD_ID_OFFSET + 5] != b"CD001" {
        return None;
    }
    let label = &sector[LABEL_OFFSET..LABEL_OFFSET + LABEL_LEN];
    Some(
        String::from_utf8_lossy(label)
            .trim_end_matches([' ', '\0'])
            .to_string(),
    )
}

const ARM64_MARKERS: [&[u8]; 4] = [
    b"BOOTAA64.EFI",
    b"bootaa64.efi",
    b"GRUBAA64.EFI",
    b"grubaa64.efi",
];

const X86_MARKERS: [&[u8]; 4] = [
    b"BOOTX64.EFI",
    b"bootx64.efi",
    b"GRUBX64.EFI",
    b"grubx64.efi",
];

/// EFI/BOOT directory records and the El Torito catalog sit within the
/// first few MB of an ISO (observed: ~1.3 MB on distro DVDs). Scanning
/// a full 10 GB DVD takes minutes; the cap bounds detection to ~1s.
const DEFAULT_SCAN_LIMIT: usize = 64 << 20;

/// Chunk carry-over so markers split across read boundaries still match.
const CARRY: usize = 64;

pub fn detect(path: &Path) -> IsoArch {
    detect_with_limit(path, DEFAULT_SCAN_LIMIT)
}

pub fn detect_with_limit(path: &Path, scan_limit: usize) -> IsoArch {
    let Ok(mut file) = File::open(path) else {
        return IsoArch::Unknown;
    };
    let mut found_arm64 = false;
    let mut found_x86 = false;
    let mut carry: Vec<u8> = Vec::new();
    let mut scanned = 0usize;
    let mut chunk = vec![0u8; 8 << 20];

    while let Ok(n) = file.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let budget = scan_limit.saturating_sub(scanned);
        if budget == 0 {
            break;
        }
        let take = n.min(budget);
        let mut window = carry.clone();
        window.extend_from_slice(&chunk[..take]);
        scanned += n;
        if !found_arm64 {
            found_arm64 = ARM64_MARKERS.iter().any(|m| find(&window, m));
        }
        if !found_x86 {
            found_x86 = X86_MARKERS.iter().any(|m| find(&window, m));
        }
        let start = window.len().saturating_sub(CARRY);
        carry = window[start..].to_vec();
    }

    // Multi-arch media (both loaders present) is distinct: the host
    // firmware boots its own loader, so callers treat Both as viable.
    match (found_arm64, found_x86) {
        (true, true) => IsoArch::Both,
        (true, false) => IsoArch::Aarch64,
        (false, true) => IsoArch::X8664,
        (false, false) => IsoArch::Unknown,
    }
}

/// Plain substring search — no memchr dependency needed at these sizes.
fn find(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_iso(contents: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "virt-iso-test-{}-{}.iso",
            std::process::id(),
            contents.len()
        ));
        let mut f = File::create(&path).unwrap();
        f.write_all(contents).unwrap();
        path
    }

    #[test]
    fn detects_arm64() {
        let iso = write_temp_iso(b"padding... /EFI/BOOT/BOOTAA64.EFI;1 ...more");
        assert_eq!(detect(&iso), IsoArch::Aarch64);
        let _ = std::fs::remove_file(&iso);
    }

    #[test]
    fn detects_arm64_grub_tree() {
        let iso = write_temp_iso(b"EFI/BOOT/GRUBAA64.EFI;1");
        assert_eq!(detect(&iso), IsoArch::Aarch64);
        let _ = std::fs::remove_file(&iso);
    }

    /// GRUB ships `file`-command help text for every architecture on all
    /// builds ("is-arm64-efi — Check if FILE is ARM64 EFI file").
    /// Filename markers must not be fooled by it.
    #[test]
    fn help_text_does_not_fool_detector() {
        let iso = write_temp_iso(
            b"is-arm64-efi.Check if FILE is ARM64 EFI file. ... /EFI/BOOT/BOOTX64.EFI;1",
        );
        assert_eq!(detect(&iso), IsoArch::X8664);
        let _ = std::fs::remove_file(&iso);
    }

    #[test]
    fn detects_x86() {
        let iso = write_temp_iso(b"padding... boot/grub/x86_64-efi/acpi.mod ... BOOTX64.EFI;1");
        assert_eq!(detect(&iso), IsoArch::X8664);
        let _ = std::fs::remove_file(&iso);
    }

    #[test]
    fn unknown_when_no_markers() {
        let iso = write_temp_iso(b"nothing recognizable here");
        assert_eq!(detect(&iso), IsoArch::Unknown);
        let _ = std::fs::remove_file(&iso);
    }

    /// Regression: a Rocky x86_64 DVD contains the stray string
    /// "aa64.efi" ~10 GB deep (grub2-efi-aa64 RPM payload text). The
    /// detector must not let package text flip an x86 image to arm64.
    #[test]
    fn stray_package_text_does_not_flip_verdict() {
        let mut contents = b"BOOTX64.EFI;1 ".to_vec();
        contents.extend(std::iter::repeat_n(b'x', 1 << 20));
        contents.extend_from_slice(b" grub2-efi-aa64 provides aa64.efi");
        let iso = write_temp_iso(&contents);
        assert_eq!(detect(&iso), IsoArch::X8664);
        let _ = std::fs::remove_file(&iso);
    }

    /// Markers split across the 8 MB chunk boundary must still match
    /// (the carry window exists for exactly this).
    #[test]
    fn marker_split_across_chunk_boundary_matches() {
        let mut contents = vec![b'p'; (8 << 20) - 5];
        contents.extend_from_slice(b"BOOTX64.EFI;1");
        let iso = write_temp_iso(&contents);
        assert_eq!(detect(&iso), IsoArch::X8664);
        let _ = std::fs::remove_file(&iso);
    }

    #[test]
    fn multi_arch_media_reports_both() {
        let iso = write_temp_iso(b"/EFI/BOOT/BOOTAA64.EFI;1 ... BOOTX64.EFI;1");
        assert_eq!(detect(&iso), IsoArch::Both);
        let _ = std::fs::remove_file(&iso);
    }

    /// A hand-built primary-volume-descriptor at sector 16 must yield
    /// its label; non-ISO9660 files yield None.
    #[test]
    fn volume_label_parses() {
        let mut image = vec![0u8; 17 * 2048];
        let sector = &mut image[16 * 2048..];
        sector[1..6].copy_from_slice(b"CD001");
        sector[40..52].copy_from_slice(b"Rocky-10.2  ");
        let path = std::env::temp_dir().join(format!("virt-iso-label-{}.bin", std::process::id()));
        std::fs::write(&path, &image).unwrap();
        assert_eq!(volume_label(&path).as_deref(), Some("Rocky-10.2"));
        let _ = std::fs::remove_file(&path);

        let junk = write_temp_iso(b"not an iso at all");
        assert_eq!(volume_label(&junk), None);
        let _ = std::fs::remove_file(&junk);
    }
}
