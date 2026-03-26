use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::install;

/// A single entry in the list output.
#[derive(Debug, serde::Serialize)]
pub struct ListEntry {
    pub name: String,
    pub resource_type: String,
    pub version: String,
    pub status: String,
}

/// Result of the list command, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct ListResult {
    pub resources: Vec<ListEntry>,
}

/// Options for the list command.
pub struct ListOpts<'a> {
    pub resource_type: Option<ResourceType>,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// Run `relava list [type]`.
///
/// Scans installed resource directories and returns a list of all
/// discovered resources. If a type filter is provided, only resources
/// of that type are returned.
pub fn run(opts: &ListOpts) -> Result<ListResult, String> {
    let manifest = load_manifest(opts.project_dir);

    let types = match opts.resource_type {
        Some(rt) => vec![rt],
        None => ResourceType::ALL.to_vec(),
    };

    let mut resources = Vec::new();
    for rt in &types {
        let entries = scan_type(opts.project_dir, *rt, &manifest);
        resources.extend(entries);
    }

    if !opts.json {
        if resources.is_empty() {
            let msg = match opts.resource_type {
                Some(rt) => format!(
                    "No {}s installed. Run `relava install {} <name>` to get started.",
                    rt, rt
                ),
                None => {
                    "No resources installed. Run `relava install <type> <name>` to get started."
                        .to_string()
                }
            };
            println!("{msg}");
        } else {
            print_table(&resources);
        }
    }

    Ok(ListResult { resources })
}

/// Scan a single resource type directory for installed resources.
fn scan_type(
    project_dir: &Path,
    resource_type: ResourceType,
    manifest: &Option<ProjectManifest>,
) -> Vec<ListEntry> {
    let type_dir = install::type_dir(project_dir, resource_type);

    if !type_dir.is_dir() {
        return Vec::new();
    }

    let mut entries = Vec::new();

    match resource_type {
        ResourceType::Skill => {
            // Skills are subdirectories containing SKILL.md
            if let Ok(read_dir) = std::fs::read_dir(&type_dir) {
                for entry in read_dir.flatten() {
                    if !entry.path().is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !install::is_installed(project_dir, ResourceType::Skill, &name) {
                        continue;
                    }
                    let version = manifest_version(manifest, resource_type, &name);
                    entries.push(ListEntry {
                        name,
                        resource_type: resource_type.to_string(),
                        version,
                        status: "active".to_string(),
                    });
                }
            }
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            // Single .md files
            if let Ok(read_dir) = std::fs::read_dir(&type_dir) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    let file_name = entry.file_name().to_string_lossy().to_string();
                    let name = match file_name.strip_suffix(".md") {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let version = manifest_version(manifest, resource_type, &name);
                    entries.push(ListEntry {
                        name,
                        resource_type: resource_type.to_string(),
                        version,
                        status: "active".to_string(),
                    });
                }
            }
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Look up the version for a resource in relava.toml, if it exists.
fn manifest_version(
    manifest: &Option<ProjectManifest>,
    resource_type: ResourceType,
    name: &str,
) -> String {
    let Some(m) = manifest else {
        return String::new();
    };
    let section = match resource_type {
        ResourceType::Skill => &m.skills,
        ResourceType::Agent => &m.agents,
        ResourceType::Command => &m.commands,
        ResourceType::Rule => &m.rules,
    };
    section.get(name).cloned().unwrap_or_default()
}

/// Load the project manifest, returning None if it doesn't exist or can't be parsed.
fn load_manifest(project_dir: &Path) -> Option<ProjectManifest> {
    let path = project_dir.join("relava.toml");
    ProjectManifest::from_file(&path).ok()
}

/// Print a formatted table of list entries.
fn print_table(entries: &[ListEntry]) {
    // Header
    println!(
        "{:<24} {:<10} {:<12} {}",
        "NAME", "TYPE", "VERSION", "STATUS"
    );
    println!("{}", "-".repeat(56));
    for entry in entries {
        let version_display = if entry.version.is_empty() {
            "-"
        } else {
            &entry.version
        };
        println!(
            "{:<24} {:<10} {:<12} {}",
            entry.name, entry.resource_type, version_display, entry.status
        );
    }
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
    fn list_empty_project() {
        let root = temp_dir();
        let opts = ListOpts {
            resource_type: None,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.resources.is_empty());
    }

    #[test]
    fn list_all_types() {
        let root = temp_dir();

        // Create a skill
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Create an agent
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        // Create a command
        let cmds_dir = root.path().join(".claude/commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("deploy.md"), "# Deploy").unwrap();

        // Create a rule
        let rules_dir = root.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("no-console-log.md"), "# Rule").unwrap();

        let opts = ListOpts {
            resource_type: None,
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 4);

        let names: Vec<&str> = result.resources.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"denden"));
        assert!(names.contains(&"debugger"));
        assert!(names.contains(&"deploy"));
        assert!(names.contains(&"no-console-log"));
    }

    #[test]
    fn list_filtered_by_type() {
        let root = temp_dir();

        // Create a skill
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Create an agent (should not appear)
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].name, "denden");
        assert_eq!(result.resources[0].resource_type, "skill");
    }

    #[test]
    fn list_skills_requires_skill_md() {
        let root = temp_dir();

        // Directory without SKILL.md should not be listed
        let skill_dir = root.path().join(".claude/skills/incomplete");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("random.txt"), "not a skill").unwrap();

        // Valid skill
        let valid_dir = root.path().join(".claude/skills/valid");
        fs::create_dir_all(&valid_dir).unwrap();
        fs::write(valid_dir.join("SKILL.md"), "# Valid").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].name, "valid");
    }

    #[test]
    fn list_ignores_non_md_files() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();
        fs::write(agents_dir.join("notes.txt"), "not a resource").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Agent),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].name, "debugger");
    }

    #[test]
    fn list_with_manifest_versions() {
        let root = temp_dir();

        // Create a skill
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Create relava.toml with version
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.2.0\"\n",
        )
        .unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].version, "1.2.0");
    }

    #[test]
    fn list_without_manifest_shows_empty_version() {
        let root = temp_dir();

        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources[0].version, "");
    }

    #[test]
    fn list_sorted_alphabetically() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("zebra.md"), "# Zebra").unwrap();
        fs::write(agents_dir.join("alpha.md"), "# Alpha").unwrap();
        fs::write(agents_dir.join("middle.md"), "# Middle").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Agent),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 3);
        assert_eq!(result.resources[0].name, "alpha");
        assert_eq!(result.resources[1].name, "middle");
        assert_eq!(result.resources[2].name, "zebra");
    }

    #[test]
    fn list_empty_type_dir() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Agent),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.resources.is_empty());
    }

    #[test]
    fn list_result_serializes_to_json() {
        let result = ListResult {
            resources: vec![ListEntry {
                name: "denden".to_string(),
                resource_type: "skill".to_string(),
                version: "1.0.0".to_string(),
                status: "active".to_string(),
            }],
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("1.0.0"));
    }

    #[test]
    fn list_multiple_skills() {
        let root = temp_dir();

        for name in &["alpha", "beta", "gamma"] {
            let dir = root.path().join(format!(".claude/skills/{name}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("# {name}")).unwrap();
        }

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 3);
    }
}
