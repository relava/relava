use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "relava", about = "A local package manager for Claude Code prompt-layer artifacts")]
pub struct Cli {
    /// Override server URL (default: http://localhost:7420)
    #[arg(long, global = true, default_value = "http://localhost:7420")]
    pub server: String,

    /// Override project directory (default: current working directory)
    #[arg(long, global = true)]
    pub project: Option<String>,

    /// Show detailed output
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Output as JSON (for scripting)
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize current directory as a Relava-managed project
    Init,

    /// Install a resource into the current project
    Install {
        /// Resource type (skill, agent, command, rule) or path to relava.toml
        resource_type: String,

        /// Resource name
        name: Option<String>,

        /// Version to install
        #[arg(long)]
        version: Option<String>,

        /// Save to relava.toml
        #[arg(long)]
        save: bool,

        /// Install globally to ~/.claude/
        #[arg(long)]
        global: bool,
    },

    /// Remove a resource from the current project
    Remove {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,

        /// Also remove from relava.toml
        #[arg(long)]
        save: bool,
    },

    /// List installed resources
    List {
        /// Resource type (skills, agents, commands, rules)
        resource_type: String,

        /// List globally installed resources
        #[arg(long)]
        global: bool,
    },

    /// Show resource details
    Info {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,
    },

    /// Search for resources in the registry
    Search {
        /// Search query
        query: String,
    },

    /// Update installed resources
    Update {
        /// Resource type
        resource_type: Option<String>,

        /// Resource name
        name: Option<String>,

        /// Update all installed resources
        #[arg(long)]
        all: bool,
    },

    /// Publish a resource to the local registry
    Publish {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,

        /// Custom source directory
        #[arg(long)]
        path: Option<String>,
    },

    /// Resolve and display the dependency tree
    Resolve {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,
    },

    /// Manage the local registry server
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },

    /// Check health of Relava installation and project
    Doctor,

    /// Import an existing resource directory into the registry
    Import {
        /// Resource type
        resource_type: String,

        /// Path to resource directory
        path: String,
    },

}

#[derive(Subcommand)]
pub enum ServerAction {
    /// Start the local registry server
    Start {
        /// Port to listen on
        #[arg(long, default_value = "7420")]
        port: u16,

        /// Run as background daemon
        #[arg(long)]
        daemon: bool,
    },

    /// Stop the running server
    Stop,

    /// Show server status
    Status,
}
