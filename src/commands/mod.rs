//! Command dispatch.

pub mod clone;
pub mod completions;
pub mod console_enable;
pub mod create;
pub mod delete;
pub mod doctor;
pub mod install;
pub mod kernel_import;
pub mod list;
pub mod set;
pub mod start;
pub mod stop;

use crate::cli::Command;
use anyhow::Result;

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Create(args) => create::run(&args),
        Command::Clone(args) => clone::run(&args),
        Command::Install(args) => install::run(&args),
        Command::Start(args) => start::run(&args),
        Command::Stop(args) => stop::run(&args),
        Command::Delete(args) => delete::run(&args),
        Command::List => list::run(),
        Command::Set(args) => set::run(&args),
        Command::Doctor => doctor::run(),
        Command::ConsoleEnable(args) => console_enable::run(&args.name),
        Command::KernelImport(args) => kernel_import::run(&args),
        Command::Completions(args) => completions::run(&args.shell),
    }
}
