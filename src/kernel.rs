//! Kernel image classification and decompression for direct boot.
//!
//! Port of virt-macos's KernelImage/KernelDecompressor, adapted for
//! QEMU: x86_64 bzImages (magic `HdrS` at 514) are self-extracting and
//! boot as-is; aarch64 guests need the uncompressed Image header
//! (magic `ARM\\x64` at offset 56, which survives even when an EFI
//! stub prefixes the file with "MZ"). Distro arm64 kernels are usually
//! gzip-compressed zboot images: PE/COFF (machine 0xAA64) whose
//! payload — including the magic — is inside the gzip stream.
//!
//! Decompression finds the gzip stream by magic bytes (the kernel's
//! own scripts/extract-vmlinux approach): it sits at an arbitrary
//! offset inside the PE/zboot wrapper, followed by trailing data.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelFormat {
    /// x86_64 bzImage — boots via `-kernel` as-is.
    X86BzImage,
    /// Uncompressed arm64 Image — boots via `-kernel` as-is.
    Aarch64Raw,
    /// arm64 zboot or plain gzip — must be decompressed before boot.
    Aarch64Compressed,
    /// PE/COFF with x86_64 machine type.
    X86Pe,
    Unknown,
}

pub fn classify(path: &Path) -> KernelFormat {
    let Ok(header) = read_header(path) else {
        return KernelFormat::Unknown;
    };

    // bzImage magic "HdrS" at offset 514 (Linux boot protocol).
    if header.len() >= 518 && &header[514..518] == b"HdrS" {
        return KernelFormat::X86BzImage;
    }
    // arm64 raw Image magic "ARM\x64" at offset 56.
    if header.len() >= 60 && &header[56..60] == b"ARMd" {
        return KernelFormat::Aarch64Raw;
    }
    if header.first() == Some(&0x4D) && header.get(1) == Some(&0x5A) {
        // "MZ" — PE/COFF wrapper.
        if let Some(pe) = pe_machine(&header) {
            return match pe {
                0xAA64 => KernelFormat::Aarch64Compressed,
                0x8664 => KernelFormat::X86Pe,
                _ => KernelFormat::Unknown,
            };
        }
    }
    // Plain gzip: decompress and reclassify.
    if find_gzip(&header).is_some() {
        return KernelFormat::Aarch64Compressed;
    }
    KernelFormat::Unknown
}

fn read_header(path: &Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut header = vec![0u8; 4096];
    let n = file.read(&mut header)?;
    header.truncate(n);
    Ok(header)
}

/// PE machine type, if the MZ header points at a valid PE signature.
fn pe_machine(header: &[u8]) -> Option<u16> {
    if header.len() < 0x3C + 4 {
        return None;
    }
    let pe_offset =
        u32::from_le_bytes([header[0x3C], header[0x3D], header[0x3E], header[0x3F]]) as usize;
    if pe_offset + 6 > header.len() || &header[pe_offset..pe_offset + 4] != b"PE\0\0" {
        return None;
    }
    Some(u16::from_le_bytes([
        header[pe_offset + 4],
        header[pe_offset + 5],
    ]))
}

fn find_gzip(data: &[u8]) -> Option<usize> {
    data.windows(3).position(|w| w == [0x1F, 0x8B, 0x08])
}

/// Decompress the gzip stream embedded at an arbitrary offset in
/// `path` into `out`. Trailing data after the stream is ignored
/// (gunzip -c semantics).
pub fn decompress_gzip(path: &Path, out: &Path) -> Result<PathBuf> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let offset = find_gzip(&data).context("no gzip stream found in kernel image")?;
    let mut decoder = flate2::read::GzDecoder::new(&data[offset..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .context("gzip decompression failed")?;
    anyhow::ensure!(
        !decompressed.is_empty(),
        "gzip stream in {} decompressed to nothing",
        path.display()
    );
    let mut file =
        std::fs::File::create(out).with_context(|| format!("cannot create {}", out.display()))?;
    file.write_all(&decompressed)
        .with_context(|| format!("cannot write {}", out.display()))?;
    Ok(out.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(bytes: &[u8], name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("virt-kernel-test-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn make_raw_arm64() -> Vec<u8> {
        let mut bytes = vec![0u8; 1024];
        bytes[56..60].copy_from_slice(&[0x41, 0x52, 0x4D, 0x64]); // "ARMd"
        bytes
    }

    fn make_pe(machine: u16) -> Vec<u8> {
        let mut bytes = vec![0u8; 1024];
        bytes[0] = 0x4D;
        bytes[1] = 0x5A; // "MZ"
        bytes[0x3C] = 0x40; // PE header at 0x40
        bytes[0x40] = 0x50;
        bytes[0x41] = 0x45; // "PE"
        bytes[0x44] = (machine & 0xFF) as u8;
        bytes[0x45] = (machine >> 8) as u8;
        bytes
    }

    fn make_bzimage() -> Vec<u8> {
        let mut bytes = vec![0u8; 1024];
        bytes[514..518].copy_from_slice(b"HdrS");
        bytes
    }

    fn gzipped(payload: &[u8], prefix: &[u8]) -> Vec<u8> {
        use flate2::write::GzEncoder;
        use std::io::Write as _;
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(payload).unwrap();
        let mut out = prefix.to_vec();
        out.extend_from_slice(&enc.finish().unwrap());
        // Trailing garbage after the stream, like a real zboot wrapper.
        out.extend_from_slice(b"TRAILING-DATA");
        out
    }

    #[test]
    fn raw_arm64_image() {
        let f = write_temp(&make_raw_arm64(), "raw.img");
        assert_eq!(classify(&f), KernelFormat::Aarch64Raw);
    }

    /// Uncompressed kernels with an EFI stub have both "MZ" and the
    /// raw magic — the raw magic wins (checked before PE).
    #[test]
    fn pe_with_raw_magic_is_bootable() {
        let mut bytes = make_raw_arm64();
        bytes[0] = 0x4D;
        bytes[1] = 0x5A;
        let f = write_temp(&bytes, "pe-raw.img");
        assert_eq!(classify(&f), KernelFormat::Aarch64Raw);
    }

    #[test]
    fn pe_aa64_is_compressed_arm64() {
        let f = write_temp(&make_pe(0xAA64), "pe-aa64.img");
        assert_eq!(classify(&f), KernelFormat::Aarch64Compressed);
    }

    #[test]
    fn pe_x86() {
        let f = write_temp(&make_pe(0x8664), "pe-x86.img");
        assert_eq!(classify(&f), KernelFormat::X86Pe);
    }

    #[test]
    fn bzimage() {
        let f = write_temp(&make_bzimage(), "bzimage");
        assert_eq!(classify(&f), KernelFormat::X86BzImage);
    }

    #[test]
    fn plain_gzip_classifies_compressed() {
        let f = write_temp(&gzipped(&make_raw_arm64(), b""), "image.gz");
        assert_eq!(classify(&f), KernelFormat::Aarch64Compressed);
    }

    /// The full import path for a zboot kernel: PE wrapper with an
    /// embedded gzip stream containing the raw arm64 Image.
    #[test]
    fn zboot_roundtrip_decompress_and_reclassify() {
        let wrapper = gzipped(&make_raw_arm64(), &make_pe(0xAA64));
        let f = write_temp(&wrapper, "zboot.img");
        assert_eq!(classify(&f), KernelFormat::Aarch64Compressed);
        let out = std::env::temp_dir().join(format!("virt-kernel-out-{}", std::process::id()));
        decompress_gzip(&f, &out).unwrap();
        assert_eq!(classify(&out), KernelFormat::Aarch64Raw);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn unknown_when_no_markers() {
        let f = write_temp(b"just some data", "junk");
        assert_eq!(classify(&f), KernelFormat::Unknown);
    }
}
