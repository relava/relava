use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Resource dependencies — parsed from `metadata.relava` in .md frontmatter
// ---------------------------------------------------------------------------

/// Dependencies extracted from a resource's .md frontmatter.
///
/// Example frontmatter:
/// ```yaml
/// ---
/// name: orchestrator
/// description: Coordinates feature development
/// metadata:
///   relava:
///     skills:
///       - notify-slack: "0.3.0"
///     agents:
///       - debugger: "0.5.0"
/// ---
/// ```
#[derive(Debug, Default, PartialEq)]
pub struct ResourceDeps {
    /// Skill dependencies: name -> version constraint
    pub skills: BTreeMap<String, String>,
    /// Agent dependencies: name -> version constraint
    pub agents: BTreeMap<String, String>,
}

/// Full frontmatter structure for deserializing .md files.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    metadata: Option<MetadataBlock>,
}

#[derive(Debug, Deserialize)]
struct MetadataBlock {
    #[serde(default)]
    relava: Option<RelavaBlock>,
}

#[derive(Debug, Deserialize)]
struct RelavaBlock {
    #[serde(default)]
    skills: Vec<serde_yaml::Value>,
    #[serde(default)]
    agents: Vec<serde_yaml::Value>,
}

/// Extract the YAML frontmatter string from markdown content.
/// Returns None if no frontmatter delimiters found.
fn extract_frontmatter(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_open = &trimmed[3..];
    let end = after_open.find("\n---")?;
    Some(&after_open[..end])
}

/// Parse a dependency list item. Supports two formats:
/// - `- name: "version"` (map with one key)
/// - `- name` (bare string, defaults to `"*"`)
fn parse_dep_item(value: &serde_yaml::Value) -> Option<(String, String)> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let (k, v) = map.iter().next()?;
            let name = k.as_str()?.to_string();
            let version = v.as_str()?.to_string();
            Some((name, version))
        }
        serde_yaml::Value::String(name) => Some((name.clone(), "*".to_string())),
        _ => None,
    }
}

impl ResourceDeps {
    /// Parse dependencies from markdown content containing YAML frontmatter.
    pub fn from_md(content: &str) -> Result<Self, ManifestError> {
        let yaml = match extract_frontmatter(content) {
            Some(y) => y,
            None => return Ok(Self::default()),
        };

        let fm: Frontmatter =
            serde_yaml::from_str(yaml).map_err(ManifestError::FrontmatterParse)?;

        let relava = match fm.metadata.and_then(|m| m.relava) {
            Some(r) => r,
            None => return Ok(Self::default()),
        };

        let mut skills = BTreeMap::new();
        for item in &relava.skills {
            if let Some((name, version)) = parse_dep_item(item) {
                skills.insert(name, version);
            }
        }

        let mut agents = BTreeMap::new();
        for item in &relava.agents {
            if let Some((name, version)) = parse_dep_item(item) {
                agents.insert(name, version);
            }
        }

        Ok(Self { skills, agents })
    }

    /// Parse dependencies from a .md file on disk.
    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        Self::from_md(&content)
    }
}

// ---------------------------------------------------------------------------
// Project manifest — parsed from project-level `relava.toml`
// ---------------------------------------------------------------------------

/// Project manifest parsed from `relava.toml` at the project root.
///
/// Example:
/// ```toml
/// [skills]
/// denden = "1.2.0"
/// notify-slack = "*"
///
/// [agents]
/// debugger = "0.5.0"
/// ```
#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    #[serde(default)]
    pub skills: BTreeMap<String, String>,

    #[serde(default)]
    pub agents: BTreeMap<String, String>,
}

impl ProjectManifest {
    pub fn from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        Self::from_str(&content).map_err(|e| ManifestError::TomlParse(path.to_path_buf(), e))
    }

    pub fn to_string_pretty(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ManifestError {
    Io(std::path::PathBuf, std::io::Error),
    TomlParse(std::path::PathBuf, toml::de::Error),
    FrontmatterParse(serde_yaml::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(path, err) => {
                write!(f, "failed to read {}: {}", path.display(), err)
            }
            ManifestError::TomlParse(path, err) => {
                write!(f, "failed to parse {}: {}", path.display(), err)
            }
            ManifestError::FrontmatterParse(err) => {
                write!(f, "failed to parse frontmatter: {}", err)
            }
        }
    }
}

impl std::error::Error for ManifestError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ResourceDeps (frontmatter) tests --

    #[test]
    fn deps_no_frontmatter() {
        let md = "# Just a heading\nSome content.";
        let deps = ResourceDeps::from_md(md).unwrap();
        assert!(deps.skills.is_empty());
        assert!(deps.agents.is_empty());
    }

    #[test]
    fn deps_no_metadata() {
        let md = "---\nname: test\ndescription: A test\n---\nBody.";
        let deps = ResourceDeps::from_md(md).unwrap();
        assert!(deps.skills.is_empty());
        assert!(deps.agents.is_empty());
    }

    #[test]
    fn deps_no_relava_block() {
        let md = "---\nname: test\nmetadata:\n  author: someone\n---\nBody.";
        let deps = ResourceDeps::from_md(md).unwrap();
        assert!(deps.skills.is_empty());
        assert!(deps.agents.is_empty());
    }

    #[test]
    fn deps_skills_only() {
        let md = r#"---
name: code-review
description: Code review skill
metadata:
  relava:
    skills:
      - notify-slack: "0.3.0"
      - style-guide: "1.0.0"
---
Instructions here.
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills.len(), 2);
        assert_eq!(deps.skills["notify-slack"], "0.3.0");
        assert_eq!(deps.skills["style-guide"], "1.0.0");
        assert!(deps.agents.is_empty());
    }

    #[test]
    fn deps_agents_only() {
        let md = r#"---
name: orchestrator
description: Orchestrator agent
metadata:
  relava:
    agents:
      - debugger: "0.5.0"
---
Body.
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert!(deps.skills.is_empty());
        assert_eq!(deps.agents.len(), 1);
        assert_eq!(deps.agents["debugger"], "0.5.0");
    }

    #[test]
    fn deps_both() {
        let md = r#"---
name: orchestrator
description: Full orchestrator
metadata:
  relava:
    skills:
      - notify-slack: "0.3.0"
    agents:
      - debugger: "0.5.0"
---
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills.len(), 1);
        assert_eq!(deps.agents.len(), 1);
    }

    #[test]
    fn deps_bare_name_defaults_to_star() {
        let md = r#"---
name: test
metadata:
  relava:
    skills:
      - notify-slack
---
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills["notify-slack"], "*");
    }

    #[test]
    fn deps_ignores_unknown_metadata_keys() {
        let md = r#"---
name: test
metadata:
  author: someone
  relava:
    skills:
      - foo: "1.0.0"
  other-tool:
    key: value
---
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills.len(), 1);
        assert_eq!(deps.skills["foo"], "1.0.0");
    }

    // -- ProjectManifest (TOML) tests --

    #[test]
    fn project_empty() {
        let manifest = ProjectManifest::from_str("").unwrap();
        assert!(manifest.skills.is_empty());
        assert!(manifest.agents.is_empty());
    }

    #[test]
    fn project_skills_only() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"
strawpot-recap = "1.0.0"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills.len(), 2);
        assert_eq!(manifest.skills["notify-slack"], "0.3.0");
    }

    #[test]
    fn project_both() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"

[agents]
debugger = "0.5.0"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills.len(), 1);
        assert_eq!(manifest.agents.len(), 1);
    }

    #[test]
    fn project_version_constraints() {
        let toml = r#"
[skills]
exact = "1.2.0"
explicit-exact = "==1.2.0"
latest = "*"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills["exact"], "1.2.0");
        assert_eq!(manifest.skills["explicit-exact"], "==1.2.0");
        assert_eq!(manifest.skills["latest"], "*");
    }

    #[test]
    fn project_roundtrip() {
        let toml = r#"
[skills]
notify-slack = "0.3.0"

[agents]
debugger = "0.5.0"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        let output = manifest.to_string_pretty().unwrap();
        let reparsed = ProjectManifest::from_str(&output).unwrap();
        assert_eq!(manifest, reparsed);
    }

    #[test]
    fn project_unknown_sections_rejected() {
        let toml = r#"
[resource]
name = "foo"
"#;
        assert!(ProjectManifest::from_str(toml).is_err());
    }
}
