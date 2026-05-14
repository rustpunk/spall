//! `ResolvedSchema` → JSON Schema Draft 2020-12 conversion for MCP
//! `inputSchema` payloads.
//!
//! The IR's `ResolvedSchema` is a near-strict subset of JSON Schema after
//! the resolver collapses `$ref`s, `oneOf`/`anyOf`/`allOf` branches, and
//! flags cycles via `is_recursive`. This module's job is mechanical
//! translation; no validation, no defaults synthesis.

use serde_json::{json, Map, Value};
use spall_core::ir::{ParameterLocation, ResolvedOperation, ResolvedSchema};

/// Build the top-level `inputSchema` object for an MCP tool that wraps
/// `op`. Properties: one entry per parameter (path/query/header/cookie),
/// plus a `body` property when the operation has a request body.
#[must_use = "the schema is the tool's contract; dropping it loses required-ness info"]
pub fn operation_input_schema(op: &ResolvedOperation) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();

    for param in &op.parameters {
        let mut prop = resolved_schema_to_json(&param.schema);
        annotate_location(&mut prop, param.location, param.description.as_deref());
        properties.insert(param.name.clone(), prop);
        if param.required {
            required.push(Value::String(param.name.clone()));
        }
    }

    if let Some(rb) = &op.request_body {
        if let Some(body_schema) = pick_body_schema(rb) {
            let mut prop = resolved_schema_to_json(body_schema);
            if let Value::Object(map) = &mut prop {
                let desc = rb
                    .description
                    .clone()
                    .unwrap_or_else(|| "Request body".to_string());
                map.entry("description")
                    .or_insert_with(|| Value::String(desc));
            }
            properties.insert("body".to_string(), prop);
            if rb.required {
                required.push(Value::String("body".to_string()));
            }
        }
    }

    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema.insert("properties".to_string(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".to_string(), Value::Array(required));
    }
    schema.insert("additionalProperties".to_string(), Value::Bool(false));
    Value::Object(schema)
}

/// Convert a single `ResolvedSchema` node into JSON Schema. Recurses into
/// `properties` and `items`. Cyclic refs (`is_recursive: true`) collapse
/// to an empty object schema — the spec MAY reject `$ref` cycles at
/// validation time, but a permissive `{}` keeps the MCP client happy and
/// matches what the resolver already decided.
#[must_use]
pub fn resolved_schema_to_json(s: &ResolvedSchema) -> Value {
    if s.is_recursive {
        return json!({ "description": "cyclic schema omitted" });
    }

    let mut out = Map::new();

    // type + nullable. OAS 3.0 `nullable: true` becomes the union
    // `["t", "null"]` in JSON Schema; missing type with nullable becomes
    // bare `"null"`.
    match (&s.type_name, s.nullable) {
        (Some(t), false) => {
            out.insert("type".to_string(), Value::String(t.clone()));
        }
        (Some(t), true) => {
            out.insert(
                "type".to_string(),
                Value::Array(vec![
                    Value::String(t.clone()),
                    Value::String("null".to_string()),
                ]),
            );
        }
        (None, true) => {
            out.insert("type".to_string(), Value::String("null".to_string()));
        }
        (None, false) => {}
    }

    if let Some(fmt) = &s.format {
        out.insert("format".to_string(), Value::String(fmt.clone()));
    }
    if let Some(desc) = &s.description {
        out.insert("description".to_string(), Value::String(desc.clone()));
    }
    if let Some(default) = &s.default {
        out.insert("default".to_string(), Value::from(default));
    }
    if !s.enum_values.is_empty() {
        out.insert(
            "enum".to_string(),
            Value::Array(s.enum_values.iter().map(Value::from).collect()),
        );
    }

    if let Some(pat) = &s.pattern {
        out.insert("pattern".to_string(), Value::String(pat.clone()));
    }
    if let Some(min) = s.min_length {
        out.insert("minLength".to_string(), json!(min));
    }
    if let Some(max) = s.max_length {
        out.insert("maxLength".to_string(), json!(max));
    }
    if let Some(min) = s.minimum {
        if s.exclusive_minimum {
            out.insert("exclusiveMinimum".to_string(), float_value(min));
        } else {
            out.insert("minimum".to_string(), float_value(min));
        }
    }
    if let Some(max) = s.maximum {
        if s.exclusive_maximum {
            out.insert("exclusiveMaximum".to_string(), float_value(max));
        } else {
            out.insert("maximum".to_string(), float_value(max));
        }
    }
    if let Some(mo) = s.multiple_of {
        out.insert("multipleOf".to_string(), float_value(mo));
    }
    if let Some(min) = s.min_items {
        out.insert("minItems".to_string(), json!(min));
    }
    if let Some(max) = s.max_items {
        out.insert("maxItems".to_string(), json!(max));
    }
    if s.unique_items {
        out.insert("uniqueItems".to_string(), Value::Bool(true));
    }

    // Only emit `additionalProperties: false` explicitly. The IR's
    // default is `true`, which is also JSON Schema's default — leave it
    // implicit to keep the payload compact.
    if !s.additional_properties {
        out.insert("additionalProperties".to_string(), Value::Bool(false));
    }

    if !s.properties.is_empty() {
        let mut props = Map::with_capacity(s.properties.len());
        for (k, v) in &s.properties {
            props.insert(k.clone(), resolved_schema_to_json(v));
        }
        out.insert("properties".to_string(), Value::Object(props));
    }

    if let Some(items) = &s.items {
        out.insert("items".to_string(), resolved_schema_to_json(items));
    }

    Value::Object(out)
}

fn pick_body_schema(rb: &spall_core::ir::ResolvedRequestBody) -> Option<&ResolvedSchema> {
    if let Some(media) = rb.content.get("application/json") {
        if let Some(schema) = media.schema.as_ref() {
            return Some(schema);
        }
    }
    rb.content
        .values()
        .find_map(|media| media.schema.as_ref())
}

fn annotate_location(prop: &mut Value, location: ParameterLocation, param_desc: Option<&str>) {
    if let Value::Object(map) = prop {
        let suffix = format!("(in: {})", location.as_str());
        let desc = match (map.get("description"), param_desc) {
            (Some(Value::String(existing)), _) => format!("{} {}", existing, suffix),
            (_, Some(d)) => format!("{} {}", d, suffix),
            _ => suffix,
        };
        map.insert("description".to_string(), Value::String(desc));
    }
}

fn float_value(f: f64) -> Value {
    // Integer-valued floats render as JSON integers for cleaner schemas
    // (`"minimum": 0` vs `"minimum": 0.0`). Falls back to f64 otherwise.
    if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        Value::from(f as i64)
    } else {
        Value::from(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use spall_core::ir::{
        HttpMethod, ParameterLocation, ResolvedOperation, ResolvedParameter, ResolvedRequestBody,
        ResolvedSchema,
    };
    use spall_core::value::SpallValue;

    fn empty_schema() -> ResolvedSchema {
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
            properties: IndexMap::new(),
            items: None,
        }
    }

    #[test]
    fn scalar_string() {
        let mut s = empty_schema();
        s.type_name = Some("string".to_string());
        s.format = Some("uuid".to_string());
        s.min_length = Some(36);
        s.max_length = Some(36);
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["type"], "string");
        assert_eq!(v["format"], "uuid");
        assert_eq!(v["minLength"], 36);
        assert_eq!(v["maxLength"], 36);
    }

    #[test]
    fn integer_with_minimum_maximum() {
        let mut s = empty_schema();
        s.type_name = Some("integer".to_string());
        s.minimum = Some(1.0);
        s.maximum = Some(100.0);
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["type"], "integer");
        assert_eq!(v["minimum"], 1);
        assert_eq!(v["maximum"], 100);
    }

    #[test]
    fn exclusive_minimum_maps_to_jsonschema_value() {
        let mut s = empty_schema();
        s.type_name = Some("number".to_string());
        s.minimum = Some(0.0);
        s.exclusive_minimum = true;
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["exclusiveMinimum"], 0);
        assert!(v.get("minimum").is_none());
    }

    #[test]
    fn nullable_string_emits_type_union() {
        let mut s = empty_schema();
        s.type_name = Some("string".to_string());
        s.nullable = true;
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["type"], json!(["string", "null"]));
    }

    #[test]
    fn nullable_without_type_is_pure_null() {
        let mut s = empty_schema();
        s.nullable = true;
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["type"], "null");
    }

    #[test]
    fn array_recurses_into_items() {
        let mut item = empty_schema();
        item.type_name = Some("integer".to_string());
        let mut arr = empty_schema();
        arr.type_name = Some("array".to_string());
        arr.items = Some(Box::new(item));
        arr.min_items = Some(1);
        arr.unique_items = true;
        let v = resolved_schema_to_json(&arr);
        assert_eq!(v["type"], "array");
        assert_eq!(v["items"]["type"], "integer");
        assert_eq!(v["minItems"], 1);
        assert_eq!(v["uniqueItems"], true);
    }

    #[test]
    fn enum_preserved() {
        let mut s = empty_schema();
        s.type_name = Some("string".to_string());
        s.enum_values = vec![
            SpallValue::Str("red".to_string()),
            SpallValue::Str("green".to_string()),
            SpallValue::Str("blue".to_string()),
        ];
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["enum"], json!(["red", "green", "blue"]));
    }

    #[test]
    fn nested_object_with_properties() {
        let mut inner = empty_schema();
        inner.type_name = Some("string".to_string());
        let mut outer = empty_schema();
        outer.type_name = Some("object".to_string());
        outer.properties.insert("name".to_string(), inner);
        outer.additional_properties = false;
        let v = resolved_schema_to_json(&outer);
        assert_eq!(v["type"], "object");
        assert_eq!(v["properties"]["name"]["type"], "string");
        assert_eq!(v["additionalProperties"], false);
    }

    #[test]
    fn additional_properties_true_is_implicit() {
        let mut s = empty_schema();
        s.type_name = Some("object".to_string());
        // additional_properties defaults to true in empty_schema(); ensure
        // no key is emitted.
        let v = resolved_schema_to_json(&s);
        assert!(v.get("additionalProperties").is_none());
    }

    #[test]
    fn recursive_collapses_to_marker() {
        let mut s = empty_schema();
        s.is_recursive = true;
        s.type_name = Some("object".to_string());
        // Even with a type, recursion wins.
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["description"], "cyclic schema omitted");
        assert!(v.get("type").is_none());
    }

    #[test]
    fn default_value_preserved() {
        let mut s = empty_schema();
        s.type_name = Some("integer".to_string());
        s.default = Some(SpallValue::I64(42));
        let v = resolved_schema_to_json(&s);
        assert_eq!(v["default"], 42);
    }

    #[test]
    fn operation_input_schema_includes_params_and_body() {
        let mut name_schema = empty_schema();
        name_schema.type_name = Some("string".to_string());
        let mut id_schema = empty_schema();
        id_schema.type_name = Some("integer".to_string());

        let mut body_schema = empty_schema();
        body_schema.type_name = Some("object".to_string());
        body_schema.properties.insert("note".to_string(), {
            let mut s = empty_schema();
            s.type_name = Some("string".to_string());
            s
        });

        let op = ResolvedOperation {
            operation_id: "createPet".to_string(),
            method: HttpMethod::Post,
            path_template: "/pets/{petId}".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![
                ResolvedParameter {
                    name: "petId".to_string(),
                    location: ParameterLocation::Path,
                    required: true,
                    deprecated: false,
                    style: "simple".to_string(),
                    explode: false,
                    schema: id_schema,
                    description: Some("Pet ID".to_string()),
                    extensions: IndexMap::new(),
                },
                ResolvedParameter {
                    name: "name".to_string(),
                    location: ParameterLocation::Query,
                    required: false,
                    deprecated: false,
                    style: "form".to_string(),
                    explode: true,
                    schema: name_schema,
                    description: None,
                    extensions: IndexMap::new(),
                },
            ],
            request_body: Some(ResolvedRequestBody {
                description: Some("Pet note".to_string()),
                required: true,
                content: {
                    let mut m = IndexMap::new();
                    m.insert(
                        "application/json".to_string(),
                        spall_core::ir::ResolvedMediaType {
                            schema: Some(body_schema),
                            example: None,
                            examples: IndexMap::new(),
                        },
                    );
                    m
                },
            }),
            responses: IndexMap::new(),
            security: Vec::new(),
            tags: Vec::new(),
            extensions: IndexMap::new(),
            servers: Vec::new(),
        };

        let v = operation_input_schema(&op);
        assert_eq!(v["type"], "object");
        assert_eq!(v["properties"]["petId"]["type"], "integer");
        assert_eq!(v["properties"]["name"]["type"], "string");
        assert_eq!(v["properties"]["body"]["type"], "object");
        assert_eq!(v["properties"]["body"]["properties"]["note"]["type"], "string");
        let required = v["required"].as_array().unwrap();
        assert!(required.iter().any(|x| x == "petId"));
        assert!(required.iter().any(|x| x == "body"));
        assert!(!required.iter().any(|x| x == "name"));
        assert_eq!(v["additionalProperties"], false);

        // Location annotations on description.
        let pet_id_desc = v["properties"]["petId"]["description"].as_str().unwrap();
        assert!(pet_id_desc.contains("(in: path)"));
        let name_desc = v["properties"]["name"]["description"].as_str().unwrap();
        assert!(name_desc.contains("(in: query)"));
    }
}
