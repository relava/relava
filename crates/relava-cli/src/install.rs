use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cache::DownloadCache;
use crate::env_check::{self, EnvResult, EnvStatus};
use crate::registry::RegistryClient;
use crate::resolver::{self, RegistryDepProvider};
use crate::tools::{self, ToolResult, ToolStatus};
use relava_types::manifest::ResourceMeta;
use relava_types::validate::{self, AgentType, ResourceType};
use relava_types::version::Version;

/// Options for the install command.
pub struct InstallOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub version_pin: Option<&'a str>,
    pub project_dir: &'a Path,
    pub global: bool,
    pub json: bool,
    pub verbose: bool,
    pub yes: bool,
}

/// Result of installing a transitive dependency.
#[derive(Debug, serde::Serialize)]
pub struct DepInstallResult {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub status: String,
}

/// Result of a successful install, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct InstallResult {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
    pub install_dir: String,
    /// Files that were overwritten during installation.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overwritten: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolResult>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvResult>,
    /// Transitive dependencies that were installed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepInstallResult>,
}

/// Result of writing files to the project directory.
struct WriteResult {
    /// The directory files were written to.
    install_dir: PathBuf,
    /// Files that already existed and were overwritten.
    overwritten: Vec<String>,
}

/// Run `relava install <type> <name>`.
///
/// Resolves transitive dependencies via DFS, installs them leaf-first,
/// then installs the root resource.
pub fn run(opts: &InstallOpts) -> Result<InstallResult, String> {
    // Validate the resource name
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    // Determine install root
    let install_root = if opts.global {
        dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())?
    } else {
        opts.project_dir.to_path_buf()
    };

    // Set up cache
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| "cannot determine home directory for cache".to_string())?
        .join(".relava")
        .join("cache");
    let cache = DownloadCache::new(cache_dir);

    // Connect to registry and resolve version
    let client = RegistryClient::new(opts.server_url);

    if opts.verbose {
        eprintln!("resolving {} {}...", opts.resource_type, opts.name);
    }

    let version = client
        .resolve_version(opts.resource_type, opts.name, opts.version_pin)
        .map_err(|e| e.to_string())?;

    // Resolve transitive dependencies
    let dep_results = resolve_and_install_deps(
        opts, &client, &cache, &install_root, &version,
    )?;

    if !opts.json {
        println!(
            "Installing {} {}@{}...",
            opts.resource_type, opts.name, version
        );
    }

    // Check cache first, download if needed
    let file_paths = download_to_cache(
        &client, &cache, opts.resource_type, opts.name, &version, opts.server_url, opts.verbose,
    )?;

    // Write files to the correct Claude Code location
    let WriteResult {
        install_dir,
        overwritten,
    } = write_to_project(
        &install_root,
        opts.resource_type,
        opts.name,
        &version,
        &cache,
    )
    .map_err(|e| e.to_string())?;

    let file_count = file_paths.len();
    let install_dir_display = install_dir.to_string_lossy().to_string();

    if !opts.json {
        // Emit overwrite warnings before the file summary
        for file in &overwritten {
            println!("  [warn]    Overwrote existing file: {file}");
        }

        let type_tag = format!("[{}]", opts.resource_type);
        let file_summary = if file_count == 1 {
            install_dir_display.clone()
        } else {
            format!(
                "{} + {} files",
                primary_file(opts.resource_type, opts.name),
                file_count - 1
            )
        };
        println!("  {type_tag:<10}{file_summary}");
    }

    // Post-install: tool checking and env var validation (skills only)
    let (tool_results, env_results) = if opts.resource_type == ResourceType::Skill {
        run_skill_post_install(opts, &install_dir)?
    } else {
        (Vec::new(), Vec::new())
    };

    if !opts.json {
        println!("Installed {} {}@{}", opts.resource_type, opts.name, version);
    }

    Ok(InstallResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        version: version.to_string(),
        files: file_paths,
        install_dir: install_dir_display,
        overwritten,
        tools: tool_results,
        env: env_results,
        dependencies: dep_results,
    })
}

/// Download a resource to cache if not already cached. Returns file paths.
fn download_to_cache(
    client: &RegistryClient,
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    version: &Version,
    server_url: &str,
    verbose: bool,
) -> Result<Vec<String>, String> {
    if cache.is_cached(resource_type, name, version) {
        if verbose {
            eprintln!("  using cached version");
        }
        cache
            .list_files(resource_type, name, version)
            .map_err(|e| e.to_string())
    } else {
        if verbose {
            eprintln!("  downloading from {server_url}");
        }
        let response = client
            .download(resource_type, name, version)
            .map_err(|e| e.to_string())?;
        cache
            .store(resource_type, name, version, &response)
            .map_err(|e| e.to_string())
    }
}

/// Resolve transitive dependencies and install them in leaf-first order.
///
/// Returns the list of installed/skipped dependency results.
/// The root resource is NOT installed here — only its transitive deps.
fn resolve_and_install_deps(
    opts: &InstallOpts,
    client: &RegistryClient,
    cache: &DownloadCache,
    install_root: &Path,
    _root_version: &Version,
) -> Result<Vec<DepInstallResult>, String> {
    // Build version pins from relava.toml if it exists
    let version_pins = load_version_pins(opts.project_dir, opts.resource_type);

    let provider = RegistryDepProvider::new(client, cache, install_root, version_pins);

    let resolve_result =
        resolver::resolve(&provider, opts.resource_type, opts.name)
            .map_err(|e| e.to_string())?;

    let deps_to_install = resolve_result.deps_to_install();
    if deps_to_install.is_empty() {
        return Ok(Vec::new());
    }

    if !opts.json {
        let count = deps_to_install.len();
        let plural = if count == 1 { "dependency" } else { "dependencies" };
        println!("Resolving {count} {plural}...");
    }

    let mut results = Vec::new();

    for dep in &resolve_result.install_order {
        // Skip the root resource (last in install_order)
        if dep.name == opts.name && dep.resource_type == opts.resource_type.to_string() {
            continue;
        }

        if dep.already_installed {
            if !opts.json {
                println!("  [skip]    {} {}@{} (already installed)", dep.resource_type, dep.name, dep.version);
            }
            results.push(DepInstallResult {
                resource_type: dep.resource_type.clone(),
                name: dep.name.clone(),
                version: dep.version.clone(),
                status: "skipped".to_string(),
            });
            continue;
        }

        let dep_rt = ResourceType::from_str(&dep.resource_type)
            .map_err(|e| e.to_string())?;
        let dep_version = Version::parse(&dep.version)
            .map_err(|e| format!("invalid version for dependency {}: {e}", dep.name))?;

        if !opts.json {
            println!("  [dep]     Installing {} {}@{}...", dep.resource_type, dep.name, dep.version);
        }

        // Download to cache if needed
        download_to_cache(client, cache, dep_rt, &dep.name, &dep_version, opts.server_url, opts.verbose)?;

        // Write to project
        write_to_project(install_root, dep_rt, &dep.name, &dep_version, cache)
            .map_err(|e| e.to_string())?;

        // Run post-install for skills
        if dep_rt == ResourceType::Skill {
            let dep_install_dir = install_root
                .join(AgentType::Claude.skills_dir())
                .join(&dep.name);
            let dep_opts = InstallOpts {
                server_url: opts.server_url,
                resource_type: dep_rt,
                name: &dep.name,
                version_pin: Some(&dep.version),
                project_dir: opts.project_dir,
                global: opts.global,
                json: opts.json,
                verbose: opts.verbose,
                yes: opts.yes,
            };
            let (tool_results, env_results) = run_skill_post_install(&dep_opts, &dep_install_dir)?;
            if !opts.json {
                for result in &tool_results {
                    print_tool_result(result);
                }
                for result in &env_results {
                    print_env_result(result);
                }
            }
        }

        results.push(DepInstallResult {
            resource_type: dep.resource_type.clone(),
            name: dep.name.clone(),
            version: dep.version.clone(),
            status: "installed".to_string(),
        });
    }

    Ok(results)
}

/// Load version pins from relava.toml for the given resource type.
pub fn load_version_pins(project_dir: &Path, resource_type: ResourceType) -> BTreeMap<String, String> {
    let toml_path = project_dir.join("relava.toml");
    if !toml_path.exists() {
        return BTreeMap::new();
    }
    match relava_types::manifest::ProjectManifest::from_file(&toml_path) {
        Ok(manifest) => match resource_type {
            ResourceType::Skill => manifest.skills,
            ResourceType::Agent => manifest.agents,
            ResourceType::Command => manifest.commands,
            ResourceType::Rule => manifest.rules,
        },
        Err(_) => BTreeMap::new(),
    }
}

/// Check if a resource is already installed in the project.
#[allow(dead_code)] // will be used by `relava remove` for dependency checking
pub fn is_installed(install_root: &Path, resource_type: ResourceType, name: &str) -> bool {
    let agent_type = AgentType::Claude;
    match resource_type {
        ResourceType::Skill => install_root
            .join(agent_type.skills_dir())
            .join(name)
            .join("SKILL.md")
            .exists(),
        ResourceType::Agent => install_root
            .join(agent_type.agents_dir())
            .join(format!("{name}.md"))
            .exists(),
        ResourceType::Command => install_root
            .join(agent_type.commands_dir())
            .join(format!("{name}.md"))
            .exists(),
        ResourceType::Rule => install_root
            .join(agent_type.rules_dir())
            .join(format!("{name}.md"))
            .exists(),
    }
}

/// Run skill-specific post-install steps: tool checking and env var validation.
///
/// Reads the installed SKILL.md frontmatter, checks tools, checks env vars.
/// Tool/env failures are non-fatal — warnings only.
fn run_skill_post_install(
    opts: &InstallOpts,
    install_dir: &Path,
) -> Result<(Vec<ToolResult>, Vec<EnvResult>), String> {
    if opts.resource_type != ResourceType::Skill {
        return Ok((Vec::new(), Vec::new()));
    }

    let skill_md = install_dir.join("SKILL.md");
    if !skill_md.exists() {
        return Ok((Vec::new(), Vec::new()));
    }

    let meta = match ResourceMeta::from_file(&skill_md) {
        Ok(m) => m,
        Err(e) => {
            if !opts.json {
                eprintln!("  [warn]    Could not parse skill metadata: {e}");
            }
            return Ok((Vec::new(), Vec::new()));
        }
    };

    // Check and install tools
    let prompt_fn: tools::PromptFn = Box::new(|msg| {
        eprint!("            {msg} ");
        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .map(|_| {
                let trimmed = input.trim().to_lowercase();
                trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
            })
            .unwrap_or(false)
    });
    let tool_results = tools::check_and_install_tools(&meta.tools, opts.yes, Some(&prompt_fn));

    // Check env vars
    let project_root = if opts.global {
        dirs::home_dir()
            .ok_or_else(|| "cannot determine home directory for env check".to_string())?
    } else {
        opts.project_dir.to_path_buf()
    };
    let env_results = env_check::check_env_vars(&meta.env, &project_root);

    if !opts.json {
        for result in &tool_results {
            print_tool_result(result);
        }
        for result in &env_results {
            print_env_result(result);
        }
    }

    Ok((tool_results, env_results))
}

/// Print a tool check result to stdout.
fn print_tool_result(result: &ToolResult) {
    let suffix = match &result.status {
        ToolStatus::Found => "found on PATH".to_string(),
        ToolStatus::Installed => "installed".to_string(),
        ToolStatus::Declined => "declined".to_string(),
        ToolStatus::Failed(err) => format!("install failed: {err}"),
        ToolStatus::NoCommand => "not found, no install command for this OS".to_string(),
        ToolStatus::Skipped => "not found on PATH (skipped)".to_string(),
    };
    println!("  [tool]    {} — {suffix}", result.name);
}

/// Print an env var check result to stdout.
fn print_env_result(result: &EnvResult) {
    match result.status {
        EnvStatus::FoundInEnv | EnvStatus::FoundInSettings => {
            // Don't print anything for found vars (clean output)
        }
        EnvStatus::MissingRequired => {
            println!("  [warn]    Missing required env: {}", result.name);
            if !result.description.is_empty() {
                println!("            {}", result.description);
            }
            println!("            Set in .claude/settings.json under env");
        }
        EnvStatus::MissingOptional => {
            if !result.description.is_empty() {
                println!(
                    "  [warn]    Missing optional env: {} — {}",
                    result.name, result.description
                );
            }
        }
    }
}

/// The primary file name for display purposes.
fn primary_file(resource_type: ResourceType, name: &str) -> String {
    let agent_type = AgentType::Claude;
    match resource_type {
        ResourceType::Skill => format!("{}/{}/SKILL.md", agent_type.skills_dir(), name),
        ResourceType::Agent => format!("{}/{name}.md", agent_type.agents_dir()),
        ResourceType::Command => format!("{}/{name}.md", agent_type.commands_dir()),
        ResourceType::Rule => format!("{}/{name}.md", agent_type.rules_dir()),
    }
}

/// Write cached resource files to the project's Claude Code directory.
///
/// Returns the install directory and a list of files that were overwritten.
fn write_to_project(
    project_root: &Path,
    resource_type: ResourceType,
    name: &str,
    version: &Version,
    cache: &DownloadCache,
) -> Result<WriteResult, String> {
    let agent_type = AgentType::Claude;
    let file_paths = cache
        .list_files(resource_type, name, version)
        .map_err(|e| e.to_string())?;

    if file_paths.is_empty() {
        return Err(format!(
            "download for {} {}@{} contains no files",
            resource_type, name, version
        ));
    }

    // Skills install into a named subdirectory; other types into a flat type directory
    let install_dir = match resource_type {
        ResourceType::Skill => project_root.join(agent_type.skills_dir()).join(name),
        ResourceType::Agent => project_root.join(agent_type.agents_dir()),
        ResourceType::Command => project_root.join(agent_type.commands_dir()),
        ResourceType::Rule => project_root.join(agent_type.rules_dir()),
    };

    std::fs::create_dir_all(&install_dir)
        .map_err(|e| format!("failed to create {}: {}", install_dir.display(), e))?;

    let mut overwritten = Vec::new();

    match resource_type {
        ResourceType::Skill => {
            // Multi-file resource: copy all files preserving directory structure
            for file_path in &file_paths {
                let content = cache
                    .read_file(resource_type, name, version, file_path)
                    .map_err(|e| e.to_string())?;
                let dest = install_dir.join(file_path);
                if dest.exists() {
                    overwritten.push(file_path.clone());
                }
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
                }
                std::fs::write(&dest, &content)
                    .map_err(|e| format!("failed to write {}: {}", dest.display(), e))?;
            }
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            // Single-file resource: write as <name>.md
            // file_paths is guaranteed non-empty by the check above
            let source_path = &file_paths[0];
            let content = cache
                .read_file(resource_type, name, version, source_path)
                .map_err(|e| e.to_string())?;
            let dest = install_dir.join(format!("{name}.md"));
            if dest.exists() {
                overwritten.push(format!("{name}.md"));
            }
            std::fs::write(&dest, &content)
                .map_err(|e| format!("failed to write {}: {}", dest.display(), e))?;
        }
    }

    Ok(WriteResult {
        install_dir,
        overwritten,
    })
}

/// Parse a resource type string from CLI input.
pub fn parse_resource_type(s: &str) -> Result<ResourceType, String> {
    ResourceType::from_str(s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::DownloadCache;
    use crate::registry::{DownloadFile, DownloadResponse};
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    fn encode_base64(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn setup_cache_with_skill(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            version: "1.0.0".to_string(),
            files: vec![
                DownloadFile {
                    path: "SKILL.md".to_string(),
                    content: encode_base64(b"# Denden\nSkill content"),
                },
                DownloadFile {
                    path: "templates/greeting.md".to_string(),
                    content: encode_base64(b"Hello template"),
                },
            ],
        };
        cache
            .store(ResourceType::Skill, "denden", &v, &response)
            .unwrap();
        cache
    }

    fn setup_cache_with_agent(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("0.5.0").unwrap();
        let response = DownloadResponse {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "0.5.0".to_string(),
            files: vec![DownloadFile {
                path: "debugger.md".to_string(),
                content: encode_base64(b"# Debugger Agent"),
            }],
        };
        cache
            .store(ResourceType::Agent, "debugger", &v, &response)
            .unwrap();
        cache
    }

    fn setup_cache_with_command(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "command".to_string(),
            name: "commit".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "commit.md".to_string(),
                content: encode_base64(b"# Commit Command"),
            }],
        };
        cache
            .store(ResourceType::Command, "commit", &v, &response)
            .unwrap();
        cache
    }

    fn setup_cache_with_rule(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "rule".to_string(),
            name: "no-console-log".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "no-console-log.md".to_string(),
                content: encode_base64(b"# No Console Log Rule"),
            }],
        };
        cache
            .store(ResourceType::Rule, "no-console-log", &v, &response)
            .unwrap();
        cache
    }

    /// Set up a cached skill with tools and env metadata in its SKILL.md frontmatter.
    fn setup_cache_with_skill_metadata(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        let skill_md = r#"---
name: code-review
description: Code review with security checks
metadata:
  relava:
    tools:
      sh:
        description: Bourne shell
        install:
          macos: echo already-installed
          linux: echo already-installed
          windows: echo already-installed
      fake-tool-xyz-never-exists:
        description: A tool that never exists
        install:
          nonexistent-os: echo nope
    env:
      PATH:
        required: true
        description: System PATH (always set)
      RELAVA_TEST_MISSING_REQ_VAR_12345:
        required: true
        description: A missing required var
      RELAVA_TEST_MISSING_OPT_VAR_12345:
        required: false
        description: A missing optional var
---
# Code Review Skill
Review code for security issues.
"#;
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "code-review".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: encode_base64(skill_md.as_bytes()),
            }],
        };
        cache
            .store(ResourceType::Skill, "code-review", &v, &response)
            .unwrap();
        cache
    }

    /// Set up a cached skill with no frontmatter metadata.
    fn setup_cache_with_plain_skill(cache_dir: &Path) -> DownloadCache {
        let cache = DownloadCache::new(cache_dir.to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "plain-skill".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: encode_base64(b"# Plain Skill\nNo metadata."),
            }],
        };
        cache
            .store(ResourceType::Skill, "plain-skill", &v, &response)
            .unwrap();
        cache
    }

    #[test]
    fn write_skill_to_project() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_skill(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        let result =
            write_to_project(project.path(), ResourceType::Skill, "denden", &v, &cache).unwrap();

        assert_eq!(
            result.install_dir,
            project.path().join(".claude/skills/denden")
        );
        assert!(result.overwritten.is_empty());
        assert!(
            project
                .path()
                .join(".claude/skills/denden/SKILL.md")
                .exists()
        );
        assert!(
            project
                .path()
                .join(".claude/skills/denden/templates/greeting.md")
                .exists()
        );

        let content =
            fs::read_to_string(project.path().join(".claude/skills/denden/SKILL.md")).unwrap();
        assert_eq!(content, "# Denden\nSkill content");
    }

    #[test]
    fn write_agent_to_project() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_agent(cache_dir.path());
        let v = Version::parse("0.5.0").unwrap();

        let result =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();

        assert_eq!(result.install_dir, project.path().join(".claude/agents"));
        assert!(result.overwritten.is_empty());
        let agent_path = project.path().join(".claude/agents/debugger.md");
        assert!(agent_path.exists());
        let content = fs::read_to_string(&agent_path).unwrap();
        assert_eq!(content, "# Debugger Agent");
    }

    #[test]
    fn write_command_to_project() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_command(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        let result =
            write_to_project(project.path(), ResourceType::Command, "commit", &v, &cache).unwrap();

        assert_eq!(result.install_dir, project.path().join(".claude/commands"));
        assert!(result.overwritten.is_empty());
        let cmd_path = project.path().join(".claude/commands/commit.md");
        assert!(cmd_path.exists());
        let content = fs::read_to_string(&cmd_path).unwrap();
        assert_eq!(content, "# Commit Command");
    }

    #[test]
    fn write_rule_to_project() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_rule(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        let result = write_to_project(
            project.path(),
            ResourceType::Rule,
            "no-console-log",
            &v,
            &cache,
        )
        .unwrap();

        assert_eq!(result.install_dir, project.path().join(".claude/rules"));
        assert!(result.overwritten.is_empty());
        let rule_path = project.path().join(".claude/rules/no-console-log.md");
        assert!(rule_path.exists());
        let content = fs::read_to_string(&rule_path).unwrap();
        assert_eq!(content, "# No Console Log Rule");
    }

    #[test]
    fn parse_resource_type_valid() {
        assert_eq!(parse_resource_type("skill").unwrap(), ResourceType::Skill);
        assert_eq!(parse_resource_type("agent").unwrap(), ResourceType::Agent);
        assert_eq!(
            parse_resource_type("command").unwrap(),
            ResourceType::Command
        );
        assert_eq!(parse_resource_type("rule").unwrap(), ResourceType::Rule);
    }

    #[test]
    fn parse_resource_type_invalid() {
        assert!(parse_resource_type("plugin").is_err());
        assert!(parse_resource_type("").is_err());
        assert!(parse_resource_type("Skill").is_err());
    }

    #[test]
    fn invalid_slug_rejected() {
        let project = temp_dir();
        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            name: "INVALID_NAME",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid slug"));
    }

    #[test]
    fn primary_file_format() {
        assert_eq!(
            primary_file(ResourceType::Skill, "denden"),
            ".claude/skills/denden/SKILL.md"
        );
        assert_eq!(
            primary_file(ResourceType::Agent, "debugger"),
            ".claude/agents/debugger.md"
        );
        assert_eq!(
            primary_file(ResourceType::Command, "commit"),
            ".claude/commands/commit.md"
        );
        assert_eq!(
            primary_file(ResourceType::Rule, "no-console-log"),
            ".claude/rules/no-console-log.md"
        );
    }

    #[test]
    fn server_unreachable_gives_clear_error() {
        let project = temp_dir();
        let opts = InstallOpts {
            server_url: "http://localhost:19999",
            resource_type: ResourceType::Skill,
            name: "denden",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        // The error should mention the server or connection issue
        let err = result.unwrap_err();
        assert!(
            err.contains("not reachable") || err.contains("error") || err.contains("HTTP"),
            "Error message should indicate server issue: {err}"
        );
    }

    // -- Post-install tests --

    /// Install the "code-review" skill with metadata and return post-install results.
    /// Shared setup for most post-install tests.
    fn run_post_install_with_metadata(
        project: &Path,
    ) -> (PathBuf, Vec<ToolResult>, Vec<EnvResult>) {
        let cache_dir = temp_dir();
        let cache = setup_cache_with_skill_metadata(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();
        let install_dir = write_to_project(project, ResourceType::Skill, "code-review", &v, &cache)
            .unwrap()
            .install_dir;

        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            name: "code-review",
            version_pin: None,
            project_dir: project,
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };

        let (tools, env) = run_skill_post_install(&opts, &install_dir).unwrap();
        (install_dir, tools, env)
    }

    #[test]
    fn post_install_parses_tool_metadata() {
        let project = temp_dir();
        let (install_dir, _, _) = run_post_install_with_metadata(project.path());

        let meta = ResourceMeta::from_file(&install_dir.join("SKILL.md")).unwrap();
        assert_eq!(meta.tools.len(), 2);
        assert!(meta.tools.contains_key("sh"));
        assert!(meta.tools.contains_key("fake-tool-xyz-never-exists"));
    }

    #[test]
    fn post_install_parses_env_metadata() {
        let project = temp_dir();
        let (install_dir, _, _) = run_post_install_with_metadata(project.path());

        let meta = ResourceMeta::from_file(&install_dir.join("SKILL.md")).unwrap();
        assert_eq!(meta.env.len(), 3);
        assert!(meta.env["PATH"].required);
        assert!(meta.env["RELAVA_TEST_MISSING_REQ_VAR_12345"].required);
        assert!(!meta.env["RELAVA_TEST_MISSING_OPT_VAR_12345"].required);
    }

    #[test]
    fn post_install_tool_found_on_path() {
        let project = temp_dir();
        let (_, tool_results, _) = run_post_install_with_metadata(project.path());

        let sh_result = tool_results.iter().find(|r| r.name == "sh").unwrap();
        assert_eq!(sh_result.status, ToolStatus::Found);
    }

    #[test]
    fn post_install_tool_no_command_for_os() {
        let project = temp_dir();
        let (_, tool_results, _) = run_post_install_with_metadata(project.path());

        let missing = tool_results
            .iter()
            .find(|r| r.name == "fake-tool-xyz-never-exists")
            .unwrap();
        assert_eq!(missing.status, ToolStatus::NoCommand);
    }

    #[test]
    fn post_install_env_found_in_process() {
        let project = temp_dir();
        let (_, _, env_results) = run_post_install_with_metadata(project.path());

        let path_result = env_results.iter().find(|r| r.name == "PATH").unwrap();
        assert_eq!(path_result.status, EnvStatus::FoundInEnv);
    }

    #[test]
    fn post_install_env_missing_required() {
        let project = temp_dir();
        let (_, _, env_results) = run_post_install_with_metadata(project.path());

        let missing = env_results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_MISSING_REQ_VAR_12345")
            .unwrap();
        assert_eq!(missing.status, EnvStatus::MissingRequired);
        assert!(missing.required);
    }

    #[test]
    fn post_install_env_missing_optional() {
        let project = temp_dir();
        let (_, _, env_results) = run_post_install_with_metadata(project.path());

        let optional = env_results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_MISSING_OPT_VAR_12345")
            .unwrap();
        assert_eq!(optional.status, EnvStatus::MissingOptional);
        assert!(!optional.required);
    }

    #[test]
    fn post_install_env_in_settings_json() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_skill_metadata(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();
        let install_dir = write_to_project(
            project.path(),
            ResourceType::Skill,
            "code-review",
            &v,
            &cache,
        )
        .unwrap()
        .install_dir;

        // Write a settings.json with the missing required env var
        let claude_dir = project.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(
            claude_dir.join("settings.json"),
            r#"{"env": {"RELAVA_TEST_MISSING_REQ_VAR_12345": "some-token"}}"#,
        )
        .unwrap();

        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            name: "code-review",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };

        let (_, env_results) = run_skill_post_install(&opts, &install_dir).unwrap();

        let req = env_results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_MISSING_REQ_VAR_12345")
            .unwrap();
        assert_eq!(req.status, EnvStatus::FoundInSettings);
    }

    #[test]
    fn post_install_skipped_for_non_skills() {
        let project = temp_dir();
        let install_dir = project.path().join(".claude/agents");
        fs::create_dir_all(&install_dir).unwrap();

        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Agent,
            name: "debugger",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };

        let (tools, env) = run_skill_post_install(&opts, &install_dir).unwrap();
        assert!(tools.is_empty());
        assert!(env.is_empty());
    }

    #[test]
    fn post_install_no_metadata_in_skill() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_plain_skill(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();
        let install_dir = write_to_project(
            project.path(),
            ResourceType::Skill,
            "plain-skill",
            &v,
            &cache,
        )
        .unwrap()
        .install_dir;

        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            name: "plain-skill",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };

        let (tools, env) = run_skill_post_install(&opts, &install_dir).unwrap();
        assert!(tools.is_empty());
        assert!(env.is_empty());
    }

    #[test]
    fn post_install_malformed_frontmatter_warns_not_errors() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let v = Version::parse("1.0.0").unwrap();
        // Malformed YAML frontmatter
        let skill_md = "---\nmetadata:\n  relava:\n    tools: [invalid\n---\n# Bad Skill\n";
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "bad-meta".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: encode_base64(skill_md.as_bytes()),
            }],
        };
        cache
            .store(ResourceType::Skill, "bad-meta", &v, &response)
            .unwrap();

        let install_dir =
            write_to_project(project.path(), ResourceType::Skill, "bad-meta", &v, &cache)
                .unwrap()
                .install_dir;

        let opts = InstallOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            name: "bad-meta",
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };

        // Should return Ok with empty results, not Err
        let (tools, env) = run_skill_post_install(&opts, &install_dir).unwrap();
        assert!(tools.is_empty());
        assert!(env.is_empty());
    }

    #[test]
    fn install_result_serializes_with_tools_and_env() {
        let result = InstallResult {
            resource_type: "skill".to_string(),
            name: "code-review".to_string(),
            version: "1.0.0".to_string(),
            files: vec!["SKILL.md".to_string()],
            install_dir: ".claude/skills/code-review".to_string(),
            overwritten: Vec::new(),
            tools: vec![ToolResult {
                name: "gh".to_string(),
                description: "GitHub CLI".to_string(),
                status: ToolStatus::Found,
            }],
            env: vec![EnvResult {
                name: "GITHUB_TOKEN".to_string(),
                description: "GitHub API token".to_string(),
                required: true,
                status: EnvStatus::MissingRequired,
            }],
            dependencies: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"tools\""));
        assert!(json.contains("\"gh\""));
        assert!(json.contains("\"env\""));
        assert!(json.contains("GITHUB_TOKEN"));
        assert!(json.contains("missing_required"));
    }

    #[test]
    fn install_result_omits_empty_tools_and_env() {
        let result = InstallResult {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "0.5.0".to_string(),
            files: vec!["debugger.md".to_string()],
            install_dir: ".claude/agents".to_string(),
            overwritten: Vec::new(),
            tools: Vec::new(),
            env: Vec::new(),
            dependencies: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(!json.contains("\"tools\""), "empty tools should be omitted");
        assert!(!json.contains("\"env\""), "empty env should be omitted");
    }

    #[test]
    fn print_tool_result_all_statuses() {
        let statuses = vec![
            ToolStatus::Found,
            ToolStatus::Installed,
            ToolStatus::Declined,
            ToolStatus::Failed("error msg".to_string()),
            ToolStatus::NoCommand,
            ToolStatus::Skipped,
        ];
        for status in statuses {
            print_tool_result(&ToolResult {
                name: "test".to_string(),
                description: "test tool".to_string(),
                status,
            });
        }
    }

    #[test]
    fn print_env_result_all_statuses() {
        let statuses = vec![
            EnvStatus::FoundInEnv,
            EnvStatus::FoundInSettings,
            EnvStatus::MissingRequired,
            EnvStatus::MissingOptional,
        ];
        for status in statuses {
            print_env_result(&EnvResult {
                name: "TEST_VAR".to_string(),
                description: "test".to_string(),
                required: true,
                status,
            });
        }
    }

    // -- Overwrite detection tests --

    #[test]
    fn overwrite_detected_for_agent() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_agent(cache_dir.path());
        let v = Version::parse("0.5.0").unwrap();

        // First install — no overwrite
        let result =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();
        assert!(result.overwritten.is_empty());

        // Second install — should detect overwrite
        let result =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();
        assert_eq!(result.overwritten, vec!["debugger.md"]);
    }

    #[test]
    fn overwrite_detected_for_command() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_command(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        // First install
        write_to_project(project.path(), ResourceType::Command, "commit", &v, &cache).unwrap();

        // Second install — overwrite
        let result =
            write_to_project(project.path(), ResourceType::Command, "commit", &v, &cache).unwrap();
        assert_eq!(result.overwritten, vec!["commit.md"]);
    }

    #[test]
    fn overwrite_detected_for_rule() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_rule(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        // First install
        write_to_project(
            project.path(),
            ResourceType::Rule,
            "no-console-log",
            &v,
            &cache,
        )
        .unwrap();

        // Second install — overwrite
        let result = write_to_project(
            project.path(),
            ResourceType::Rule,
            "no-console-log",
            &v,
            &cache,
        )
        .unwrap();
        assert_eq!(result.overwritten, vec!["no-console-log.md"]);
    }

    #[test]
    fn overwrite_detected_for_skill() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_skill(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        // First install
        write_to_project(project.path(), ResourceType::Skill, "denden", &v, &cache).unwrap();

        // Second install — should detect overwritten files
        let result =
            write_to_project(project.path(), ResourceType::Skill, "denden", &v, &cache).unwrap();
        assert!(result.overwritten.contains(&"SKILL.md".to_string()));
        assert!(
            result
                .overwritten
                .contains(&"templates/greeting.md".to_string())
        );
    }

    #[test]
    fn overwrite_updates_file_content() {
        let project = temp_dir();
        let cache_dir = temp_dir();

        // Install initial version
        let cache = setup_cache_with_agent(cache_dir.path());
        let v = Version::parse("0.5.0").unwrap();
        write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();

        let path = project.path().join(".claude/agents/debugger.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "# Debugger Agent");

        // Install updated version with different content
        let v2 = Version::parse("0.6.0").unwrap();
        let response = DownloadResponse {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "0.6.0".to_string(),
            files: vec![DownloadFile {
                path: "debugger.md".to_string(),
                content: encode_base64(b"# Debugger Agent v2\nUpdated content"),
            }],
        };
        cache
            .store(ResourceType::Agent, "debugger", &v2, &response)
            .unwrap();

        let result =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v2, &cache).unwrap();
        assert_eq!(result.overwritten, vec!["debugger.md"]);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Debugger Agent v2\nUpdated content"
        );
    }

    #[test]
    fn agent_creates_directory_if_missing() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_agent(cache_dir.path());
        let v = Version::parse("0.5.0").unwrap();

        // Directory doesn't exist yet
        assert!(!project.path().join(".claude/agents").exists());

        let result =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();

        assert!(project.path().join(".claude/agents").is_dir());
        assert!(project.path().join(".claude/agents/debugger.md").exists());
        assert!(result.overwritten.is_empty());
    }

    #[test]
    fn command_creates_directory_if_missing() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_command(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        assert!(!project.path().join(".claude/commands").exists());

        write_to_project(project.path(), ResourceType::Command, "commit", &v, &cache).unwrap();

        assert!(project.path().join(".claude/commands").is_dir());
        assert!(project.path().join(".claude/commands/commit.md").exists());
    }

    #[test]
    fn rule_creates_directory_if_missing() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_rule(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        assert!(!project.path().join(".claude/rules").exists());

        write_to_project(
            project.path(),
            ResourceType::Rule,
            "no-console-log",
            &v,
            &cache,
        )
        .unwrap();

        assert!(project.path().join(".claude/rules").is_dir());
        assert!(
            project
                .path()
                .join(".claude/rules/no-console-log.md")
                .exists()
        );
    }

    #[test]
    fn install_result_serializes_overwritten() {
        let result = InstallResult {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "0.5.0".to_string(),
            files: vec!["debugger.md".to_string()],
            install_dir: ".claude/agents".to_string(),
            overwritten: vec!["debugger.md".to_string()],
            tools: Vec::new(),
            env: Vec::new(),
            dependencies: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"overwritten\""));
        assert!(json.contains("debugger.md"));
    }

    #[test]
    fn install_result_omits_empty_overwritten() {
        let result = InstallResult {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "0.5.0".to_string(),
            files: vec!["debugger.md".to_string()],
            install_dir: ".claude/agents".to_string(),
            overwritten: Vec::new(),
            tools: Vec::new(),
            env: Vec::new(),
            dependencies: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(
            !json.contains("\"overwritten\""),
            "empty overwritten should be omitted"
        );
    }

    #[test]
    fn multiple_resources_in_same_directory() {
        let project = temp_dir();
        let cache_dir = temp_dir();

        // Install two different agents to the same .claude/agents/ directory
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let v = Version::parse("1.0.0").unwrap();

        let response1 = DownloadResponse {
            resource_type: "agent".to_string(),
            name: "debugger".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "debugger.md".to_string(),
                content: encode_base64(b"# Debugger Agent"),
            }],
        };
        cache
            .store(ResourceType::Agent, "debugger", &v, &response1)
            .unwrap();

        let response2 = DownloadResponse {
            resource_type: "agent".to_string(),
            name: "reviewer".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "reviewer.md".to_string(),
                content: encode_base64(b"# Reviewer Agent"),
            }],
        };
        cache
            .store(ResourceType::Agent, "reviewer", &v, &response2)
            .unwrap();

        write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();
        write_to_project(project.path(), ResourceType::Agent, "reviewer", &v, &cache).unwrap();

        // Both agents should exist side by side
        assert!(project.path().join(".claude/agents/debugger.md").exists());
        assert!(project.path().join(".claude/agents/reviewer.md").exists());
    }
}
