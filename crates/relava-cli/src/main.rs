mod cli;
mod init;

use clap::Parser;
use cli::{Cli, Command, ServerAction};

fn main() {
    let cli = Cli::parse();

    if cli.verbose {
        eprintln!("server: {}", cli.server);
        if let Some(ref project) = cli.project {
            eprintln!("project: {}", project);
        }
    }

    match cli.command {
        Command::Init => {
            let project_dir = cli
                .project
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().expect("cannot determine current directory")
                });
            if let Err(msg) = init::run(&project_dir) {
                eprintln!("{msg}");
                std::process::exit(1);
            }
        }
        Command::Install {
            resource_type,
            name,
            ..
        } => {
            println!(
                "relava install {resource_type} {}",
                name.unwrap_or_default()
            );
        }
        Command::Remove {
            resource_type,
            name,
            ..
        } => {
            println!("relava remove {resource_type} {name}");
        }
        Command::List { resource_type, .. } => {
            println!("relava list {resource_type}");
        }
        Command::Info {
            resource_type,
            name,
        } => {
            println!("relava info {resource_type} {name}");
        }
        Command::Search { query } => {
            println!("relava search {query}");
        }
        Command::Update {
            resource_type,
            name,
            all,
        } => {
            if all {
                println!("relava update --all");
            } else {
                println!(
                    "relava update {} {}",
                    resource_type.unwrap_or_default(),
                    name.unwrap_or_default()
                );
            }
        }
        Command::Publish {
            resource_type,
            name,
            ..
        } => {
            println!("relava publish {resource_type} {name}");
        }
        Command::Resolve {
            resource_type,
            name,
        } => {
            println!("relava resolve {resource_type} {name}");
        }
        Command::Server { action } => match action {
            ServerAction::Start { port, daemon } => {
                println!("relava server start --port {port} --daemon={daemon}");
            }
            ServerAction::Stop => {
                println!("relava server stop");
            }
            ServerAction::Status => {
                println!("relava server status");
            }
        },
        Command::Doctor => {
            println!("relava doctor");
        }
        Command::Import {
            resource_type,
            path,
        } => {
            println!("relava import {resource_type} {path}");
        }
    }
}
