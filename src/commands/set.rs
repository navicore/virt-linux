//! `virt set` — update VM settings.

use crate::cli::Set;
use crate::config::VmConfig;
use crate::vmdir::VmDir;
use anyhow::{Result, bail};

pub fn run(args: &Set) -> Result<()> {
    let dir = VmDir::new(&args.name);
    if !dir.exists() {
        bail!("VM '{}' does not exist.", args.name);
    }

    let config = VmConfig::load(&dir.config_path())?;
    config
        .with_description(&args.description)
        .write(&dir.config_path())?;

    println!(
        "Updated '{}': description = {}",
        args.name, args.description
    );
    Ok(())
}
