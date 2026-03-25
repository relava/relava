use std::collections::BTreeMap;
use std::path::Path;

use crate::manifest::EnvSpec;

/// Status of an env var check.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvStatus {
    /// Found in the process environment.
    FoundInEnv,
    /// Found in `.claude/settings.json` `env` entries.
    FoundInSettings,
    /// Missing and required.
    MissingRequired,
    /// Missing but optional.
    MissingOptional,
}

/// Result of checking a single env var.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EnvResult {
    pub name: String,
    pub description: String,
    pub required: bool,
    pub status: EnvStatus,
}

/// Read env vars from `.claude/settings.json`.
///
/// Looks for the `env` key at the top level, which maps var names to values.
/// Returns an empty map if the file is missing or has no `env` key.
/// Warns on IO errors (other than not-found) and JSON parse failures.
fn read_settings_env(project_root: &Path) -> BTreeMap<String, String> {
    let settings_path = project_root.join(".claude").join("settings.json");

    let content = match std::fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(e) => {
            eprintln!("warning: cannot read {}: {e}", settings_path.display());
            return BTreeMap::new();
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("warning: failed to parse {}: {e}", settings_path.display());
            return BTreeMap::new();
        }
    };

    parsed
        .get("env")
        .and_then(|v| v.as_object())
        .map(|env_obj| {
            env_obj
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Check all declared env vars against the process environment and settings.json.
///
/// Returns a result per env var. Missing vars produce warnings, not errors.
pub fn check_env_vars(
    env_specs: &BTreeMap<String, EnvSpec>,
    project_root: &Path,
) -> Vec<EnvResult> {
    let settings_env = read_settings_env(project_root);
    let mut results = Vec::new();

    for (name, spec) in env_specs {
        let in_process = std::env::var_os(name).is_some();
        let in_settings = settings_env.contains_key(name);

        let status = if in_process {
            EnvStatus::FoundInEnv
        } else if in_settings {
            EnvStatus::FoundInSettings
        } else if spec.required {
            EnvStatus::MissingRequired
        } else {
            EnvStatus::MissingOptional
        };

        results.push(EnvResult {
            name: name.clone(),
            description: spec.description.clone(),
            required: spec.required,
            status,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    fn make_settings(dir: &Path, env_json: &str) {
        let claude_dir = dir.join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        let settings = format!(r#"{{"env": {env_json}}}"#);
        fs::write(claude_dir.join("settings.json"), settings).unwrap();
    }

    #[test]
    fn check_missing_required() {
        let project = temp_dir();
        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_MISSING_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "test var".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, EnvStatus::MissingRequired);
        assert!(results[0].required);
    }

    #[test]
    fn check_missing_optional() {
        let project = temp_dir();
        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_OPTIONAL_VAR_12345".to_string(),
            EnvSpec {
                required: false,
                description: "optional var".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, EnvStatus::MissingOptional);
    }

    #[test]
    fn check_found_in_process_env() {
        let project = temp_dir();
        // PATH is always set
        let mut specs = BTreeMap::new();
        specs.insert(
            "PATH".to_string(),
            EnvSpec {
                required: true,
                description: "system path".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, EnvStatus::FoundInEnv);
    }

    #[test]
    fn check_found_in_settings_json() {
        let project = temp_dir();
        make_settings(
            project.path(),
            r#"{"RELAVA_TEST_SETTINGS_VAR_12345": "value"}"#,
        );

        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_SETTINGS_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "from settings".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, EnvStatus::FoundInSettings);
    }

    #[test]
    fn check_process_env_takes_precedence_over_settings() {
        let project = temp_dir();
        // PATH exists in process env AND we put it in settings
        make_settings(project.path(), r#"{"PATH": "override"}"#);

        let mut specs = BTreeMap::new();
        specs.insert(
            "PATH".to_string(),
            EnvSpec {
                required: true,
                description: "system path".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results[0].status, EnvStatus::FoundInEnv);
    }

    #[test]
    fn check_no_settings_file() {
        let project = temp_dir();
        // No .claude/settings.json at all
        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_NO_SETTINGS_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "missing".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results[0].status, EnvStatus::MissingRequired);
    }

    #[test]
    fn check_invalid_settings_json() {
        let project = temp_dir();
        let claude_dir = project.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("settings.json"), "not json").unwrap();

        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_BAD_JSON_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "missing".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results[0].status, EnvStatus::MissingRequired);
    }

    #[test]
    fn check_settings_no_env_key() {
        let project = temp_dir();
        let claude_dir = project.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("settings.json"), r#"{"other": "key"}"#).unwrap();

        let mut specs = BTreeMap::new();
        specs.insert(
            "RELAVA_TEST_NO_ENV_KEY_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "missing".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results[0].status, EnvStatus::MissingRequired);
    }

    #[test]
    fn check_empty_specs() {
        let project = temp_dir();
        let specs = BTreeMap::new();
        let results = check_env_vars(&specs, project.path());
        assert!(results.is_empty());
    }

    #[test]
    fn check_multiple_vars_mixed() {
        let project = temp_dir();
        make_settings(
            project.path(),
            r#"{"RELAVA_TEST_IN_SETTINGS_VAR_12345": "val"}"#,
        );

        let mut specs = BTreeMap::new();
        // This one is in process env
        specs.insert(
            "PATH".to_string(),
            EnvSpec {
                required: true,
                description: "path".to_string(),
            },
        );
        // This one is in settings
        specs.insert(
            "RELAVA_TEST_IN_SETTINGS_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "in settings".to_string(),
            },
        );
        // This one is missing and required
        specs.insert(
            "RELAVA_TEST_TOTALLY_MISSING_VAR_12345".to_string(),
            EnvSpec {
                required: true,
                description: "missing".to_string(),
            },
        );
        // This one is missing but optional
        specs.insert(
            "RELAVA_TEST_OPTIONAL_MISSING_VAR_12345".to_string(),
            EnvSpec {
                required: false,
                description: "optional".to_string(),
            },
        );

        let results = check_env_vars(&specs, project.path());
        assert_eq!(results.len(), 4);

        let path_result = results.iter().find(|r| r.name == "PATH").unwrap();
        assert_eq!(path_result.status, EnvStatus::FoundInEnv);

        let settings_result = results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_IN_SETTINGS_VAR_12345")
            .unwrap();
        assert_eq!(settings_result.status, EnvStatus::FoundInSettings);

        let missing_req = results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_TOTALLY_MISSING_VAR_12345")
            .unwrap();
        assert_eq!(missing_req.status, EnvStatus::MissingRequired);

        let missing_opt = results
            .iter()
            .find(|r| r.name == "RELAVA_TEST_OPTIONAL_MISSING_VAR_12345")
            .unwrap();
        assert_eq!(missing_opt.status, EnvStatus::MissingOptional);
    }
}
