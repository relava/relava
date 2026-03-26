mod cache;
mod cli;
mod env_check;
mod init;
mod install;
mod registry;
mod tools;

use clap::Parser;
use cli::{Cli, Command, ServerAction};

/// Resolve the project directory from `--project` flag or current working directory.
fn resolve_project_dir(project_flag: Option<&str>) -> std::path::PathBuf {
    match project_flag {
        Some(p) => std::path::PathBuf::from(p),
        None => std::env::current_dir().unwrap_or_else(|e| {
            eprintln!("cannot determine current directory: {e}");
            std::process::exit(1);
        }),
    }
}

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
            let project_dir = resolve_project_dir(cli.project.as_deref());
            if let Err(msg) = init::run(&project_dir) {
                eprintln!("{msg}");
                std::process::exit(1);
            }
        }
        Command::Install {
            resource_type,
            name,
            version,
            save: _save,
            global,
            yes,
        } => {
            let Some(name) = name else {
                eprintln!("missing resource name. Usage: relava install <type> <name>");
                std::process::exit(1);
            };

            let rt = match install::parse_resource_type(&resource_type) {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            };

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = install::InstallOpts {
                server_url: &cli.server,
                resource_type: rt,
                name: &name,
                version_pin: version.as_deref(),
                project_dir: &project_dir,
                global,
                json: cli.json,
                verbose: cli.verbose,
                yes,
            };

            match install::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        match serde_json::to_string_pretty(&result) {
                            Ok(json) => println!("{json}"),
                            Err(e) => {
                                eprintln!("failed to serialize result: {e}");
                                std::process::exit(1);
                            }
                        }
                    }
                }
                Err(e) => {
                    if cli.json {
                        let err_json = serde_json::json!({ "error": e });
                        match serde_json::to_string_pretty(&err_json) {
                            Ok(json) => println!("{json}"),
                            Err(se) => eprintln!("failed to serialize error: {se}: {e}"),
                        }
                    } else {
                        eprintln!("{e}");
                    }
                    std::process::exit(1);
                }
            }
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
