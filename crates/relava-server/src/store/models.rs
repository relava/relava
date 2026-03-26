/// A resource registered in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub id: i64,
    pub scope: Option<String>,
    pub name: String,
    pub resource_type: String,
    pub description: Option<String>,
    pub latest_version: Option<String>,
    pub metadata_json: Option<String>,
    pub updated_at: Option<String>,
}

/// A published version of a resource.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub id: i64,
    pub resource_id: i64,
    pub version: String,
    pub store_path: Option<String>,
    pub checksum: Option<String>,
    pub manifest_json: Option<String>,
    pub published_by: Option<String>,
    pub published_at: Option<String>,
}
