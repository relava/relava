use std::path::Path;

use relava_types::manifest::{ProjectManifest, ResourceMeta};
use relava_types::validate::{self, ResourceType};

use crate::api_client::ApiClient;
use crate::install;

/// Options for the info command.
pub struct InfoOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub _verbose: bool,
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
    pub status: String,
}

/// Run `relava info <type> <name>`.
///
/// Queries the server for resource metadata and supplements with local
/// install information (file count, size, location).
pub fn run(opts: &InfoOpts) -> Result<InfoResult, String> {
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    let client = ApiClient::new(opts.server_url);
    let rt_str = opts.resource_type.to_string();

    // Query server for resource metadata
    let server_info = client.get_resource(&rt_str, opts.name);

    // Determine local install status
    let is_installed = install::is_installed(opts.project_dir, opts.resource_type, opts.name);
    let install_path = install::resource_path(opts.project_dir, opts.resource_type, opts.name);

    // Build result from server data + local data
    match server_info {
        Ok(resource) => {
            let (file_count, total_size, install_location) = if is_installed {
                let (fc, ts) = compute_size(opts.resource_type, &install_path);
                let loc = install_path
                    .strip_prefix(opts.project_dir)
                    .unwrap_or(&install_path)
                    .to_string_lossy()
                    .replace('\\', "/");
                (fc, ts, loc)
            } else {
                (0, 0, String::new())
            };

            let version = load_manifest_version(opts.project_dir, opts.resource_type, opts.name)
                .or(resource.latest_version)
                .unwrap_or_default();

            let description = resource.description.unwrap_or_default();

            // Get dependencies from local file if installed
            let dependencies = if is_installed {
                load_dependencies(opts.resource_type, &install_path)
            } else {
                Vec::new()
            };

            let status = if is_installed {
                "installed".to_string()
            } else {
                "registered".to_string()
            };

            let result = InfoResult {
                name: opts.name.to_string(),
                resource_type: rt_str,
                version,
                description,
                dependencies,
                file_count,
                total_size,
                install_location,
                status,
            };

            if !opts.json {
                print_info(&result);
            }

            Ok(result)
        }
        Err(crate::api_client::ApiError::NotFound(_)) if is_installed => {
            // Resource is installed locally but not in the registry
            run_local(opts, &install_path)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Fall back to local-only info when the server doesn't know about a resource
/// that is installed locally.
fn run_local(opts: &InfoOpts, install_path: &Path) -> Result<InfoResult, String> {
    let (file_count, total_size) = compute_size(opts.resource_type, install_path);
    let version =
        load_manifest_version(opts.project_dir, opts.resource_type, opts.name).unwrap_or_default();
    let (description, dependencies) = load_metadata(opts.resource_type, install_path);

    let install_display = install_path
        .strip_prefix(opts.project_dir)
        .unwrap_or(install_path)
        .to_string_lossy()
        .replace('\\', "/");

    let result = InfoResult {
        name: opts.name.to_string(),
        resource_type: opts.resource_type.to_string(),
        version,
        description,
        dependencies,
        file_count,
        total_size,
        install_location: install_display,
        status: "installed".to_string(),
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
fn load_manifest_version(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
) -> Option<String> {
    let path = project_dir.join("relava.toml");
    let manifest = match ProjectManifest::from_file(&path) {
        Ok(m) => m,
        Err(e) => {
            if path.exists() {
                eprintln!("[warn] failed to read relava.toml: {e}");
            }
            return None;
        }
    };
    let section = match resource_type {
        ResourceType::Skill => &manifest.skills,
        ResourceType::Agent => &manifest.agents,
        ResourceType::Command => &manifest.commands,
        ResourceType::Rule => &manifest.rules,
    };
    let v = section.get(name).cloned().unwrap_or_default();
    if v.is_empty() { None } else { Some(v) }
}

/// Extract dependencies from local resource metadata.
fn load_dependencies(resource_type: ResourceType, path: &Path) -> Vec<String> {
    let (_, deps) = load_metadata(resource_type, path);
    deps
}

/// Extract description and dependencies from metadata.
fn load_metadata(resource_type: ResourceType, path: &Path) -> (String, Vec<String>) {
    let md_path = match resource_type {
        ResourceType::Skill => path.join("SKILL.md"),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => path.to_path_buf(),
    };

    let content = match std::fs::read_to_string(&md_path) {
        Ok(c) => c,
        Err(_) => return (String::new(), Vec::new()),
    };

    let meta = ResourceMeta::from_md(&content).unwrap_or_default();
    let mut deps = meta.skills;
    deps.extend(meta.agents);

    let description = extract_description(&content);
    (description, deps)
}

/// Extract a short description from markdown content.
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

/// Strip YAML frontmatter from markdown content.
fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_open = &trimmed[3..];
    match after_open.find("\n---") {
        Some(end) => {
            let rest = &after_open[end + 4..];
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

/// Print info in human-readable key-value format.
fn print_info(info: &InfoResult) {
    let version = if info.version.is_empty() {
        "-".to_string()
    } else {
        info.version.clone()
    };

    let entries: Vec<(&str, String)> = vec![
        ("Name", info.name.clone()),
        ("Type", info.resource_type.clone()),
        ("Version", version),
        ("Status", info.status.clone()),
        ("Description", info.description.clone()),
        ("Dependencies", info.dependencies.join(", ")),
        ("Files", info.file_count.to_string()),
        ("Size", format_size(info.total_size)),
        ("Location", info.install_location.clone()),
    ];

    println!("{}", crate::output::kv_table(&entries));
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
    fn strip_frontmatter_unclosed() {
        let content = "---\nname: test\n# No closing delimiter\nSome body text.";
        assert_eq!(strip_frontmatter(content), content);
    }

    #[test]
    fn load_metadata_with_dependencies() {
        let root = temp_dir();
        let skill_dir = root.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nmetadata:\n  relava:\n    skills:\n      - dep-a\n      - dep-b\n---\n# Test\nA test skill.",
        )
        .unwrap();

        let (desc, deps) = load_metadata(ResourceType::Skill, &skill_dir);
        assert_eq!(desc, "A test skill.");
        assert_eq!(deps, vec!["dep-a", "dep-b"]);
    }

    #[test]
    fn compute_size_single_file() {
        let root = temp_dir();
        let agents_dir = root.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        let file_path = agents_dir.join("test.md");
        fs::write(&file_path, "# Test\nSome content.").unwrap();

        let (count, size) = compute_size(ResourceType::Agent, &file_path);
        assert_eq!(count, 1);
        assert!(size > 0);
    }

    #[test]
    fn compute_size_directory() {
        let root = temp_dir();
        let skill_dir = root.path().join("skill");
        fs::create_dir_all(skill_dir.join("sub")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill_dir.join("sub/extra.md"), "extra").unwrap();

        let (count, size) = compute_size(ResourceType::Skill, &skill_dir);
        assert_eq!(count, 2);
        assert!(size > 0);
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
            status: "installed".to_string(),
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("dep-a"));
        assert!(json.contains("1024"));
        assert!(json.contains("installed"));
    }

    // --- Integration tests: server interaction ---

    #[test]
    fn info_fails_when_server_unreachable_and_not_installed() {
        let root = temp_dir();
        let opts = InfoOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: ResourceType::Skill,
            name: "denden",
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
    fn info_fails_when_server_unreachable_even_if_installed() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "# Denden\nA communication skill.",
        )
        .unwrap();

        let opts = InfoOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: ResourceType::Skill,
            name: "denden",
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
    fn info_invalid_slug_errors() {
        let root = temp_dir();
        let opts = InfoOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: ResourceType::Skill,
            name: "../../../etc",
            project_dir: root.path(),
            json: false,
            _verbose: false,
        };
        assert!(run(&opts).is_err());
    }
}
