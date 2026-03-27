use rusqlite::types::ToSql;
use rusqlite::{Connection, Row};
use std::path::Path;

use super::models::{Resource, Version};
use super::traits::{ResourceStore, StoreError};
use relava_types::validate::ResourceType;

/// Shorthand: convert a `rusqlite::Error` into `StoreError::Database`.
fn db_err(e: rusqlite::Error) -> StoreError {
    StoreError::Database(e.to_string())
}

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
const SCHEMA_VERSION: i64 = 2;

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
            .map_err(|e| {
                StoreError::Database(format!("failed to create schema_version table: {e}"))
            })?;

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

        if current < 2 {
            self.migrate_v2()?;
        }

        if current < SCHEMA_VERSION {
            self.conn
                .execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    [SCHEMA_VERSION],
                )
                .map_err(|e| {
                    StoreError::Database(format!("failed to update schema version: {e}"))
                })?;
        }

        Ok(())
    }

    /// Prepare, execute, and collect a resource query into a `Vec<Resource>`.
    fn query_resources(
        &self,
        sql: &str,
        params: &[&dyn ToSql],
    ) -> Result<Vec<Resource>, StoreError> {
        let mut stmt = self.conn.prepare(sql).map_err(db_err)?;
        let rows = stmt.query_map(params, resource_from_row).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
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

    fn migrate_v2(&self) -> Result<(), StoreError> {
        self.conn
            .execute_batch(
                "CREATE VIRTUAL TABLE IF NOT EXISTS resources_fts USING fts5(
                    name,
                    description,
                    keywords,
                    resource_type
                );",
            )
            .map_err(|e| StoreError::Database(format!("migration v2 (FTS5) failed: {e}")))?;

        // Clear any partial FTS data (idempotent re-run safety), then back-fill.
        self.conn
            .execute_batch("DELETE FROM resources_fts;")
            .map_err(|e| StoreError::Database(format!("FTS5 cleanup failed: {e}")))?;

        self.conn
            .execute_batch(
                "INSERT INTO resources_fts(rowid, name, description, keywords, resource_type)
                 SELECT id, name, COALESCE(description, ''), '', type
                 FROM resources;",
            )
            .map_err(|e| StoreError::Database(format!("FTS5 back-fill failed: {e}")))?;

        Ok(())
    }

    /// Check database connectivity by running a lightweight query.
    pub fn is_healthy(&self) -> bool {
        self.conn
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .is_ok()
    }

    /// Return resource counts grouped by type, e.g. `[("skill", 5), ("agent", 2)]`.
    pub fn resource_counts_by_type(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT type, COUNT(*) FROM resources GROUP BY type ORDER BY type")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Return the total number of versions across all resources.
    pub fn total_version_count(&self) -> Result<i64, StoreError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM versions", [], |row| row.get(0))
            .map_err(db_err)
    }

    /// Return the database page count × page size in bytes (0 for in-memory).
    pub fn database_size_bytes(&self) -> Result<i64, StoreError> {
        let page_count: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(db_err)?;
        let page_size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(db_err)?;
        Ok(page_count * page_size)
    }

    /// Update the FTS index for a resource after publish or update.
    fn update_fts_index(&self, resource_id: i64, resource: &Resource) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM resources_fts WHERE rowid = ?1", [resource_id])
            .map_err(db_err)?;

        let keywords = extract_keywords(resource.metadata_json.as_deref());

        self.conn
            .execute(
                "INSERT INTO resources_fts(rowid, name, description, keywords, resource_type) VALUES(?1, ?2, ?3, ?4, ?5)",
                (
                    resource_id,
                    &resource.name,
                    resource.description.as_deref().unwrap_or(""),
                    &keywords,
                    &resource.resource_type,
                ),
            )
            .map_err(db_err)?;

        Ok(())
    }
}

/// Extract keywords from a JSON metadata string.
///
/// Expects `{"keywords": ["foo", "bar"]}` and returns `"foo bar"`.
fn extract_keywords(metadata_json: Option<&str>) -> String {
    let Some(json) = metadata_json else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return String::new();
    };
    value
        .get("keywords")
        .and_then(|k| k.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
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
            .map_err(db_err)?;

        stmt.query_row((scope, name, &rt), resource_from_row)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    StoreError::NotFound(format!("{resource_type} '{name}' not found"))
                }
                other => db_err(other),
            })
    }

    fn list_resources(
        &self,
        resource_type: Option<ResourceType>,
    ) -> Result<Vec<Resource>, StoreError> {
        if let Some(rt) = resource_type {
            let rt_str = rt.to_string();
            self.query_resources(
                "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                 FROM resources
                 WHERE type = ?1
                 ORDER BY name",
                &[&rt_str as &dyn ToSql],
            )
        } else {
            self.query_resources(
                "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                 FROM resources
                 ORDER BY name",
                &[],
            )
        }
    }

    fn create_resource(&self, resource: &Resource) -> Result<Resource, StoreError> {
        // Explicit duplicate check: SQLite UNIQUE treats NULL != NULL, so the
        // constraint alone won't catch duplicates when scope is NULL.
        let exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM resources WHERE (scope IS ?1) AND name = ?2 AND type = ?3",
                (&resource.scope, &resource.name, &resource.resource_type),
                |row| row.get(0),
            )
            .map_err(db_err)?;

        if exists {
            return Err(StoreError::AlreadyExists(format!(
                "{} '{}' already exists",
                resource.resource_type, resource.name
            )));
        }

        self.conn
            .execute(
                "INSERT INTO resources (scope, name, type, description, latest_version, metadata_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))",
                (
                    &resource.scope,
                    &resource.name,
                    &resource.resource_type,
                    &resource.description,
                    &resource.latest_version,
                    &resource.metadata_json,
                ),
            )
            .map_err(db_err)?;

        let id = self.conn.last_insert_rowid();
        let created = self.conn
            .query_row(
                "SELECT id, scope, name, type, description, latest_version, metadata_json, updated_at
                 FROM resources WHERE id = ?1",
                [id],
                resource_from_row,
            )
            .map_err(db_err)?;

        // Add to FTS index.
        self.update_fts_index(id, &created)?;

        Ok(created)
    }

    fn delete_resource(
        &self,
        scope: Option<&str>,
        name: &str,
        resource_type: ResourceType,
    ) -> Result<(), StoreError> {
        let rt = resource_type.to_string();

        // Use a transaction so versions and resource are deleted atomically.
        self.conn.execute("BEGIN IMMEDIATE", []).map_err(db_err)?;

        let result = (|| {
            // Find the resource id (also validates existence).
            let resource_id: i64 = self
                .conn
                .query_row(
                    "SELECT id FROM resources WHERE (scope IS ?1) AND name = ?2 AND type = ?3",
                    (scope, name, &rt),
                    |row| row.get(0),
                )
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => {
                        StoreError::NotFound(format!("{resource_type} '{name}' not found"))
                    }
                    other => db_err(other),
                })?;

            // Remove from FTS index before deleting the resource.
            self.conn
                .execute("DELETE FROM resources_fts WHERE rowid = ?1", [resource_id])
                .map_err(db_err)?;

            // Delete all versions first, then the resource.
            self.conn
                .execute("DELETE FROM versions WHERE resource_id = ?1", [resource_id])
                .map_err(db_err)?;
            self.conn
                .execute("DELETE FROM resources WHERE id = ?1", [resource_id])
                .map_err(db_err)?;

            Ok(())
        })();

        if result.is_ok() {
            self.conn.execute("COMMIT", []).map_err(db_err)?;
        } else {
            let _ = self.conn.execute("ROLLBACK", []);
        }

        result
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
            .map_err(db_err)?;

        let rows = stmt
            .query_map([resource_id], version_from_row)
            .map_err(db_err)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    fn get_version(&self, resource_id: i64, version: &str) -> Result<Version, StoreError> {
        self.conn
            .query_row(
                "SELECT id, resource_id, version, store_path, checksum, manifest_json, published_by, published_at
                 FROM versions
                 WHERE resource_id = ?1 AND version = ?2",
                (resource_id, version),
                version_from_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    StoreError::NotFound(format!("version '{version}' not found"))
                }
                other => db_err(other),
            })
    }

    fn publish(&self, resource: &Resource, version: &Version) -> Result<(), StoreError> {
        // Check if the resource already exists. Propagate real DB errors
        // (locked, corrupt, etc.) — only treat "no rows" as absence.
        let existing_id: Option<i64> = match self.conn.query_row(
            "SELECT id FROM resources WHERE (scope IS ?1) AND name = ?2 AND type = ?3",
            (&resource.scope, &resource.name, &resource.resource_type),
            |row| row.get(0),
        ) {
            Ok(id) => Some(id),
            Err(rusqlite::Error::QueryReturnedNoRows) => None,
            Err(e) => return Err(db_err(e)),
        };

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
                .map_err(db_err)?;
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
                .map_err(db_err)?;
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
                rusqlite::Error::SqliteFailure(err, _)
                    if err.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    StoreError::AlreadyExists(format!(
                        "version {} already exists for {}",
                        version.version, resource.name
                    ))
                }
                other => db_err(other),
            })?;

        self.update_fts_index(resource_id, resource)?;

        Ok(())
    }

    fn search(
        &self,
        query: &str,
        resource_type: Option<ResourceType>,
    ) -> Result<Vec<Resource>, StoreError> {
        // Sanitize the query for FTS5: wrap each token in double quotes to
        // prevent FTS syntax injection, append * for prefix matching, and
        // combine with implicit AND.
        let fts_query: String = query
            .split_whitespace()
            .map(|token| {
                let escaped = token.replace('"', "\"\"");
                format!("\"{escaped}\"*")
            })
            .collect::<Vec<_>>()
            .join(" ");

        if fts_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "SELECT r.id, r.scope, r.name, r.type, r.description, r.latest_version, r.metadata_json, r.updated_at
             FROM resources_fts f
             JOIN resources r ON r.id = f.rowid
             WHERE resources_fts MATCH ?1",
        );
        let mut params: Vec<&dyn ToSql> = vec![&fts_query];

        let rt_str = resource_type.map(|rt| rt.to_string());
        if let Some(ref rt) = rt_str {
            sql.push_str(" AND r.type = ?2");
            params.push(rt);
        }

        sql.push_str(" ORDER BY rank");
        self.query_resources(&sql, &params)
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

        let found = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
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

        let found = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
        assert_eq!(found.latest_version.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn list_versions_returns_all() {
        let store = test_store();
        let resource = sample_resource();

        store.publish(&resource, &sample_version("1.0.0")).unwrap();
        store.publish(&resource, &sample_version("1.1.0")).unwrap();

        let found = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
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
    fn search_handles_fts_syntax_in_query() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        // FTS5 operators should not cause parse errors.
        let _ = store.search("OR AND NEAR", None).unwrap();
        let _ = store.search("\"quoted\"", None).unwrap();
        let _ = store.search("prefix*", None).unwrap();
        let _ = store.search("den OR something", None).unwrap();
    }

    #[test]
    fn search_by_keywords() {
        let store = test_store();
        let mut resource = sample_resource();
        resource.metadata_json = Some(r#"{"keywords":["monitoring","alerting"]}"#.to_string());
        store.publish(&resource, &sample_version("1.0.0")).unwrap();

        let results = store.search("monitoring", None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "denden");

        let results = store.search("alerting", None).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn scoped_resources_are_separate() {
        let store = test_store();
        let global = sample_resource();
        let mut scoped = sample_resource();
        scoped.scope = Some("myteam".to_string());

        store.publish(&global, &sample_version("1.0.0")).unwrap();
        store.publish(&scoped, &sample_version("2.0.0")).unwrap();

        let g = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
        assert_eq!(g.latest_version.as_deref(), Some("1.0.0"));

        let s = store
            .get_resource(Some("myteam"), "denden", ResourceType::Skill)
            .unwrap();
        assert_eq!(s.latest_version.as_deref(), Some("2.0.0"));
    }

    // -- create_resource tests --

    #[test]
    fn create_resource_returns_created() {
        let store = test_store();
        let resource = sample_resource();
        let created = store.create_resource(&resource).unwrap();
        assert_eq!(created.name, "denden");
        assert_eq!(created.resource_type, "skill");
        assert!(created.id > 0);
        assert!(created.updated_at.is_some());
    }

    #[test]
    fn create_resource_duplicate_errors() {
        let store = test_store();
        let resource = sample_resource();
        store.create_resource(&resource).unwrap();
        let result = store.create_resource(&resource);
        assert!(matches!(result, Err(StoreError::AlreadyExists(_))));
    }

    // -- delete_resource tests --

    #[test]
    fn delete_resource_removes_resource() {
        let store = test_store();
        store.create_resource(&sample_resource()).unwrap();

        store
            .delete_resource(None, "denden", ResourceType::Skill)
            .unwrap();

        let result = store.get_resource(None, "denden", ResourceType::Skill);
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    #[test]
    fn delete_resource_cascades_versions() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();
        store
            .publish(&sample_resource(), &sample_version("2.0.0"))
            .unwrap();

        let resource = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
        assert_eq!(store.list_versions(resource.id).unwrap().len(), 2);

        store
            .delete_resource(None, "denden", ResourceType::Skill)
            .unwrap();

        // Verify versions are also gone (use raw query since resource is deleted).
        let count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM versions WHERE resource_id = ?1",
                [resource.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_resource_not_found_errors() {
        let store = test_store();
        let result = store.delete_resource(None, "nonexistent", ResourceType::Skill);
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    // -- list_resources tests --

    #[test]
    fn list_resources_all() {
        let store = test_store();
        store.create_resource(&sample_resource()).unwrap();

        let mut agent = sample_resource();
        agent.name = "debugger".to_string();
        agent.resource_type = "agent".to_string();
        store.create_resource(&agent).unwrap();

        let all = store.list_resources(None).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn list_resources_with_type_filter() {
        let store = test_store();
        store.create_resource(&sample_resource()).unwrap();

        let mut agent = sample_resource();
        agent.name = "debugger".to_string();
        agent.resource_type = "agent".to_string();
        store.create_resource(&agent).unwrap();

        let skills = store.list_resources(Some(ResourceType::Skill)).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "denden");

        let agents = store.list_resources(Some(ResourceType::Agent)).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "debugger");
    }

    // -- get_version tests --

    #[test]
    fn get_version_found() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        let resource = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
        let version = store.get_version(resource.id, "1.0.0").unwrap();
        assert_eq!(version.version, "1.0.0");
    }

    #[test]
    fn get_version_not_found_errors() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        let resource = store
            .get_resource(None, "denden", ResourceType::Skill)
            .unwrap();
        let result = store.get_version(resource.id, "9.9.9");
        assert!(matches!(result, Err(StoreError::NotFound(_))));
    }

    // -- Health / stats helpers tests --

    #[test]
    fn is_healthy_returns_true() {
        let store = test_store();
        assert!(store.is_healthy());
    }

    #[test]
    fn resource_counts_by_type_empty() {
        let store = test_store();
        let counts = store.resource_counts_by_type().unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn resource_counts_by_type_groups_correctly() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();

        let mut agent = sample_resource();
        agent.name = "debugger".to_string();
        agent.resource_type = "agent".to_string();
        store.publish(&agent, &sample_version("1.0.0")).unwrap();

        let counts = store.resource_counts_by_type().unwrap();
        assert_eq!(counts.len(), 2);
        assert!(counts.contains(&("agent".to_string(), 1)));
        assert!(counts.contains(&("skill".to_string(), 1)));
    }

    #[test]
    fn total_version_count_empty() {
        let store = test_store();
        assert_eq!(store.total_version_count().unwrap(), 0);
    }

    #[test]
    fn total_version_count_with_data() {
        let store = test_store();
        store
            .publish(&sample_resource(), &sample_version("1.0.0"))
            .unwrap();
        store
            .publish(&sample_resource(), &sample_version("2.0.0"))
            .unwrap();
        assert_eq!(store.total_version_count().unwrap(), 2);
    }

    #[test]
    fn database_size_bytes_is_nonnegative() {
        let store = test_store();
        assert!(store.database_size_bytes().unwrap() >= 0);
    }
}
