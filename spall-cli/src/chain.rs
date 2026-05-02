use serde_json::Value;

/// Parsed chain expression.
#[derive(Debug, Clone)]
pub struct ChainExpr {
    pub target_op_id: String,
    /// (param_id, jmespath_expression) pairs
    pub bindings: Vec<(String, String)>,
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

    /// Evaluate JMESPath expressions against the response JSON and return resolved CLI args.
    pub fn resolve(&self, response_json: &Value) -> Result<Vec<String>, crate::SpallCliError> {
        let mut args = vec![self.target_op_id.clone()];
        for (param, expr_str) in &self.bindings {
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
            args.push(format!("--{}", param));
            args.push(s);
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn resolve_single_string() {
        let c = ChainExpr::parse("op2 --id id").unwrap();
        let args = c.resolve(&json!({"id": "42"})).unwrap();
        assert_eq!(args, vec!["op2", "--id", "42"]);
    }

    #[test]
    fn resolve_number_to_string() {
        let c = ChainExpr::parse("op2 --n n").unwrap();
        let args = c.resolve(&json!({"n": 7})).unwrap();
        assert_eq!(args, vec!["op2", "--n", "7"]);
    }

    #[test]
    fn resolve_array_to_string() {
        let c = ChainExpr::parse("op2 --arr arr").unwrap();
        let args = c.resolve(&json!({"arr": [1, 2]})).unwrap();
        assert_eq!(args, vec!["op2", "--arr", "[1,2]"]);
    }

    #[test]
    fn resolve_nested_jmespath() {
        let c = ChainExpr::parse("op2 --name data.name").unwrap();
        let args = c.resolve(&json!({"data": {"name": "x"}})).unwrap();
        assert_eq!(args, vec!["op2", "--name", "x"]);
    }

    #[test]
    fn resolve_invalid_expr() {
        let c = ChainExpr::parse("op2 --id bad[expr").unwrap();
        let err = c.resolve(&json!({})).unwrap_err();
        assert!(matches!(err, crate::SpallCliError::Usage(_)));
    }
}
