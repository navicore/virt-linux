use clap::Parser;
use virt_linux::cli::Cli;
use virt_linux::commands;

fn main() {
    // Rust ignores SIGPIPE by default, so writes to a closed pipe
    // (e.g. `virt-linux start vm | head`) panic instead of dying
    // cleanly like every other Unix tool. Restore the default.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    if let Err(e) = commands::run(cli.command) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
