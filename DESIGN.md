# Relava — Plan

> A local registry and package manager for Claude Code prompt-layer artifacts.

---

## 1. Vision and Goals

### What Relava Is

Relava is a **local package manager and registry** for Claude Code resources. It manages the prompt-layer artifacts that shape how Claude thinks and behaves: skills, agents, commands, rules, and hooks. Think `npm` or `brew`, but for Claude Code extensions.

Relava runs **entirely on the developer's machine**. There is no cloud dependency. The local registry server is the single source of truth for published resources.

### Why It Exists

Claude Code's extension model is file-based — skills are directories, agents are `.md` files, commands are `.md` files, rules are `.md` files, hooks are JSON in `settings.json`. There is no built-in package manager, no versioning, no dependency tracking, no discovery mechanism. Developers manually copy files between projects.

Relava solves this by providing:

- **Individual resource management** — each skill, agent, command, and rule is versioned and managed independently
- **Version management** so resources can be updated, rolled back, and pinned
- **A local registry** with GUI for browsing, searching, and managing resources
- **A CLI** that reads a project's `relava.toml` and fetches resources from the registry
- **A declarative manifest** (`relava.toml`) for reproducible project setups

### Design Principles

1. **Local-first.** Everything works offline. No account required.
2. **Prompt-layer only.** Relava manages text/files that get injected into Claude's context. It does NOT manage infrastructure (MCP servers, runtimes, databases).
3. **Non-invasive.** Relava writes files to standard Claude Code locations. If you remove Relava, your installed resources still work — they're just files.
4. **Multi-file aware.** Skills can contain templates and support files. Relava handles the full complexity.
5. **Individual resources.** No bundling or archive step. Each resource is published and installed independently, with its directory contents uploaded as-is.

---

## 2. Architecture Overview

```
+------------------------------------------------------+
|                   Developer Machine                   |
|                                                       |
|  +-------------+     +----------------------------+   |
|  |  relava CLI  |---->|   Relava Registry Server   |   |
|  +-------------+     |                            |   |
|        |              |  REST API (:7420)          |   |
|        |              |  Resource Store             |   |
|  +-------------+     |  SQLite Metadata DB        |   |
|  |  Relava GUI  |---->|                            |   |
|  | (Web App)    |     +----------------------------+   |
|  +-------------+                                     |
|        |                                              |
|        v                                              |
|  +---------------------------------------------------+|
|  |              Project Filesystem                    ||
|  |  (managed by CLI, not by server)                   ||
|  |                                                    ||
|  |  .claude/                                          ||
|  |    skills/        <-- skill directories            ||
|  |    agents/        <-- agent .md files              ||
|  |    commands/       <-- command .md files            ||
|  |    rules/          <-- rule .md files              ||
|  |  relava.toml       <-- project resource declarations ||
|  +---------------------------------------------------+|
|                                                       |
|  +---------------------------------------------------+|
|  |         ~/.relava/  (Server State)                 ||
|  |                                                    ||
|  |  store/            <-- published resource files    ||
|  |  db.sqlite         <-- resource metadata           ||
|  |  config.toml       <-- server configuration        ||
|  |  cache/            <-- download cache              ||
|  +---------------------------------------------------+|
+------------------------------------------------------+
```

### Component Interactions

1. **Registry Server** is a pure resource registry — it stores published resources and serves them via REST API. It does NOT track projects, installations, or manage project files.
2. **CLI** reads the project's `relava.toml`, requests resources from the server, and writes files to the project filesystem. The CLI manages all project-level operations.
3. **GUI** is a web application served by the server for browsing and searching the registry.
4. **Project Filesystem** is managed entirely by the CLI — the server never touches it.

### Crate Structure

The codebase is a Cargo workspace with four crates, split by concern and license:

```
relava-types        (Apache-2.0)   Shared types, validation, versioning, manifest parsing
    ^                              Zero IO dependencies — pure logic only
    |
    +--- relava-cli     (Apache-2.0)   CLI binary, registry client, caching,
    |                                  dependency resolution, env checks, tool checks
    |
    +--- relava-server  (ELv2)         Registry server, REST API, storage layer,
             ^                         SQLite DB, blob store, web GUI
             |
         relava-server-ext (ELv2)      Cloud and enterprise extensions (future)
                                       (depends on both relava-server and relava-types)
```

| Crate | Contains | License |
|-------|----------|---------|
| `relava-types` | `manifest`, `validate`, `version` modules | Apache-2.0 |
| `relava-cli` | `registry`, `cache`, `env_check`, `tools`, `install`, `init` | Apache-2.0 |
| `relava-server` | `store` (traits, db, blob, models, dirs), HTTP handlers, GUI serving | ELv2 |
| `relava-server-ext` | Stub — future cloud/enterprise extensions | ELv2 |

The split licensing keeps shared types and the CLI open source (Apache-2.0) while protecting the server against competing managed services (ELv2).

### REST-First Architecture

All CLI operations go through the server's REST API — there is no direct mode. If the server is not running, the CLI prints an error: `Server not reachable. Run 'relava server start' first.`

This design choice is deliberate:

- **Enterprise-ready** — the same CLI works against a local server (`localhost:7420`) or an organization's hosted registry (`registry.company.com`). Switching is just a `--server` URL change.
- **Keeps the store opaque** — only the server reads/writes `~/.relava/store/` and the SQLite DB. The CLI never touches them directly.
- **Single code path** — no branching between "direct mode" and "server mode" simplifies the CLI and avoids subtle behavioral differences.
- **Supports caching** — downloaded resources are cached in `~/.relava/cache/` for faster re-installs.

---

## 3. Resource Format Specification

### Dependency Declaration: `metadata.relava` in frontmatter

Dependencies are declared in the `metadata.relava` block of a resource's `.md` frontmatter. This follows the [Agent Skills specification](https://agentskills.io/specification), which defines `metadata` as an open-ended extension point for custom fields. Claude Code and other agent products ignore unknown metadata keys.

Frontmatter dependencies are **names only** — no version pins. Version control belongs at the project level (`relava.toml`), not the resource level. This keeps frontmatter simple and allows `relava update` to pull latest versions without conflicting pins.

**Skill example** (`SKILL.md`):
```yaml
---
name: code-review
description: Comprehensive code review with security and style checks
metadata:
  relava:
    skills:
      - security-baseline
      - style-guide
    tools:
      gh:
        description: GitHub CLI
        install:
          macos: brew install gh
          linux: apt install gh
          windows: winget install GitHub.cli
    env:
      GITHUB_TOKEN:
        required: true
        description: GitHub API token for PR access
---
```

**Agent example** (`.claude/agents/orchestrator.md`):
```yaml
---
name: orchestrator
description: Coordinates feature development workflow
tools: Agent, Glob, Grep, Read
model: sonnet
metadata:
  relava:
    skills:
      - notify-slack
      - code-review
    agents:
      - debugger
    env:
      SLACK_WEBHOOK:
        required: false
        description: Slack webhook URL for notifications
---
```

#### `metadata.relava` Fields

| Field | Description |
|-------|-------------|
| `skills` | List of skill dependency names (resolved transitively) |
| `agents` | List of agent dependency names (resolved transitively) |
| `tools` | System tool dependencies with OS-specific install commands |
| `env` | Environment variable requirements (required/optional with descriptions) |

#### Tool Installation (`tools`)

Resources can declare system tools they depend on. On `relava install`, the CLI:

1. For each declared tool, checks if the binary exists on `PATH` via `which`/`where`
2. If missing, detects current OS (macOS/Linux/Windows) and looks up the OS-specific install command
3. Prompts the user for confirmation (skip with `--yes` flag)
4. Executes the install command via subprocess
5. Reports status: `installed`, `skipped`, `failed`, `declined`, `no_command`

Tool installation failures are **non-fatal** — the resource is still installed, with a warning.

```bash
$ relava install skill code-review
Installing skill code-review@1.0.0...
  [skill]   .claude/skills/code-review/SKILL.md + 2 files
  [tool]    gh — not found on PATH
            Install with: brew install gh? [Y/n] y
  [tool]    gh — installed
Installed skill code-review@1.0.0
```

#### Environment Variables (`env`)

Resources can declare required and optional environment variables. On `relava install`, the CLI:

1. Checks each required env var against the process environment and `.claude/settings.json` `env` entries
2. Warns for any missing required vars (does not block installation)
3. `relava doctor` rechecks all installed resources for missing env vars

```bash
$ relava install skill code-review
Installing skill code-review@1.0.0...
  [skill]   .claude/skills/code-review/SKILL.md + 2 files
  [warn]    Missing required env: GITHUB_TOKEN
            Set in .claude/settings.json under "env"
Installed skill code-review@1.0.0
```

On `relava install`, the CLI parses the `metadata.relava` block from the resource's `.md` file to discover and recursively install transitive dependencies, system tools, and check environment variables. Each dependency resolves to the version pinned in the project's `relava.toml`, or the latest version in the registry if not pinned.

There is no separate `relava.toml` per resource — all metadata lives in the frontmatter. The project-level `relava.toml` (see below) is where versions are pinned.

### Resource Naming (Slug Format)

Resource names (slugs) must follow a strict format to ensure URL-safety and cross-platform compatibility:

- **Length**: 1–64 characters
- **Characters**: lowercase alphanumeric (`a-z`, `0-9`) and hyphens (`-`)
- **Must start with**: a letter or digit (not a hyphen)
- **Must end with**: a letter or digit (not a hyphen)
- **No consecutive hyphens**

Valid: `denden`, `notify-slack`, `code-review`, `my-skill-v2`
Invalid: `-denden`, `Notify_Slack`, `code--review`, `my.skill`

Slug validation is enforced on `relava publish` (client + server), when parsing project `relava.toml`, and when parsing `metadata.relava` frontmatter.

### Resource Directory Structures

Each resource type has its own layout. Dependencies are declared in `metadata.relava` frontmatter — no per-resource `relava.toml`.

**Skill** (multi-file directory):
```
denden/
  SKILL.md                 # Skill definition with frontmatter (required)
  README.md                # Documentation (recommended)
  templates/               # Support files
  lib/                     # Additional code/data
```

**Agent** (single `.md` file):
```
orchestrator.md            # Agent definition with frontmatter
```
Installed to `.claude/agents/orchestrator.md`. Dependencies on skills and other agents declared in frontmatter.

**Command** (single `.md` file):
```
commit.md                  # Command definition
```
Installed to `.claude/commands/commit.md`.

**Rule** (single `.md` file):
```
no-console-log.md          # Rule definition
```
Installed to `.claude/rules/no-console-log.md`.

### Versioning

- Follows [Semantic Versioning 2.0.0](https://semver.org/).
- The local store keeps multiple versions. Only one version is installed per project at a time.
- On publish, if no version is specified, the patch version is auto-incremented from the latest published version.

### Version Constraints

Project manifests and dependency declarations support these constraint formats:

| Format | Meaning | Example |
|--------|---------|---------|
| `"X.Y.Z"` | Exact version pin | `"1.2.0"` |
| `"*"` | Latest available version | `"*"` |

On install, `"*"` resolves to the latest published version. When `--save` writes to `relava.toml`, it always pins the resolved exact version (e.g., `"*"` becomes `"1.2.0"`). The `--save-exact` flag is implicit — Relava always saves exact versions.

### Project Manifest: `relava.toml` (per project)

A project-level `relava.toml` declares which resources are installed with version constraints:

```toml
# Target agent platform — determines install paths and supported features
agent_type = "claude"  # "claude" | "codex" | "gemini" (only "claude" supported in MVP)

# Project resource declarations
# Managed by `relava install --save` or edited by hand.

[skills]
denden = "1.2.0"              # exact version pin
notify-slack = "*"             # latest available
strawpot-recap = "1.0.0"

[agents]
debugger = "0.5.0"

[commands]
delegate = "1.0.0"
commit = "0.2.0"

[rules]
no-console-log = "1.0.0"
```

The `agent_type` field tells Relava which platform conventions to follow for install paths, frontmatter parsing, and available resource types. In MVP, only `"claude"` is supported — other values produce a clear error.

**Install paths by `agent_type`:**

| agent_type | Skills | Agents | Commands | Rules |
|------------|--------|--------|----------|-------|
| `claude` | `.claude/skills/<name>/` | `.claude/agents/<name>.md` | `.claude/commands/<name>.md` | `.claude/rules/<name>.md` |
| `codex` | TBD | TBD | TBD | TBD |
| `gemini` | TBD | TBD | TBD | TBD |

This file is:
- **User-editable** — developers can hand-edit it directly
- **Read by Relava** — `relava install relava.toml` installs all declared resources
- **Written by Relava** only when `--save` is used — `relava install skill code-review --save` adds the entry

---

## 4. Local Registry Server Design

The Relava server is a local registry that stores published resources and serves the GUI. It is the single source that `relava install` pulls from and `relava publish` pushes to.

### Storage

```
~/.relava/
  config.toml          # Server config (port, defaults)
  db.sqlite            # All metadata
  store/               # Published resource files (stored as-is, no archives)
    skills/
      denden/
        1.0.0/         # Version directory
          SKILL.md
          templates/...
        1.2.0/
          SKILL.md
          templates/...
    agents/
      debugger/
        0.5.0/
          debugger.md
  cache/               # Temporary cache
  logs/                # Server logs
```

### Database Schema (SQLite)

```sql
-- Available resources in the registry
CREATE TABLE resources (
  id            INTEGER PRIMARY KEY,
  scope         TEXT,           -- nullable; reserved for future scoping (@org/name)
  name          TEXT NOT NULL,
  type          TEXT NOT NULL,  -- 'skill' | 'agent' | 'command' | 'rule'
  description   TEXT,
  latest_version TEXT,
  metadata_json TEXT,           -- full manifest as JSON
  updated_at    TIMESTAMP,
  UNIQUE(scope, name, type)
);

-- Resource versions
CREATE TABLE versions (
  id            INTEGER PRIMARY KEY,
  resource_id   INTEGER REFERENCES resources(id),
  version       TEXT NOT NULL,
  store_path    TEXT,           -- path in store/ directory
  checksum      TEXT,           -- SHA-256 of directory contents
  manifest_json TEXT,           -- full frontmatter metadata as JSON
  published_by  TEXT,           -- nullable; reserved for audit logging when auth is enabled
  published_at  TIMESTAMP,
  UNIQUE(resource_id, version)
);

```

The server does not track projects or installations. Project management is handled entirely by the CLI via `relava.toml`.

### Future: Enterprise Scoping & Permissions

Not implemented in MVP, but the schema and API are designed to accommodate:

**Scope types:**
- **Personal** (`@alice/code-review`) — owned by a user, visible only to them or explicitly shared
- **Team** (`@platform-team/deploy-check`) — owned by a team, visible to team members
- **Global** (no scope, `code-review`) — the current MVP behavior, visible to all

**Users & permissions (future tables, not created in MVP):**
```sql
-- Reserved for enterprise extension
-- CREATE TABLE users (id, username, email, created_at);
-- CREATE TABLE teams (id, name, created_at);
-- CREATE TABLE team_members (team_id, user_id, role);  -- role: 'admin' | 'member' | 'reader'
-- CREATE TABLE resource_permissions (resource_id, scope_type, scope_id, permission);  -- permission: 'read' | 'write'
```

**Visibility & sharing permissions (per scope):**
- **Global** — default visibility is `public` (all users can read). Write requires `publish` permission (configurable: open to all, or restricted to admins).
- **Team** — default visibility is team members only. Admins can grant `read` to other teams or individual users for cross-team sharing. Write requires `member` or `admin` role.
- **Personal** — default visibility is owner only. Owner can explicitly share `read` or `read+write` with specific users or teams.

**Sharing model:**
- Permissions are additive — a user's effective access is the union of their personal, team, and global permissions
- Sharing is done via `resource_permissions` entries that grant `read` or `write` to a user or team on a specific resource
- A team admin can make a team resource "public to org" (readable by all authenticated users) without moving it to global scope

**Design constraints for MVP code:**
- The `scope` column on `resources` is nullable — `NULL` means global (current behavior)
- API paths should accept an optional scope prefix: `/resources/:type/:name` (global) and `/resources/:type/@:scope/:name` (scoped) — only the global form is implemented now
- Resource slugs remain flat within a scope — `@team/foo` and `@user/foo` can coexist
- No permission checks in MVP — all resources are globally readable and writable

### Future: Authentication & SSO

Not implemented in MVP (server binds to `127.0.0.1`, no auth), but the design must accommodate:

**Authentication methods (progressive):**
1. **API tokens** (Phase 1 enterprise) — server generates bearer tokens, CLI stores in `~/.relava/config.toml`. Simple, works for CI/CD.
2. **Username/password** — `relava login` command, token returned and stored locally.
3. **SSO/OIDC** (enterprise) — integrate with corporate identity providers (Okta, Azure AD, Google Workspace) via OpenID Connect. `relava login` opens browser for OAuth flow, receives token via localhost callback.
4. **SAML** (enterprise) — for organizations that require SAML-based SSO.

**Design constraints for MVP code:**
- All API endpoints should accept an optional `Authorization: Bearer <token>` header — ignored in MVP, enforced when auth is enabled
- CLI should have a `--token` global option and read from `RELAVA_TOKEN` env var — wired up but not required in MVP
- Server config should have an `auth.enabled` flag (default: `false` for local, `true` for enterprise)
- Session/token validation should be a middleware layer in Axum — easy to add without changing endpoint handlers

### Future: Scalability & Storage Abstraction

The MVP uses SQLite + local filesystem. For enterprise, the storage layer must be swappable without changing business logic or the API.

**Storage abstraction traits (defined in `relava-server`):**

```rust
// Implemented in MVP with SQLite, swappable to PostgreSQL/CockroachDB
trait ResourceStore {
    fn get_resource(&self, scope: Option<&str>, name: &str, resource_type: &str) -> Result<Resource>;
    fn list_versions(&self, resource_id: i64) -> Result<Vec<Version>>;
    fn publish(&self, resource: &Resource, version: &Version) -> Result<()>;
    fn search(&self, query: &str, resource_type: Option<&str>) -> Result<Vec<Resource>>;
}

// Implemented in MVP with local filesystem, swappable to S3/GCS/MinIO
trait BlobStore {
    fn store(&self, path: &str, data: &[u8]) -> Result<()>;
    fn fetch(&self, path: &str) -> Result<Vec<u8>>;
    fn delete(&self, path: &str) -> Result<()>;
    fn exists(&self, path: &str) -> Result<bool>;
}

// Implemented in MVP with SQLite FTS5, swappable to vector search
trait SearchBackend {
    fn index(&self, resource: &Resource, version: &Version) -> Result<()>;
    fn search(&self, query: &str, resource_type: Option<&str>, limit: usize) -> Result<Vec<SearchResult>>;
}
```

**Migration paths:**

| Layer | MVP | Enterprise | Migration |
|-------|-----|------------|-----------|
| **Database** | SQLite (single file) | PostgreSQL or CockroachDB | Swap `ResourceStore` impl. Schema is already standard SQL. |
| **File store** | `~/.relava/store/` (local fs) | S3, GCS, or MinIO | Swap `BlobStore` impl. Store paths map directly to object keys. |
| **Search** | SQLite FTS5 (keyword) | Hybrid: vector embeddings + text | Swap `SearchBackend` impl. Add embeddings on publish. |
| **HTTP server** | Single Axum instance | Multiple instances + load balancer | Already stateless. Add shared DB + object store. |
| **Auth** | None (localhost only) | Bearer tokens → OIDC/SAML SSO | Add Axum middleware layer. Endpoints unchanged. |

**Semantic search extension:**
- On publish, generate embeddings (via OpenAI API or local model) and store alongside resource metadata
- On search, embed the query and combine vector similarity score with FTS text score (hybrid ranking)
- SQLite has `sqlite-vec` extension for local vector search; PostgreSQL has `pgvector`
- The `SearchBackend` trait abstracts this — MVP uses FTS5, enterprise uses hybrid

**Design constraints for MVP code:**
- Database access must go through the `ResourceStore` trait, not raw SQL in handlers
- File I/O must go through the `BlobStore` trait, not direct `fs::read`/`fs::write` in handlers
- Search must go through the `SearchBackend` trait
- These traits live in the `relava-server` crate and have SQLite/filesystem implementations — swapping is adding a new impl, not refactoring existing code

### Future: Registry Federation

MVP uses a single `--server` URL. Enterprise needs to pull from multiple registries with priority ordering.

**Project manifest extension:**
```toml
agent_type = "claude"

[registries]
urls = [
  "https://registry.company.com",
  "http://localhost:7420",
]
# Resolves top-down: first match wins
```

**Resolution behavior:**
- CLI tries each URL in order until it finds the requested resource+version
- First match wins — company registry overrides public
- `relava publish` always targets the first URL (or `--server` override)
- If `[registries]` is absent, falls back to `--server` flag (default `localhost:7420`)

**Design constraints for MVP code:**
- The CLI's HTTP client should accept a list of base URLs, not just one — in MVP the list has one entry
- Resource resolution should be a loop over registries, not a single call — trivial to extend

### Future: Audit Logging

Enterprise compliance requires tracking who published what and when.

**Schema extension (not created in MVP):**
```sql
-- Add to versions table when auth is enabled:
-- ALTER TABLE versions ADD COLUMN published_by TEXT;  -- user ID or token identifier

-- CREATE TABLE audit_log (
--   id          INTEGER PRIMARY KEY,
--   timestamp   TIMESTAMP NOT NULL,
--   actor       TEXT NOT NULL,       -- user ID or token
--   action      TEXT NOT NULL,       -- 'publish', 'delete', 'update'
--   resource_type TEXT,
--   resource_name TEXT,
--   version     TEXT,
--   details_json TEXT                -- additional context
-- );
```

**Design constraints for MVP code:**
- The `published_by` field should be accepted (nullable) in the publish endpoint now — ignored in MVP, populated when auth is enabled

### Future: Webhooks & Events

Enterprise needs to notify external systems when resources change (e.g., trigger CI/CD on publish).

**Event types:**
- `resource.published` — new version published
- `resource.deleted` — resource removed from registry

**Delivery model:**
- Server maintains a webhook subscription list (URL + secret + event filter)
- On matching event, POST JSON payload to subscriber URL with HMAC signature
- Retry with exponential backoff on failure

**Design constraints for MVP code:**
- Server handlers should emit events after successful operations (even if no subscribers exist in MVP) — makes adding webhook delivery a thin layer later

### Future: API Versioning Strategy

Current API is `/api/v1`. Strategy for evolution:

- **v1 is supported indefinitely** — no breaking changes, only additive
- **v2 alongside v1** — when breaking changes are needed, add `/api/v2` while keeping `/api/v1` operational
- **Deprecation**: 6-month sunset period with `Deprecation` header on v1 responses before removal
- **CLI compatibility**: CLI includes its expected API version in requests (`Accept: application/vnd.relava.v1+json`), server routes accordingly

### Future: Offline & Air-gapped Environments

Some enterprise environments can't reach external networks. Need portable resource bundles.

**Commands:**
- `relava export <type> <name> [--version <ver>] --output bundle.tar.gz` — bundle resource + transitive dependencies into a portable archive
- `relava import-bundle bundle.tar.gz` — load resources from archive into local registry

**Bundle format:**
- Tar.gz containing resource directories + a manifest listing all included resources and versions
- Self-contained: includes all transitive dependencies so the target registry can serve them

**Design constraints for MVP code:**
- Resource store layout (`~/.relava/store/<type>/<name>/<version>/`) is already archive-friendly — export is essentially `tar` of store paths

### REST API

Base URL: `http://localhost:7420/api/v1`

#### Resources

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/resources` | List all available resources. Query: `?q=search&type=skill` |
| `GET` | `/resources/:type/:name` | Get resource details |
| `GET` | `/resources/:type/:name/versions` | List versions |
| `GET` | `/resources/:type/:name/versions/:version` | Get specific version details |
| `GET` | `/resources/:type/:name/versions/:version/download` | Download resource files as multipart response (used by `relava install`) |
| `POST` | `/resources/:type/:name` | Publish a resource (multipart upload of directory contents) |
| `DELETE` | `/resources/:type/:name` | Remove resource from registry |

#### Resolution

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/resolve/:type/:name` | Resolve full dependency tree. Query: `?version=1.2.0`. Returns topologically sorted install order as JSON. Skills use DFS; agents use topological sort with cycle detection. |

#### Server

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Server health check |
| `GET` | `/stats` | Server statistics (resource count, project count, etc.) |

#### GUI

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Serve the GUI web application |

---

## 5. CLI Design

### Command Format

All CLI commands follow the pattern:

```
relava <verb> <resource-type> <resource-name>
```

### Global Options

```
relava [--server URL] [--project PATH] [--verbose] [--json] [--no-update-check] <command>
```

- `--server` — Override server URL (default: `http://localhost:7420`)
- `--project` — Override project detection (default: current working directory)
- `--verbose` — Show detailed output
- `--json` — Output as JSON (for scripting)
- `--no-update-check` — Suppress automatic update checks (both resource updates and self-update prompt)

### Commands

#### `relava init`

Initialize current directory as a Relava-managed project.

```bash
$ cd ~/projects/my-app
$ relava init
Created relava.toml
```

What it does:
- Creates an empty `relava.toml` in project root

#### `relava install <resource-type> <resource-name> [options]`

Install a resource into the current project.

**Options:**

| Flag | Description |
|------|-------------|
| `--version <ver>` | Install a specific version (default: latest) |
| `--save` | Write resource and version to `relava.toml` |
| `--global` | Install to `~/.claude/` instead of project |
| `--update` | Update to latest version if already installed |
| `--recursive` | With `--update`, also update transitive dependencies |
| `--force` | Replace existing installation even if same version |
| `--skip-tools` | Skip system tool installation prompts |
| `-y, --yes` | Auto-confirm all prompts (tool installs, etc.) |

```bash
# Install a skill
$ relava install skill denden
Installing skill denden@1.2.0...
  [skill]   .claude/skills/denden/SKILL.md + 3 files
Installed skill denden@1.2.0

# Install and save to relava.toml
$ relava install skill notify-slack --save

# Install a specific version
$ relava install skill notify-slack --version 0.2.0 --save

# Update to latest, including dependencies
$ relava install skill denden --update --recursive

# Force reinstall, skip tool prompts
$ relava install skill denden --force --skip-tools

# Auto-confirm everything
$ relava install skill code-review --save -y

# Install an agent
$ relava install agent debugger --save

# Install a command
$ relava install command commit --save

# Install a rule
$ relava install rule no-console-log
```

What it does:
1. Resolves resource and version from the registry server
2. Resolves transitive dependencies from `metadata.relava` frontmatter
3. Downloads resource files from the registry server
4. Writes files to the correct Claude Code locations in the project
5. Runs tool installation checks (unless `--skip-tools`)
6. Checks required env vars and warns if missing
7. Updates `relava.lock` with installed versions and dependency graph
8. If `--save` is used, writes the resource and version to `relava.toml`

#### `relava install relava.toml`

Install all resources declared in the project's `relava.toml`.

```bash
$ relava install relava.toml
Reading relava.toml...
Installing 4 resources:
  skill denden@1.2.0 ............ ok
  skill notify-slack@0.3.0 ...... ok
  agent debugger@0.5.0 .......... ok
  command commit@0.2.0 .......... ok
All resources installed.
```

This is analogous to `npm install` with no arguments — it reads the manifest and ensures all declared resources are present.

#### `relava remove <resource-type> <resource-name> [--save]`

```bash
$ relava remove skill denden
Removing skill denden@1.2.0...
  Removed .claude/skills/denden/ (4 files)
Removed skill denden.

$ relava remove skill denden --save
Removing skill denden@1.2.0...
  Removed .claude/skills/denden/ (4 files)
Removed skill denden.
Removed from relava.toml
```

What it does:
1. Removes resource files from the project (skill directory or `.md` file)
2. Cleans up empty directories
3. If `--save` is used, removes the entry from `relava.toml`

#### `relava list <resource-type> [--global]`

```bash
$ relava list skills
Project: /Users/woong/projects/my-app

Name              Version  Status
denden            1.2.0    active
notify-slack      0.3.0    active
strawpot-recap    1.0.0    disabled

$ relava list agents
Project: /Users/woong/projects/my-app

Name              Version  Status
debugger          0.5.0    active

$ relava list commands
No commands installed.
```

#### `relava search <query>`

```bash
$ relava search notify
Type     Name              Version  Description
skill    notify-slack      0.3.0    Send messages to Slack via Web API
skill    notify-discord    0.2.1    Send messages to Discord via webhooks
skill    notify-telegram   0.1.0    Send messages to Telegram via Bot API
```

#### `relava info <resource-type> <resource-name>`

```bash
$ relava info skill denden
Name:        denden
Type:        skill
Version:     1.2.0 (latest)
Skills:      notify-slack@0.3.0
Size:        12 KB
```

#### `relava update <resource-type> <resource-name> [--all]`

```bash
$ relava update skill denden
Updating skill denden 1.0.0 -> 1.2.0...
  Updated .claude/skills/denden/SKILL.md
  Updated .claude/skills/denden/lib/helpers.md
Updated skill denden to 1.2.0.

$ relava update --all
Checking 4 resources...
  skill denden: 1.2.0 (up to date)
  skill notify-slack: 0.2.0 -> 0.3.0 (updated)
  skill strawpot-recap: 1.0.0 (up to date)
  agent debugger: 0.5.0 (up to date)
```

#### `relava publish <resource-type> <resource-name> [--path PATH]`

Publish a resource to the local Relava registry server.

```bash
$ relava publish skill denden
Publishing skill denden@1.2.0 to local registry...
  Uploading .claude/skills/denden/ (5 files, 4.2 MB)
Published skill denden@1.2.0

$ relava publish skill my-skill --path ./my-custom-skill/
Publishing skill my-skill@0.1.0 to local registry...
  Uploading ./my-custom-skill/ (2 files, 12 KB)
Published skill my-skill@0.1.0
```

What it does:
1. Reads the resource's `.md` file and parses frontmatter for metadata
2. Validates the resource structure and slug format (see Validation)
3. Collects all files in the resource directory (excludes hidden files/directories)
4. Validates file limits: max **100 files**, **10 MB per file**, **50 MB total**
5. Computes SHA-256 hash for each file
6. Uploads as **multipart HTTP POST** to `POST /api/v1/resources/:type/:name` — the same transport that will work against future cloud registries
7. The server parses the multipart payload, validates server-side, and stores files in `~/.relava/store/<type>/<name>/<version>/`
8. If no version is specified and a prior version exists, the patch version is auto-incremented

By default, publishes from the standard location for the resource type (e.g., `.claude/skills/<name>/` for skills). Use `--path` to specify a custom source directory.

### Publish Validation

Both client-side (CLI) and server-side validation are enforced:

| Check | Client | Server |
|-------|--------|--------|
| Slug format (1-64 chars, lowercase alphanumeric + hyphens, starts with alphanumeric) | Yes | Yes |
| File count (max 100) | Yes | Yes |
| File size (max 10 MB each) | Yes | Yes |
| Total size (max 50 MB) | Yes | Yes |
| Semver format | Yes | Yes |
| File type filtering (text-only for skills/commands/rules) | Yes | Yes |
| Version monotonicity (must be > latest published) | No | Yes |
| Dependency existence (all deps must exist in registry) | No | Yes |
| SHA-256 per file | Yes | Yes |

#### `relava server start [--port PORT] [--daemon]`

```bash
$ relava server start --daemon
Relava server started on http://localhost:7420
GUI available at http://localhost:7420

$ relava server stop
Relava server stopped.

$ relava server status
Relava server is running on http://localhost:7420
  Resources: 12 published, 6 installed in current project
  Projects: 2 registered
```

#### `relava doctor`

Check the health of the Relava installation and project.

```bash
$ relava doctor
Checking Relava installation...
  [ok]   Server running on :7420
  [ok]   Database accessible
  [ok]   Store directory exists
  [ok]   All installed files present on disk
  [ok]   relava.toml in sync
```

#### `relava import <resource-type> <path>`

Import an existing resource directory into the local registry.

```bash
$ relava import skill ./.claude/skills/denden
Detected: 1 skill (denden)
Published skill denden@0.1.0 to local registry.
```

#### `relava validate <resource-type> <path>`

Validate a resource offline before publishing. Runs all client-side checks without uploading.

```bash
$ relava validate skill ./.claude/skills/code-review
Validating skill code-review...
  [ok]   Slug format valid
  [ok]   SKILL.md present
  [ok]   Frontmatter parseable
  [ok]   File count: 3 (max 100)
  [ok]   Total size: 24 KB (max 50 MB)
  [ok]   All files are text
  [ok]   Dependencies exist: security-baseline, style-guide
Validation passed.

$ relava validate skill ./bad-skill
Validating skill bad-skill...
  [ok]   Slug format valid
  [fail] SKILL.md missing
  [fail] Contains binary file: bin/tool (skills must be text-only)
Validation failed: 2 errors.
```

What it checks:
1. Slug format (1-64 chars, lowercase alphanumeric + hyphens)
2. Directory structure (SKILL.md for skills, `<name>.md` for agents/commands/rules)
3. Frontmatter is valid YAML with parseable `metadata.relava`
4. File limits (max 100 files, 10 MB each, 50 MB total)
5. File type filtering (see File Type Filtering below)
6. Semver format if version is present in frontmatter
7. Dependencies exist in the registry (requires server connection)

### File Type Filtering

Different resource types have different file type rules:

| Resource Type | Allowed Files | Rationale |
|---------------|--------------|-----------|
| **Skills** | Text only (`.md`, `.txt`, `.json`, `.yaml`, `.yml`, `.toml`, `.xml`, `.csv`, `.sh`, `.py`, `.js`, `.ts`, `.rb`, `.go`, `.rs`, `.html`, `.css`) | Skills are prompt-layer — injected into context. Binary files waste tokens. |
| **Agents** | Any files | Agents may include compiled binaries or data files. |
| **Commands** | Text only | Commands are markdown instructions. |
| **Rules** | Text only | Rules are markdown instructions. |

Binary detection uses the same heuristic as git: check the first 8,000 bytes for null bytes. If null bytes are found, the file is binary.

Enforced on both `relava validate` and `relava publish`.

#### Startup Self-Update Check

The CLI automatically checks for newer versions of itself at program startup, before any command runs.

**Behavior:**
1. Queries the GitHub Releases API for the latest relava version (throttled to once per 24 hours)
2. If a newer version is available and stdout is a TTY, prompts the user:
   ```
   A new version of relava is available (current: 0.1.0, latest: 0.2.0). Update now? [Y/n]
   ```
3. If the user accepts (Enter or `y`), downloads the release, verifies the SHA-256 checksum, and atomically replaces both the `relava` and `relava-server` binaries
4. If the user declines (`n`), continues normally with the current version

**Non-interactive environments** (non-TTY, CI): prints a notice to stderr but does not block or prompt.

**Suppressed by:** `--json`, `--no-update-check` flags.

**Startup order:**
1. Self-update check (blocking interactive prompt)
2. Resource update check (non-blocking notification, see Issue #49)
3. Command dispatch

#### `relava cache clean [--older-than DURATION]`

Clean cached downloads. Without flags, removes all cached entries. The optional `--older-than` flag removes only entries older than the given duration.

```bash
$ relava cache clean
Cleaned 42 cached entries (128 MB freed)

$ relava cache clean --older-than 7d
Cleaned 12 cached entries older than 7 days (34 MB freed)
```

Duration format: `7d` (days), `24h` (hours), `30m` (minutes). Supports `--json` output.

#### `relava cache status`

Show cache disk usage and entry count.

```bash
$ relava cache status
Cache directory: /Users/you/.relava/cache
  Entries: 42
  Total size: 128 MB
  Oldest entry: 2026-03-01
  Cache limit: 500 MB
```

Supports `--json` output. Automatic LRU eviction runs when cache exceeds the configured limit (default 500 MB).

---

## 6. GUI Design

### Tech Stack

- **Framework**: React (Vite build)
- **Styling**: Tailwind CSS
- **Bundling**: Built into a static SPA, served by the Relava server
- **State**: React Query for API calls, minimal client state

### Pages

#### Dashboard (`/`)
- Registry stats: total resources published, by type
- Recently published/updated resources

#### Resource Browser (`/browse`)
- Search and filter all available resources
- Filter by type (skill, agent, command, rule)
- Sort by name, recently updated
- Resource cards with description, version, type

#### Resource Detail (`/resources/:type/:name`)
- Full README rendered as markdown
- Version history
- Resource contents (file list)
- Dependencies from `metadata.relava` frontmatter

#### Settings (`/settings`)
- Server configuration (port, data directory)
- Cache management (clear cache, store size)
```

---

## 7. Installation and Lifecycle

### How Install Works (per resource type)

All installs download resource files via HTTP from the server (`GET /api/v1/resources/:type/:name/versions/:version/download`). Downloaded files are cached in `~/.relava/cache/` before being written to the project. This ensures the same install pipeline works against both local and future remote registries.

#### Skills

1. Download skill files from server via HTTP
2. Create `.claude/skills/<skill-name>/` in project
3. Write `SKILL.md` and all support files into it
4. Skill is automatically discoverable by Claude Code

#### Agents

1. Download agent files from server via HTTP
2. Write `.md` file to `.claude/agents/<agent-name>.md`
3. Agent is immediately available via `/agents` in Claude Code

#### Commands

1. Download command files from server via HTTP
2. Write `.md` file to `.claude/commands/<command-name>.md`
3. Command is immediately available via `/<command-name>` in Claude Code

#### Rules

1. Download rule files from server via HTTP
2. Write `.md` file to `.claude/rules/<rule-name>.md`
3. Rule is automatically loaded into every Claude Code conversation in this project

#### Hooks (Phase 4+)

1. Read current `.claude/settings.json`
2. Merge hook definitions into the appropriate event arrays (PreToolUse, PostToolUse, etc.)
3. Write updated `settings.json`
4. Record the specific hook entries added (for clean removal)

### How Remove Works

1. Delete resource files from the project (skill directory or `.md` file)
2. Clean up empty directories
3. If `--save` was specified, remove the entry from `relava.toml`

### Update Flow

1. Download new version from the registry server
2. Overwrite existing files with new version
3. Handle new files (add) and removed files (delete)
4. If resource is tracked in `relava.toml`, update the version there

### The `--save` Flag

The `--save` flag controls whether `relava.toml` is modified:

- **Without `--save`**: CLI downloads and writes the resource files, but does NOT touch `relava.toml`.
- **With `--save`**: Same as above, plus writes the resource name and version to `relava.toml`.

This mirrors `npm install --save` behavior. The `relava.toml` file is the declarative manifest that can be committed to version control, allowing collaborators to run `relava install relava.toml` to reproduce the same resource set.

### Lockfile: `relava.lock`

Separate from the editable `relava.toml`, the lockfile tracks the exact state of what's installed for reproducibility. It is auto-generated by the CLI and should be committed to version control.

```json
{
  "version": 1,
  "directInstalls": [
    { "type": "skill", "name": "denden", "version": "1.2.0" },
    { "type": "agent", "name": "debugger", "version": "0.5.0" }
  ],
  "packages": {
    "skill:denden:1.2.0": {
      "type": "skill",
      "name": "denden",
      "version": "1.2.0",
      "dependents": []
    },
    "skill:notify-slack:0.3.0": {
      "type": "skill",
      "name": "notify-slack",
      "version": "0.3.0",
      "dependents": ["skill:denden:1.2.0"]
    },
    "agent:debugger:0.5.0": {
      "type": "agent",
      "name": "debugger",
      "version": "0.5.0",
      "dependents": []
    }
  }
}
```

**`directInstalls`** — resources the user explicitly installed (top-level requests).

**`packages`** — all installed resources including transitive dependencies. Each entry tracks its `dependents` (which resources caused it to be installed).

**Behavior:**
- `relava install` writes/updates `relava.lock` after every install or remove
- `relava install relava.toml` reads `relava.lock` if present and installs exact versions from it (like `npm ci`). If absent, resolves fresh and creates the lockfile.
- `relava update` resolves fresh versions, updates both `relava.toml` (if `--save`) and `relava.lock`
- `relava remove` removes the entry and any orphaned transitive dependencies (packages with no remaining dependents)

---

## 8. Dependency Resolution

Resources declare dependencies via `metadata.relava` in their `.md` frontmatter (see Section 3). Relava resolves these transitively before installation, using a strategy that varies by resource type.

### Resolution Strategies

#### Skills: Client-Side DFS

Skills use a **depth-first search** resolved entirely by the CLI:

1. Read the target skill's `SKILL.md` frontmatter from the local store
2. For each dependency, recursively fetch its manifest and resolve its dependencies
3. Build a flat, deduplicated install list (leaves first)
4. Detect circular dependencies and abort with an error
5. Depth limit of **100 levels** to prevent runaway recursion

```
install skill A
  → A depends on B, C
    → B depends on D
    → C depends on D, E
  → resolved order: D, E, B, C, A  (leaves first, deduplicated)
```

The CLI performs this via the server — it fetches frontmatter from the registry for each dependency.

#### Agents: Server-Side Topological Sort

Agents can have mixed dependencies (skills + other agents), creating a more complex graph. Resolution is performed **server-side**:

1. CLI sends `GET /api/v1/resolve/agent/<name>?version=<ver>` to the local server
2. Server loads the agent's manifest and recursively collects all dependencies
3. Server performs **topological sort** with cycle detection on the full dependency graph
4. Returns a sorted install order (leaves first) as JSON
5. CLI installs each resource in the returned order

```
resolve agent orchestrator
  → orchestrator (agent) depends on: debugger (agent), notify-slack (skill)
    → debugger (agent) depends on: log-capture (skill)
  → server returns: [log-capture, notify-slack, debugger, orchestrator]
```

Server-side resolution is required because agents may depend on a mix of resource types, and the topological sort benefits from having the full registry index available.

### Resolution Behavior

- **Already installed**: If a dependency is already installed at the correct version, it is skipped
- **Version mismatch**: If a dependency is installed at a different version, Relava warns and asks to update
- **Missing from store**: If a dependency version is not published in the local registry, resolution fails with a clear error listing the missing resource
- **Circular dependencies**: Detected and reported as errors — the dependency chain is printed for debugging
- **`--save` propagation**: When installing with `--save`, only the top-level resource is written to `relava.toml`; transitive dependencies are recorded in the database but not in the manifest

### Resolve Command

```bash
# Resolve and display the full dependency tree (does not install)
$ relava resolve skill denden
skill denden@1.2.0
  ├── skill notify-slack@0.3.0
  └── skill strawpot-recap@1.0.0

$ relava resolve agent orchestrator
agent orchestrator@1.0.0
  ├── agent debugger@0.5.0
  │   └── skill log-capture@0.2.0
  └── skill notify-slack@0.3.0

# Output as JSON (for scripting)
$ relava resolve skill denden --json
{
  "root": "skill/denden@1.2.0",
  "order": [
    {"type": "skill", "name": "notify-slack", "version": "0.3.0"},
    {"type": "skill", "name": "strawpot-recap", "version": "1.0.0"},
    {"type": "skill", "name": "denden", "version": "1.2.0"}
  ]
}
```

### Uninstall and Dependency Tracking

When removing a resource, Relava checks whether it is a dependency of other installed resources:

- If **no other resource depends on it**: remove normally
- If **other resources depend on it**: warn and list dependents, require `--force` to proceed
- Orphaned transitive dependencies (no longer needed by any installed resource) are reported but not auto-removed — use `relava remove --prune` to clean them up

---

## 9. Syncing Local Changes Back to Registry

When a resource is installed into a project, the user may modify it (e.g., edit SKILL.md, add files). They may then want to publish those changes back to the registry as a new version.

### The Problem

Installed resource directories may contain files that should NOT be synced back:
- Binaries installed during setup
- Generated artifacts, caches, or build outputs
- OS-specific files (`.DS_Store`, etc.)

### Solution: `.relavaignore` + Install Record

Two mechanisms work together:

1. **`.relavaignore`** — A file in the resource directory (like `.gitignore`) that lists patterns to exclude from publish/sync. Resource authors include this in their package.

```
# .relavaignore
bin/
*.so
*.dylib
*.exe
.DS_Store
```

2. **Install record filtering** — Relava tracks which files it installed in `installations.installed_files_json`. On sync/publish, it can automatically exclude files it knows it placed (binaries, generated artifacts) unless the user explicitly modified them.

### Change Detection

Before publishing, Relava compares the local resource directory against the version currently in the registry. If nothing has changed, it skips the publish and reports "no changes detected."

```bash
$ relava publish skill denden
Comparing skill denden against registry version 1.2.0...
No changes detected. Nothing to publish.

$ relava publish skill denden
Comparing skill denden against registry version 1.2.0...
  [modified] SKILL.md
  [added]    templates/review-checklist.md
Publish as 1.3.0? [Y/n] y
Published skill denden@1.3.0
```

Change detection works by comparing SHA-256 checksums of each file (after applying `.relavaignore` filters) against the checksums stored in the registry for the latest version. This is a content-level comparison — timestamps are ignored.

### Publish Flow

On `relava publish`, the CLI:
1. Reads `.relavaignore` if present
2. Collects all files in the resource directory, excluding ignored patterns
3. Computes SHA-256 per file and compares against the latest published version in the registry
4. If no files changed: print "no changes detected" and exit
5. If changes exist: show a diff summary (added/modified/removed files) and prompt for confirmation
6. Validates file limits (100 files, 10 MB each, 50 MB total)
7. Uploads via multipart HTTP POST as a new version

### Name Conflicts on Publish

When publishing a new or modified resource to the registry and the name already exists:

- **Same resource, new version**: The normal case. Relava requires a version bump — the new version must be strictly greater than the latest published version.
- **Name taken by another resource**: Reject with error: `"Resource 'code-review' already exists in the registry. Choose a different name."` In the future, scoping (`@org/code-review`) will allow multiple owners to use the same base name.

---

## 10. Implementation Plan

### Phase 1: Core CLI + Resource Format + Local Storage (Weeks 1-3)

**Goal**: A working CLI that can install, remove, and list resources locally.

| Week | Deliverable |
|------|-------------|
| 1 | Project scaffolding (Rust CLI with clap). `relava.toml` project manifest parser. Frontmatter parser for `metadata.relava` dependencies. Resource validation. |
| 2 | Local store (`~/.relava/store/`). SQLite database setup with schema. `relava init`, `relava install <type> <name>` (from local store), `relava remove <type> <name>`. `--save` flag support. |
| 3 | `relava list <type>`, `relava info <type> <name>`, `relava update <type> <name>`, `relava doctor`. `relava install relava.toml` (bulk install from manifest). `relava import` for converting existing resource directories. |

**Milestone**: Developer can publish a resource to local store, install it into a project, list installed resources, remove, and update — all via CLI.

### Phase 2: Local Registry Server + REST API (Weeks 4-5)

**Goal**: A running HTTP server that the CLI and GUI can talk to.

| Week | Deliverable |
|------|-------------|
| 4 | HTTP server (Axum). Core REST endpoints: resources CRUD. `relava server start/stop/status`. CLI uses REST API for all operations. |
| 5 | Installation endpoints. Search endpoint with full-text search (SQLite FTS5). Health and stats endpoints. `relava publish <type> <name>` command (uploads directory to server). Server serves static files for future GUI. |

**Milestone**: CLI works against the running server. All operations available via REST API. Server manages all state. Resources are published and installed through the server.

### Phase 3: GUI (Weeks 6-8)

**Goal**: A web-based GUI for browsing and managing resources.

| Week | Deliverable |
|------|-------------|
| 6 | React app scaffolding (Vite + Tailwind). Dashboard page. Project list and project view with installed resources grouped by type. |
| 7 | Resource browser with search and type filtering. Resource detail page with README rendering. Install/remove from GUI. |
| 8 | Settings page. Disable/enable toggle. Update flow with diff preview. Polish and responsive design. |

**Milestone**: Developer can manage all resources through a web GUI at `localhost:7420`.

### Phase 4: Advanced Features (Weeks 9+)

- Hook installation and management
- Resource templates (`relava create skill`, `relava create agent`)
- Project scaffolding (`relava new project`)
- Auto-update notifications
- CLAUDE.md auto-management (adding/removing skill references)
- Version conflict resolution
- Cache management and cleanup

---

## 11. Tech Stack Recommendations

### CLI + Server: Rust

| Aspect | Choice | Rationale |
|--------|--------|-----------|
| Language | **Rust** | Single binary distribution (critical for a developer tool). Fast startup. Strong TOML/SQLite ecosystem. |
| CLI framework | **clap** | De facto standard for Rust CLIs. Derive macros for ergonomic command definitions. |
| HTTP server | **Axum** | Async, performant, well-maintained. Pairs with tokio. |
| Database | **SQLite via rusqlite** | Zero-config, file-based, perfect for local-first. FTS5 for search. |
| TOML parsing | **toml** crate | Native format, first-class Rust support. |
| YAML parsing | **serde_yaml** | For `.md` frontmatter parsing. |
| HTTP client | **reqwest** | For CLI-to-server communication. |
| Console output | **comfy-table** | Rich tables for list/search/info output. |
| Colors | **colored** | Colored status tags (`[ok]`, `[fail]`, `[warn]`). |
| Logging | **tracing** | Structured logging, compatible with Axum. |

### Console Output

The CLI uses rich formatted output for readability:

**Tables** (`comfy-table`) for list, search, and info commands:
```
Name              Version  Type    Description
notify-slack      0.3.0    skill   Send messages to Slack via Web API
notify-discord    0.2.1    skill   Send messages to Discord via webhooks
```

**Status tags** (`colored`) for install, validate, and doctor:
```
  [ok]     Server running on :7420
  [skill]  .claude/skills/denden/SKILL.md + 3 files
  [tool]   gh — installed
  [warn]   Missing required env: GITHUB_TOKEN
  [fail]   SKILL.md missing
```

**JSON mode** (`--json`) outputs structured JSON for scripting:
```bash
$ relava list skills --json
[{"name":"denden","version":"1.2.0","type":"skill"},{"name":"notify-slack","version":"0.3.0","type":"skill"}]
```

### GUI: React + Vite

| Aspect | Choice | Rationale |
|--------|--------|-----------|
| Framework | **React 19** | Widely known, large ecosystem. |
| Build tool | **Vite** | Fast dev server, optimized production builds. |
| Styling | **Tailwind CSS** | Utility-first, fast iteration, small bundle. |
| API layer | **TanStack Query (React Query)** | Caching, refetching, loading states for REST calls. |
| Markdown rendering | **react-markdown** | For rendering resource READMEs. |
| Bundling | Built as static SPA, embedded in Rust binary via `include_dir` or served from `~/.relava/gui/`. |

### Why Rust over alternatives

- **vs. Go**: Rust's `clap` + `serde` make CLI + config parsing more ergonomic. Single binary either way, but Rust's type system catches more bugs at compile time for a package manager where correctness matters.
- **vs. TypeScript/Node**: Node requires a runtime. Developer tools should be single binaries. Also, npm-inception is awkward.
- **vs. Python**: Same runtime problem. Also slower for file operations at scale.

The GUI is React because it's the most practical choice for a small web application that needs to look polished — Tailwind + React Query is a well-trodden path.

---

## 12. Open Questions

### Must Resolve Before Phase 1

1. ~~**Global vs. project resources**~~ **Resolved.** `relava install --global` targets `~/.claude/` (skills → `~/.claude/skills/`, agents → `~/.claude/agents/`, etc.). On conflict, project-local takes precedence (Claude Code's existing behavior). `relava list` shows both scopes with `[global]`/`[local]` labels.

2. ~~**CLAUDE.md management**~~ **Resolved.** Relava does not touch `CLAUDE.md`. Skills are auto-discovered by Claude Code from `.claude/skills/` directories — no CLAUDE.md reference needed. Resource authors document any manual steps in their README. Also resolves #8.

3. ~~**settings.json env injection**~~ **Resolved.** Warn only — Relava never modifies `.claude/settings.json` for env vars. At install time, print required env vars and where to set them. `relava doctor` checks all installed resources and reports any env vars missing from both the process environment and `.claude/settings.json` `env` entries.

### Must Resolve Before Phase 2

4. ~~**Port selection**~~ **Resolved.** Default port `7420`, stable. If occupied, fail with a clear error suggesting `--port` override or `relava server stop`. Active port is written to `~/.relava/config.toml` so the CLI always knows where to connect.

5. ~~**Authentication**~~ **Resolved.** No auth for MVP. Server binds to `127.0.0.1` only (not `0.0.0.0`), limiting access to the local machine. Revisit if/when a cloud registry is introduced.

6. ~~**Concurrency**~~ **Resolved.** SQLite WAL mode for concurrent reads. Server serializes publish operations per resource name. CLI handles its own project-level concurrency (file writes are local).

### Design Decisions Deferred

7. **Hook management complexity**: Deferred to Phase 4 implementation. Will decide merge strategy when we have concrete hook install/remove scenarios to test against.

8. ~~**CLAUDE.md skill trigger descriptions**~~ **Resolved by #2.** Relava does not manage CLAUDE.md or trigger descriptions.

9. ~~**Migration tooling**~~ **Resolved.** No additional migration DSL. The update flow already diffs files (add/remove/overwrite). No migration scripts needed for now.

10. ~~**Resource naming**~~ **Resolved.** Flat namespace with slug validation (1-64 chars, lowercase alphanumeric + hyphens). Scoped namespaces (`@org/name`) deferred to when/if a cloud registry is introduced.

---

## 13. Implementation Order

Trackable checklist of every deliverable from the Implementation Plan (Section 8). Items are numbered sequentially across all phases. Status key: ⬜ Not Started · 🟡 In Progress · ✅ Complete.

### Phase 1: Core CLI + Resource Format + Local Storage

#### Week 1 — Scaffolding & Parsing

- ✅ 1. Project scaffolding — Rust workspace, Cargo.toml, clap CLI skeleton with global options (`--server`, `--project`, `--verbose`, `--json`)
- ✅ 2. Frontmatter parser — parse `metadata.relava` block from `.md` files to extract skill/agent dependency declarations
- ✅ 3. `relava.toml` parser — project manifest format (skills, agents, commands, rules sections with name=version constraint entries: `"X.Y.Z"` or `"*"`)
- ✅ 3a. Version constraint resolver — parse and resolve `"*"` to latest, `"X.Y.Z"` to exact version from local store
- ✅ 4. Resource validation — validate directory structure per resource type (skill needs `SKILL.md`, agent needs `<name>.md`, etc.)
- ✅ 4a. Slug validation — enforce slug format (1-64 chars, lowercase alphanumeric + hyphens, starts/ends with alphanumeric, no consecutive hyphens) on all resource names
- ✅ 5. Resource validation — validate manifest fields (semver format, valid type enum)

#### Week 2 — Local Store & Core Commands

- ✅ 6. Local store directory structure — create and manage `~/.relava/store/<type>/<name>/<version>/`
- ✅ 7. SQLite database setup — schema creation (resources, versions tables), migrations
- ✅ 8. `relava init` — create empty project `relava.toml`
- ✅ 9. `relava install <type> <name>` — resolve version, download files via HTTP from server, write to correct Claude Code locations. Flags: `--version`, `--save`, `--global`, `--update`, `--recursive`, `--force`, `--skip-tools`, `-y/--yes` — *depends on 6, 7, 3a*
- ✅ 9a. HTTP download transport — implement `GET /resources/:type/:name/versions/:version/download` client, cache downloaded files in `~/.relava/cache/` — *depends on 6*
- ✅ 10. Skill installation logic — write `SKILL.md` + support files to `.claude/skills/<name>/`, handle multi-file directories
- ✅ 10a. Tool installation — parse `metadata.relava.tools`, check PATH via `which`, detect OS, prompt user, execute install commands — *depends on 2*
- ✅ 10b. Env var checking — parse `metadata.relava.env`, check required vars against environment and `.claude/settings.json`, warn if missing — *depends on 2*
- ✅ 11. Agent/command/rule installation logic — write `.md` file to `.claude/agents/`, `.claude/commands/`, or `.claude/rules/`
- ✅ 12a. Dependency resolution from frontmatter — parse `metadata.relava.skills` and `metadata.relava.agents` from `.md` files in the registry — *depends on 2*
- ✅ 12b. Client-side DFS resolver for skills — recursively resolve skill dependencies from local store, build deduplicated leaf-first install order, detect circular deps, enforce depth limit of 100 — *depends on 12a*
- ✅ 12c. Dependency-aware install — install transitive dependencies in resolved order before the target resource, skip already-installed versions — *depends on 9, 12b*
- ✅ 13. Lockfile management — write/update `relava.lock` after install/remove with directInstalls and packages (including dependents tracking) — *depends on 9, 12c*
- ✅ 14. `relava remove <type> <name>` — delete resource files, remove from lockfile, clean up orphaned transitive deps
- ✅ 15. `--save` flag — write resource name + version to project `relava.toml` on install, remove entry on remove — *depends on 3*

#### Week 3 — Remaining CLI Commands

- ✅ 16. `relava list <type>` — list installed resources for current project with version and status (active/disabled)
- ✅ 17. `relava info <type> <name>` — display full resource details (dependencies, size)
- ✅ 18. `relava update <type> <name>` — download new version from registry, overwrite project files — *depends on 9*
- ✅ 19. `relava update --all` — check and update all installed resources in current project — *depends on 18*
- ✅ 20. `relava doctor` — check server reachability, validate project relava.toml against installed files
- ⬜ 21. `relava install relava.toml` — read project manifest, use `relava.lock` for exact versions if present, otherwise resolve fresh and create lockfile — *depends on 3, 9, 13*
- ⬜ 22. `relava import <type> <path>` — scan existing resource directory/file, validate structure, publish to registry
- ⬜ 22a. `relava resolve <type> <name>` — display full dependency tree (tree view + `--json` output), does not install — *depends on 12b*
- ⬜ 22b. `relava validate <type> <path>` — offline pre-publish validation (slug, structure, frontmatter, file limits, file type filtering, semver, deps) — *depends on 4, 4a, 5*
- ⬜ 22c. File type filtering — binary detection (null-byte check in first 8KB), enforce text-only for skills/commands/rules, any files for agents — *depends on 4*
- ⬜ 22d. Rich console output — `comfy-table` for tables (list/search/info), `colored` for status tags, `--json` mode for structured output
- ⬜ 23. Disable/enable mechanism — rename files with `.disabled` suffix
- ⬜ 24. End-to-end integration testing — publish to local store, install into test project, list, update, remove cycle

**Phase 1 Milestone**: Developer can publish a resource to local store, install it into a project, list installed resources, remove, and update — all via CLI.

---

### Phase 2: Local Registry Server + REST API

#### Week 4 — Server Foundation

- ⬜ 25. HTTP server scaffolding — Axum + tokio async runtime, server startup/shutdown lifecycle
- ⬜ 26. `relava server start` / `stop` / `status` commands — daemon mode, PID management, port binding
- ⬜ 27. Resources REST endpoints — `GET /resources`, `GET /resources/:type/:name`, `POST /resources/:type/:name`, `DELETE /resources/:type/:name` — *depends on 25*
- ⬜ 28. Resource versions REST endpoints — `GET /resources/:type/:name/versions`, `GET /resources/:type/:name/versions/:version` — *depends on 27*
- ⬜ 30. CLI refactor — all operations go through REST API, fail with clear error if server is unreachable — *depends on 27*

#### Week 5 — Server Features & Publish

- ⬜ 31a. Resolution endpoint — `GET /api/v1/resolve/:type/:name?version=<ver>`, server-side topological sort with cycle detection for agents (mixed skill + agent dependencies), returns sorted install order as JSON — *depends on 27*
- ⬜ 31b. CLI integration for server-side resolve — agent installs use the resolve endpoint for dependency resolution — *depends on 30, 31a*
- ⬜ 32. Search endpoint with SQLite FTS5 — `GET /resources?q=search&type=skill`, full-text indexing of name + description + keywords
- ⬜ 33. `relava search <query>` CLI command — search resources via server API — *depends on 32*
- ⬜ 34. Health and stats endpoints — `GET /health`, `GET /stats` (resource count, version count)
- ⬜ 35. `relava publish <type> <name>` — read manifest, validate slug + fields + file limits + file type filtering (100 files / 10MB each / 50MB total), compute SHA-256 per file, multipart HTTP POST to server — *depends on 27, 4a, 22c*
- ⬜ 35a. Server-side publish validation — parse multipart payload, validate slug format, semver, version monotonicity, dependency existence, file limits, store in `~/.relava/store/` — *depends on 27*
- ⬜ 35b. Download endpoint — `GET /resources/:type/:name/versions/:version/download` serves resource files for CLI install — *depends on 27*
- ⬜ 35c. Version auto-increment — on publish without explicit version, auto-increment patch from latest published version — *depends on 35a*
- ⬜ 36. `relava publish <type> <name> --path PATH` — publish from custom source directory — *depends on 35*
- ⬜ 36a. `.relavaignore` support — exclude file patterns from publish/sync, works like `.gitignore`, combined with install record filtering — *depends on 35*
- ⬜ 36b. Publish change detection — compare local resource directory against registry version using SHA-256 checksums, skip publish if no changes, show diff summary and prompt for confirmation — *depends on 35*
- ⬜ 37. Static file serving — server serves files from GUI directory for future web app — *depends on 25*

**Phase 2 Milestone**: CLI works against the running server. All operations available via REST API. Resources are published and installed through the server.

---

### Phase 3: GUI

#### Week 6 — App Shell & Dashboard

- ⬜ 38. React app scaffolding — Vite + Tailwind CSS + TanStack Query, project structure, API client setup
- ⬜ 39. App shell — navigation header (Dashboard, Browse, Settings), layout components, routing
- ⬜ 40. Dashboard page — registry stats (total resources by type), recently published/updated resources

#### Week 7 — Resource Browser & Details

- ⬜ 43. Resource browser page — search input, type filter (skill/agent/command/rule), sort options, resource cards with description and version — *depends on 38*
- ⬜ 44. Resource detail page — full README rendered as markdown (react-markdown), version history, file list, dependencies from frontmatter

#### Week 8 — Settings & Polish

- ⬜ 47. Settings page — server configuration (port, data directory), cache size and cleanup
- ⬜ 50. GUI build pipeline — production build, embed static assets into Rust binary (or serve from `~/.relava/gui/`)
- ⬜ 51. Responsive design pass — ensure usable at various viewport sizes, visual polish

**Phase 3 Milestone**: Developer can browse and search the registry through a web GUI at `localhost:7420`.

---

### Phase 4: Advanced Features (Weeks 9+)

No week assignments — each feature is an independent work item.

- ⬜ 52. Hook installation — read `settings.json`, merge hook definitions into event arrays (PreToolUse, PostToolUse, etc.)
- ⬜ 53. Hook removal — remove specific hook entries from `settings.json`
- ⬜ 54. Resource templates — `relava create skill <name>`, `relava create agent <name>` scaffolding with starter `.md` files and frontmatter
- ✅ 56. Auto-update notifications — CLI check (throttled once/hour, batch POST to server), GUI UpdateBanner component with amber badge. Suppressed by --no-update-check and --json flags
- ✅ 58. Self-update check at startup — blocking interactive prompt at program startup (throttled once/24h), checks GitHub Releases API, SHA-256 verified atomic binary replacement for both relava and relava-server. Suppressed by --no-update-check, --json, or non-TTY
- ✅ 59. Cache management — `relava cache clean [--older-than DURATION]` and `relava cache status` commands, LRU eviction policy, disk usage reporting with entry counts

---

## Appendix A: Claude Code Resource Locations Reference

| Resource | Location | Discovery |
|----------|----------|-----------|
| Skills | `./.claude/skills/<name>/SKILL.md` or `~/.claude/skills/<name>/SKILL.md` | Auto-discovered by Claude Code |
| Agents | `.claude/agents/<name>.md` | Available via `/agents` command |
| Commands | `.claude/commands/<name>.md` | Available via `/<name>` command |
| Rules | `.claude/rules/<name>.md` | Auto-loaded into every conversation |
| Hooks | `.claude/settings.json` → `hooks` object | Auto-executed on matching events |
| Env vars | `.claude/settings.json` → `env` object | Injected into Claude session |
| Permissions | `.claude/settings.json` → `permissions` object | Controls tool access |

## Appendix B: Comparison with Existing Tools

| Feature | npm | brew | Relava |
|---------|-----|------|--------|
| Package format | package.json + node_modules | Formula (Ruby DSL) | .md frontmatter + directory |
| Registry | npmjs.com | Homebrew/core | Local server |
| Install target | node_modules/ | /usr/local/ | .claude/ |
| GUI | npmjs.com (web) | None | Built-in local web GUI |
| Manifest | package.json | Brewfile | relava.toml |
| Scope | JS packages | System software | Claude Code artifacts |
| Bundling | tar.gz via npm pack | Source build | None (directory as-is) |

Relava is closest in spirit to **brew** (installing self-contained resources into known locations) but with **npm**'s project-level manifest and version pinning via `relava.toml`.
