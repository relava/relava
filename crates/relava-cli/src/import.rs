use std::path::{Component, Path, PathBuf};

use base64::Engine;
use serde::Serialize;

use crate::registry::RegistryClient;
use relava_types::validate::{self, ResourceType};
use relava_types::version::Version;

/// Options for the import command.
pub struct ImportOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub path: &'a Path,
    pub version: Option<&'a str>,
    pub json: bool,
    pub verbose: bool,
}

/// Result of a successful import, used for JSON output.
#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub resource_type: String,
    pub name: String,
    pub version: String,
    pub files: Vec<String>,
    pub description: Option<String>,
}

/// Run `relava import <type> <path>`.
///
/// Validates the local resource at `path`, collects its files, and publishes
/// them to the local registry server.
pub fn run(opts: &ImportOpts) -> Result<ImportResult, String> {
    // 1. Verify path exists
    let path = opts
        .path
        .canonicalize()
        .map_err(|e| format!("cannot access '{}': {e}", opts.path.display()))?;

    // 2. Derive resource name from directory/file name
    let name = derive_name(&path)?;

    // 3. Validate slug
    validate::validate_slug(&name).map_err(|e| e.to_string())?;

    // 4. Validate resource structure
    validate::validate_resource_structure(&path, opts.resource_type, &name)
        .map_err(|e| e.to_string())?;

    // 5. Determine version
    let version = match opts.version {
        Some(v) => validate::validate_version(v).map_err(|e| e.to_string())?,
        None => {
            let from_frontmatter =
                extract_frontmatter_field(&path, opts.resource_type, &name, "version")
                    .and_then(|v| Version::parse(&v).ok());
            match from_frontmatter {
                Some(v) => v,
                None => {
                    let default = Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    };
                    if !opts.json {
                        eprintln!(
                            "warning: no version found in frontmatter, defaulting to {default}"
                        );
                    }
                    default
                }
            }
        }
    };

    if opts.verbose {
        eprintln!(
            "importing {} '{}' at version {} from {}",
            opts.resource_type,
            name,
            version,
            path.display()
        );
    }

    // 6. Collect files as base64-encoded entries
    let files = collect_files(&path, opts.resource_type)?;
    if files.is_empty() {
        return Err("resource contains no files".to_string());
    }

    let file_paths: Vec<String> = files.iter().map(|(p, _)| p.clone()).collect();

    // 7. Extract description from frontmatter
    let description = extract_frontmatter_field(&path, opts.resource_type, &name, "description");

    // 8. Publish to registry
    let client = RegistryClient::new(opts.server_url);
    client
        .publish(
            opts.resource_type,
            &name,
            &version,
            &files,
            description.as_deref(),
        )
        .map_err(|e| e.to_string())?;

    if !opts.json {
        println!(
            "Published {} {}@{} ({} file{})",
            opts.resource_type,
            name,
            version,
            file_paths.len(),
            if file_paths.len() == 1 { "" } else { "s" }
        );
    }

    Ok(ImportResult {
        resource_type: opts.resource_type.to_string(),
        name,
        version: version.to_string(),
        files: file_paths,
        description,
    })
}

/// Derive the resource name from a path.
///
/// For directories, uses the directory name.
/// For files, uses the file stem (without extension).
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

/// Extract a field value from the resource's YAML frontmatter.
///
/// Reads the primary markdown file, parses its frontmatter, and returns
/// the value of the specified field. Returns `None` if the file can't be
/// read or the field is absent.
fn extract_frontmatter_field(
    path: &Path,
    resource_type: ResourceType,
    name: &str,
    key: &str,
) -> Option<String> {
    let md_path = primary_md_path(path, resource_type, name);
    let content = std::fs::read_to_string(&md_path).ok()?;
    let yaml = extract_frontmatter_yaml(&content)?;
    extract_yaml_field(yaml, key)
}

/// Extract a top-level scalar YAML field value by key.
///
/// Simple line-based parser — works for flat `key: value` fields without
/// requiring a full YAML parser dependency in the CLI crate.
fn extract_yaml_field(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in yaml.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
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

/// Collect all files from a resource path as (relative_path, base64_content) pairs.
fn collect_files(
    path: &Path,
    resource_type: ResourceType,
) -> Result<Vec<(String, String)>, String> {
    let encoder = base64::engine::general_purpose::STANDARD;

    if path.is_file() {
        // Single file resource (agent, command, rule as .md file)
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "cannot read file name".to_string())?;
        let content =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Ok(vec![(file_name.to_string(), encoder.encode(&content))])
    } else {
        // Directory resource — walk recursively
        let mut files = Vec::new();
        collect_dir_files(path, path, &encoder, &mut files)?;
        files.sort_by(|a, b| a.0.cmp(&b.0));

        if files.is_empty() {
            return Err(format!(
                "{resource_type} directory is empty: {}",
                path.display()
            ));
        }

        Ok(files)
    }
}

/// Recursively collect files from a directory.
fn collect_dir_files(
    dir: &Path,
    base: &Path,
    encoder: &base64::engine::GeneralPurpose,
    files: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {e}"))?;
        let entry_path = entry.path();

        // Skip hidden files/directories
        if entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }

        if entry_path.is_dir() {
            collect_dir_files(&entry_path, base, encoder, files)?;
        } else {
            let relative = entry_path
                .strip_prefix(base)
                .map_err(|e| format!("path error: {e}"))?;

            // Validate no path traversal
            if relative.components().any(|c| c == Component::ParentDir) {
                return Err(format!("unsafe file path: {}", relative.display()));
            }

            let relative_str = relative.to_string_lossy().replace('\\', "/");
            let content = std::fs::read(&entry_path)
                .map_err(|e| format!("cannot read {}: {e}", entry_path.display()))?;
            files.push((relative_str, encoder.encode(&content)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("relava-import-test-{}-{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -- derive_name --

    #[test]
    fn derive_name_from_directory() {
        let dir = tempdir();
        let skill_dir = dir.join("code-review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "content").unwrap();
        let name = derive_name(&skill_dir).unwrap();
        assert_eq!(name, "code-review");
    }

    #[test]
    fn derive_name_from_file() {
        let dir = tempdir();
        let file = dir.join("debugger.md");
        fs::write(&file, "content").unwrap();
        let name = derive_name(&file).unwrap();
        assert_eq!(name, "debugger");
    }

    // -- extract_frontmatter_field --

    #[test]
    fn extracts_version_from_skill_frontmatter() {
        let dir = tempdir();
        let skill_dir = dir.join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\nversion: 2.3.1\n---\n# My Skill",
        )
        .unwrap();
        let raw = extract_frontmatter_field(&skill_dir, ResourceType::Skill, "my-skill", "version");
        let version = raw.and_then(|v| Version::parse(&v).ok());
        assert_eq!(
            version,
            Some(Version {
                major: 2,
                minor: 3,
                patch: 1
            })
        );
    }

    #[test]
    fn defaults_version_when_no_frontmatter() {
        let dir = tempdir();
        let skill_dir = dir.join("bare-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# No frontmatter").unwrap();
        let raw =
            extract_frontmatter_field(&skill_dir, ResourceType::Skill, "bare-skill", "version");
        assert_eq!(raw, None);
    }

    #[test]
    fn extracts_version_from_agent_file() {
        let dir = tempdir();
        let file = dir.join("my-agent.md");
        fs::write(&file, "---\nname: my-agent\nversion: 0.1.0\n---\n# Agent").unwrap();
        let raw = extract_frontmatter_field(&file, ResourceType::Agent, "my-agent", "version");
        let version = raw.and_then(|v| Version::parse(&v).ok());
        assert_eq!(
            version,
            Some(Version {
                major: 0,
                minor: 1,
                patch: 0
            })
        );
    }

    // -- extract_frontmatter_field (description) --

    #[test]
    fn extracts_description() {
        let dir = tempdir();
        let skill_dir = dir.join("desc-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: desc-skill\ndescription: A great skill\n---\n",
        )
        .unwrap();
        let desc =
            extract_frontmatter_field(&skill_dir, ResourceType::Skill, "desc-skill", "description");
        assert_eq!(desc, Some("A great skill".to_string()));
    }

    #[test]
    fn no_description_when_absent() {
        let dir = tempdir();
        let skill_dir = dir.join("no-desc");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: no-desc\n---\n").unwrap();
        let desc =
            extract_frontmatter_field(&skill_dir, ResourceType::Skill, "no-desc", "description");
        assert_eq!(desc, None);
    }

    // -- collect_files --

    #[test]
    fn collects_skill_directory_files() {
        let dir = tempdir();
        let skill_dir = dir.join("my-skill");
        fs::create_dir_all(skill_dir.join("templates")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Skill").unwrap();
        fs::write(skill_dir.join("templates/foo.md"), "template").unwrap();

        let files = collect_files(&skill_dir, ResourceType::Skill).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["SKILL.md", "templates/foo.md"]);
    }

    #[test]
    fn collects_single_file_resource() {
        let dir = tempdir();
        let file = dir.join("my-agent.md");
        fs::write(&file, "# Agent content").unwrap();

        let files = collect_files(&file, ResourceType::Agent).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "my-agent.md");
    }

    #[test]
    fn skips_hidden_files() {
        let dir = tempdir();
        let skill_dir = dir.join("hidden-test");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "content").unwrap();
        fs::write(skill_dir.join(".gitignore"), "ignored").unwrap();

        let files = collect_files(&skill_dir, ResourceType::Skill).unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, vec!["SKILL.md"]);
    }

    #[test]
    fn empty_directory_errors() {
        let dir = tempdir();
        let skill_dir = dir.join("empty-skill");
        fs::create_dir_all(&skill_dir).unwrap();

        let result = collect_files(&skill_dir, ResourceType::Skill);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    // -- validate structure errors --

    #[test]
    fn rejects_skill_without_skill_md() {
        let dir = tempdir();
        let skill_dir = dir.join("bad-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("README.md"), "not a SKILL.md").unwrap();

        let opts = ImportOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            path: &skill_dir,
            version: None,
            json: false,
            verbose: false,
        };

        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("SKILL.md"));
    }

    #[test]
    fn rejects_invalid_slug() {
        let dir = tempdir();
        let bad_dir = dir.join("Invalid_Name");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("SKILL.md"), "content").unwrap();

        let opts = ImportOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            path: &bad_dir,
            version: None,
            json: false,
            verbose: false,
        };

        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid slug"));
    }

    #[test]
    fn rejects_nonexistent_path() {
        let path = PathBuf::from("/nonexistent/path/to/resource");

        let opts = ImportOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            path: &path,
            version: None,
            json: false,
            verbose: false,
        };

        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot access"));
    }

    #[test]
    fn respects_explicit_version() {
        let dir = tempdir();
        let skill_dir = dir.join("versioned");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: versioned\nversion: 1.0.0\n---\n",
        )
        .unwrap();

        // Explicit version should override frontmatter version.
        // This test only validates parsing — the publish call will fail
        // since there's no server running.
        let opts = ImportOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            path: &skill_dir,
            version: Some("3.0.0"),
            json: false,
            verbose: false,
        };

        let result = run(&opts);
        // Expect a server connection error, not a version error
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not reachable"));
    }

    #[test]
    fn rejects_invalid_explicit_version() {
        let dir = tempdir();
        let skill_dir = dir.join("bad-ver");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "content").unwrap();

        let opts = ImportOpts {
            server_url: "http://localhost:7420",
            resource_type: ResourceType::Skill,
            path: &skill_dir,
            version: Some("not-a-version"),
            json: false,
            verbose: false,
        };

        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid version"));
    }

    // -- extract_frontmatter_yaml --

    #[test]
    fn extract_yaml_basic() {
        let content = "---\nname: test\n---\n# Body";
        let yaml = extract_frontmatter_yaml(content);
        assert_eq!(yaml, Some("\nname: test"));
    }

    #[test]
    fn extract_yaml_no_frontmatter() {
        let content = "# Just a heading\nSome content.";
        let yaml = extract_frontmatter_yaml(content);
        assert_eq!(yaml, None);
    }

    #[test]
    fn extract_yaml_no_closing() {
        let content = "---\nname: test\n# No closing delimiter";
        let yaml = extract_frontmatter_yaml(content);
        assert_eq!(yaml, None);
    }

    // -- primary_md_path --

    #[test]
    fn primary_md_path_skill() {
        let path = PathBuf::from("/tmp/my-skill");
        assert_eq!(
            primary_md_path(&path, ResourceType::Skill, "my-skill"),
            PathBuf::from("/tmp/my-skill/SKILL.md")
        );
    }

    #[test]
    fn primary_md_path_agent_dir() {
        let dir = tempdir();
        let agent_dir = dir.join("my-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        assert_eq!(
            primary_md_path(&agent_dir, ResourceType::Agent, "my-agent"),
            agent_dir.join("my-agent.md")
        );
    }

    #[test]
    fn primary_md_path_agent_file() {
        let dir = tempdir();
        let file = dir.join("my-agent.md");
        fs::write(&file, "content").unwrap();
        assert_eq!(
            primary_md_path(&file, ResourceType::Agent, "my-agent"),
            file
        );
    }
}
