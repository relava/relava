use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Resource dependencies — parsed from `metadata.relava` in .md frontmatter
// ---------------------------------------------------------------------------

/// Dependencies extracted from a resource's .md frontmatter.
///
/// Frontmatter dependencies are names only — no version pins.
/// Version control belongs at the project level (relava.toml).
///
/// Example frontmatter:
/// ```yaml
/// ---
/// name: orchestrator
/// description: Coordinates feature development
/// metadata:
///   relava:
///     skills:
///       - notify-slack
///       - code-review
///     agents:
///       - debugger
/// ---
/// ```
#[derive(Debug, Default, PartialEq)]
pub struct ResourceDeps {
    /// Skill dependency names
    pub skills: Vec<String>,
    /// Agent dependency names
    pub agents: Vec<String>,
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
    skills: Vec<String>,
    #[serde(default)]
    agents: Vec<String>,
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

        Ok(Self {
            skills: relava.skills,
            agents: relava.agents,
        })
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
      - notify-slack
      - style-guide
---
Instructions here.
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills, vec!["notify-slack", "style-guide"]);
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
      - debugger
---
Body.
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert!(deps.skills.is_empty());
        assert_eq!(deps.agents, vec!["debugger"]);
    }

    #[test]
    fn deps_both() {
        let md = r#"---
name: orchestrator
description: Full orchestrator
metadata:
  relava:
    skills:
      - notify-slack
    agents:
      - debugger
---
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills, vec!["notify-slack"]);
        assert_eq!(deps.agents, vec!["debugger"]);
    }

    #[test]
    fn deps_ignores_unknown_metadata_keys() {
        let md = r#"---
name: test
metadata:
  author: someone
  relava:
    skills:
      - foo
  other-tool:
    key: value
---
"#;
        let deps = ResourceDeps::from_md(md).unwrap();
        assert_eq!(deps.skills, vec!["foo"]);
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
latest = "*"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        assert_eq!(manifest.skills["exact"], "1.2.0");
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
