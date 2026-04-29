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
            .ok_or_else(|| crate::SpallCliError::Usage("chain expression requires target operation".to_string()))?
            .to_string();

        let mut bindings = Vec::new();
        while let Some(token) = parts.next() {
            if token.starts_with("--") {
                let param_id = token.trim_start_matches("--").to_string();
                let jmespath_expr = parts
                    .next()
                    .ok_or_else(|| crate::SpallCliError::Usage(format!("chain param '{}' needs a jmespath expression", param_id)))?;
                bindings.push((param_id, jmespath_expr.to_string()));
            }
        }

        Ok(ChainExpr { target_op_id, bindings })
    }

    /// Evaluate JMESPath expressions against the response JSON and return resolved CLI args.
    pub fn resolve(
        &self,
        response_json: &Value,
    ) -> Result<Vec<String>, crate::SpallCliError> {
        let mut args = vec![self.target_op_id.clone()];
        for (param, expr_str) in &self.bindings {
            let expr = jmespath::compile(expr_str)
                .map_err(|e| crate::SpallCliError::Usage(format!("chain jmespath compile error for '{}': {}", expr_str, e)))?;
            let result = expr.search(response_json)
                .map_err(|e| crate::SpallCliError::Usage(format!("chain jmespath search error for '{}': {}", expr_str, e)))?;
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
