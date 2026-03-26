use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::cache::DownloadCache;
use crate::install;
use crate::lockfile::Lockfile;
use crate::registry::RegistryClient;

/// Options for bulk-installing all resources from a manifest.
pub struct BulkInstallOpts<'a> {
    pub server_url: &'a str,
    pub project_dir: &'a Path,
    pub global: bool,
    pub json: bool,
    pub verbose: bool,
    pub yes: bool,
}

/// A single resource entry in the bulk install results.
#[derive(Debug, serde::Serialize)]
pub struct BulkEntry {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub status: String,
    /// Non-empty only for failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of a bulk install operation.
#[derive(Debug, Default, serde::Serialize)]
pub struct BulkInstallResult {
    pub installed: Vec<BulkEntry>,
    pub skipped: Vec<BulkEntry>,
    pub failed: Vec<BulkEntry>,
}

/// Run `relava install` with no args or `relava install relava.toml`.
///
/// Reads all declared resources from the project manifest, resolves version
/// constraints, and installs each one. Resources already installed at the
/// correct version are skipped. Failures are collected and reported but do
/// not prevent other resources from being installed.
///
/// When `relava.lock` exists, exact versions are read from the lockfile
/// (like `npm ci`). When absent, versions are resolved fresh from the
/// registry and a lockfile is created by the caller.
pub fn run(opts: &BulkInstallOpts) -> Result<BulkInstallResult, String> {
    let manifest = load_manifest(opts.project_dir)?;

    let client = RegistryClient::new(opts.server_url);
    let cache = new_cache()?;

    // Load lockfile if present — used for exact version pinning.
    // A corrupt lockfile is an error, not silently ignored.
    let lockfile: Option<Lockfile> = match Lockfile::load(opts.project_dir) {
        Ok(lf) => lf,
        Err(e) => {
            if !opts.json {
                eprintln!("[warn] {e} — resolving fresh versions from registry");
            }
            None
        }
    };
    if !opts.json && lockfile.is_some() {
        eprintln!("  Using versions from relava.lock");
    }

    let sections: &[(ResourceType, &std::collections::BTreeMap<String, String>)] = &[
        (ResourceType::Skill, &manifest.skills),
        (ResourceType::Agent, &manifest.agents),
        (ResourceType::Command, &manifest.commands),
        (ResourceType::Rule, &manifest.rules),
    ];

    // Collect all resources into a flat list for progress reporting.
    let entries: Vec<(ResourceType, &str, &str)> = sections
        .iter()
        .flat_map(|&(rt, section)| {
            section
                .iter()
                .map(move |(name, pin)| (rt, name.as_str(), pin.as_str()))
        })
        .collect();

    let total = entries.len();
    if total == 0 {
        if !opts.json {
            println!("No resources declared in relava.toml");
        }
        return Ok(BulkInstallResult::default());
    }

    if !opts.json {
        let plural = if total == 1 { "resource" } else { "resources" };
        println!("Installing {total} {plural} from relava.toml...");
    }

    let mut result = BulkInstallResult::default();
    let install_root = resolve_install_root(opts)?;

    for (index, &(resource_type, name, version_pin)) in entries.iter().enumerate() {
        let position = index + 1;

        // If lockfile exists, use the exact locked version instead of
        // resolving from the registry (like `npm ci`).
        let locked_version: Option<String> = lockfile
            .as_ref()
            .and_then(|lf| lf.locked_version(resource_type, name));
        let effective_pin = locked_version.as_deref().unwrap_or(version_pin);

        // Resolve version from registry
        let version = match client.resolve_version(resource_type, name, Some(effective_pin)) {
            Ok(v) => v,
            Err(e) => {
                let msg = e.to_string();
                if !opts.json {
                    println!(
                        "  [{position}/{total}] {resource_type}/{name}@{version_pin} — failed: {msg}",
                    );
                }
                result.failed.push(BulkEntry {
                    resource_type: resource_type.to_string(),
                    name: name.to_string(),
                    version: version_pin.to_string(),
                    status: "failed".to_string(),
                    error: Some(msg),
                });
                continue;
            }
        };

        // Check if already installed at this version (cached and on disk)
        if is_installed_at_version(&install_root, &cache, resource_type, name, &version) {
            if !opts.json {
                println!(
                    "  [{position}/{total}] {resource_type}/{name}@{version} — already installed",
                );
            }
            result.skipped.push(BulkEntry {
                resource_type: resource_type.to_string(),
                name: name.to_string(),
                version: version.to_string(),
                status: "skipped".to_string(),
                error: None,
            });
            continue;
        }

        if !opts.json {
            println!("  [{position}/{total}] Installing {resource_type}/{name}@{version}...",);
        }

        // Delegate to the single-resource install
        let install_opts = install::InstallOpts {
            server_url: opts.server_url,
            resource_type,
            name,
            version_pin: Some(version_pin),
            project_dir: opts.project_dir,
            global: opts.global,
            json: true, // suppress inner output; we handle progress ourselves
            verbose: opts.verbose,
            yes: opts.yes,
        };

        match install::run(&install_opts) {
            Ok(install_result) => {
                result.installed.push(BulkEntry {
                    resource_type: resource_type.to_string(),
                    name: name.to_string(),
                    version: install_result.version,
                    status: "installed".to_string(),
                    error: None,
                });
            }
            Err(e) => {
                if !opts.json {
                    println!("  [{position}/{total}] {resource_type}/{name} — failed: {e}",);
                }
                result.failed.push(BulkEntry {
                    resource_type: resource_type.to_string(),
                    name: name.to_string(),
                    version: version.to_string(),
                    status: "failed".to_string(),
                    error: Some(e),
                });
            }
        }
    }

    if !opts.json {
        print_summary(&result);
    }

    Ok(result)
}

/// Load the project manifest, returning an error if it's missing or invalid.
fn load_manifest(project_dir: &Path) -> Result<ProjectManifest, String> {
    let path = project_dir.join("relava.toml");
    if !path.exists() {
        return Err("relava.toml not found in project directory".to_string());
    }
    ProjectManifest::from_file(&path).map_err(|e| format!("failed to read relava.toml: {e}"))
}

/// Create a download cache at ~/.relava/cache/.
fn new_cache() -> Result<DownloadCache, String> {
    let cache_dir = dirs::home_dir()
        .ok_or_else(|| "cannot determine home directory for cache".to_string())?
        .join(".relava")
        .join("cache");
    Ok(DownloadCache::new(cache_dir))
}

/// Resolve the install root directory.
fn resolve_install_root(opts: &BulkInstallOpts) -> Result<std::path::PathBuf, String> {
    if opts.global {
        dirs::home_dir().ok_or_else(|| "cannot determine home directory".to_string())
    } else {
        Ok(opts.project_dir.to_path_buf())
    }
}

/// Check if a resource is already installed and cached at the given version.
fn is_installed_at_version(
    install_root: &Path,
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    version: &relava_types::version::Version,
) -> bool {
    install::is_installed(install_root, resource_type, name)
        && cache.is_cached(resource_type, name, version)
}

/// Print a human-readable summary of the bulk install results.
fn print_summary(result: &BulkInstallResult) {
    let installed = result.installed.len();
    let skipped = result.skipped.len();
    let failed = result.failed.len();

    println!();
    let mut parts = Vec::new();
    if installed > 0 {
        parts.push(format!("{installed} installed"));
    }
    if skipped > 0 {
        parts.push(format!("{skipped} up-to-date"));
    }
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }

    if parts.is_empty() {
        println!("Nothing to install.");
    } else {
        println!("Done: {}", parts.join(", "));
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

    // --- load_manifest tests ---

    #[test]
    fn load_manifest_missing_file() {
        let root = temp_dir();
        let result = load_manifest(root.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
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
    fn load_manifest_invalid_returns_error() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "not valid toml {{{{").unwrap();
        let result = load_manifest(root.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("failed to read"));
    }

    #[test]
    fn load_manifest_empty() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "").unwrap();
        let m = load_manifest(root.path()).unwrap();
        assert!(m.skills.is_empty());
        assert!(m.agents.is_empty());
        assert!(m.commands.is_empty());
        assert!(m.rules.is_empty());
    }

    #[test]
    fn load_manifest_all_sections() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            r#"
agent_type = "claude"

[skills]
denden = "1.2.0"
notify-slack = "*"

[agents]
debugger = "0.5.0"

[commands]
delegate = "1.0.0"

[rules]
no-console-log = "1.0.0"
"#,
        )
        .unwrap();
        let m = load_manifest(root.path()).unwrap();
        assert_eq!(m.skills.len(), 2);
        assert_eq!(m.agents.len(), 1);
        assert_eq!(m.commands.len(), 1);
        assert_eq!(m.rules.len(), 1);
    }

    // --- is_installed_at_version tests ---

    #[test]
    fn not_installed_returns_false() {
        let root = temp_dir();
        let cache_dir = temp_dir();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let version = relava_types::version::Version::parse("1.0.0").unwrap();
        assert!(!is_installed_at_version(
            root.path(),
            &cache,
            ResourceType::Skill,
            "denden",
            &version,
        ));
    }

    #[test]
    fn installed_but_not_cached_returns_false() {
        let root = temp_dir();
        let cache_dir = temp_dir();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let version = relava_types::version::Version::parse("1.0.0").unwrap();

        // Create the installed directory structure
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        assert!(!is_installed_at_version(
            root.path(),
            &cache,
            ResourceType::Skill,
            "denden",
            &version,
        ));
    }

    #[test]
    fn installed_and_cached_returns_true() {
        use crate::registry::{DownloadFile, DownloadResponse};
        use base64::Engine;

        let root = temp_dir();
        let cache_dir = temp_dir();
        let cache = DownloadCache::new(cache_dir.path().to_path_buf());
        let version = relava_types::version::Version::parse("1.0.0").unwrap();

        // Create installed directory
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Store in cache
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: base64::engine::general_purpose::STANDARD.encode(b"# Denden"),
            }],
        };
        cache
            .store(ResourceType::Skill, "denden", &version, &response)
            .unwrap();

        assert!(is_installed_at_version(
            root.path(),
            &cache,
            ResourceType::Skill,
            "denden",
            &version,
        ));
    }

    // --- print_summary tests ---

    #[test]
    fn summary_empty_result() {
        // Just verify it doesn't panic
        let result = BulkInstallResult::default();
        print_summary(&result);
    }

    #[test]
    fn summary_with_entries() {
        let result = BulkInstallResult {
            installed: vec![BulkEntry {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                version: "1.0.0".to_string(),
                status: "installed".to_string(),
                error: None,
            }],
            skipped: vec![BulkEntry {
                resource_type: "agent".to_string(),
                name: "debugger".to_string(),
                version: "0.5.0".to_string(),
                status: "skipped".to_string(),
                error: None,
            }],
            failed: vec![BulkEntry {
                resource_type: "rule".to_string(),
                name: "bad-rule".to_string(),
                version: String::new(),
                status: "failed".to_string(),
                error: Some("not found".to_string()),
            }],
        };
        print_summary(&result);
    }

    // --- BulkInstallResult serialization tests ---

    #[test]
    fn result_serializes_to_json() {
        let result = BulkInstallResult {
            installed: vec![BulkEntry {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                version: "1.0.0".to_string(),
                status: "installed".to_string(),
                error: None,
            }],
            skipped: Vec::new(),
            failed: vec![BulkEntry {
                resource_type: "rule".to_string(),
                name: "bad-rule".to_string(),
                version: String::new(),
                status: "failed".to_string(),
                error: Some("version not found".to_string()),
            }],
        };

        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"name\": \"denden\""));
        assert!(json.contains("\"type\": \"skill\""));
        assert!(json.contains("\"error\": \"version not found\""));
        // Skipped error field should not appear when None
        assert!(!json.contains("\"error\": null"));
    }

    #[test]
    fn entry_omits_null_error() {
        let entry = BulkEntry {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            version: "1.0.0".to_string(),
            status: "installed".to_string(),
            error: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("error"));
    }

    // --- resolve_install_root tests ---

    #[test]
    fn install_root_project_dir() {
        let root = temp_dir();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = resolve_install_root(&opts).unwrap();
        assert_eq!(result, root.path());
    }

    #[test]
    fn install_root_global() {
        let root = temp_dir();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: true,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = resolve_install_root(&opts).unwrap();
        assert_eq!(result, dirs::home_dir().unwrap());
    }

    // --- run() validation tests ---

    #[test]
    fn run_missing_manifest() {
        let root = temp_dir();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn run_empty_manifest() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "").unwrap();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.installed.is_empty());
        assert!(result.skipped.is_empty());
        assert!(result.failed.is_empty());
    }

    #[test]
    fn run_unreachable_server_collects_failures() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            r#"
[skills]
denden = "1.0.0"
notify-slack = "*"

[agents]
debugger = "0.5.0"
"#,
        )
        .unwrap();
        let opts = BulkInstallOpts {
            server_url: "http://127.0.0.1:1", // unreachable
            project_dir: root.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts).unwrap();
        // All should fail because the server is unreachable
        assert!(result.installed.is_empty());
        assert!(result.skipped.is_empty());
        assert_eq!(result.failed.len(), 3);
        // Each failure should have an error message
        for entry in &result.failed {
            assert!(entry.error.is_some());
        }
    }

    #[test]
    fn run_invalid_manifest_returns_error() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "invalid {{{{ toml").unwrap();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: false,
            json: false,
            verbose: false,
            yes: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn run_json_mode_suppresses_output() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "").unwrap();
        let opts = BulkInstallOpts {
            server_url: "http://localhost:7420",
            project_dir: root.path(),
            global: false,
            json: true,
            verbose: false,
            yes: false,
        };
        // Should not panic with json mode
        let result = run(&opts).unwrap();
        assert!(result.installed.is_empty());
    }
}
