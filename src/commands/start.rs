//! `virt start` — boot a VM headless with the console in this terminal.

use crate::cli::Start;
use crate::commands::console_enable;
use crate::config::VmConfig;
use crate::lock::VmLock;
use crate::supervisor;
use crate::vmdir::VmDir;
use anyhow::{Result, bail};

pub fn run(args: &Start) -> Result<()> {
    let dir = VmDir::new(&args.name);

    if !dir.exists() {
        bail!("VM '{}' does not exist.", args.name);
    }

    let config = VmConfig::load(&dir.config_path())?;

    // One-time offline console setup for installed guests (no-op once
    // enabled; must run before we hold the lock, while the disk is
    // still offline).
    console_enable::maybe_auto_enable(&dir, &config);

    // Held for the life of the process; released by the kernel on death.
    let _lock = VmLock::acquire(&dir)?;

    eprintln!("Starting VM '{}'...", args.name);
    eprintln!("  CPUs: {}, Memory: {} MB", config.cpus, config.memory_mb);
    match config.network_mode.as_deref() {
        Some("bridge") => eprintln!(
            "  Network: bridge{} — the VM is directly on the LAN",
            config
                .bridge_interface
                .as_ref()
                .map(|i| format!(" ({i})"))
                .unwrap_or_default()
        ),
        Some("lan") => eprintln!(
            "  Network: lan:{} — shared L2 with other '{}' VMs",
            config.lan_name.as_deref().unwrap_or("?"),
            config.lan_name.as_deref().unwrap_or("?")
        ),
        _ => {}
    }
    eprintln!("  Boot: {}", supervisor::boot_banner(&dir, &config));
    if !dir.kernel_path().exists()
        && matches!(
            supervisor::resolve_firmware(&dir, &config),
            crate::supervisor::Firmware::Efi
        )
        && config.console_enabled != Some(true)
    {
        eprintln!("        EFI boots are SILENT until the guest's kernel uses the serial");
        eprintln!("        console. One-time fix, in the guest:");
        eprintln!("          grubby --update-kernel=ALL --args=\"console=ttyS0\"");
        eprintln!("          (RHEL/Rocky/Fedora; Debian/Ubuntu: edit grub.cfg)");
    }
    eprintln!(
        "  Console attached. Use 'virt stop {}' to shut down.",
        args.name
    );

    if let Some(share) = &args.share {
        eprintln!("  Shared folder: {share} (mount in guest: mount -t virtiofs share /mnt)");
    }

    let share = args.share.as_deref().map(std::path::Path::new);
    supervisor::run_headless(&config, &dir, share)
}
