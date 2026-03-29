# Relava

A local package manager and registry for Claude Code resources.

## What is Relava?

Claude Code's extension model is file-based -- skills are directories, agents, commands, and rules are `.md` files. There is no built-in package manager, no versioning, no dependency tracking, no discovery. Developers manually copy files between projects.

Relava fixes this. It manages Claude Code prompt-layer artifacts the same way `npm` manages JavaScript packages -- but everything runs locally on your machine.

## Key Features

- **Install & manage** skills, agents, commands, and rules with a single CLI
- **Semantic versioning** with pinned versions per project (`"1.2.0"` or `"*"` for latest)
- **Dependency resolution** -- resources declare dependencies in `metadata.relava` frontmatter; Relava installs them transitively
- **Declarative manifest** -- `relava.toml` declares your project's resources, committable to version control
- **Lockfile** -- `relava.lock` tracks exact installed versions and dependency graph for reproducibility
- **Disable/enable** -- temporarily deactivate resources without uninstalling them
- **Validate & import** -- validate resources offline before publishing; import existing resources into the registry
- **Download caching** -- resources cached in `~/.relava/cache/` for fast re-installs
- **Auto-update notifications** -- checks for newer resource versions after commands, with GUI badge
- **Self-updating CLI** -- automatic startup check for new CLI versions with interactive upgrade prompt
- **Cache management** -- `relava cache clean` and `relava cache status` for download cache control
- **Multi-platform** -- `agent_type` field supports Claude Code (MVP), with Codex and Gemini CLI planned
- **Registry server** -- local HTTP server with REST API, web GUI for browsing, and SQLite metadata
- **Non-invasive** -- writes to standard `.claude/` locations; remove Relava and your resources still work

## Installation

### Quick install (recommended)

**macOS / Linux:**

```bash
curl -fsSL https://raw.githubusercontent.com/relava/relava/main/scripts/install.sh | sh
```

**Windows (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/relava/relava/main/scripts/install.ps1 | iex
```

### Install via Cargo

```bash
cargo install relava
```

### Manual download

Download pre-built binaries from [GitHub Releases](https://github.com/relava/relava/releases). Available for:

- Linux (x86_64, aarch64)
- macOS (x86_64, Apple Silicon)
- Windows (x86_64)

## Quick Start

```bash
# Start the registry server
relava server start --daemon

# Initialize a project
relava init

# Install resources
relava install skill denden --save
relava install agent debugger --save

# Install all resources from manifest
relava install relava.toml

# Search, list, info
relava search notify
relava list skills
relava info skill denden

# Update resources
relava update skill denden
relava update --all

# Publish to registry
relava publish skill my-skill
relava import skill ./existing-skill/

# Validate before publishing
relava validate skill ./my-skill/

# Resolve dependency tree
relava resolve skill denden

# Disable/enable resources
relava disable skill denden
relava enable skill denden

# Cache management
relava cache status
relava cache clean --older-than 7d

# Check health
relava doctor
```

## `relava.toml`

The project manifest declares the target platform and installed resources:

```toml
agent_type = "claude"

[skills]
denden = "1.2.0"
notify-slack = "*"

[agents]
debugger = "0.5.0"

[commands]
commit = "0.2.0"

[rules]
no-console-log = "1.0.0"
```

- `agent_type` -- target platform (`"claude"`, future: `"codex"`, `"gemini"`)
- Version constraints: `"X.Y.Z"` (exact pin) or `"*"` (latest)
- User-editable; Relava only writes to it with `--save`
- Commit to version control so collaborators can run `relava install relava.toml`

## Resource Dependencies

Dependencies are declared in the resource's `.md` frontmatter using `metadata.relava`, following the [Agent Skills specification](https://agentskills.io/specification):

```yaml
---
name: orchestrator
description: Coordinates feature development
tools: Agent, Glob, Grep, Read
model: sonnet
metadata:
  relava:
    skills:
      - notify-slack
      - code-review
    agents:
      - debugger
---
```

Relava parses these and recursively installs all transitive dependencies. Dependency names only -- version pinning is at the project level (`relava.toml`).

## Install Locations

All resources install under `.claude/` in the project:

| Type | Location |
|------|----------|
| Skills | `.claude/skills/<name>/SKILL.md` |
| Agents | `.claude/agents/<name>.md` |
| Commands | `.claude/commands/<name>.md` |
| Rules | `.claude/rules/<name>.md` |

For `--global`, resources install to `~/.claude/` instead. Disabled resources are moved to `.disabled/` subdirectories (e.g., `.claude/skills/.disabled/<name>/`).

## Architecture

```
CLI (relava) ──REST──> Registry Server (localhost:7420)
     │                       │
     │                  Resource Store (~/.relava/store/)
     │                  SQLite Metadata DB
     │                  Web GUI
     v
Project Filesystem (.claude/)
```

- **Registry Server** -- pure resource registry. Stores published resources, serves them via REST API. Does not track projects.
- **CLI** -- reads `relava.toml`, fetches resources from server, writes files to the project. All project management is local.
- **GUI** -- React SPA at `localhost:7420` for browsing and searching the registry. Features dashboard with stats, resource browser with search/filter/sort, resource detail with markdown rendering, and settings page with server status and cache management.

The CLI always talks to the server via REST API.

### Crate Structure

The project is organized as a Cargo workspace with three crates:

| Crate | Purpose | License |
|-------|---------|---------|
| `relava-types` | Shared types, validation, versioning, manifest parsing, file filtering | Apache-2.0 |
| `relava-cli` | CLI binary -- all commands (install, remove, update, list, info, search, publish, import, validate, doctor, disable, enable, cache, resolve), registry client, caching, dependency resolution, self-update, environment checks | Apache-2.0 |
| `relava-server` | Registry server -- REST API, storage (SQLite, blob store), dependency resolver, web GUI serving | ELv2 |

```
relava-cli        → relava-types
relava-server     → relava-types
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `relava init` | Initialize project with `relava.toml` |
| `relava install <type> <name>` | Install a resource (`--version`, `--save`, `--global`, `--yes`) |
| `relava install relava.toml` | Install all resources from manifest |
| `relava remove <type> <name>` | Remove a resource (`--save`) |
| `relava list [<type>]` | List installed resources |
| `relava info <type> <name>` | Show resource details |
| `relava search <query>` | Search registry (`--type`) |
| `relava update [<type> <name>]` | Update resources (`--all`) |
| `relava publish <type> <name>` | Publish to registry (`--path`, `--force`, `--yes`) |
| `relava import <type> <path>` | Import existing resource into registry (`--version`) |
| `relava validate <type> <path>` | Validate resource offline before publishing |
| `relava resolve <type> <name>` | Show dependency tree (`--version`) |
| `relava disable <type> <name>` | Disable a resource (moves to `.disabled/`) |
| `relava enable <type> <name>` | Re-enable a disabled resource |
| `relava doctor` | Check installation and project health |
| `relava cache status` | Show cache disk usage |
| `relava cache clean` | Clean cached downloads (`--older-than`) |
| `relava server start` | Start registry server (`--port`, `--daemon`, `--gui-dir`) |
| `relava server stop` | Stop the running server |
| `relava server status` | Show server status |

Global options: `--server URL`, `--project PATH`, `--verbose`, `--json`, `--no-update-check`

## Tech Stack

| Component | Technology |
|-----------|------------|
| Workspace | Rust (3-crate Cargo workspace) |
| CLI | Rust, clap, reqwest, comfy-table, colored |
| Server | Rust, Axum, SQLite (rusqlite), tokio |
| GUI | React 19, Vite 8, Tailwind CSS 4, TanStack Query 5, React Router 7 |
| CI | GitHub Actions |

## Status

Relava is in active development. Phases 1–3 are complete (CLI, registry server, GUI). The CLI supports all core commands: install, remove, update, list, info, search, publish, import, validate, resolve, disable/enable, doctor, cache management, and self-update. The registry server provides a full REST API with SQLite FTS5 search. The GUI provides a dashboard, resource browser, detail pages, and settings. See [DESIGN.md](DESIGN.md) for the full design document and implementation roadmap.

## License

Relava uses split licensing:

- **`relava-types` and `relava-cli`** -- [Apache License 2.0](crates/relava-types/LICENSE). Open source, free to use and modify.
- **`relava-server`** -- [Elastic License 2.0 (ELv2)](crates/relava-server/LICENSE). Free for personal and commercial use. Cannot be offered as a managed service.
