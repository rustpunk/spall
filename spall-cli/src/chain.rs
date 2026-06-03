use serde_json::Value;
use spall_core::extensions::CliExtensions;
use spall_core::ir::{ParameterLocation, ResolvedOperation, ResolvedParameter};

/// Parsed chain expression.
#[derive(Debug, Clone)]
pub struct ChainExpr {
    pub target_op_id: String,
    /// (param_id, jmespath_expression) pairs
    pub bindings: Vec<(String, String)>,
}

/// How a single target-operation parameter is emitted on the chained command
/// line: a path parameter becomes a positional value, every other location a
/// `--long` flag. The `order` preserves the operation's declaration order so
/// path positionals are emitted left-to-right as clap expects them.
struct TargetParam {
    /// Parameter name, matched against a binding key.
    name: String,
    location: ParameterLocation,
    /// The clap long flag (without value) for non-path params, mirroring
    /// `crate::command::build_op_cmd`. `None` for path params.
    flag: Option<String>,
    /// Declaration index in the target operation.
    order: usize,
}

/// Build the chainable parameter metadata for a target operation, deriving each
/// non-path flag exactly as `crate::command::build_op_cmd` does so the
/// recursive parse accepts the emitted tokens.
fn target_params(op: &ResolvedOperation) -> Vec<TargetParam> {
    op.parameters
        .iter()
        .enumerate()
        .map(|(order, p)| TargetParam {
            name: p.name.clone(),
            location: p.location,
            flag: flag_for(p),
            order,
        })
        .collect()
}

/// Derive the clap long flag for a non-path parameter, matching the builder in
/// `crate::command`. Path parameters are positional and return `None`.
fn flag_for(param: &ResolvedParameter) -> Option<String> {
    let ext = CliExtensions::from_parameter(param);
    let long_name = ext.cli_name.as_deref().unwrap_or(&param.name);
    match param.location {
        ParameterLocation::Path => None,
        ParameterLocation::Query => Some(format!("--{}", long_name)),
        ParameterLocation::Header => {
            let kebab = long_name.to_ascii_lowercase().replace('_', "-");
            Some(format!("--header-{}", kebab))
        }
        ParameterLocation::Cookie => {
            let kebab = long_name.to_ascii_lowercase().replace('_', "-");
            Some(format!("--cookie-{}", kebab))
        }
    }
}

impl ChainExpr {
    /// Parse a simple expression like `op2 --id $.id --name $.name`
    pub fn parse(expr: &str) -> Result<Self, crate::SpallCliError> {
        let mut parts = expr.split_whitespace();
        let target_op_id = parts
            .next()
            .ok_or_else(|| {
                crate::SpallCliError::Usage(
                    "chain expression requires target operation".to_string(),
                )
            })?
            .to_string();

        let mut bindings = Vec::new();
        while let Some(token) = parts.next() {
            if token.starts_with("--") {
                let param_id = token.trim_start_matches("--").to_string();
                let jmespath_expr = parts.next().ok_or_else(|| {
                    crate::SpallCliError::Usage(format!(
                        "chain param '{}' needs a jmespath expression",
                        param_id
                    ))
                })?;
                bindings.push((param_id, jmespath_expr.to_string()));
            }
        }

        Ok(ChainExpr {
            target_op_id,
            bindings,
        })
    }

    /// Evaluate JMESPath expressions against the response JSON and return
    /// resolved CLI args for the target operation.
    ///
    /// Each binding key names a parameter of `target_op`. A path parameter is
    /// emitted as a positional value (in the operation's declaration order, so
    /// multiple path params land in the right slots); every other location is
    /// emitted as its `--long` flag. An unknown key — one that matches no
    /// parameter of the target operation — is a usage error rather than a
    /// silently wrong flag.
    pub fn resolve(
        &self,
        response_json: &Value,
        target_op: &ResolvedOperation,
    ) -> Result<Vec<String>, crate::SpallCliError> {
        let params = target_params(target_op);

        // Resolved path positionals keyed by declaration order, and flag pairs
        // in binding order.
        let mut path_values: Vec<(usize, String)> = Vec::new();
        let mut flag_args: Vec<String> = Vec::new();

        for (key, expr_str) in &self.bindings {
            let param = params.iter().find(|p| &p.name == key).ok_or_else(|| {
                crate::SpallCliError::Usage(format!(
                    "chain target operation '{}' has no parameter '{}'",
                    self.target_op_id, key
                ))
            })?;

            let value = Self::eval(response_json, expr_str)?;

            match (&param.flag, param.location) {
                (None, ParameterLocation::Path) => {
                    path_values.push((param.order, value));
                }
                (Some(flag), _) => {
                    flag_args.push(flag.clone());
                    flag_args.push(value);
                }
                // A path param always has flag == None; any other location
                // always has Some(flag). This arm is unreachable but keeps the
                // match exhaustive without an unwrap.
                (None, _) => {
                    flag_args.push(format!("--{}", key));
                    flag_args.push(value);
                }
            }
        }

        // Positionals first (in operation declaration order), then flags.
        path_values.sort_by_key(|(order, _)| *order);

        let mut args = vec![self.target_op_id.clone()];
        args.extend(path_values.into_iter().map(|(_, v)| v));
        args.extend(flag_args);
        Ok(args)
    }

    /// Compile and search a single JMESPath expression, stringifying the result.
    fn eval(response_json: &Value, expr_str: &str) -> Result<String, crate::SpallCliError> {
        let expr = jmespath::compile(expr_str).map_err(|e| {
            crate::SpallCliError::Usage(format!(
                "chain jmespath compile error for '{}': {}",
                expr_str, e
            ))
        })?;
        let result = expr.search(response_json).map_err(|e| {
            crate::SpallCliError::Usage(format!(
                "chain jmespath search error for '{}': {}",
                expr_str, e
            ))
        })?;
        let s = match result.as_ref() {
            jmespath::Variable::String(s) => s.clone(),
            other => other.to_string(),
        };
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use spall_core::ir::{HttpMethod, ResolvedSchema};

    fn schema() -> ResolvedSchema {
        ResolvedSchema {
            type_name: None,
            format: None,
            description: None,
            default: None,
            enum_values: Vec::new(),
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
            pattern: None,
            min_length: None,
            max_length: None,
            minimum: None,
            maximum: None,
            multiple_of: None,
            exclusive_minimum: false,
            exclusive_maximum: false,
            min_items: None,
            max_items: None,
            unique_items: false,
            additional_properties: true,
            properties: Default::default(),
            items: None,
        }
    }

    fn param(name: &str, location: ParameterLocation) -> ResolvedParameter {
        ResolvedParameter {
            name: name.to_string(),
            location,
            required: true,
            deprecated: false,
            style: "simple".to_string(),
            explode: false,
            schema: schema(),
            description: None,
            extensions: Default::default(),
        }
    }

    fn op(op_id: &str, params: Vec<ResolvedParameter>) -> ResolvedOperation {
        ResolvedOperation {
            operation_id: op_id.to_string(),
            method: HttpMethod::Get,
            path_template: "/x".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: params,
            request_body: None,
            responses: Default::default(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: Default::default(),
            servers: Vec::new(),
        }
    }

    #[test]
    fn parse_simple() {
        let c = ChainExpr::parse("op2 --id $.id").unwrap();
        assert_eq!(c.target_op_id, "op2");
        assert_eq!(c.bindings, vec![("id".to_string(), "$.id".to_string())]);
    }

    #[test]
    fn parse_multiple_bindings() {
        let c = ChainExpr::parse("op3 --a $.a --b $.b").unwrap();
        assert_eq!(c.target_op_id, "op3");
        assert_eq!(
            c.bindings,
            vec![
                ("a".to_string(), "$.a".to_string()),
                ("b".to_string(), "$.b".to_string()),
            ]
        );
    }

    #[test]
    fn parse_missing_jmespath() {
        let err = ChainExpr::parse("op2 --id").unwrap_err();
        assert!(matches!(err, crate::SpallCliError::Usage(_)));
    }

    #[test]
    fn parse_empty() {
        let err = ChainExpr::parse("").unwrap_err();
        assert!(matches!(err, crate::SpallCliError::Usage(_)));
    }

    #[test]
    fn resolve_path_param_is_positional() {
        // The canonical chain case (#34): a captured id feeds a path parameter,
        // which must be emitted positionally — not as `--id`.
        let c = ChainExpr::parse("update-thing --id id").unwrap();
        let target = op("update-thing", vec![param("id", ParameterLocation::Path)]);
        let args = c.resolve(&json!({"id": "42"}), &target).unwrap();
        assert_eq!(args, vec!["update-thing", "42"]);
    }

    #[test]
    fn resolve_query_param_is_long_flag() {
        let c = ChainExpr::parse("op2 --q q").unwrap();
        let target = op("op2", vec![param("q", ParameterLocation::Query)]);
        let args = c.resolve(&json!({"q": "x"}), &target).unwrap();
        assert_eq!(args, vec!["op2", "--q", "x"]);
    }

    #[test]
    fn resolve_header_param_is_kebab_flag() {
        let c = ChainExpr::parse("op2 --X_Trace_Id tid").unwrap();
        let target = op("op2", vec![param("X_Trace_Id", ParameterLocation::Header)]);
        let args = c.resolve(&json!({"tid": "abc"}), &target).unwrap();
        assert_eq!(args, vec!["op2", "--header-x-trace-id", "abc"]);
    }

    #[test]
    fn resolve_path_then_query_ordering() {
        // Path positionals come first (declaration order), flags after.
        let c = ChainExpr::parse("op2 --limit limit --id id").unwrap();
        let target = op(
            "op2",
            vec![
                param("id", ParameterLocation::Path),
                param("limit", ParameterLocation::Query),
            ],
        );
        let args = c
            .resolve(&json!({"id": "7", "limit": "5"}), &target)
            .unwrap();
        assert_eq!(args, vec!["op2", "7", "--limit", "5"]);
    }

    #[test]
    fn resolve_dash_prefixed_value() {
        // A captured value beginning with '-' is passed through verbatim (#36);
        // clap accepts it because the op args set allow_hyphen_values(true).
        let c = ChainExpr::parse("op2 --offset offset").unwrap();
        let target = op("op2", vec![param("offset", ParameterLocation::Query)]);
        let args = c.resolve(&json!({"offset": -5}), &target).unwrap();
        assert_eq!(args, vec!["op2", "--offset", "-5"]);
    }

    #[test]
    fn resolve_number_to_string() {
        let c = ChainExpr::parse("op2 --n n").unwrap();
        let target = op("op2", vec![param("n", ParameterLocation::Query)]);
        let args = c.resolve(&json!({"n": 7}), &target).unwrap();
        assert_eq!(args, vec!["op2", "--n", "7"]);
    }

    #[test]
    fn resolve_nested_jmespath() {
        let c = ChainExpr::parse("op2 --name data.name").unwrap();
        let target = op("op2", vec![param("name", ParameterLocation::Query)]);
        let args = c.resolve(&json!({"data": {"name": "x"}}), &target).unwrap();
        assert_eq!(args, vec!["op2", "--name", "x"]);
    }

    #[test]
    fn resolve_unknown_key_errors() {
        // A binding key that matches no target parameter is a usage error, not
        // a silently wrong flag.
        let c = ChainExpr::parse("op2 --bogus id").unwrap();
        let target = op("op2", vec![param("id", ParameterLocation::Path)]);
        let err = c.resolve(&json!({"id": "1"}), &target).unwrap_err();
        assert!(matches!(err, crate::SpallCliError::Usage(_)));
    }

    #[test]
    fn resolve_invalid_expr() {
        let c = ChainExpr::parse("op2 --id bad[expr").unwrap();
        let target = op("op2", vec![param("id", ParameterLocation::Path)]);
        let err = c.resolve(&json!({}), &target).unwrap_err();
        assert!(matches!(err, crate::SpallCliError::Usage(_)));
    }
}
