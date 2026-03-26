use std::path::Path;

use relava_types::validate::{self, ResourceType};

use crate::disable;
use crate::install;

/// Options for the enable command.
pub struct EnableOpts<'a> {
    pub resource_type: ResourceType,
    pub name: &'a str,
    pub project_dir: &'a Path,
    pub json: bool,
    pub verbose: bool,
}

/// Result of the enable command, used for JSON output.
#[derive(Debug, serde::Serialize)]
pub struct EnableResult {
    pub resource_type: String,
    pub name: String,
    pub restored_path: String,
    /// Whether the resource was actually enabled (false if already active).
    pub was_enabled: bool,
}

/// Run `relava enable <type> <name>`.
///
/// Removes the `.disabled` suffix from the resource file or directory,
/// restoring it so Claude Code can discover it again.
pub fn run(opts: &EnableOpts) -> Result<EnableResult, String> {
    validate::validate_slug(opts.name).map_err(|e| e.to_string())?;

    let active_path = install::resource_path(opts.project_dir, opts.resource_type, opts.name);
    let disabled_path = disable::disabled_path_for(opts.project_dir, opts.resource_type, opts.name);

    // Already active?
    if active_path.exists() {
        if !opts.json {
            println!("{} '{}' is already enabled", opts.resource_type, opts.name);
        }
        let display = relative_display(&active_path, opts.project_dir);
        return Ok(EnableResult {
            resource_type: opts.resource_type.to_string(),
            name: opts.name.to_string(),
            restored_path: display,
            was_enabled: false,
        });
    }

    // Must be disabled to enable
    if !disabled_path.exists() {
        return Err(format!(
            "{} '{}' is not installed",
            opts.resource_type, opts.name
        ));
    }

    if opts.verbose {
        eprintln!(
            "renaming {} -> {}",
            disabled_path.display(),
            active_path.display()
        );
    }

    std::fs::rename(&disabled_path, &active_path).map_err(|e| {
        format!(
            "failed to enable {} '{}': {e}",
            opts.resource_type, opts.name
        )
    })?;

    let display = relative_display(&active_path, opts.project_dir);

    if !opts.json {
        println!("Enabled {} '{}'", opts.resource_type, opts.name);
    }

    Ok(EnableResult {
        resource_type: opts.resource_type.to_string(),
        name: opts.name.to_string(),
        restored_path: display,
        was_enabled: true,
    })
}

/// Format a path relative to the project directory for display.
fn relative_display(path: &Path, project_dir: &Path) -> String {
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
    fn enable_disabled_skill() {
        let root = temp_dir();
        let disabled = root.path().join(".claude/skills/denden.disabled");
        fs::create_dir_all(&disabled).unwrap();
        fs::write(disabled.join("SKILL.md"), "# Denden").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_enabled);
        assert!(!disabled.exists());
        assert!(root.path().join(".claude/skills/denden/SKILL.md").exists());
    }

    #[test]
    fn enable_disabled_agent() {
        let root = temp_dir();
        let agents_dir = root.path().join(".claude/agents");
        fs::create_dir_all(&agents_dir).unwrap();
        fs::write(agents_dir.join("debugger.md.disabled"), "# Debugger").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Agent,
            name: "debugger",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_enabled);
        assert!(!agents_dir.join("debugger.md.disabled").exists());
        assert!(agents_dir.join("debugger.md").exists());
    }

    #[test]
    fn enable_disabled_command() {
        let root = temp_dir();
        let cmds_dir = root.path().join(".claude/commands");
        fs::create_dir_all(&cmds_dir).unwrap();
        fs::write(cmds_dir.join("deploy.md.disabled"), "# Deploy").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Command,
            name: "deploy",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_enabled);
        assert!(cmds_dir.join("deploy.md").exists());
    }

    #[test]
    fn enable_disabled_rule() {
        let root = temp_dir();
        let rules_dir = root.path().join(".claude/rules");
        fs::create_dir_all(&rules_dir).unwrap();
        fs::write(rules_dir.join("no-console-log.md.disabled"), "# Rule").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Rule,
            name: "no-console-log",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(result.was_enabled);
        assert!(rules_dir.join("no-console-log.md").exists());
    }

    #[test]
    fn enable_already_active_is_noop() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let result = run(&opts).unwrap();
        assert!(!result.was_enabled);
        assert!(skill_dir.exists());
    }

    #[test]
    fn enable_not_installed_errors() {
        let root = temp_dir();

        let opts = EnableOpts {
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
    fn enable_invalid_slug_errors() {
        let root = temp_dir();

        let opts = EnableOpts {
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
    fn enable_preserves_skill_contents() {
        let root = temp_dir();
        let disabled = root.path().join(".claude/skills/myskill.disabled");
        fs::create_dir_all(disabled.join("templates")).unwrap();
        fs::write(disabled.join("SKILL.md"), "# MySkill").unwrap();
        fs::write(disabled.join("templates/tmpl.md"), "template").unwrap();

        let opts = EnableOpts {
            resource_type: ResourceType::Skill,
            name: "myskill",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        run(&opts).unwrap();

        let active = root.path().join(".claude/skills/myskill");
        assert!(active.join("SKILL.md").exists());
        assert!(active.join("templates/tmpl.md").exists());
    }

    #[test]
    fn enable_result_serializes_to_json() {
        let result = EnableResult {
            resource_type: "skill".to_string(),
            name: "denden".to_string(),
            restored_path: ".claude/skills/denden".to_string(),
            was_enabled: true,
        };
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("denden"));
        assert!(json.contains("was_enabled"));
        assert!(json.contains("true"));
    }

    #[test]
    fn round_trip_disable_then_enable() {
        let root = temp_dir();
        let skill_dir = root.path().join(".claude/skills/denden");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        // Disable
        let disable_opts = crate::disable::DisableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let dr = crate::disable::run(&disable_opts).unwrap();
        assert!(dr.was_disabled);
        assert!(!skill_dir.exists());

        // Enable
        let enable_opts = EnableOpts {
            resource_type: ResourceType::Skill,
            name: "denden",
            project_dir: root.path(),
            json: true,
            verbose: false,
        };
        let er = run(&enable_opts).unwrap();
        assert!(er.was_enabled);
        assert!(skill_dir.join("SKILL.md").exists());
    }
}
