use std::path::Path;

use crate::version::Version;

/// Resource types managed by Relava.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceType {
    Skill,
    Agent,
    Command,
    Rule,
}

impl ResourceType {
    pub fn from_str(s: &str) -> Result<Self, ValidationError> {
        match s {
            "skill" => Ok(Self::Skill),
            "agent" => Ok(Self::Agent),
            "command" => Ok(Self::Command),
            "rule" => Ok(Self::Rule),
            _ => Err(ValidationError::InvalidResourceType(s.to_string())),
        }
    }
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skill => write!(f, "skill"),
            Self::Agent => write!(f, "agent"),
            Self::Command => write!(f, "command"),
            Self::Rule => write!(f, "rule"),
        }
    }
}

// ---------------------------------------------------------------------------
// Slug validation
// ---------------------------------------------------------------------------

/// Validate a resource name (slug).
///
/// Rules (aligned with Agent Skills spec):
/// - 1–64 characters
/// - Lowercase alphanumeric (a-z, 0-9) and hyphens (-)
/// - Must not start or end with a hyphen
/// - No consecutive hyphens
pub fn validate_slug(slug: &str) -> Result<(), ValidationError> {
    if slug.is_empty() || slug.len() > 64 {
        return Err(ValidationError::InvalidSlug(
            slug.to_string(),
            "must be 1-64 characters".to_string(),
        ));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(ValidationError::InvalidSlug(
            slug.to_string(),
            "must not start or end with a hyphen".to_string(),
        ));
    }
    if slug.contains("--") {
        return Err(ValidationError::InvalidSlug(
            slug.to_string(),
            "must not contain consecutive hyphens".to_string(),
        ));
    }
    if !slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(ValidationError::InvalidSlug(
            slug.to_string(),
            "must contain only lowercase alphanumeric characters and hyphens".to_string(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Resource structure validation
// ---------------------------------------------------------------------------

/// Validate that a resource directory/file has the correct structure.
pub fn validate_resource_structure(
    path: &Path,
    resource_type: ResourceType,
    name: &str,
) -> Result<(), ValidationError> {
    match resource_type {
        ResourceType::Skill => validate_skill_structure(path),
        ResourceType::Agent => validate_single_md_structure(path, name, resource_type),
        ResourceType::Command => validate_single_md_structure(path, name, resource_type),
        ResourceType::Rule => validate_single_md_structure(path, name, resource_type),
    }
}

fn validate_skill_structure(path: &Path) -> Result<(), ValidationError> {
    if !path.is_dir() {
        return Err(ValidationError::InvalidStructure(
            "skill must be a directory".to_string(),
        ));
    }
    let skill_md = path.join("SKILL.md");
    if !skill_md.exists() {
        return Err(ValidationError::InvalidStructure(
            format!("skill directory missing SKILL.md at {}", skill_md.display()),
        ));
    }
    Ok(())
}

fn validate_single_md_structure(
    path: &Path,
    name: &str,
    resource_type: ResourceType,
) -> Result<(), ValidationError> {
    // Accept either a direct .md file or a directory containing name.md
    if path.is_file() {
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            return Err(ValidationError::InvalidStructure(
                format!("{resource_type} must be a .md file"),
            ));
        }
        Ok(())
    } else if path.is_dir() {
        let md_file = path.join(format!("{name}.md"));
        if !md_file.exists() {
            return Err(ValidationError::InvalidStructure(
                format!("{resource_type} directory missing {name}.md"),
            ));
        }
        Ok(())
    } else {
        Err(ValidationError::InvalidStructure(
            format!("{resource_type} path does not exist: {}", path.display()),
        ))
    }
}

// ---------------------------------------------------------------------------
// Version validation
// ---------------------------------------------------------------------------

/// Validate that a version string is valid semver.
pub fn validate_version(version: &str) -> Result<Version, ValidationError> {
    Version::parse(version).map_err(|_| ValidationError::InvalidVersion(version.to_string()))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum ValidationError {
    InvalidResourceType(String),
    InvalidSlug(String, String),
    InvalidStructure(String),
    InvalidVersion(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidResourceType(t) => {
                write!(f, "invalid resource type '{t}': must be skill, agent, command, or rule")
            }
            Self::InvalidSlug(slug, reason) => write!(f, "invalid slug '{slug}': {reason}"),
            Self::InvalidStructure(msg) => write!(f, "invalid resource structure: {msg}"),
            Self::InvalidVersion(v) => write!(f, "invalid version '{v}': must be X.Y.Z"),
        }
    }
}

impl std::error::Error for ValidationError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // -- Resource type tests --

    #[test]
    fn resource_type_valid() {
        assert_eq!(ResourceType::from_str("skill").unwrap(), ResourceType::Skill);
        assert_eq!(ResourceType::from_str("agent").unwrap(), ResourceType::Agent);
        assert_eq!(ResourceType::from_str("command").unwrap(), ResourceType::Command);
        assert_eq!(ResourceType::from_str("rule").unwrap(), ResourceType::Rule);
    }

    #[test]
    fn resource_type_invalid() {
        assert!(ResourceType::from_str("plugin").is_err());
        assert!(ResourceType::from_str("").is_err());
        assert!(ResourceType::from_str("Skill").is_err());
    }

    // -- Slug tests --

    #[test]
    fn slug_valid() {
        assert!(validate_slug("denden").is_ok());
        assert!(validate_slug("notify-slack").is_ok());
        assert!(validate_slug("code-review").is_ok());
        assert!(validate_slug("a").is_ok());
        assert!(validate_slug("my-skill-v2").is_ok());
        assert!(validate_slug("a1b2c3").is_ok());
    }

    #[test]
    fn slug_empty() {
        assert!(validate_slug("").is_err());
    }

    #[test]
    fn slug_too_long() {
        let long = "a".repeat(65);
        assert!(validate_slug(&long).is_err());
        let exact = "a".repeat(64);
        assert!(validate_slug(&exact).is_ok());
    }

    #[test]
    fn slug_starts_with_hyphen() {
        assert!(validate_slug("-denden").is_err());
    }

    #[test]
    fn slug_ends_with_hyphen() {
        assert!(validate_slug("denden-").is_err());
    }

    #[test]
    fn slug_consecutive_hyphens() {
        assert!(validate_slug("code--review").is_err());
    }

    #[test]
    fn slug_uppercase() {
        assert!(validate_slug("Denden").is_err());
        assert!(validate_slug("NOTIFY").is_err());
    }

    #[test]
    fn slug_invalid_chars() {
        assert!(validate_slug("my.skill").is_err());
        assert!(validate_slug("my_skill").is_err());
        assert!(validate_slug("my skill").is_err());
        assert!(validate_slug("my@skill").is_err());
    }

    // -- Structure tests --

    #[test]
    fn skill_valid_structure() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("my-skill")).unwrap();
        fs::write(dir.join("my-skill/SKILL.md"), "---\nname: my-skill\n---\n").unwrap();
        assert!(validate_resource_structure(&dir.join("my-skill"), ResourceType::Skill, "my-skill").is_ok());
    }

    #[test]
    fn skill_missing_skill_md() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("my-skill")).unwrap();
        assert!(validate_resource_structure(&dir.join("my-skill"), ResourceType::Skill, "my-skill").is_err());
    }

    #[test]
    fn skill_not_a_directory() {
        let dir = tempdir();
        fs::write(dir.join("my-skill"), "not a dir").unwrap();
        assert!(validate_resource_structure(&dir.join("my-skill"), ResourceType::Skill, "my-skill").is_err());
    }

    #[test]
    fn agent_as_file() {
        let dir = tempdir();
        fs::write(dir.join("debugger.md"), "---\nname: debugger\n---\n").unwrap();
        assert!(validate_resource_structure(&dir.join("debugger.md"), ResourceType::Agent, "debugger").is_ok());
    }

    #[test]
    fn agent_as_directory() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("debugger")).unwrap();
        fs::write(dir.join("debugger/debugger.md"), "content").unwrap();
        assert!(validate_resource_structure(&dir.join("debugger"), ResourceType::Agent, "debugger").is_ok());
    }

    #[test]
    fn agent_wrong_extension() {
        let dir = tempdir();
        fs::write(dir.join("debugger.txt"), "content").unwrap();
        assert!(validate_resource_structure(&dir.join("debugger.txt"), ResourceType::Agent, "debugger").is_err());
    }

    #[test]
    fn agent_dir_missing_md() {
        let dir = tempdir();
        fs::create_dir_all(dir.join("debugger")).unwrap();
        assert!(validate_resource_structure(&dir.join("debugger"), ResourceType::Agent, "debugger").is_err());
    }

    #[test]
    fn command_as_file() {
        let dir = tempdir();
        fs::write(dir.join("commit.md"), "content").unwrap();
        assert!(validate_resource_structure(&dir.join("commit.md"), ResourceType::Command, "commit").is_ok());
    }

    #[test]
    fn rule_as_file() {
        let dir = tempdir();
        fs::write(dir.join("no-console-log.md"), "content").unwrap();
        assert!(validate_resource_structure(&dir.join("no-console-log.md"), ResourceType::Rule, "no-console-log").is_ok());
    }

    // -- Version validation tests --

    #[test]
    fn version_valid() {
        assert!(validate_version("1.2.3").is_ok());
        assert!(validate_version("0.0.0").is_ok());
    }

    #[test]
    fn version_invalid() {
        assert!(validate_version("1.2").is_err());
        assert!(validate_version("abc").is_err());
        assert!(validate_version("").is_err());
    }

    // -- Test helpers --

    fn tempdir() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "relava-test-{}-{}",
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
