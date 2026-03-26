use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;

use relava_types::manifest::ResourceMeta;
use relava_types::validate::{AgentType, ResourceType};
use relava_types::version::Version;

use crate::cache::DownloadCache;
use crate::registry::RegistryClient;

/// Maximum dependency tree depth to prevent runaway recursion.
const MAX_DEPTH: usize = 100;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ResolveError {
    /// Circular dependency detected. Contains the cycle path (e.g. "A -> B -> A").
    CircularDependency(Vec<String>),
    /// Dependency depth exceeds the limit.
    DepthLimitExceeded { depth: usize, limit: usize },
    /// Failed to resolve a version from the registry.
    VersionResolution(String),
    /// Failed to fetch dependency metadata.
    MetadataFetch(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CircularDependency(path) => {
                write!(f, "circular dependency detected: {}", path.join(" -> "))
            }
            Self::DepthLimitExceeded { depth, limit } => {
                write!(f, "dependency depth {depth} exceeds limit of {limit}")
            }
            Self::VersionResolution(msg) => write!(f, "version resolution failed: {msg}"),
            Self::MetadataFetch(msg) => write!(f, "failed to fetch dependency metadata: {msg}"),
        }
    }
}

impl std::error::Error for ResolveError {}

// ---------------------------------------------------------------------------
// Dependency provider trait — abstracts registry access for testability
// ---------------------------------------------------------------------------

/// Provides dependency information for resolution.
///
/// Production code uses `RegistryDepProvider`; tests use `MockDepProvider`.
pub trait DepProvider {
    /// Resolve the version for a resource (from registry, using optional pins).
    fn resolve_version(
        &self,
        resource_type: ResourceType,
        name: &str,
    ) -> Result<Version, ResolveError>;

    /// Fetch the dependency metadata (skills/agents lists) for a specific version.
    fn fetch_deps(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> Result<ResourceMeta, ResolveError>;

    /// Check if a resource is already installed at the given version.
    fn is_installed(&self, resource_type: ResourceType, name: &str, version: &Version) -> bool;
}

// ---------------------------------------------------------------------------
// Resolution result types
// ---------------------------------------------------------------------------

/// A single resolved dependency in the install order.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedDep {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub already_installed: bool,
}

/// A node in the dependency tree (for display and JSON output).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DepTreeNode {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<DepTreeNode>,
}

/// JSON output format for `relava resolve --json`.
#[derive(Debug, serde::Serialize)]
pub struct ResolveJsonOutput {
    pub root: String,
    pub order: Vec<ResolvedDep>,
}

/// Full result of dependency resolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// Leaf-first install order (root is the last element).
    pub install_order: Vec<ResolvedDep>,
    /// Tree structure for display.
    pub tree: DepTreeNode,
}

impl ResolveResult {
    /// Convert to JSON-friendly output matching the DESIGN.md format.
    pub fn to_json_output(&self) -> ResolveJsonOutput {
        ResolveJsonOutput {
            root: format!(
                "{}/{}@{}",
                self.tree.resource_type, self.tree.name, self.tree.version
            ),
            order: self.install_order.clone(),
        }
    }

    /// Return only the dependencies that need installation (not already installed,
    /// and excluding the root resource which is the last element).
    pub fn deps_to_install(&self) -> Vec<&ResolvedDep> {
        let len = self.install_order.len();
        if len <= 1 {
            return Vec::new();
        }
        self.install_order[..len - 1]
            .iter()
            .filter(|d| !d.already_installed)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tree display
// ---------------------------------------------------------------------------

impl DepTreeNode {
    /// Format the tree as a human-readable string with box-drawing characters.
    pub fn display(&self) -> String {
        let mut output = format!("{} {}@{}\n", self.resource_type, self.name, self.version);
        format_children(&self.dependencies, "", &mut output);
        output
    }
}

fn format_children(children: &[DepTreeNode], prefix: &str, output: &mut String) {
    for (i, child) in children.iter().enumerate() {
        let is_last = i == children.len() - 1;
        let connector = if is_last { "\u{2514}\u{2500}\u{2500} " } else { "\u{251c}\u{2500}\u{2500} " };
        let child_prefix = if is_last {
            format!("{prefix}    ")
        } else {
            format!("{prefix}\u{2502}   ")
        };

        output.push_str(&format!(
            "{prefix}{connector}{} {}@{}\n",
            child.resource_type, child.name, child.version
        ));
        format_children(&child.dependencies, &child_prefix, output);
    }
}

// ---------------------------------------------------------------------------
// DFS resolver
// ---------------------------------------------------------------------------

/// Resolve all transitive dependencies for a resource using depth-first search.
///
/// Returns a leaf-first install order and a tree structure for display.
/// Detects circular dependencies and enforces a depth limit of 100.
pub fn resolve<P: DepProvider>(
    provider: &P,
    resource_type: ResourceType,
    name: &str,
) -> Result<ResolveResult, ResolveError> {
    let mut visited: HashSet<(ResourceType, String)> = HashSet::new();
    let mut path: Vec<(ResourceType, String)> = Vec::new();
    let mut install_order: Vec<ResolvedDep> = Vec::new();

    let tree = dfs(
        provider,
        resource_type,
        name,
        0,
        &mut visited,
        &mut path,
        &mut install_order,
    )?;

    Ok(ResolveResult {
        install_order,
        tree,
    })
}

fn dfs<P: DepProvider>(
    provider: &P,
    resource_type: ResourceType,
    name: &str,
    depth: usize,
    visited: &mut HashSet<(ResourceType, String)>,
    path: &mut Vec<(ResourceType, String)>,
    install_order: &mut Vec<ResolvedDep>,
) -> Result<DepTreeNode, ResolveError> {
    // Enforce depth limit
    if depth > MAX_DEPTH {
        return Err(ResolveError::DepthLimitExceeded {
            depth,
            limit: MAX_DEPTH,
        });
    }

    let key = (resource_type, name.to_string());

    // Check for circular dependency
    if let Some(pos) = path.iter().position(|k| *k == key) {
        let cycle: Vec<String> = path[pos..]
            .iter()
            .map(|(rt, n)| format!("{rt}/{n}"))
            .chain(std::iter::once(format!("{resource_type}/{name}")))
            .collect();
        return Err(ResolveError::CircularDependency(cycle));
    }

    // Already fully resolved — return a leaf node (deduplication)
    if visited.contains(&key) {
        let version = provider.resolve_version(resource_type, name)?;
        return Ok(DepTreeNode {
            resource_type: resource_type.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            dependencies: Vec::new(),
        });
    }

    // Mark as "currently resolving" for cycle detection
    path.push(key.clone());

    // Resolve version from registry (using pins if available)
    let version = provider.resolve_version(resource_type, name)?;

    // Fetch metadata to discover sub-dependencies
    let meta = provider.fetch_deps(resource_type, name, &version)?;

    // Recurse into skill dependencies
    let mut children = Vec::new();
    for dep_name in &meta.skills {
        let child = dfs(
            provider,
            ResourceType::Skill,
            dep_name,
            depth + 1,
            visited,
            path,
            install_order,
        )?;
        children.push(child);
    }

    // Recurse into agent dependencies
    for dep_name in &meta.agents {
        let child = dfs(
            provider,
            ResourceType::Agent,
            dep_name,
            depth + 1,
            visited,
            path,
            install_order,
        )?;
        children.push(child);
    }

    // Done resolving this node
    path.pop();
    visited.insert(key);

    // Check if already installed at the resolved version
    let already_installed = provider.is_installed(resource_type, name, &version);

    // Post-order: add to install list after all children (leaf-first)
    install_order.push(ResolvedDep {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        already_installed,
    });

    Ok(DepTreeNode {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        version: version.to_string(),
        dependencies: children,
    })
}

// ---------------------------------------------------------------------------
// Production implementation: RegistryDepProvider
// ---------------------------------------------------------------------------

/// Fetches dependency information from the Relava registry server.
///
/// Downloads resources to the local cache to read their frontmatter metadata.
/// Uses version pins from `relava.toml` when available.
pub struct RegistryDepProvider<'a> {
    client: &'a RegistryClient,
    cache: &'a DownloadCache,
    install_root: &'a Path,
    /// Version pins from relava.toml (resource name -> version constraint string).
    version_pins: BTreeMap<String, String>,
}

impl<'a> RegistryDepProvider<'a> {
    pub fn new(
        client: &'a RegistryClient,
        cache: &'a DownloadCache,
        install_root: &'a Path,
        version_pins: BTreeMap<String, String>,
    ) -> Self {
        Self {
            client,
            cache,
            install_root,
            version_pins,
        }
    }

    /// Ensure a resource version is downloaded to cache.
    fn ensure_cached(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> Result<(), ResolveError> {
        if !self.cache.is_cached(resource_type, name, version) {
            let response = self
                .client
                .download(resource_type, name, version)
                .map_err(|e| ResolveError::MetadataFetch(e.to_string()))?;
            self.cache
                .store(resource_type, name, version, &response)
                .map_err(|e| ResolveError::MetadataFetch(e.to_string()))?;
        }
        Ok(())
    }

    /// Path to the primary .md file in the cache for a resource.
    fn md_path(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> std::path::PathBuf {
        let dir = self.cache.version_dir(resource_type, name, version);
        match resource_type {
            ResourceType::Skill => dir.join("SKILL.md"),
            ResourceType::Agent
            | ResourceType::Command
            | ResourceType::Rule => dir.join(format!("{name}.md")),
        }
    }
}

impl<'a> DepProvider for RegistryDepProvider<'a> {
    fn resolve_version(
        &self,
        resource_type: ResourceType,
        name: &str,
    ) -> Result<Version, ResolveError> {
        let pin = self.version_pins.get(name).map(|s| s.as_str());
        self.client
            .resolve_version(resource_type, name, pin)
            .map_err(|e| ResolveError::VersionResolution(e.to_string()))
    }

    fn fetch_deps(
        &self,
        resource_type: ResourceType,
        name: &str,
        version: &Version,
    ) -> Result<ResourceMeta, ResolveError> {
        self.ensure_cached(resource_type, name, version)?;
        let md_path = self.md_path(resource_type, name, version);
        if !md_path.exists() {
            return Ok(ResourceMeta::default());
        }
        ResourceMeta::from_file(&md_path)
            .map_err(|e| ResolveError::MetadataFetch(e.to_string()))
    }

    fn is_installed(&self, resource_type: ResourceType, name: &str, _version: &Version) -> bool {
        let agent_type = AgentType::Claude;
        let path = match resource_type {
            ResourceType::Skill => self
                .install_root
                .join(agent_type.skills_dir())
                .join(name)
                .join("SKILL.md"),
            ResourceType::Agent => self
                .install_root
                .join(agent_type.agents_dir())
                .join(format!("{name}.md")),
            ResourceType::Command => self
                .install_root
                .join(agent_type.commands_dir())
                .join(format!("{name}.md")),
            ResourceType::Rule => self
                .install_root
                .join(agent_type.rules_dir())
                .join(format!("{name}.md")),
        };
        path.exists()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // -- Mock provider --

    struct MockDepProvider {
        resources: HashMap<(ResourceType, String), MockResource>,
    }

    struct MockResource {
        version: Version,
        meta: ResourceMeta,
        installed: bool,
    }

    impl MockDepProvider {
        fn new() -> Self {
            Self {
                resources: HashMap::new(),
            }
        }

        fn add(
            &mut self,
            resource_type: ResourceType,
            name: &str,
            version: &str,
            skill_deps: &[&str],
            agent_deps: &[&str],
            installed: bool,
        ) {
            self.resources.insert(
                (resource_type, name.to_string()),
                MockResource {
                    version: Version::parse(version).unwrap(),
                    meta: ResourceMeta {
                        skills: skill_deps.iter().map(|s| s.to_string()).collect(),
                        agents: agent_deps.iter().map(|s| s.to_string()).collect(),
                        tools: BTreeMap::new(),
                        env: BTreeMap::new(),
                    },
                    installed,
                },
            );
        }
    }

    impl DepProvider for MockDepProvider {
        fn resolve_version(
            &self,
            resource_type: ResourceType,
            name: &str,
        ) -> Result<Version, ResolveError> {
            self.resources
                .get(&(resource_type, name.to_string()))
                .map(|r| r.version.clone())
                .ok_or_else(|| {
                    ResolveError::VersionResolution(format!(
                        "{resource_type} '{name}' not found in registry"
                    ))
                })
        }

        fn fetch_deps(
            &self,
            resource_type: ResourceType,
            name: &str,
            _version: &Version,
        ) -> Result<ResourceMeta, ResolveError> {
            self.resources
                .get(&(resource_type, name.to_string()))
                .map(|r| r.meta.clone())
                .ok_or_else(|| {
                    ResolveError::MetadataFetch(format!(
                        "{resource_type} '{name}' not found in registry"
                    ))
                })
        }

        fn is_installed(
            &self,
            resource_type: ResourceType,
            name: &str,
            _version: &Version,
        ) -> bool {
            self.resources
                .get(&(resource_type, name.to_string()))
                .map(|r| r.installed)
                .unwrap_or(false)
        }
    }

    // -- Helpers --

    fn names(result: &ResolveResult) -> Vec<&str> {
        result.install_order.iter().map(|d| d.name.as_str()).collect()
    }

    fn types_and_names(result: &ResolveResult) -> Vec<(&str, &str)> {
        result
            .install_order
            .iter()
            .map(|d| (d.resource_type.as_str(), d.name.as_str()))
            .collect()
    }

    // -- No dependencies --

    #[test]
    fn resolve_no_deps() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "denden", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "denden").unwrap();
        assert_eq!(names(&result), vec!["denden"]);
        assert_eq!(result.tree.name, "denden");
        assert!(result.tree.dependencies.is_empty());
    }

    // -- Simple dependency chain --

    #[test]
    fn resolve_single_dep() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "code-review",
            "1.2.0",
            &["security-baseline"],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "security-baseline",
            "1.0.0",
            &[],
            &[],
            false,
        );

        let result = resolve(&provider, ResourceType::Skill, "code-review").unwrap();
        // Leaf-first: security-baseline before code-review
        assert_eq!(names(&result), vec!["security-baseline", "code-review"]);
    }

    #[test]
    fn resolve_chain_three_deep() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["b"], &[], false);
        provider.add(ResourceType::Skill, "b", "1.0.0", &["c"], &[], false);
        provider.add(ResourceType::Skill, "c", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        assert_eq!(names(&result), vec!["c", "b", "a"]);
    }

    // -- Diamond dependency (deduplication) --

    #[test]
    fn resolve_diamond_deduplicates() {
        // A depends on B and C, both B and C depend on D
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["b", "c"], &[], false);
        provider.add(ResourceType::Skill, "b", "1.0.0", &["d"], &[], false);
        provider.add(ResourceType::Skill, "c", "1.0.0", &["d", "e"], &[], false);
        provider.add(ResourceType::Skill, "d", "1.0.0", &[], &[], false);
        provider.add(ResourceType::Skill, "e", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        // D appears once (via B first), then E (via C), then B, C, A
        assert_eq!(names(&result), vec!["d", "b", "e", "c", "a"]);
    }

    // -- Mixed resource types --

    #[test]
    fn resolve_mixed_skill_and_agent_deps() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Agent,
            "orchestrator",
            "1.0.0",
            &["notify-slack"],
            &["debugger"],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            &[],
            &[],
            false,
        );
        provider.add(
            ResourceType::Agent,
            "debugger",
            "0.5.0",
            &["log-capture"],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "log-capture",
            "0.2.0",
            &[],
            &[],
            false,
        );

        let result = resolve(&provider, ResourceType::Agent, "orchestrator").unwrap();
        assert_eq!(
            types_and_names(&result),
            vec![
                ("skill", "notify-slack"),
                ("skill", "log-capture"),
                ("agent", "debugger"),
                ("agent", "orchestrator"),
            ]
        );
    }

    // -- Already installed deps are marked --

    #[test]
    fn resolve_marks_already_installed() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "a",
            "1.0.0",
            &["b", "c"],
            &[],
            false,
        );
        provider.add(ResourceType::Skill, "b", "1.0.0", &[], &[], true); // installed
        provider.add(ResourceType::Skill, "c", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        assert_eq!(names(&result), vec!["b", "c", "a"]);
        assert!(result.install_order[0].already_installed); // b
        assert!(!result.install_order[1].already_installed); // c
        assert!(!result.install_order[2].already_installed); // a
    }

    #[test]
    fn deps_to_install_filters_installed() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "a",
            "1.0.0",
            &["b", "c"],
            &[],
            false,
        );
        provider.add(ResourceType::Skill, "b", "1.0.0", &[], &[], true);
        provider.add(ResourceType::Skill, "c", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        let to_install = result.deps_to_install();
        assert_eq!(to_install.len(), 1);
        assert_eq!(to_install[0].name, "c");
    }

    #[test]
    fn deps_to_install_empty_for_no_deps() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        assert!(result.deps_to_install().is_empty());
    }

    // -- Circular dependency detection --

    #[test]
    fn resolve_detects_direct_circular() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["b"], &[], false);
        provider.add(ResourceType::Skill, "b", "1.0.0", &["a"], &[], false);

        let err = resolve(&provider, ResourceType::Skill, "a").unwrap_err();
        match err {
            ResolveError::CircularDependency(cycle) => {
                assert_eq!(cycle, vec!["skill/a", "skill/b", "skill/a"]);
            }
            other => panic!("expected CircularDependency, got: {other}"),
        }
    }

    #[test]
    fn resolve_detects_indirect_circular() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["b"], &[], false);
        provider.add(ResourceType::Skill, "b", "1.0.0", &["c"], &[], false);
        provider.add(ResourceType::Skill, "c", "1.0.0", &["a"], &[], false);

        let err = resolve(&provider, ResourceType::Skill, "a").unwrap_err();
        match err {
            ResolveError::CircularDependency(cycle) => {
                assert_eq!(
                    cycle,
                    vec!["skill/a", "skill/b", "skill/c", "skill/a"]
                );
            }
            other => panic!("expected CircularDependency, got: {other}"),
        }
    }

    #[test]
    fn resolve_self_dependency() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["a"], &[], false);

        let err = resolve(&provider, ResourceType::Skill, "a").unwrap_err();
        match err {
            ResolveError::CircularDependency(cycle) => {
                assert_eq!(cycle, vec!["skill/a", "skill/a"]);
            }
            other => panic!("expected CircularDependency, got: {other}"),
        }
    }

    #[test]
    fn circular_dep_error_display() {
        let err = ResolveError::CircularDependency(vec![
            "skill/a".to_string(),
            "skill/b".to_string(),
            "skill/a".to_string(),
        ]);
        assert_eq!(
            err.to_string(),
            "circular dependency detected: skill/a -> skill/b -> skill/a"
        );
    }

    // -- Depth limit --

    #[test]
    fn resolve_enforces_depth_limit() {
        let mut provider = MockDepProvider::new();
        // Create a chain of 102 levels: s0 -> s1 -> s2 -> ... -> s101
        for i in 0..102 {
            let name = format!("s{i}");
            let deps = if i < 101 {
                vec![format!("s{}", i + 1)]
            } else {
                vec![]
            };
            provider.resources.insert(
                (ResourceType::Skill, name.clone()),
                MockResource {
                    version: Version::parse("1.0.0").unwrap(),
                    meta: ResourceMeta {
                        skills: deps,
                        agents: Vec::new(),
                        tools: BTreeMap::new(),
                        env: BTreeMap::new(),
                    },
                    installed: false,
                },
            );
        }

        let err = resolve(&provider, ResourceType::Skill, "s0").unwrap_err();
        match err {
            ResolveError::DepthLimitExceeded { limit, .. } => {
                assert_eq!(limit, MAX_DEPTH);
            }
            other => panic!("expected DepthLimitExceeded, got: {other}"),
        }
    }

    #[test]
    fn resolve_allows_depth_at_limit() {
        let mut provider = MockDepProvider::new();
        // Create a chain of exactly 101 levels (depth 0..100), which is allowed
        for i in 0..=100 {
            let name = format!("s{i}");
            let deps = if i < 100 {
                vec![format!("s{}", i + 1)]
            } else {
                vec![]
            };
            provider.resources.insert(
                (ResourceType::Skill, name.clone()),
                MockResource {
                    version: Version::parse("1.0.0").unwrap(),
                    meta: ResourceMeta {
                        skills: deps,
                        agents: Vec::new(),
                        tools: BTreeMap::new(),
                        env: BTreeMap::new(),
                    },
                    installed: false,
                },
            );
        }

        let result = resolve(&provider, ResourceType::Skill, "s0").unwrap();
        assert_eq!(result.install_order.len(), 101);
        // Leaf-first: s100 is first, s0 is last
        assert_eq!(result.install_order[0].name, "s100");
        assert_eq!(result.install_order[100].name, "s0");
    }

    #[test]
    fn depth_limit_error_display() {
        let err = ResolveError::DepthLimitExceeded {
            depth: 101,
            limit: 100,
        };
        assert_eq!(err.to_string(), "dependency depth 101 exceeds limit of 100");
    }

    // -- Missing dependency --

    #[test]
    fn resolve_errors_on_missing_dep() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "a",
            "1.0.0",
            &["nonexistent"],
            &[],
            false,
        );

        let err = resolve(&provider, ResourceType::Skill, "a").unwrap_err();
        match err {
            ResolveError::VersionResolution(msg) => {
                assert!(msg.contains("nonexistent"), "error should name the missing dep: {msg}");
            }
            other => panic!("expected VersionResolution, got: {other}"),
        }
    }

    #[test]
    fn resolve_errors_on_missing_root() {
        let provider = MockDepProvider::new();
        let err = resolve(&provider, ResourceType::Skill, "nonexistent").unwrap_err();
        assert!(matches!(err, ResolveError::VersionResolution(_)));
    }

    // -- Version information --

    #[test]
    fn resolve_preserves_versions() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "a",
            "2.1.0",
            &["b"],
            &[],
            false,
        );
        provider.add(ResourceType::Skill, "b", "0.3.1", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        assert_eq!(result.install_order[0].version, "0.3.1");
        assert_eq!(result.install_order[1].version, "2.1.0");
        assert_eq!(result.tree.version, "2.1.0");
    }

    // -- Tree display --

    #[test]
    fn tree_display_no_deps() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "denden", "1.2.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "denden").unwrap();
        assert_eq!(result.tree.display(), "skill denden@1.2.0\n");
    }

    #[test]
    fn tree_display_with_deps() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "denden",
            "1.2.0",
            &["notify-slack", "strawpot-recap"],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            &[],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "strawpot-recap",
            "1.0.0",
            &[],
            &[],
            false,
        );

        let result = resolve(&provider, ResourceType::Skill, "denden").unwrap();
        let display = result.tree.display();
        assert!(display.contains("skill denden@1.2.0"));
        assert!(display.contains("skill notify-slack@0.3.0"));
        assert!(display.contains("skill strawpot-recap@1.0.0"));
        assert!(display.contains("\u{251c}\u{2500}\u{2500}")); // ├──
        assert!(display.contains("\u{2514}\u{2500}\u{2500}")); // └──
    }

    #[test]
    fn tree_display_nested() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Agent,
            "orchestrator",
            "1.0.0",
            &["notify-slack"],
            &["debugger"],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            &[],
            &[],
            false,
        );
        provider.add(
            ResourceType::Agent,
            "debugger",
            "0.5.0",
            &["log-capture"],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "log-capture",
            "0.2.0",
            &[],
            &[],
            false,
        );

        let result = resolve(&provider, ResourceType::Agent, "orchestrator").unwrap();
        let display = result.tree.display();
        assert!(display.starts_with("agent orchestrator@1.0.0\n"));
        assert!(display.contains("skill log-capture@0.2.0"));
    }

    // -- JSON output --

    #[test]
    fn json_output_format() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "denden",
            "1.2.0",
            &["notify-slack"],
            &[],
            false,
        );
        provider.add(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            &[],
            &[],
            false,
        );

        let result = resolve(&provider, ResourceType::Skill, "denden").unwrap();
        let json = result.to_json_output();
        assert_eq!(json.root, "skill/denden@1.2.0");
        assert_eq!(json.order.len(), 2);
        assert_eq!(json.order[0].name, "notify-slack");
        assert_eq!(json.order[1].name, "denden");
    }

    #[test]
    fn json_output_serializes() {
        let mut provider = MockDepProvider::new();
        provider.add(
            ResourceType::Skill,
            "a",
            "1.0.0",
            &["b"],
            &[],
            false,
        );
        provider.add(ResourceType::Skill, "b", "1.0.0", &[], &[], true);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        let json_str = serde_json::to_string_pretty(&result.to_json_output()).unwrap();
        assert!(json_str.contains("\"root\""));
        assert!(json_str.contains("skill/a@1.0.0"));
        assert!(json_str.contains("\"order\""));
        assert!(json_str.contains("\"already_installed\": true"));
    }

    #[test]
    fn json_output_omits_already_installed_when_false() {
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &[], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        let json_str = serde_json::to_string(&result.to_json_output()).unwrap();
        assert!(
            !json_str.contains("already_installed"),
            "already_installed: false should be omitted"
        );
    }

    // -- Error display --

    #[test]
    fn version_resolution_error_display() {
        let err = ResolveError::VersionResolution("skill 'foo' not found".to_string());
        assert_eq!(
            err.to_string(),
            "version resolution failed: skill 'foo' not found"
        );
    }

    #[test]
    fn metadata_fetch_error_display() {
        let err = ResolveError::MetadataFetch("connection refused".to_string());
        assert_eq!(
            err.to_string(),
            "failed to fetch dependency metadata: connection refused"
        );
    }

    // -- Complex graph --

    #[test]
    fn resolve_complex_graph() {
        // A -> B, C
        // B -> D, E
        // C -> E, F
        // D -> (none)
        // E -> (none)
        // F -> D
        let mut provider = MockDepProvider::new();
        provider.add(ResourceType::Skill, "a", "1.0.0", &["b", "c"], &[], false);
        provider.add(ResourceType::Skill, "b", "1.0.0", &["d", "e"], &[], false);
        provider.add(ResourceType::Skill, "c", "1.0.0", &["e", "f"], &[], false);
        provider.add(ResourceType::Skill, "d", "1.0.0", &[], &[], false);
        provider.add(ResourceType::Skill, "e", "1.0.0", &[], &[], false);
        provider.add(ResourceType::Skill, "f", "1.0.0", &["d"], &[], false);

        let result = resolve(&provider, ResourceType::Skill, "a").unwrap();
        let order = names(&result);

        // Verify constraints: each dep appears before its dependents
        let pos = |name: &str| order.iter().position(|n| *n == name).unwrap();
        assert!(pos("d") < pos("b"));
        assert!(pos("e") < pos("b"));
        assert!(pos("e") < pos("c"));
        assert!(pos("f") < pos("c"));
        assert!(pos("d") < pos("f"));
        assert!(pos("b") < pos("a"));
        assert!(pos("c") < pos("a"));

        // Each name appears exactly once
        let unique: HashSet<&str> = order.iter().copied().collect();
        assert_eq!(unique.len(), order.len());
    }

    // -- RegistryDepProvider unit tests (no HTTP) --

    #[test]
    fn registry_provider_is_installed_skill() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path();

        // Create a skill directory
        let skill_dir = install_root.join(".claude/skills/denden");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "# Denden").unwrap();

        let client = RegistryClient::new("http://localhost:7420");
        let cache = DownloadCache::new(dir.path().join("cache"));
        let provider =
            RegistryDepProvider::new(&client, &cache, install_root, BTreeMap::new());

        let v = Version::parse("1.0.0").unwrap();
        assert!(provider.is_installed(ResourceType::Skill, "denden", &v));
        assert!(!provider.is_installed(ResourceType::Skill, "nonexistent", &v));
    }

    #[test]
    fn registry_provider_is_installed_agent() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path();

        let agent_dir = install_root.join(".claude/agents");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("debugger.md"), "# Debugger").unwrap();

        let client = RegistryClient::new("http://localhost:7420");
        let cache = DownloadCache::new(dir.path().join("cache"));
        let provider =
            RegistryDepProvider::new(&client, &cache, install_root, BTreeMap::new());

        let v = Version::parse("1.0.0").unwrap();
        assert!(provider.is_installed(ResourceType::Agent, "debugger", &v));
        assert!(!provider.is_installed(ResourceType::Agent, "nonexistent", &v));
    }

    #[test]
    fn registry_provider_fetch_deps_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let cache = DownloadCache::new(cache_dir);

        // Store a skill with deps in cache
        let skill_md = r#"---
name: code-review
metadata:
  relava:
    skills:
      - security-baseline
      - style-guide
---
# Code Review
"#;
        use crate::registry::{DownloadFile, DownloadResponse};
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: "code-review".to_string(),
            version: "1.0.0".to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(skill_md.as_bytes())
                },
            }],
        };
        let v = Version::parse("1.0.0").unwrap();
        cache
            .store(ResourceType::Skill, "code-review", &v, &response)
            .unwrap();

        let client = RegistryClient::new("http://localhost:7420");
        let provider =
            RegistryDepProvider::new(&client, &cache, dir.path(), BTreeMap::new());

        let meta = provider
            .fetch_deps(ResourceType::Skill, "code-review", &v)
            .unwrap();
        assert_eq!(meta.skills, vec!["security-baseline", "style-guide"]);
        assert!(meta.agents.is_empty());
    }

    #[test]
    fn registry_provider_fetch_deps_no_md_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let cache = DownloadCache::new(cache_dir.clone());

        // Create a cache directory without a SKILL.md
        let version_dir = cache_dir.join("skills/empty/1.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();

        let client = RegistryClient::new("http://localhost:7420");
        let provider =
            RegistryDepProvider::new(&client, &cache, dir.path(), BTreeMap::new());

        let v = Version::parse("1.0.0").unwrap();
        let meta = provider
            .fetch_deps(ResourceType::Skill, "empty", &v)
            .unwrap();
        assert!(meta.skills.is_empty());
        assert!(meta.agents.is_empty());
    }
}
