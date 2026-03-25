use rusqlite::{Connection, Row};
use std::path::Path;

use super::models::{Resource, Version};
use super::traits::{ResourceStore, StoreError};
use crate::validate::ResourceType;

/// Map a row from the standard 8-column resource SELECT into a `Resource`.
fn resource_from_row(row: &Row<'_>) -> rusqlite::Result<Resource> {
    Ok(Resource {
        id: row.get(0)?,
        scope: row.get(1)?,
        name: row.get(2)?,
        resource_type: row.get(3)?,
        description: row.get(4)?,
        latest_version: row.get(5)?,
        metadata_json: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// Map a row from the standard 8-column version SELECT into a `Version`.
fn version_from_row(row: &Row<'_>) -> rusqlite::Result<Version> {
    Ok(Version {
        id: row.get(0)?,
        resource_id: row.get(1)?,
        version: row.get(2)?,
        store_path: row.get(3)?,
        checksum: row.get(4)?,
        manifest_json: row.get(5)?,
        published_by: row.get(6)?,
        published_at: row.get(7)?,
    })
}

/// Current schema version. Increment when adding migrations.
const SCHEMA_VERSION: i64 = 1;

/// SQLite-backed implementation of `ResourceStore`.
pub struct SqliteResourceStore {
    conn: Connection,
}

impl SqliteResourceStore {
    /// Open (or create) a SQLite database at `path` and run migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)
            .map_err(|e| StoreError::Database(format!("failed to open database: {e}")))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory database (for testing).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| StoreError::Database(format!("failed to open in-memory database: {e}")))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Apply all pending migrations up to `SCHEMA_VERSION`.
    fn migrate(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
            .map_err(|e| StoreError::Database(format!("failed to create schema_version table: {e}")))?;

        let current: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .map_err(|e| StoreError::Database(format!("failed to read schema version: {e}")))?;

        if current < 1 {
            self.migrate_v1()?;
        }

        // Future migrations go here:
        // if current < 2 { self.migrate_v2()?; }

        if current < SCHEMA_VERSION {
            self.conn
                .execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )
                .map_err(|e| StoreError::Database(format!("failed to update schema version: {e}")))?;
        }

        Ok(())
    }

    fn migrate_v1(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS resources (
                    id             INTEGER PRIMARY KEY,
                    scope          TEXT,
                    name           TEXT NOT NULL,
                    type           TEXT NOT NULL,
                    description    TEXT,
                    latest_version TEXT,
                    metadata_json  TEXT,
                    updated_at     TIMESTAMP,
                    UNIQUE(scope, name, type)
                );

                CREATE TABLE IF NOT EXISTS versions (
                    id             INTEGER PRIMARY KEY,
                    resource_id    INTEGER REFERENCES resources(id),
                    version        TEXT NOT NULL,
                    store_path     TEXT,
                    checksum       TEXT,
                    manifest_json  TEXT,
                    published_by   TEXT,
                    published_at   TIMESTAMP,
                    UNIQUE(resource_id, version)
                );",
            )
            .map_err(|e| StoreError::Database(format!("migration v1 failed: {e}")))?;
        Ok(())
    }
}

impl ResourceStore for SqliteResourceStore {
    fn get_resource(
        &self,
        scope: Option<&str>,
        name: &str,
        resource_type: ResourceType,
    ) -> Result<Resource, StoreError> {
        let rt = resource_type.to_string();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                 FROM resources
                 WHERE (scope IS ?1) AND name = ?2 AND type = ?3",
            )
            .map_err(|e| StoreError::Database(e.to_string()))?;

        stmt.query_row((scope, name, &rt), resource_from_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => StoreError::NotFound(format!(
                    "{resource_type} '{name}' not found"
                )),
                _ => StoreError::Database(e.to_string()),
            })
    }

    fn list_versions(&self, resource_id: i64) -> Result<Vec<Version>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, resource_id, version, store_path, checksum, manifest_json, published_by, published_at
                 FROM versions
                 WHERE resource_id = ?1
                 ORDER BY published_at DESC",
            )
            .map_err(|e| StoreError::Database(e.to_string()))?;

        let rows = stmt
            .query_map([resource_id], version_from_row)
            .map_err(|e| StoreError::Database(e.to_string()))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Database(e.to_string()))
    }

    fn publish(&self, resource: &Resource, version: &Version) -> Result<(), StoreError> {
        // Check if the resource already exists.
        let existing_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM resources WHERE (scope IS ?1) AND name = ?2 AND type = ?3",
                (&resource.scope, &resource.name, &resource.resource_type),
                |row| row.get(0),
            )
            .ok();

        let resource_id = if let Some(id) = existing_id {
            // Update existing resource.
            self.conn
                .execute(
                    "UPDATE resources SET description = ?1, latest_version = ?2, metadata_json = ?3, updated_at = datetime('now') WHERE id = ?4",
                    (
                        &resource.description,
                        &version.version,
                        &resource.metadata_json,
                        id,
                    ),
                )
                .map_err(|e| StoreError::Database(e.to_string()))?;
            id
        } else {
            // Insert new resource.
            self.conn
                .execute(
                    "INSERT INTO resources (scope, name, type, description, latest_version, metadata_json, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
                    (
                        &resource.scope,
                        &resource.name,
                        &resource.resource_type,
                        &resource.description,
                        &version.version,
                        &resource.metadata_json,
                    ),
                )
                .map_err(|e| StoreError::Database(e.to_string()))?;
            self.conn.last_insert_rowid()
        };

        // Insert the version row — fail on duplicate (resource_id, version).
        self.conn
            .execute(
                "INSERT INTO versions (resource_id, version, store_path, checksum, manifest_json, published_by, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
                (
                    resource_id,
                    &version.version,
                    &version.store_path,
                    &version.checksum,
                    &version.manifest_json,
                    &version.published_by,
                ),
            )
            .map_err(|e| match e {
                // Fix #4: Match on ErrorCode instead of string-matching error messages.
                rusqlite::Error::SqliteFailure(err, _)
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    StoreError::AlreadyExists(format!(
                        "version {} already exists for {}",
                        version.version, resource.name
                    ))
                }
                _ => StoreError::Database(e.to_string()),
            })?;

        Ok(())
    }

    fn search(
        &self,
        query: &str,
        resource_type: Option<ResourceType>,
    ) -> Result<Vec<Resource>, StoreError> {
        let pattern = format!("%{query}%");

        // Fix #1: Both branches use fully parameterized queries — no string
        // interpolation of user-supplied values into SQL.
        if let Some(rt) = resource_type {
            let rt_str = rt.to_string();
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                     FROM resources
                     WHERE (name LIKE ?1 OR description LIKE ?1) AND type = ?2
                     ORDER BY name",
                )
                .map_err(|e| StoreError::Database(e.to_string()))?;

            let rows = stmt
                .query_map((&pattern, &rt_str), resource_from_row)
                .map_err(|e| StoreError::Database(e.to_string()))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| StoreError::Database(e.to_string()))
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                     FROM resources
                     WHERE (name LIKE ?1 OR description LIKE ?1)
                     ORDER BY name",
                )
                .map_err(|e| StoreError::Database(e.to_string()))?;

            let rows = stmt
                .query_map([&pattern], resource_from_row)
                .map_err(|e| StoreError::Database(e.to_string()))?;

            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| StoreError::Database(e.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> SqliteResourceStore {
        SqliteResourceStore::open_in_memory().unwrap()
    }

    fn sample_resource() -> Resource {
        Resource {
            id: 0,
            scope: None,
            name: "denden".to_string(),
            resource_type: "skill".to_string(),
            description: Some("Communication skill".to_string()),
            latest_version: None,
            metadata_json: None,
            updated_at: None,
        }
    }

    fn sample_version(ver: &str) -> Version {
        Version {
            id: 0,
            resource_id: 0,
            version: ver.to_string(),
            store_path: Some(format!("skills/denden/{ver}")),
            checksum: Some("abc123".to_string()),
            manifest_json: None,
            published_by: None,
            published_at: None,
        }
    }

    #[test]
    fn tables_exist_after_open() {
        let store = test_store();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('resources', 'versions')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn migration_is_idempotent() {
        let store = test_store();
        store.migrate().unwrap(); // second migration should not error
    }

    #[test]
    fn schema_version_is_tracked() {
        let store = test_store();
        let ver: i64 = store
            .conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
    }

    #[test]
    fn publish_and_get_resource() {
        let store = test_store();
        let resource = sample_resource();
        let version = sample_version("1.0.0");

        store.publish(&resource, &version).unwrap();

        let found = store.get_resource(None, "denden", ResourceType::Skill).unwrap();
        assert_eq!(found.name, "denden");
        assert_eq!(found.resource_type, "skill");
        assert_eq!(found.latest_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn publish_updates_latest_version() {
        let store = test_store();
        let resource = sample_resource();

        store.publish(&resource, &sample_version("1.0.0")).unwrap();
        store.publish(&resource, &sample_version("1.1.0")).unwrap();

        let found = store.get_resource(None, "denden", ResourceType::Skill).unwrap();
        assert_eq!(found.latest_version.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn list_versions_returns_all() {
        let store = test_store();
        let resource = sample_resource();

        store.publish(&resource, &sample_version("1.0.0")).unwrap();
        store.publish(&resource, &sample_version("1.1.0")).unwrap();

        let found = store.get_resource(None, "denden", ResourceType::Skill).unwrap();
        let versions = store.list_versions(found.id).unwrap();
        assert_eq!(versions.len(), 2);
    }

    #[test]
    fn get_resource_not_found() {
        let store = test_store();
        let result = store.get_resource(None, "nonexistent", ResourceType::Skill);
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn publish_duplicate_version_errors() {
        let store = test_store();
        let resource = sample_resource();
        let version = sample_version("1.0.0");

        store.publish(&resource, &version).unwrap();
        let result = store.publish(&resource, &version);
        assert!(matches!(result, Err(StoreError::AlreadyExists(_))));
    }

    #[test]
    fn search_by_name() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        let results = store.search("den", None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "denden");
    }

    #[test]
    fn search_by_type_filter() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        let results = store.search("den", Some(ResourceType::Agent)).unwrap();
        assert!(results.is_empty());

        let results = store.search("den", Some(ResourceType::Skill)).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_results() {
        let store = test_store();
        let results = store.search("nonexistent", None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn scoped_resources_are_separate() {
        let store = test_store();
        let global = sample_resource();
        let mut scoped = sample_resource();
        scoped.scope = Some("myteam".to_string());

        store.publish(&global, &sample_version("1.0.0")).unwrap();
        store.publish(&scoped, &sample_version("2.0.0")).unwrap();

        let g = store.get_resource(None, "denden", ResourceType::Skill).unwrap();
        assert_eq!(g.latest_version.as_deref(), Some("1.0.0"));

        let s = store
            .get_resource(Some("myteam"), "denden", ResourceType::Skill)
            .unwrap();
        assert_eq!(s.latest_version.as_deref(), Some("2.0.0"));
    }
}
