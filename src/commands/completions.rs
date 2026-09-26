//! `virt completions` — shell completion generation.

use crate::cli::{Cli, Shell};
use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, shells};

pub fn run(shell: &Shell) -> Result<()> {
    let mut cmd = Cli::command();
    match shell {
        Shell::Bash => generate(shells::Bash, &mut cmd, "virt-linux", &mut std::io::stdout()),
        Shell::Zsh => generate(shells::Zsh, &mut cmd, "virt-linux", &mut std::io::stdout()),
        Shell::Fish => generate(shells::Fish, &mut cmd, "virt-linux", &mut std::io::stdout()),
    }
    Ok(())
}
