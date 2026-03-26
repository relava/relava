use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

use crate::install;
use crate::registry::RegistryClient;

/// Options for the doctor command.
pub struct DoctorOpts<'a> {
    pub server_url: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub _verbose: bool,
}

/// A single diagnostic check result.
#[derive(Debug, serde::Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

/// Status of a diagnostic check.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn label(&self) -> &str {
        match self {
            Self::Pass => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

/// Result of the doctor command.
#[derive(Debug, serde::Serialize)]
pub struct DoctorResult {
    pub checks: Vec<CheckResult>,
    pub passed: usize,
    pub warnings: usize,
    pub failures: usize,
}

impl DoctorResult {
    /// Whether all checks passed (no failures).
    pub fn is_healthy(&self) -> bool {
        self.failures == 0
    }
}

/// Run `relava doctor`.
///
/// Performs a series of health checks and returns structured results.
/// The caller decides exit codes and output formatting.
pub fn run(opts: &DoctorOpts) -> DoctorResult {
    let mut checks = Checks::new(opts.json);

    if !opts.json {
        println!("Checking Relava installation...");
    }

    check_registry(&mut checks, opts);
    let manifest = check_manifest(&mut checks, opts);
    check_installed_files(&mut checks, opts);

    if let Some(ref m) = manifest {
        check_cross_reference(&mut checks, opts, m);
    }

    let (passed, warnings, failures) = checks.counts();

    if !opts.json {
        println!();
        println!("{passed} passed, {warnings} warning(s), {failures} failure(s)");
    }

    DoctorResult {
        checks: checks.into_inner(),
        passed,
        warnings,
        failures,
    }
}

/// Accumulator for check results. Handles printing and storage.
struct Checks {
    inner: Vec<CheckResult>,
    json: bool,
}

impl Checks {
    fn new(json: bool) -> Self {
        Self { inner: Vec::new(), json }
    }

    fn push(&mut self, result: CheckResult) {
        if !self.json {
            println!("  [{:<4}]  {}", result.status.label(), result.message);
        }
        self.inner.push(result);
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut passed = 0;
        let mut warnings = 0;
        let mut failures = 0;
        for c in &self.inner {
            match c.status {
                CheckStatus::Pass => passed += 1,
                CheckStatus::Warn => warnings += 1,
                CheckStatus::Fail => failures += 1,
            }
        }
        (passed, warnings, failures)
    }

    fn into_inner(self) -> Vec<CheckResult> {
        self.inner
    }
}

/// Scan results from reading a type directory.
struct ScanResult {
    names: Vec<String>,
    errors: Vec<String>,
}

/// List resource names found on disk for a given type directory.
///
/// For skills, returns directory names. For agents/commands/rules, returns
/// `.md` filenames with the extension stripped. Per-entry I/O errors are
/// collected in `errors` rather than silently dropped.
fn installed_names_on_disk(
    type_dir: &Path,
    resource_type: ResourceType,
) -> Result<ScanResult, std::io::Error> {
    let read_dir = std::fs::read_dir(type_dir)?;

    let mut names = Vec::new();
    let mut errors = Vec::new();

    for item in read_dir {
        let entry = match item {
            Ok(e) => e,
            Err(e) => {
                errors.push(format!("{}: {e}", type_dir.display()));
                continue;
            }
        };

        let path = entry.path();
        match resource_type {
            ResourceType::Skill if path.is_dir() => {
                names.push(entry.file_name().to_string_lossy().to_string());
            }
            ResourceType::Agent | ResourceType::Command | ResourceType::Rule
                if path.is_file() =>
            {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if let Some(name) = file_name.strip_suffix(".md") {
                    names.push(name.to_string());
                }
            }
            _ => {}
        }
    }

    Ok(ScanResult { names, errors })
}

/// Check if the registry server is reachable.
fn check_registry(checks: &mut Checks, opts: &DoctorOpts) {
    let client = RegistryClient::new(opts.server_url);
    let (status, message) = match client.health_check() {
        Ok(()) => (
            CheckStatus::Pass,
            format!("Server reachable at {}", opts.server_url),
        ),
        Err(e) => (
            CheckStatus::Fail,
            format!("Server health check failed at {}: {e}", opts.server_url),
        ),
    };
    checks.push(CheckResult { name: "registry".into(), status, message });
}

/// Validate relava.toml syntax and return the parsed manifest (if valid).
fn check_manifest(checks: &mut Checks, opts: &DoctorOpts) -> Option<ProjectManifest> {
    let toml_path = opts.project_dir.join("relava.toml");

    if !toml_path.exists() {
        checks.push(CheckResult {
            name: "manifest".into(),
            status: CheckStatus::Warn,
            message: "relava.toml not found — skipping manifest checks".into(),
        });
        return None;
    }

    match ProjectManifest::from_file(&toml_path) {
        Ok(manifest) => {
            checks.push(CheckResult {
                name: "manifest".into(),
                status: CheckStatus::Pass,
                message: "relava.toml is valid".into(),
            });
            Some(manifest)
        }
        Err(e) => {
            checks.push(CheckResult {
                name: "manifest".into(),
                status: CheckStatus::Fail,
                message: format!("relava.toml has errors: {e}"),
            });
            None
        }
    }
}

/// Verify that installed resource files exist at expected paths.
fn check_installed_files(checks: &mut Checks, opts: &DoctorOpts) {
    let mut total = 0u32;
    let mut issues: Vec<String> = Vec::new();

    for rt in ResourceType::ALL {
        let type_dir = install::type_dir(opts.project_dir, rt);
        if !type_dir.is_dir() {
            continue;
        }

        let scan = match installed_names_on_disk(&type_dir, rt) {
            Ok(s) => s,
            Err(e) => {
                issues.push(format!("{rt}: cannot read directory: {e}"));
                continue;
            }
        };

        for err in &scan.errors {
            issues.push(format!("{rt}: {err}"));
        }

        for name in &scan.names {
            total += 1;
            // Skills must contain a SKILL.md file; other types are single .md files
            // that exist by definition (we found them on disk).
            if rt == ResourceType::Skill && !type_dir.join(name).join("SKILL.md").exists() {
                issues.push(format!("{rt}/{name} (missing SKILL.md)"));
            }
        }
    }

    let (status, message) = if issues.is_empty() {
        (
            CheckStatus::Pass,
            format!("All {total} installed resource(s) present on disk"),
        )
    } else {
        (
            CheckStatus::Fail,
            format!(
                "{} of {} resource(s) have issues: {}",
                issues.len(),
                total,
                issues.join(", ")
            ),
        )
    };

    checks.push(CheckResult { name: "installed_files".into(), status, message });
}

/// Cross-reference relava.toml entries against actually installed files.
///
/// Reports:
/// - **missing**: listed in relava.toml but not installed on disk
/// - **orphaned**: installed on disk but not listed in relava.toml
fn check_cross_reference(
    checks: &mut Checks,
    opts: &DoctorOpts,
    manifest: &ProjectManifest,
) {
    let mut missing_on_disk: Vec<String> = Vec::new();
    let mut orphaned: Vec<String> = Vec::new();
    let mut scan_warnings: Vec<String> = Vec::new();

    for rt in ResourceType::ALL {
        let section = manifest_section(manifest, rt);

        // Manifest entries not installed on disk
        for name in section.keys() {
            if !install::is_installed(opts.project_dir, rt, name) {
                missing_on_disk.push(format!("{rt}/{name}"));
            }
        }

        // Installed resources not tracked in manifest
        let type_dir = install::type_dir(opts.project_dir, rt);
        match installed_names_on_disk(&type_dir, rt) {
            Ok(scan) => {
                for name in scan.names {
                    if install::is_installed(opts.project_dir, rt, &name)
                        && !section.contains_key(&name)
                    {
                        orphaned.push(format!("{rt}/{name}"));
                    }
                }
            }
            Err(e) => {
                // Directory doesn't exist is normal; other errors are warnings
                if type_dir.exists() {
                    scan_warnings.push(format!(
                        "Cannot read {} directory: {e}",
                        type_dir.display()
                    ));
                }
            }
        }
    }

    // Emit scan warnings if any directories were unreadable
    for warning in &scan_warnings {
        checks.push(CheckResult {
            name: "cross_reference_scan".into(),
            status: CheckStatus::Warn,
            message: warning.clone(),
        });
    }

    // Missing on disk
    let (status, message) = if missing_on_disk.is_empty() {
        (CheckStatus::Pass, "All manifest entries are installed".into())
    } else {
        (
            CheckStatus::Fail,
            format!(
                "{} manifest entry(ies) not installed: {}",
                missing_on_disk.len(),
                missing_on_disk.join(", ")
            ),
        )
    };
    checks.push(CheckResult { name: "manifest_sync".into(), status, message });

    // Orphaned
    let (status, message) = if orphaned.is_empty() {
        (
            CheckStatus::Pass,
            "No orphaned resources (all installed resources tracked in manifest)".into(),
        )
    } else {
        (
            CheckStatus::Warn,
            format!(
                "{} installed resource(s) not in relava.toml: {}",
                orphaned.len(),
                orphaned.join(", ")
            ),
        )
    };
    checks.push(CheckResult { name: "orphaned_resources".into(), status, message });
}

/// Get a read-only reference to the manifest section for a resource type.
fn manifest_section(
    manifest: &ProjectManifest,
    resource_type: ResourceType,
) -> &std::collections::BTreeMap<String, String> {
    match resource_type {
        ResourceType::Skill => &manifest.skills,
        ResourceType::Agent => &manifest.agents,
        ResourceType::Command => &manifest.commands,
        ResourceType::Rule => &manifest.rules,
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

    fn test_opts(root: &Path) -> DoctorOpts<'_> {
        DoctorOpts {
            server_url: "http://localhost:99999",
            project_dir: root,
            json: true,
            _verbose: false,
        }
    }

    // -----------------------------------------------------------------------
    // Registry connectivity
    // -----------------------------------------------------------------------

    #[test]
    fn registry_unreachable_fails() {
        let root = temp_dir();
        let opts = test_opts(root.path());
        let result = run(&opts);

        let registry_check = result.checks.iter().find(|c| c.name == "registry").unwrap();
        assert_eq!(registry_check.status, CheckStatus::Fail);
        assert!(registry_check.message.contains("health check failed"));
    }

    // -----------------------------------------------------------------------
    // Manifest validation
    // -----------------------------------------------------------------------

    #[test]
    fn manifest_missing_warns() {
        let root = temp_dir();
        let opts = test_opts(root.path());
        let result = run(&opts);

        let manifest_check = result.checks.iter().find(|c| c.name == "manifest").unwrap();
        assert_eq!(manifest_check.status, CheckStatus::Warn);
        assert!(manifest_check.message.contains("not found"));
    }

    #[test]
    fn manifest_valid_passes() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let manifest_check = result.checks.iter().find(|c| c.name == "manifest").unwrap();
        assert_eq!(manifest_check.status, CheckStatus::Pass);
        assert!(manifest_check.message.contains("valid"));
    }

    #[test]
    fn manifest_invalid_fails() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[[[invalid toml syntax",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let manifest_check = result.checks.iter().find(|c| c.name == "manifest").unwrap();
        assert_eq!(manifest_check.status, CheckStatus::Fail);
        assert!(manifest_check.message.contains("errors"));
    }

    // -----------------------------------------------------------------------
    // File integrity
    // -----------------------------------------------------------------------

    #[test]
    fn installed_files_all_present() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("1 installed"));
    }

    #[test]
    fn installed_files_missing_skill_md() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/broken");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("random.txt"), "not a skill").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.message.contains("missing SKILL.md"));
    }

    #[test]
    fn installed_files_empty_project() {
        let root = temp_dir();
        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("0 installed"));
    }

    #[test]
    fn installed_files_agents_counted() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();
        fs::write(agents_dir.join("planner.md"), "# Planner").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("2 installed"));
    }

    // -----------------------------------------------------------------------
    // Cross-reference checks
    // -----------------------------------------------------------------------

    #[test]
    fn cross_ref_all_in_sync() {
        let root = temp_dir();

        // Install a skill
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Manifest matches
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let sync_check = result.checks.iter().find(|c| c.name == "manifest_sync").unwrap();
        assert_eq!(sync_check.status, CheckStatus::Pass);

        let orphan_check = result
            .checks
            .iter()
            .find(|c| c.name == "orphaned_resources")
            .unwrap();
        assert_eq!(orphan_check.status, CheckStatus::Pass);
    }

    #[test]
    fn cross_ref_missing_on_disk() {
        let root = temp_dir();

        // Manifest says skill exists, but no files on disk
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\nmissing-skill = \"1.0.0\"\n",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let sync_check = result.checks.iter().find(|c| c.name == "manifest_sync").unwrap();
        assert_eq!(sync_check.status, CheckStatus::Fail);
        assert!(sync_check.message.contains("missing-skill"));
    }

    #[test]
    fn cross_ref_orphaned_resource() {
        let root = temp_dir();

        // Install a skill not in manifest
        let skill_dir = root.path().join(".claude/skills/orphan");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Orphan").unwrap();

        // Empty manifest
        fs::write(root.path().join("relava.toml"), "").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let orphan_check = result
            .checks
            .iter()
            .find(|c| c.name == "orphaned_resources")
            .unwrap();
        assert_eq!(orphan_check.status, CheckStatus::Warn);
        assert!(orphan_check.message.contains("orphan"));
    }

    #[test]
    fn cross_ref_multiple_types() {
        let root = temp_dir();

        // Install agent on disk but not in manifest
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        // Manifest lists a rule that's not installed
        fs::write(
            root.path().join("relava.toml"),
            "[rules]\nmissing-rule = \"1.0.0\"\n",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let sync_check = result.checks.iter().find(|c| c.name == "manifest_sync").unwrap();
        assert_eq!(sync_check.status, CheckStatus::Fail);
        assert!(sync_check.message.contains("missing-rule"));

        let orphan_check = result
            .checks
            .iter()
            .find(|c| c.name == "orphaned_resources")
            .unwrap();
        assert_eq!(orphan_check.status, CheckStatus::Warn);
        assert!(orphan_check.message.contains("debugger"));
    }

    // -----------------------------------------------------------------------
    // Result aggregation
    // -----------------------------------------------------------------------

    #[test]
    fn result_counts_correct() {
        let root = temp_dir();
        // No manifest → warn; unreachable registry → fail; files pass
        let opts = test_opts(root.path());
        let result = run(&opts);

        assert!(result.failures >= 1); // registry
        assert!(result.warnings >= 1); // no manifest
        assert_eq!(
            result.passed + result.warnings + result.failures,
            result.checks.len()
        );
    }

    #[test]
    fn is_healthy_with_no_failures() {
        let result = DoctorResult {
            checks: vec![
                CheckResult {
                    name: "test".to_string(),
                    status: CheckStatus::Pass,
                    message: "ok".to_string(),
                },
                CheckResult {
                    name: "test2".to_string(),
                    status: CheckStatus::Warn,
                    message: "warning".to_string(),
                },
            ],
            passed: 1,
            warnings: 1,
            failures: 0,
        };
        assert!(result.is_healthy());
    }

    #[test]
    fn is_unhealthy_with_failures() {
        let result = DoctorResult {
            checks: vec![CheckResult {
                name: "test".to_string(),
                status: CheckStatus::Fail,
                message: "bad".to_string(),
            }],
            passed: 0,
            warnings: 0,
            failures: 1,
        };
        assert!(!result.is_healthy());
    }

    // -----------------------------------------------------------------------
    // JSON serialization
    // -----------------------------------------------------------------------

    #[test]
    fn result_serializes_to_json() {
        let result = DoctorResult {
            checks: vec![CheckResult {
                name: "registry".to_string(),
                status: CheckStatus::Pass,
                message: "Server reachable".to_string(),
            }],
            passed: 1,
            warnings: 0,
            failures: 0,
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"registry\""));
        assert!(json.contains("\"pass\""));
        assert!(json.contains("Server reachable"));
    }

    #[test]
    fn check_status_serializes_lowercase() {
        let json = serde_json::to_string(&CheckStatus::Pass).unwrap();
        assert_eq!(json, "\"pass\"");
        let json = serde_json::to_string(&CheckStatus::Warn).unwrap();
        assert_eq!(json, "\"warn\"");
        let json = serde_json::to_string(&CheckStatus::Fail).unwrap();
        assert_eq!(json, "\"fail\"");
    }

    // -----------------------------------------------------------------------
    // No cross-reference without manifest
    // -----------------------------------------------------------------------

    #[test]
    fn no_cross_ref_checks_without_manifest() {
        let root = temp_dir();
        // No relava.toml, so cross-reference checks should not appear
        let opts = test_opts(root.path());
        let result = run(&opts);

        assert!(result
            .checks
            .iter()
            .all(|c| c.name != "manifest_sync" && c.name != "orphaned_resources"));
    }

    // -----------------------------------------------------------------------
    // Commands/rules resource types in cross-reference
    // -----------------------------------------------------------------------

    #[test]
    fn installed_files_non_md_ignored() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();
        fs::write(agents_dir.join("notes.txt"), "not a resource").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        // Only the .md file should be counted
        assert!(check.message.contains("1 installed"));
    }

    #[test]
    fn installed_files_mixed_valid_and_broken() {
        let root = temp_dir();

        // Two valid skills
        for name in &["good1", "good2"] {
            let dir = root.path().join(format!(".claude/skills/{name}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("# {name}")).unwrap();
        }

        // One broken skill (missing SKILL.md)
        let broken_dir = root.path().join(".claude/skills/broken");
        fs::create_dir_all(&broken_dir).unwrap();
        fs::write(broken_dir.join("random.txt"), "not a skill").unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let check = result.checks.iter().find(|c| c.name == "installed_files").unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.message.contains("1 of 3"));
        assert!(check.message.contains("broken"));
    }

    #[test]
    fn cross_ref_commands_and_rules() {
        let root = temp_dir();

        // Install command and rule
        let cmds_dir = root.path().join(".claude/commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("deploy.md"), "# Deploy").unwrap();

        let rules_dir = root.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("no-console.md"), "# Rule").unwrap();

        // Manifest tracks both
        fs::write(
            root.path().join("relava.toml"),
            "[commands]\ndeploy = \"1.0.0\"\n\n[rules]\nno-console = \"1.0.0\"\n",
        )
        .unwrap();

        let opts = test_opts(root.path());
        let result = run(&opts);

        let sync_check = result.checks.iter().find(|c| c.name == "manifest_sync").unwrap();
        assert_eq!(sync_check.status, CheckStatus::Pass);

        let orphan_check = result
            .checks
            .iter()
            .find(|c| c.name == "orphaned_resources")
            .unwrap();
        assert_eq!(orphan_check.status, CheckStatus::Pass);
    }
}
