use indexmap::IndexMap;
use openapiv3::{
    OpenAPI, Operation, PathItem, QueryStyle, ReferenceOr, Schema, SchemaData, SchemaKind, Server,
    StringType, Type,
};
use spall_core::resolver::{merge_parameters, resolve_security, resolve_spec};

fn minimal_spec(title: &str) -> OpenAPI {
    OpenAPI {
        openapi: "3.0.0".to_string(),
        info: openapiv3::Info {
            title: title.to_string(),
            description: None,
            terms_of_service: None,
            contact: None,
            license: None,
            version: "1.0.0".to_string(),
            extensions: IndexMap::new(),
        },
        servers: vec![Server {
            url: "https://example.com".to_string(),
            description: None,
            variables: Some(IndexMap::new()),
            extensions: IndexMap::new(),
        }],
        paths: openapiv3::Paths {
            paths: IndexMap::new(),
            extensions: IndexMap::new(),
        },
        components: Some(openapiv3::Components {
            schemas: IndexMap::new(),
            responses: IndexMap::new(),
            parameters: IndexMap::new(),
            examples: IndexMap::new(),
            request_bodies: IndexMap::new(),
            headers: IndexMap::new(),
            security_schemes: IndexMap::new(),
            links: IndexMap::new(),
            callbacks: IndexMap::new(),
            extensions: IndexMap::new(),
        }),
        security: None,
        tags: vec![],
        external_docs: None,
        extensions: IndexMap::new(),
    }
}

#[test]
fn ref_resolution_one_level() {
    let mut spec = minimal_spec("ref-test");
    let schema = Schema {
        schema_data: SchemaData {
            nullable: false,
            read_only: false,
            write_only: false,
            deprecated: false,
            external_docs: None,
            example: None,
            title: None,
            description: Some("target".to_string()),
            discriminator: None,
            default: None,
            extensions: IndexMap::new(),
        },
        schema_kind: SchemaKind::Type(Type::String(StringType {
            format: openapiv3::VariantOrUnknownOrEmpty::Empty,
            pattern: None,
            enumeration: vec![Some("hello".to_string())],
            min_length: None,
            max_length: None,
        })),
    };

    spec.components
        .as_mut()
        .unwrap()
        .schemas
        .insert("Foo".to_string(), ReferenceOr::Item(schema));

    let resolved = resolve_spec(&spec, "test").unwrap();
    assert_eq!(resolved.title, "ref-test");
}

#[test]
fn ref_cycle_detected() {
    let mut spec = minimal_spec("cycle-test");
    let schema = Schema {
        schema_data: SchemaData {
            nullable: false,
            read_only: false,
            write_only: false,
            deprecated: false,
            external_docs: None,
            example: None,
            title: None,
            description: None,
            discriminator: None,
            default: None,
            extensions: IndexMap::new(),
        },
        schema_kind: SchemaKind::AllOf {
            all_of: vec![ReferenceOr::Reference {
                reference: "#/components/schemas/SelfRef".to_string(),
            }],
        },
    };

    spec.components
        .as_mut()
        .unwrap()
        .schemas
        .insert("SelfRef".to_string(), ReferenceOr::Item(schema));

    let resolved = resolve_spec(&spec, "test").unwrap();
    // Should not panic; cycle is handled gracefully
    assert_eq!(resolved.title, "cycle-test");
}

#[test]
fn ref_depth_limit() {
    let mut spec = minimal_spec("depth-test");
    // Build a chain of 11 schemas (MAX_REF_DEPTH = 10)
    for i in 0..=11 {
        let name = format!("S{}", i);
        let next = if i < 11 {
            ReferenceOr::Reference {
                reference: format!("#/components/schemas/S{}", i + 1),
            }
        } else {
            ReferenceOr::Item(Schema {
                schema_data: SchemaData {
                    nullable: false,
                    read_only: false,
                    write_only: false,
                    deprecated: false,
                    external_docs: None,
                    example: None,
                    title: None,
                    description: None,
                    discriminator: None,
                    default: None,
                    extensions: IndexMap::new(),
                },
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            })
        };
        spec.components.as_mut().unwrap().schemas.insert(name, next);
    }

    let resolved = resolve_spec(&spec, "test").unwrap();
    assert_eq!(resolved.title, "depth-test");
}

fn make_query_param(name: &str, required: bool) -> openapiv3::Parameter {
    openapiv3::Parameter::Query {
        parameter_data: openapiv3::ParameterData {
            name: name.to_string(),
            description: None,
            required,
            deprecated: None,
            format: openapiv3::ParameterSchemaOrContent::Schema(ReferenceOr::Item(Schema {
                schema_data: SchemaData::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: openapiv3::VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            })),
            extensions: IndexMap::new(),
            example: None,
            examples: IndexMap::new(),
            explode: None,
        },
        allow_reserved: false,
        style: QueryStyle::Form,
        allow_empty_value: None,
    }
}

#[test]
fn parameter_merge_dedup_operation_wins() {
    let spec = minimal_spec("param-merge");
    let path_param = make_query_param("q", false);
    let op_param = make_query_param("q", true);

    let merged = merge_parameters(
        &[ReferenceOr::Item(path_param)],
        &[ReferenceOr::Item(op_param)],
        &spec,
    )
    .unwrap();

    assert_eq!(merged.len(), 1);
    assert!(merged[0].required); // operation wins
}

#[test]
fn security_inheritance_empty_means_no_auth() {
    let op_security: Vec<openapiv3::SecurityRequirement> = vec![];
    let inherited = resolve_security(None, Some(&op_security));
    assert!(inherited.is_empty());
}

#[test]
fn security_inheritance_from_root() {
    let root: Vec<openapiv3::SecurityRequirement> = vec![{
        let mut map = IndexMap::new();
        map.insert("bearerAuth".to_string(), vec![]);
        map
    }];
    let inherited = resolve_security(Some(&root), None);
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].name, "bearerAuth");
}

#[test]
fn server_resolution_priority() {
    let mut spec = minimal_spec("server-test");
    spec.servers = vec![Server {
        url: "https://spec.com".to_string(),
        description: None,
        variables: Some(IndexMap::new()),
        extensions: IndexMap::new(),
    }];

    let mut path = PathItem {
        servers: vec![Server {
            url: "https://path.com".to_string(),
            description: None,
            variables: Some(IndexMap::new()),
            extensions: IndexMap::new(),
        }],
        ..Default::default()
    };

    path.get = Some(Operation {
        operation_id: Some("getTest".to_string()),
        servers: vec![Server {
            url: "https://op.com".to_string(),
            description: None,
            variables: Some(IndexMap::new()),
            extensions: IndexMap::new(),
        }],
        ..Default::default()
    });

    spec.paths
        .paths
        .insert("/test".to_string(), ReferenceOr::Item(path));
    let resolved = resolve_spec(&spec, "test").unwrap();

    assert_eq!(resolved.operations.len(), 1);
    let op = &resolved.operations[0];
    assert_eq!(op.operation_id, "gettest");
    assert_eq!(op.servers[0].url, "https://op.com");
    assert_eq!(resolved.servers[0].url, "https://spec.com");
}
