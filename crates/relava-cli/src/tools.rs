use std::collections::BTreeMap;
use std::process::Command;

use relava_types::manifest::ToolSpec;

/// Status of a tool check/install attempt.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    /// Already present on PATH.
    Found,
    /// Successfully installed.
    Installed,
    /// User declined the install prompt.
    Declined,
    /// Install command failed (with stderr output).
    Failed(String),
    /// No install command for this OS.
    NoCommand,
    /// Skipped (not found, but `--yes` was not set and we're non-interactive).
    Skipped,
}

/// Result of checking/installing a single tool.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolResult {
    pub name: String,
    pub description: String,
    pub status: ToolStatus,
}

/// Detect the current OS as a key matching the `install` map in `ToolSpec`.
///
/// Returns `"macos"`, `"linux"`, or `"windows"`.
pub fn detect_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

/// Check if a binary exists on PATH.
///
/// Uses `which` on Unix-like systems and `where` on Windows.
pub fn tool_on_path(name: &str) -> bool {
    let cmd = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    match Command::new(cmd)
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(e) => {
            eprintln!("warning: failed to run `{cmd}`: {e}");
            false
        }
    }
}

/// Run an install command string via the system shell.
///
/// Returns `Ok(())` on success, `Err(stderr)` on failure.
pub fn run_install_command(command: &str) -> Result<(), String> {
    let (shell, flag) = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let output = Command::new(shell)
        .args([flag, command])
        .output()
        .map_err(|e| format!("failed to execute install command: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Err(stdout.trim().to_string())
        } else {
            Err(msg.to_string())
        }
    }
}

/// Prompt callback type: given a prompt string, return true to proceed.
pub type PromptFn = Box<dyn Fn(&str) -> bool>;

/// Determine the status of a single tool: found, install attempted, or skipped.
fn check_single_tool(
    name: &str,
    spec: &ToolSpec,
    os: &str,
    auto_yes: bool,
    prompt_fn: Option<&PromptFn>,
) -> ToolStatus {
    if tool_on_path(name) {
        return ToolStatus::Found;
    }

    let install_cmd = match spec.install.get(os) {
        Some(cmd) => cmd,
        None => return ToolStatus::NoCommand,
    };

    let should_install = if auto_yes {
        true
    } else {
        match prompt_fn {
            Some(f) => f(&format!("Install with: {install_cmd}? [Y/n]")),
            None => return ToolStatus::Skipped,
        }
    };

    if !should_install {
        return ToolStatus::Declined;
    }

    eprintln!("  running: {install_cmd}");
    match run_install_command(install_cmd) {
        Ok(()) => ToolStatus::Installed,
        Err(e) => ToolStatus::Failed(e),
    }
}

/// Check and optionally install all declared tools.
///
/// - `tools`: the `metadata.relava.tools` map from the skill's frontmatter
/// - `auto_yes`: if true, skip prompts and auto-install
/// - `prompt_fn`: callback for interactive prompts (only called when `auto_yes` is false)
///
/// Returns a result per tool. Tool failures are non-fatal.
pub fn check_and_install_tools(
    tools: &BTreeMap<String, ToolSpec>,
    auto_yes: bool,
    prompt_fn: Option<&PromptFn>,
) -> Vec<ToolResult> {
    let os = detect_os();

    tools
        .iter()
        .map(|(name, spec)| {
            let status = check_single_tool(name, spec, os, auto_yes, prompt_fn);
            ToolResult {
                name: name.clone(),
                description: spec.description.clone(),
                status,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_os_returns_known_value() {
        let os = detect_os();
        assert!(
            ["macos", "linux", "windows"].contains(&os),
            "unexpected OS: {os}"
        );
    }

    #[test]
    fn tool_on_path_finds_common_tool() {
        // `sh` is always available on Unix CI; `cmd` on Windows
        if cfg!(target_os = "windows") {
            assert!(tool_on_path("cmd"));
        } else {
            assert!(tool_on_path("sh"));
        }
    }

    #[test]
    fn tool_on_path_missing_tool() {
        assert!(!tool_on_path("this-tool-definitely-does-not-exist-xyz"));
    }

    #[test]
    fn check_tool_already_on_path() {
        let mut tools = BTreeMap::new();
        let existing = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        tools.insert(
            existing.to_string(),
            ToolSpec {
                description: "shell".to_string(),
                install: BTreeMap::new(),
            },
        );

        let results = check_and_install_tools(&tools, false, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ToolStatus::Found);
    }

    #[test]
    fn check_tool_missing_no_command_for_os() {
        let mut tools = BTreeMap::new();
        // Use an OS key that won't match
        let mut install = BTreeMap::new();
        install.insert("nonexistent-os".to_string(), "echo hello".to_string());
        tools.insert(
            "fake-tool-xyz".to_string(),
            ToolSpec {
                description: "fake".to_string(),
                install,
            },
        );

        let results = check_and_install_tools(&tools, false, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ToolStatus::NoCommand);
    }

    #[test]
    fn check_tool_missing_skipped_no_prompt() {
        let mut tools = BTreeMap::new();
        let os = detect_os();
        let mut install = BTreeMap::new();
        install.insert(os.to_string(), "echo installed".to_string());
        tools.insert(
            "fake-tool-xyz".to_string(),
            ToolSpec {
                description: "fake".to_string(),
                install,
            },
        );

        // No prompt function provided, non-interactive -> Skipped
        let results = check_and_install_tools(&tools, false, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ToolStatus::Skipped);
    }

    #[test]
    fn check_tool_declined_by_user() {
        let mut tools = BTreeMap::new();
        let os = detect_os();
        let mut install = BTreeMap::new();
        install.insert(os.to_string(), "echo installed".to_string());
        tools.insert(
            "fake-tool-xyz".to_string(),
            ToolSpec {
                description: "fake".to_string(),
                install,
            },
        );

        let prompt: PromptFn = Box::new(|_| false);
        let results = check_and_install_tools(&tools, false, Some(&prompt));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, ToolStatus::Declined);
    }

    #[test]
    fn check_tool_auto_yes_installs() {
        let mut tools = BTreeMap::new();
        let os = detect_os();
        let mut install = BTreeMap::new();
        // Use a harmless command that succeeds
        install.insert(os.to_string(), "echo ok".to_string());
        tools.insert(
            "fake-tool-xyz-auto".to_string(),
            ToolSpec {
                description: "fake".to_string(),
                install,
            },
        );

        let results = check_and_install_tools(&tools, true, None);
        assert_eq!(results.len(), 1);
        // Command succeeds, so tool is "installed" (even though it's just echo)
        assert_eq!(results[0].status, ToolStatus::Installed);
    }

    #[test]
    fn check_tool_install_failure() {
        let mut tools = BTreeMap::new();
        let os = detect_os();
        let mut install = BTreeMap::new();
        install.insert(os.to_string(), "false".to_string());
        tools.insert(
            "fake-tool-xyz-fail".to_string(),
            ToolSpec {
                description: "fake".to_string(),
                install,
            },
        );

        let results = check_and_install_tools(&tools, true, None);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].status, ToolStatus::Failed(_)));
    }

    #[test]
    fn run_install_command_success() {
        assert!(run_install_command("echo hello").is_ok());
    }

    #[test]
    fn run_install_command_failure() {
        let cmd = if cfg!(target_os = "windows") {
            "exit /b 1"
        } else {
            "false"
        };
        let result = run_install_command(cmd);
        assert!(result.is_err());
    }

    #[test]
    fn empty_tools_map() {
        let tools = BTreeMap::new();
        let results = check_and_install_tools(&tools, false, None);
        assert!(results.is_empty());
    }

    #[test]
    fn multiple_tools_mixed_status() {
        let mut tools = BTreeMap::new();

        // Tool that exists on PATH
        let existing = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        tools.insert(
            existing.to_string(),
            ToolSpec {
                description: "shell".to_string(),
                install: BTreeMap::new(),
            },
        );

        // Tool that doesn't exist and has no command for this OS
        let mut install = BTreeMap::new();
        install.insert("nonexistent-os".to_string(), "echo hello".to_string());
        tools.insert(
            "zzz-nonexistent-tool".to_string(),
            ToolSpec {
                description: "missing".to_string(),
                install,
            },
        );

        let results = check_and_install_tools(&tools, false, None);
        assert_eq!(results.len(), 2);

        let found = results.iter().find(|r| r.name == existing).unwrap();
        assert_eq!(found.status, ToolStatus::Found);

        let missing = results
            .iter()
            .find(|r| r.name == "zzz-nonexistent-tool")
            .unwrap();
        assert_eq!(missing.status, ToolStatus::NoCommand);
    }
}
