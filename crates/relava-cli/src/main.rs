mod bulk_install;
mod cache;
mod cli;
mod disable;
mod doctor;
mod enable;
mod env_check;
mod import;
mod info;
mod init;
mod install;
mod list;
mod lockfile;
mod registry;
mod remove;
mod resolver;
mod save;
mod tools;
mod update;

#[cfg(test)]
mod lifecycle_tests;

use clap::Parser;
use cli::{Cli, Command, ServerAction};

/// Print a serializable value as pretty JSON. Exits on serialization failure.
fn print_json(value: &impl serde::Serialize) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("failed to serialize output: {e}");
            std::process::exit(1);
        }
    }
}

/// Print an error, formatting as JSON if `json` is true. Always exits with code 1.
fn exit_with_error(msg: &str, json: bool) -> ! {
    if json {
        print_json(&serde_json::json!({ "error": msg }));
    } else {
        eprintln!("{msg}");
    }
    std::process::exit(1);
}

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
            save,
            global,
            yes,
        } => {
            let project_dir = resolve_project_dir(cli.project.as_deref());

            // Determine if this is a bulk install (no args or "relava.toml")
            let is_bulk = matches!(resource_type.as_deref(), None | Some("relava.toml"));

            if is_bulk {
                // Guard against flags that don't apply to bulk install
                if name.is_some() {
                    exit_with_error(
                        "unexpected argument. Usage: relava install (or relava install relava.toml)",
                        cli.json,
                    );
                }
                if save {
                    eprintln!(
                        "warning: --save is ignored during bulk install (resources are already in relava.toml)"
                    );
                }
                if version.is_some() {
                    eprintln!(
                        "warning: --version is ignored during bulk install (versions come from relava.toml)"
                    );
                }

                let opts = bulk_install::BulkInstallOpts {
                    server_url: &cli.server,
                    project_dir: &project_dir,
                    global,
                    json: cli.json,
                    verbose: cli.verbose,
                    yes,
                };

                match bulk_install::run(&opts) {
                    Ok(result) => {
                        // Update lockfile for all successfully installed resources
                        if let Err(e) = lockfile::update_after_bulk_install(&project_dir, &result)
                            && !cli.json
                        {
                            eprintln!("[warn] failed to update relava.lock: {e}");
                        }
                        if cli.json {
                            print_json(&result);
                        }
                        if !result.failed.is_empty() {
                            std::process::exit(1);
                        }
                    }
                    Err(e) => exit_with_error(&e, cli.json),
                }
            } else {
                let resource_type_str = resource_type.unwrap(); // safe: is_bulk is false
                let Some(name) = name else {
                    eprintln!("missing resource name. Usage: relava install <type> <name>");
                    std::process::exit(1);
                };

                let rt = install::parse_resource_type(&resource_type_str)
                    .unwrap_or_else(|e| exit_with_error(&e, cli.json));

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
                        if save
                            && let Err(e) = save::add_to_manifest(
                                &project_dir,
                                rt,
                                &name,
                                &result.version,
                                cli.json,
                            )
                        {
                            exit_with_error(&e, cli.json);
                        }
                        // Update lockfile
                        if let Err(e) = lockfile::update_after_install(
                            &project_dir,
                            rt,
                            &name,
                            &result.version,
                            &result.dependencies,
                        ) && !cli.json
                        {
                            eprintln!("[warn] failed to update relava.lock: {e}");
                        }
                        if cli.json {
                            print_json(&result);
                        }
                    }
                    Err(e) => exit_with_error(&e, cli.json),
                }
            }
        }
        Command::Remove {
            resource_type,
            name,
            save,
        } => {
            let rt = install::parse_resource_type(&resource_type)
                .unwrap_or_else(|e| exit_with_error(&e, cli.json));

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = remove::RemoveOpts {
                resource_type: rt,
                name: &name,
                project_dir: &project_dir,
                json: cli.json,
                verbose: cli.verbose,
            };

            match remove::run(&opts) {
                Ok(result) => {
                    // Always run --save on remove: clean up manifest even
                    // if files were already gone from disk.
                    if save
                        && let Err(e) =
                            save::remove_from_manifest(&project_dir, rt, &name, cli.json)
                    {
                        exit_with_error(&e, cli.json);
                    }
                    // Update lockfile — remove entry and orphaned deps
                    if result.was_removed
                        && let Err(e) = lockfile::update_after_remove(&project_dir, rt, &name)
                        && !cli.json
                    {
                        eprintln!("[warn] failed to update relava.lock: {e}");
                    }
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
        Command::List { resource_type } => {
            let rt = resource_type.as_ref().map(|s| {
                install::parse_resource_type(s).unwrap_or_else(|e| exit_with_error(&e, cli.json))
            });

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = list::ListOpts {
                resource_type: rt,
                project_dir: &project_dir,
                json: cli.json,
                _verbose: cli.verbose,
            };

            match list::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
        Command::Info {
            resource_type,
            name,
        } => {
            let rt = install::parse_resource_type(&resource_type)
                .unwrap_or_else(|e| exit_with_error(&e, cli.json));

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = info::InfoOpts {
                resource_type: rt,
                name: &name,
                project_dir: &project_dir,
                json: cli.json,
                _verbose: cli.verbose,
            };

            match info::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
        Command::Search { query } => {
            println!("relava search {query}");
        }
        Command::Update {
            resource_type,
            name,
            all,
        } => {
            let rt = resource_type.as_ref().map(|s| {
                install::parse_resource_type(s).unwrap_or_else(|e| exit_with_error(&e, cli.json))
            });

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = update::UpdateOpts {
                server_url: &cli.server,
                resource_type: rt,
                name: name.as_deref(),
                all,
                project_dir: &project_dir,
                json: cli.json,
                verbose: cli.verbose,
            };

            match update::run(&opts) {
                Ok(result) => {
                    // Update lockfile for each updated resource
                    if let Err(e) = lockfile::update_after_update(&project_dir, &result, cli.json)
                        && !cli.json
                    {
                        eprintln!("[warn] failed to update relava.lock: {e}");
                    }
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
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
            version,
        } => {
            let rt = match install::parse_resource_type(&resource_type) {
                Ok(rt) => rt,
                Err(e) => exit_with_error(&e, cli.json),
            };

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let client = registry::RegistryClient::new(&cli.server);
            let cache_dir = dirs::home_dir()
                .unwrap_or_else(|| {
                    eprintln!("cannot determine home directory");
                    std::process::exit(1);
                })
                .join(".relava")
                .join("cache");
            let cache = cache::DownloadCache::new(cache_dir);

            // Merge --version flag into the version pins so the resolver
            // uses it when resolving the root resource.
            let mut version_pins = install::load_version_pins(&project_dir, rt);
            if let Some(ref v) = version {
                version_pins.insert(name.clone(), v.clone());
            }

            let provider =
                resolver::RegistryDepProvider::new(&client, &cache, &project_dir, version_pins);

            match resolver::resolve(&provider, rt, &name) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result.to_json_output());
                    } else {
                        print!("{}", result.tree.display());
                    }
                }
                Err(e) => exit_with_error(&e.to_string(), cli.json),
            }
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
            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = doctor::DoctorOpts {
                server_url: &cli.server,
                project_dir: &project_dir,
                json: cli.json,
                _verbose: cli.verbose,
            };

            let result = doctor::run(&opts);
            if cli.json {
                print_json(&result);
            }
            if !result.is_healthy() {
                std::process::exit(1);
            }
        }
        Command::Disable {
            resource_type,
            name,
        } => {
            let rt = install::parse_resource_type(&resource_type)
                .unwrap_or_else(|e| exit_with_error(&e, cli.json));

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = disable::DisableOpts {
                resource_type: rt,
                name: &name,
                project_dir: &project_dir,
                json: cli.json,
                verbose: cli.verbose,
            };

            match disable::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
        Command::Enable {
            resource_type,
            name,
        } => {
            let rt = install::parse_resource_type(&resource_type)
                .unwrap_or_else(|e| exit_with_error(&e, cli.json));

            let project_dir = resolve_project_dir(cli.project.as_deref());

            let opts = enable::EnableOpts {
                resource_type: rt,
                name: &name,
                project_dir: &project_dir,
                json: cli.json,
                verbose: cli.verbose,
            };

            match enable::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
        Command::Import {
            resource_type,
            path,
            version,
        } => {
            let rt = install::parse_resource_type(&resource_type)
                .unwrap_or_else(|e| exit_with_error(&e, cli.json));

            let opts = import::ImportOpts {
                server_url: &cli.server,
                resource_type: rt,
                path: std::path::Path::new(&path),
                version: version.as_deref(),
                json: cli.json,
                verbose: cli.verbose,
            };

            match import::run(&opts) {
                Ok(result) => {
                    if cli.json {
                        print_json(&result);
                    }
                }
                Err(e) => exit_with_error(&e, cli.json),
            }
        }
    }
}
