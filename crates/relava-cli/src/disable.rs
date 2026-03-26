use std::path::Path;

use relava_types::validate::{self, ResourceType};

use crate::install;

/// Options for the disable command.
pub struct DisableOpts<'a> {
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// Result of the disable command, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct DisableResult {
    pub resource_type: String,
    pub name: String,
    pub disabled_path: String,
    /// Whether the resource was actually disabled (false if already disabled).
    pub was_disabled: bool,
}

/// Run `relava disable <type> <name>`.
///
/// Moves the installed resource file or directory into a `.disabled/`
/// subdirectory within the type directory. Disabled resources are not
/// discovered by Claude Code since they no longer reside at expected paths.
pub fn run(opts: &DisableOpts) -> Result<DisableResult, String> {
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    let active_path = install::resource_path(opts.project_dir, opts.resource_type, opts.name);
    let disabled_path = disabled_path_for(opts.project_dir, opts.resource_type, opts.name);

    // Conflict: both active and disabled versions exist
    if active_path.exists() && disabled_path.exists() {
        return Err(format!(
            "conflict: both active and disabled versions of {} '{}' exist; resolve manually",
            opts.resource_type, opts.name
        ));
    }

    // Already disabled?
    if disabled_path.exists() {
        if !opts.json {
            println!("{} '{}' is already disabled", opts.resource_type, opts.name);
        }
        let display = relative_display(&disabled_path, opts.project_dir);
        return Ok(DisableResult {
            resource_type: opts.resource_type.to_string(),
            name: opts.name.to_string(),
            disabled_path: display,
            was_disabled: false,
        });
    }

    // Must be installed (active) to disable
    if !active_path.exists() {
        return Err(format!(
            "{} '{}' is not installed",
            opts.resource_type, opts.name
        ));
    }

    // Ensure the .disabled/ directory exists
    let disabled_dir = disabled_dir_for(opts.project_dir, opts.resource_type);
    std::fs::create_dir_all(&disabled_dir).map_err(|e| {
        format!(
            "failed to create .disabled directory at {}: {e}",
            disabled_dir.display()
        )
    })?;

    if opts.verbose {
        eprintln!(
            "moving {} -> {}",
            active_path.display(),
            disabled_path.display()
        );
    }

    std::fs::rename(&active_path, &disabled_path).map_err(|e| {
        format!(
            "failed to disable {} '{}': {e}",
            opts.resource_type, opts.name
        )
    })?;

    let display = relative_display(&disabled_path, opts.project_dir);

    if !opts.json {
        println!("Disabled {} '{}'", opts.resource_type, opts.name);
    }

    Ok(DisableResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        disabled_path: display,
        was_disabled: true,
    })
}

/// Return the `.disabled/` subdirectory path for a resource type.
pub(crate) fn disabled_dir_for(
    project_dir: &Path,
    resource_type: ResourceType,
) -> std::path::PathBuf {
    install::type_dir(project_dir, resource_type).join(".disabled")
}

/// Compute the path inside the `.disabled/` subdirectory for a resource.
///
/// Skills: `<type_dir>/.disabled/<name>/`
/// Others: `<type_dir>/.disabled/<name>.md`
pub fn disabled_path_for(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
) -> std::path::PathBuf {
    let disabled_dir = disabled_dir_for(project_dir, resource_type);
    match resource_type {
        ResourceType::Skill => disabled_dir.join(name),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            disabled_dir.join(format!("{name}.md"))
        }
    }
}

/// Check if a resource is disabled (exists inside the `.disabled/` subdirectory).
#[cfg(test)]
pub fn is_disabled(project_dir: &Path, resource_type: ResourceType, name: &str) -> bool {
    disabled_path_for(project_dir, resource_type, name).exists()
}

/// Format a path relative to the project directory for display.
pub(crate) fn relative_display(path: &Path, project_dir: &Path) -> String {
    path.strip_prefix(project_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
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
    fn disable_skill() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_disabled);
        assert!(!skill_dir.exists());
        assert!(root.path().join(".claude/skills/.disabled/denden").exists());
        assert!(
            root.path()
                .join(".claude/skills/.disabled/denden/SKILL.md")
                .exists()
        );
    }

    #[test]
    fn disable_agent() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md"), "# Debugger").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Agent,
            name: "debugger",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_disabled);
        assert!(!agents_dir.join("debugger.md").exists());
        assert!(agents_dir.join(".disabled/debugger.md").exists());
    }

    #[test]
    fn disable_command() {
        let root = temp_dir();
        let cmds_dir = root.path().join(".claude/commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("deploy.md"), "# Deploy").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Command,
            name: "deploy",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_disabled);
        assert!(!cmds_dir.join("deploy.md").exists());
        assert!(cmds_dir.join(".disabled/deploy.md").exists());
    }

    #[test]
    fn disable_rule() {
        let root = temp_dir();
        let rules_dir = root.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("no-console-log.md"), "# Rule").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Rule,
            name: "no-console-log",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_disabled);
        assert!(!rules_dir.join("no-console-log.md").exists());
        assert!(rules_dir.join(".disabled/no-console-log.md").exists());
    }

    #[test]
    fn disable_already_disabled_is_noop() {
        let root = temp_dir();
        let disabled_dir = root.path().join(".claude/skills/.disabled/denden");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(!result.was_disabled);
        // disabled path still exists
        assert!(disabled_dir.exists());
    }

    #[test]
    fn disable_not_installed_errors() {
        let root = temp_dir();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "nonexistent",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not installed"));
    }

    #[test]
    fn disable_invalid_slug_errors() {
        let root = temp_dir();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "../traversal",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
    }

    #[test]
    fn disable_conflict_both_exist_errors() {
        let root = temp_dir();

        // Create both active and disabled versions
        let active = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("SKILL.md"), "# Active").unwrap();

        let disabled = root.path().join(".claude/skills/.disabled/denden");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("SKILL.md"), "# Disabled").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("conflict"));
    }

    #[test]
    fn disable_preserves_skill_contents() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/myskill");
        fs::create_dir_all(skill_dir.join("templates")).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# MySkill").unwrap();
        fs::write(skill_dir.join("templates/tmpl.md"), "template").unwrap();

        let opts = DisableOpts {
            resource_type: ResourceType::Skill,
            name: "myskill",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        run(&opts).unwrap();

        let disabled = root.path().join(".claude/skills/.disabled/myskill");
        assert!(disabled.join("SKILL.md").exists());
        assert!(disabled.join("templates/tmpl.md").exists());
    }

    #[test]
    fn is_disabled_returns_true_for_disabled_skill() {
        let root = temp_dir();
        let disabled = root.path().join(".claude/skills/.disabled/denden");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("SKILL.md"), "# Denden").unwrap();

        assert!(is_disabled(root.path(), ResourceType::Skill, "denden"));
    }

    #[test]
    fn is_disabled_returns_false_for_active_resource() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        assert!(!is_disabled(root.path(), ResourceType::Skill, "denden"));
    }

    #[test]
    fn is_disabled_returns_true_for_disabled_agent() {
        let root = temp_dir();
        let disabled_dir = root.path().join(".claude/agents/.disabled");
        fs::create_dir_all(&disabled_dir).unwrap();
        fs::write(disabled_dir.join("debugger.md"), "# Debugger").unwrap();

        assert!(is_disabled(root.path(), ResourceType::Agent, "debugger"));
    }

    #[test]
    fn disable_result_serializes_to_json() {
        let result = DisableResult {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            disabled_path: ".claude/skills/.disabled/denden".to_string(),
            was_disabled: true,
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("was_disabled"));
        assert!(json.contains("true"));
    }
}
