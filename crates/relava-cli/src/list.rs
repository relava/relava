use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::disable;
use crate::install;

/// A single entry in the list output.
#[derive(Debug, serde::Serialize)]
pub struct ListEntry {
    pub name: String,
    pub resource_type: String,
    pub version: String,
    pub status: String,
}

/// Result of the list command.
#[derive(Debug, serde::Serialize)]
pub struct ListResult {
    pub resources: Vec<ListEntry>,
}

/// Options for the list command.
pub struct ListOpts<'a> {
    pub resource_type: Option<ResourceType>,
    pub project_dir: &'a Path,
    pub json: bool,
    pub _verbose: bool,
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

    let read_dir = match std::fs::read_dir(&type_dir) {
        Ok(rd) => rd,
        Err(e) => {
            eprintln!("[warn] cannot read {}: {e}", type_dir.display());
            return entries;
        }
    };

    // Scan active resources
    match resource_type {
        ResourceType::Skill => {
            for entry in read_dir.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let dir_name = entry.file_name().to_string_lossy().to_string();

                // Skip the .disabled/ subdirectory itself
                if dir_name == ".disabled" {
                    continue;
                }

                if !install::is_installed(project_dir, ResourceType::Skill, &dir_name) {
                    continue;
                }
                entries.push(make_entry(dir_name, resource_type, manifest, "active"));
            }
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
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
                entries.push(make_entry(name, resource_type, manifest, "active"));
            }
        }
    }

    // Scan disabled resources from .disabled/ subdirectory
    let disabled_dir = disable::disabled_dir_for(project_dir, resource_type);
    if disabled_dir.is_dir() {
        match std::fs::read_dir(&disabled_dir) {
            Ok(disabled_entries) => {
                scan_disabled_entries(disabled_entries, resource_type, manifest, &mut entries);
            }
            Err(e) => eprintln!("[warn] cannot read {}: {e}", disabled_dir.display()),
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Scan a `.disabled/` subdirectory for disabled resources.
fn scan_disabled_entries(
    read_dir: std::fs::ReadDir,
    resource_type: ResourceType,
    manifest: &Option<ProjectManifest>,
    entries: &mut Vec<ListEntry>,
) {
    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[warn] error reading disabled entry: {e}");
                continue;
            }
        };
        let file_name = entry.file_name().to_string_lossy().to_string();
        let name = match resource_type {
            ResourceType::Skill => {
                if !entry.path().is_dir() {
                    continue;
                }
                file_name
            }
            ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
                if !entry.path().is_file() {
                    continue;
                }
                match file_name.strip_suffix(".md") {
                    Some(n) => n.to_string(),
                    None => continue,
                }
            }
        };
        entries.push(make_entry(name, resource_type, manifest, "disabled"));
    }
}

/// Build a `ListEntry` with its manifest version resolved.
fn make_entry(
    name: String,
    resource_type: ResourceType,
    manifest: &Option<ProjectManifest>,
    status: &str,
) -> ListEntry {
    let version = manifest_version(manifest, resource_type, &name);
    ListEntry {
        name,
        resource_type: resource_type.to_string(),
        version,
        status: status.to_string(),
    }
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

/// Load the project manifest, returning None if it doesn't exist.
///
/// Warns on parse errors to distinguish from a missing file.
fn load_manifest(project_dir: &Path) -> Option<ProjectManifest> {
    let path = project_dir.join("relava.toml");
    match ProjectManifest::from_file(&path) {
        Ok(m) => Some(m),
        Err(e) => {
            if path.exists() {
                eprintln!("[warn] failed to read relava.toml: {e}");
            }
            None
        }
    }
}

/// Print a formatted table of list entries.
fn print_table(entries: &[ListEntry]) {
    // Header
    println!("{:<24} {:<10} {:<12} STATUS", "NAME", "TYPE", "VERSION");
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
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
            _verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 3);
    }

    #[test]
    fn list_disabled_skill() {
        let root = temp_dir();

        // Active skill
        let active = root.path().join(".claude/skills/active-skill");
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("SKILL.md"), "# Active").unwrap();

        // Disabled skill (in .disabled/ subdirectory)
        let disabled = root.path().join(".claude/skills/.disabled/disabled-skill");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("SKILL.md"), "# Disabled").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 2);

        let active_entry = result
            .resources
            .iter()
            .find(|e| e.name == "active-skill")
            .unwrap();
        assert_eq!(active_entry.status, "active");

        let disabled_entry = result
            .resources
            .iter()
            .find(|e| e.name == "disabled-skill")
            .unwrap();
        assert_eq!(disabled_entry.status, "disabled");
    }

    #[test]
    fn list_disabled_agent() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        // Active
        fs::write(agents_dir.join("active-agent.md"), "# Active").unwrap();
        // Disabled (in .disabled/ subdirectory)
        let disabled_dir = agents_dir.join(".disabled");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("disabled-agent.md"), "# Disabled").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Agent),
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 2);

        let active_entry = result
            .resources
            .iter()
            .find(|e| e.name == "active-agent")
            .unwrap();
        assert_eq!(active_entry.status, "active");

        let disabled_entry = result
            .resources
            .iter()
            .find(|e| e.name == "disabled-agent")
            .unwrap();
        assert_eq!(disabled_entry.status, "disabled");
    }

    #[test]
    fn list_disabled_with_manifest_version() {
        let root = temp_dir();

        // Disabled skill (in .disabled/ subdirectory)
        let disabled = root.path().join(".claude/skills/.disabled/denden");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("SKILL.md"), "# Denden").unwrap();

        // Manifest with version
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.2.0\"\n",
        )
        .unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Skill),
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 1);
        assert_eq!(result.resources[0].name, "denden");
        assert_eq!(result.resources[0].status, "disabled");
        assert_eq!(result.resources[0].version, "1.2.0");
    }

    #[test]
    fn list_disabled_sorted_with_active() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();

        fs::write(agents_dir.join("zebra.md"), "# Zebra").unwrap();
        fs::write(agents_dir.join("middle.md"), "# Middle").unwrap();

        // Disabled agent in .disabled/ subdirectory
        let disabled_dir = agents_dir.join(".disabled");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("alpha.md"), "# Alpha disabled").unwrap();

        let opts = ListOpts {
            resource_type: Some(ResourceType::Agent),
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let result = run(&opts).unwrap();
        assert_eq!(result.resources.len(), 3);
        assert_eq!(result.resources[0].name, "alpha");
        assert_eq!(result.resources[0].status, "disabled");
        assert_eq!(result.resources[1].name, "middle");
        assert_eq!(result.resources[1].status, "active");
        assert_eq!(result.resources[2].name, "zebra");
        assert_eq!(result.resources[2].status, "active");
    }
}
