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
- **Multi-platform** -- `agent_type` field supports Claude Code (MVP), with Codex and Gemini CLI planned
- **Registry server** -- local HTTP server with REST API, web GUI for browsing, and SQLite metadata
- **Non-invasive** -- writes to standard `.claude/` locations; remove Relava and your resources still work
- **Enterprise-ready architecture** -- REST-first design, storage abstraction traits, future support for SSO, scoping, and registry federation

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

# Search, list, update
relava search notify
relava list skills
relava update --all

# Publish to registry
relava publish skill my-skill

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

For `--global`, resources install to `~/.claude/` instead.

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
- **GUI** -- web app at `localhost:7420` for browsing and searching the registry.

The CLI always talks to the server via REST API. Switching from local (`localhost:7420`) to an enterprise registry is just a URL change.

### Crate Structure

The project is organized as a Cargo workspace with four crates:

| Crate | Purpose | License |
|-------|---------|---------|
| `relava-types` | Shared types, validation, versioning, and manifest parsing | Apache-2.0 |
| `relava-cli` | CLI binary -- registry client, caching, dependency resolution, environment checks | Apache-2.0 |
| `relava-server` | Registry server -- REST API, storage (SQLite, blob store), web GUI | ELv2 |
| `relava-server-ext` | Cloud and enterprise extensions (future) | ELv2 |

```
relava-cli        → relava-types
relava-server     → relava-types
relava-server-ext → relava-server, relava-types
```

## Enterprise Extensibility

The MVP is local-first, but the architecture is designed for enterprise:

- **Scoping** -- personal (`@user/name`), team (`@team/name`), and global namespaces with permissions
- **Auth & SSO** -- API tokens, OIDC, SAML support planned
- **Registry federation** -- `[registries]` in `relava.toml` for multi-registry resolution
- **Storage abstraction** -- `ResourceStore`, `BlobStore`, `SearchBackend` traits for swapping SQLite/filesystem to PostgreSQL/S3/vector search
- **Semantic search** -- hybrid vector + text search via embeddings
- **Audit logging, webhooks, offline bundles** -- documented in DESIGN.md

## Tech Stack

| Component | Technology |
|-----------|------------|
| Workspace | Rust (4-crate Cargo workspace) |
| CLI | Rust, clap |
| Server | Rust, Axum, SQLite |
| GUI | React, Vite, Tailwind CSS, TanStack Query |

## Status

Relava is in active development. Week 1 (scaffolding, parsers, validation) is complete. See [DESIGN.md](DESIGN.md) for the full design document and implementation roadmap.

## License

Relava uses split licensing:

- **`relava-types` and `relava-cli`** -- [Apache License 2.0](crates/relava-types/LICENSE). Open source, free to use and modify.
- **`relava-server` and `relava-server-ext`** -- [Elastic License 2.0 (ELv2)](crates/relava-server/LICENSE). Free for personal and commercial use. Cannot be offered as a managed service.
