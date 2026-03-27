//! Server-side dependency resolution with topological sort.
//!
//! Performs a depth-first traversal of the dependency graph stored in the
//! registry, reading `manifest_json` from each version to discover
//! sub-dependencies. Returns a leaf-first install order suitable for
//! sequential installation without missing prerequisites.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::store::models::{Resource, Version};
use crate::store::traits::{ResourceStore, StoreError};
use relava_types::validate::ResourceType;

/// Maximum dependency tree depth to prevent runaway recursion.
const MAX_DEPTH: usize = 100;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single entry in the resolved install order.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedDep {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
}

/// Full resolution response returned by the endpoint.
#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    pub root: String,
    pub order: Vec<ResolvedDep>,
}

/// Errors that can occur during resolution.
#[derive(Debug)]
pub enum ResolveError {
    /// A resource or version was not found in the store.
    NotFound(String),
    /// Circular dependency detected. Contains the cycle path.
    CyclicDependency(Vec<String>),
    /// Dependency depth exceeds the limit.
    DepthLimitExceeded { depth: usize, limit: usize },
    /// Store error during resolution.
    Store(StoreError),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(msg) => write!(f, "{msg}"),
            Self::CyclicDependency(cycle) => {
                write!(f, "circular dependency detected: {}", cycle.join(" -> "))
            }
            Self::DepthLimitExceeded { depth, limit } => {
                write!(f, "dependency depth {depth} exceeds limit of {limit}")
            }
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl From<StoreError> for ResolveError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(msg) => Self::NotFound(msg),
            other => Self::Store(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Manifest dependencies (parsed from manifest_json)
// ---------------------------------------------------------------------------

/// Subset of manifest_json we need for dependency resolution.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, derive(Serialize))]
struct ManifestDeps {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    agents: Vec<String>,
}

/// Extract dependency lists from a version's manifest_json.
fn parse_deps(version: &Version) -> ManifestDeps {
    version
        .manifest_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Resolution algorithm
// ---------------------------------------------------------------------------

/// Mutable state threaded through the DFS traversal.
struct DfsState {
    /// Resources already fully resolved (deduplication).
    resolved: HashMap<(ResourceType, String), String>,
    /// Current traversal path (cycle detection).
    path: Vec<(ResourceType, String)>,
    /// Leaf-first install order being built.
    order: Vec<ResolvedDep>,
}

/// Resolve all transitive dependencies for a resource, returning a leaf-first
/// install order.
///
/// Uses the store to look up resources and their version manifests. Detects
/// cycles and enforces a depth limit.
pub fn resolve<S: ResourceStore>(
    store: &S,
    resource_type: ResourceType,
    name: &str,
    version_str: Option<&str>,
) -> Result<ResolveResponse, ResolveError> {
    let mut state = DfsState {
        resolved: HashMap::new(),
        path: Vec::new(),
        order: Vec::new(),
    };

    let root_version = dfs(store, resource_type, name, version_str, 0, &mut state)?;

    Ok(ResolveResponse {
        root: format!("{resource_type}/{name}@{root_version}"),
        order: state.order,
    })
}

/// Depth-first traversal that builds the leaf-first install order.
fn dfs<S: ResourceStore>(
    store: &S,
    resource_type: ResourceType,
    name: &str,
    version_hint: Option<&str>,
    depth: usize,
    state: &mut DfsState,
) -> Result<String, ResolveError> {
    if depth > MAX_DEPTH {
        return Err(ResolveError::DepthLimitExceeded {
            depth,
            limit: MAX_DEPTH,
        });
    }

    let key = (resource_type, name.to_string());

    // Cycle detection
    if let Some(pos) = state.path.iter().position(|k| *k == key) {
        let cycle: Vec<String> = state.path[pos..]
            .iter()
            .map(|(rt, n)| format!("{rt}/{n}"))
            .chain(std::iter::once(format!("{resource_type}/{name}")))
            .collect();
        return Err(ResolveError::CyclicDependency(cycle));
    }

    // Already resolved — skip (deduplication)
    if let Some(ver) = state.resolved.get(&key) {
        return Ok(ver.clone());
    }

    state.path.push(key.clone());

    // Look up the resource in the store
    let resource = store.get_resource(None, name, resource_type)?;

    // Determine which version to resolve
    let version = resolve_version(store, &resource, version_hint)?;
    let version_str = version.version.clone();

    // Parse dependencies from manifest
    let deps = parse_deps(&version);

    // Recurse into dependencies (skills first, then agents)
    let dep_entries: Vec<(ResourceType, &str)> = deps
        .skills
        .iter()
        .map(|n| (ResourceType::Skill, n.as_str()))
        .chain(
            deps.agents
                .iter()
                .map(|n| (ResourceType::Agent, n.as_str())),
        )
        .collect();

    for (dep_type, dep_name) in dep_entries {
        dfs(store, dep_type, dep_name, None, depth + 1, state)?;
    }

    // Done resolving — pop from path, record in resolved map
    state.path.pop();
    state.resolved.insert(key, version_str.clone());

    // Post-order: add after all children (leaf-first)
    state.order.push(ResolvedDep {
        resource_type: resource_type.to_string(),
        name: name.to_string(),
        version: version_str.clone(),
    });

    Ok(version_str)
}

/// Resolve which version to use for a resource.
///
/// If `version_hint` is provided, looks up that specific version.
/// Otherwise, uses the resource's `latest_version` field and falls back
/// to the first version in the list.
fn resolve_version<S: ResourceStore>(
    store: &S,
    resource: &Resource,
    version_hint: Option<&str>,
) -> Result<Version, ResolveError> {
    if let Some(ver) = version_hint {
        return store
            .get_version(resource.id, ver)
            .map_err(ResolveError::from);
    }

    // Use latest_version if available
    if let Some(ref latest) = resource.latest_version {
        return store
            .get_version(resource.id, latest)
            .map_err(ResolveError::from);
    }

    // Fallback: pick the first version
    let versions = store.list_versions(resource.id)?;
    versions.into_iter().next().ok_or_else(|| {
        ResolveError::NotFound(format!(
            "no versions published for {} '{}'",
            resource.resource_type, resource.name
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::SqliteResourceStore;
    use crate::store::models::{Resource, Version};

    fn test_store() -> SqliteResourceStore {
        SqliteResourceStore::open_in_memory().unwrap()
    }

    fn publish_resource(
        store: &SqliteResourceStore,
        rtype: &str,
        name: &str,
        version: &str,
        deps: &ManifestDeps,
    ) {
        let resource = Resource {
            id: 0,
            scope: None,
            name: name.to_string(),
            resource_type: rtype.to_string(),
            description: None,
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let manifest_json = serde_json::to_string(deps).unwrap();
        let ver = Version {
            id: 0,
            resource_id: 0,
            version: version.to_string(),
            store_path: None,
            checksum: None,
            manifest_json: Some(manifest_json),
            published_by: None,
            published_at: None,
        };
        store.publish(&resource, &ver).unwrap();
    }

    fn no_deps() -> ManifestDeps {
        ManifestDeps::default()
    }

    fn skill_deps(skills: &[&str]) -> ManifestDeps {
        ManifestDeps {
            skills: skills.iter().map(|s| s.to_string()).collect(),
            agents: Vec::new(),
        }
    }

    fn mixed_deps(skills: &[&str], agents: &[&str]) -> ManifestDeps {
        ManifestDeps {
            skills: skills.iter().map(|s| s.to_string()).collect(),
            agents: agents.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn names(response: &ResolveResponse) -> Vec<&str> {
        response.order.iter().map(|d| d.name.as_str()).collect()
    }

    fn types_and_names(response: &ResolveResponse) -> Vec<(&str, &str)> {
        response
            .order
            .iter()
            .map(|d| (d.resource_type.as_str(), d.name.as_str()))
            .collect()
    }

    // -- No dependencies --

    #[test]
    fn resolve_single_resource() {
        let store = test_store();
        publish_resource(&store, "skill", "denden", "1.0.0", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "denden", None).unwrap();
        assert_eq!(names(&result), vec!["denden"]);
        assert_eq!(result.root, "skill/denden@1.0.0");
    }

    #[test]
    fn resolve_with_specific_version() {
        let store = test_store();
        publish_resource(&store, "skill", "denden", "1.0.0", &no_deps());
        publish_resource(&store, "skill", "denden", "2.0.0", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "denden", Some("1.0.0")).unwrap();
        assert_eq!(result.root, "skill/denden@1.0.0");
        assert_eq!(result.order[0].version, "1.0.0");
    }

    // -- Simple dependency chain --

    #[test]
    fn resolve_single_dependency() {
        let store = test_store();
        publish_resource(
            &store,
            "skill",
            "code-review",
            "1.0.0",
            &skill_deps(&["security-baseline"]),
        );
        publish_resource(&store, "skill", "security-baseline", "1.0.0", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "code-review", None).unwrap();
        assert_eq!(names(&result), vec!["security-baseline", "code-review"]);
    }

    #[test]
    fn resolve_chain_three_deep() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["b"]));
        publish_resource(&store, "skill", "b", "1.0.0", &skill_deps(&["c"]));
        publish_resource(&store, "skill", "c", "1.0.0", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "a", None).unwrap();
        assert_eq!(names(&result), vec!["c", "b", "a"]);
    }

    // -- Diamond dependency (deduplication) --

    #[test]
    fn resolve_diamond_deduplicates() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["b", "c"]));
        publish_resource(&store, "skill", "b", "1.0.0", &skill_deps(&["d"]));
        publish_resource(&store, "skill", "c", "1.0.0", &skill_deps(&["d"]));
        publish_resource(&store, "skill", "d", "1.0.0", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "a", None).unwrap();
        // d appears once, before b and c
        let order = names(&result);
        assert_eq!(order.len(), 4);
        let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
        assert!(pos("d") < pos("b"));
        assert!(pos("d") < pos("c"));
        assert!(pos("b") < pos("a"));
        assert!(pos("c") < pos("a"));
    }

    // -- Mixed resource types --

    #[test]
    fn resolve_mixed_skill_and_agent_deps() {
        let store = test_store();
        publish_resource(
            &store,
            "agent",
            "orchestrator",
            "1.0.0",
            &mixed_deps(&["notify-slack"], &["debugger"]),
        );
        publish_resource(&store, "skill", "notify-slack", "0.3.0", &no_deps());
        publish_resource(
            &store,
            "agent",
            "debugger",
            "0.5.0",
            &skill_deps(&["log-capture"]),
        );
        publish_resource(&store, "skill", "log-capture", "0.2.0", &no_deps());

        let result = resolve(&store, ResourceType::Agent, "orchestrator", None).unwrap();
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

    // -- Cycle detection --

    #[test]
    fn resolve_detects_direct_cycle() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["b"]));
        publish_resource(&store, "skill", "b", "1.0.0", &skill_deps(&["a"]));

        let err = resolve(&store, ResourceType::Skill, "a", None).unwrap_err();
        match err {
            ResolveError::CyclicDependency(cycle) => {
                assert_eq!(cycle, vec!["skill/a", "skill/b", "skill/a"]);
            }
            other => panic!("expected CyclicDependency, got: {other}"),
        }
    }

    #[test]
    fn resolve_detects_indirect_cycle() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["b"]));
        publish_resource(&store, "skill", "b", "1.0.0", &skill_deps(&["c"]));
        publish_resource(&store, "skill", "c", "1.0.0", &skill_deps(&["a"]));

        let err = resolve(&store, ResourceType::Skill, "a", None).unwrap_err();
        match err {
            ResolveError::CyclicDependency(cycle) => {
                assert_eq!(cycle, vec!["skill/a", "skill/b", "skill/c", "skill/a"]);
            }
            other => panic!("expected CyclicDependency, got: {other}"),
        }
    }

    #[test]
    fn resolve_detects_self_dependency() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["a"]));

        let err = resolve(&store, ResourceType::Skill, "a", None).unwrap_err();
        match err {
            ResolveError::CyclicDependency(cycle) => {
                assert_eq!(cycle, vec!["skill/a", "skill/a"]);
            }
            other => panic!("expected CyclicDependency, got: {other}"),
        }
    }

    #[test]
    fn cycle_error_message_format() {
        let err = ResolveError::CyclicDependency(vec![
            "skill/a".to_string(),
            "skill/b".to_string(),
            "skill/a".to_string(),
        ]);
        assert_eq!(
            err.to_string(),
            "circular dependency detected: skill/a -> skill/b -> skill/a"
        );
    }

    // -- Not found --

    #[test]
    fn resolve_resource_not_found() {
        let store = test_store();
        let err = resolve(&store, ResourceType::Skill, "nonexistent", None).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn resolve_missing_dependency() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["missing"]));

        let err = resolve(&store, ResourceType::Skill, "a", None).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    #[test]
    fn resolve_version_not_found() {
        let store = test_store();
        publish_resource(&store, "skill", "denden", "1.0.0", &no_deps());

        let err = resolve(&store, ResourceType::Skill, "denden", Some("9.9.9")).unwrap_err();
        assert!(matches!(err, ResolveError::NotFound(_)));
    }

    // -- No versions published --

    #[test]
    fn resolve_no_versions() {
        let store = test_store();
        let resource = Resource {
            id: 0,
            scope: None,
            name: "empty".to_string(),
            resource_type: "skill".to_string(),
            description: None,
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        store.create_resource(&resource).unwrap();

        let err = resolve(&store, ResourceType::Skill, "empty", None).unwrap_err();
        match err {
            ResolveError::NotFound(msg) => {
                assert!(msg.contains("no versions published"));
            }
            other => panic!("expected NotFound, got: {other}"),
        }
    }

    // -- Version preserved in output --

    #[test]
    fn resolve_preserves_versions() {
        let store = test_store();
        publish_resource(&store, "skill", "a", "2.1.0", &skill_deps(&["b"]));
        publish_resource(&store, "skill", "b", "0.3.1", &no_deps());

        let result = resolve(&store, ResourceType::Skill, "a", None).unwrap();
        assert_eq!(result.order[0].version, "0.3.1");
        assert_eq!(result.order[0].name, "b");
        assert_eq!(result.order[1].version, "2.1.0");
        assert_eq!(result.order[1].name, "a");
    }

    // -- Null/empty manifest_json --

    #[test]
    fn resolve_null_manifest_json() {
        let store = test_store();
        let resource = Resource {
            id: 0,
            scope: None,
            name: "bare".to_string(),
            resource_type: "skill".to_string(),
            description: None,
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let ver = Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: None, // no manifest
            published_by: None,
            published_at: None,
        };
        store.publish(&resource, &ver).unwrap();

        let result = resolve(&store, ResourceType::Skill, "bare", None).unwrap();
        assert_eq!(names(&result), vec!["bare"]);
    }

    // -- Complex graph --

    #[test]
    fn resolve_complex_graph() {
        let store = test_store();
        // A -> B, C
        // B -> D, E
        // C -> E, F
        // F -> D
        publish_resource(&store, "skill", "a", "1.0.0", &skill_deps(&["b", "c"]));
        publish_resource(&store, "skill", "b", "1.0.0", &skill_deps(&["d", "e"]));
        publish_resource(&store, "skill", "c", "1.0.0", &skill_deps(&["e", "f"]));
        publish_resource(&store, "skill", "d", "1.0.0", &no_deps());
        publish_resource(&store, "skill", "e", "1.0.0", &no_deps());
        publish_resource(&store, "skill", "f", "1.0.0", &skill_deps(&["d"]));

        let result = resolve(&store, ResourceType::Skill, "a", None).unwrap();
        let order = names(&result);

        // Each dep appears before its dependents
        let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
        assert!(pos("d") < pos("b"));
        assert!(pos("e") < pos("b"));
        assert!(pos("e") < pos("c"));
        assert!(pos("f") < pos("c"));
        assert!(pos("d") < pos("f"));
        assert!(pos("b") < pos("a"));
        assert!(pos("c") < pos("a"));

        // Each name appears exactly once
        assert_eq!(
            order.len(),
            order.iter().collect::<std::collections::HashSet<_>>().len()
        );
    }

    // -- parse_deps --

    #[test]
    fn parse_deps_valid_json() {
        let version = Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: Some(r#"{"skills":["foo","bar"],"agents":["baz"]}"#.to_string()),
            published_by: None,
            published_at: None,
        };
        let deps = parse_deps(&version);
        assert_eq!(deps.skills, vec!["foo", "bar"]);
        assert_eq!(deps.agents, vec!["baz"]);
    }

    #[test]
    fn parse_deps_invalid_json_returns_empty() {
        let version = Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: Some("not json".to_string()),
            published_by: None,
            published_at: None,
        };
        let deps = parse_deps(&version);
        assert!(deps.skills.is_empty());
        assert!(deps.agents.is_empty());
    }

    #[test]
    fn parse_deps_none_returns_empty() {
        let version = Version {
            id: 0,
            resource_id: 0,
            version: "1.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: None,
            published_by: None,
            published_at: None,
        };
        let deps = parse_deps(&version);
        assert!(deps.skills.is_empty());
        assert!(deps.agents.is_empty());
    }
}
