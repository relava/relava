use std::collections::BTreeMap;
use std::path::Path;

use relava_types::validate::ResourceType;
use serde::{Deserialize, Serialize};

use crate::bulk_install::BulkInstallResult;
use crate::install::DepInstallResult;
use crate::update::UpdateResult;

/// The lockfile filename.
const LOCKFILE_NAME: &str = "relava.lock";

/// Current lockfile schema version.
const LOCKFILE_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A top-level resource explicitly installed by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectInstall {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
}

/// A package entry in the lockfile, tracking a single installed resource
/// and which other packages depend on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageEntry {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub version: String,
    /// Keys of packages that caused this one to be installed (empty for
    /// direct installs with no reverse dependencies).
    pub dependents: Vec<String>,
}

/// The `relava.lock` file content. Tracks exact state of installed resources
/// for reproducibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    pub version: u32,
    pub direct_installs: Vec<DirectInstall>,
    /// Keyed by `"type:name:version"`.
    pub packages: BTreeMap<String, PackageEntry>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            direct_installs: Vec::new(),
            packages: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Key helpers
// ---------------------------------------------------------------------------

/// Build the `packages` map key for a resource: `"type:name:version"`.
pub fn package_key(resource_type: &str, name: &str, version: &str) -> String {
    format!("{resource_type}:{name}:{version}")
}

/// Build a package key from typed args.
pub fn package_key_typed(resource_type: ResourceType, name: &str, version: &str) -> String {
    package_key(&resource_type.to_string(), name, version)
}

// ---------------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------------

impl Lockfile {
    /// Load `relava.lock` from the project directory.
    /// Returns `Ok(None)` if the file does not exist.
    /// Returns `Err` if the file exists but cannot be parsed.
    pub fn load(project_dir: &Path) -> Result<Option<Self>, String> {
        let path = project_dir.join(LOCKFILE_NAME);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let lockfile: Self = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        Ok(Some(lockfile))
    }

    /// Write the lockfile to the project directory as pretty-printed JSON.
    pub fn save(&self, project_dir: &Path) -> Result<(), String> {
        let path = project_dir.join(LOCKFILE_NAME);
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize lockfile: {e}"))?;
        std::fs::write(&path, format!("{content}\n"))
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Load existing lockfile or create a new empty one.
    pub fn load_or_default(project_dir: &Path) -> Result<Self, String> {
        Ok(Self::load(project_dir)?.unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Mutation methods
// ---------------------------------------------------------------------------

impl Lockfile {
    /// Record a direct install (top-level user request).
    ///
    /// Adds to `directInstalls` and `packages`. If the resource is already
    /// in `directInstalls` at a different version, the old entry is replaced.
    pub fn add_direct_install(&mut self, resource_type: ResourceType, name: &str, version: &str) {
        let rt = resource_type.to_string();

        // If upgrading from a different version, remove the old package entry
        // to avoid stale entries that would confuse `locked_version()`.
        if let Some(old) = self
            .direct_installs
            .iter()
            .find(|d| d.resource_type == rt && d.name == name && d.version != version)
        {
            let old_key = package_key(&rt, name, &old.version);
            self.remove_package_and_orphans(&old_key);
        }

        // Replace or add in directInstalls
        self.direct_installs
            .retain(|d| !(d.resource_type == rt && d.name == name));
        self.direct_installs.push(DirectInstall {
            resource_type: rt.clone(),
            name: name.to_string(),
            version: version.to_string(),
        });

        // Ensure it exists in packages
        let key = package_key(&rt, name, version);
        self.packages.entry(key).or_insert_with(|| PackageEntry {
            resource_type: rt,
            name: name.to_string(),
            version: version.to_string(),
            dependents: Vec::new(),
        });
    }

    /// Record a transitive dependency.
    ///
    /// `dependent_key` is the package key of the resource that depends on
    /// this one (e.g. `"skill:denden:1.2.0"`).
    pub fn add_dependency(
        &mut self,
        resource_type: ResourceType,
        name: &str,
        version: &str,
        dependent_key: &str,
    ) {
        let rt = resource_type.to_string();
        let key = package_key(&rt, name, version);

        let entry = self.packages.entry(key).or_insert_with(|| PackageEntry {
            resource_type: rt,
            name: name.to_string(),
            version: version.to_string(),
            dependents: Vec::new(),
        });

        if !entry.dependents.iter().any(|d| d == dependent_key) {
            entry.dependents.push(dependent_key.to_string());
        }
    }

    /// Remove a direct install and clean up orphaned transitive dependencies.
    ///
    /// Returns the list of package keys that were removed (including the
    /// resource itself and any orphaned dependencies).
    pub fn remove_direct_install(
        &mut self,
        resource_type: ResourceType,
        name: &str,
    ) -> Vec<String> {
        let rt = resource_type.to_string();

        // Find the version from directInstalls (needed for the package key)
        let version = self
            .direct_installs
            .iter()
            .find(|d| d.resource_type == rt && d.name == name)
            .map(|d| d.version.clone());

        // Remove from directInstalls
        self.direct_installs
            .retain(|d| !(d.resource_type == rt && d.name == name));

        let Some(version) = version else {
            return Vec::new();
        };

        let root_key = package_key(&rt, name, &version);
        self.remove_package_and_orphans(&root_key)
    }

    /// Remove a package and recursively remove any dependencies that become
    /// orphaned (no remaining dependents and not a direct install).
    fn remove_package_and_orphans(&mut self, key: &str) -> Vec<String> {
        let mut removed = Vec::new();

        if self.packages.remove(key).is_none() {
            return removed;
        };
        removed.push(key.to_string());

        // Remove this key from all other packages' dependents lists
        for pkg in self.packages.values_mut() {
            pkg.dependents.retain(|d| d != key);
        }

        // Find orphaned packages: no dependents and not a direct install
        let orphans: Vec<String> = self
            .packages
            .iter()
            .filter(|(_, pkg)| {
                pkg.dependents.is_empty() && !self.is_direct_install(&pkg.resource_type, &pkg.name)
            })
            .map(|(k, _)| k.clone())
            .collect();

        // Recursively remove orphans
        for orphan_key in orphans {
            removed.extend(self.remove_package_and_orphans(&orphan_key));
        }

        removed
    }

    /// Check if a resource is in the directInstalls list.
    fn is_direct_install(&self, resource_type: &str, name: &str) -> bool {
        self.direct_installs
            .iter()
            .any(|d| d.resource_type == resource_type && d.name == name)
    }

    /// Update a package's version. Handles both the directInstalls entry and
    /// the packages map (re-keys the entry).
    pub fn update_package(
        &mut self,
        resource_type: ResourceType,
        name: &str,
        old_version: &str,
        new_version: &str,
    ) {
        let rt = resource_type.to_string();
        let old_key = package_key(&rt, name, old_version);
        let new_key = package_key(&rt, name, new_version);

        // Update directInstalls version
        for d in &mut self.direct_installs {
            if d.resource_type == rt && d.name == name {
                d.version = new_version.to_string();
            }
        }

        // Move the package entry to the new key
        if let Some(mut entry) = self.packages.remove(&old_key) {
            entry.version = new_version.to_string();
            self.packages.insert(new_key.clone(), entry);
        } else {
            // Old version not in lockfile — lockfile may be out of sync.
            eprintln!("[warn] lockfile: {old_key} not found during update — creating {new_key}");
            self.packages
                .entry(new_key.clone())
                .or_insert_with(|| PackageEntry {
                    resource_type: rt.clone(),
                    name: name.to_string(),
                    version: new_version.to_string(),
                    dependents: Vec::new(),
                });
        }

        // Update all dependents references from old_key to new_key
        for pkg in self.packages.values_mut() {
            for dep in &mut pkg.dependents {
                if *dep == old_key {
                    *dep = new_key.clone();
                }
            }
        }
    }

    /// Look up the locked version for a resource.
    /// Returns `None` if not found in the lockfile.
    pub fn locked_version(&self, resource_type: ResourceType, name: &str) -> Option<String> {
        let rt = resource_type.to_string();
        self.packages
            .values()
            .find(|pkg| pkg.resource_type == rt && pkg.name == name)
            .map(|pkg| pkg.version.clone())
    }
}

// ---------------------------------------------------------------------------
// Integration helpers — called from main.rs after each command
// ---------------------------------------------------------------------------

/// Update the lockfile after a single `relava install <type> <name>`.
pub fn update_after_install(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
    version: &str,
    dependencies: &[DepInstallResult],
) -> Result<(), String> {
    let mut lf = Lockfile::load_or_default(project_dir)?;

    lf.add_direct_install(resource_type, name, version);

    // Record transitive dependencies
    let root_key = package_key_typed(resource_type, name, version);
    for dep in dependencies {
        let dep_rt = ResourceType::from_str(&dep.resource_type)
            .map_err(|e| format!("invalid resource type in dependency: {e}"))?;
        lf.add_dependency(dep_rt, &dep.name, &dep.version, &root_key);
    }

    lf.save(project_dir)
}

/// Update the lockfile after `relava remove <type> <name>`.
pub fn update_after_remove(
    project_dir: &Path,
    resource_type: ResourceType,
    name: &str,
) -> Result<(), String> {
    let mut lf = Lockfile::load_or_default(project_dir)?;
    lf.remove_direct_install(resource_type, name);
    lf.save(project_dir)
}

/// Update the lockfile after `relava update`.
pub fn update_after_update(project_dir: &Path, result: &UpdateResult) -> Result<(), String> {
    if result.updated.is_empty() {
        return Ok(());
    }

    let mut lf = Lockfile::load_or_default(project_dir)?;

    for entry in &result.updated {
        let rt = ResourceType::from_str(&entry.resource_type)
            .map_err(|e| format!("invalid resource type in update result: {e}"))?;
        lf.update_package(rt, &entry.name, &entry.old_version, &entry.new_version);
    }

    lf.save(project_dir)
}

/// Update the lockfile after `relava install relava.toml` (bulk install).
pub fn update_after_bulk_install(
    project_dir: &Path,
    result: &BulkInstallResult,
) -> Result<(), String> {
    if result.installed.is_empty() {
        return Ok(());
    }

    let mut lf = Lockfile::load_or_default(project_dir)?;

    for entry in &result.installed {
        let rt = ResourceType::from_str(&entry.resource_type)
            .map_err(|e| format!("invalid resource type in bulk install result: {e}"))?;
        lf.add_direct_install(rt, &entry.name, &entry.version);
    }

    lf.save(project_dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        TempDir::new().expect("failed to create temp dir")
    }

    // --- Lockfile default ---

    #[test]
    fn default_lockfile_has_version_1() {
        let lf = Lockfile::default();
        assert_eq!(lf.version, 1);
        assert!(lf.direct_installs.is_empty());
        assert!(lf.packages.is_empty());
    }

    // --- package_key ---

    #[test]
    fn package_key_format() {
        assert_eq!(
            package_key("skill", "denden", "1.2.0"),
            "skill:denden:1.2.0"
        );
    }

    #[test]
    fn package_key_typed_format() {
        assert_eq!(
            package_key_typed(ResourceType::Agent, "debugger", "0.5.0"),
            "agent:debugger:0.5.0"
        );
    }

    // --- I/O ---

    #[test]
    fn load_missing_returns_none() {
        let root = temp_dir();
        let result = Lockfile::load(root.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let root = temp_dir();
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        lf.save(root.path()).unwrap();
        let loaded = Lockfile::load(root.path()).unwrap().unwrap();
        assert_eq!(lf, loaded);
    }

    #[test]
    fn save_produces_valid_json() {
        let root = temp_dir();
        let lf = Lockfile::default();
        lf.save(root.path()).unwrap();

        let content = fs::read_to_string(root.path().join("relava.lock")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["version"], 1);
    }

    #[test]
    fn save_is_human_readable() {
        let root = temp_dir();
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.save(root.path()).unwrap();

        let content = fs::read_to_string(root.path().join("relava.lock")).unwrap();
        // Pretty-printed JSON has newlines
        assert!(content.contains('\n'));
        assert!(content.contains("  "));
    }

    #[test]
    fn load_invalid_json_returns_error() {
        let root = temp_dir();
        fs::write(root.path().join("relava.lock"), "not json {{{").unwrap();
        let result = Lockfile::load(root.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_or_default_missing_file() {
        let root = temp_dir();
        let lf = Lockfile::load_or_default(root.path()).unwrap();
        assert_eq!(lf, Lockfile::default());
    }

    #[test]
    fn load_or_default_existing_file() {
        let root = temp_dir();
        let mut original = Lockfile::default();
        original.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        original.save(root.path()).unwrap();

        let loaded = Lockfile::load_or_default(root.path()).unwrap();
        assert_eq!(loaded, original);
    }

    // --- add_direct_install ---

    #[test]
    fn add_direct_install_creates_entries() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        assert_eq!(lf.direct_installs.len(), 1);
        assert_eq!(lf.direct_installs[0].name, "denden");
        assert_eq!(lf.direct_installs[0].version, "1.2.0");

        let key = "skill:denden:1.2.0";
        assert!(lf.packages.contains_key(key));
        assert!(lf.packages[key].dependents.is_empty());
    }

    #[test]
    fn add_direct_install_replaces_old_version() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.0.0");
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        // Should have only one directInstalls entry
        assert_eq!(lf.direct_installs.len(), 1);
        assert_eq!(lf.direct_installs[0].version, "1.2.0");

        // Old version should be removed from packages
        assert!(!lf.packages.contains_key("skill:denden:1.0.0"));
        assert!(lf.packages.contains_key("skill:denden:1.2.0"));
        assert_eq!(lf.packages.len(), 1);

        // locked_version should return the new version
        assert_eq!(
            lf.locked_version(ResourceType::Skill, "denden"),
            Some("1.2.0".to_string())
        );
    }

    #[test]
    fn add_multiple_direct_installs() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.add_direct_install(ResourceType::Agent, "debugger", "0.5.0");

        assert_eq!(lf.direct_installs.len(), 2);
        assert_eq!(lf.packages.len(), 2);
    }

    // --- add_dependency ---

    #[test]
    fn add_dependency_creates_entry_with_dependent() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );

        let key = "skill:notify-slack:0.3.0";
        assert!(lf.packages.contains_key(key));
        assert_eq!(lf.packages[key].dependents, vec!["skill:denden:1.2.0"]);
    }

    #[test]
    fn add_dependency_deduplicates_dependents() {
        let mut lf = Lockfile::default();
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );

        let key = "skill:notify-slack:0.3.0";
        assert_eq!(lf.packages[key].dependents.len(), 1);
    }

    #[test]
    fn add_dependency_multiple_dependents() {
        let mut lf = Lockfile::default();
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:code-review:2.0.0",
        );

        let key = "skill:notify-slack:0.3.0";
        assert_eq!(lf.packages[key].dependents.len(), 2);
    }

    // --- remove_direct_install ---

    #[test]
    fn remove_direct_install_cleans_up() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        let removed = lf.remove_direct_install(ResourceType::Skill, "denden");

        assert!(lf.direct_installs.is_empty());
        assert!(lf.packages.is_empty());
        assert_eq!(removed, vec!["skill:denden:1.2.0"]);
    }

    #[test]
    fn remove_direct_install_removes_orphaned_deps() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );

        let removed = lf.remove_direct_install(ResourceType::Skill, "denden");

        assert!(lf.direct_installs.is_empty());
        assert!(lf.packages.is_empty());
        assert!(removed.contains(&"skill:denden:1.2.0".to_string()));
        assert!(removed.contains(&"skill:notify-slack:0.3.0".to_string()));
    }

    #[test]
    fn remove_preserves_shared_deps() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.add_direct_install(ResourceType::Skill, "code-review", "2.0.0");

        // notify-slack is depended on by both
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:code-review:2.0.0",
        );

        // Remove denden — notify-slack should survive
        lf.remove_direct_install(ResourceType::Skill, "denden");

        assert_eq!(lf.direct_installs.len(), 1);
        assert!(lf.packages.contains_key("skill:notify-slack:0.3.0"));
        assert_eq!(
            lf.packages["skill:notify-slack:0.3.0"].dependents,
            vec!["skill:code-review:2.0.0"]
        );
    }

    #[test]
    fn remove_nonexistent_is_noop() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        let removed = lf.remove_direct_install(ResourceType::Skill, "nonexistent");

        assert!(removed.is_empty());
        assert_eq!(lf.direct_installs.len(), 1);
    }

    #[test]
    fn remove_cascading_orphans() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "root", "1.0.0");
        // root -> mid -> leaf
        lf.add_dependency(ResourceType::Skill, "mid", "1.0.0", "skill:root:1.0.0");
        lf.add_dependency(ResourceType::Skill, "leaf", "1.0.0", "skill:mid:1.0.0");

        let removed = lf.remove_direct_install(ResourceType::Skill, "root");

        assert!(lf.packages.is_empty());
        assert_eq!(removed.len(), 3);
    }

    // --- update_package ---

    #[test]
    fn update_package_changes_version() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.0.0");

        lf.update_package(ResourceType::Skill, "denden", "1.0.0", "2.0.0");

        assert_eq!(lf.direct_installs[0].version, "2.0.0");
        assert!(!lf.packages.contains_key("skill:denden:1.0.0"));
        assert!(lf.packages.contains_key("skill:denden:2.0.0"));
    }

    #[test]
    fn update_package_updates_dependent_references() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.0.0");
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.0.0",
        );

        lf.update_package(ResourceType::Skill, "denden", "1.0.0", "2.0.0");

        let dep = &lf.packages["skill:notify-slack:0.3.0"];
        assert_eq!(dep.dependents, vec!["skill:denden:2.0.0"]);
    }

    #[test]
    fn update_package_nonexistent_creates_entry() {
        let mut lf = Lockfile::default();
        lf.update_package(ResourceType::Skill, "denden", "1.0.0", "2.0.0");

        assert!(lf.packages.contains_key("skill:denden:2.0.0"));
    }

    // --- locked_version ---

    #[test]
    fn locked_version_found() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");

        assert_eq!(
            lf.locked_version(ResourceType::Skill, "denden"),
            Some("1.2.0".to_string())
        );
    }

    #[test]
    fn locked_version_not_found() {
        let lf = Lockfile::default();
        assert_eq!(lf.locked_version(ResourceType::Skill, "denden"), None);
    }

    // --- is_direct_install ---

    #[test]
    fn is_direct_install_true() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        assert!(lf.is_direct_install("skill", "denden"));
    }

    #[test]
    fn is_direct_install_false() {
        let lf = Lockfile::default();
        assert!(!lf.is_direct_install("skill", "denden"));
    }

    // --- Serialization format ---

    #[test]
    fn serializes_to_camel_case() {
        let lf = Lockfile::default();
        let json = serde_json::to_string(&lf).unwrap();
        assert!(json.contains("directInstalls"));
        assert!(!json.contains("direct_installs"));
    }

    #[test]
    fn matches_design_md_format() {
        let mut lf = Lockfile::default();
        lf.add_direct_install(ResourceType::Skill, "denden", "1.2.0");
        lf.add_direct_install(ResourceType::Agent, "debugger", "0.5.0");
        lf.add_dependency(
            ResourceType::Skill,
            "notify-slack",
            "0.3.0",
            "skill:denden:1.2.0",
        );

        let json = serde_json::to_string_pretty(&lf).unwrap();

        // Verify the key fields exist in the expected format
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"directInstalls\""));
        assert!(json.contains("\"skill:denden:1.2.0\""));
        assert!(json.contains("\"skill:notify-slack:0.3.0\""));
        assert!(json.contains("\"agent:debugger:0.5.0\""));

        // Verify it's valid JSON that round-trips
        let parsed: Lockfile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, lf);
    }

    // --- Integration helper tests ---

    #[test]
    fn update_after_install_creates_lockfile() {
        let root = temp_dir();
        update_after_install(root.path(), ResourceType::Skill, "denden", "1.2.0", &[]).unwrap();

        let lf = Lockfile::load(root.path()).unwrap().unwrap();
        assert_eq!(lf.direct_installs.len(), 1);
        assert_eq!(lf.direct_installs[0].name, "denden");
    }

    #[test]
    fn update_after_install_with_deps() {
        let root = temp_dir();
        let deps = vec![DepInstallResult {
            resource_type: "skill".to_string(),
            name: "notify-slack".to_string(),
            version: "0.3.0".to_string(),
            status: "installed".to_string(),
        }];
        update_after_install(root.path(), ResourceType::Skill, "denden", "1.2.0", &deps).unwrap();

        let lf = Lockfile::load(root.path()).unwrap().unwrap();
        assert_eq!(lf.packages.len(), 2);
        let dep_key = "skill:notify-slack:0.3.0";
        assert_eq!(lf.packages[dep_key].dependents, vec!["skill:denden:1.2.0"]);
    }

    #[test]
    fn update_after_remove_cleans_lockfile() {
        let root = temp_dir();
        // First install
        update_after_install(root.path(), ResourceType::Skill, "denden", "1.2.0", &[]).unwrap();
        // Then remove
        update_after_remove(root.path(), ResourceType::Skill, "denden").unwrap();

        let lf = Lockfile::load(root.path()).unwrap().unwrap();
        assert!(lf.direct_installs.is_empty());
        assert!(lf.packages.is_empty());
    }

    #[test]
    fn update_after_update_changes_version() {
        let root = temp_dir();
        update_after_install(root.path(), ResourceType::Skill, "denden", "1.0.0", &[]).unwrap();

        let result = UpdateResult {
            updated: vec![crate::update::UpdateEntry {
                resource_type: "skill".to_string(),
                name: "denden".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "2.0.0".to_string(),
                status: "updated".to_string(),
            }],
            up_to_date: Vec::new(),
            skipped: Vec::new(),
        };
        update_after_update(root.path(), &result).unwrap();

        let lf = Lockfile::load(root.path()).unwrap().unwrap();
        assert_eq!(lf.direct_installs[0].version, "2.0.0");
        assert!(lf.packages.contains_key("skill:denden:2.0.0"));
    }

    #[test]
    fn update_after_update_noop_when_empty() {
        let root = temp_dir();
        let result = UpdateResult::default();
        // Should not create a lockfile when nothing was updated
        update_after_update(root.path(), &result).unwrap();
        assert!(Lockfile::load(root.path()).unwrap().is_none());
    }

    #[test]
    fn update_after_bulk_install_records_all() {
        let root = temp_dir();
        let result = BulkInstallResult {
            installed: vec![
                crate::bulk_install::BulkEntry {
                    resource_type: "skill".to_string(),
                    name: "denden".to_string(),
                    version: "1.2.0".to_string(),
                    status: "installed".to_string(),
                    error: None,
                },
                crate::bulk_install::BulkEntry {
                    resource_type: "agent".to_string(),
                    name: "debugger".to_string(),
                    version: "0.5.0".to_string(),
                    status: "installed".to_string(),
                    error: None,
                },
            ],
            skipped: Vec::new(),
            failed: Vec::new(),
        };

        update_after_bulk_install(root.path(), &result).unwrap();

        let lf = Lockfile::load(root.path()).unwrap().unwrap();
        assert_eq!(lf.direct_installs.len(), 2);
        assert_eq!(lf.packages.len(), 2);
    }

    #[test]
    fn update_after_bulk_install_noop_when_empty() {
        let root = temp_dir();
        let result = BulkInstallResult::default();
        update_after_bulk_install(root.path(), &result).unwrap();
        assert!(Lockfile::load(root.path()).unwrap().is_none());
    }
}
