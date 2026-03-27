//! Seed default resources into the registry on server startup.
//!
//! Embeds bundled resource content at compile time and publishes it to the
//! store if it is missing or outdated. Idempotent: re-running with the same
//! version is a no-op.

use sha2::{Digest, Sha256};

use crate::store::db::SqliteResourceStore;
use crate::store::traits::{BlobStore, ResourceStore, StoreError};
use crate::store::{LocalBlobStore, models};
use relava_types::validate::ResourceType;
use relava_types::version::Version;

/// The relava skill content, embedded at compile time.
const BUNDLED_RELAVA_SKILL: &str = include_str!("../../../default-skills/relava/SKILL.md");

/// A resource that ships with the server binary.
struct BundledResource {
    name: &'static str,
    resource_type: ResourceType,
    content: &'static str,
    file_name: &'static str,
}

/// All resources bundled with the server.
fn bundled_resources() -> Vec<BundledResource> {
    vec![BundledResource {
        name: "relava",
        resource_type: ResourceType::Skill,
        content: BUNDLED_RELAVA_SKILL,
        file_name: "SKILL.md",
    }]
}

/// Parse YAML frontmatter from content, extracting `version` and `description`.
///
/// Returns `(version, description)` or an error if frontmatter is missing or
/// malformed.
fn parse_frontmatter(content: &str) -> Result<(String, String), String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err("missing frontmatter delimiters".to_string());
    }
    let after_open = &trimmed[3..];
    let end = after_open
        .find("\n---")
        .ok_or_else(|| "missing closing frontmatter delimiter".to_string())?;
    let yaml_str = &after_open[..end];

    let yaml_value: serde_json::Value =
        serde_yaml::from_str(yaml_str).map_err(|e| format!("invalid frontmatter YAML: {e}"))?;

    let version = yaml_value
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "frontmatter missing 'version' field".to_string())?
        .to_string();

    let description = yaml_value
        .get("description")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "frontmatter missing 'description' field".to_string())?
        .to_string();

    Ok((version, description))
}

/// Compute SHA-256 hex digest.
fn sha256_hex(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

/// Seed all bundled resources into the registry.
///
/// For each bundled resource:
/// - If missing from the registry, publish it.
/// - If the registry has an older version, publish the new version.
/// - If the registry has the same or newer version, skip (no-op).
/// - If the resource was previously removed, re-seed it.
///
/// Blob writes happen before database writes. On publish failure, the blob
/// is cleaned up (compensating transaction).
pub fn seed(store: &SqliteResourceStore, blob_store: &LocalBlobStore) -> Result<(), StoreError> {
    for res in bundled_resources() {
        seed_one(store, blob_store, &res)?;
    }
    Ok(())
}

/// Seed a single bundled resource.
fn seed_one(
    store: &SqliteResourceStore,
    blob_store: &LocalBlobStore,
    bundled: &BundledResource,
) -> Result<(), StoreError> {
    // 1. Parse frontmatter
    let (version_str, description) = parse_frontmatter(bundled.content)
        .map_err(|e| StoreError::Database(format!("bundled {} frontmatter: {e}", bundled.name)))?;

    // 2. Parse version
    let embedded_version = Version::parse(&version_str).map_err(|e| {
        StoreError::Database(format!(
            "bundled {} version '{}': {e}",
            bundled.name, version_str
        ))
    })?;

    // 3. Check existing resource
    let is_reseed = match store.get_resource(None, bundled.name, bundled.resource_type) {
        Ok(existing) => {
            // Resource exists — compare versions
            if let Some(ref latest) = existing.latest_version {
                if let Ok(registry_version) = Version::parse(latest) {
                    if registry_version >= embedded_version {
                        // Registry has same or newer version — skip
                        return Ok(());
                    }
                }
                // Registry version is older or unparseable — update
            }
            false
        }
        Err(StoreError::NotFound(_)) => {
            // Check if blob already exists (indicates re-seed after removal)
            let blob_path = format!(
                "{}/{}/{}/{}",
                bundled.resource_type.store_dir_name(),
                bundled.name,
                version_str,
                bundled.file_name
            );
            blob_store.exists(&blob_path).unwrap_or(false)
        }
        Err(e) => return Err(e),
    };

    // 4. Blob-first write with compensating transaction
    let checksum = sha256_hex(bundled.content.as_bytes());
    let store_path = format!(
        "{}/{}/{}",
        bundled.resource_type.store_dir_name(),
        bundled.name,
        version_str
    );
    let blob_path = format!("{store_path}/{}", bundled.file_name);

    blob_store.store(&blob_path, bundled.content.as_bytes())?;

    let resource = models::Resource {
        id: 0,
        scope: None,
        name: bundled.name.to_string(),
        resource_type: bundled.resource_type.store_dir_name().trim_end_matches('s').to_string(),
        description: Some(description),
        latest_version: None,
        metadata_json: None,
        updated_at: None,
    };

    let manifest_json = serde_json::to_string(&serde_json::json!({
        "files": [{ "path": bundled.file_name, "sha256": &checksum }]
    }))
    .ok();

    let version = models::Version {
        id: 0,
        resource_id: 0,
        version: version_str.clone(),
        store_path: Some(store_path),
        checksum: Some(checksum),
        manifest_json,
        published_by: Some("relava-server".to_string()),
        published_at: None,
    };

    match store.publish(&resource, &version) {
        Ok(()) => {}
        Err(StoreError::AlreadyExists(_)) => {
            // Treat as success — concurrent seed or duplicate
            return Ok(());
        }
        Err(e) => {
            // Compensating transaction: clean up the blob
            let _ = blob_store.delete(&blob_path);
            return Err(e);
        }
    }

    // 5. Logging
    let type_name = bundled.resource_type.store_dir_name().trim_end_matches('s');
    if is_reseed {
        eprintln!(
            "[relava-server] re-seeded default {type_name}: {}@{version_str} (previously removed)",
            bundled.name
        );
    } else {
        eprintln!(
            "[relava-server] seeded default {type_name}: {}@{version_str}",
            bundled.name
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::db::SqliteResourceStore;
    use crate::store::traits::ResourceStore;
    use std::path::PathBuf;

    /// Create a test store and blob store in a temp directory.
    fn test_env() -> (SqliteResourceStore, LocalBlobStore, PathBuf) {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("relava-seed-test-{}-{}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let db_path = root.join("relava.db");
        let store = SqliteResourceStore::open(&db_path).unwrap();
        let blob_store = LocalBlobStore::new(root.join("store"));
        (store, blob_store, root)
    }

    // -- Core seed tests --

    #[test]
    fn seed_publishes_to_empty_registry() {
        let (store, blob_store, _root) = test_env();
        seed(&store, &blob_store).unwrap();

        let resource = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        assert_eq!(resource.name, "relava");
        assert_eq!(resource.latest_version.as_deref(), Some("0.1.0"));
        assert!(resource.description.is_some());
    }

    #[test]
    fn seed_is_noop_when_same_version_exists() {
        let (store, blob_store, _root) = test_env();
        seed(&store, &blob_store).unwrap();

        // Second seed should be a no-op (no error, same version)
        seed(&store, &blob_store).unwrap();

        let resource = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        assert_eq!(resource.latest_version.as_deref(), Some("0.1.0"));

        // Should still have exactly one version
        let versions = store.list_versions(resource.id).unwrap();
        assert_eq!(versions.len(), 1);
    }

    #[test]
    fn seed_is_noop_when_registry_has_newer_version() {
        let (store, blob_store, _root) = test_env();

        // Manually publish a newer version
        let resource = models::Resource {
            id: 0,
            scope: None,
            name: "relava".to_string(),
            resource_type: "skill".to_string(),
            description: Some("newer".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let version = models::Version {
            id: 0,
            resource_id: 0,
            version: "99.0.0".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: None,
            published_by: None,
            published_at: None,
        };
        store.publish(&resource, &version).unwrap();

        // Seed should not downgrade
        seed(&store, &blob_store).unwrap();

        let existing = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        assert_eq!(existing.latest_version.as_deref(), Some("99.0.0"));
    }

    #[test]
    fn seed_updates_when_embedded_version_is_newer() {
        let (store, blob_store, _root) = test_env();

        // Publish an older version first
        let resource = models::Resource {
            id: 0,
            scope: None,
            name: "relava".to_string(),
            resource_type: "skill".to_string(),
            description: Some("old".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        };
        let version = models::Version {
            id: 0,
            resource_id: 0,
            version: "0.0.1".to_string(),
            store_path: None,
            checksum: None,
            manifest_json: None,
            published_by: None,
            published_at: None,
        };
        store.publish(&resource, &version).unwrap();

        // Seed should publish newer embedded version
        seed(&store, &blob_store).unwrap();

        let existing = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        assert_eq!(existing.latest_version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn seed_reseeds_after_resource_removed() {
        let (store, blob_store, _root) = test_env();

        // Seed, then delete the resource
        seed(&store, &blob_store).unwrap();
        store
            .delete_resource(None, "relava", ResourceType::Skill)
            .unwrap();

        // Verify it's gone
        assert!(matches!(
            store.get_resource(None, "relava", ResourceType::Skill),
            Err(StoreError::NotFound(_))
        ));

        // Re-seed should bring it back
        seed(&store, &blob_store).unwrap();

        let resource = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        assert_eq!(resource.name, "relava");
        assert_eq!(resource.latest_version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn seed_stores_blob_with_correct_path() {
        let (store, blob_store, _root) = test_env();
        seed(&store, &blob_store).unwrap();

        let resource = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        let versions = store.list_versions(resource.id).unwrap();
        assert_eq!(versions.len(), 1);

        let ver = &versions[0];
        assert_eq!(
            ver.store_path.as_deref(),
            Some("skills/relava/0.1.0")
        );
        assert!(ver.checksum.is_some());
        assert_eq!(ver.published_by.as_deref(), Some("relava-server"));

        // Verify blob exists and matches content
        let blob_data = blob_store.fetch("skills/relava/0.1.0/SKILL.md").unwrap();
        assert_eq!(blob_data, BUNDLED_RELAVA_SKILL.as_bytes());
    }

    #[test]
    fn seed_stores_manifest_json_with_checksums() {
        let (store, blob_store, _root) = test_env();
        seed(&store, &blob_store).unwrap();

        let resource = store
            .get_resource(None, "relava", ResourceType::Skill)
            .unwrap();
        let versions = store.list_versions(resource.id).unwrap();
        let ver = &versions[0];

        let manifest: serde_json::Value =
            serde_json::from_str(ver.manifest_json.as_ref().unwrap()).unwrap();
        let files = manifest["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "SKILL.md");
        assert_eq!(
            files[0]["sha256"].as_str().unwrap(),
            sha256_hex(BUNDLED_RELAVA_SKILL.as_bytes())
        );
    }

    // -- Frontmatter parsing tests --

    #[test]
    fn parse_frontmatter_extracts_version_and_description() {
        let content = "---\nname: test\nversion: 1.2.3\ndescription: a test\n---\n# Body";
        let (version, description) = parse_frontmatter(content).unwrap();
        assert_eq!(version, "1.2.3");
        assert_eq!(description, "a test");
    }

    #[test]
    fn parse_frontmatter_rejects_missing_delimiters() {
        assert!(parse_frontmatter("no frontmatter here").is_err());
    }

    #[test]
    fn parse_frontmatter_rejects_missing_version() {
        let content = "---\nname: test\ndescription: hi\n---\n";
        let err = parse_frontmatter(content).unwrap_err();
        assert!(err.contains("version"));
    }

    #[test]
    fn parse_frontmatter_rejects_missing_description() {
        let content = "---\nname: test\nversion: 1.0.0\n---\n";
        let err = parse_frontmatter(content).unwrap_err();
        assert!(err.contains("description"));
    }
}
