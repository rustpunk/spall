use indexmap::IndexMap;
use spall_core::ir::{
    HttpMethod, ParameterLocation, ResolvedMediaType, ResolvedOperation, ResolvedParameter,
    ResolvedRequestBody, ResolvedResponse, ResolvedSchema, ResolvedServer, ResolvedSpec,
};
use spall_core::value::SpallValue;

fn make_spec_with_values(title: &str) -> ResolvedSpec {
    let mut extensions = IndexMap::new();
    extensions.insert("x-custom".into(), SpallValue::Str("hello".into()));
    extensions.insert("x-count".into(), SpallValue::U64(42));
    extensions.insert("x-flag".into(), SpallValue::Bool(true));
    extensions.insert("x-null".into(), SpallValue::Null);
    extensions.insert(
        "x-nested".into(),
        SpallValue::Object({
            let mut m = IndexMap::new();
            m.insert("key".into(), SpallValue::F64(3.14));
            m.insert(
                "arr".into(),
                SpallValue::Array(vec![SpallValue::I64(-1), SpallValue::I64(0)]),
            );
            m
        }),
    );

    let schema = ResolvedSchema {
        type_name: Some("string".into()),
        format: None,
        description: None,
        default: Some(SpallValue::Str("defaultval".into())),
        enum_values: vec![SpallValue::Str("a".into()), SpallValue::Str("b".into())],
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
    };

    let mut content = IndexMap::new();
    content.insert(
        "application/json".into(),
        ResolvedMediaType {
            schema: Some(schema.clone()),
            example: Some(SpallValue::Object({
                let mut m = IndexMap::new();
                m.insert("id".into(), SpallValue::U64(1));
                m
            })),
            examples: {
                let mut ex = IndexMap::new();
                ex.insert("default".into(), SpallValue::Str("example1".into()));
                ex
            },
        },
    );

    ResolvedSpec {
        title: title.into(),
        version: "1.0.0".into(),
        base_url: "https://example.com".into(),
        operations: vec![ResolvedOperation {
            operation_id: "test".into(),
            method: HttpMethod::Get,
            path_template: "/test".into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: vec![ResolvedParameter {
                name: "q".into(),
                location: ParameterLocation::Query,
                required: false,
                deprecated: false,
                style: "form".into(),
                explode: false,
                schema,
                description: None,
                extensions: Default::default(),
            }],
            request_body: Some(ResolvedRequestBody {
                description: None,
                required: false,
                content,
            }),
            responses: {
                let mut r = IndexMap::new();
                r.insert(
                    "200".into(),
                    ResolvedResponse {
                        description: Some("ok".into()),
                        content: IndexMap::new(),
                        headers: IndexMap::new(),
                    },
                );
                r
            },
            security: vec![],
            tags: vec![],
            extensions,
            servers: vec![],
        }],
        servers: vec![ResolvedServer {
            url: "https://example.com".into(),
            description: None,
        }],
    }
}

#[test]
fn cache_roundtrip_all_spallvalue_variants() {
    let tmp = tempfile::tempdir().unwrap();
    let spec = make_spec_with_values("roundtrip");
    let raw = b"opaque";
    let raw_hash = spall_core::cache::spec_hash(raw);

    spall_core::cache::write_cache("src", &spec, raw_hash, tmp.path()).unwrap();
    let loaded = spall_core::cache::load_or_resolve("src", raw, tmp.path()).unwrap();

    assert_eq!(loaded.title, spec.title);

    let op = &loaded.operations[0];
    assert_eq!(op.extensions.len(), 5);

    // Spot-check specific variants
    assert_eq!(
        op.extensions.get("x-custom").unwrap().as_str(),
        Some("hello")
    );
    assert_eq!(op.extensions.get("x-null").unwrap(), &SpallValue::Null);

    let nested = op.extensions.get("x-nested").unwrap();
    match nested {
        SpallValue::Object(m) => {
            assert_eq!(m.get("key").unwrap(), &SpallValue::F64(3.14));
            match m.get("arr").unwrap() {
                SpallValue::Array(a) => assert_eq!(a.len(), 2),
                _ => panic!("expected array"),
            }
        }
        _ => panic!("expected object"),
    }

    // Request body content
    let body = op.request_body.as_ref().unwrap();
    let mt = body.content.get("application/json").unwrap();
    assert!(mt.example.is_some());
    assert_eq!(
        mt.examples.get("default").unwrap().as_str(),
        Some("example1")
    );
}

#[test]
fn petstore_parsed_spec_roundtrips_via_postcard() {
    // Uses a real downloaded spec (fetched during CI or present locally).
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/fixtures/petstore.json");
    let raw = std::fs::read(&fixture).unwrap_or_else(|_| {
        // If fixture not present, skip (integration test in CI should download it).
        eprintln!("petstore fixture not found at {:?}, skipping", fixture);
        return Vec::new();
    });
    if raw.is_empty() {
        return;
    }

    let spec =
        spall_core::loader::load_spec_from_bytes(&raw, "petstore.json").expect("parse petstore");

    let tmp = tempfile::tempdir().unwrap();
    spall_core::cache::write_cache(
        "petstore.json",
        &spec,
        spall_core::cache::spec_hash(&raw),
        tmp.path(),
    )
    .expect("write cache");

    let loaded =
        spall_core::cache::load_or_resolve("petstore.json", &raw, tmp.path()).expect("load cache");
    assert_eq!(loaded.title, spec.title);
}
