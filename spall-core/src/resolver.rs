use crate::error::SpallCoreError;
use crate::ir::{
    HttpMethod, ParameterLocation, ResolvedMediaType, ResolvedOperation, ResolvedParameter,
    ResolvedRequestBody, ResolvedResponse, ResolvedSchema, ResolvedServer, ResolvedSpec,
    SecurityRequirement,
};
use crate::value::SpallValue;
use indexmap::IndexMap;
use openapiv3::{
    Components, Header, OpenAPI, Parameter, ReferenceOr, RequestBody, Response, Schema,
    SecurityRequirement as OpenApiSecurityRequirement,
};

const MAX_REF_DEPTH: usize = 10;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn resolve_spec(raw: &OpenAPI, _source: &str) -> Result<ResolvedSpec, SpallCoreError> {
    let title = raw.info.title.clone();
    let version = raw.info.version.clone();
    let base_url = raw
        .servers
        .first()
        .map(|s| s.url.clone())
        .unwrap_or_else(|| "/".to_string());

    let spec_servers: Vec<ResolvedServer> = raw
        .servers
        .iter()
        .map(|s| ResolvedServer {
            url: s.url.clone(),
            description: s.description.clone(),
        })
        .collect();

    let mut operations: Vec<ResolvedOperation> = Vec::new();
    let mut seen_ids: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for (path_template, path_item_ref) in &raw.paths.paths {
        let path_item = match path_item_ref {
            ReferenceOr::Reference { reference } => {
                return Err(SpallCoreError::UnresolvedRef {
                    path: reference.clone(),
                    context: format!("path {}", path_template),
                })
            }
            ReferenceOr::Item(item) => item,
        };

        let path_servers: Vec<ResolvedServer> = if path_item.servers.is_empty() {
            spec_servers.clone()
        } else {
            path_item
                .servers
                .iter()
                .map(|s| ResolvedServer {
                    url: s.url.clone(),
                    description: s.description.clone(),
                })
                .collect()
        };

        let path_params: Vec<ReferenceOr<Parameter>> = path_item.parameters.clone();

        let ops = [
            ("get", path_item.get.as_ref(), HttpMethod::Get),
            ("post", path_item.post.as_ref(), HttpMethod::Post),
            ("put", path_item.put.as_ref(), HttpMethod::Put),
            ("delete", path_item.delete.as_ref(), HttpMethod::Delete),
            ("patch", path_item.patch.as_ref(), HttpMethod::Patch),
            ("head", path_item.head.as_ref(), HttpMethod::Head),
            ("options", path_item.options.as_ref(), HttpMethod::Options),
            ("trace", path_item.trace.as_ref(), HttpMethod::Trace),
        ];

        for (method_str, op_opt, method) in ops {
            let Some(op) = op_opt else { continue };

            let operation_id = synthesize_operation_id(
                method_str,
                path_template,
                op.operation_id.as_deref(),
                &mut seen_ids,
            );

            let operation_servers = if op.servers.is_empty() {
                if path_servers.is_empty() {
                    vec![ResolvedServer {
                        url: "/".into(),
                        description: None,
                    }]
                } else {
                    path_servers.clone()
                }
            } else {
                op.servers
                    .iter()
                    .map(|s| ResolvedServer {
                        url: s.url.clone(),
                        description: s.description.clone(),
                    })
                    .collect()
            };

            let security = resolve_security(
                raw.security.as_deref(),
                op.security.as_deref(),
            );

            let parameters = merge_parameters(
                &path_params,
                &op.parameters,
                raw,
            )?;

            let request_body = op
                .request_body
                .as_ref()
                .map(|rb| resolve_request_body_ref(rb, raw))
                .transpose()?;

            let mut responses = IndexMap::new();
            for (code, resp_ref) in &op.responses.responses {
                let resp = resolve_response_ref(resp_ref, raw)?;
                responses.insert(code.to_string(), resolve_response(resp, raw)?);
            }

            operations.push(ResolvedOperation {
                operation_id: operation_id.clone(),
                method,
                path_template: path_template.clone(),
                summary: op.summary.clone(),
                description: op.description.clone(),
                deprecated: op.deprecated,
                parameters,
                request_body,
                responses,
                security,
                tags: op.tags.clone(),
                extensions: op.extensions
                    .iter()
                    .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                    .collect(),
                servers: operation_servers,
            });
        }
    }

    Ok(ResolvedSpec {
        title,
        version,
        base_url,
        operations,
        servers: spec_servers,
    })
}

// ---------------------------------------------------------------------------
// Parameter resolution
// ---------------------------------------------------------------------------

pub fn merge_parameters(
    path_params: &[ReferenceOr<Parameter>],
    op_params: &[ReferenceOr<Parameter>],
    spec: &OpenAPI,
) -> Result<Vec<ResolvedParameter>, SpallCoreError> {
    let mut map: IndexMap<(String, ParameterLocation), ResolvedParameter> = IndexMap::new();

    for p_ref in path_params {
        let p = resolve_one_parameter(p_ref, spec)?;
        let key = (p.name.clone(), p.location);
        map.insert(key, p);
    }

    for p_ref in op_params {
        let p = resolve_one_parameter(p_ref, spec)?;
        let key = (p.name.clone(), p.location);
        map.insert(key, p); // operation overrides path
    }

    Ok(map.into_values().collect())
}

fn resolve_one_parameter(
    p_ref: &ReferenceOr<Parameter>,
    spec: &OpenAPI,
) -> Result<ResolvedParameter, SpallCoreError> {
    let p = match p_ref {
        ReferenceOr::Reference { reference } => {
            resolve_parameter_ref(reference, spec)?.ok_or_else(|| SpallCoreError::UnresolvedRef {
                path: reference.clone(),
                context: "parameter".to_string(),
            })?
        }
        ReferenceOr::Item(item) => item.clone(),
    };

    let (location, name, required, deprecated, description, style_str, explode, schema_ref, extensions) = match &p {
        Parameter::Query { parameter_data, style, .. } => (
            ParameterLocation::Query,
            &parameter_data.name,
            parameter_data.required,
            parameter_data.deprecated.unwrap_or(false),
            parameter_data.description.clone(),
            serde_json::to_string(style).unwrap_or_else(|_| "\"form\"".to_string()),
            parameter_data.explode.unwrap_or(true),
            extract_schema_ref(&parameter_data.format),
            parameter_data.extensions
                .iter()
                .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                .collect(),
        ),
        Parameter::Header { parameter_data, style, .. } => (
            ParameterLocation::Header,
            &parameter_data.name,
            parameter_data.required,
            parameter_data.deprecated.unwrap_or(false),
            parameter_data.description.clone(),
            serde_json::to_string(style).unwrap_or_else(|_| "\"simple\"".to_string()),
            parameter_data.explode.unwrap_or(false),
            extract_schema_ref(&parameter_data.format),
            parameter_data.extensions
                .iter()
                .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                .collect(),
        ),
        Parameter::Path { parameter_data, style, .. } => (
            ParameterLocation::Path,
            &parameter_data.name,
            true, // path params are always required
            parameter_data.deprecated.unwrap_or(false),
            parameter_data.description.clone(),
            serde_json::to_string(style).unwrap_or_else(|_| "\"simple\"".to_string()),
            parameter_data.explode.unwrap_or(false),
            extract_schema_ref(&parameter_data.format),
            parameter_data.extensions
                .iter()
                .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                .collect(),
        ),
        Parameter::Cookie { parameter_data, style, .. } => (
            ParameterLocation::Cookie,
            &parameter_data.name,
            parameter_data.required,
            parameter_data.deprecated.unwrap_or(false),
            parameter_data.description.clone(),
            serde_json::to_string(style).unwrap_or_else(|_| "\"form\"".to_string()),
            parameter_data.explode.unwrap_or(false),
            extract_schema_ref(&parameter_data.format),
            parameter_data.extensions
                .iter()
                .map(|(k, v)| (k.clone(), SpallValue::from(v)))
                .collect(),
        ),
    };

    let style = style_str.trim_matches('"').to_ascii_lowercase();

    let schema = match schema_ref {
        Some(s_ref) => {
            let mut visited = std::collections::HashSet::new();
            resolve_schema(s_ref, spec, &mut visited, 0)?
        }
        None => ResolvedSchema {
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
        },
    };

    Ok(ResolvedParameter {
        name: name.clone(),
        location,
        required,
        deprecated,
        style,
        explode,
        schema,
        description,
        extensions,
    })
}

fn extract_schema_ref(
    fmt: &openapiv3::ParameterSchemaOrContent,
) -> Option<&ReferenceOr<Schema>> {
    match fmt {
        openapiv3::ParameterSchemaOrContent::Schema(s_ref) => Some(s_ref),
        openapiv3::ParameterSchemaOrContent::Content(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Security inheritance
// ---------------------------------------------------------------------------

pub fn resolve_security(
    root: Option<&[OpenApiSecurityRequirement]>,
    operation: Option<&[OpenApiSecurityRequirement]>,
) -> Vec<SecurityRequirement> {
    let src: &[OpenApiSecurityRequirement] = match operation {
        Some(op) => op,
        None => {
            return root
                .map(|r| {
                    r.iter()
                        .flat_map(|req| {
                            req.iter().map(|(name, scopes)| SecurityRequirement {
                                name: name.clone(),
                                scopes: scopes.clone(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
    };

    if src.is_empty() {
        return Vec::new();
    }

    src.iter()
        .flat_map(|req| {
            req.iter().map(|(name, scopes)| SecurityRequirement {
                name: name.clone(),
                scopes: scopes.clone(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Schema resolution with cycle detection
// ---------------------------------------------------------------------------

pub fn resolve_schema(
    raw: &ReferenceOr<Schema>,
    spec: &OpenAPI,
    visited: &mut std::collections::HashSet<String>,
    depth: usize,
) -> Result<ResolvedSchema, SpallCoreError> {
    if depth > MAX_REF_DEPTH {
        return Ok(ResolvedSchema {
            type_name: None,
            format: None,
            description: Some("Schema omitted: exceeded maximum $ref depth".to_string()),
            default: None,
            enum_values: Vec::new(),
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: true,
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
        });
    }

    let schema = match raw {
        ReferenceOr::Reference { reference } => {
            if !reference.starts_with("#/") {
                return Err(SpallCoreError::ExternalRefNotSupported {
                    path: reference.clone(),
                });
            }
            if !visited.insert(reference.clone()) {
                return Ok(ResolvedSchema {
                    type_name: None,
                    format: None,
                    description: Some("Schema omitted: cyclic $ref".to_string()),
                    default: None,
                    enum_values: Vec::new(),
                    nullable: false,
                    read_only: false,
                    write_only: false,
                    is_recursive: true,
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
                });
            }
            resolve_schema_ref(reference, spec)?.ok_or_else(|| SpallCoreError::UnresolvedRef {
                path: reference.clone(),
                context: "schema".to_string(),
            })?
        }
        ReferenceOr::Item(item) => item.clone(),
    };

    // Extract type name from SchemaKind
    let type_name = match &schema.schema_kind {
        openapiv3::SchemaKind::Type(t) => {
            let name = match t {
                openapiv3::Type::String(_) => "string",
                openapiv3::Type::Number(_) => "number",
                openapiv3::Type::Integer(_) => "integer",
                openapiv3::Type::Object(_) => "object",
                openapiv3::Type::Array(_) => "array",
                openapiv3::Type::Boolean(_) => "boolean",
            };
            Some(name.to_string())
        }
        _ => None,
    };

    let format = match &schema.schema_kind {
        openapiv3::SchemaKind::Type(openapiv3::Type::String(s)) => {
            variant_or_empty_to_string(&s.format)
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Number(n)) => {
            variant_or_empty_to_string(&n.format)
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Integer(i)) => {
            variant_or_empty_to_string(&i.format)
        }
        _ => None,
    };

    let enum_values = match &schema.schema_kind {
        openapiv3::SchemaKind::Type(openapiv3::Type::String(s)) => s
            .enumeration
            .iter()
            .filter_map(|v| v.as_ref().map(|s| SpallValue::Str(s.clone())))
            .collect(),
        openapiv3::SchemaKind::Type(openapiv3::Type::Number(n)) => n
            .enumeration
            .iter()
            .filter_map(|v| v.map(SpallValue::F64))
            .collect(),
        openapiv3::SchemaKind::Type(openapiv3::Type::Integer(i)) => i
            .enumeration
            .iter()
            .filter_map(|v| v.map(SpallValue::I64))
            .collect(),
        _ => Vec::new(),
    };

    // Extract validation fields
    let mut pattern = None;
    let mut min_length = None;
    let mut max_length = None;
    let mut minimum = None;
    let mut maximum = None;
    let mut multiple_of = None;
    let mut exclusive_minimum = false;
    let mut exclusive_maximum = false;
    let mut min_items = None;
    let mut max_items = None;
    let mut unique_items = false;
    let mut additional_properties = true;
    let mut properties: IndexMap<String, ResolvedSchema> = IndexMap::new();
    let mut items: Option<Box<ResolvedSchema>> = None;

    match &schema.schema_kind {
        openapiv3::SchemaKind::Type(openapiv3::Type::String(s)) => {
            pattern = s.pattern.clone();
            min_length = s.min_length;
            max_length = s.max_length;
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Number(n)) => {
            minimum = n.minimum;
            maximum = n.maximum;
            multiple_of = n.multiple_of;
            exclusive_minimum = n.exclusive_minimum;
            exclusive_maximum = n.exclusive_maximum;
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Integer(i)) => {
            minimum = i.minimum.map(|v| v as f64);
            maximum = i.maximum.map(|v| v as f64);
            multiple_of = i.multiple_of.map(|v| v as f64);
            exclusive_minimum = i.exclusive_minimum;
            exclusive_maximum = i.exclusive_maximum;
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Array(a)) => {
            min_items = a.min_items;
            max_items = a.max_items;
            unique_items = a.unique_items;
            if let Some(ref item_ref) = a.items {
                items = Some(Box::new(resolve_schema(
                    &deref_boxed_ref(item_ref),
                    spec,
                    visited,
                    depth + 1,
                )?));
            }
        }
        openapiv3::SchemaKind::Type(openapiv3::Type::Object(o)) => {
            additional_properties = match &o.additional_properties {
                Some(openapiv3::AdditionalProperties::Any(b)) => *b,
                Some(openapiv3::AdditionalProperties::Schema(_)) => true,
                None => true,
            };
            for (prop_name, prop_ref) in &o.properties {
                let resolved = resolve_schema(&deref_boxed_ref(prop_ref),
                    spec,
                    visited,
                    depth + 1,
                )?;
                properties.insert(prop_name.clone(), resolved);
            }
        }
        openapiv3::SchemaKind::Any(any) => {
            pattern = any.pattern.clone();
            min_length = any.min_length;
            max_length = any.max_length;
            minimum = any.minimum;
            maximum = any.maximum;
            multiple_of = any.multiple_of;
            exclusive_minimum = any.exclusive_minimum.unwrap_or(false);
            exclusive_maximum = any.exclusive_maximum.unwrap_or(false);
            min_items = any.min_items;
            max_items = any.max_items;
            unique_items = any.unique_items.unwrap_or(false);
            additional_properties = match &any.additional_properties {
                Some(openapiv3::AdditionalProperties::Any(b)) => *b,
                Some(openapiv3::AdditionalProperties::Schema(_)) => true,
                None => true,
            };
            for (prop_name, prop_ref) in &any.properties {
                let resolved = resolve_schema(
                    &deref_boxed_ref(prop_ref),
                    spec,
                    visited,
                    depth + 1,
                )?;
                properties.insert(prop_name.clone(), resolved);
            }
            if let Some(ref item_ref) = &any.items {
                items = Some(Box::new(resolve_schema(
                    &deref_boxed_ref(item_ref),
                    spec,
                    visited,
                    depth + 1,
                )?));
            }
        }
        _ => {}
    }

    Ok(ResolvedSchema {
        type_name,
        format,
        description: schema.schema_data.description.clone(),
        default: schema.schema_data.default.as_ref().map(SpallValue::from),
        enum_values,
        nullable: schema.schema_data.nullable,
        read_only: schema.schema_data.read_only,
        write_only: schema.schema_data.write_only,
        is_recursive: false,
        pattern,
        min_length,
        max_length,
        minimum,
        maximum,
        multiple_of,
        exclusive_minimum,
        exclusive_maximum,
        min_items,
        max_items,
        unique_items,
        additional_properties,
        properties,
        items,
    })
}

/// Dereference a `Box<ReferenceOr<Schema>>` to a `ReferenceOr<Schema>`.
fn deref_boxed_ref(r: &ReferenceOr<Box<Schema>>) -> ReferenceOr<Schema> {
    match r {
        ReferenceOr::Reference { reference } => ReferenceOr::Reference {
            reference: reference.clone(),
        },
        ReferenceOr::Item(item) => ReferenceOr::Item(*item.clone()),
    }
}

// ---------------------------------------------------------------------------
// Request body / response resolution
// ---------------------------------------------------------------------------

fn resolve_request_body(
    rb: RequestBody,
    spec: &OpenAPI,
) -> Result<ResolvedRequestBody, SpallCoreError> {
    let mut content = IndexMap::new();
    for (ct, mt) in rb.content {
        content.insert(ct, resolve_media_type(&mt, spec)?);
    }

    Ok(ResolvedRequestBody {
        description: rb.description,
        required: rb.required,
        content,
    })
}

fn resolve_media_type(
    mt: &openapiv3::MediaType,
    spec: &OpenAPI,
) -> Result<ResolvedMediaType, SpallCoreError> {
    let schema = mt
        .schema
        .as_ref()
        .map(|s| {
            let mut visited = std::collections::HashSet::new();
            resolve_schema(s, spec, &mut visited, 0)
        })
        .transpose()?;

    Ok(ResolvedMediaType {
        schema,
        example: mt.example.as_ref().map(SpallValue::from),
        examples: mt
            .examples
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        ReferenceOr::Reference { reference } => {
                            SpallValue::Str(reference.clone())
                        }
                        ReferenceOr::Item(ex) => {
                            ex.value
                                .as_ref()
                                .map(SpallValue::from)
                                .unwrap_or(SpallValue::Null)
                        }
                    },
                )
            })
            .collect(),
    })
}

fn resolve_response(
    resp: Response,
    spec: &OpenAPI,
) -> Result<ResolvedResponse, SpallCoreError> {
    let mut content = IndexMap::new();
    for (ct, mt) in resp.content {
        content.insert(ct, resolve_media_type(&mt, spec)?);
    }

    let mut headers = IndexMap::new();
    for (name, h_ref) in resp.headers {
        let h = resolve_header_ref(&h_ref, spec)?.ok_or_else(|| SpallCoreError::UnresolvedRef {
            path: match h_ref {
                ReferenceOr::Reference { reference } => reference.clone(),
                ReferenceOr::Item(_) => "header".to_string(),
            },
            context: "response header".to_string(),
        })?;

        let schema = match &h.format {
            openapiv3::ParameterSchemaOrContent::Schema(s_ref) => {
                let mut visited = std::collections::HashSet::new();
                resolve_schema(s_ref, spec, &mut visited, 0)?
            }
            openapiv3::ParameterSchemaOrContent::Content(_) => ResolvedSchema {
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
            },
        };
        headers.insert(
            name,
            crate::ir::ResolvedHeader {
                description: h.description,
                required: h.required,
                deprecated: h.deprecated.unwrap_or(false),
                schema,
            },
        );
    }

    Ok(ResolvedResponse {
        description: Some(resp.description),
        content,
        headers,
    })
}

// ---------------------------------------------------------------------------
// Concrete $ref resolution helpers
// ---------------------------------------------------------------------------

fn resolve_schema_ref(
    path: &str,
    spec: &OpenAPI,
) -> Result<Option<Schema>, SpallCoreError> {
    let components = match spec.components.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };
    let name = ref_name(path, "schemas")?;
    if let Some(r) = components.schemas.get(name) {
        Ok(match r {
            ReferenceOr::Reference { reference } => resolve_schema_ref(reference, spec)?,
            ReferenceOr::Item(item) => Some(item.clone()),
        })
    } else {
        Ok(None)
    }
}

fn resolve_parameter_ref(
    path: &str,
    spec: &OpenAPI,
) -> Result<Option<Parameter>, SpallCoreError> {
    let components = match spec.components.as_ref() {
        Some(c) => c,
        None => return Ok(None),
    };
    let name = ref_name(path, "parameters")?;
    if let Some(r) = components.parameters.get(name) {
        Ok(match r {
            ReferenceOr::Reference { reference } => {
                return Err(SpallCoreError::UnresolvedRef {
                    path: reference.clone(),
                    context: "nested parameter ref".to_string(),
                })
            }
            ReferenceOr::Item(item) => Some(item.clone()),
        })
    } else {
        Ok(None)
    }
}

fn resolve_request_body_ref(
    rb: &ReferenceOr<RequestBody>,
    spec: &OpenAPI,
) -> Result<ResolvedRequestBody, SpallCoreError> {
    let rb = match rb {
        ReferenceOr::Reference { reference } => {
            let components = match spec.components.as_ref() {
                Some(c) => c,
                None => {
                    return Err(SpallCoreError::UnresolvedRef {
                        path: reference.clone(),
                        context: "request body".to_string(),
                    })
                }
            };
            let name = ref_name(reference, "requestBodies")?;
            match components.request_bodies.get(name) {
                Some(ReferenceOr::Item(item)) => item.clone(),
                _ => {
                    return Err(SpallCoreError::UnresolvedRef {
                        path: reference.clone(),
                        context: "request body".to_string(),
                    })
                }
            }
        }
        ReferenceOr::Item(item) => item.clone(),
    };
    resolve_request_body(rb, spec)
}

fn resolve_response_ref(
    r: &ReferenceOr<Response>,
    spec: &OpenAPI,
) -> Result<Response, SpallCoreError> {
    match r {
        ReferenceOr::Reference { reference } => {
            let components = match spec.components.as_ref() {
                Some(c) => c,
                None => {
                    return Err(SpallCoreError::UnresolvedRef {
                        path: reference.clone(),
                        context: "response".to_string(),
                    })
                }
            };
            let name = ref_name(reference, "responses")?;
            match components.responses.get(name) {
                Some(ReferenceOr::Item(item)) => Ok(item.clone()),
                _ => Err(SpallCoreError::UnresolvedRef {
                    path: reference.clone(),
                    context: "response".to_string(),
                }),
            }
        }
        ReferenceOr::Item(item) => Ok(item.clone()),
    }
}

fn resolve_header_ref(
    r: &ReferenceOr<Header>,
    spec: &OpenAPI,
) -> Result<Option<Header>, SpallCoreError> {
    match r {
        ReferenceOr::Reference { reference } => {
            let components = match spec.components.as_ref() {
                Some(c) => c,
                None => return Ok(None),
            };
            let name = ref_name(reference, "headers")?;
            if let Some(h) = components.headers.get(name) {
                Ok(match h {
                    ReferenceOr::Item(item) => Some(item.clone()),
                    ReferenceOr::Reference { reference } => {
                        return Err(SpallCoreError::UnresolvedRef {
                            path: reference.clone(),
                            context: "nested header ref".to_string(),
                        })
                    }
                })
            } else {
                Ok(None)
            }
        }
        ReferenceOr::Item(item) => Ok(Some(item.clone())),
    }
}

/// Extract the name from a `$ref` path fragment like `#/components/schemas/Foo`.
fn ref_name<'a>(path: &'a str, expected_category: &str) -> Result<&'a str, SpallCoreError> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 4
        && parts[0] == "#"
        && parts[1] == "components"
        && parts[2] == expected_category
    {
        Ok(parts[3])
    } else {
        Err(SpallCoreError::UnresolvedRef {
            path: path.to_string(),
            context: format!("expected #/components/{}/name", expected_category),
        })
    }
}

fn variant_or_empty_to_string<T: std::fmt::Debug>(
    v: &openapiv3::VariantOrUnknownOrEmpty<T>,
) -> Option<String> {
    match v {
        openapiv3::VariantOrUnknownOrEmpty::Item(t) => {
            Some(format!("{:?}", t).to_ascii_lowercase())
        }
        openapiv3::VariantOrUnknownOrEmpty::Unknown(s) => Some(s.clone()),
        openapiv3::VariantOrUnknownOrEmpty::Empty => None,
    }
}

// ---------------------------------------------------------------------------
// Operation ID synthesis
// ---------------------------------------------------------------------------

fn synthesize_operation_id(
    method: &str,
    path: &str,
    existing: Option<&str>,
    seen: &mut std::collections::HashMap<String, u32>,
) -> String {
    let raw = if let Some(id) = existing {
        id.to_string()
    } else {
        let mut parts: Vec<String> = vec![method.to_ascii_lowercase()];
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            let cleaned = segment.trim_start_matches('{').trim_end_matches('}');
            parts.push(cleaned.to_string());
        }
        parts.join("-")
    };

    let kebab = raw
        .replace('_', "-")
        .replace(" ", "-")
        .replace('.', "-")
        .to_lowercase();

    let count = seen.entry(kebab.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        kebab
    } else {
        format!("{}-{}", kebab, count)
    }
}
