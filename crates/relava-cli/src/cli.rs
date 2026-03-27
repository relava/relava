use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "relava",
    about = "A local package manager for Claude Code prompt-layer artifacts"
)]
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

    /// Suppress automatic update availability check
    #[arg(long, global = true)]
    pub no_update_check: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize current directory as a Relava-managed project
    Init,

    /// Install a resource into the current project
    ///
    /// With no arguments, installs all resources declared in relava.toml.
    /// With `relava.toml` as the argument, same as no arguments.
    /// With `<type> <name>`, installs a single resource.
    Install {
        /// Resource type (skill, agent, command, rule) or "relava.toml" for bulk install
        resource_type: Option<String>,

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

        /// Auto-accept tool install prompts
        #[arg(long, short = 'y')]
        yes: bool,
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
        /// Resource type (skill, agent, command, rule); omit to list all
        resource_type: Option<String>,
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

        /// Filter by resource type (skill, agent, command, rule)
        #[arg(long, rename_all = "lowercase")]
        r#type: Option<String>,
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

        /// Skip change detection and publish regardless
        #[arg(long)]
        force: bool,

        /// Auto-confirm publish prompt (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Resolve and display the dependency tree
    Resolve {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,

        /// Version to resolve (default: latest)
        #[arg(long)]
        version: Option<String>,
    },

    /// Manage the local registry server
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },

    /// Check health of Relava installation and project
    Doctor,

    /// Disable an installed resource (moves to .disabled/ subdirectory)
    Disable {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,
    },

    /// Enable a disabled resource (restores from .disabled/ subdirectory)
    Enable {
        /// Resource type
        resource_type: String,

        /// Resource name
        name: String,
    },

    /// Import an existing resource directory into the registry
    Import {
        /// Resource type
        resource_type: String,

        /// Path to resource directory or file
        path: String,

        /// Version to publish (default: from frontmatter or 1.0.0)
        #[arg(long)]
        version: Option<String>,
    },

    /// Validate a resource offline before publishing
    Validate {
        /// Resource type
        resource_type: String,

        /// Path to resource directory or file
        path: String,
    },

    /// Manage the download cache
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
pub enum CacheAction {
    /// Remove cached downloads
    Clean {
        /// Only remove entries older than this duration (e.g. 30d, 12h, 45m)
        #[arg(long)]
        older_than: Option<String>,
    },

    /// Show cache disk usage and entry summary
    Status,
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

        /// Directory to serve static GUI files from (default: ~/.relava/gui/)
        #[arg(long)]
        gui_dir: Option<String>,
    },

    /// Stop the running server
    Stop,

    /// Show server status
    Status,
}
