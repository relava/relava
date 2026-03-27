---
name: relava
description: CLI reference for the relava local package manager for prompt-layer resources (skills, agents, commands, rules). Use when installing, removing, publishing, updating, searching, or managing resources. Also use when setting up a new project (relava init), troubleshooting resource issues (relava doctor), managing the local registry server, or when a relava.toml exists and resources need syncing. Trigger whenever the user mentions relava, package management for AI resources, or needs to find/share prompt-layer resources.
version: 0.1.0
---

# Relava

Relava is a local package manager for prompt-layer resources — skills, agents, commands, and rules. It runs a local registry server and provides a CLI to install, publish, update, and manage resources across projects. Use relava commands whenever the user needs to install, discover, publish, or troubleshoot prompt-layer resources.

## Commands Reference

### Project Setup

- `relava init` — Create a `relava.toml` manifest in the current directory. Run this first in any new project.

### Install & Remove

- `relava install <type> <name>` — Install a resource from the registry.
  - `--save` — Also add to `relava.toml` so it persists across installs.
  - `--version <ver>` — Pin to a specific version (default: latest).
  - `--global` — Install to `~/.claude/` instead of the project directory.
  - `-y` / `--yes` — Auto-accept tool installation prompts.
- `relava install` — Install all resources declared in `relava.toml` (bulk install).
- `relava remove <type> <name>` — Remove an installed resource.
  - `--save` — Also remove from `relava.toml`.

### Discovery

- `relava search <query>` — Search the registry for resources by name or description.
  - `--type <type>` — Filter results by resource type.
- `relava list [type]` — List installed resources. Omit type to list all.
- `relava info <type> <name>` — Show detailed information about an installed resource.

### Update

- `relava update <type> <name>` — Update a single resource to the latest version.
- `relava update --all` — Update all installed resources.

### Publish & Validate

- `relava publish <type> <name>` — Publish a resource to the local registry.
  - `--path <dir>` — Publish from a custom source directory.
  - `--force` — Skip change detection and publish regardless.
  - `-y` / `--yes` — Auto-confirm the publish prompt.
- `relava validate <type> <path>` — Validate a resource offline before publishing. Checks frontmatter, file structure, and content rules.
- `relava import <type> <path>` — Import an existing resource directory into the registry.
  - `--version <ver>` — Override the version (default: from frontmatter or 1.0.0).

### Enable & Disable

- `relava disable <type> <name>` — Temporarily disable a resource (moves to `.disabled/`).
- `relava enable <type> <name>` — Re-enable a previously disabled resource.

### Dependency Resolution

- `relava resolve <type> <name>` — Display the dependency tree for a resource.
  - `--version <ver>` — Resolve a specific version.

### Server Management

- `relava server start` — Start the local registry server.
  - `--port <port>` — Port to listen on (default: 7420).
  - `--daemon` — Run as a background process.
  - `--gui-dir <dir>` — Serve the web GUI from a custom directory.
- `relava server stop` — Stop the running server.
- `relava server status` — Show whether the server is running, its PID, port, and uptime.

### Health & Cache

- `relava doctor` — Check health of the relava installation, registry connectivity, manifests, and file integrity.
- `relava cache status` — Show download cache disk usage.
- `relava cache clean` — Remove cached downloads.
  - `--older-than <duration>` — Only remove entries older than a duration (e.g. `30d`, `12h`).

### Global Flags

- `--server <url>` — Override the registry server URL (default: `http://localhost:7420`).
- `--project <dir>` — Override the project directory.
- `--json` — Output as JSON (for scripting and automation).
- `--verbose` — Show detailed output.
- `--no-update-check` — Suppress the automatic update availability check.

## Resource Types

| Type | File | Install Location (project) | Purpose |
|------|------|---------------------------|---------|
| **skill** | `SKILL.md` | `.claude/skills/<name>/` | Teaches domain knowledge, workflows, or tool usage |
| **agent** | `AGENT.md` + optional files | `.claude/agents/<name>/` | Defines autonomous agent behaviors and capabilities |
| **command** | `COMMAND.md` | `.claude/commands/<name>/` | Adds slash commands (e.g. `/review`, `/deploy`) |
| **rule** | `RULE.md` | `.claude/rules/<name>/` | Sets project-wide rules and constraints for the agent |

All resource types use YAML frontmatter with at least `name`, `description`, and `version` fields. Skills, commands, and rules must be text-only (no binary files).

## Common Workflows

### Set up a new project

```
relava init
relava install skill relava --save
relava install skill git-workflow --save
```

### Install all declared resources

```
relava install
```

This reads `relava.toml` and installs every resource listed there.

### Publish a resource

```
relava validate skill ./my-skill/
relava publish skill my-skill --path ./my-skill/
```

Validate first to catch issues before publishing. The registry performs change detection — publishing identical content is a no-op.

### Check project health

```
relava doctor
```

Checks registry connectivity, validates all manifests, verifies file integrity, and detects sync mismatches between `relava.toml` and installed resources.

### Search and install a resource

```
relava search "code review"
relava install skill code-reviewer --save
```

## Project Manifest

The `relava.toml` file declares which resources a project uses:

```toml
# relava.toml

[skills]
relava = "*"          # latest version
git-workflow = "1.2.0" # pinned version

[agents]

[commands]
review = "*"

[rules]
security = "1.0.0"
```

- Each section (`[skills]`, `[agents]`, `[commands]`, `[rules]`) maps resource names to version constraints.
- `"*"` means latest available version.
- `"X.Y.Z"` pins to an exact version.
- Use `--save` with `install` and `remove` to keep `relava.toml` in sync automatically.
- Run `relava install` with no arguments to install everything in the manifest.

## Proactive Suggestions

Once triggered, suggest the most relevant relava command for the situation:

- Project has no `relava.toml` → suggest `relava init`.
- `relava.toml` exists but resources are not installed → suggest `relava install`.
- User adds or removes prompt-layer resources → suggest `--save` to keep `relava.toml` in sync.
- User mentions publishing or sharing a resource → suggest `relava validate` then `relava publish`.
- User encounters issues with installed resources → suggest `relava doctor`.
- User is looking for a resource → suggest `relava search`.
