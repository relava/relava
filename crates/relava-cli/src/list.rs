use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::api_client::ApiClient;
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
    pub server_url: &'a str,
    pub resource_type: Option<ResourceType>,
    pub project_dir: &'a Path,
    pub json: bool,
    pub _verbose: bool,
}

/// Run `relava list [type]`.
///
/// Queries the registry server for available resources and cross-references
/// with the local project directory for install/disabled status. The server
/// must be running; returns an error if it is unreachable.
pub fn run(opts: &ListOpts) -> Result<ListResult, String> {
    let client = ApiClient::new(opts.server_url);

    // Query server for registry resources (server must be running)
    let type_filter = opts.resource_type.map(|rt| rt.to_string());
    let server_resources = client
        .list_resources(type_filter.as_deref())
        .map_err(|e| e.to_string())?;

    let manifest = load_manifest(opts.project_dir);

    // Build entries from server response, enriched with local install status
    let mut resources: Vec<ListEntry> = server_resources
        .into_iter()
        .map(|r| {
            let rt = ResourceType::from_str(&r.resource_type).ok();
            let status = rt
                .map(|rt| local_status(opts.project_dir, rt, &r.name))
                .unwrap_or_else(|| "registered".to_string());

            // Prefer manifest version, fall back to server latest_version
            let version = rt
                .and_then(|rt| manifest_version(&manifest, rt, &r.name))
                .or(r.latest_version)
                .unwrap_or_default();

            ListEntry {
                name: r.name,
                resource_type: r.resource_type,
                version,
                status,
            }
        })
        .collect();

    // Also include locally installed resources not yet in the registry
    let types = match opts.resource_type {
        Some(rt) => vec![rt],
        None => ResourceType::ALL.to_vec(),
    };
    for rt in &types {
        let local_entries = scan_local_only(opts.project_dir, *rt, &manifest, &resources);
        resources.extend(local_entries);
    }

    resources.sort_by(|a, b| a.resource_type.cmp(&b.resource_type).then(a.name.cmp(&b.name)));

    if !opts.json {
        if resources.is_empty() {
            let msg = match opts.resource_type {
                Some(rt) => format!(
                    "No {}s found. Run `relava install {} <name>` to get started.",
                    rt, rt
                ),
                None => {
                    "No resources found. Run `relava install <type> <name>` to get started."
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

/// Check local install status for a resource.
fn local_status(project_dir: &Path, resource_type: ResourceType, name: &str) -> String {
    let disabled_path = disable::disabled_path_for(project_dir, resource_type, name);
    if disabled_path.exists() {
        return "disabled".to_string();
    }
    if install::is_installed(project_dir, resource_type, name) {
        return "active".to_string();
    }
    "registered".to_string()
}

/// Scan local directories for resources not already in the server list.
///
/// This ensures locally installed resources that haven't been registered
/// in the server still appear in the list.
fn scan_local_only(
    project_dir: &Path,
    resource_type: ResourceType,
    manifest: &Option<ProjectManifest>,
    existing: &[ListEntry],
) -> Vec<ListEntry> {
    let type_dir = install::type_dir(project_dir, resource_type);
    if !type_dir.is_dir() {
        return Vec::new();
    }

    let rt_str = resource_type.to_string();
    let mut entries = Vec::new();

    // Active resources
    match std::fs::read_dir(&type_dir) {
        Err(e) => eprintln!("[warn] cannot read {}: {e}", type_dir.display()),
        Ok(read_dir) => for entry in read_dir.flatten() {
            let name = match extract_name(resource_type, &entry) {
                Some(n) => n,
                None => continue,
            };
            if already_listed(existing, &rt_str, &name) {
                continue;
            }
            if resource_type == ResourceType::Skill
                && !install::is_installed(project_dir, ResourceType::Skill, &name)
            {
                continue;
            }
            let version = manifest_version(manifest, resource_type, &name).unwrap_or_default();
            entries.push(ListEntry {
                name,
                resource_type: rt_str.clone(),
                version,
                status: "active".to_string(),
            });
        },
    }

    // Disabled resources
    let disabled_dir = disable::disabled_dir_for(project_dir, resource_type);
    if disabled_dir.is_dir() {
    match std::fs::read_dir(&disabled_dir) {
        Err(e) => eprintln!("[warn] cannot read {}: {e}", disabled_dir.display()),
        Ok(read_dir) => {
        for entry in read_dir.flatten() {
            let name = match extract_name(resource_type, &entry) {
                Some(n) => n,
                None => continue,
            };
            if already_listed(existing, &rt_str, &name)
                || already_listed(&entries, &rt_str, &name)
            {
                continue;
            }
            let version =
                manifest_version(manifest, resource_type, &name).unwrap_or_default();
            entries.push(ListEntry {
                name,
                resource_type: rt_str.clone(),
                version,
                status: "disabled".to_string(),
            });
        }
        },
    }
    }

    entries
}

/// Extract resource name from a directory entry.
fn extract_name(resource_type: ResourceType, entry: &std::fs::DirEntry) -> Option<String> {
    let file_name = entry.file_name().to_string_lossy().to_string();
    if file_name == ".disabled" {
        return None;
    }
    match resource_type {
        ResourceType::Skill => {
            if entry.path().is_dir() {
                Some(file_name)
            } else {
                None
            }
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            if entry.path().is_file() {
                file_name.strip_suffix(".md").map(|n| n.to_string())
            } else {
                None
            }
        }
    }
}

/// Check if a resource is already in the list.
fn already_listed(entries: &[ListEntry], resource_type: &str, name: &str) -> bool {
    entries
        .iter()
        .any(|e| e.resource_type == resource_type && e.name == name)
}

/// Look up the version for a resource in relava.toml.
fn manifest_version(
    manifest: &Option<ProjectManifest>,
    resource_type: ResourceType,
    name: &str,
) -> Option<String> {
    let m = manifest.as_ref()?;
    let section = match resource_type {
        ResourceType::Skill => &m.skills,
        ResourceType::Agent => &m.agents,
        ResourceType::Command => &m.commands,
        ResourceType::Rule => &m.rules,
    };
    let v = section.get(name).cloned().unwrap_or_default();
    if v.is_empty() { None } else { Some(v) }
}

/// Load the project manifest, returning None if it doesn't exist.
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

/// Print a formatted table of list entries using comfy-table.
fn print_table(entries: &[ListEntry]) {
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            let version = if e.version.is_empty() {
                "-".to_string()
            } else {
                e.version.clone()
            };
            vec![
                e.name.clone(),
                e.resource_type.clone(),
                version,
                e.status.clone(),
            ]
        })
        .collect();

    println!(
        "{}",
        crate::output::table(&["Name", "Type", "Version", "Status"], &rows)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    // --- Unit tests for internal helpers (no server needed) ---

    #[test]
    fn manifest_version_returns_some_when_present() {
        let mut skills = std::collections::BTreeMap::new();
        skills.insert("denden".to_string(), "1.2.0".to_string());
        let manifest = ProjectManifest {
            skills,
            ..Default::default()
        };
        assert_eq!(
            manifest_version(&Some(manifest), ResourceType::Skill, "denden"),
            Some("1.2.0".to_string())
        );
    }

    #[test]
    fn manifest_version_returns_none_when_missing() {
        let manifest = ProjectManifest::default();
        assert_eq!(
            manifest_version(&Some(manifest), ResourceType::Skill, "denden"),
            None
        );
    }

    #[test]
    fn manifest_version_returns_none_without_manifest() {
        assert_eq!(
            manifest_version(&None, ResourceType::Skill, "denden"),
            None
        );
    }

    #[test]
    fn local_status_active() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        assert_eq!(
            local_status(root.path(), ResourceType::Skill, "denden"),
            "active"
        );
    }

    #[test]
    fn local_status_disabled() {
        let root = temp_dir();
        let disabled_dir = root.path().join(".claude/skills/.disabled/denden");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("SKILL.md"), "# Denden").unwrap();

        assert_eq!(
            local_status(root.path(), ResourceType::Skill, "denden"),
            "disabled"
        );
    }

    #[test]
    fn local_status_not_installed() {
        let root = temp_dir();
        assert_eq!(
            local_status(root.path(), ResourceType::Skill, "denden"),
            "registered"
        );
    }

    #[test]
    fn already_listed_finds_match() {
        let entries = vec![ListEntry {
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            version: String::new(),
            status: "active".to_string(),
        }];
        assert!(already_listed(&entries, "skill", "denden"));
        assert!(!already_listed(&entries, "skill", "other"));
        assert!(!already_listed(&entries, "agent", "denden"));
    }

    #[test]
    fn scan_local_only_finds_unregistered() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/local-only");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Local Only").unwrap();

        let existing: Vec<ListEntry> = vec![];
        let result = scan_local_only(root.path(), ResourceType::Skill, &None, &existing);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "local-only");
        assert_eq!(result[0].status, "active");
    }

    #[test]
    fn scan_local_only_skips_already_listed() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let existing = vec![ListEntry {
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            version: String::new(),
            status: "active".to_string(),
        }];
        let result = scan_local_only(root.path(), ResourceType::Skill, &None, &existing);
        assert!(result.is_empty());
    }

    #[test]
    fn scan_local_only_finds_disabled() {
        let root = temp_dir();
        let disabled = root.path().join(".claude/agents/.disabled");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("debugger.md"), "# Debugger").unwrap();

        let result = scan_local_only(root.path(), ResourceType::Agent, &None, &[]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "debugger");
        assert_eq!(result[0].status, "disabled");
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

    // --- Integration tests: server interaction ---

    #[test]
    fn list_fails_when_server_unreachable() {
        let root = temp_dir();
        let opts = ListOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: None,
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let err = run(&opts).unwrap_err();
        assert!(
            err.contains("Registry server not running"),
            "got: {err}"
        );
    }

    #[test]
    fn list_fails_when_server_unreachable_even_with_local_resources() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = ListOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: None,
            project_dir: root.path(),
            json: true,
            _verbose: false,
        };
        let err = run(&opts).unwrap_err();
        assert!(
            err.contains("Registry server not running"),
            "got: {err}"
        );
    }
}
