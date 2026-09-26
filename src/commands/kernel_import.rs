//! `virt kernel-import` — import a kernel and initrd for direct boot
//! (no EFI/GRUB). Port of virt-macos's KernelImport with QEMU-adapted
//! classification: x86_64 hosts accept self-extracting bzImages as-is;
//! aarch64 hosts need the raw Image (gzip/zboot wrappers are extracted
//! automatically, in-process, at import time).

use crate::cli::KernelImport;
use crate::config::VmConfig;
use crate::kernel::{self, KernelFormat};
use crate::logger;
use crate::qemu::arch::Arch;
use crate::vmdir::VmDir;
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

pub fn run(args: &KernelImport) -> Result<()> {
    let dir = VmDir::new(&args.name);
    if !dir.exists() {
        bail!("VM '{}' does not exist.", args.name);
    }
    let root = args.root.clone().unwrap_or_else(|| "/dev/vda2".into());
    import(
        &dir,
        Path::new(&args.from),
        &root,
        args.kernel_args.as_deref(),
    )?;

    println!("Imported kernel for direct boot:");
    println!(
        "  Kernel: {}",
        dir.kernel_path().file_name().unwrap().to_string_lossy()
    );
    println!("  Initrd: present");
    println!("  Root:   {root}");
    println!(
        "'virt start {}' now boots directly — console output in ~1s, no GRUB setup needed.",
        args.name
    );
    Ok(())
}

fn import(dir: &VmDir, from: &Path, root: &str, extra: Option<&str>) -> Result<()> {
    let kernel_src = find_file(from, "vmlinuz", &["vmlinuz-"], "kernel")?;
    let initrd_src = find_file(from, "initrd.img", &["initrd.img-", "initramfs-"], "initrd")?;

    let host = Arch::host();
    let (kernel_final, was_decompressed, label) = prepare_kernel(&kernel_src, host)
        .with_context(|| format!("classifying {}", kernel_src.display()))?;

    // Replace-on-import, like virt-macos.
    copy_replacing(&kernel_final, &dir.kernel_path())?;
    copy_replacing(&initrd_src, &dir.initrd_path())?;
    if was_decompressed {
        let _ = std::fs::remove_file(&kernel_final);
    }

    let mut config = VmConfig::load(&dir.config_path())?;
    config.root_device = Some(root.to_string());
    config.extra_kernel_args = extra.map(str::to_string);
    config.write(&dir.config_path())?;

    logger::log(
        dir,
        &format!("kernel imported ({label}, root={root}, decompressed={was_decompressed})"),
    );
    Ok(())
}

/// Classify against the host arch, decompressing when needed. Returns
/// the path to the bootable kernel (possibly a temp file), whether it
/// was decompressed, and a short label for the log.
fn prepare_kernel(src: &Path, host: Arch) -> Result<(PathBuf, bool, String)> {
    let name = src.file_name().unwrap_or_default().to_string_lossy();
    let host_label = match host {
        Arch::X86_64 => "x86_64",
        Arch::Aarch64 => "aarch64",
    };

    let bootable = |f: KernelFormat| {
        matches!(
            (host, f),
            (Arch::X86_64, KernelFormat::X86BzImage | KernelFormat::X86Pe)
                | (Arch::Aarch64, KernelFormat::Aarch64Raw)
        )
    };
    let foreign = |f: KernelFormat| {
        matches!(
            (host, f),
            (
                Arch::X86_64,
                KernelFormat::Aarch64Raw | KernelFormat::Aarch64Compressed
            ) | (
                Arch::Aarch64,
                KernelFormat::X86BzImage | KernelFormat::X86Pe
            )
        )
    };

    let classified = kernel::classify(src);
    if bootable(classified) {
        return Ok((src.to_path_buf(), false, format!("{name} ({host_label})")));
    }
    if foreign(classified) {
        bail!(
            "{name} is a {} kernel — this host boots {host_label} guests.",
            foreign_label(classified)
        );
    }

    // Compressed or unknown: extract the gzip stream and reclassify.
    let tmp = std::env::temp_dir().join(format!(
        "virt-kernel-import-{}-{}",
        std::process::id(),
        name
    ));
    let decompressed = kernel::decompress_gzip(src, &tmp)
        .with_context(|| format!("{name} is not a recognized {host_label} kernel (bzImage, raw Image, or gzip/zboot). Files swapped?"))?;
    let reclassified = kernel::classify(&decompressed);
    ensure!(
        bootable(reclassified),
        "{name} did not decompress into a bootable {host_label} kernel"
    );
    Ok((
        decompressed,
        true,
        format!("{name} (decompressed, {host_label})"),
    ))
}

fn foreign_label(f: KernelFormat) -> &'static str {
    match f {
        KernelFormat::X86BzImage | KernelFormat::X86Pe => "x86_64",
        KernelFormat::Aarch64Raw | KernelFormat::Aarch64Compressed => "aarch64",
        KernelFormat::Unknown => "unrecognized-arch",
    }
}

/// Locate a kernel/initrd file: prefer the exact name, else the newest
/// versioned match (string sort, like virt-macos).
fn find_file(dir: &Path, exact: &str, prefixes: &[&str], kind: &str) -> Result<PathBuf> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    if entries.iter().any(|e| e == exact) {
        return Ok(dir.join(exact));
    }
    let versioned = entries
        .iter()
        .filter(|name| prefixes.iter().any(|p| name.starts_with(p)))
        .max()
        .cloned();
    if let Some(versioned) = versioned {
        return Ok(dir.join(versioned));
    }
    bail!(
        "No {kind} found in {} — expected {exact} or {}*",
        dir.display(),
        prefixes.first().unwrap_or(&"")
    )
}

fn copy_replacing(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        std::fs::remove_file(dst).with_context(|| format!("cannot remove {}", dst.display()))?;
    }
    std::fs::copy(src, dst)
        .with_context(|| format!("cannot copy {} -> {}", src.display(), dst.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("virt-kimport-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A fake VM dir plus a share dir holding a bzImage + initrd; the
    /// import must copy both and stamp the config.
    #[test]
    fn import_bzimage_and_initrd() {
        let share = tempdir("share");
        let vmdir = tempdir("vm");
        let mut kernel = vec![0u8; 1024];
        kernel[514..518].copy_from_slice(b"HdrS");
        std::fs::write(share.join("vmlinuz-6.8.0"), &kernel).unwrap();
        std::fs::write(share.join("initrd.img-6.8.0"), b"initramfs").unwrap();
        std::fs::write(
            vmdir.join("config.json"),
            r#"{"name":"t","cpus":1,"memoryMB":512,"diskSizeGB":1}"#,
        )
        .unwrap();

        let dir = VmDir {
            name: "t".into(),
            root: vmdir.clone(),
        };
        import(&dir, &share, "/dev/vda1", Some("quiet")).unwrap();

        assert!(dir.kernel_path().exists());
        assert!(dir.initrd_path().exists());
        let cfg = VmConfig::load(&dir.config_path()).unwrap();
        assert_eq!(cfg.root_device.as_deref(), Some("/dev/vda1"));
        assert_eq!(cfg.extra_kernel_args.as_deref(), Some("quiet"));
        assert!(dir.kernel_path().metadata().unwrap().len() > 0);

        let _ = std::fs::remove_dir_all(&share);
        let _ = std::fs::remove_dir_all(&vmdir);
    }

    #[test]
    fn find_file_prefers_exact_then_newest_versioned() {
        let d = tempdir("find");
        std::fs::write(d.join("vmlinuz-6.1.0"), b"a").unwrap();
        std::fs::write(d.join("vmlinuz-6.10.0"), b"b").unwrap();
        let found = find_file(&d, "vmlinuz", &["vmlinuz-"], "kernel").unwrap();
        assert_eq!(found, d.join("vmlinuz-6.10.0"));
        std::fs::write(d.join("vmlinuz"), b"c").unwrap();
        let found = find_file(&d, "vmlinuz", &["vmlinuz-"], "kernel").unwrap();
        assert_eq!(found, d.join("vmlinuz"));
        assert!(find_file(&d, "missing", &["nope-"], "x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An arm64 zboot kernel on an x86_64 host must be rejected with a
    /// clear message (unless this test runs on aarch64, where it is the
    /// happy path — branch on host arch so the test is portable).
    #[test]
    fn arch_mismatch_rejected() {
        let d = tempdir("mismatch");
        // PE wrapper with embedded gzip of a raw arm64 Image.
        let mut raw = vec![0u8; 1024];
        raw[56..60].copy_from_slice(&[0x41, 0x52, 0x4D, 0x64]);
        let mut pe = vec![0u8; 1024];
        pe[0] = 0x4D;
        pe[1] = 0x5A;
        pe[0x3C] = 0x40;
        pe[0x40] = 0x50;
        pe[0x41] = 0x45;
        pe[0x44] = 0x64;
        pe[0x45] = 0xAA;
        use flate2::write::GzEncoder;
        use std::io::Write as _;
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        pe.extend_from_slice(&enc.finish().unwrap());
        std::fs::write(d.join("vmlinuz"), &pe).unwrap();

        match Arch::host() {
            Arch::X86_64 => {
                let err = prepare_kernel(&d.join("vmlinuz"), Arch::X86_64).unwrap_err();
                assert!(err.to_string().contains("aarch64"), "{err}");
            }
            Arch::Aarch64 => {
                let (path, decompressed, _) =
                    prepare_kernel(&d.join("vmlinuz"), Arch::Aarch64).unwrap();
                assert!(decompressed);
                assert!(path.exists());
                let _ = std::fs::remove_file(&path);
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
