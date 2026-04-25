use crate::value::SpallValue;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Lightweight spec index cached for fast degraded --help.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpecIndex {
    pub title: String,
    pub base_url: String,
    /// Spec version string (e.g. "1.0.0").
    pub version: String,
    pub operations: Vec<SpecIndexOp>,
    /// When this index was cached.
    pub cached_at: String,
}

/// Minimal operation metadata for the index.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SpecIndexOp {
    pub operation_id: String,
    pub method: HttpMethod,
    pub path_template: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub parameters: Vec<ParamIndex>,
    pub has_request_body: bool,
    pub request_body_required: bool,
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
    pub extensions: IndexMap<String, SpallValue>,
    pub servers: Vec<ResolvedServer>,
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
    pub example: Option<SpallValue>,
    pub examples: IndexMap<String, SpallValue>,
}

/// Resolved header.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedHeader {
    pub description: Option<String>,
    pub required: bool,
    pub deprecated: bool,
    pub schema: ResolvedSchema,
}

/// A resolved server entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedServer {
    pub url: String,
    pub description: Option<String>,
}

/// Lightweight parameter index for degraded --help.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ParamIndex {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
}

/// Simplified schema representation for IR.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolvedSchema {
    pub type_name: Option<String>,
    pub format: Option<String>,
    pub description: Option<String>,
    pub default: Option<SpallValue>,
    pub enum_values: Vec<SpallValue>,
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

/// Resolved servers for an operation (operation > path > spec > default).
impl ResolvedSpec {
    pub fn to_index(&self) -> SpecIndex {
        SpecIndex {
            title: self.title.clone(),
            base_url: self.base_url.clone(),
            version: self.version.clone(),
            operations: self.operations.iter().map(|op| op.to_index_op()).collect(),
            cached_at: {
                let d = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                format!("{}", d.as_secs())
            },
        }
    }
}

impl ResolvedOperation {
    pub fn to_index_op(&self) -> SpecIndexOp {
        SpecIndexOp {
            operation_id: self.operation_id.clone(),
            method: self.method,
            path_template: self.path_template.clone(),
            summary: self.summary.clone(),
            tags: self.tags.clone(),
            deprecated: self.deprecated,
            parameters: self.parameters.iter().map(|p| ParamIndex {
                name: p.name.clone(),
                location: p.location,
                required: p.required,
            }).collect(),
            has_request_body: self.request_body.is_some(),
            request_body_required: self.request_body.as_ref().map(|b| b.required).unwrap_or(false),
        }
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
    pub servers: Vec<ResolvedServer>,
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
