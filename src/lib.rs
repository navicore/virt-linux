//! virt-linux — manage Linux VMs on Linux hosts via QEMU/KVM.
//!
//! Rust sibling of virt-macos: same command surface, same per-VM
//! on-disk model (`~/.virt-linux/vms/<name>/` with `config.json`,
//! `disk.raw`, pid/lock/log files). Where virt-macos embeds Apple's
//! Virtualization.framework, this tool supervises a QEMU subprocess
//! per VM and talks to it over QMP. See docs/design/001-architecture.md.

pub mod cli;
pub mod commands;
pub mod config;
pub mod lock;
pub mod logger;
pub mod mac;
pub mod qemu;
pub mod qmp;
pub mod supervisor;
pub mod tty;
pub mod vmdir;
