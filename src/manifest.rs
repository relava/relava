use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Resource manifest parsed from a per-resource `relava.toml`.
///
/// Example:
/// ```toml
/// [skills]
/// notify-slack = "0.3.0"
///
/// [agents]
/// debugger = "0.5.0"
/// ```
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResourceManifest {
    /// Skill dependencies: name -> version constraint
    #[serde(default)]
    pub skills: BTreeMap<String, String>,

    /// Agent dependencies: name -> version constraint
    #[serde(default)]
    pub agents: BTreeMap<String, String>,
}

impl ResourceManifest {
    pub fn from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        Self::from_str(&content).map_err(|e| ManifestError::Parse(path.to_path_buf(), e))
    }

    pub fn to_string_pretty(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::path::PathBuf, std::io::Error),
    Parse(std::path::PathBuf, toml::de::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(path, err) => {
                write!(f, "failed to read {}: {}", path.display(), err)
            }
            ManifestError::Parse(path, err) => {
                write!(f, "failed to parse {}: {}", path.display(), err)
            }
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        let manifest = ResourceManifest::from_str("").unwrap();
        assert!(manifest.skills.is_empty());
        assert!(manifest.agents.is_empty());
    }

    #[test]
    fn parse_skills_only() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"
strawpot-recap = "1.0.0"
"#;
        let manifest = ResourceManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills.len(), 2);
        assert_eq!(manifest.skills["notify-slack"], "0.3.0");
        assert_eq!(manifest.skills["strawpot-recap"], "1.0.0");
        assert!(manifest.agents.is_empty());
    }

    #[test]
    fn parse_agents_only() {
        let toml = r#"
[agents]
debugger = "0.5.0"
"#;
        let manifest = ResourceManifest::from_str(toml).unwrap();
        assert!(manifest.skills.is_empty());
        assert_eq!(manifest.agents.len(), 1);
        assert_eq!(manifest.agents["debugger"], "0.5.0");
    }

    #[test]
    fn parse_both_sections() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"

[agents]
debugger = "0.5.0"
"#;
        let manifest = ResourceManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.agents.len(), 1);
    }

    #[test]
    fn parse_version_constraints() {
        let toml = r#"
[skills]
exact = "1.2.0"
explicit-exact = "==1.2.0"
latest = "*"
"#;
        let manifest = ResourceManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills["exact"], "1.2.0");
        assert_eq!(manifest.skills["explicit-exact"], "==1.2.0");
        assert_eq!(manifest.skills["latest"], "*");
    }

    #[test]
    fn roundtrip() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"

[agents]
debugger = "0.5.0"
"#;
        let manifest = ResourceManifest::from_str(toml).unwrap();
        let output = manifest.to_string_pretty().unwrap();
        let reparsed = ResourceManifest::from_str(&output).unwrap();
        assert_eq!(manifest, reparsed);
    }

    #[test]
    fn unknown_sections_are_rejected() {
        let toml = r#"
[resource]
name = "foo"
"#;
        assert!(ResourceManifest::from_str(toml).is_err());
    }
}
