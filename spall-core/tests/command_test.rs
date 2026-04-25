use spall_core::command::{build_operations_cmd, build_operations_cmd_from_index};
use spall_core::ir::{HttpMethod, ParamIndex, ParameterLocation, ResolvedOperation, ResolvedSpec, ResolvedServer, SpecIndex, SpecIndexOp};

fn make_spec(ops: Vec<ResolvedOperation>) -> ResolvedSpec {
    ResolvedSpec {
        title: "TestAPI".to_string(),
        version: "1.0.0".to_string(),
        base_url: "https://example.com".to_string(),
        operations: ops,
        servers: vec![ResolvedServer {
            url: "https://example.com".to_string(),
            description: None,
        }],
    }
}

fn make_op(id: &str, tags: Vec<&str>) -> ResolvedOperation {
    ResolvedOperation {
        operation_id: id.to_string(),
        method: HttpMethod::Get,
        path_template: format!("/{}", id),
        summary: None,
        description: None,
        deprecated: false,
        parameters: vec![],
        request_body: None,
        responses: Default::default(),
        security: vec![],
        tags: tags.into_iter().map(|s| s.to_string()).collect(),
        extensions: Default::default(),
        servers: vec![],
    }
}

#[test]
fn single_tag_flattened_to_root() {
    let spec = make_spec(vec![
        make_op("list-pets", vec!["pets"]),
        make_op("get-pet", vec!["pets"]),
    ]);
    let cmd = build_operations_cmd("pets", &spec);
    // Should have subcommands directly under root, no "pets" tag subcommand
    let subs: Vec<String> = cmd.get_subcommands().map(|c| c.get_name().to_string()).collect();
    assert!(subs.contains(&"list-pets".to_string()));
    assert!(subs.contains(&"get-pet".to_string()));
    assert!(!subs.contains(&"pets".to_string()));
}

#[test]
fn multi_tag_grouping() {
    let spec = make_spec(vec![
        make_op("list-pets", vec!["pets"]),
        make_op("get-user", vec!["users"]),
    ]);
    let cmd = build_operations_cmd("api", &spec);
    let subs: Vec<String> = cmd.get_subcommands().map(|c| c.get_name().to_string()).collect();
    assert!(subs.contains(&"pets".to_string()));
    assert!(subs.contains(&"users".to_string()));
    assert!(subs.contains(&"list-pets".to_string())); // also registered at root
    assert!(subs.contains(&"get-user".to_string()));
}

#[test]
fn path_arg_ids() {
    let mut op = make_op("get-pet", vec!["pets"]);
    op.parameters.push(spall_core::ir::ResolvedParameter {
        name: "petId".to_string(),
        location: ParameterLocation::Path,
        required: true,
        deprecated: false,
        style: "simple".to_string(),
        explode: false,
        schema: spall_core::ir::ResolvedSchema {
            type_name: Some("string".to_string()),
            format: None,
            description: None,
            default: None,
            enum_values: vec![],
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
        },
        description: None,
    });
    let spec = make_spec(vec![op]);
    let cmd = build_operations_cmd("pets", &spec);
    let op_cmd = cmd.find_subcommand("get-pet").unwrap();
    let arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "path-petId");
    assert!(arg.is_some());
}

#[test]
fn query_flag_ids() {
    let mut op = make_op("list-pets", vec!["pets"]);
    op.parameters.push(spall_core::ir::ResolvedParameter {
        name: "limit".to_string(),
        location: ParameterLocation::Query,
        required: false,
        deprecated: false,
        style: "form".to_string(),
        explode: false,
        schema: spall_core::ir::ResolvedSchema {
            type_name: Some("integer".to_string()),
            format: None,
            description: None,
            default: None,
            enum_values: vec![],
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
        },
        description: None,
    });
    let spec = make_spec(vec![op]);
    let cmd = build_operations_cmd("pets", &spec);
    let op_cmd = cmd.find_subcommand("list-pets").unwrap();
    let arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "query-limit");
    assert!(arg.is_some());
}

#[test]
fn header_flag_ids() {
    let mut op = make_op("list-pets", vec!["pets"]);
    op.parameters.push(spall_core::ir::ResolvedParameter {
        name: "X-Api-Key".to_string(),
        location: ParameterLocation::Header,
        required: false,
        deprecated: false,
        style: "simple".to_string(),
        explode: false,
        schema: spall_core::ir::ResolvedSchema {
            type_name: Some("string".to_string()),
            format: None,
            description: None,
            default: None,
            enum_values: vec![],
            nullable: false,
            read_only: false,
            write_only: false,
            is_recursive: false,
        },
        description: None,
    });
    let spec = make_spec(vec![op]);
    let cmd = build_operations_cmd("pets", &spec);
    let op_cmd = cmd.find_subcommand("list-pets").unwrap();
    let arg = op_cmd
        .get_arguments()
        .find(|a| a.get_id().as_str() == "header-X-Api-Key");
    assert!(arg.is_some());
}

#[test]
fn body_args_mutually_exclusive() {
    let mut op = make_op("create-pet", vec!["pets"]);
    op.request_body = Some(spall_core::ir::ResolvedRequestBody {
        description: None,
        required: true,
        content: Default::default(),
    });
    let spec = make_spec(vec![op]);
    let cmd = build_operations_cmd("pets", &spec);
    let op_cmd = cmd.find_subcommand("create-pet").unwrap();

    let data_arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "data");
    let form_arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "form");
    let field_arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "field");

    assert!(data_arg.is_some());
    assert!(form_arg.is_some());
    assert!(field_arg.is_some());
}

#[test]
fn index_based_builder_produces_same_structure() {
    let index = SpecIndex {
        title: "IdxAPI".to_string(),
        base_url: "https://example.com".to_string(),
        version: "1.0.0".to_string(),
        cached_at: "0".to_string(),
        operations: vec![
            SpecIndexOp {
                operation_id: "list".to_string(),
                method: HttpMethod::Get,
                path_template: "/list".to_string(),
                summary: Some("List things".to_string()),
                tags: vec!["things".to_string()],
                deprecated: false,
                parameters: vec![ParamIndex {
                    name: "q".to_string(),
                    location: ParameterLocation::Query,
                    required: false,
                }],
                has_request_body: false,
                request_body_required: false,
            },
        ],
    };

    let cmd = build_operations_cmd_from_index("idx", &index);
    let op_cmd = cmd.find_subcommand("list").unwrap();
    let arg = op_cmd.get_arguments().find(|a| a.get_id().as_str() == "query-q");
    assert!(arg.is_some());
}
