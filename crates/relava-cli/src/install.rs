use std::path::{Path, PathBuf};

use relava_core::cache::DownloadCache;
use relava_core::registry::RegistryClient;
use relava_core::store::RelavaDir;
use relava_core::validate::{self, AgentType, ResourceType};
use relava_core::version::Version;

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
}

/// Result of a successful install, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct InstallResult {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
    pub install_dir: String,
}

/// Run `relava install <type> <name>`.
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
    let relava_dir = RelavaDir::default_location()
        .ok_or_else(|| "cannot determine home directory for cache".to_string())?;
    let cache = DownloadCache::new(relava_dir.cache_dir());

    // Connect to registry and resolve version
    let client = RegistryClient::new(opts.server_url);

    if opts.verbose {
        eprintln!("resolving {} {}...", opts.resource_type, opts.name);
    }

    let version = client
        .resolve_version(opts.resource_type, opts.name, opts.version_pin)
        .map_err(|e| e.to_string())?;

    if !opts.json {
        println!(
            "Installing {} {}@{}...",
            opts.resource_type, opts.name, version
        );
    }

    // Check cache first, download if needed
    let file_paths = if cache.is_cached(opts.resource_type, opts.name, &version) {
        if opts.verbose {
            eprintln!("  using cached version");
        }
        cache
            .list_files(opts.resource_type, opts.name, &version)
            .map_err(|e| e.to_string())?
    } else {
        if opts.verbose {
            eprintln!("  downloading from {}", opts.server_url);
        }
        let response = client
            .download(opts.resource_type, opts.name, &version)
            .map_err(|e| e.to_string())?;
        cache
            .store(opts.resource_type, opts.name, &version, &response)
            .map_err(|e| e.to_string())?
    };

    // Write files to the correct Claude Code location
    let install_dir = write_to_project(
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
        println!("Installed {} {}@{}", opts.resource_type, opts.name, version);
    }

    Ok(InstallResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        version: version.to_string(),
        files: file_paths,
        install_dir: install_dir_display,
    })
}

/// The primary file name for display purposes.
fn primary_file(resource_type: ResourceType, name: &str) -> String {
    match resource_type {
        ResourceType::Skill => format!(".claude/skills/{}/SKILL.md", name),
        ResourceType::Agent => format!(".claude/agents/{}.md", name),
        ResourceType::Command => format!(".claude/commands/{}.md", name),
        ResourceType::Rule => format!(".claude/rules/{}.md", name),
    }
}

/// Write cached resource files to the project's Claude Code directory.
///
/// Returns the install directory path.
fn write_to_project(
    project_root: &Path,
    resource_type: ResourceType,
    name: &str,
    version: &Version,
    cache: &DownloadCache,
) -> Result<PathBuf, String> {
    let agent_type = AgentType::Claude;
    let file_paths = cache
        .list_files(resource_type, name, version)
        .map_err(|e| e.to_string())?;

    let install_dir = match resource_type {
        ResourceType::Skill => {
            let dir = project_root.join(agent_type.skills_dir()).join(name);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create {}: {}", dir.display(), e))?;

            for file_path in &file_paths {
                let content = cache
                    .read_file(resource_type, name, version, file_path)
                    .map_err(|e| e.to_string())?;
                let dest = dir.join(file_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("failed to create {}: {}", parent.display(), e))?;
                }
                std::fs::write(&dest, &content)
                    .map_err(|e| format!("failed to write {}: {}", dest.display(), e))?;
            }
            dir
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            let type_dir = match resource_type {
                ResourceType::Agent => agent_type.agents_dir(),
                ResourceType::Command => agent_type.commands_dir(),
                ResourceType::Rule => agent_type.rules_dir(),
                _ => unreachable!(),
            };
            let dir = project_root.join(type_dir);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create {}: {}", dir.display(), e))?;

            // Single-file resources: the download should contain one .md file.
            // Write it as <name>.md in the type directory.
            let md_file = format!("{name}.md");
            if let Some(source_path) = file_paths.first() {
                let content = cache
                    .read_file(resource_type, name, version, source_path)
                    .map_err(|e| e.to_string())?;
                let dest = dir.join(&md_file);
                std::fs::write(&dest, &content)
                    .map_err(|e| format!("failed to write {}: {}", dest.display(), e))?;
            }
            dir
        }
    };

    Ok(install_dir)
}

/// Parse a resource type string from CLI input.
pub fn parse_resource_type(s: &str) -> Result<ResourceType, String> {
    ResourceType::from_str(s).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relava_core::cache::DownloadCache;
    use relava_core::registry::{DownloadFile, DownloadResponse};
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    fn encode_base64(data: &[u8]) -> String {
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let combined = (b0 << 16) | (b1 << 8) | b2;
            result.push(alphabet[(combined >> 18) as usize & 0x3f] as char);
            result.push(alphabet[(combined >> 12) as usize & 0x3f] as char);
            if chunk.len() > 1 {
                result.push(alphabet[(combined >> 6) as usize & 0x3f] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(alphabet[combined as usize & 0x3f] as char);
            } else {
                result.push('=');
            }
        }
        result
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

    #[test]
    fn write_skill_to_project() {
        let project = temp_dir();
        let cache_dir = temp_dir();
        let cache = setup_cache_with_skill(cache_dir.path());
        let v = Version::parse("1.0.0").unwrap();

        let dir =
            write_to_project(project.path(), ResourceType::Skill, "denden", &v, &cache).unwrap();

        assert_eq!(dir, project.path().join(".claude/skills/denden"));
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

        let dir =
            write_to_project(project.path(), ResourceType::Agent, "debugger", &v, &cache).unwrap();

        assert_eq!(dir, project.path().join(".claude/agents"));
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

        let dir =
            write_to_project(project.path(), ResourceType::Command, "commit", &v, &cache).unwrap();

        assert_eq!(dir, project.path().join(".claude/commands"));
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

        let dir = write_to_project(
            project.path(),
            ResourceType::Rule,
            "no-console-log",
            &v,
            &cache,
        )
        .unwrap();

        assert_eq!(dir, project.path().join(".claude/rules"));
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
}
