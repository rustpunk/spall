//! Arazzo 1.0.1 workflow document support.
//!
//! Three submodules:
//!
//! - [`model`] — `serde`-derived structs for the Arazzo doc shape.
//! - [`expressions`] — parser + evaluator for the Arazzo expression dialect
//!   and the `successCriteria` runtime-condition mini-language.
//! - [`sources`] — model for `sourceDescriptions[]` plus a pure helper that
//!   turns raw bytes into a `ResolvedSpec` via the existing IR cache.

pub mod expressions;
pub mod model;
pub mod sources;

pub use expressions::{
    eval, eval_condition, parse_condition, parse_expression, CompareOp, Condition, Context,
    ExprError, Operand, ResponseSnapshot, StepResult,
};
pub use model::{
    ArazzoDocument, Info, Parameter, RequestBody, SourceDescription, Step, SuccessCriterion,
    Workflow,
};
pub use sources::resolve_source_from_bytes;
