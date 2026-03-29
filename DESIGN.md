# Relava — Plan

> The reliability layer for Claude Code — score, compare, and safely improve skills and agents.

---

## 1. Vision and Goals

### What Relava Is

Relava is the **reliability layer for Claude Code**. It manages the prompt-layer artifacts that shape how Claude thinks and behaves (skills, agents, commands, rules, hooks), then observes how they perform via hooks and scores each resource on compliance and error recovery.

Relava is built in two layers:

1. **Layer 1: Resource Management** — a local package manager and registry for Claude Code resources. Install, publish, version, and resolve dependencies. Think `npm` or `brew`, but for Claude Code extensions.
2. **Layer 2: Runtime Scoring** — hooks observe Claude Code's execution and score each skill and agent against their declared contracts. Scores feed into version comparison and score-driven auto-update.

Relava runs **entirely on the developer's machine**. There is no cloud dependency. The local daemon is the single source of truth for published resources and scoring data.

### Why It Exists

Claude Code's extension model is file-based — skills are directories, agents are `.md` files, commands are `.md` files, rules are `.md` files, hooks are JSON in `settings.json`. There is no built-in package manager, no versioning, no dependency tracking, no discovery mechanism. Developers manually copy files between projects. And there is no way to know whether a skill or agent is performing well or regressing.

Relava solves this by providing:

- **Individual resource management** — each skill, agent, command, and rule is versioned and managed independently
- **Version management** so resources can be updated, rolled back, and pinned
- **Runtime scoring** — hooks observe execution and score compliance against declared contracts
- **Version comparison** — see how scores changed between resource versions
- **Score-driven auto-update** — automatically update to better-scoring versions
- **A local registry** with GUI for browsing, searching, and managing resources
- **A CLI** that reads a project's `relava.toml` and fetches resources from the registry
- **A declarative manifest** (`relava.toml`) for reproducible project setups

### Design Principles

1. **Local-first.** Everything works offline. No account required. SQLite + filesystem.
2. **Observe, don't replace.** Claude Code already has an execution runtime. Relava observes it via hooks and scores the output. It does not inject itself into the execution loop.
3. **Deterministic first, LLM second.** Tool compliance, skill compliance, delegation compliance, and error recovery are checkable without an LLM. LLM evaluation (purpose alignment, instruction alignment, delegation quality) is opt-in and costs tokens. Deterministic scores are the baseline; LLM scores are a premium layer.
4. **Non-invasive.** Resources install to standard locations. Remove Relava and your agents still work. Hooks observe but never block.
5. **Invisible by default.** The human uses skills and agents normally. Relava handles frontmatter inference, scoring, and improvement in the background. No manual frontmatter authoring, no score dashboards to monitor, no version decisions to make.
6. **Single target platform.** Claude Code first. Codex and Gemini CLI later as separate `agent_type` targets. No hybrid mixing.
7. **Prompt-layer only.** Relava manages text/files that get injected into Claude's context. It does NOT manage infrastructure (MCP servers, runtimes, databases).
8. **Multi-file aware.** Skills can contain templates and support files. Relava handles the full complexity.
9. **Individual resources.** No bundling or archive step. Each resource is published and installed independently, with its directory contents uploaded as-is.

---

## 2. Architecture Overview

```
+------------------------------------------------------+
|                   Developer Machine                   |
|                                                       |
|  +-------------+     +----------------------------+   |
|  |  relava CLI  |---->|   Relava Daemon             |   |
|  +-------------+     |                            |   |
|        |              |  REST API (:7420)          |   |
|        |              |  Resource Store             |   |
|  +-------------+     |  SQLite Metadata DB        |   |
|  |  Relava GUI  |---->|  Hook Event Endpoints      |   |
|  | (Web App)    |     |  Scoring Engine             |   |
|  +-------------+     +----------------------------+   |
|                              ^                        |
|  +---------------------------------------------------+|
|  |              Project Filesystem                    ||
|  |  (managed by CLI, not by daemon)                   ||
|  |                                                    ||
|  |  .claude/                                          ||
|  |    skills/        <-- skill directories            ||
|  |    agents/        <-- agent .md files              ||
|  |    commands/       <-- command .md files            ||
|  |    rules/          <-- rule .md files              ||
|  |  relava.toml       <-- project resource declarations ||
|  +---------------------------------------------------+|
|                                                       |
|  Claude Code hooks ----HTTP POST----> Daemon (:7420)  |
|    (SubagentStart, PreToolUse, etc.)                  |
|                                                       |
|  +---------------------------------------------------+|
|  |         ~/.relava/  (Daemon State)                 ||
|  |                                                    ||
|  |  store/            <-- published resource files    ||
|  |  db.sqlite         <-- resource + scoring metadata ||
|  |  config.toml       <-- daemon configuration        ||
|  |  cache/            <-- download cache              ||
|  |  scores/           <-- RunRecords per resource     ||
|  |  trajectories/     <-- raw hook event logs         ||
|  |  manifests/        <-- deduplicated snapshots      ||
|  |  daemon.state      <-- session + crash recovery    ||
|  +---------------------------------------------------+|
+------------------------------------------------------+
```

### Daemon Architecture

Relava runs as a **local daemon process**, not as an on-demand process spawned per hook event.

**Lifecycle:**
- `relava daemon start` — starts the daemon (background process)
- `relava daemon stop` — stops the daemon
- `relava init` starts the daemon automatically as part of project setup

**The daemon IS the local server.** It hosts:
1. **REST API endpoints** for CLI commands (`relava info`, `relava scores`, `relava compare`, etc.)
2. **Hook event endpoints** that Claude Code hooks POST events to

**Why a daemon, not per-event process spawning:**
- **State continuity** — the daemon maintains the call stack in memory across hook events within a session. Per-event spawning would require serializing/deserializing call stack state to disk on every hook event.
- **No process spawn overhead** — hook events fire frequently (every tool use). Spawning a process per event adds latency. The daemon receives HTTP POSTs with near-zero overhead.
- **Batch trajectory writes** — the daemon buffers trajectory events in memory and flushes to disk periodically, rather than opening/writing/closing the file on every event.
- **Consistent with hook handler type** — hooks use the HTTP handler type (not command type), POSTing events to the daemon's HTTP endpoints.

The daemon listens on `localhost:7420` by default. It is the same process that serves the registry REST API — one daemon, one port, serving both hook events and CLI queries.

### Component Interactions

1. **Daemon** is both the registry server and the hook event processor. It stores published resources, serves them via REST API, receives hook events from Claude Code, and runs the scoring engine.
2. **CLI** reads the project's `relava.toml`, requests resources from the daemon, and writes files to the project filesystem. The CLI manages all project-level operations.
3. **GUI** is a web application served by the daemon for browsing and searching the registry.
4. **Project Filesystem** is managed entirely by the CLI — the daemon never touches it.
5. **Claude Code Hooks** POST events to the daemon's HTTP endpoints during agent execution. The daemon processes these events to build call trees, track trajectories, and compute scores.

### Crate Structure

The codebase is a Cargo workspace with four crates, split by concern and license:

```
relava-types        (Apache-2.0)   Shared types, validation, versioning, manifest parsing,
    ^                              RunRecord/Violation/RelavaEvent schemas
    |                              Zero IO dependencies — pure logic only
    |
    +--- relava-cli     (Apache-2.0)   CLI binary, registry client, caching,
    |                                  dependency resolution, env checks, tool checks,
    |                                  hooks, scoring engine, trajectory storage,
    |                                  compare, scores, auto-update, frontmatter inference
    |
    +--- relava-server  (ELv2)         Daemon server, REST API, storage layer,
             ^                         SQLite DB, blob store, web GUI,
             |                         hook event endpoints, score storage/querying
             |
         relava-server-ext (ELv2)      Cloud and enterprise extensions (future)
                                       (depends on both relava-server and relava-types)
```

| Crate | Contains | License |
|-------|----------|---------|
| `relava-types` | `manifest`, `validate`, `version`, `file_filter`, `run_record`, `violation`, `relava_event`, `implication_record` modules | Apache-2.0 |
| `relava-cli` | `install`, `remove`, `update`, `list`, `info`, `search`, `publish`, `import`, `validate`, `init`, `doctor`, `disable`, `enable`, `resolve`, `cache_manage`, `self_update`, `update_check`, `bulk_install`, `lockfile`, `save`, `registry`, `cache`, `api_client`, `env_check`, `tools`, `output`, `server`, `hooks`, `scoring`, `compare`, `scores`, `auto_update`, `frontmatter`, `daemon` | Apache-2.0 |
| `relava-server` | `store` (traits, db, blob, models, dirs), HTTP handlers, dependency resolver, GUI serving, hook event handlers, score storage, trajectory storage | ELv2 |
| `relava-server-ext` | Stub — future cloud/enterprise extensions | ELv2 |

The split licensing keeps shared types and the CLI open source (Apache-2.0) while protecting the server against competing managed services (ELv2).

### REST-First Architecture

All CLI operations go through the daemon's REST API — there is no direct mode. If the daemon is not running, the CLI prints an error: `Daemon not reachable. Run 'relava daemon start' first.`

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

# Scoring configuration (Layer 2)
[scoring]
llm_eval = false           # Enable LLM-as-judge evaluation (costs tokens)
# api_key should be set in ~/.relava/config.toml or RELAVA_API_KEY env var, not here

# Auto-update configuration (Layer 2)
[auto_update]
enabled = true             # Auto-update to better-scoring versions
min_runs = 5               # Minimum runs before trusting a version's scores
require_no_regression = true # Newer version must not score worse on any metric
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

## 4. Local Daemon Design

The Relava daemon is a local registry and scoring engine. It stores published resources, serves the GUI, receives hook events from Claude Code, and computes compliance scores. It is the single source that `relava install` pulls from, `relava publish` pushes to, and Claude Code hooks POST events to.

### Storage

```
~/.relava/
  config.toml          # Daemon config (port, defaults, scoring settings)
  db.sqlite            # Resource metadata + scoring data
  daemon.state         # Current session ID, last event timestamp (crash recovery)
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
  scores/              # RunRecords per resource per version
    registry/          # RunRecords for published registry versions
      skill/
        code-review/
          1.2.0/       # One RunRecord JSON file per run
      agent/
        orchestrator/
          1.0.0/
    local/             # RunRecords for unpublished local changes
      skill/
        my-skill/
          <content-hash>/  # Keyed by content hash of current local version
  trajectories/        # Raw hook event logs
    <session_id>.jsonl # One JSON line per hook event, tagged with call stack
  manifests/           # Deduplicated manifest snapshots
    <content_hash>.json # Resource name → version mapping
  frontmatter/         # Inferred frontmatter for resources without explicit declarations
    skill/
      code-review/
        1.2.0.json     # Auto-inferred purpose, tools, constraints
  cache/               # Temporary cache
  logs/                # Daemon logs
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

The daemon does not track projects or installations. Project management is handled entirely by the CLI via `relava.toml`.

### Layer 2 Database Tables (Scoring)

```sql
-- RunRecords: one per resource per session
CREATE TABLE run_records (
  id                    TEXT PRIMARY KEY,
  session_id            TEXT NOT NULL,
  parent_id             TEXT,          -- RunRecord ID of calling agent (null for top-level)
  call_depth            INTEGER NOT NULL DEFAULT 0,

  -- Deterministic scores (always computed, free)
  tool_compliance       REAL,          -- 0.0–1.0
  skill_compliance      REAL,          -- 0.0–1.0 (agents only, null for skills)
  delegation_compliance REAL,          -- 0.0–1.0 (agents only, null for skills)
  error_recovery_rate   REAL,          -- 0.0–1.0 (null if no errors)

  -- LLM-evaluated scores (opt-in, null if disabled)
  purpose_alignment     REAL,          -- 0.0–1.0
  instruction_alignment REAL,          -- 0.0–1.0
  delegation_quality    REAL,          -- 0.0–1.0 (agents only, null for skills)

  -- Metadata
  completion            TEXT NOT NULL,  -- 'completed' | 'partial' | 'abandoned'
  data_complete         BOOLEAN NOT NULL DEFAULT TRUE,
  tool_calls            INTEGER,
  errors                INTEGER,
  recoveries            INTEGER,
  wall_time_ms          INTEGER,
  timestamp             TIMESTAMP NOT NULL,

  -- Identity
  resource_type         TEXT NOT NULL,  -- 'skill' | 'agent'
  resource_name         TEXT NOT NULL,
  resource_version      TEXT,
  project_dir           TEXT,
  is_local              BOOLEAN NOT NULL DEFAULT FALSE,

  -- Manifest snapshot reference
  manifest_hash         TEXT,           -- content hash of installed resource versions

  -- Trajectory reference
  trajectory_id         TEXT            -- pointer to raw event log (= session_id)
);

CREATE INDEX idx_run_records_session ON run_records(session_id);
CREATE INDEX idx_run_records_resource ON run_records(resource_type, resource_name, resource_version);

-- Violations: structured record of every compliance failure
CREATE TABLE violations (
  id                INTEGER PRIMARY KEY,
  run_record_id     TEXT NOT NULL REFERENCES run_records(id),
  type              TEXT NOT NULL,  -- 'tool' | 'skill' | 'delegation' | 'purpose' | 'instruction' | 'recovery_failure'
  severity          TEXT NOT NULL,  -- 'hard' (deterministic) | 'soft' (LLM-evaluated)
  what_happened     TEXT NOT NULL,
  context           TEXT,
  declared          TEXT,           -- what the frontmatter declares
  actual            TEXT,           -- what the agent actually did
  trajectory_offset INTEGER,       -- line number in trajectory file
  timestamp         TIMESTAMP
);

CREATE INDEX idx_violations_run ON violations(run_record_id);

-- ImplicationRecords: LLM judge root-cause analysis linking failures to skills/agents
CREATE TABLE implication_records (
  record_id         TEXT PRIMARY KEY,
  run_record_id     TEXT NOT NULL REFERENCES run_records(id),
  agent_name        TEXT NOT NULL,
  agent_version     TEXT,
  skill_name        TEXT,           -- implicated skill (null for AGENT/ENVIRONMENT types)
  skill_version     TEXT,
  implication_type  TEXT NOT NULL,  -- 'SKILL' | 'AGENT' | 'MIXED' | 'ENVIRONMENT'
  severity          TEXT NOT NULL,  -- 'primary' | 'contributing'
  category          TEXT NOT NULL,  -- 'instruction_gap' | 'missing_edge_case' | 'conflicting_guidance'
                                    -- | 'over_permissive' | 'under_specified' | 'ambiguous_instruction'
                                    -- | 'stale_reference'
  summary           TEXT NOT NULL,  -- LLM judge's explanation
  confidence        REAL,           -- 0.0–1.0
  timestamp         TIMESTAMP NOT NULL
);

CREATE INDEX idx_implications_run ON implication_records(run_record_id);
CREATE INDEX idx_implications_skill ON implication_records(skill_name, skill_version);

-- Inferred frontmatter: auto-generated contracts for resources without explicit declarations
CREATE TABLE inferred_frontmatter (
  id                INTEGER PRIMARY KEY,
  resource_type     TEXT NOT NULL,
  resource_name     TEXT NOT NULL,
  resource_version  TEXT,
  content_hash      TEXT NOT NULL,  -- hash of resource content that was analyzed
  purpose           TEXT,           -- inferred purpose statement
  expected_tools    TEXT,           -- JSON list of expected tools
  constraints       TEXT,           -- JSON list of behavioral constraints
  inferred_at       TIMESTAMP NOT NULL,
  UNIQUE(resource_type, resource_name, content_hash)
);
```

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
| `GET` | `/resources/:type/:name/versions/:version/download` | Download resource files as JSON response (used by `relava install`) |
| `GET` | `/resources/:type/:name/versions/:version/checksums` | Get file paths and SHA-256 checksums for a version |
| `POST` | `/resources/:type/:name` | Create a resource entry |
| `POST` | `/resources/:type/:name/publish` | Publish a resource version (multipart upload of directory contents) |
| `DELETE` | `/resources/:type/:name` | Remove resource from registry |

#### Resolution & Updates

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/resolve/:type/:name` | Resolve full dependency tree. Query: `?version=1.2.0`. Returns topologically sorted install order as JSON. |
| `POST` | `/updates/check` | Batch check for available updates. Accepts list of installed resources, returns available newer versions. |

#### Server

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Server health check (status, version, uptime, database connectivity) |
| `GET` | `/stats` | Server statistics (resource counts by type, version count, database size) |
| `GET` | `/config` | Server configuration (host, port, data directory, cache directory, cache size) |
| `POST` | `/cache/clean` | Clean server-side cache, returns bytes freed |

#### Hook Events (Layer 2)

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/hooks/event` | Receive a hook event from Claude Code (RelavaEvent). Processed asynchronously — daemon returns 200 immediately. |

#### Scoring (Layer 2)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/scores/:type/:name` | Get score history for a resource. Query: `?version=1.2.0&limit=50` |
| `GET` | `/scores` | Aggregate scores across all resources in a project. Query: `?project_dir=/path` |
| `GET` | `/compare/:type/:name` | Compare two versions. Query: `?v1=1.2.0&v2=1.3.0` |

#### GUI

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Serve the GUI web application (SPA with fallback to `index.html`) |

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

Initialize current directory as a Relava-managed project. Sets up resource management and scoring infrastructure.

```bash
$ cd ~/projects/my-app
$ relava init
Created relava.toml
Starting daemon... ok (http://localhost:7420)
Installing hooks... ok (6 hooks in .claude/settings.json)
Ready. Your next Claude Code session will be scored by Relava.
```

What it does:
- Creates an empty `relava.toml` in project root
- Starts the daemon if not already running (`relava daemon start`)
- Installs Claude Code hooks (`relava hooks install`) — configures `SubagentStart`, `SubagentStop`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, and `Stop` hooks to POST to daemon

#### `relava install <resource-type> <resource-name> [options]`

Install a resource into the current project.

**Options:**

| Flag | Description |
|------|-------------|
| `--version <ver>` | Install a specific version (default: latest) |
| `--save` | Write resource and version to `relava.toml` |
| `--global` | Install to `~/.claude/` instead of project |
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
3. Downloads resource files from the registry server (cached in `~/.relava/cache/`)
4. Writes files to the correct Claude Code locations in the project
5. Runs tool installation checks (prompts user, skipped with `--yes`)
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

#### `relava publish <resource-type> <resource-name> [--path PATH] [--force] [-y/--yes]`

Publish a resource to the local Relava registry server.

| Flag | Description |
|------|-------------|
| `--path <PATH>` | Custom source directory (default: standard location for resource type) |
| `--force` | Skip change detection and publish regardless |
| `-y, --yes` | Auto-confirm publish prompt (non-interactive) |

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

#### `relava daemon start [--port PORT]`

Start the Relava daemon (background process). The daemon serves both the registry REST API and hook event endpoints.

```bash
$ relava daemon start
Relava daemon started on http://localhost:7420
GUI available at http://localhost:7420

$ relava daemon stop
Relava daemon stopped.

$ relava daemon status
Relava daemon is running on http://localhost:7420
  Resources: 12 published, 6 installed in current project
  Active sessions: 1
```

**Note:** `relava server start/stop/status` are retained as aliases for backward compatibility.

#### `relava hooks install`

Configure Claude Code hooks to POST events to the Relava daemon. Writes hook entries to `.claude/settings.json`.

```bash
$ relava hooks install
Installing hooks in .claude/settings.json...
  [hook] SubagentStart  → POST http://localhost:7420/api/v1/hooks/event (async)
  [hook] SubagentStop   → POST http://localhost:7420/api/v1/hooks/event (async)
  [hook] PreToolUse     → POST http://localhost:7420/api/v1/hooks/event (async)
  [hook] PostToolUse    → POST http://localhost:7420/api/v1/hooks/event (async)
  [hook] Stop           → POST http://localhost:7420/api/v1/hooks/event (async)
Hooks installed. Claude Code sessions will now be scored by Relava.
```

All hooks use `async: true` — they observe but never block Claude Code execution.

#### `relava scores`

Show aggregate compliance scores across all resources in the current project.

```bash
$ relava scores
Project: /Users/woong/projects/my-app

Resource                   Version  Runs  Tool   Skill  Deleg  Recovery
skill/code-review          1.2.0    47    0.94   —      —      0.71
agent/orchestrator         1.0.0    31    0.97   0.95   0.98   0.82
agent/coder                0.5.0    31    0.91   1.00   1.00   0.65
```

#### `relava info <type> <name> --scores`

Show score history for a specific resource version.

```bash
$ relava info skill code-review --scores
Name:        code-review
Type:        skill
Version:     1.2.0 (latest)
Runs:        47

Scores (n=47):
  tool_compliance:    0.94 ± 0.08
  error_recovery:     0.71 ± 0.15

Recent Violations:
  2026-03-28  tool: Used Edit (not in declared tools [Read, Grep])
  2026-03-25  tool: Used Write (not in declared tools [Read, Grep])
```

#### `relava compare <type> <name> <version-a> <version-b>`

Compare two versions of a resource — content diff plus score comparison.

```bash
$ relava compare skill code-review 1.2.0 1.3.0
Content Diff:
  SKILL.md: +12 -3 lines

Agent Score Correlation:
┌─ agent/code-reviewer (47 runs v1.2.0, 31 runs v1.3.0) ──────────────┐
│  Metric              v1.2.0 (n=47)      v1.3.0 (n=31)    Δ          │
│  tool_compliance     0.94 ± 0.08        0.97 ± 0.04      +0.03 ↑    │
│  error_recovery      0.71 ± 0.15        0.82 ± 0.11      +0.11 ↑    │
└──────────────────────────────────────────────────────────────────────┘

Implication Summary:
  v1.2.0: 6 implications across 69 runs (8.7% implication rate)
  v1.3.0: 1 implication across 39 runs (2.6% implication rate)
```

**Skill comparison:** Skills are instructions, not actors — they have no direct scores. Skill quality is derived from the agents that use them. `relava compare skill` shows per-agent stratified score correlations. Scores are NEVER blended across agents to prevent Simpson's paradox.

**Agent comparison:** Shows the agent's own scores plus confounder detection — if a dependency also changed versions during the observation window, a warning is displayed.

**Minimum data requirements:** 5+ runs per version to show data. 20+ runs recommended.

#### `relava auto-update [--dry-run] [--status]`

Check all installed resources and update any with better-scoring versions available.

```bash
$ relava auto-update --status
Resource                   Current  Available  Score Δ      Status
skill/code-review          1.2.0    1.3.0      +0.03 ↑      Update available
agent/orchestrator         1.0.0    —          —            Up to date

$ relava auto-update
Checking 4 resources...
  skill/code-review: 1.2.0 → 1.3.0 (better scores, n=31)
Updated 1 resource.

$ relava auto-update --dry-run
Would update:
  skill/code-review: 1.2.0 → 1.3.0 (tool_compliance: +0.03, error_recovery: +0.11)
```

**Safeguards:**
- Only updates to published registry versions
- Requires minimum sample size (default: 5 runs)
- `require_no_regression = true` means the new version must score equal or better on *every* deterministic metric
- Updates are logged with before/after versions and scores
- `relava.lock` is updated to reflect new versions
- Pin a version in `relava.toml` (e.g., `code-review = "1.2.0"`) to opt out of auto-update

#### `relava frontmatter show|edit <type> <name>`

View or edit the inferred frontmatter contract for a resource.

```bash
$ relava frontmatter show skill code-review
Inferred frontmatter for skill/code-review@1.2.0:
  Purpose:   Comprehensive code review with security and style checks
  Tools:     [Read, Grep]
  Constraints:
    - Do not modify source files
    - Focus on security vulnerabilities and style issues

$ relava frontmatter edit skill code-review
# Opens editor with inferred frontmatter for manual adjustment
```

#### `relava server start [--port PORT] [--daemon]`

Retained as alias for `relava daemon start` for backward compatibility.

```bash
$ relava server start --daemon
Relava server started on http://localhost:7420
GUI available at http://localhost:7420

$ relava server stop
Relava server stopped.

$ relava server status
Relava server is running on http://localhost:7420
  Resources: 12 published, 6 installed in current project
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

#### `relava disable <resource-type> <resource-name>`

Temporarily disable a resource without removing it from the project. Moves the resource to a `.disabled/` subdirectory so Claude Code no longer discovers it.

```bash
$ relava disable skill denden
Disabled skill denden (moved to .claude/skills/.disabled/denden/)
```

#### `relava enable <resource-type> <resource-name>`

Re-enable a previously disabled resource. Restores it from `.disabled/` to its active location.

```bash
$ relava enable skill denden
Enabled skill denden (restored to .claude/skills/denden/)
```

#### `relava cache status`

Show download cache disk usage and entry count.

```bash
$ relava cache status
Cache: ~/.relava/cache/
  Size: 12.4 MB
  Entries: 23
```

Supports `--json` output. Automatic LRU eviction runs when cache exceeds the configured limit (default 500 MB).

#### `relava cache clean [--older-than DURATION]`

Remove cached downloads. Without flags, removes all cached entries. Optional `--older-than` flag with duration format (e.g., `7d`, `24h`, `30m`).

```bash
$ relava cache clean
Cleaned 23 entries (12.4 MB freed)

$ relava cache clean --older-than 7d
Cleaned 8 entries (4.1 MB freed)
```

Supports `--json` output.

### Startup Self-Update Check

Before dispatching any command, the CLI performs an automatic self-update check:

1. Checks the GitHub Releases API for newer CLI/server versions (throttled to once per 24 hours via `~/.relava/last_self_update_check`)
2. If a newer version is available and stdout is a TTY, prompts the user with `[Y/n]`
3. If the user accepts, downloads the new binaries, verifies SHA-256 checksums, and atomically replaces both `relava` and `relava-server` binaries
4. In non-interactive environments (non-TTY, CI): prints a notice to stderr, does not block
5. Suppressed by `--json`, `--no-update-check` flags

**Startup order:**
1. Self-update check (blocking interactive prompt)
2. Resource update check (non-blocking notification)
3. Command dispatch

### Automatic Resource Update Check

After read-only commands (`list`, `info`, `search`), the CLI checks for available resource updates:

1. Sends a batch POST to the server with all installed resources and their versions (throttled to once per hour via `~/.relava/last_update_check`)
2. If updates are available, prints a notification to stderr
3. Suppressed by `--json`, `--no-update-check` flags

---

## 6. GUI Design

### Tech Stack

- **Framework**: React (Vite build)
- **Styling**: Tailwind CSS
- **Bundling**: Built into a static SPA, served by the Relava server
- **State**: React Query for API calls, minimal client state

### Pages

#### Dashboard (`/`)
- Statistics overview: total resource count, version count, database size
- Resource type cards showing counts per type (skill, agent, command, rule)
- Update banner showing count of recently published resources (last 24 hours)
- Recently updated resources list (top 10, sorted by update time)
- Empty state with `relava publish` command hint

#### Resource Browser (`/browse`)
- Search input with 300ms debounce
- Type filter dropdown (all types or specific: skill, agent, command, rule)
- Sort dropdown (by name, recently updated, or version)
- Responsive resource card grid (1 column mobile, 2 columns desktop)
- Color-coded type badges: blue (skill), purple (agent), green (command), amber (rule)
- Description with 2-line clamp, version, and updated date

#### Resource Detail (`/browse/:type/:name`)
- Header with resource name, type badge, version, description, and last updated date
- Description rendered as GitHub-flavored markdown (via react-markdown)
- Files section showing file paths with SHA-256 checksums
- Dependencies section with clickable links to dependency detail pages
- Version history with checksums and publication dates

#### Settings (`/settings`)
- Server status section: status badge, version, uptime, database connection indicator
- Configuration section: host, port, data directory
- Storage section: database size, cache size, Clean Cache button with feedback

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

## 10. Layer 2: Runtime Scoring

Relava does not replace Claude Code's execution loop. It **observes** it via hooks and scores each skill and agent run using a three-tier evaluation model:

- **Deterministic (free, always on):** tool compliance, skill compliance, delegation compliance, error recovery — checked against frontmatter declarations
- **LLM-as-judge (opt-in, costs tokens):** purpose alignment, instruction alignment, delegation quality — semantic evaluation after session completes
- **Human review (async):** ambiguous cases and high-risk mutation promotions

Each resource in a session is scored independently. When an orchestrator delegates to a coder which invokes a skill, the session produces one `RunRecord` per resource, linked by `parent_id` into a call tree. This means when an agent's scores drop, you can see whether the problem is the agent itself or a dependency.

### 10.1 Frontmatter Inference

Users should never need to write frontmatter manually. On the first hook event that observes a resource without inferred frontmatter, Relava queues it for LLM-based inference. After the session completes, the daemon auto-generates:

- **Purpose statement** — what the resource is designed to do
- **Expected tools** — which tools the resource should use
- **Constraints** — behavioral boundaries inferred from the resource content

**Timing:** Inference runs on first hook observation only. Install and publish work fully offline — no LLM required. Resources without inferred frontmatter produce N/A compliance scores (not 0.0) until inference completes.

**How inference is triggered:** The daemon detects a resource without inferred frontmatter during hook processing. It queues the resource for inference. After the session completes (on `Stop` event), the daemon calls the LLM API directly using the user's configured API key (`RELAVA_API_KEY` env var or `api_key` in `~/.relava/config.toml`) to generate frontmatter from the resource content. No injection into the active Claude session occurs; the daemon makes its own API calls post-session.

**Re-inference:** When a resource's content changes (new version installed, content hash changes on local edit), frontmatter is re-inferred on the next hook event.

**Conservative by default:** Inference declares the **minimum** tool set, constraints, and dependencies — not the maximum. Better to produce false positives (user corrects via `relava frontmatter edit`) than to miss real violations.

**Publish behavior:** When a user runs `relava publish`, the inferred frontmatter is embedded into the published resource as explicit frontmatter. This makes the scoring contract portable — all users who install this version score against the same contract. `relava publish --dry-run` shows the embedded frontmatter.

### 10.2 Deterministic Scores (Always Computed, Free)

These are checkable by comparing hook events against frontmatter declarations (either author-written or auto-inferred). No judgment required.

**Tool compliance (0.0–1.0)** — did the resource only use tools declared in its `tools` frontmatter field? Each undeclared tool use reduces the score.

**Skill compliance (0.0–1.0, agents only)** — did the agent only activate skills declared in `metadata.relava.skills`?

**Delegation compliance (0.0–1.0, agents only)** — did the agent only delegate to sub-agents declared in `metadata.relava.agents`?

**Error recovery rate (0.0–1.0, null if no errors)** — when a tool call fails (signaled by a `PostToolUseFailure` event), does the next action address the error? Error followed by corrective action = recovery. Error followed by same failing action or session abandonment = failure.

| Resource type | Tool compliance | Skill compliance | Delegation compliance | Error recovery |
|--------------|----------------|-----------------|----------------------|----------------|
| Skill | Yes | No | No | Yes |
| Agent | Yes | Yes | Yes | Yes |

### 10.3 LLM-Evaluated Scores (Opt-In, Costs Tokens)

Enabled per project: `[scoring] llm_eval = true` in `relava.toml`.

**Purpose alignment (0.0–1.0)** — did the resource's actions align with its declared `description`?

**Instruction alignment (0.0–1.0)** — did the resource follow the intent of its constraints, not just the letter?

**Delegation quality (0.0–1.0, agents only)** — was the right sub-agent chosen for the task? Were the instructions appropriate?

LLM evaluation runs after the session completes. The scoring engine sends specific actions to an LLM with a structured prompt including the resource's frontmatter and the relevant trajectory segment. LLM scores are stored separately from deterministic scores with lower confidence weight.

### 10.4 RunRecord Schema

See §4 Layer 2 Database Tables for the full `run_records` table schema.

**One RunRecord per resource per session.** When an orchestrator delegates to a coder which invokes a skill, the session produces three RunRecords:

```
Session: abc-123
  RunRecord: agent/orchestrator@1.0.0  (parent: null,         depth: 0)
  RunRecord: agent/coder@0.5.0        (parent: orchestrator,  depth: 1)
  RunRecord: skill/code-review@1.2.0  (parent: coder,         depth: 2)
```

Each resource is scored against its own frontmatter. The call tree is reconstructable from `parent_id` links at any depth.

**Violation schema:** See §4 Layer 2 Database Tables for the full `violations` table. Violations are the **actionable context** — aggregate scores tell you *that* a version is worse, violations tell you *what* went wrong, *how*, and *where* in the trajectory.

**Manifest hash:** Each RunRecord captures a `manifest_hash` — a content hash of a manifest snapshot stored in `~/.relava/manifests/<hash>.json`. This records which resource versions were installed during each run, enabling confounder detection during version comparison.

### 10.5 Skill Quality Derivation

Skills are injected instructions with no discrete lifecycle — they cannot be scored directly. Skill quality is derived from the agents that use them.

**Agent evaluation is primary.** Score agents on compliance and output quality. Agent scores are the ground truth.

**When agent scores drop, diagnose root cause.** The LLM judge receives the agent's definition, all active skill definitions, and the full trajectory, then produces `ImplicationRecord`s (see §4 Layer 2 Database Tables).

**ImplicationRecord fields:**
- `implication_type`: `SKILL` | `AGENT` | `MIXED` | `ENVIRONMENT`
- `severity`: `primary` | `contributing`
- `category`: `instruction_gap` | `missing_edge_case` | `conflicting_guidance` | `over_permissive` | `under_specified` | `ambiguous_instruction` | `stale_reference`
- `summary`: LLM judge's explanation
- `confidence`: 0.0–1.0

`relava compare skill` shows `SKILL` and `MIXED` implications. `relava compare agent` shows `AGENT` and `MIXED` implications. This prevents double-counting while ensuring `MIXED` cases are visible in both views.

**Correlation analysis (cheap screening):** Compare agent scores when skill X is installed at version A vs version B. Uses data already being collected via `manifest_hash`. When correlation suggests a skill version change affected scores, the LLM judge provides the causal diagnosis.

### 10.6 Call Tree Tracking

Claude Code provides native **`SubagentStart`** and **`SubagentStop`** hook events that give a clean push/pop lifecycle for agent attribution.

When a `SubagentStart` event fires, the new agent is pushed onto the call stack. When `SubagentStop` fires, it's popped. Every hook event between a start/stop pair is attributed to that agent.

```json
{"ts": "...", "stack": ["orchestrator"], "event": "SubagentStart", "agent": "coder"}
{"ts": "...", "stack": ["orchestrator", "coder"], "event": "PreToolUse", "tool": "Edit"}
{"ts": "...", "stack": ["orchestrator", "coder"], "event": "PostToolUse", "tool": "Edit"}
{"ts": "...", "stack": ["orchestrator", "coder"], "event": "SubagentStart", "agent": "test-runner"}
{"ts": "...", "stack": ["orchestrator", "coder", "test-runner"], "event": "PreToolUse", "tool": "Bash"}
{"ts": "...", "stack": ["orchestrator", "coder", "test-runner"], "event": "PostToolUseFailure", "tool": "Bash"}
{"ts": "...", "stack": ["orchestrator", "coder", "test-runner"], "event": "SubagentStop", "agent": "test-runner"}
{"ts": "...", "stack": ["orchestrator", "coder"], "event": "SubagentStop", "agent": "coder"}
{"ts": "...", "stack": ["orchestrator"], "event": "Stop", "completion": "completed"}
```

The call stack enables:
- Attributing each action to the correct resource for scoring
- Building the `parent_id` tree in `RunRecord`s
- Detecting compliance violations at any depth
- Tracking error recovery within each resource's scope

### 10.7 Trajectory Storage

One trajectory file per session (not per resource). All resources in the session share the same trajectory. Stored at `~/.relava/trajectories/<session_id>.jsonl`. Referenced by `trajectory_id` (= `session_id`) from each `RunRecord`. Old trajectories can be cleaned up via `relava cache clean` — scores persist independently. Trajectories are stored locally and never sent externally.

### 10.8 Hook Infrastructure

Hooks observe Claude Code's execution. They do not replace it.

**Hook handler type:** All hooks use the **HTTP handler type** (not command type). Each hook event is POSTed to the Relava daemon's HTTP endpoint.

**Async execution:** All observation hooks use **`async: true`** configuration. Hooks observe but never block Claude Code execution. Claude Code does not wait for Relava's response before continuing.

**Hook configuration** (installed by `relava init` or `relava hooks install`):

| Hook Event | What it captures |
|------------|-----------------|
| `SubagentStart` | Agent name, agent type. Pushes agent onto call stack. Loads agent's frontmatter for compliance checking. |
| `SubagentStop` | Agent completion status. Pops agent from call stack. Finalizes that agent's RunRecord. |
| `PreToolUse` | Tool name, inputs. Checked against current stack-top resource's declared tools/skills/agents. |
| `PostToolUse` | Tool result (success), file paths. Used for trajectory recording and compliance tracking. |
| `PostToolUseFailure` | Tool failure: error message, exit codes. Distinct from PostToolUse — provides a clean signal for error detection and recovery tracking. |
| `Stop` | Session end. Finalize all remaining RunRecords, compute scores, close trajectory. |

**Processing flow:**

1. On session start (first hook event received), daemon initializes call stack and trajectory log
2. On `SubagentStart`: push agent onto stack, load its frontmatter, begin tracking its actions
3. On each `PreToolUse`: append to trajectory with current stack. Check tool against current stack-top resource's declared tools
4. On each `PostToolUse`: append to trajectory. Record successful tool completion
5. On each `PostToolUseFailure`: append to trajectory. Flag error for recovery tracking
6. On `SubagentStop`: pop agent from stack. Compute that agent's deterministic scores
7. On `Stop`: finalize any remaining RunRecords. Optionally trigger LLM evaluation. Write `RunRecord` per resource

**What hooks do NOT do:**
- They do not block tool calls (async observation only, not enforcement)
- They do not modify agent behavior during a run
- They do not send data anywhere without explicit opt-in

### 10.9 Vendor-Neutral Event Schema (RelavaEvent)

All hook events from any supported platform are normalized into a canonical **RelavaEvent** schema before any processing occurs. The scoring engine, trajectory storage, ImplicationRecord analysis, and all downstream systems only see RelavaEvent — they are platform-agnostic.

**The vendor adapter pattern:**
```
Claude Code hooks  →  Claude Code Adapter  →  RelavaEvent
Codex hooks        →  Codex Adapter        →  RelavaEvent  (Phase D)
Gemini CLI hooks   →  Gemini Adapter       →  RelavaEvent  (Phase D)
                                                  │
                                                  v
                                     Scoring Engine / Trajectory Storage
                                     (platform-agnostic — sees only RelavaEvent)
```

**RelavaEvent schema:**

```
RelavaEvent
  event_type          TEXT        -- SUBAGENT_START | SUBAGENT_STOP
                                  -- | PRE_TOOL_USE | POST_TOOL_USE | POST_TOOL_USE_FAILURE
                                  -- | SESSION_STOP
  timestamp           TIMESTAMP
  session_id          TEXT
  agent_platform      TEXT        -- "claude" | "codex" | "gemini"
  tool_name           TEXT        -- normalized tool name (null for agent lifecycle events)
  tool_inputs         MAP         -- sanitized tool inputs (no secrets, no file contents)
  tool_result_summary TEXT        -- brief result summary (not raw output)
  agent_name          TEXT        -- current agent (from call stack top)
  call_stack          LIST<TEXT>  -- current agent call stack
  error               BOOL       -- whether this event represents a failure
  extras              MAP         -- platform-specific metadata
```

**Phase B ships with the Claude Code adapter only.** The adapter maps Claude Code hook events to canonical RelavaEvent types:
- `SubagentStart` → `SUBAGENT_START`
- `SubagentStop` → `SUBAGENT_STOP`
- `PreToolUse` → `PRE_TOOL_USE`
- `PostToolUse` → `POST_TOOL_USE`
- `PostToolUseFailure` → `POST_TOOL_USE_FAILURE`
- `Stop` → `SESSION_STOP`

**Design constraint:** No downstream system (scoring engine, trajectory writer, RunRecord builder, ImplicationRecord analyzer) may reference platform-specific event types or payload fields. All platform knowledge is encapsulated in adapters.

### 10.10 Score-Driven Auto-Update

When a newer version of a resource is available in the registry and has better scores than the currently installed version, Relava can automatically update it.

**How it works:**
1. After each scoring session, check: are there newer versions of the scored resource in the registry?
2. If yes, compare the current version's aggregate scores against the newer version's scores
3. If the newer version scores equal or better on all deterministic scores with sufficient sample size (default: 5+ runs), auto-update

**Relationship to version pinning:** Users can pin a version in `relava.toml` to opt out of auto-update for specific resources. `relava.lock` is updated to reflect new versions after auto-update.

---

## 11. Implementation Plan

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

- Resource templates (`relava create skill`, `relava create agent`)
- Project scaffolding (`relava new project`)
- CLAUDE.md auto-management (adding/removing skill references)
- Version conflict resolution

### Phase B: Runtime Scoring (4 weeks)

**Prerequisite:** Phases 1–3 complete (resource management, registry server, GUI).

**Cold start:** Relava must ship with a curated set of 10-20 default skills covering common Claude Code workflows (code review, commit messages, PR descriptions, debugging, test writing, refactoring, documentation, etc.). These are seeded in the registry with pre-authored frontmatter and serve as the initial score surface.

**Day-1 experience:** `relava init` installs hooks + a starter skill. The user's next Claude Code session is scored, and at session end Relava prints a one-line summary: `skill/code-review — tool compliance: 1.0, error recovery: 0.8 (2 errors, 1 recovered)`.

| Week | Deliverable |
|------|-------------|
| 1 | Hook infrastructure — `relava hooks install` configures Claude Code hooks (HTTP handler type, async:true). Daemon architecture (`relava daemon start/stop`). Call stack tracking via `SubagentStart`/`SubagentStop` events. `PreToolUse`/`PostToolUse`/`PostToolUseFailure` events normalized to RelavaEvent and written to `~/.relava/trajectories/`. Claude Code adapter (RelavaEvent normalization). Frontmatter loading for declared tools/skills/agents/constraints. |
| 2 | Deterministic scoring engine — tool compliance, skill compliance (agents), delegation compliance (agents), error recovery rate. `RunRecord` schema with `session_id`, `parent_id`, `call_depth`. One `RunRecord` per resource per session. Local storage for scores and trajectories. |
| 3 | Score CLI + auto-update — `relava info <type> <name> --scores` (score history per version), `relava scores` (aggregate across project), `relava compare <type> <name> <v1> <v2>` (content diff + score comparison). `relava auto-update` (score-driven version selection with `--dry-run` and `--status`). Local change tracking by content hash. Score migration on `relava publish`. Inferred frontmatter embedded on publish. |
| 4 | LLM infrastructure — frontmatter inference engine (auto-generation of purpose, expected tools, constraints from resource content, triggered post-session via daemon's own API calls) + LLM evaluation (opt-in) — purpose alignment, instruction alignment, delegation quality scorers. Post-session async evaluation. `[scoring] llm_eval = true` config. LLM scores stored separately with lower confidence weight. |

**Milestone:** User installs a skill or agent, Relava auto-infers its frontmatter contract on first use, hooks produce compliance scores on every run, resources auto-update to better-scoring versions, and the scoring data is accurate enough to feed failure clustering.

**Phase B Gate Criteria (must pass before Phase C):**
1. **Frontmatter inference accuracy:** >85% agreement with human-reviewed frontmatter on N=50 resources
2. **Deterministic score false-positive rate:** <10% of violations are actually legitimate behavior
3. **Violation log usefulness:** >70% of violations contain enough context for a human to understand what went wrong
4. **Trajectory completeness:** call stack correctly attributes >95% of actions to the right resource

---

## 12. Tech Stack Recommendations

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

## 13. Open Questions

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

## 14. Implementation Order

Trackable checklist of every deliverable from the Implementation Plan (Section 11). Items are numbered sequentially across all phases. Status key: ⬜ Not Started · 🟡 In Progress · ✅ Complete.

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
- ✅ 21. `relava install relava.toml` — bulk install from project manifest via `bulk_install` module, installs all declared resources with lockfile tracking — *depends on 3, 9, 13*
- ✅ 22. `relava import <type> <path>` — scan existing resource directory/file, auto-derive name from path, validate structure, publish to registry with optional `--version` flag
- ✅ 22a. `relava resolve <type> <name>` — display full dependency tree (tree view + `--json` output), optional `--version` flag, does not install — *depends on 12b*
- ✅ 22b. `relava validate <type> <path>` — offline pre-publish validation (slug, structure, frontmatter, file limits, file type filtering, semver, `.relavaignore` support) — *depends on 4, 4a, 5*
- ✅ 22c. File type filtering — binary detection (null-byte check in first 8KB), enforce text-only for skills/commands/rules, any files for agents, `.relavaignore` pattern support — *depends on 4*
- ✅ 22d. Rich console output — `comfy-table` for tables (list/search/info), `colored` for status tags, `--json` mode for structured output via `output` module
- ✅ 23. Disable/enable mechanism — moves resources to `.disabled/` subdirectory (e.g., `.claude/skills/.disabled/denden/`), detects conflicts, cleans up empty directories
- ✅ 24. End-to-end integration testing — full lifecycle tests in `lifecycle_tests.rs` covering publish, install, list, update, remove cycle

**Phase 1 Milestone**: ✅ Complete. Developer can publish a resource to local store, install it into a project, list installed resources, remove, and update — all via CLI.

---

### Phase 2: Local Registry Server + REST API

#### Week 4 — Server Foundation

- ✅ 25. HTTP server scaffolding — Axum + tokio async runtime, server startup/shutdown lifecycle, Mutex-protected shared state
- ✅ 26. `relava server start` / `stop` / `status` commands — daemon mode with PID file (`~/.relava/server.pid`), log redirection (`~/.relava/server.log`), port binding, `--gui-dir` option, platform-aware process management (Unix SIGTERM / Windows taskkill)
- ✅ 27. Resources REST endpoints — `GET /resources`, `GET /resources/:type/:name`, `POST /resources/:type/:name`, `DELETE /resources/:type/:name` — *depends on 25*
- ✅ 28. Resource versions REST endpoints — `GET /resources/:type/:name/versions`, `GET /resources/:type/:name/versions/:version`, `GET /resources/:type/:name/versions/:version/checksums` — *depends on 27*
- ✅ 30. CLI refactor — all operations go through REST API via `api_client` module, fail with clear error if server is unreachable — *depends on 27*

#### Week 5 — Server Features & Publish

- ✅ 31a. Resolution endpoint — `GET /api/v1/resolve/:type/:name?version=<ver>`, server-side recursive resolution with cycle detection, returns topologically sorted install order as JSON — *depends on 27*
- ✅ 31b. CLI integration for server-side resolve — `resolver` module uses the resolve endpoint for dependency resolution — *depends on 30, 31a*
- ✅ 32. Search endpoint with SQLite FTS5 — `GET /resources?q=search&type=skill`, full-text indexing of name + description
- ✅ 33. `relava search <query>` CLI command — search resources via server API with optional `--type` filter, truncates descriptions to 60 chars — *depends on 32*
- ✅ 34. Health and stats endpoints — `GET /health` (status, version, uptime, database connectivity), `GET /stats` (resource counts by type, version count, database size), `GET /config` (server configuration), `POST /cache/clean`
- ✅ 35. `relava publish <type> <name>` — read manifest, validate slug + fields + file limits + file type filtering (100 files / 10MB each / 50MB total), compute SHA-256 per file, base64-encoded JSON POST to server, `--force` and `--yes` flags — *depends on 27, 4a, 22c*
- ✅ 35a. Server-side publish validation — parse JSON payload, validate slug format, semver, version monotonicity, file limits, store in `~/.relava/store/` via blob store — *depends on 27*
- ✅ 35b. Download endpoint — `GET /resources/:type/:name/versions/:version/download` serves resource files as JSON for CLI install — *depends on 27*
- ✅ 35c. Version auto-increment — on publish without explicit version, auto-increment patch from latest published version — *depends on 35a*
- ✅ 36. `relava publish <type> <name> --path PATH` — publish from custom source directory — *depends on 35*
- ✅ 36a. `.relavaignore` support — exclude file patterns from publish/sync, gitignore-style syntax via `file_filter` module in `relava-types` — *depends on 35*
- ✅ 36b. Publish change detection — compare local resource directory against registry version using SHA-256 checksums, skip publish if no changes, show diff summary and prompt for confirmation, bypass with `--force` — *depends on 35*
- ✅ 37. Static file serving — server serves SPA files from GUI directory with fallback to `index.html`, configurable via `--gui-dir` — *depends on 25*

**Phase 2 Milestone**: ✅ Complete. CLI works against the running server. All operations available via REST API. Resources are published and installed through the server.

---

### Phase 3: GUI

#### Week 6 — App Shell & Dashboard

- ✅ 38. React app scaffolding — Vite 8 + Tailwind CSS 4 + TanStack React Query 5, React Router DOM 7, TypeScript 5.9, API client with typed endpoints
- ✅ 39. App shell — navigation header (Dashboard, Browse, Settings), responsive Layout component with mobile hamburger menu, React Router with BrowserRouter
- ✅ 40. Dashboard page — statistics overview (resource count, version count, database size), resource type cards with counts, update banner for recently published resources, recently updated list (top 10)

#### Week 7 — Resource Browser & Details

- ✅ 43. Resource browser page — search input with 300ms debounce, type filter dropdown, sort dropdown (name/updated/version), responsive card grid, color-coded type badges (blue/purple/green/amber) — *depends on 38*
- ✅ 44. Resource detail page — description rendered as GitHub-flavored markdown (react-markdown + remark-gfm), version history with checksums, file list with SHA-256 hashes, dependency tree with clickable links

#### Week 8 — Settings & Polish

- ✅ 47. Settings page — server status (badge, version, uptime, database connection), configuration (host, port, data directory), storage (database size, cache size, Clean Cache button with feedback)
- ✅ 50. GUI build pipeline — Vite production build, served from `~/.relava/gui/` or configurable directory via `--gui-dir`, deploy script in package.json
- ✅ 51. Responsive design pass — mobile-first Tailwind CSS, responsive navigation with auto-closing hamburger menu, responsive card grids, consistent spacing and typography

**Phase 3 Milestone**: ✅ Complete. Developer can browse and search the registry through a web GUI at `localhost:7420`.

---

### Phase 4: Advanced Features (Weeks 9+)

No week assignments — each feature is an independent work item.

- ⬜ 54. Resource templates — `relava create skill <name>`, `relava create agent <name>` scaffolding with starter `.md` files and frontmatter
- ✅ 56. Auto-update notifications — CLI check (throttled once/hour, batch POST to server), GUI UpdateBanner component with amber badge. Suppressed by `--no-update-check` and `--json` flags
- ✅ 58. Self-update check at startup — blocking interactive prompt at program startup (throttled once/24h), checks GitHub Releases API, SHA-256 verified atomic binary replacement for both `relava` and `relava-server`. Suppressed by `--no-update-check`, `--json`, or non-TTY
- ✅ 59. Cache management — `relava cache clean [--older-than DURATION]` and `relava cache status` commands, LRU eviction policy, disk usage reporting with entry counts, duration parsing (e.g., `7d`, `24h`, `30m`)

---

### Phase B: Runtime Scoring

#### Week B1 — Hook Infrastructure & Call Tree

- ⬜ 65. Daemon lifecycle — `relava daemon start/stop/status` commands, background process management with PID file, alias `relava server` commands for backward compatibility — *depends on 26*
- ⬜ 66. Hook installation — `relava hooks install` writes hook entries to `.claude/settings.json` (SubagentStart, SubagentStop, PreToolUse, PostToolUse, PostToolUseFailure, Stop), all using HTTP handler type with `async: true`, POST to `localhost:7420/api/v1/hooks/event`
- ⬜ 67. Hook removal — `relava hooks remove` removes Relava hook entries from `.claude/settings.json`
- ⬜ 68. Hook event endpoint — `POST /api/v1/hooks/event` receives raw Claude Code hook events, returns 200 immediately (async processing)
- ⬜ 69. RelavaEvent schema — canonical event types (`SUBAGENT_START`, `SUBAGENT_STOP`, `PRE_TOOL_USE`, `POST_TOOL_USE`, `POST_TOOL_USE_FAILURE`, `SESSION_STOP`) in `relava-types` crate
- ⬜ 70. Claude Code adapter — normalize Claude Code hook payloads into RelavaEvent. All platform knowledge encapsulated here — no downstream system references Claude-specific types
- ⬜ 71. Call stack tracking — in-memory call stack maintained by daemon. Push on `SUBAGENT_START`, pop on `SUBAGENT_STOP`. Every event between start/stop attributed to stack-top agent
- ⬜ 72. Trajectory writer — write RelavaEvent stream to `~/.relava/trajectories/<session_id>.jsonl`, one JSON line per event with call stack annotation
- ⬜ 73. Frontmatter loading — on `SUBAGENT_START`, load agent's frontmatter from installed resources. Resolve declared tools/skills/agents/constraints for compliance checking
- ⬜ 74. `relava init` expansion — add daemon start and hooks install to `relava init` flow. Print scoring-ready confirmation message — *depends on 65, 66*

#### Week B2 — Deterministic Scoring Engine

- ⬜ 75. RunRecord schema — `run_records` table in SQLite (see §4 Layer 2 Database Tables). One RunRecord per resource per session, linked by `session_id` and `parent_id` into call tree
- ⬜ 76. Violation schema — `violations` table in SQLite. Structured record of every compliance failure with type, severity, context, declared vs actual, trajectory offset
- ⬜ 77. Tool compliance scorer — compare each `PRE_TOOL_USE` event's tool name against stack-top resource's declared tools. Score = (declared tool uses) / (total tool uses). Each undeclared use produces a Violation
- ⬜ 78. Skill compliance scorer (agents only) — check whether activated skills match declared `metadata.relava.skills`. Score = (declared skills) / (total skills activated)
- ⬜ 79. Delegation compliance scorer (agents only) — check whether delegated agents match declared `metadata.relava.agents`. Score = (declared agents) / (total agents delegated to)
- ⬜ 80. Error recovery scorer — on `POST_TOOL_USE_FAILURE`, check if next action by same resource addresses the error. Recovery = corrective action follows. Failure = same failing action repeated or session abandoned. Score = (recoveries) / (total errors)
- ⬜ 81. RunRecord finalization — on `SUBAGENT_STOP` or `SESSION_STOP`, compute deterministic scores for each resource from attributed trajectory events, write RunRecord to `~/.relava/scores/` and SQLite
- ⬜ 82. Manifest snapshot — on session start, capture installed resource versions as `~/.relava/manifests/<content_hash>.json`. Deduplicated across consecutive runs with identical versions. Store `manifest_hash` in RunRecord
- ⬜ 83. Daemon crash recovery — write `daemon.state` (session_id, last event timestamp) to disk periodically. On restart, detect incomplete sessions and mark RunRecords with `data_complete = false`

#### Week B3 — Score CLI & Auto-Update

- ⬜ 84. `relava scores` CLI — aggregate scores across all resources in project. Query daemon API, display formatted table with resource, version, run count, and score columns — *depends on 75*
- ⬜ 85. `relava info --scores` — extend existing `relava info` to show score history per version when `--scores` flag is set. Show recent violations — *depends on 75, 76*
- ⬜ 86. `relava compare` CLI — `relava compare <type> <name> <v1> <v2>`. Content diff (SKILL.md / agent .md diff between versions) + score comparison. For skills: per-agent stratified score correlations (auto-discover dependent agents from RunRecords). For agents: own scores + confounder detection (warn if dependencies also changed versions). Minimum data thresholds (5+ runs to show, 20+ recommended) — *depends on 75, 82*
- ⬜ 87. `relava auto-update` CLI — check all installed resources for better-scoring versions in registry. `--dry-run` shows what would change. `--status` shows available updates with score deltas. Respects `[auto_update]` config in `relava.toml` (enabled, min_runs, require_no_regression). Updates `relava.lock` — *depends on 75, 18*
- ⬜ 88. Local change tracking — track unpublished local edits under `~/.relava/scores/local/<type>/<name>/<content-hash>/`. Content hash computed from resource directory. On `relava publish`, migrate local scores to `~/.relava/scores/registry/` under new version
- ⬜ 89. Score migration on publish — when `relava publish` creates a new version, embed inferred frontmatter into published resource. Local scores become version-pinned registry scores

#### Week B4 — LLM Infrastructure

- ⬜ 90. Frontmatter inference engine — on first hook observation of a resource without frontmatter, queue for inference. After session completes (`SESSION_STOP`), daemon calls LLM API (using `RELAVA_API_KEY` or `~/.relava/config.toml` api_key) to generate purpose, expected tools, and constraints from resource content. Store in `~/.relava/frontmatter/` and `inferred_frontmatter` table. Conservative by default (minimum tool set). Re-infer on content hash change — *depends on 68, 73*
- ⬜ 91. `relava frontmatter show|edit` CLI — view inferred frontmatter for a resource. `edit` opens editor for manual adjustment. Explicit frontmatter in resource file takes precedence over inferred
- ⬜ 92. LLM evaluation engine (opt-in) — purpose alignment, instruction alignment, delegation quality scorers. Enabled by `[scoring] llm_eval = true` in `relava.toml`. Runs post-session. Sends resource frontmatter + trajectory segment to LLM with structured prompt, expects JSON response with scores and reasons. LLM scores stored separately with lower confidence weight
- ⬜ 93. ImplicationRecord analysis — when agent scores drop and LLM eval is enabled, LLM judge receives agent definition + active skill definitions + trajectory. Produces ImplicationRecords linking failure to specific skills/agents. Schema: implication_type, severity, category, summary, confidence. Stored in `implication_records` table — *depends on 92, 76*

---

### Phase 5: Reliability and Scalability

No week assignments — each item is an independent work item.

- ⬜ 60. Atomic bulk install — currently `bulk_install::run()` iterates resources sequentially and delegates each to `install::run()`, which calls `write_to_project()` using individual `std::fs::write()` calls. Failures are collected in `BulkInstallResult.failed` but previously installed resources are not rolled back. **Change:** add a staging phase that downloads and validates all resources into a temp directory first, then atomically moves them into `.claude/` via `std::fs::rename()`. On staging failure, no project files are modified. On move failure, write a rollback log (`relava.lock.rollback`) that `relava doctor --fix` can use to clean up partial state. — *depends on 9, 21*
- ⬜ 61. Connection pool for SqliteResourceStore — currently `AppState.store` is `Mutex<SqliteResourceStore>` wrapping a single `Connection` opened in `SqliteResourceStore::open()`. All route handlers acquire this lock (e.g., `state.store.lock()` in `lib.rs`), serializing every database operation. **Change:** replace the single `Mutex<SqliteResourceStore>` with a connection pool (e.g., `r2d2` or `deadpool-sqlite`). The `ResourceStore` trait (8 methods in `store/traits.rs`) stays unchanged; only `AppState` and `SqliteResourceStore` internals change. Read operations proceed concurrently under SQLite WAL mode; only writes need serialization. — *depends on 25*
- ⬜ 62. Automatic cache eviction — `cache_manage::evict()` and `EvictionOpts` struct already exist with LRU logic (sort by mtime, remove oldest until under `max_bytes`), and `DEFAULT_MAX_CACHE_BYTES` is set to 500 MB. However, `install::run()` and `update::run()` never call `evict()` after downloading to cache. **Change:** call `cache_manage::evict()` at the end of `install::run()` and `update::run()` when the cache exceeds the threshold. Expose the threshold via `~/.relava/config.toml` (`cache_max_bytes`). Report eviction counts in `--verbose` mode. — *depends on 9, 59*
- ⬜ 63. Atomic lockfile updates — `Lockfile::save()` uses `std::fs::write()` directly to write `relava.lock`, which is not atomic on all filesystems. The four callers (`update_after_install`, `update_after_remove`, `update_after_update`, `update_after_bulk_install`) in `lockfile.rs` all go through `save()`. **Change:** write to a temporary file (`relava.lock.tmp`) in the same directory first, then `std::fs::rename()` to `relava.lock` (atomic on POSIX). Add a `relava doctor` check that compares `relava.lock` entries against actual files in `.claude/` and reports drift with an option to reconcile (`--fix`). — *depends on 13, 20*
- ⬜ 64. Enforce minimum Rust version — the workspace sets `edition = "2024"` (stabilized in Rust 1.85) but declares no `rust-version`. CI (`ci.yml`) uses `dtolnay/rust-toolchain@stable` without pinning. **Change:** add `rust-version = "1.85"` to `[workspace.package]` in root `Cargo.toml`. Pin CI to `1.85` or a specific stable version. Add an MSRV check job in `ci.yml` that builds with the declared minimum version.

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
