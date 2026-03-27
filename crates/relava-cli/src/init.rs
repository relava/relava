use std::path::Path;

/// The default content for a new `relava.toml` manifest.
///
/// Uses a hand-written template rather than serializing `ProjectManifest::default()`
/// so the output includes comments and all section headers — even when empty.
const DEFAULT_MANIFEST: &str = r#"# Relava project manifest
# Docs: https://github.com/relava/relava

# Target agent platform — determines install paths and supported features.
# Supported: "claude". Future: "codex", "gemini".
# agent_type = "claude"

[skills]

[agents]

[commands]

[rules]
"#;

const MANIFEST_FILE: &str = "relava.toml";

/// Run `relava init` in `project_dir`.
///
/// Creates a `relava.toml` with empty sections. Returns an error message on
/// failure; prints status to stdout on success.
pub fn run(project_dir: &Path) -> Result<(), String> {
    let manifest_path = project_dir.join(MANIFEST_FILE);

    if manifest_path.exists() {
        return Err(format!(
            "{} already exists — not overwriting",
            manifest_path.display()
        ));
    }

    std::fs::write(&manifest_path, DEFAULT_MANIFEST)
        .map_err(|e| format!("failed to write {}: {}", manifest_path.display(), e))?;

    println!("Created {}", manifest_path.display());
    println!("Tip: run 'relava install skill relava --save' to teach Claude Code about relava.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relava_types::manifest::ProjectManifest;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    #[test]
    fn creates_manifest_in_empty_dir() {
        let dir = temp_dir();
        run(dir.path()).expect("init should succeed");

        let path = dir.path().join(MANIFEST_FILE);
        assert!(path.exists(), "relava.toml should exist");

        // The generated file must parse as a valid ProjectManifest
        let content = fs::read_to_string(&path).unwrap();
        let manifest = ProjectManifest::from_str(&content).unwrap();
        assert!(manifest.skills.is_empty());
        assert!(manifest.agents.is_empty());
        assert!(manifest.commands.is_empty());
        assert!(manifest.rules.is_empty());
        assert!(manifest.agent_type.is_none());
    }

    #[test]
    fn refuses_to_overwrite_existing() {
        let dir = temp_dir();
        let path = dir.path().join(MANIFEST_FILE);
        fs::write(&path, "existing content").unwrap();

        let result = run(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));

        // Original content must be preserved
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "existing content");
    }

    #[test]
    fn template_contains_all_sections() {
        let content = DEFAULT_MANIFEST;
        assert!(content.contains("[skills]"));
        assert!(content.contains("[agents]"));
        assert!(content.contains("[commands]"));
        assert!(content.contains("[rules]"));
    }

    #[test]
    fn hint_text_is_in_source() {
        // Verify the hint about the relava skill is present in the init module.
        // The hint is printed unconditionally on the success path, so as long as
        // this string exists in the compiled binary, it will be shown to users.
        let source = include_str!("init.rs");
        assert!(
            source.contains("relava install skill relava --save"),
            "init module should contain hint about installing the relava skill"
        );
    }
}
