use std::path::Path;

use relava_types::manifest::{ProjectManifest, ResourceMeta};
use relava_types::validate::{self, ResourceType};

use crate::install;

/// Options for the info command.
pub struct InfoOpts<'a> {
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// Result of the info command.
#[derive(Debug, serde::Serialize)]
pub struct InfoResult {
    pub name: String,
    pub resource_type: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub file_count: usize,
    pub total_size: u64,
    pub install_location: String,
}

/// Run `relava info <type> <name>`.
///
/// Displays detailed information about an installed resource, including
/// name, version, type, description, dependencies, file count, total
/// size, and install location.
pub fn run(opts: &InfoOpts) -> Result<InfoResult, String> {
    // Validate slug before filesystem operations
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    if !install::is_installed(opts.project_dir, opts.resource_type, opts.name) {
        return Err(format!(
            "{} '{}' is not installed",
            opts.resource_type, opts.name
        ));
    }

    let install_path = install::resource_path(opts.project_dir, opts.resource_type, opts.name);
    let (file_count, total_size) = compute_size(opts.resource_type, &install_path);
    let version = load_manifest_version(opts.project_dir, opts.resource_type, opts.name);
    let (description, dependencies) = load_metadata(opts.resource_type, &install_path);

    let install_display = install_path
        .strip_prefix(opts.project_dir)
        .unwrap_or(&install_path)
        .to_string_lossy()
        .to_string();

    let result = InfoResult {
        name: opts.name.to_string(),
        resource_type: opts.resource_type.to_string(),
        version,
        description,
        dependencies,
        file_count,
        total_size,
        install_location: install_display,
    };

    if !opts.json {
        print_info(&result);
    }

    Ok(result)
}

/// Count files and total byte size for a resource.
fn compute_size(resource_type: ResourceType, path: &Path) -> (usize, u64) {
    match resource_type {
        ResourceType::Skill => dir_size(path),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            match std::fs::metadata(path) {
                Ok(meta) => (1, meta.len()),
                Err(e) => {
                    eprintln!("[warn] cannot read {}: {e}", path.display());
                    (0, 0)
                }
            }
        }
    }
}

/// Recursively compute file count and total size of a directory.
fn dir_size(path: &Path) -> (usize, u64) {
    let mut count = 0usize;
    let mut size = 0u64;

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[warn] cannot read {}: {e}", path.display());
            return (0, 0);
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            let (c, s) = dir_size(&entry_path);
            count += c;
            size += s;
        } else if entry_path.is_file() {
            count += 1;
            if let Ok(meta) = std::fs::metadata(&entry_path) {
                size += meta.len();
            }
        }
    }

    (count, size)
}

/// Look up version from relava.toml.
///
/// Returns empty string if relava.toml does not exist. Warns on parse errors.
fn load_manifest_version(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
) -> String {
    let path = project_dir.join("relava.toml");
    let manifest = match ProjectManifest::from_file(&path) {
        Ok(m) => m,
        Err(e) => {
            // Silently ignore missing file; warn on parse errors
            if !path.exists() {
                return String::new();
            }
            eprintln!("[warn] failed to read relava.toml: {e}");
            return String::new();
        }
    };
    let section = match resource_type {
        ResourceType::Skill => &manifest.skills,
        ResourceType::Agent => &manifest.agents,
        ResourceType::Command => &manifest.commands,
        ResourceType::Rule => &manifest.rules,
    };
    section.get(name).cloned().unwrap_or_default()
}

/// Extract description and resource dependencies (skills + agents) from metadata.
fn load_metadata(resource_type: ResourceType, path: &Path) -> (String, Vec<String>) {
    let md_path = match resource_type {
        ResourceType::Skill => path.join("SKILL.md"),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => path.to_path_buf(),
    };

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(_) => return (String::new(), Vec::new()),
    };

    // Try to parse frontmatter metadata
    let meta = ResourceMeta::from_md(&content).unwrap_or_default();
    let mut deps = meta.skills;
    deps.extend(meta.agents);

    // Extract description from the first non-empty, non-heading line after frontmatter
    let description = extract_description(&content);

    (description, deps)
}

/// Extract a short description from markdown content.
///
/// Looks for the first paragraph line after frontmatter and the title heading.
fn extract_description(content: &str) -> String {
    let body = strip_frontmatter(content);

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        return trimmed.to_string();
    }

    String::new()
}

/// Strip YAML frontmatter (including delimiters) from markdown content, returning the body.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_open = &trimmed[3..];
    match after_open.find("\n---") {
        Some(end) => {
            let rest = &after_open[end + 4..];
            // Skip the newline after closing ---
            rest.strip_prefix('\n').unwrap_or(rest)
        }
        None => content,
    }
}

/// Format a byte size for human-readable display.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Print info in human-readable format.
fn print_info(info: &InfoResult) {
    println!("Name:           {}", info.name);
    println!("Type:           {}", info.resource_type);
    let version_display = if info.version.is_empty() {
        "-"
    } else {
        &info.version
    };
    println!("Version:        {version_display}");
    if !info.description.is_empty() {
        println!("Description:    {}", info.description);
    }
    if !info.dependencies.is_empty() {
        println!("Dependencies:   {}", info.dependencies.join(", "));
    }
    println!("Files:          {}", info.file_count);
    println!("Size:           {}", format_size(info.total_size));
    println!("Location:       {}", info.install_location);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    #[test]
    fn info_installed_skill() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden\nA communication skill.").unwrap();
        fs::write(skill_dir.join("extra.md"), "extra content").unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.name, "denden");
        assert_eq!(result.resource_type, "skill");
        assert_eq!(result.file_count, 2);
        assert!(result.total_size > 0);
        assert_eq!(result.description, "A communication skill.");
        assert_eq!(result.install_location, ".claude/skills/denden");
    }

    #[test]
    fn info_installed_agent() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger\nDebugs code.").unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Agent,
            name: "debugger",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.name, "debugger");
        assert_eq!(result.resource_type, "agent");
        assert_eq!(result.file_count, 1);
        assert!(result.total_size > 0);
        assert_eq!(result.description, "Debugs code.");
        assert_eq!(result.install_location, ".claude/agents/debugger.md");
    }

    #[test]
    fn info_installed_command() {
        let root = temp_dir();
        let cmds_dir = root.path().join(".claude/commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("deploy.md"), "# Deploy\nDeploy to production.").unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Command,
            name: "deploy",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.name, "deploy");
        assert_eq!(result.resource_type, "command");
        assert_eq!(result.description, "Deploy to production.");
    }

    #[test]
    fn info_installed_rule() {
        let root = temp_dir();
        let rules_dir = root.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(
            rules_dir.join("no-console-log.md"),
            "# No Console Log\nDisallow console.log in production.",
        )
        .unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Rule,
            name: "no-console-log",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.name, "no-console-log");
        assert_eq!(result.resource_type, "rule");
        assert_eq!(result.description, "Disallow console.log in production.");
    }

    #[test]
    fn info_not_installed_errors() {
        let root = temp_dir();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "nonexistent",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        let err = run(&opts).unwrap_err();
        assert!(err.contains("not installed"));
    }

    #[test]
    fn info_with_manifest_version() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.2.0\"\n",
        )
        .unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.version, "1.2.0");
    }

    #[test]
    fn info_without_manifest_empty_version() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.version, "");
    }

    #[test]
    fn info_with_dependencies() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/code-review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nmetadata:\n  relava:\n    skills:\n      - security-baseline\n      - linting\n---\n# Code Review\nReview code for quality.",
        )
        .unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "code-review",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.dependencies, vec!["security-baseline", "linting"]);
        assert_eq!(result.description, "Review code for quality.");
    }

    #[test]
    fn info_result_serializes_to_json() {
        let result = InfoResult {
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A skill".to_string(),
            dependencies: vec!["dep-a".to_string()],
            file_count: 3,
            total_size: 1024,
            install_location: ".claude/skills/denden".to_string(),
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("dep-a"));
        assert!(json.contains("1024"));
    }

    #[test]
    fn info_skill_with_subdirectories() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(skill_dir.join("templates")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();
        fs::write(skill_dir.join("templates/greeting.md"), "Hello!").unwrap();
        fs::write(skill_dir.join("templates/farewell.md"), "Goodbye!").unwrap();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert_eq!(result.file_count, 3);
    }

    #[test]
    fn extract_description_simple() {
        let content = "# My Skill\nThis is a description.\nMore text.";
        assert_eq!(extract_description(content), "This is a description.");
    }

    #[test]
    fn extract_description_with_frontmatter() {
        let content = "---\nname: test\n---\n# My Skill\nDescription after frontmatter.";
        assert_eq!(
            extract_description(content),
            "Description after frontmatter."
        );
    }

    #[test]
    fn extract_description_empty() {
        let content = "# Title Only\n";
        assert_eq!(extract_description(content), "");
    }

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(2048), "2.0 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1_500_000), "1.4 MB");
    }

    #[test]
    fn info_invalid_slug_errors() {
        let root = temp_dir();

        let opts = InfoOpts {
            resource_type: ResourceType::Skill,
            name: "../../../etc",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        assert!(run(&opts).is_err());
    }

    #[test]
    fn strip_frontmatter_unclosed() {
        // Unclosed frontmatter should return original content
        let content = "---\nname: test\n# No closing delimiter\nSome body text.";
        assert_eq!(strip_frontmatter(content), content);
    }
}
