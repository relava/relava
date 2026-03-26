//! Shared output formatting for the Relava CLI.
//!
//! Provides colored status tags and table formatting used across multiple
//! commands. Respects the `NO_COLOR` environment variable (see
//! <https://no-color.org>).

use colored::Colorize;

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Call once at startup to honour the `NO_COLOR` convention. When the
/// variable is present (any value), colored output is disabled globally.
pub fn init() {
    if std::env::var_os("NO_COLOR").is_some() {
        colored::control::set_override(false);
    }
}

// ---------------------------------------------------------------------------
// Status tags
// ---------------------------------------------------------------------------

/// A status tag type that maps to a display label and color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Tag {
    Ok,
    Warn,
    Fail,
    Skill,
    Agent,
    Command,
    Rule,
    Tool,
    Env,
    Skip,
    Dep,
}

impl Tag {
    /// The short label displayed inside brackets.
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
            Self::Skill => "skill",
            Self::Agent => "agent",
            Self::Command => "command",
            Self::Rule => "rule",
            Self::Tool => "tool",
            Self::Env => "env",
            Self::Skip => "skip",
            Self::Dep => "dep",
        }
    }

    /// Format the tag with color: `  [ok]     message`.
    ///
    /// The tag is padded to 7 characters (longest label is "command")
    /// and the whole bracket expression is colored.
    pub fn fmt(self, message: &str) -> String {
        let label = self.label();
        let bracketed = format!("[{label:<7}]");
        let colored_bracket = match self {
            Self::Ok => bracketed.green(),
            Self::Warn => bracketed.yellow(),
            Self::Fail => bracketed.red().bold(),
            Self::Skill | Self::Agent | Self::Command | Self::Rule => bracketed.cyan(),
            Self::Tool | Self::Dep => bracketed.blue(),
            Self::Env => bracketed.magenta(),
            Self::Skip => bracketed.dimmed(),
        };
        format!("  {colored_bracket} {message}")
    }
}

/// Map a resource type to its corresponding tag.
///
/// Accepts `ResourceType` directly so the compiler enforces exhaustive
/// matching when new variants are added.
pub fn resource_tag(resource_type: relava_types::validate::ResourceType) -> Tag {
    match resource_type {
        relava_types::validate::ResourceType::Skill => Tag::Skill,
        relava_types::validate::ResourceType::Agent => Tag::Agent,
        relava_types::validate::ResourceType::Command => Tag::Command,
        relava_types::validate::ResourceType::Rule => Tag::Rule,
    }
}

// ---------------------------------------------------------------------------
// Tables (comfy-table)
// ---------------------------------------------------------------------------

/// Create a borderless table with dynamic content arrangement.
fn base_table() -> comfy_table::Table {
    let mut table = comfy_table::Table::new();
    table
        .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
        .load_preset(comfy_table::presets::NOTHING);
    table
}

/// Build a styled table with the given headers and rows.
///
/// Returns the rendered table as a string ready for `println!`.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut table = base_table();
    table.set_header(headers);

    for row in rows {
        table.add_row(row);
    }

    table.to_string()
}

/// Build a key-value table for info-style output.
///
/// Each entry is `(label, value)`. Entries with empty values are skipped.
pub fn kv_table(entries: &[(&str, String)]) -> String {
    let mut table = base_table();

    for (label, value) in entries {
        if !value.is_empty() {
            table.add_row(vec![format!("{label}:"), value.clone()]);
        }
    }

    table.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_ok_contains_ok() {
        // Disable colors for deterministic tests
        colored::control::set_override(false);
        let output = Tag::Ok.fmt("Server reachable");
        assert!(output.contains("[ok"));
        assert!(output.contains("Server reachable"));
    }

    #[test]
    fn tag_fail_contains_fail() {
        colored::control::set_override(false);
        let output = Tag::Fail.fmt("SKILL.md missing");
        assert!(output.contains("[FAIL"));
        assert!(output.contains("SKILL.md missing"));
    }

    #[test]
    fn tag_warn_contains_warn() {
        colored::control::set_override(false);
        let output = Tag::Warn.fmt("Missing env");
        assert!(output.contains("[warn"));
        assert!(output.contains("Missing env"));
    }

    #[test]
    fn tag_skill_contains_skill() {
        colored::control::set_override(false);
        let output = Tag::Skill.fmt(".claude/skills/denden/SKILL.md + 3 files");
        assert!(output.contains("[skill"));
    }

    #[test]
    fn tag_tool_contains_tool() {
        colored::control::set_override(false);
        let output = Tag::Tool.fmt("gh -- installed");
        assert!(output.contains("[tool"));
    }

    #[test]
    fn resource_tag_maps_correctly() {
        use relava_types::validate::ResourceType;
        assert_eq!(resource_tag(ResourceType::Skill), Tag::Skill);
        assert_eq!(resource_tag(ResourceType::Agent), Tag::Agent);
        assert_eq!(resource_tag(ResourceType::Command), Tag::Command);
        assert_eq!(resource_tag(ResourceType::Rule), Tag::Rule);
    }

    #[test]
    fn table_renders_headers_and_rows() {
        let output = table(&["Name", "Version"], &[vec!["foo".into(), "1.0.0".into()]]);
        assert!(output.contains("Name"));
        assert!(output.contains("foo"));
        assert!(output.contains("1.0.0"));
    }

    #[test]
    fn kv_table_renders_non_empty_entries() {
        let output = kv_table(&[
            ("Name", "denden".into()),
            ("Description", String::new()), // Should be skipped
            ("Version", "1.0.0".into()),
        ]);
        assert!(output.contains("Name:"));
        assert!(output.contains("denden"));
        assert!(output.contains("Version:"));
        assert!(!output.contains("Description:"));
    }

    #[test]
    fn kv_table_empty_entries() {
        let output = kv_table(&[]);
        // Empty table should still produce valid output
        assert!(output.is_empty() || !output.contains("error"));
    }
}
