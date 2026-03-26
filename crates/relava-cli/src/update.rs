use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::{self, ResourceType};
use relava_types::version::{Version, VersionConstraint};

use crate::cache::DownloadCache;
use crate::install;
use crate::registry::RegistryClient;

/// Options for the update command.
pub struct UpdateOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: Option<ResourceType>,
    pub name: Option<&'a str>,
    pub all: bool,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// A single resource update entry.
#[derive(Debug, serde::Serialize)]
pub struct UpdateEntry {
    pub resource_type: String,
    pub name: String,
    pub old_version: String,
    pub new_version: String,
    pub status: String,
}

/// Result of the update command.
#[derive(Debug, serde::Serialize)]
pub struct UpdateResult {
    pub updated: Vec<UpdateEntry>,
    pub up_to_date: Vec<UpdateEntry>,
    pub skipped: Vec<UpdateEntry>,
}

/// Run `relava update`.
///
/// In single-resource mode (`<type> <name>`), checks the registry for a
/// newer version and overwrites local files if one is found.
///
/// In `--all` mode, iterates all installed resources tracked in
/// relava.toml and updates each.
pub fn run(opts: &UpdateOpts) -> Result<UpdateResult, String> {
    let client = RegistryClient::new(opts.server_url);
    let cache = new_cache()?;
    let manifest = load_manifest(opts.project_dir);

    if opts.all {
        run_all(opts, &client, &cache, &manifest)
    } else {
        run_single(opts, &client, &cache, &manifest)
    }
}

/// Update a single resource.
fn run_single(
    opts: &UpdateOpts,
    client: &RegistryClient,
    cache: &DownloadCache,
    manifest: &Option<ProjectManifest>,
) -> Result<UpdateResult, String> {
    let resource_type = opts
        .resource_type
        .ok_or_else(|| "missing resource type. Usage: relava update <type> <name>".to_string())?;
    let name = opts
        .name
        .ok_or_else(|| "missing resource name. Usage: relava update <type> <name>".to_string())?;

    validate::validate_slug(name).map_err(|e| e.to_string())?;

    if !install::is_installed(opts.project_dir, resource_type, name) {
        return Err(format!(
            "{} '{}' is not installed. Run `relava install {} {}` first.",
            resource_type, name, resource_type, name
        ));
    }

    let entry = update_resource(opts, client, cache, manifest, resource_type, name)?;

    let mut result = UpdateResult {
        updated: Vec::new(),
        up_to_date: Vec::new(),
        skipped: Vec::new(),
    };
    classify_entry(&mut result, entry);

    Ok(result)
}

/// Update all installed resources tracked in relava.toml.
fn run_all(
    opts: &UpdateOpts,
    client: &RegistryClient,
    cache: &DownloadCache,
    manifest: &Option<ProjectManifest>,
) -> Result<UpdateResult, String> {
    let Some(manifest) = manifest else {
        return Err(
            "relava.toml not found. Cannot determine which resources to update.".to_string(),
        );
    };

    let mut result = UpdateResult {
        updated: Vec::new(),
        up_to_date: Vec::new(),
        skipped: Vec::new(),
    };

    let sections: &[(ResourceType, &std::collections::BTreeMap<String, String>)] = &[
        (ResourceType::Skill, &manifest.skills),
        (ResourceType::Agent, &manifest.agents),
        (ResourceType::Command, &manifest.commands),
        (ResourceType::Rule, &manifest.rules),
    ];

    for &(resource_type, section) in sections {
        for name in section.keys() {
            if !install::is_installed(opts.project_dir, resource_type, name) {
                if !opts.json {
                    eprintln!(
                        "[warn] {}/{} listed in relava.toml but not installed — skipping",
                        resource_type, name
                    );
                }
                continue;
            }

            match update_resource(
                opts,
                client,
                cache,
                &Some(manifest.clone()),
                resource_type,
                name,
            ) {
                Ok(entry) => classify_entry(&mut result, entry),
                Err(e) => {
                    if !opts.json {
                        eprintln!("[warn] failed to update {}/{}: {}", resource_type, name, e);
                    }
                    result.skipped.push(UpdateEntry {
                        resource_type: resource_type.to_string(),
                        name: name.clone(),
                        old_version: String::new(),
                        new_version: String::new(),
                        status: format!("error: {e}"),
                    });
                }
            }
        }
    }

    if !opts.json {
        print_summary(&result);
    }

    Ok(result)
}

/// Check and update a single resource. Returns the resulting entry.
fn update_resource(
    opts: &UpdateOpts,
    client: &RegistryClient,
    cache: &DownloadCache,
    manifest: &Option<ProjectManifest>,
    resource_type: ResourceType,
    name: &str,
) -> Result<UpdateEntry, String> {
    let old_version = manifest_version(manifest, resource_type, name);

    // Check for a pinned (exact) version constraint
    let version_pin = manifest_version_pin(manifest, resource_type, name);
    if let Some(ref pin) = version_pin {
        let constraint =
            VersionConstraint::parse(pin).map_err(|e| format!("invalid version pin: {e}"))?;
        if let VersionConstraint::Exact(ref pinned) = constraint {
            if !opts.json {
                println!(
                    "{}/{}: pinned at {} — skipping",
                    resource_type, name, pinned
                );
            }
            return Ok(UpdateEntry {
                resource_type: resource_type.to_string(),
                name: name.to_string(),
                old_version: old_version.clone(),
                new_version: old_version,
                status: "pinned".to_string(),
            });
        }
    }

    if opts.verbose {
        eprintln!("checking {}/{}...", resource_type, name);
    }

    // Resolve latest version from registry
    let latest = client
        .resolve_version(resource_type, name, None)
        .map_err(|e| e.to_string())?;

    // Compare with installed version
    let old_parsed = if old_version.is_empty() {
        None
    } else {
        Version::parse(&old_version).ok()
    };

    if old_parsed.as_ref() == Some(&latest) {
        if !opts.json {
            println!(
                "{}/{}: already up to date ({})",
                resource_type, name, latest
            );
        }
        return Ok(UpdateEntry {
            resource_type: resource_type.to_string(),
            name: name.to_string(),
            old_version: old_version.clone(),
            new_version: latest.to_string(),
            status: "up_to_date".to_string(),
        });
    }

    // Download and install newer version
    if !opts.json {
        if old_version.is_empty() {
            println!(
                "{}/{}: installing {} (no previous version tracked)",
                resource_type, name, latest
            );
        } else {
            println!(
                "{}/{}: {} \u{2192} {}",
                resource_type, name, old_version, latest
            );
        }
    }

    // Download to cache
    if !cache.is_cached(resource_type, name, &latest) {
        if opts.verbose {
            eprintln!("  downloading from {}", opts.server_url);
        }
        let response = client
            .download(resource_type, name, &latest)
            .map_err(|e| e.to_string())?;
        cache
            .store(resource_type, name, &latest, &response)
            .map_err(|e| e.to_string())?;
    }

    // Write files to project (overwrites existing)
    install::write_to_project_public(opts.project_dir, resource_type, name, &latest, cache)
        .map_err(|e| e.to_string())?;

    // Update relava.toml with new version
    crate::save::add_to_manifest(
        opts.project_dir,
        resource_type,
        name,
        &latest.to_string(),
        opts.json,
    )?;

    Ok(UpdateEntry {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        old_version,
        new_version: latest.to_string(),
        status: "updated".to_string(),
    })
}

/// Place an entry into the appropriate result bucket.
fn classify_entry(result: &mut UpdateResult, entry: UpdateEntry) {
    match entry.status.as_str() {
        "updated" => result.updated.push(entry),
        "up_to_date" => result.up_to_date.push(entry),
        _ => result.skipped.push(entry),
    }
}

/// Print a summary for `--all` mode.
fn print_summary(result: &UpdateResult) {
    let updated = result.updated.len();
    let up_to_date = result.up_to_date.len();
    let skipped = result.skipped.len();
    let total = updated + up_to_date + skipped;

    if total == 0 {
        println!("No resources to update.");
        return;
    }

    println!();
    let mut parts = Vec::new();
    if updated > 0 {
        parts.push(format!("{updated} updated"));
    }
    if up_to_date > 0 {
        parts.push(format!("{up_to_date} already up to date"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} skipped"));
    }
    println!("{} resource(s): {}", total, parts.join(", "));
}

/// Get the version string for a resource from relava.toml.
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

/// Get the raw version pin string from relava.toml (for constraint checking).
fn manifest_version_pin(
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
    section.get(name).cloned()
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

/// Create a download cache at ~/.relava/cache/.
fn new_cache() -> Result<DownloadCache, String> {
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| "cannot determine home directory for cache".to_string())?
        .join(".relava")
        .join("cache");
    Ok(DownloadCache::new(cache_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    // --- UpdateResult classification ---

    #[test]
    fn classify_entry_updated() {
        let mut result = UpdateResult {
            updated: Vec::new(),
            up_to_date: Vec::new(),
            skipped: Vec::new(),
        };
        let entry = UpdateEntry {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            old_version: "1.0.0".to_string(),
            new_version: "1.1.0".to_string(),
            status: "updated".to_string(),
        };
        classify_entry(&mut result, entry);
        assert_eq!(result.updated.len(), 1);
        assert_eq!(result.up_to_date.len(), 0);
        assert_eq!(result.skipped.len(), 0);
    }

    #[test]
    fn classify_entry_up_to_date() {
        let mut result = UpdateResult {
            updated: Vec::new(),
            up_to_date: Vec::new(),
            skipped: Vec::new(),
        };
        let entry = UpdateEntry {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            old_version: "1.0.0".to_string(),
            new_version: "1.0.0".to_string(),
            status: "up_to_date".to_string(),
        };
        classify_entry(&mut result, entry);
        assert_eq!(result.updated.len(), 0);
        assert_eq!(result.up_to_date.len(), 1);
    }

    #[test]
    fn classify_entry_pinned() {
        let mut result = UpdateResult {
            updated: Vec::new(),
            up_to_date: Vec::new(),
            skipped: Vec::new(),
        };
        let entry = UpdateEntry {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            old_version: "1.0.0".to_string(),
            new_version: "1.0.0".to_string(),
            status: "pinned".to_string(),
        };
        classify_entry(&mut result, entry);
        assert_eq!(result.skipped.len(), 1);
    }

    // --- manifest_version ---

    #[test]
    fn manifest_version_returns_version() {
        let manifest = ProjectManifest {
            agent_type: None,
            skills: [("denden".to_string(), "1.2.0".to_string())]
                .into_iter()
                .collect(),
            agents: Default::default(),
            commands: Default::default(),
            rules: Default::default(),
        };
        assert_eq!(
            manifest_version(&Some(manifest), ResourceType::Skill, "denden"),
            "1.2.0"
        );
    }

    #[test]
    fn manifest_version_missing_returns_empty() {
        let manifest = ProjectManifest {
            agent_type: None,
            skills: Default::default(),
            agents: Default::default(),
            commands: Default::default(),
            rules: Default::default(),
        };
        assert_eq!(
            manifest_version(&Some(manifest), ResourceType::Skill, "denden"),
            ""
        );
    }

    #[test]
    fn manifest_version_no_manifest_returns_empty() {
        assert_eq!(manifest_version(&None, ResourceType::Skill, "denden"), "");
    }

    // --- manifest_version_pin ---

    #[test]
    fn manifest_version_pin_exact() {
        let manifest = ProjectManifest {
            agent_type: None,
            skills: [("denden".to_string(), "1.2.0".to_string())]
                .into_iter()
                .collect(),
            agents: Default::default(),
            commands: Default::default(),
            rules: Default::default(),
        };
        assert_eq!(
            manifest_version_pin(&Some(manifest), ResourceType::Skill, "denden"),
            Some("1.2.0".to_string())
        );
    }

    #[test]
    fn manifest_version_pin_wildcard() {
        let manifest = ProjectManifest {
            agent_type: None,
            skills: [("denden".to_string(), "*".to_string())]
                .into_iter()
                .collect(),
            agents: Default::default(),
            commands: Default::default(),
            rules: Default::default(),
        };
        assert_eq!(
            manifest_version_pin(&Some(manifest), ResourceType::Skill, "denden"),
            Some("*".to_string())
        );
    }

    #[test]
    fn manifest_version_pin_none() {
        assert_eq!(
            manifest_version_pin(&None, ResourceType::Skill, "denden"),
            None
        );
    }

    // --- load_manifest ---

    #[test]
    fn load_manifest_missing_file() {
        let root = temp_dir();
        assert!(load_manifest(root.path()).is_none());
    }

    #[test]
    fn load_manifest_valid() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();
        let m = load_manifest(root.path()).unwrap();
        assert_eq!(m.skills["denden"], "1.0.0");
    }

    #[test]
    fn load_manifest_invalid_warns() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "not valid toml {{{{").unwrap();
        assert!(load_manifest(root.path()).is_none());
    }

    // --- run_single validation ---

    #[test]
    fn single_missing_type_returns_error() {
        let root = temp_dir();
        let opts = UpdateOpts {
            server_url: "http://localhost:7420",
            resource_type: None,
            name: None,
            all: false,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing resource type"));
    }

    #[test]
    fn single_missing_name_returns_error() {
        let root = temp_dir();
        let opts = UpdateOpts {
            server_url: "http://localhost:7420",
            resource_type: Some(ResourceType::Skill),
            name: None,
            all: false,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing resource name"));
    }

    #[test]
    fn single_invalid_slug_returns_error() {
        let root = temp_dir();
        let opts = UpdateOpts {
            server_url: "http://localhost:7420",
            resource_type: Some(ResourceType::Skill),
            name: Some("INVALID_SLUG"),
            all: false,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn single_not_installed_returns_error() {
        let root = temp_dir();
        let opts = UpdateOpts {
            server_url: "http://localhost:7420",
            resource_type: Some(ResourceType::Skill),
            name: Some("nonexistent"),
            all: false,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not installed"));
    }

    // --- run_all validation ---

    #[test]
    fn all_without_manifest_returns_error() {
        let root = temp_dir();
        let opts = UpdateOpts {
            server_url: "http://localhost:7420",
            resource_type: None,
            name: None,
            all: true,
            project_dir: root.path(),
            json: false,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("relava.toml not found"));
    }

    // --- UpdateResult serialization ---

    #[test]
    fn update_result_serializes_to_json() {
        let result = UpdateResult {
            updated: vec![UpdateEntry {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "1.1.0".to_string(),
                status: "updated".to_string(),
            }],
            up_to_date: vec![UpdateEntry {
                resource_type: "agent".to_string(),
                name: "debugger".to_string(),
                old_version: "0.5.0".to_string(),
                new_version: "0.5.0".to_string(),
                status: "up_to_date".to_string(),
            }],
            skipped: Vec::new(),
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("1.1.0"));
        assert!(json.contains("debugger"));
        assert!(json.contains("up_to_date"));
    }

    #[test]
    fn update_entry_contains_version_transition() {
        let entry = UpdateEntry {
            resource_type: "skill".to_string(),
            name: "code-review".to_string(),
            old_version: "1.0.0".to_string(),
            new_version: "1.1.0".to_string(),
            status: "updated".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("1.1.0"));
    }

    // --- print_summary (no assertions, just ensure no panic) ---

    #[test]
    fn print_summary_empty() {
        let result = UpdateResult {
            updated: Vec::new(),
            up_to_date: Vec::new(),
            skipped: Vec::new(),
        };
        print_summary(&result);
    }

    #[test]
    fn print_summary_mixed() {
        let result = UpdateResult {
            updated: vec![UpdateEntry {
                resource_type: "skill".to_string(),
                name: "a".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "2.0.0".to_string(),
                status: "updated".to_string(),
            }],
            up_to_date: vec![UpdateEntry {
                resource_type: "skill".to_string(),
                name: "b".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "1.0.0".to_string(),
                status: "up_to_date".to_string(),
            }],
            skipped: vec![UpdateEntry {
                resource_type: "agent".to_string(),
                name: "c".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "1.0.0".to_string(),
                status: "pinned".to_string(),
            }],
        };
        print_summary(&result);
    }
}
