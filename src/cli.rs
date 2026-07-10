use crate::status::Status;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "tmux-legion", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the sidebar TUI (internal: executed inside the sidebar pane)
    Sidebar,
    /// Toggle the sidebar pane in the current window
    Toggle,
    /// Open the sidebar pane
    Open,
    /// Close the sidebar pane
    Close,
    /// Redraw the sidebar if it is running (internal: called from tmux hooks)
    Poke,
    /// Handle an agent platform hook event; payload JSON on stdin (internal)
    Hook {
        /// Agent platform (e.g. "claude")
        agent: String,
        /// Event name (e.g. "UserPromptSubmit")
        event: String,
    },
    /// Report this pane's agent status
    Report {
        /// New status
        #[arg(value_enum)]
        status: Status,
        /// Short human-readable detail shown in the sidebar
        #[arg(short, long)]
        message: Option<String>,
        /// Agent name shown in the sidebar (defaults to the existing name)
        #[arg(long)]
        name: Option<String>,
        /// Target pane id (defaults to $TMUX_PANE)
        #[arg(long)]
        pane: Option<String>,
    },
    /// List tracked agents
    List {
        /// Output JSON instead of a table
        #[arg(long)]
        json: bool,
    },
    /// Spawn a command in a new pane and track it as an agent
    Spawn {
        /// Agent name shown in the sidebar
        #[arg(long)]
        name: Option<String>,
        /// Split direction relative to the current pane
        #[arg(long, value_enum, default_value_t = Direction::Right)]
        direction: Direction,
        /// Open a new window instead of splitting
        #[arg(long)]
        window: bool,
        /// Working directory for the new pane
        #[arg(long)]
        cwd: Option<String>,
        /// Focus the new pane instead of staying in the current one
        #[arg(long)]
        focus: bool,
        /// Command to run
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Block until a pane's agent reaches a status (exit 0 = matched, 2 = timeout, 3 = pane gone)
    Wait {
        /// Pane id to watch (defaults to $TMUX_PANE)
        #[arg(long)]
        pane: Option<String>,
        /// Status to wait for
        #[arg(long, value_enum)]
        status: Status,
        /// Give up after this many seconds
        #[arg(long)]
        timeout: Option<u64>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Direction {
    Right,
    Down,
    Left,
    Up,
}
