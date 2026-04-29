use thiserror::Error;

/// Errors originating in spall-core (spec loading, resolution, IR).
#[derive(Error, Debug)]
pub enum SpallCoreError {
    #[error("spec parse failed: {message} (url: {url})")]
    SpecParse { message: String, url: String },

    #[error("unresolved $ref: {path}")]
    UnresolvedRef { path: String, context: String },

    #[error("cycle detected in $ref at depth {depth}")]
    RefCycle { path: String, depth: usize },

    #[error("invalid spec source: {0}")]
    InvalidSource(String),

    #[error("network error fetching spec: {0}")]
    Network(String),

    #[error("external file $ref not supported: {path}")]
    ExternalRefNotSupported { path: String },

    #[error("IR cache error: {0}")]
    Cache(String),

    #[error("IO error: {0}")]
    Io(String),
}
