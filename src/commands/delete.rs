//! `virt delete` — remove a VM directory and all its data.

use crate::cli::Delete;
use crate::lock::VmLock;
use crate::vmdir::VmDir;
use anyhow::{Result, bail};

pub fn run(args: &Delete) -> Result<()> {
    let dir = VmDir::new(&args.name);
    if !dir.exists() {
        bail!("VM '{}' does not exist.", args.name);
    }

    // Refuse to delete a running VM (lock is authoritative, not the PID
    // file).
    if VmLock::is_locked(&dir) {
        let detail = dir.pid().map(|p| format!(" (PID {p})")).unwrap_or_default();
        bail!("VM '{}' is running{detail}. Stop it first.", args.name);
    }

    if !args.force {
        print!("Delete VM '{}' and all its data? [y/N] ", args.name);
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut response = String::new();
        std::io::stdin().read_line(&mut response)?;
        if response.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    dir.remove()?;
    println!("Deleted VM '{}'.", args.name);
    Ok(())
}
