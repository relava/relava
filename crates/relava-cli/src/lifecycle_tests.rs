//! End-to-end integration tests for the relava CLI resource lifecycle.
//!
//! These tests exercise the full lifecycle: publish (import) -> install -> list ->
//! update -> disable/enable -> remove, using a mock HTTP server for registry
//! interactions and isolated temp directories for all filesystem operations.

use std::fs;
use std::path::Path;

use base64::Engine;
use tempfile::TempDir;

use crate::cache::DownloadCache;
use crate::disable;
use crate::enable;
use crate::import;
use crate::install;
use crate::list;
use crate::registry::{DownloadFile, DownloadResponse};
use crate::remove;
use crate::save;
use crate::update;
use relava_types::validate::ResourceType;
use relava_types::version::Version;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn temp_dir() -> TempDir {
    TempDir::new().expect("failed to create temp dir")
}

fn encode_base64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Create a mock server that serves version list and download endpoints
/// for a given resource.
fn mock_versions(
    server: &mut mockito::ServerGuard,
    resource_type: &str,
    name: &str,
    versions: &[&str],
) -> mockito::Mock {
    let version_entries: Vec<String> = versions
        .iter()
        .map(|v| format!(r#"{{"version": "{v}"}}"#))
        .collect();
    let body = format!(r#"{{"versions": [{}]}}"#, version_entries.join(","));

    server
        .mock(
            "GET",
            format!("/api/v1/resources/{resource_type}/{name}/versions").as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

fn mock_download(
    server: &mut mockito::ServerGuard,
    resource_type: &str,
    name: &str,
    version: &str,
    files: &[(&str, &[u8])],
) -> mockito::Mock {
    let file_entries: Vec<String> = files
        .iter()
        .map(|(path, content)| {
            format!(
                r#"{{"path": "{path}", "content": "{}"}}"#,
                encode_base64(content)
            )
        })
        .collect();
    let body = format!(
        r#"{{"resource_type": "{resource_type}", "name": "{name}", "version": "{version}", "files": [{}]}}"#,
        file_entries.join(",")
    );

    server
        .mock(
            "GET",
            format!("/api/v1/resources/{resource_type}/{name}/versions/{version}/download")
                .as_str(),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

fn mock_publish(
    server: &mut mockito::ServerGuard,
    resource_type: &str,
    name: &str,
    version: &str,
) -> mockito::Mock {
    server
        .mock(
            "PUT",
            format!("/api/v1/resources/{resource_type}/{name}/versions/{version}").as_str(),
        )
        .with_status(201)
        .create()
}

fn mock_publish_conflict(
    server: &mut mockito::ServerGuard,
    resource_type: &str,
    name: &str,
    version: &str,
) -> mockito::Mock {
    server
        .mock(
            "PUT",
            format!("/api/v1/resources/{resource_type}/{name}/versions/{version}").as_str(),
        )
        .with_status(409)
        .create()
}

fn mock_not_found(
    server: &mut mockito::ServerGuard,
    resource_type: &str,
    name: &str,
) -> mockito::Mock {
    server
        .mock(
            "GET",
            format!("/api/v1/resources/{resource_type}/{name}/versions").as_str(),
        )
        .with_status(404)
        .create()
}

/// Populate the download cache directly (bypassing HTTP) for tests that
/// only need cached data without running the download mock.
fn populate_cache(
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    version: &str,
    files: &[(&str, &[u8])],
) {
    let v = Version::parse(version).unwrap();
    let response = DownloadResponse {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        files: files
            .iter()
            .map(|(path, content)| DownloadFile {
                path: path.to_string(),
                content: encode_base64(content),
            })
            .collect(),
    };
    cache
        .store(resource_type, name, &v, &response)
        .expect("failed to populate cache");
}

/// Set up a relava.toml in the project directory.
fn write_manifest(project_dir: &Path, content: &str) {
    fs::write(project_dir.join("relava.toml"), content).unwrap();
}

// ---------------------------------------------------------------------------
// Full lifecycle: skill
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_skill_install_list_update_remove() {
    let mut server = mockito::Server::new();
    let project = temp_dir();
    // --- Step 1: Install skill v1.0.0 ---
    let _mock_ver = mock_versions(&mut server, "skill", "code-review", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "code-review",
        "1.0.0",
        &[
            ("SKILL.md", b"# Code Review\nVersion 1 content"),
            ("templates/checklist.md", b"- [ ] Check tests"),
        ],
    );

    let install_result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "code-review",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .expect("install should succeed");

    assert_eq!(install_result.name, "code-review");
    assert_eq!(install_result.version, "1.0.0");
    assert_eq!(install_result.files.len(), 2);

    // Verify files on disk
    let skill_dir = project.path().join(".claude/skills/code-review");
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("templates/checklist.md").exists());
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
        "# Code Review\nVersion 1 content"
    );

    // --- Step 2: List resources ---
    let list_result = list::run(&list::ListOpts {
        resource_type: None,
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .expect("list should succeed");

    assert_eq!(list_result.resources.len(), 1);
    assert_eq!(list_result.resources[0].name, "code-review");
    assert_eq!(list_result.resources[0].resource_type, "skill");
    assert_eq!(list_result.resources[0].status, "active");

    // --- Step 3: Update to v2.0.0 ---
    // Write relava.toml so update knows the installed version
    write_manifest(project.path(), "[skills]\ncode-review = \"*\"\n");

    // Clear old mocks and set up v2 mocks
    drop(_mock_ver);
    drop(_mock_dl);
    let _mock_ver2 = mock_versions(&mut server, "skill", "code-review", &["1.0.0", "2.0.0"]);
    let _mock_dl2 = mock_download(
        &mut server,
        "skill",
        "code-review",
        "2.0.0",
        &[
            ("SKILL.md", b"# Code Review\nVersion 2 updated"),
            (
                "templates/checklist.md",
                b"- [ ] Check tests\n- [ ] Check coverage",
            ),
        ],
    );

    let update_result = update::run(&update::UpdateOpts {
        server_url: &server.url(),
        resource_type: Some(ResourceType::Skill),
        name: Some("code-review"),
        all: false,
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .expect("update should succeed");

    assert_eq!(update_result.updated.len(), 1);
    assert_eq!(update_result.updated[0].new_version, "2.0.0");

    // Verify updated content on disk
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
        "# Code Review\nVersion 2 updated"
    );

    // --- Step 4: Remove ---
    let remove_result = remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Skill,
        name: "code-review",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .expect("remove should succeed");

    assert!(remove_result.was_removed);
    assert!(!skill_dir.exists());

    // List should be empty now
    let list_after = list::run(&list::ListOpts {
        resource_type: None,
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();
    assert!(list_after.resources.is_empty());
}

// ---------------------------------------------------------------------------
// Full lifecycle: agent
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_agent_install_list_remove() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "agent", "debugger", &["0.5.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "agent",
        "debugger",
        "0.5.0",
        &[("debugger.md", b"# Debugger Agent\nDebug instructions")],
    );

    // Install
    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Agent,
        name: "debugger",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .expect("install agent should succeed");

    assert_eq!(result.resource_type, "agent");
    assert_eq!(result.version, "0.5.0");

    // Verify file on disk
    let agent_file = project.path().join(".claude/agents/debugger.md");
    assert!(agent_file.exists());
    assert_eq!(
        fs::read_to_string(&agent_file).unwrap(),
        "# Debugger Agent\nDebug instructions"
    );

    // List
    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Agent),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();

    assert_eq!(list_result.resources.len(), 1);
    assert_eq!(list_result.resources[0].name, "debugger");
    assert_eq!(list_result.resources[0].resource_type, "agent");

    // Remove
    let remove_result = remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Agent,
        name: "debugger",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();

    assert!(remove_result.was_removed);
    assert!(!agent_file.exists());
}

// ---------------------------------------------------------------------------
// Full lifecycle: command
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_command_install_list_remove() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "command", "deploy", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "command",
        "deploy",
        "1.0.0",
        &[("deploy.md", b"# Deploy Command\nDeploy steps")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Command,
        name: "deploy",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    assert_eq!(result.resource_type, "command");

    let cmd_file = project.path().join(".claude/commands/deploy.md");
    assert!(cmd_file.exists());

    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Command),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();
    assert_eq!(list_result.resources.len(), 1);
    assert_eq!(list_result.resources[0].name, "deploy");

    remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Command,
        name: "deploy",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(!cmd_file.exists());
}

// ---------------------------------------------------------------------------
// Full lifecycle: rule
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_rule_install_list_remove() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "rule", "no-console-log", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "rule",
        "no-console-log",
        "1.0.0",
        &[("no-console-log.md", b"# No Console Log\nAvoid console.log")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Rule,
        name: "no-console-log",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    assert_eq!(result.resource_type, "rule");

    let rule_file = project.path().join(".claude/rules/no-console-log.md");
    assert!(rule_file.exists());

    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Rule),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();
    assert_eq!(list_result.resources.len(), 1);
    assert_eq!(list_result.resources[0].name, "no-console-log");

    remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Rule,
        name: "no-console-log",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(!rule_file.exists());
}

// ---------------------------------------------------------------------------
// Disable / Enable round-trip
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_install_disable_enable_remove() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "skill", "git-workflow", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "git-workflow",
        "1.0.0",
        &[("SKILL.md", b"# Git Workflow\nBranching guide")],
    );

    // Install
    install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "git-workflow",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    let active_path = project.path().join(".claude/skills/git-workflow");
    let disabled_path = project.path().join(".claude/skills/.disabled/git-workflow");
    assert!(active_path.exists());

    // Disable
    let disable_result = disable::run(&disable::DisableOpts {
        resource_type: ResourceType::Skill,
        name: "git-workflow",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(disable_result.was_disabled);
    assert!(!active_path.exists());
    assert!(disabled_path.join("SKILL.md").exists());

    // List shows disabled
    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Skill),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();
    assert_eq!(list_result.resources.len(), 1);
    assert_eq!(list_result.resources[0].status, "disabled");

    // Enable
    let enable_result = enable::run(&enable::EnableOpts {
        resource_type: ResourceType::Skill,
        name: "git-workflow",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(enable_result.was_enabled);
    assert!(active_path.join("SKILL.md").exists());
    assert!(!disabled_path.exists());

    // List shows active again
    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Skill),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();
    assert_eq!(list_result.resources[0].status, "active");

    // Remove
    remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Skill,
        name: "git-workflow",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(!active_path.exists());
}

// ---------------------------------------------------------------------------
// Disable / Enable for file-based resources (agent)
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_agent_disable_enable() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "agent", "reviewer", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "agent",
        "reviewer",
        "1.0.0",
        &[("reviewer.md", b"# Reviewer\nReview instructions")],
    );

    install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Agent,
        name: "reviewer",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    let active_path = project.path().join(".claude/agents/reviewer.md");
    let disabled_path = project.path().join(".claude/agents/.disabled/reviewer.md");
    assert!(active_path.exists());

    // Disable
    disable::run(&disable::DisableOpts {
        resource_type: ResourceType::Agent,
        name: "reviewer",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(!active_path.exists());
    assert!(disabled_path.exists());

    // Enable
    enable::run(&enable::EnableOpts {
        resource_type: ResourceType::Agent,
        name: "reviewer",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();
    assert!(active_path.exists());
    assert!(!disabled_path.exists());
}

// ---------------------------------------------------------------------------
// Install with version pin
// ---------------------------------------------------------------------------

#[test]
fn install_with_exact_version_pin() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    // Both versions available, but we pin to 1.0.0
    let _mock_ver = mock_versions(&mut server, "skill", "pinned-skill", &["1.0.0", "2.0.0"]);
    let _mock_dl_v1 = mock_download(
        &mut server,
        "skill",
        "pinned-skill",
        "1.0.0",
        &[("SKILL.md", b"# Pinned v1")],
    );
    // The resolver resolves to latest (2.0.0) for dependency checking,
    // so we must also mock that download.
    let _mock_dl_v2 = mock_download(
        &mut server,
        "skill",
        "pinned-skill",
        "2.0.0",
        &[("SKILL.md", b"# Pinned v2")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "pinned-skill",
        version_pin: Some("1.0.0"),
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    // Should install v1.0.0 despite v2.0.0 being available
    assert_eq!(result.version, "1.0.0");
    // Verify the v1 content is on disk (not v2)
    let content =
        fs::read_to_string(project.path().join(".claude/skills/pinned-skill/SKILL.md")).unwrap();
    assert_eq!(content, "# Pinned v1");
}

// ---------------------------------------------------------------------------
// Install with --save writes to relava.toml
// ---------------------------------------------------------------------------

#[test]
fn install_save_writes_manifest() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    // Create empty relava.toml
    write_manifest(project.path(), "");

    let _mock_ver = mock_versions(&mut server, "agent", "saver", &["1.5.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "agent",
        "saver",
        "1.5.0",
        &[("saver.md", b"# Saver Agent")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Agent,
        name: "saver",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    // Simulate --save (main.rs calls save::add_to_manifest after install)
    save::add_to_manifest(
        project.path(),
        ResourceType::Agent,
        "saver",
        &result.version,
        true,
    )
    .unwrap();

    // Verify manifest was updated
    let content = fs::read_to_string(project.path().join("relava.toml")).unwrap();
    assert!(content.contains("saver"));
    assert!(content.contains("1.5.0"));
}

// ---------------------------------------------------------------------------
// Import (publish) lifecycle
// ---------------------------------------------------------------------------

#[test]
fn import_publishes_skill_to_registry() {
    let mut server = mockito::Server::new();
    let source = temp_dir();

    // Create a skill directory to import
    let skill_dir = source.path().join("my-importer");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-importer\nversion: 1.0.0\ndescription: A test skill\n---\n# My Importer",
    )
    .unwrap();

    let _mock_pub = mock_publish(&mut server, "skill", "my-importer", "1.0.0");

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &skill_dir,
        version: None,
        json: true,
        verbose: false,
    })
    .expect("import should succeed");

    assert_eq!(result.name, "my-importer");
    assert_eq!(result.version, "1.0.0");
    assert!(result.files.contains(&"SKILL.md".to_string()));
    assert_eq!(result.description, Some("A test skill".to_string()));
}

#[test]
fn import_agent_file() {
    let mut server = mockito::Server::new();
    let source = temp_dir();

    let agent_file = source.path().join("test-agent.md");
    fs::write(
        &agent_file,
        "---\nname: test-agent\nversion: 0.1.0\n---\n# Agent",
    )
    .unwrap();

    let _mock_pub = mock_publish(&mut server, "agent", "test-agent", "0.1.0");

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Agent,
        path: &agent_file,
        version: None,
        json: true,
        verbose: false,
    })
    .unwrap();

    assert_eq!(result.name, "test-agent");
    assert_eq!(result.version, "0.1.0");
}

#[test]
fn import_with_explicit_version_overrides_frontmatter() {
    let mut server = mockito::Server::new();
    let source = temp_dir();

    let skill_dir = source.path().join("override-ver");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: override-ver\nversion: 1.0.0\n---\n# Skill",
    )
    .unwrap();

    let _mock_pub = mock_publish(&mut server, "skill", "override-ver", "5.0.0");

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &skill_dir,
        version: Some("5.0.0"),
        json: true,
        verbose: false,
    })
    .unwrap();

    assert_eq!(result.version, "5.0.0");
}

// ---------------------------------------------------------------------------
// Import → Install round-trip
// ---------------------------------------------------------------------------

#[test]
fn import_then_install_round_trip() {
    let mut server = mockito::Server::new();
    let source = temp_dir();
    let project = temp_dir();

    // Create a skill to import
    let skill_dir = source.path().join("roundtrip-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: roundtrip-skill\nversion: 1.0.0\n---\n# Round Trip Content",
    )
    .unwrap();

    // Import (publish) to registry
    let _mock_pub = mock_publish(&mut server, "skill", "roundtrip-skill", "1.0.0");
    import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &skill_dir,
        version: None,
        json: true,
        verbose: false,
    })
    .unwrap();

    // Install from registry (mock the download endpoint with same content)
    let _mock_ver = mock_versions(&mut server, "skill", "roundtrip-skill", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "roundtrip-skill",
        "1.0.0",
        &[(
            "SKILL.md",
            b"---\nname: roundtrip-skill\nversion: 1.0.0\n---\n# Round Trip Content",
        )],
    );

    let install_result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "roundtrip-skill",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    assert_eq!(install_result.version, "1.0.0");
    assert!(
        project
            .path()
            .join(".claude/skills/roundtrip-skill/SKILL.md")
            .exists()
    );
}

// ---------------------------------------------------------------------------
// Update --all mode
// ---------------------------------------------------------------------------

#[test]
fn update_all_updates_multiple_resources() {
    let mut server = mockito::Server::new();
    let project = temp_dir();
    let cache_root = temp_dir();
    let cache = DownloadCache::new(cache_root.path().to_path_buf());

    // Pre-install a skill and an agent by populating cache + writing files
    populate_cache(
        &cache,
        ResourceType::Skill,
        "alpha",
        "1.0.0",
        &[("SKILL.md", b"# Alpha v1")],
    );
    install::write_to_project_public(
        project.path(),
        ResourceType::Skill,
        "alpha",
        &Version::parse("1.0.0").unwrap(),
        &cache,
    )
    .unwrap();

    populate_cache(
        &cache,
        ResourceType::Agent,
        "beta",
        "1.0.0",
        &[("beta.md", b"# Beta v1")],
    );
    install::write_to_project_public(
        project.path(),
        ResourceType::Agent,
        "beta",
        &Version::parse("1.0.0").unwrap(),
        &cache,
    )
    .unwrap();

    // Write manifest with wildcard pins
    write_manifest(
        project.path(),
        "[skills]\nalpha = \"*\"\n\n[agents]\nbeta = \"*\"\n",
    );

    // Mock v2.0.0 for both
    let _mock_ver_a = mock_versions(&mut server, "skill", "alpha", &["1.0.0", "2.0.0"]);
    let _mock_dl_a = mock_download(
        &mut server,
        "skill",
        "alpha",
        "2.0.0",
        &[("SKILL.md", b"# Alpha v2")],
    );
    let _mock_ver_b = mock_versions(&mut server, "agent", "beta", &["1.0.0", "2.0.0"]);
    let _mock_dl_b = mock_download(
        &mut server,
        "agent",
        "beta",
        "2.0.0",
        &[("beta.md", b"# Beta v2")],
    );

    let result = update::run(&update::UpdateOpts {
        server_url: &server.url(),
        resource_type: None,
        name: None,
        all: true,
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();

    assert_eq!(result.updated.len(), 2);

    // Verify files were updated
    assert_eq!(
        fs::read_to_string(project.path().join(".claude/skills/alpha/SKILL.md")).unwrap(),
        "# Alpha v2"
    );
    assert_eq!(
        fs::read_to_string(project.path().join(".claude/agents/beta.md")).unwrap(),
        "# Beta v2"
    );
}

// ---------------------------------------------------------------------------
// Update skips pinned versions
// ---------------------------------------------------------------------------

#[test]
fn update_skips_exact_pinned_version() {
    let project = temp_dir();
    let cache_root = temp_dir();
    let cache = DownloadCache::new(cache_root.path().to_path_buf());

    // Pre-install skill
    populate_cache(
        &cache,
        ResourceType::Skill,
        "pinned",
        "1.0.0",
        &[("SKILL.md", b"# Pinned")],
    );
    install::write_to_project_public(
        project.path(),
        ResourceType::Skill,
        "pinned",
        &Version::parse("1.0.0").unwrap(),
        &cache,
    )
    .unwrap();

    // Pin exact version in manifest
    write_manifest(project.path(), "[skills]\npinned = \"1.0.0\"\n");

    // No server needed — pinned resources skip the registry check
    let server = mockito::Server::new();
    let result = update::run(&update::UpdateOpts {
        server_url: &server.url(),
        resource_type: Some(ResourceType::Skill),
        name: Some("pinned"),
        all: false,
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();

    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].status, "pinned");
}

// ---------------------------------------------------------------------------
// Install overwrites existing files
// ---------------------------------------------------------------------------

#[test]
fn install_overwrites_existing_skill() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    // Pre-create skill files
    let skill_dir = project.path().join(".claude/skills/overwrite-me");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# Old content").unwrap();

    let _mock_ver = mock_versions(&mut server, "skill", "overwrite-me", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "overwrite-me",
        "1.0.0",
        &[("SKILL.md", b"# New content from registry")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "overwrite-me",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    assert!(!result.overwritten.is_empty());
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
        "# New content from registry"
    );
}

// ---------------------------------------------------------------------------
// Multi-resource listing
// ---------------------------------------------------------------------------

#[test]
fn list_all_resource_types_after_install() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    // Install one of each type
    struct TypeInfo {
        rt: ResourceType,
        rt_str: &'static str,
        name: &'static str,
        file_name: &'static str,
        content: &'static [u8],
    }

    let types_and_names = [
        TypeInfo {
            rt: ResourceType::Skill,
            rt_str: "skill",
            name: "my-skill",
            file_name: "SKILL.md",
            content: b"# Skill",
        },
        TypeInfo {
            rt: ResourceType::Agent,
            rt_str: "agent",
            name: "my-agent",
            file_name: "my-agent.md",
            content: b"# Agent",
        },
        TypeInfo {
            rt: ResourceType::Command,
            rt_str: "command",
            name: "my-command",
            file_name: "my-command.md",
            content: b"# Command",
        },
        TypeInfo {
            rt: ResourceType::Rule,
            rt_str: "rule",
            name: "my-rule",
            file_name: "my-rule.md",
            content: b"# Rule",
        },
    ];

    for info in &types_and_names {
        let _mv = mock_versions(&mut server, info.rt_str, info.name, &["1.0.0"]);
        let _md = mock_download(
            &mut server,
            info.rt_str,
            info.name,
            "1.0.0",
            &[(info.file_name, info.content)],
        );

        install::run(&install::InstallOpts {
            server_url: &server.url(),
            resource_type: info.rt,
            name: info.name,
            version_pin: None,
            project_dir: project.path(),
            global: false,
            json: true,
            verbose: false,
            yes: true,
        })
        .unwrap();
    }

    // List all
    let result = list::run(&list::ListOpts {
        resource_type: None,
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();

    assert_eq!(result.resources.len(), 4);
    let names: Vec<&str> = result.resources.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"my-skill"));
    assert!(names.contains(&"my-agent"));
    assert!(names.contains(&"my-command"));
    assert!(names.contains(&"my-rule"));
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[test]
fn install_missing_resource_returns_error() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock = mock_not_found(&mut server, "skill", "nonexistent");

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "nonexistent",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn install_invalid_version_returns_error() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "skill", "ver-test", &["1.0.0"]);

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "ver-test",
        version_pin: Some("9.9.9"),
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    });

    assert!(result.is_err());
}

#[test]
fn install_invalid_slug_returns_error() {
    let server = mockito::Server::new();
    let project = temp_dir();

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "INVALID_SLUG",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid slug"));
}

#[test]
fn remove_nonexistent_resource_is_not_error() {
    let project = temp_dir();

    let result = remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Skill,
        name: "ghost",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();

    assert!(!result.was_removed);
}

#[test]
fn update_not_installed_returns_error() {
    let server = mockito::Server::new();
    let project = temp_dir();

    let result = update::run(&update::UpdateOpts {
        server_url: &server.url(),
        resource_type: Some(ResourceType::Skill),
        name: Some("not-here"),
        all: false,
        project_dir: project.path(),
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not installed"));
}

#[test]
fn update_all_without_manifest_returns_error() {
    let server = mockito::Server::new();
    let project = temp_dir();

    let result = update::run(&update::UpdateOpts {
        server_url: &server.url(),
        resource_type: None,
        name: None,
        all: true,
        project_dir: project.path(),
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("relava.toml not found"));
}

#[test]
fn import_nonexistent_path_returns_error() {
    let server = mockito::Server::new();
    let bad_path = std::path::PathBuf::from("/tmp/relava-nonexistent-path-test");

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &bad_path,
        version: None,
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("cannot access"));
}

#[test]
fn import_invalid_version_returns_error() {
    let source = temp_dir();
    let server = mockito::Server::new();

    let skill_dir = source.path().join("bad-ver");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "# content").unwrap();

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &skill_dir,
        version: Some("not-a-version"),
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid version"));
}

#[test]
fn import_duplicate_version_returns_error() {
    let mut server = mockito::Server::new();
    let source = temp_dir();

    let skill_dir = source.path().join("dup-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: dup-skill\nversion: 1.0.0\n---\n# Skill",
    )
    .unwrap();

    let _mock = mock_publish_conflict(&mut server, "skill", "dup-skill", "1.0.0");

    let result = import::run(&import::ImportOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        path: &skill_dir,
        version: None,
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already exists"));
}

// ---------------------------------------------------------------------------
// Install + list with manifest version tracking
// ---------------------------------------------------------------------------

#[test]
fn list_shows_manifest_versions() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "skill", "versioned", &["2.3.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "versioned",
        "2.3.0",
        &[("SKILL.md", b"# Versioned")],
    );

    install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "versioned",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    // Write manifest with the installed version
    write_manifest(project.path(), "[skills]\nversioned = \"2.3.0\"\n");

    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Skill),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();

    assert_eq!(list_result.resources[0].version, "2.3.0");
}

// ---------------------------------------------------------------------------
// Remove with --save updates manifest
// ---------------------------------------------------------------------------

#[test]
fn remove_with_save_updates_manifest() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    let _mock_ver = mock_versions(&mut server, "skill", "removable", &["1.0.0"]);
    let _mock_dl = mock_download(
        &mut server,
        "skill",
        "removable",
        "1.0.0",
        &[("SKILL.md", b"# Removable")],
    );

    install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "removable",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    // Write manifest
    write_manifest(project.path(), "[skills]\nremovable = \"1.0.0\"\n");

    // Remove
    remove::run(&remove::RemoveOpts {
        resource_type: ResourceType::Skill,
        name: "removable",
        project_dir: project.path(),
        json: true,
        verbose: false,
    })
    .unwrap();

    // Simulate --save (main.rs calls save::remove_from_manifest after remove)
    save::remove_from_manifest(project.path(), ResourceType::Skill, "removable", true).unwrap();

    // Verify manifest no longer has the entry
    let content = fs::read_to_string(project.path().join("relava.toml")).unwrap();
    assert!(!content.contains("removable"));
}

// ---------------------------------------------------------------------------
// Dependency resolution (using cache directly)
// ---------------------------------------------------------------------------

#[test]
fn install_skill_with_dependency() {
    let mut server = mockito::Server::new();
    let project = temp_dir();

    // Parent skill depends on "dep-skill" via frontmatter
    let parent_content = b"---\nname: parent-skill\nmetadata:\n  relava:\n    skills:\n      - dep-skill\n---\n# Parent";

    // Mock parent: versions + download
    let _mock_parent_ver = mock_versions(&mut server, "skill", "parent-skill", &["1.0.0"]);
    let _mock_parent_dl = mock_download(
        &mut server,
        "skill",
        "parent-skill",
        "1.0.0",
        &[("SKILL.md", parent_content)],
    );

    // Mock dependency: versions + download
    let _mock_dep_ver = mock_versions(&mut server, "skill", "dep-skill", &["1.0.0"]);
    let _mock_dep_dl = mock_download(
        &mut server,
        "skill",
        "dep-skill",
        "1.0.0",
        &[("SKILL.md", b"# Dependency Skill\nShared utilities")],
    );

    let result = install::run(&install::InstallOpts {
        server_url: &server.url(),
        resource_type: ResourceType::Skill,
        name: "parent-skill",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    })
    .unwrap();

    // Parent should be installed
    assert_eq!(result.name, "parent-skill");

    // Dependency should also be installed
    assert!(
        !result.dependencies.is_empty(),
        "should have dependency results"
    );
    assert_eq!(result.dependencies[0].name, "dep-skill");

    // Both should exist on disk
    assert!(
        project
            .path()
            .join(".claude/skills/parent-skill/SKILL.md")
            .exists()
    );
    assert!(
        project
            .path()
            .join(".claude/skills/dep-skill/SKILL.md")
            .exists()
    );

    // List should show both
    let list_result = list::run(&list::ListOpts {
        resource_type: Some(ResourceType::Skill),
        project_dir: project.path(),
        json: true,
        _verbose: false,
    })
    .unwrap();

    assert_eq!(list_result.resources.len(), 2);
}

// ---------------------------------------------------------------------------
// Concurrent disable detection (conflict guard)
// ---------------------------------------------------------------------------

#[test]
fn disable_conflict_when_both_exist() {
    let project = temp_dir();

    // Create both active and disabled versions manually
    let active = project.path().join(".claude/skills/conflict-skill");
    let disabled = project
        .path()
        .join(".claude/skills/.disabled/conflict-skill");
    fs::create_dir_all(&active).unwrap();
    fs::write(active.join("SKILL.md"), "# Active").unwrap();
    fs::create_dir_all(&disabled).unwrap();
    fs::write(disabled.join("SKILL.md"), "# Disabled").unwrap();

    let result = disable::run(&disable::DisableOpts {
        resource_type: ResourceType::Skill,
        name: "conflict-skill",
        project_dir: project.path(),
        json: true,
        verbose: false,
    });

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_lowercase();
    assert!(err_msg.contains("conflict") || err_msg.contains("both"));
}

// ---------------------------------------------------------------------------
// Server unreachable error
// ---------------------------------------------------------------------------

#[test]
fn install_server_unreachable_returns_error() {
    let project = temp_dir();

    // Use a port that nothing is listening on
    let result = install::run(&install::InstallOpts {
        server_url: "http://127.0.0.1:1",
        resource_type: ResourceType::Skill,
        name: "test-skill",
        version_pin: None,
        project_dir: project.path(),
        global: false,
        json: true,
        verbose: false,
        yes: true,
    });

    assert!(result.is_err());
}
