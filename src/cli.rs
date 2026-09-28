//! CLI definition — the command surface mirrors virt-macos 1:1.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "virt",
    version,
    about = "Manage Linux VMs on Linux hosts via QEMU/KVM (Rust sibling of virt-macos)"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Create a new VM
    Create(Create),

    /// Clone a stopped VM under a new name (fresh MAC and guest identity)
    Clone(Clone),

    /// Boot VM with a GUI window for OS install
    Install(Install),

    /// Start a VM headless with console in terminal
    Start(Start),

    /// Shut down a running VM
    Stop(Stop),

    /// Delete a VM and its disk image
    Delete(Delete),

    /// List all VMs and their status
    List,

    /// Update VM settings
    Set(Set),

    /// Diagnose host setup (KVM, QEMU, firmware)
    Doctor,

    /// Enable the guest serial console by editing its bootloader
    /// offline (requires libguestfs-tools; VM must be stopped)
    #[command(name = "console-enable")]
    ConsoleEnable(ConsoleEnable),

    /// Import guest kernel for direct boot
    #[command(name = "kernel-import")]
    KernelImport(KernelImport),

    /// Generate shell completions
    Completions(Completions),
}

#[derive(clap::Args)]
pub struct Create {
    /// Name of the VM
    pub name: String,

    /// Short description of the VM's purpose
    #[arg(long)]
    pub description: String,

    /// Disk size in GB
    #[arg(long)]
    pub disk: u32,

    /// Memory in MB
    #[arg(long)]
    pub memory: u32,

    /// Number of CPU cores
    #[arg(long, default_value_t = 2)]
    pub cpus: u32,

    /// Network mode: nat (default), bridge, or lan:NAME (VMs sharing a
    /// lan name share one virtual L2 segment — k3s-style clusters)
    #[arg(long, default_value = "nat")]
    pub network: String,

    /// Host bridge for bridge mode
    #[arg(long)]
    pub bridge_interface: Option<String>,
}

#[derive(clap::Args)]
pub struct Clone {
    /// Name of the VM to clone (must be stopped)
    pub source: String,

    /// Name for the new VM
    pub new_name: String,

    /// Description for the clone (default: "clone of <source>")
    #[arg(long)]
    pub description: Option<String>,

    /// Skip guest identity reset (keeps template machine-id/host keys)
    #[arg(long)]
    pub no_sysprep: bool,
}

#[derive(clap::Args)]
pub struct Install {
    /// Name of the VM
    pub name: String,

    /// ISO image to install from (omit to re-enter an installed VM)
    #[arg(long)]
    pub iso: Option<String>,

    /// Host directory to share with the VM
    #[arg(long)]
    pub share: Option<String>,

    /// Do not spawn remote-viewer; print the SPICE URI for manual connect
    /// (headless hosts)
    #[arg(long)]
    pub no_viewer: bool,
}

#[derive(clap::Args)]
pub struct Start {
    /// Name of the VM
    pub name: String,

    /// Host directory to share with the VM
    #[arg(long)]
    pub share: Option<String>,
}

#[derive(clap::Args)]
pub struct Stop {
    /// Name of the VM
    pub name: String,
}

#[derive(clap::Args)]
pub struct Delete {
    /// Name of the VM
    pub name: String,

    /// Skip confirmation prompt
    #[arg(long, short)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct Set {
    /// Name of the VM
    pub name: String,

    /// Short description of the VM's purpose
    #[arg(long)]
    pub description: String,
}

#[derive(clap::Args)]
pub struct KernelImport {
    /// Name of the VM
    pub name: String,

    /// Directory containing kernel and initrd copied out of the guest
    #[arg(long)]
    pub from: String,

    /// Root device for direct kernel boot (default /dev/vda2)
    #[arg(long)]
    pub root: Option<String>,

    /// Extra kernel command-line arguments
    #[arg(long)]
    pub kernel_args: Option<String>,
}

#[derive(clap::Args)]
pub struct ConsoleEnable {
    /// Name of the VM
    pub name: String,
}

#[derive(clap::Args)]
pub struct Completions {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(clap::ValueEnum, Clone)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}
