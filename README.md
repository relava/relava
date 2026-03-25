# Relava

A local package manager and registry for Claude Code resources.

## What is Relava?

Claude Code's extension model is file-based -- skills are directories, agents are `.md` files, commands are `.md` files, rules are `.md` files, hooks live in `settings.json`. There is no built-in package manager, no versioning, no discovery. Developers manually copy files between projects.

Relava fixes this. It manages Claude Code prompt-layer artifacts the same way `npm` manages JavaScript packages or `brew` manages system software -- but everything runs locally on your machine with no cloud dependency.

## Key Features

- **Individual resource management** -- install, update, and remove skills, agents, commands, and rules independently
- **Versioning** -- semantic versioning with pinned versions per project; multiple versions stored locally
- **Declarative manifest** -- `relava.toml` declares your project's resources, committable to version control
- **Local registry** -- a local server stores published resources with a web GUI for browsing and management
- **CLI** -- scriptable command-line interface for all operations
- **Multi-project** -- manage different resource sets across different projects
- **Non-invasive** -- Relava writes to standard Claude Code locations; remove Relava and your resources still work

## CLI Usage

All commands follow the pattern: `relava <verb> <resource-type> <resource-name>`

### Install resources

```bash
# Install a skill into the current project
relava install skill denden

# Install and save to relava.toml
relava install skill denden --save

# Install a specific version
relava install skill notify-slack --version 0.2.0 --save

# Install an agent
relava install agent debugger --save

# Install a command
relava install command commit --save

# Install a rule
relava install rule no-console-log
```

### Install from manifest

```bash
# Install all resources declared in relava.toml (like npm install)
relava install relava.toml
```

### Other commands

```bash
# List installed resources by type
relava list skills
relava list agents

# Search the local registry
relava search notify

# View resource details
relava info skill denden

# Update a resource
relava update skill denden
relava update --all

# Remove a resource
relava remove skill denden --save

# Publish to local registry
relava publish skill my-skill

# Start the local server
relava server start --daemon

# Check installation health
relava doctor
```

## The `--save` Flag

- **Without `--save`**: downloads and installs resource files, but does not modify `relava.toml`
- **With `--save`**: same as above, plus writes the resource name and pinned version to `relava.toml`

This mirrors `npm install --save`. The `relava.toml` file is the declarative manifest you commit to version control so collaborators can run `relava install relava.toml` to reproduce the same setup.

## `relava.toml`

A project-level manifest that declares installed resources with explicit versions:

```toml
[skills]
denden = "1.2.0"
notify-slack = "0.3.0"

[agents]
debugger = "0.5.0"

[commands]
commit = "0.2.0"

[rules]
no-console-log = "1.0.0"
```

This file is user-editable. Relava reads it but only writes to it when `--save` is used.

## Architecture

Relava has three components, all running on your machine:

- **CLI** (`relava`) -- Rust binary for all command-line operations
- **Local Server** -- HTTP server (port 7420) that stores published resources, manages a SQLite metadata database, and serves the GUI
- **GUI** -- web application at `localhost:7420` for browsing, searching, and managing resources

The CLI talks to the server via REST API. For basic operations, the CLI can work directly against the local SQLite database when the server isn't running.

All state lives in `~/.relava/` -- published resource files, the SQLite database, and configuration.

## Design Principles

1. **Local-first** -- everything works offline, no account required
2. **Prompt-layer only** -- manages text and files injected into Claude's context, not infrastructure (no MCP servers, runtimes, or databases)
3. **Non-invasive** -- writes files to standard Claude Code locations; resources work with or without Relava
4. **Individual resources** -- no bundling or archive step; each resource is published and installed independently

## Tech Stack

| Component | Technology |
|-----------|------------|
| CLI | Rust, clap |
| Server | Rust, Axum, SQLite (rusqlite) |
| GUI | React, Vite, Tailwind CSS, TanStack Query |

## Status

Relava is in the design and planning phase. See [PLAN.md](PLAN.md) for the full design document and implementation roadmap.
