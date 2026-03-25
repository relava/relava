#![allow(dead_code)]
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Resource metadata — parsed from `metadata.relava` in .md frontmatter
// ---------------------------------------------------------------------------

/// A system tool dependency with OS-specific install commands.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct ToolSpec {
    pub description: String,
    #[serde(default)]
    pub install: BTreeMap<String, String>, // os -> command (e.g., "macos" -> "brew install gh")
}

/// An environment variable requirement.
#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
pub struct EnvSpec {
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: String,
}

/// Resource metadata extracted from a resource's .md frontmatter.
///
/// Example frontmatter:
/// ```yaml
/// ---
/// name: code-review
/// description: Code review with security checks
/// metadata:
///   relava:
///     skills:
///       - security-baseline
///     tools:
///       gh:
///         description: GitHub CLI
///         install:
///           macos: brew install gh
///     env:
///       GITHUB_TOKEN:
///         required: true
///         description: GitHub API token
/// ---
/// ```
#[derive(Debug, Default, PartialEq)]
pub struct ResourceMeta {
    /// Skill dependency names
    pub skills: Vec<String>,
    /// Agent dependency names
    pub agents: Vec<String>,
    /// System tool dependencies
    pub tools: BTreeMap<String, ToolSpec>,
    /// Environment variable requirements
    pub env: BTreeMap<String, EnvSpec>,
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
    #[serde(default)]
    tools: BTreeMap<String, ToolSpec>,
    #[serde(default)]
    env: BTreeMap<String, EnvSpec>,
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

impl ResourceMeta {
    /// Parse resource metadata from markdown content containing YAML frontmatter.
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
            tools: relava.tools,
            env: relava.env,
        })
    }

    /// Parse resource metadata from a .md file on disk.
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
/// agent_type = "claude"
///
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
    /// Target agent platform — determines install paths.
    /// Supported: "claude". Future: "codex", "gemini".
    #[serde(default)]
    pub agent_type: Option<String>,

    #[serde(default)]
    pub skills: BTreeMap<String, String>,

    #[serde(default)]
    pub agents: BTreeMap<String, String>,
}

impl ProjectManifest {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn from_file(path: &Path) -> Result<Self, ManifestError> {
        let content =
            std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        Self::from_str(&content)
            .map_err(|e| ManifestError::TomlParse(Box::new((path.to_path_buf(), e))))
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
    TomlParse(Box<(std::path::PathBuf, toml::de::Error)>),
    FrontmatterParse(serde_yaml::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Io(path, err) => {
                write!(f, "failed to read {}: {}", path.display(), err)
            }
            ManifestError::TomlParse(boxed) => {
                let (path, err) = boxed.as_ref();
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

    // -- ResourceMeta (frontmatter) tests --

    #[test]
    fn meta_no_frontmatter() {
        let md = "# Just a heading\nSome content.";
        let meta = ResourceMeta::from_md(md).unwrap();
        assert!(meta.skills.is_empty());
        assert!(meta.agents.is_empty());
        assert!(meta.tools.is_empty());
        assert!(meta.env.is_empty());
    }

    #[test]
    fn meta_no_metadata() {
        let md = "---\nname: test\ndescription: A test\n---\nBody.";
        let meta = ResourceMeta::from_md(md).unwrap();
        assert!(meta.skills.is_empty());
        assert!(meta.tools.is_empty());
    }

    #[test]
    fn meta_no_relava_block() {
        let md = "---\nname: test\nmetadata:\n  author: someone\n---\nBody.";
        let meta = ResourceMeta::from_md(md).unwrap();
        assert!(meta.skills.is_empty());
    }

    #[test]
    fn meta_skills_only() {
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
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.skills, vec!["notify-slack", "style-guide"]);
        assert!(meta.agents.is_empty());
        assert!(meta.tools.is_empty());
        assert!(meta.env.is_empty());
    }

    #[test]
    fn meta_agents_only() {
        let md = r#"---
name: orchestrator
description: Orchestrator agent
metadata:
  relava:
    agents:
      - debugger
---
"#;
        let meta = ResourceMeta::from_md(md).unwrap();
        assert!(meta.skills.is_empty());
        assert_eq!(meta.agents, vec!["debugger"]);
    }

    #[test]
    fn meta_deps_both() {
        let md = r#"---
name: orchestrator
metadata:
  relava:
    skills:
      - notify-slack
    agents:
      - debugger
---
"#;
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.skills, vec!["notify-slack"]);
        assert_eq!(meta.agents, vec!["debugger"]);
    }

    #[test]
    fn meta_ignores_unknown_metadata_keys() {
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
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.skills, vec!["foo"]);
    }

    #[test]
    fn meta_tools() {
        let md = r#"---
name: code-review
metadata:
  relava:
    tools:
      gh:
        description: GitHub CLI
        install:
          macos: brew install gh
          linux: apt install gh
          windows: winget install GitHub.cli
      jq:
        description: JSON processor
        install:
          macos: brew install jq
          linux: apt install jq
---
"#;
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.tools.len(), 2);

        let gh = &meta.tools["gh"];
        assert_eq!(gh.description, "GitHub CLI");
        assert_eq!(gh.install["macos"], "brew install gh");
        assert_eq!(gh.install["linux"], "apt install gh");
        assert_eq!(gh.install["windows"], "winget install GitHub.cli");

        let jq = &meta.tools["jq"];
        assert_eq!(jq.description, "JSON processor");
        assert_eq!(jq.install.len(), 2); // no windows entry
    }

    #[test]
    fn meta_env() {
        let md = r#"---
name: code-review
metadata:
  relava:
    env:
      GITHUB_TOKEN:
        required: true
        description: GitHub API token
      SLACK_WEBHOOK:
        required: false
        description: Slack webhook URL
---
"#;
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.env.len(), 2);

        let gh_token = &meta.env["GITHUB_TOKEN"];
        assert!(gh_token.required);
        assert_eq!(gh_token.description, "GitHub API token");

        let slack = &meta.env["SLACK_WEBHOOK"];
        assert!(!slack.required);
        assert_eq!(slack.description, "Slack webhook URL");
    }

    #[test]
    fn meta_full_example() {
        let md = r#"---
name: code-review
description: Comprehensive code review
metadata:
  relava:
    skills:
      - security-baseline
    tools:
      gh:
        description: GitHub CLI
        install:
          macos: brew install gh
    env:
      GITHUB_TOKEN:
        required: true
        description: GitHub API token
---
"#;
        let meta = ResourceMeta::from_md(md).unwrap();
        assert_eq!(meta.skills, vec!["security-baseline"]);
        assert_eq!(meta.tools.len(), 1);
        assert_eq!(meta.env.len(), 1);
        assert!(meta.env["GITHUB_TOKEN"].required);
    }

    // -- ProjectManifest (TOML) tests --

    #[test]
    fn project_empty() {
        let manifest = ProjectManifest::from_str("").unwrap();
        assert!(manifest.agent_type.is_none());
        assert!(manifest.skills.is_empty());
        assert!(manifest.agents.is_empty());
    }

    #[test]
    fn project_with_agent_type() {
        let toml = r#"
agent_type = "claude"

[skills]
denden = "1.2.0"
"#;
        let manifest = ProjectManifest::from_str(toml).unwrap();
        assert_eq!(manifest.agent_type.as_deref(), Some("claude"));
        assert_eq!(manifest.skills["denden"], "1.2.0");
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
