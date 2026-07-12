mod cli;
mod commands;
mod hook;
mod notify;
mod process;
mod sidebar;
mod state;
mod status;
mod tmux;

use clap::Parser;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Command::Sidebar => run(sidebar::run()),
        cli::Command::Toggle => run(commands::toggle::toggle()),
        cli::Command::Open => run(commands::toggle::open()),
        cli::Command::Close => run(commands::toggle::close()),
        cli::Command::Poke => {
            // Fired from tmux hooks on every pane/window event: never fail, never print.
            let _ = notify::poke();
            ExitCode::SUCCESS
        }
        cli::Command::Hook { agent, event } => {
            // Hook handlers must never break the calling agent: always exit 0.
            commands::hook::handle(&agent, &event);
            ExitCode::SUCCESS
        }
        cli::Command::Report {
            status,
            message,
            name,
            pane,
        } => run(commands::report::report(status, message, name, pane)),
        cli::Command::List { json } => run(commands::list::list(json)),
        cli::Command::Spawn {
            name,
            direction,
            window,
            cwd,
            focus,
            command,
        } => run(commands::spawn::spawn(
            name, direction, window, cwd, focus, command,
        )),
        cli::Command::Wait {
            pane,
            status,
            timeout,
        } => commands::wait::wait(pane, status, timeout),
    }
}

fn run(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tmux-legion: {err:#}");
            ExitCode::FAILURE
        }
    }
}
