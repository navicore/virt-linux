//! Command dispatch.

pub mod completions;
pub mod create;
pub mod delete;
pub mod doctor;
pub mod list;
pub mod set;
pub mod stubs;

use crate::cli::Command;
use anyhow::Result;

pub fn run(command: Command) -> Result<()> {
    match command {
        Command::Create(args) => create::run(&args),
        Command::Install(args) => stubs::install(&args.name),
        Command::Start(args) => stubs::start(&args.name),
        Command::Stop(args) => stubs::stop(&args.name),
        Command::Delete(args) => delete::run(&args),
        Command::List => list::run(),
        Command::Set(args) => set::run(&args),
        Command::Doctor => doctor::run(),
        Command::KernelImport(args) => {
            stubs::kernel_import(&args.name, &args.from, args.root.as_deref())
        }
        Command::Completions(args) => completions::run(&args.shell),
    }
}
