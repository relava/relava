use std::path::Path;

use relava_types::manifest::ProjectManifest;
use relava_types::validate::ResourceType;

/// Add a resource entry to relava.toml.
///
/// If relava.toml does not exist, prints a warning and returns Ok.
/// Creates the appropriate section (skills/agents/commands/rules) if missing.
pub fn add_to_manifest(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
    version: &str,
    json: bool,
) -> Result<(), String> {
    let toml_path = project_dir.join("relava.toml");
    if !toml_path.exists() {
        if !json {
            eprintln!("[warn] relava.toml not found — skipping --save");
        }
        return Ok(());
    }

    let mut manifest = ProjectManifest::from_file(&toml_path)
        .map_err(|e| format!("failed to read relava.toml: {e}"))?;

    let section = manifest_section(&mut manifest, resource_type);
    section.insert(name.to_string(), version.to_string());

    write_manifest(&toml_path, &manifest)?;

    if !json {
        println!("  [save]    Added {name} = \"{version}\" to relava.toml [{resource_type}s]");
    }

    Ok(())
}

/// Remove a resource entry from relava.toml.
///
/// If relava.toml does not exist, prints a warning and returns Ok.
/// If the entry does not exist in the manifest, this is a no-op.
pub fn remove_from_manifest(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
    json: bool,
) -> Result<(), String> {
    let toml_path = project_dir.join("relava.toml");
    if !toml_path.exists() {
        if !json {
            eprintln!("[warn] relava.toml not found — skipping --save");
        }
        return Ok(());
    }

    let mut manifest = ProjectManifest::from_file(&toml_path)
        .map_err(|e| format!("failed to read relava.toml: {e}"))?;

    let section = manifest_section(&mut manifest, resource_type);
    if section.remove(name).is_none() {
        // Entry wasn't in the manifest — nothing to do
        return Ok(());
    }

    write_manifest(&toml_path, &manifest)?;

    if !json {
        println!("  [save]    Removed {name} from relava.toml [{resource_type}s]");
    }

    Ok(())
}

/// Get a mutable reference to the appropriate section in the manifest.
fn manifest_section(
    manifest: &mut ProjectManifest,
    resource_type: ResourceType,
) -> &mut std::collections::BTreeMap<String, String> {
    match resource_type {
        ResourceType::Skill => &mut manifest.skills,
        ResourceType::Agent => &mut manifest.agents,
        ResourceType::Command => &mut manifest.commands,
        ResourceType::Rule => &mut manifest.rules,
    }
}

/// Write the manifest back to disk.
fn write_manifest(path: &Path, manifest: &ProjectManifest) -> Result<(), String> {
    let content = manifest
        .to_string_pretty()
        .map_err(|e| format!("failed to serialize relava.toml: {e}"))?;
    std::fs::write(path, content).map_err(|e| format!("failed to write {}: {e}", path.display()))
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
    fn add_to_empty_manifest() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "").unwrap();

        add_to_manifest(root.path(), ResourceType::Skill, "denden", "1.2.0", false).unwrap();

        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert_eq!(manifest.skills["denden"], "1.2.0");
    }

    #[test]
    fn add_to_existing_section() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\nexisting = \"1.0.0\"\n",
        )
        .unwrap();

        add_to_manifest(root.path(), ResourceType::Skill, "denden", "2.0.0", false).unwrap();

        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert_eq!(manifest.skills["existing"], "1.0.0");
        assert_eq!(manifest.skills["denden"], "2.0.0");
    }

    #[test]
    fn add_overwrites_existing_version() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\n",
        )
        .unwrap();

        add_to_manifest(root.path(), ResourceType::Skill, "denden", "2.0.0", false).unwrap();

        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert_eq!(manifest.skills["denden"], "2.0.0");
    }

    #[test]
    fn add_different_resource_types() {
        let root = temp_dir();
        fs::write(root.path().join("relava.toml"), "").unwrap();

        add_to_manifest(root.path(), ResourceType::Agent, "debugger", "0.5.0", false).unwrap();
        add_to_manifest(root.path(), ResourceType::Command, "deploy", "1.0.0", false).unwrap();
        add_to_manifest(
            root.path(),
            ResourceType::Rule,
            "no-console-log",
            "1.0.0",
            false,
        )
        .unwrap();

        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert_eq!(manifest.agents["debugger"], "0.5.0");
        assert_eq!(manifest.commands["deploy"], "1.0.0");
        assert_eq!(manifest.rules["no-console-log"], "1.0.0");
    }

    #[test]
    fn add_no_toml_warns_no_error() {
        let root = temp_dir();
        // No relava.toml created
        let result = add_to_manifest(root.path(), ResourceType::Skill, "denden", "1.0.0", false);
        assert!(result.is_ok());
    }

    #[test]
    fn remove_entry_from_manifest() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\ndenden = \"1.0.0\"\nother = \"2.0.0\"\n",
        )
        .unwrap();

        remove_from_manifest(root.path(), ResourceType::Skill, "denden", false).unwrap();

        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert!(!manifest.skills.contains_key("denden"));
        assert_eq!(manifest.skills["other"], "2.0.0");
    }

    #[test]
    fn remove_nonexistent_entry_is_noop() {
        let root = temp_dir();
        fs::write(
            root.path().join("relava.toml"),
            "[skills]\nother = \"1.0.0\"\n",
        )
        .unwrap();

        let result = remove_from_manifest(root.path(), ResourceType::Skill, "nonexistent", false);
        assert!(result.is_ok());

        // File should be unchanged
        let content = fs::read_to_string(root.path().join("relava.toml")).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert_eq!(manifest.skills.len(), 1);
    }

    #[test]
    fn remove_no_toml_warns_no_error() {
        let root = temp_dir();
        let result = remove_from_manifest(root.path(), ResourceType::Skill, "denden", false);
        assert!(result.is_ok());
    }
}
