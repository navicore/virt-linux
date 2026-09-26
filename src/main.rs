use clap::Parser;
use virt_linux::cli::Cli;
use virt_linux::commands;

fn main() {
    let cli = Cli::parse();
    if let Err(e) = commands::run(cli.command) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
