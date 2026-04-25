use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Lightweight spec index cached for fast degraded --help.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpecIndex {
    pub title: String,
    pub base_url: String,
    /// IR format version for automatic cache invalidation on upgrades.
    pub version: u32,
    pub operations: Vec<OperationMeta>,
    /// When this index was cached.
    pub cached_at: String,
}

/// Minimal operation metadata for the index.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OperationMeta {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub deprecated: bool,
}

/// Full resolved operation.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedOperation {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub parameters: Vec<ResolvedParameter>,
    pub request_body: Option<ResolvedRequestBody>,
    pub responses: IndexMap<String, ResolvedResponse>,
    pub security: Vec<SecurityRequirement>,
    pub tags: Vec<String>,
    pub extensions: IndexMap<String, serde_json::Value>,
}

/// A resolved parameter with all $refs flattened.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedParameter {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub deprecated: bool,
    pub style: String,
    pub explode: bool,
    pub schema: ResolvedSchema,
    pub description: Option<String>,
}

/// Request body after resolution.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedRequestBody {
    pub description: Option<String>,
    pub required: bool,
    pub content: IndexMap<String, ResolvedMediaType>,
}

/// A response after resolution.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedResponse {
    pub description: Option<String>,
    pub content: IndexMap<String, ResolvedMediaType>,
    pub headers: IndexMap<String, ResolvedHeader>,
}

/// Media type with resolved schema.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedMediaType {
    pub schema: Option<ResolvedSchema>,
    pub example: Option<serde_json::Value>,
    pub examples: IndexMap<String, serde_json::Value>,
}

/// Resolved header.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedHeader {
    pub description: Option<String>,
    pub required: bool,
    pub deprecated: bool,
    pub schema: ResolvedSchema,
}

/// Simplified schema representation for IR.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedSchema {
    pub type_name: Option<String>,
    pub format: Option<String>,
    pub description: Option<String>,
    pub default: Option<serde_json::Value>,
    pub enum_values: Vec<serde_json::Value>,
    pub nullable: bool,
    pub read_only: bool,
    pub write_only: bool,
    /// Marker for schemas that exceeded $ref depth / cycle limits.
    pub is_recursive: bool,
}

/// HTTP method enum.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    Trace,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Trace => "TRACE",
        };
        write!(f, "{}", s)
    }
}

/// Security requirement copied from OpenAPI.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SecurityRequirement {
    pub name: String,
    pub scopes: Vec<String>,
}

/// Full resolved spec containing all operations.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedSpec {
    pub title: String,
    pub version: String,
    pub base_url: String,
    pub operations: Vec<ResolvedOperation>,
}

/// Parameter location.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParameterLocation {
    Query,
    Header,
    Path,
    Cookie,
}

impl ParameterLocation {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParameterLocation::Query => "query",
            ParameterLocation::Header => "header",
            ParameterLocation::Path => "path",
            ParameterLocation::Cookie => "cookie",
        }
    }
}
