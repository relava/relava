use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use relava_types::file_filter::{self, IgnorePatterns, RELAVAIGNORE_FILE};
use relava_types::validate::{self, ResourceType};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_FILE_COUNT: usize = 100;
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_TOTAL_SIZE: u64 = 50 * 1024 * 1024; // 50 MB
// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for the validate command.
pub struct ValidateOpts<'a> {
    #[allow(dead_code)]
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub path: &'a Path,
    pub json: bool,
    pub _verbose: bool,
}

/// A single validation check result.
#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

/// Status of a validation check.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Fail,
}

impl CheckStatus {
    fn tag(&self) -> crate::output::Tag {
        match self {
            Self::Pass => crate::output::Tag::Ok,
            Self::Fail => crate::output::Tag::Fail,
        }
    }
}

/// Result of the validate command.
#[derive(Debug, Serialize)]
pub struct ValidateResult {
    pub resource_type: String,
    pub name: String,
    pub checks: Vec<CheckResult>,
    pub passed: usize,
    pub failures: usize,
}

impl ValidateResult {
    pub fn is_valid(&self) -> bool {
        self.failures == 0
    }
}

// ---------------------------------------------------------------------------
// Accumulator
// ---------------------------------------------------------------------------

/// Accumulator for check results. Handles printing and storage.
struct Checks {
    inner: Vec<CheckResult>,
    json: bool,
}

impl Checks {
    fn new(json: bool) -> Self {
        Self {
            inner: Vec::new(),
            json,
        }
    }

    fn push(&mut self, result: CheckResult) {
        if !self.json {
            println!("{}", result.status.tag().fmt(&result.message));
        }
        self.inner.push(result);
    }

    fn pass(&mut self, name: &str, message: impl Into<String>) {
        self.push(CheckResult {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: message.into(),
        });
    }

    fn fail(&mut self, name: &str, message: impl Into<String>) {
        self.push(CheckResult {
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: message.into(),
        });
    }

    fn counts(&self) -> (usize, usize) {
        let passed = self
            .inner
            .iter()
            .filter(|c| c.status == CheckStatus::Pass)
            .count();
        let failures = self
            .inner
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .count();
        (passed, failures)
    }

    fn into_inner(self) -> Vec<CheckResult> {
        self.inner
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run `relava validate <type> <path>`.
pub fn run(opts: &ValidateOpts) -> Result<ValidateResult, String> {
    // Verify path exists
    let path = opts
        .path
        .canonicalize()
        .map_err(|e| format!("cannot access '{}': {e}", opts.path.display()))?;

    // Derive resource name
    let name = derive_name(&path)?;

    if !opts.json {
        println!("Validating {} {name}...", opts.resource_type);
    }

    let mut checks = Checks::new(opts.json);

    // 1. Slug format
    check_slug(&mut checks, &name);

    // 2. Directory structure
    check_structure(&mut checks, &path, opts.resource_type, &name);

    // 3. Frontmatter YAML (also extracts version and deps for later checks)
    let frontmatter = check_frontmatter(&mut checks, &path, opts.resource_type, &name);

    // 4 & 5. File limits and file type filtering
    check_files(&mut checks, &path, opts.resource_type);

    // 6 & 7. Semver format and dependency declarations (require parsed frontmatter)
    if let Some(ref fm) = frontmatter {
        check_semver(&mut checks, fm);
        check_dependencies(&mut checks, fm);
    }

    let (passed, failures) = checks.counts();

    if !opts.json {
        println!();
        if failures == 0 {
            println!("Validation passed.");
        } else {
            println!(
                "Validation failed: {failures} error{}.",
                if failures == 1 { "" } else { "s" }
            );
        }
    }

    Ok(ValidateResult {
        resource_type: opts.resource_type.to_string(),
        name,
        checks: checks.into_inner(),
        passed,
        failures,
    })
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

/// Check 1: Slug format.
fn check_slug(checks: &mut Checks, name: &str) {
    match validate::validate_slug(name) {
        Ok(()) => checks.pass("slug", "Slug format valid"),
        Err(e) => checks.fail("slug", format!("Slug invalid: {e}")),
    }
}

/// Check 2: Directory structure.
fn check_structure(checks: &mut Checks, path: &Path, resource_type: ResourceType, name: &str) {
    match validate::validate_resource_structure(path, resource_type, name) {
        Ok(()) => {
            let detail = match resource_type {
                ResourceType::Skill => "SKILL.md present".to_string(),
                _ => format!("{name}.md present"),
            };
            checks.pass("structure", detail);
        }
        Err(e) => checks.fail("structure", e.to_string()),
    }
}

/// Parsed frontmatter data extracted during validation.
struct FrontmatterData {
    version: Option<String>,
    skills: Vec<String>,
    agents: Vec<String>,
}

/// Check 3: Frontmatter YAML validity.
fn check_frontmatter(
    checks: &mut Checks,
    path: &Path,
    resource_type: ResourceType,
    name: &str,
) -> Option<FrontmatterData> {
    let md_path = primary_md_path(path, resource_type, name);
    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(e) => {
            checks.fail(
                "frontmatter",
                format!("Cannot read {}: {e}", md_path.display()),
            );
            return None;
        }
    };

    let yaml_str = match extract_frontmatter_yaml(&content) {
        Some(y) => y,
        None => {
            // No frontmatter is acceptable — just not parseable
            checks.pass("frontmatter", "No frontmatter present (optional)");
            return Some(FrontmatterData {
                version: None,
                skills: Vec::new(),
                agents: Vec::new(),
            });
        }
    };

    // Try to parse as YAML
    let yaml_value: serde_yaml::Value = match serde_yaml::from_str(yaml_str) {
        Ok(v) => v,
        Err(e) => {
            checks.fail("frontmatter", format!("Frontmatter YAML is invalid: {e}"));
            return None;
        }
    };

    // Extract version
    let version = yaml_value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract metadata.relava dependencies
    let relava_block = yaml_value.get("metadata").and_then(|m| m.get("relava"));
    let skills = extract_string_list(relava_block, "skills");
    let agents = extract_string_list(relava_block, "agents");

    checks.pass("frontmatter", "Frontmatter parseable");

    Some(FrontmatterData {
        version,
        skills,
        agents,
    })
}

/// Check 4 & 5: File limits and file type filtering.
fn check_files(checks: &mut Checks, path: &Path, resource_type: ResourceType) {
    // Load .relavaignore patterns (continue with empty patterns on error)
    let ignore = match IgnorePatterns::load(path) {
        Ok(p) => p,
        Err(e) => {
            checks.fail("relavaignore", format!("Cannot load .relavaignore: {e}"));
            IgnorePatterns::default()
        }
    };

    // Collect all files (skip hidden, apply .relavaignore)
    let files = match collect_file_paths(path) {
        Ok(f) => file_filter::filter_ignored(path, f, &ignore),
        Err(e) => {
            checks.fail("file_count", format!("Cannot scan files: {e}"));
            return;
        }
    };

    // 4a. File count
    let file_count_msg = format!("File count: {} (max {MAX_FILE_COUNT})", files.len());
    if files.len() > MAX_FILE_COUNT {
        checks.fail("file_count", file_count_msg);
    } else {
        checks.pass("file_count", file_count_msg);
    }

    // 4b. File sizes
    let mut total_size: u64 = 0;
    let mut oversized: Vec<String> = Vec::new();
    let mut size_errors: Vec<String> = Vec::new();

    for file_path in &files {
        let metadata = match std::fs::metadata(file_path) {
            Ok(m) => m,
            Err(e) => {
                size_errors.push(format!("{}: {e}", relative_display(file_path, path)));
                continue;
            }
        };

        let size = metadata.len();
        total_size += size;

        if size > MAX_FILE_SIZE {
            oversized.push(format!(
                "{} ({})",
                relative_display(file_path, path),
                format_size(size)
            ));
        }
    }

    // 5. Binary detection (for text-only resource types)
    let binary_scan = file_filter::scan_for_binary_files(
        resource_type,
        files.iter().map(|f| (f.clone(), relative_display(f, path))),
    );

    // Report per-file size violations
    if oversized.is_empty() && size_errors.is_empty() {
        checks.pass("file_sizes", "All files within 10 MB limit");
    } else {
        for f in &oversized {
            checks.fail("file_sizes", format!("File exceeds 10 MB: {f}"));
        }
        for e in &size_errors {
            checks.fail("file_sizes", format!("Cannot read file size: {e}"));
        }
    }

    // Report total size
    let total_size_msg = format!(
        "Total size: {} (max {})",
        format_size(total_size),
        format_size(MAX_TOTAL_SIZE)
    );
    if total_size > MAX_TOTAL_SIZE {
        checks.fail("total_size", total_size_msg);
    } else {
        checks.pass("total_size", total_size_msg);
    }

    // 5. File type filtering
    if file_filter::requires_text_only(resource_type) {
        if binary_scan.is_clean() {
            checks.pass("file_types", "All files are text");
        } else {
            let type_label = match resource_type {
                ResourceType::Skill => "skills",
                ResourceType::Command => "commands",
                ResourceType::Rule => "rules",
                ResourceType::Agent => unreachable!(),
            };
            for f in &binary_scan.binary_files {
                checks.fail(
                    "file_types",
                    format!("Contains binary file: {f} ({type_label} must be text-only)"),
                );
            }
        }
    } else {
        checks.pass("file_types", "All file types allowed for agents");
    }
}

/// Check 6: Semver format.
fn check_semver(checks: &mut Checks, fm: &FrontmatterData) {
    match &fm.version {
        Some(v) => match validate::validate_version(v) {
            Ok(_) => checks.pass("semver", format!("Version {v} is valid semver")),
            Err(e) => checks.fail("semver", e.to_string()),
        },
        None => checks.pass("semver", "No version in frontmatter (optional)"),
    }
}

/// Check 7: Dependency declarations.
///
/// This is an offline check — we validate that declared dependency names are
/// valid slugs. Checking whether they exist in the registry would require a
/// server connection, which contradicts the "offline" nature of validate.
fn check_dependencies(checks: &mut Checks, fm: &FrontmatterData) {
    let all_deps: Vec<(&str, &str)> = fm
        .skills
        .iter()
        .map(|s| (s.as_str(), "skill"))
        .chain(fm.agents.iter().map(|s| (s.as_str(), "agent")))
        .collect();

    if all_deps.is_empty() {
        checks.pass("dependencies", "No dependencies declared");
        return;
    }

    let mut invalid: Vec<String> = Vec::new();
    let mut valid_names: Vec<String> = Vec::new();

    for (name, dep_type) in &all_deps {
        if let Err(e) = validate::validate_slug(name) {
            invalid.push(format!("{dep_type} '{name}': {e}"));
        } else {
            valid_names.push(name.to_string());
        }
    }

    if invalid.is_empty() {
        checks.pass(
            "dependencies",
            format!("Dependencies declared: {}", valid_names.join(", ")),
        );
    } else {
        for err in &invalid {
            checks.fail("dependencies", format!("Invalid dependency: {err}"));
        }
        if !valid_names.is_empty() {
            checks.pass(
                "dependencies",
                format!("Valid dependencies: {}", valid_names.join(", ")),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a list of strings from a YAML mapping key.
fn extract_string_list(parent: Option<&serde_yaml::Value>, key: &str) -> Vec<String> {
    parent
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Derive the resource name from a path.
fn derive_name(path: &Path) -> Result<String, String> {
    let component = if path.is_dir() {
        path.file_name()
    } else {
        path.file_stem()
    };
    component
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("cannot derive name from path: {}", path.display()))
}

/// Get the primary markdown file path for a resource.
fn primary_md_path(path: &Path, resource_type: ResourceType, name: &str) -> PathBuf {
    match resource_type {
        ResourceType::Skill => path.join("SKILL.md"),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            if path.is_file() {
                path.to_path_buf()
            } else {
                path.join(format!("{name}.md"))
            }
        }
    }
}

/// Extract the raw YAML frontmatter string from markdown content.
fn extract_frontmatter_yaml(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_open = &trimmed[3..];
    after_open.find("\n---").map(|end| &after_open[..end])
}

/// Recursively collect all file paths under a directory, skipping hidden files.
/// For single-file resources, returns just that file.
fn collect_file_paths(path: &Path) -> Result<Vec<PathBuf>, String> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = Vec::new();
    collect_dir_paths(path, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_dir_paths(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {e}"))?;
        let entry_path = entry.path();

        // Skip hidden files/directories, but allow .relavaignore through
        if entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.') && n != RELAVAIGNORE_FILE)
        {
            continue;
        }

        // Skip path traversal
        if entry_path.components().any(|c| c == Component::ParentDir) {
            continue;
        }

        if entry_path.is_dir() {
            collect_dir_paths(&entry_path, files)?;
        } else {
            files.push(entry_path);
        }
    }
    Ok(())
}

/// Format a byte count as a human-readable string.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Get a relative display path for error messages.
fn relative_display(file_path: &Path, base: &Path) -> String {
    file_path
        .strip_prefix(base)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| file_path.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    fn test_opts(resource_type: ResourceType, path: &Path) -> ValidateOpts<'_> {
        ValidateOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type,
            path,
            json: true,
            _verbose: false,
        }
    }

    // -----------------------------------------------------------------------
    // Slug validation
    // -----------------------------------------------------------------------

    #[test]
    fn valid_slug_passes() {
        let root = temp_dir();
        let skill = root.path().join("code-review");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Code Review").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result.checks.iter().find(|c| c.name == "slug").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn invalid_slug_fails() {
        let root = temp_dir();
        let skill = root.path().join("Invalid_Name");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Bad").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result.checks.iter().find(|c| c.name == "slug").unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    // -----------------------------------------------------------------------
    // Directory structure
    // -----------------------------------------------------------------------

    #[test]
    fn skill_with_skill_md_passes() {
        let root = temp_dir();
        let skill = root.path().join("my-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "structure")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn skill_missing_skill_md_fails() {
        let root = temp_dir();
        let skill = root.path().join("bad-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("README.md"), "# README").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "structure")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn agent_file_passes() {
        let root = temp_dir();
        let file = root.path().join("debugger.md");
        fs::write(&file, "---\nname: debugger\n---\n# Agent").unwrap();

        let opts = test_opts(ResourceType::Agent, &file);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "structure")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn agent_dir_with_md_passes() {
        let root = temp_dir();
        let agent = root.path().join("planner");
        fs::create_dir_all(&agent).unwrap();
        fs::write(agent.join("planner.md"), "# Planner").unwrap();

        let opts = test_opts(ResourceType::Agent, &agent);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "structure")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    // -----------------------------------------------------------------------
    // Frontmatter
    // -----------------------------------------------------------------------

    #[test]
    fn valid_frontmatter_passes() {
        let root = temp_dir();
        let skill = root.path().join("my-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: my-skill\nversion: 1.0.0\n---\n# Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "frontmatter")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("parseable"));
    }

    #[test]
    fn invalid_yaml_frontmatter_fails() {
        let root = temp_dir();
        let skill = root.path().join("bad-fm");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\n[[[invalid yaml\n---\n# Skill").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "frontmatter")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn no_frontmatter_passes() {
        let root = temp_dir();
        let skill = root.path().join("no-fm");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Just markdown, no frontmatter").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "frontmatter")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn frontmatter_with_metadata_relava_passes() {
        let root = temp_dir();
        let skill = root.path().join("with-deps");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: with-deps\nversion: 1.0.0\nmetadata:\n  relava:\n    skills:\n      - security-baseline\n      - style-guide\n---\n# Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let fm_check = result
            .checks
            .iter()
            .find(|c| c.name == "frontmatter")
            .unwrap();
        assert_eq!(fm_check.status, CheckStatus::Pass);

        let dep_check = result
            .checks
            .iter()
            .find(|c| c.name == "dependencies")
            .unwrap();
        assert_eq!(dep_check.status, CheckStatus::Pass);
        assert!(dep_check.message.contains("security-baseline"));
        assert!(dep_check.message.contains("style-guide"));
    }

    // -----------------------------------------------------------------------
    // File limits
    // -----------------------------------------------------------------------

    #[test]
    fn file_count_within_limit_passes() {
        let root = temp_dir();
        let skill = root.path().join("few-files");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill.join("helper.md"), "# Helper").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "file_count")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("2"));
    }

    #[test]
    fn total_size_within_limit_passes() {
        let root = temp_dir();
        let skill = root.path().join("small-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "total_size")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    // -----------------------------------------------------------------------
    // File type filtering (binary detection)
    // -----------------------------------------------------------------------

    #[test]
    fn text_only_skill_passes() {
        let root = temp_dir();
        let skill = root.path().join("text-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill.join("config.json"), "{}").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "file_types")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("text"));
    }

    #[test]
    fn binary_in_skill_fails() {
        let root = temp_dir();
        let skill = root.path().join("binary-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();
        // Write a file with null bytes (binary)
        let mut binary_content = vec![0u8; 100];
        binary_content[0] = b'M';
        binary_content[1] = b'Z'; // PE header-like
        binary_content[50] = 0; // null byte
        fs::write(skill.join("tool.bin"), &binary_content).unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let checks: Vec<_> = result
            .checks
            .iter()
            .filter(|c| c.name == "file_types")
            .collect();
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail));
        assert!(
            checks
                .iter()
                .any(|c| c.message.contains("binary") && c.message.contains("tool.bin"))
        );
    }

    #[test]
    fn binary_in_agent_allowed() {
        let root = temp_dir();
        let agent = root.path().join("agent-with-bin");
        fs::create_dir_all(&agent).unwrap();
        fs::write(agent.join("agent-with-bin.md"), "# Agent").unwrap();
        fs::write(agent.join("data.bin"), [0u8; 100]).unwrap();

        let opts = test_opts(ResourceType::Agent, &agent);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "file_types")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("allowed"));
    }

    // -----------------------------------------------------------------------
    // Semver validation
    // -----------------------------------------------------------------------

    #[test]
    fn valid_semver_passes() {
        let root = temp_dir();
        let skill = root.path().join("versioned");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: versioned\nversion: 2.3.1\n---\n# Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result.checks.iter().find(|c| c.name == "semver").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("2.3.1"));
    }

    #[test]
    fn invalid_semver_fails() {
        let root = temp_dir();
        let skill = root.path().join("bad-version");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: bad-version\nversion: banana\n---\n# Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result.checks.iter().find(|c| c.name == "semver").unwrap();
        assert_eq!(check.status, CheckStatus::Fail);
    }

    #[test]
    fn no_version_in_frontmatter_passes() {
        let root = temp_dir();
        let skill = root.path().join("no-ver");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: no-ver\n---\n# Skill").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result.checks.iter().find(|c| c.name == "semver").unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
    }

    // -----------------------------------------------------------------------
    // Dependency validation
    // -----------------------------------------------------------------------

    #[test]
    fn no_dependencies_passes() {
        let root = temp_dir();
        let skill = root.path().join("no-deps");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: no-deps\nversion: 1.0.0\n---\n# Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "dependencies")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("No dependencies"));
    }

    #[test]
    fn valid_dependencies_pass() {
        let root = temp_dir();
        let skill = root.path().join("has-deps");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: has-deps\nmetadata:\n  relava:\n    skills:\n      - foo\n      - bar-baz\n---\n",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "dependencies" && c.status == CheckStatus::Pass)
            .unwrap();
        assert!(check.message.contains("foo"));
        assert!(check.message.contains("bar-baz"));
    }

    #[test]
    fn invalid_dependency_slug_fails() {
        let root = temp_dir();
        let skill = root.path().join("bad-deps");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: bad-deps\nmetadata:\n  relava:\n    skills:\n      - Valid-Dep\n---\n",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let checks: Vec<_> = result
            .checks
            .iter()
            .filter(|c| c.name == "dependencies")
            .collect();
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail));
    }

    #[test]
    fn agent_dependencies_validated() {
        let root = temp_dir();
        let agent = root.path().join("orchestrator.md");
        fs::write(
            &agent,
            "---\nname: orchestrator\nmetadata:\n  relava:\n    skills:\n      - code-review\n    agents:\n      - debugger\n---\n",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Agent, &agent);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "dependencies" && c.status == CheckStatus::Pass)
            .unwrap();
        assert!(check.message.contains("code-review"));
        assert!(check.message.contains("debugger"));
    }

    // -----------------------------------------------------------------------
    // Overall result
    // -----------------------------------------------------------------------

    #[test]
    fn all_checks_pass_for_valid_skill() {
        let root = temp_dir();
        let skill = root.path().join("perfect-skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: perfect-skill\nversion: 1.0.0\nmetadata:\n  relava:\n    skills:\n      - helper\n---\n# Perfect Skill",
        )
        .unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        assert!(result.is_valid());
        assert_eq!(result.failures, 0);
        assert!(result.passed > 0);
        assert_eq!(result.resource_type, "skill");
        assert_eq!(result.name, "perfect-skill");
    }

    #[test]
    fn multiple_failures_counted() {
        let root = temp_dir();
        // Invalid slug + missing SKILL.md
        let skill = root.path().join("Bad_Name");
        fs::create_dir_all(&skill).unwrap();
        // No SKILL.md

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        assert!(!result.is_valid());
        assert!(result.failures >= 2); // at least slug + structure
    }

    #[test]
    fn nonexistent_path_errors() {
        let path = PathBuf::from("/nonexistent/path");
        let opts = test_opts(ResourceType::Skill, &path);
        assert!(run(&opts).is_err());
    }

    // -----------------------------------------------------------------------
    // JSON serialization
    // -----------------------------------------------------------------------

    #[test]
    fn result_serializes_to_json() {
        let result = ValidateResult {
            resource_type: "skill".to_string(),
            name: "test".to_string(),
            checks: vec![CheckResult {
                name: "slug".to_string(),
                status: CheckStatus::Pass,
                message: "Slug format valid".to_string(),
            }],
            passed: 1,
            failures: 0,
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("\"slug\""));
        assert!(json.contains("\"pass\""));
    }

    #[test]
    fn check_status_serializes_lowercase() {
        let json = serde_json::to_string(&CheckStatus::Pass).unwrap();
        assert_eq!(json, "\"pass\"");
        let json = serde_json::to_string(&CheckStatus::Fail).unwrap();
        assert_eq!(json, "\"fail\"");
    }

    // -----------------------------------------------------------------------
    // Hidden files skipped
    // -----------------------------------------------------------------------

    #[test]
    fn hidden_files_skipped_in_count() {
        let root = temp_dir();
        let skill = root.path().join("hidden-test");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill.join(".hidden"), "secret").unwrap();
        fs::write(skill.join("visible.md"), "visible").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "file_count")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("2")); // SKILL.md + visible.md, not .hidden
    }

    // -----------------------------------------------------------------------
    // Helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(24_576), "24 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1_048_576), "1.0 MB");
        assert_eq!(format_size(52_428_800), "50.0 MB");
    }

    #[test]
    fn derive_name_from_dir() {
        let root = temp_dir();
        let dir = root.path().join("my-skill");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(derive_name(&dir).unwrap(), "my-skill");
    }

    #[test]
    fn derive_name_from_file() {
        let root = temp_dir();
        let file = root.path().join("my-agent.md");
        fs::write(&file, "content").unwrap();
        assert_eq!(derive_name(&file).unwrap(), "my-agent");
    }

    #[test]
    fn extract_frontmatter_basic() {
        let content = "---\nname: test\nversion: 1.0.0\n---\n# Body";
        let yaml = extract_frontmatter_yaml(content);
        assert!(yaml.is_some());
        assert!(yaml.unwrap().contains("name: test"));
    }

    #[test]
    fn extract_frontmatter_none() {
        let content = "# Just a heading";
        assert!(extract_frontmatter_yaml(content).is_none());
    }

    // -----------------------------------------------------------------------
    // Command / Rule resource types
    // -----------------------------------------------------------------------

    #[test]
    fn command_file_validates() {
        let root = temp_dir();
        let file = root.path().join("deploy.md");
        fs::write(&file, "---\nname: deploy\nversion: 1.0.0\n---\n# Deploy").unwrap();

        let opts = test_opts(ResourceType::Command, &file);
        let result = run(&opts).unwrap();

        assert!(result.is_valid());
        assert_eq!(result.name, "deploy");
    }

    #[test]
    fn rule_file_validates() {
        let root = temp_dir();
        let file = root.path().join("no-console.md");
        fs::write(&file, "---\nname: no-console\nversion: 1.0.0\n---\n# Rule").unwrap();

        let opts = test_opts(ResourceType::Rule, &file);
        let result = run(&opts).unwrap();

        assert!(result.is_valid());
        assert_eq!(result.name, "no-console");
    }

    #[test]
    fn binary_in_command_fails() {
        let root = temp_dir();
        let cmd = root.path().join("my-cmd");
        fs::create_dir_all(&cmd).unwrap();
        fs::write(cmd.join("my-cmd.md"), "# Command").unwrap();
        fs::write(cmd.join("binary.dat"), [0u8; 50]).unwrap();

        let opts = test_opts(ResourceType::Command, &cmd);
        let result = run(&opts).unwrap();

        let checks: Vec<_> = result
            .checks
            .iter()
            .filter(|c| c.name == "file_types")
            .collect();
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail));
    }

    #[test]
    fn binary_in_rule_fails() {
        let root = temp_dir();
        let rule = root.path().join("my-rule");
        fs::create_dir_all(&rule).unwrap();
        fs::write(rule.join("my-rule.md"), "# Rule").unwrap();
        fs::write(rule.join("binary.dat"), [0u8; 50]).unwrap();

        let opts = test_opts(ResourceType::Rule, &rule);
        let result = run(&opts).unwrap();

        let checks: Vec<_> = result
            .checks
            .iter()
            .filter(|c| c.name == "file_types")
            .collect();
        assert!(checks.iter().any(|c| c.status == CheckStatus::Fail));
    }

    // -----------------------------------------------------------------------
    // Subdirectory scanning
    // -----------------------------------------------------------------------

    #[test]
    fn nested_files_counted() {
        let root = temp_dir();
        let skill = root.path().join("nested");
        fs::create_dir_all(skill.join("templates/sub")).unwrap();
        fs::write(skill.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill.join("templates/a.md"), "a").unwrap();
        fs::write(skill.join("templates/sub/b.md"), "b").unwrap();

        let opts = test_opts(ResourceType::Skill, &skill);
        let result = run(&opts).unwrap();

        let check = result
            .checks
            .iter()
            .find(|c| c.name == "file_count")
            .unwrap();
        assert_eq!(check.status, CheckStatus::Pass);
        assert!(check.message.contains("3"));
    }
}
