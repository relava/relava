use std::path::Path;

use relava_types::validate::{self, ResourceType};

use crate::api_client::ApiClient;
use crate::install;

/// Options for the remove command.
pub struct RemoveOpts<'a> {
    pub server_url: &'a str,
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// Result of a successful remove, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct RemoveResult {
    pub resource_type: String,
    pub name: String,
    pub removed_path: String,
    /// Whether a resource was actually removed from disk.
    pub was_removed: bool,
}

/// Run `relava remove <type> <name>`.
///
/// Deletes the installed resource files from the project. Skills are
/// directories; agents, commands, and rules are individual .md files.
/// Warns (does not error) if the resource is not installed.
/// Cleans up empty parent directories after removal.
pub fn run(opts: &RemoveOpts) -> Result<RemoveResult, String> {
    // Validate the resource name
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    // Remove from the registry (server must be running)
    let client = ApiClient::new(opts.server_url);
    let rt_str = opts.resource_type.to_string();
    match client.delete_resource(&rt_str, opts.name) {
        Ok(()) => {}
        Err(crate::api_client::ApiError::NotFound(_)) => {} // already gone from registry
        Err(e) => return Err(e.to_string()),
    }

    // Check if installed locally — warn but don't error
    if !install::is_installed(opts.project_dir, opts.resource_type, opts.name) {
        if !opts.json {
            eprintln!(
                "[warn] {} '{}' is not installed locally",
                opts.resource_type, opts.name
            );
        }
        return Ok(RemoveResult {
            resource_type: opts.resource_type.to_string(),
            name: opts.name.to_string(),
            removed_path: String::new(),
            was_removed: false,
        });
    }

    let target_path = install::resource_path(opts.project_dir, opts.resource_type, opts.name);

    if opts.verbose {
        eprintln!("removing {}", target_path.display());
    }

    let remove_err = |e| format!("failed to remove {}: {e}", target_path.display());
    match opts.resource_type {
        ResourceType::Skill => {
            std::fs::remove_dir_all(&target_path).map_err(remove_err)?;
        }
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            std::fs::remove_file(&target_path).map_err(remove_err)?;
        }
    }

    // Clean up empty parent directories
    cleanup_empty_parents(&target_path, opts.project_dir);

    let removed_display = target_path
        .strip_prefix(opts.project_dir)
        .unwrap_or(&target_path)
        .to_string_lossy()
        .to_string();

    if !opts.json {
        println!("Removed {} '{}'", opts.resource_type, opts.name);
    }

    Ok(RemoveResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        removed_path: removed_display,
        was_removed: true,
    })
}

/// Walk up from a removed path and remove empty directories, stopping
/// at (but never removing) `stop_at`.
fn cleanup_empty_parents(removed_path: &Path, stop_at: &Path) {
    let mut current = removed_path.parent();
    while let Some(dir) = current {
        if dir == stop_at || !dir.starts_with(stop_at) {
            break;
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(e) => {
                eprintln!("[warn] could not clean up {}: {e}", dir.display());
                break;
            }
        }
        current = dir.parent();
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
    fn cleanup_empty_parents_removes_empty_dirs() {
        let root = temp_dir();
        let deep = root.path().join("a/b/c");
        fs::create_dir_all(&deep).unwrap();
        let file = deep.join("file.txt");
        fs::write(&file, "data").unwrap();
        fs::remove_file(&file).unwrap();

        cleanup_empty_parents(&file, root.path());
        assert!(!root.path().join("a").exists());
    }

    #[test]
    fn cleanup_empty_parents_preserves_nonempty() {
        let root = temp_dir();
        let dir_a = root.path().join("a");
        let dir_b = dir_a.join("b");
        fs::create_dir_all(&dir_b).unwrap();
        fs::write(dir_a.join("keep.txt"), "keep").unwrap();
        let file = dir_b.join("file.txt");
        fs::write(&file, "data").unwrap();
        fs::remove_file(&file).unwrap();

        cleanup_empty_parents(&file, root.path());
        assert!(!dir_b.exists());
        assert!(dir_a.exists());
    }

    #[test]
    fn remove_skill_deletes_directory() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("DELETE", "/api/v1/resources/skill/denden")
            .with_status(204)
            .create();

        let opts = RemoveOpts {
            server_url: &server.url(),
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert!(result.was_removed);
        assert!(!skill_dir.exists());
    }

    #[test]
    fn remove_agent_deletes_file() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("DELETE", "/api/v1/resources/agent/debugger")
            .with_status(204)
            .create();

        let opts = RemoveOpts {
            server_url: &server.url(),
            resource_type: ResourceType::Agent,
            name: "debugger",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert!(result.was_removed);
        assert!(!agents_dir.join("debugger.md").exists());
    }

    #[test]
    fn remove_not_installed_warns() {
        let root = temp_dir();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("DELETE", "/api/v1/resources/skill/nonexistent")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();

        let opts = RemoveOpts {
            server_url: &server.url(),
            resource_type: ResourceType::Skill,
            name: "nonexistent",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        let result = run(&opts).unwrap();
        assert!(!result.was_removed);
    }

    #[test]
    fn remove_cleans_empty_parents() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        let mut server = mockito::Server::new();
        let _mock = server
            .mock("DELETE", "/api/v1/resources/agent/debugger")
            .with_status(204)
            .create();

        let opts = RemoveOpts {
            server_url: &server.url(),
            resource_type: ResourceType::Agent,
            name: "debugger",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        run(&opts).unwrap();
        assert!(!agents_dir.exists());
        assert!(!root.path().join(".claude").exists());
    }

    #[test]
    fn remove_invalid_slug_errors() {
        let root = temp_dir();

        let opts = RemoveOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: ResourceType::Skill,
            name: "Invalid-Name",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        assert!(run(&opts).is_err());
    }

    #[test]
    fn remove_fails_when_server_unreachable() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = RemoveOpts {
            server_url: "http://127.0.0.1:19999",
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: false,
            verbose: false,
        };

        let err = run(&opts).unwrap_err();
        assert!(
            err.contains("Registry server not running"),
            "got: {err}"
        );
        // Files should NOT be removed when server is unreachable
        assert!(skill_dir.exists());
    }
}
