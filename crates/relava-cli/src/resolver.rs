use std::collections::{HashSet, VecDeque};

use crate::cache::DownloadCache;
use crate::registry::RegistryClient;
use relava_types::manifest::ResourceMeta;
use relava_types::validate::ResourceType;
use relava_types::version::Version;

/// Maximum recursion depth for dependency resolution.
const MAX_DEPTH: usize = 100;

/// A resolved resource with its type, name, and version.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedResource {
    pub resource_type: ResourceType,
    pub name: String,
    pub version: Version,
}

/// Errors from dependency resolution.
#[derive(Debug)]
pub enum ResolveError {
    /// Circular dependency detected. Contains the cycle path (e.g., "A -> B -> A").
    CircularDependency(String),
    /// Depth limit exceeded.
    DepthLimitExceeded(usize),
    /// Failed to fetch or parse a dependency from the registry.
    Registry(String),
    /// Failed to parse frontmatter of a dependency.
    Frontmatter(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CircularDependency(path) => {
                write!(f, "circular dependency detected: {path}")
            }
            Self::DepthLimitExceeded(limit) => {
                write!(
                    f,
                    "dependency depth limit of {limit} exceeded — possible infinite recursion"
                )
            }
            Self::Registry(msg) => write!(f, "registry error during resolution: {msg}"),
            Self::Frontmatter(msg) => {
                write!(f, "failed to parse dependency frontmatter: {msg}")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve all transitive dependencies of a resource in leaf-first (topological) order.
///
/// For each dependency declared in frontmatter (`metadata.relava.skills` and
/// `metadata.relava.agents`), this function recursively fetches the dependency's
/// manifest from the registry, resolves its own dependencies, and builds a flat,
/// deduplicated install list with leaves first.
///
/// The root resource itself is **not** included in the returned list.
///
/// # Errors
/// - `CircularDependency` if A depends on B depends on A (with full cycle path)
/// - `DepthLimitExceeded` if resolution exceeds 100 levels
/// - `Registry` if a dependency cannot be fetched from the registry
/// - `Frontmatter` if a dependency's frontmatter cannot be parsed
pub fn resolve(
    client: &RegistryClient,
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    version: &Version,
) -> Result<Vec<ResolvedResource>, ResolveError> {
    let mut state = ResolveState {
        result: Vec::new(),
        visited: HashSet::new(),
        path: VecDeque::new(),
    };

    // Get the root resource's dependencies
    let meta = fetch_meta(client, cache, resource_type, name, version)?;
    let deps = collect_deps(&meta);

    if deps.is_empty() {
        return Ok(state.result);
    }

    // Add root to path for cycle detection
    state.path.push_back((resource_type, name.to_string()));
    state.visited.insert((resource_type, name.to_string()));

    // Resolve each direct dependency
    for (dep_type, dep_name) in deps {
        resolve_recursive(client, cache, dep_type, &dep_name, &mut state, 1)?;
    }

    Ok(state.result)
}

/// State carried through the recursive resolution.
struct ResolveState {
    result: Vec<ResolvedResource>,
    visited: HashSet<(ResourceType, String)>,
    path: VecDeque<(ResourceType, String)>,
}

/// Recursively resolve a single dependency and its transitive dependencies.
fn resolve_recursive(
    client: &RegistryClient,
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    state: &mut ResolveState,
    depth: usize,
) -> Result<(), ResolveError> {
    // Check depth limit
    if depth > MAX_DEPTH {
        return Err(ResolveError::DepthLimitExceeded(MAX_DEPTH));
    }

    let key = (resource_type, name.to_string());

    // Check for circular dependency (in current path, not just visited)
    if state.path.iter().any(|p| p == &key) {
        let cycle: Vec<String> = state
            .path
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .chain(std::iter::once(format!("{resource_type} {name}")))
            .collect();
        return Err(ResolveError::CircularDependency(cycle.join(" -> ")));
    }

    // Skip if already resolved (deduplication)
    if state.visited.contains(&key) {
        return Ok(());
    }

    // Resolve version from registry
    let version = client
        .resolve_version(resource_type, name, None)
        .map_err(|e| ResolveError::Registry(e.to_string()))?;

    // Fetch manifest to discover transitive deps
    let meta = fetch_meta(client, cache, resource_type, name, &version)?;
    let deps = collect_deps(&meta);

    // Add to path for cycle detection
    state.path.push_back(key.clone());

    // Recurse into transitive dependencies first (DFS, leaves first)
    for (dep_type, dep_name) in deps {
        resolve_recursive(client, cache, dep_type, &dep_name, state, depth + 1)?;
    }

    // Remove from path after processing children
    state.path.pop_back();

    // Mark as visited and add to result (after all children = leaf-first)
    state.visited.insert(key);
    state.result.push(ResolvedResource {
        resource_type,
        name: name.to_string(),
        version,
    });

    Ok(())
}

/// Fetch the frontmatter metadata for a resource from the cache or registry.
fn fetch_meta(
    client: &RegistryClient,
    cache: &DownloadCache,
    resource_type: ResourceType,
    name: &str,
    version: &Version,
) -> Result<ResourceMeta, ResolveError> {
    // Determine the primary .md file name
    let md_filename = match resource_type {
        ResourceType::Skill => "SKILL.md".to_string(),
        ResourceType::Agent | ResourceType::Command | ResourceType::Rule => {
            format!("{name}.md")
        }
    };

    // Try reading from cache first
    if cache.is_cached(resource_type, name, version)
        && let Ok(content) = cache.read_file(resource_type, name, version, &md_filename)
        && let Ok(content_str) = String::from_utf8(content)
    {
        return ResourceMeta::from_md(&content_str)
            .map_err(|e| ResolveError::Frontmatter(e.to_string()));
    }

    // Download from registry
    let response = client
        .download(resource_type, name, version)
        .map_err(|e| ResolveError::Registry(e.to_string()))?;

    // Store in cache
    let _ = cache.store(resource_type, name, version, &response);

    // Find the .md file in the download response
    let md_file = response
        .files
        .iter()
        .find(|f| f.path == md_filename)
        .ok_or_else(|| {
            ResolveError::Frontmatter(format!(
                "{} {}@{} has no {md_filename}",
                resource_type, name, version
            ))
        })?;

    // Decode and parse
    use base64::Engine;
    let content = base64::engine::general_purpose::STANDARD
        .decode(&md_file.content)
        .map_err(|e| ResolveError::Frontmatter(format!("base64 decode failed: {e}")))?;

    let content_str = String::from_utf8(content)
        .map_err(|e| ResolveError::Frontmatter(format!("invalid UTF-8: {e}")))?;

    ResourceMeta::from_md(&content_str).map_err(|e| ResolveError::Frontmatter(e.to_string()))
}

/// Collect dependency references from a resource's metadata.
///
/// Returns a list of (ResourceType, name) pairs — skills first, then agents.
fn collect_deps(meta: &ResourceMeta) -> Vec<(ResourceType, String)> {
    let mut deps = Vec::new();
    for skill_name in &meta.skills {
        deps.push((ResourceType::Skill, skill_name.clone()));
    }
    for agent_name in &meta.agents {
        deps.push((ResourceType::Agent, agent_name.clone()));
    }
    deps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::DownloadCache;
    use crate::registry::{DownloadFile, DownloadResponse};

    fn encode_base64(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    fn test_cache() -> (std::path::PathBuf, DownloadCache) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "relava-resolver-test-{}-{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cache = DownloadCache::new(root.clone());
        (root, cache)
    }

    /// Store a skill with dependencies in the cache.
    fn cache_skill(cache: &DownloadCache, name: &str, version: &str, deps: &[&str]) {
        let v = Version::parse(version).unwrap();
        let skills_yaml = if deps.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = deps.iter().map(|d| format!("      - {d}")).collect();
            format!("metadata:\n  relava:\n    skills:\n{}", items.join("\n"))
        };
        let content = format!("---\nname: {name}\n{skills_yaml}\n---\n# {name}\n");
        let response = DownloadResponse {
            resource_type: "skill".to_string(),
            name: name.to_string(),
            version: version.to_string(),
            files: vec![DownloadFile {
                path: "SKILL.md".to_string(),
                content: encode_base64(content.as_bytes()),
            }],
        };
        cache
            .store(ResourceType::Skill, name, &v, &response)
            .unwrap();
    }

    /// Store an agent with skill and/or agent dependencies in the cache.
    fn cache_agent(
        cache: &DownloadCache,
        name: &str,
        version: &str,
        skill_deps: &[&str],
        agent_deps: &[&str],
    ) {
        let v = Version::parse(version).unwrap();
        let mut yaml_parts = Vec::new();
        if !skill_deps.is_empty() {
            let items: Vec<String> = skill_deps.iter().map(|d| format!("      - {d}")).collect();
            yaml_parts.push(format!("    skills:\n{}", items.join("\n")));
        }
        if !agent_deps.is_empty() {
            let items: Vec<String> = agent_deps.iter().map(|d| format!("      - {d}")).collect();
            yaml_parts.push(format!("    agents:\n{}", items.join("\n")));
        }
        let metadata_block = if yaml_parts.is_empty() {
            String::new()
        } else {
            format!("metadata:\n  relava:\n{}", yaml_parts.join("\n"))
        };
        let content = format!("---\nname: {name}\n{metadata_block}\n---\n# {name}\n");
        let response = DownloadResponse {
            resource_type: "agent".to_string(),
            name: name.to_string(),
            version: version.to_string(),
            files: vec![DownloadFile {
                path: format!("{name}.md"),
                content: encode_base64(content.as_bytes()),
            }],
        };
        cache
            .store(ResourceType::Agent, name, &v, &response)
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // Unit tests for collect_deps
    // -----------------------------------------------------------------------

    #[test]
    fn collect_deps_empty() {
        let meta = ResourceMeta::default();
        assert!(collect_deps(&meta).is_empty());
    }

    #[test]
    fn collect_deps_skills_only() {
        let meta = ResourceMeta {
            skills: vec!["a".to_string(), "b".to_string()],
            ..Default::default()
        };
        let deps = collect_deps(&meta);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0], (ResourceType::Skill, "a".to_string()));
        assert_eq!(deps[1], (ResourceType::Skill, "b".to_string()));
    }

    #[test]
    fn collect_deps_agents_only() {
        let meta = ResourceMeta {
            agents: vec!["x".to_string()],
            ..Default::default()
        };
        let deps = collect_deps(&meta);
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], (ResourceType::Agent, "x".to_string()));
    }

    #[test]
    fn collect_deps_mixed() {
        let meta = ResourceMeta {
            skills: vec!["s1".to_string()],
            agents: vec!["a1".to_string()],
            ..Default::default()
        };
        let deps = collect_deps(&meta);
        assert_eq!(deps.len(), 2);
        // Skills come before agents
        assert_eq!(deps[0].0, ResourceType::Skill);
        assert_eq!(deps[1].0, ResourceType::Agent);
    }

    // -----------------------------------------------------------------------
    // Unit tests for fetch_meta from cache
    // -----------------------------------------------------------------------

    #[test]
    fn fetch_meta_from_cache_skill() {
        let (_root, cache) = test_cache();
        cache_skill(&cache, "test-skill", "1.0.0", &["dep-a"]);

        let client = RegistryClient::new("http://localhost:99999");
        let v = Version::parse("1.0.0").unwrap();

        let meta = fetch_meta(&client, &cache, ResourceType::Skill, "test-skill", &v).unwrap();
        assert_eq!(meta.skills, vec!["dep-a"]);
    }

    #[test]
    fn fetch_meta_from_cache_no_deps() {
        let (_root, cache) = test_cache();
        cache_skill(&cache, "leaf", "1.0.0", &[]);

        let client = RegistryClient::new("http://localhost:99999");
        let v = Version::parse("1.0.0").unwrap();

        let meta = fetch_meta(&client, &cache, ResourceType::Skill, "leaf", &v).unwrap();
        assert!(meta.skills.is_empty());
        assert!(meta.agents.is_empty());
    }

    #[test]
    fn fetch_meta_from_cache_agent() {
        let (_root, cache) = test_cache();
        cache_agent(&cache, "my-agent", "0.5.0", &["skill-x"], &["agent-y"]);

        let client = RegistryClient::new("http://localhost:99999");
        let v = Version::parse("0.5.0").unwrap();

        let meta = fetch_meta(&client, &cache, ResourceType::Agent, "my-agent", &v).unwrap();
        assert_eq!(meta.skills, vec!["skill-x"]);
        assert_eq!(meta.agents, vec!["agent-y"]);
    }

    // -----------------------------------------------------------------------
    // Error display tests
    // -----------------------------------------------------------------------

    #[test]
    fn error_display_circular() {
        let err = ResolveError::CircularDependency("skill A -> skill B -> skill A".to_string());
        assert!(err.to_string().contains("circular dependency"));
        assert!(err.to_string().contains("A -> skill B -> skill A"));
    }

    #[test]
    fn error_display_depth_limit() {
        let err = ResolveError::DepthLimitExceeded(100);
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("depth limit"));
    }

    #[test]
    fn error_display_registry() {
        let err = ResolveError::Registry("not found".to_string());
        assert!(err.to_string().contains("registry error"));
    }

    #[test]
    fn error_display_frontmatter() {
        let err = ResolveError::Frontmatter("parse error".to_string());
        assert!(err.to_string().contains("frontmatter"));
    }

    // -----------------------------------------------------------------------
    // ResolvedResource tests
    // -----------------------------------------------------------------------

    #[test]
    fn resolved_resource_equality() {
        let a = ResolvedResource {
            resource_type: ResourceType::Skill,
            name: "test".to_string(),
            version: Version::parse("1.0.0").unwrap(),
        };
        let b = ResolvedResource {
            resource_type: ResourceType::Skill,
            name: "test".to_string(),
            version: Version::parse("1.0.0").unwrap(),
        };
        assert_eq!(a, b);
    }

    #[test]
    fn resolved_resource_clone() {
        let a = ResolvedResource {
            resource_type: ResourceType::Agent,
            name: "debugger".to_string(),
            version: Version::parse("0.5.0").unwrap(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
