use clap::{Arg, ArgAction, ArgGroup, Command};
use indexmap::IndexMap;
use spall_core::extensions::CliExtensions;
use spall_core::ir::{ParameterLocation, ResolvedSpec, SpecIndex};
use spall_core::value::SpallValue;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a clap `Command` tree from a resolved spec for a given API.
pub fn build_operations_cmd(api_name: &str, spec: &ResolvedSpec) -> Command {
    let light_ops: Vec<LightOp> = spec
        .operations
        .iter()
        .map(|op| {
            let ext = CliExtensions::from_operation(op);
            LightOp {
                operation_id: &op.operation_id,
                cli_name: ext.cli_name,
                hidden: ext.hidden,
                group: ext.group,
                summary: op.summary.as_deref(),
                deprecated: op.deprecated,
                tags: &op.tags,
                params: op
                    .parameters
                    .iter()
                    .map(|p| {
                        let p_ext = CliExtensions::from_parameter(p);
                        LightParam {
                            name: &p.name,
                            cli_name: p_ext.cli_name,
                            hidden: p_ext.hidden,
                            location: p.location,
                            required: p.required,
                            description: p.description.as_deref(),
                            schema: Some(&p.schema),
                        }
                    })
                    .collect(),
                has_body: op.request_body.is_some(),
                body_required: op
                    .request_body
                    .as_ref()
                    .map(|b| b.required)
                    .unwrap_or(false),
            }
        })
        .collect();
    build_root_cmd(api_name, &spec.title, &spec.version, &light_ops)
}

/// Build a clap `Command` tree from a lightweight cached index.
pub fn build_operations_cmd_from_index(api_name: &str, index: &SpecIndex) -> Command {
    let light_ops: Vec<LightOp> = index
        .operations
        .iter()
        .map(|op| LightOp {
            operation_id: &op.operation_id,
            cli_name: None,
            hidden: false,
            group: None,
            summary: op.summary.as_deref(),
            deprecated: op.deprecated,
            tags: &op.tags,
            params: op
                .parameters
                .iter()
                .map(|p| LightParam {
                    name: &p.name,
                    cli_name: None,
                    hidden: false,
                    location: p.location,
                    required: p.required,
                    description: None,
                    schema: None,
                })
                .collect(),
            has_body: op.has_request_body,
            body_required: op.request_body_required,
        })
        .collect();
    build_root_cmd(api_name, &index.title, &index.version, &light_ops)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

struct LightParam<'a> {
    name: &'a str,
    cli_name: Option<String>,
    hidden: bool,
    location: ParameterLocation,
    required: bool,
    description: Option<&'a str>,
    schema: Option<&'a spall_core::ir::ResolvedSchema>,
}

struct LightOp<'a> {
    operation_id: &'a str,
    cli_name: Option<String>,
    hidden: bool,
    group: Option<String>,
    summary: Option<&'a str>,
    deprecated: bool,
    tags: &'a [String],
    params: Vec<LightParam<'a>>,
    has_body: bool,
    body_required: bool,
}

fn build_root_cmd(api_name: &str, title: &str, version: &str, ops: &[LightOp]) -> Command {
    let mut root = Command::new(api_name.to_string()).about(format!("{} API ({})", title, version));

    let groups = group_by_tag(ops);

    if groups.len() == 1 {
        for op in groups.values().next().unwrap() {
            root = root.subcommand(build_op_cmd(op));
        }
        return root;
    }

    let mut seen_root: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (tag, tag_ops) in &groups {
        let mut tag_cmd = Command::new(tag.clone()).about(format!("{} operations", tag));

        for op in tag_ops {
            tag_cmd = tag_cmd.subcommand(build_op_cmd(op));
            let display = op
                .cli_name
                .clone()
                .unwrap_or_else(|| op.operation_id.to_string());
            if seen_root.insert(display) {
                root = root.subcommand(build_op_cmd(op));
            }
        }

        root = root.subcommand(tag_cmd);
    }

    root
}

fn build_op_cmd(op: &LightOp) -> Command {
    let cmd_name = op
        .cli_name
        .clone()
        .unwrap_or_else(|| op.operation_id.to_string());
    let mut cmd = Command::new(cmd_name.clone()).about(op.summary.unwrap_or_default().to_string());

    if op.deprecated {
        cmd = cmd.before_help("[DEPRECATED] This operation is deprecated.");
    }

    for param in &op.params {
        if param.hidden {
            continue;
        }
        let long_name = param.cli_name.as_deref().unwrap_or(param.name);
        match param.location {
            ParameterLocation::Path => {
                let id = format!("path-{}", param.name);
                let mut arg = Arg::new(id.clone())
                    .value_name(param.name.to_string())
                    .required(param.required)
                    .allow_hyphen_values(true)
                    .help(param.description.unwrap_or_default().to_string());
                if let Some(schema) = param.schema {
                    arg = apply_schema_parsing(arg, schema);
                }
                cmd = cmd.arg(arg);
            }
            ParameterLocation::Query => {
                let id = format!("query-{}", param.name);
                let mut arg = Arg::new(id.clone())
                    .long(long_name.to_string())
                    .required(param.required)
                    .allow_hyphen_values(true)
                    .help(param.description.unwrap_or_default().to_string());
                if let Some(schema) = param.schema {
                    arg = apply_schema_parsing(arg, schema);
                }
                cmd = cmd.arg(arg);
            }
            ParameterLocation::Header => {
                let id = format!("header-{}", param.name);
                let kebab = long_name.to_ascii_lowercase().replace('_', "-");
                let mut arg = Arg::new(id.clone())
                    .long(format!("header-{}", kebab))
                    .required(param.required)
                    .allow_hyphen_values(true)
                    .help(param.description.unwrap_or_default().to_string());
                if let Some(schema) = param.schema {
                    arg = apply_schema_parsing(arg, schema);
                }
                cmd = cmd.arg(arg);
            }
            ParameterLocation::Cookie => {
                let id = format!("cookie-{}", param.name);
                let kebab = long_name.to_ascii_lowercase().replace('_', "-");
                let mut arg = Arg::new(id.clone())
                    .long(format!("cookie-{}", kebab))
                    .required(param.required)
                    .allow_hyphen_values(true)
                    .help(param.description.unwrap_or_default().to_string());
                if let Some(schema) = param.schema {
                    arg = apply_schema_parsing(arg, schema);
                }
                cmd = cmd.arg(arg);
            }
        }
    }

    if op.has_body {
        let mut data_arg = Arg::new("data")
            .long("data")
            .short('d')
            .action(ArgAction::Append)
            .allow_hyphen_values(true)
            .help("Request body (JSON). Use @file.json or - for stdin.");

        if op.body_required {
            data_arg = data_arg.required(true);
        } else {
            cmd = cmd.arg(
                Arg::new("no-data")
                    .long("no-data")
                    .action(ArgAction::SetTrue)
                    .help("Send request with no body"),
            );
        }

        cmd = cmd.arg(data_arg);

        cmd = cmd.arg(
            Arg::new("form")
                .long("form")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .help("Multipart form field (e.g., file=@image.png)")
                .conflicts_with("data")
                .conflicts_with("field"),
        );

        cmd = cmd.arg(
            Arg::new("field")
                .long("field")
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .help("URL-encoded form field (e.g., grant_type=client_credentials)")
                .conflicts_with("data")
                .conflicts_with("form"),
        );

        cmd = cmd.group(
            ArgGroup::new("body")
                .args(["data", "form", "field"])
                .multiple(false),
        );
    }

    cmd
}

fn apply_schema_parsing(mut arg: Arg, schema: &spall_core::ir::ResolvedSchema) -> Arg {
    if !schema.enum_values.is_empty() {
        let values: Vec<String> = schema
            .enum_values
            .iter()
            .filter_map(|v| match v {
                SpallValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        if !values.is_empty() {
            arg = arg.value_parser(clap::builder::PossibleValuesParser::new(values));
        }
    }

    if let Some(default) = &schema.default {
        let s = match default {
            SpallValue::Str(s) => s.clone(),
            other => format!("{}", other),
        };
        arg = arg.default_value(s);
    }

    arg
}

fn group_by_tag<'a>(ops: &'a [LightOp<'a>]) -> IndexMap<String, Vec<&'a LightOp<'a>>> {
    let mut map: IndexMap<String, Vec<&LightOp>> = IndexMap::new();

    for op in ops {
        if op.hidden {
            continue;
        }
        let tag = op
            .group
            .clone()
            .or_else(|| op.tags.first().cloned())
            .unwrap_or_else(|| "default".to_string());
        map.entry(tag).or_default().push(op);
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light_param(name: &'static str, location: ParameterLocation) -> LightParam<'static> {
        LightParam {
            name,
            cli_name: None,
            hidden: false,
            location,
            required: true,
            description: None,
            schema: None,
        }
    }

    /// An op command must accept a positional path value and a `--long` query
    /// value that begin with `-`, so chained dash-prefixed values (issue #36)
    /// parse instead of being mistaken for flags.
    #[test]
    fn op_cmd_accepts_dash_prefixed_path_and_query_values() {
        let op = LightOp {
            operation_id: "update-thing",
            cli_name: None,
            hidden: false,
            group: None,
            summary: None,
            deprecated: false,
            tags: &[],
            params: vec![
                light_param("id", ParameterLocation::Path),
                light_param("offset", ParameterLocation::Query),
            ],
            has_body: false,
            body_required: false,
        };

        let cmd = build_op_cmd(&op);
        let matches = cmd
            .try_get_matches_from(["update-thing", "-5", "--offset", "-10"])
            .expect("dash-prefixed positional path and --long query values must parse");

        assert_eq!(
            matches.get_one::<String>("path-id").map(String::as_str),
            Some("-5"),
            "positional path value beginning with '-' should be captured"
        );
        assert_eq!(
            matches
                .get_one::<String>("query-offset")
                .map(String::as_str),
            Some("-10"),
            "query value beginning with '-' should be captured"
        );
    }

    /// A dash-prefixed `--data` body value must parse too (issue #36 covers the
    /// body args as well as path/query/header/cookie params).
    #[test]
    fn op_cmd_accepts_dash_prefixed_data_body() {
        let op = LightOp {
            operation_id: "create-thing",
            cli_name: None,
            hidden: false,
            group: None,
            summary: None,
            deprecated: false,
            tags: &[],
            params: vec![],
            has_body: true,
            body_required: false,
        };

        let cmd = build_op_cmd(&op);
        let matches = cmd
            .try_get_matches_from(["create-thing", "--data", "-not-a-flag"])
            .expect("dash-prefixed --data value must parse");

        assert_eq!(
            matches
                .get_many::<String>("data")
                .map(|v| v.map(String::as_str).collect::<Vec<_>>()),
            Some(vec!["-not-a-flag"]),
            "data value beginning with '-' should be captured"
        );
    }

    use spall_core::ir::{
        HttpMethod, ParamIndex, ResolvedOperation, ResolvedServer, ResolvedSpec, SpecIndexOp,
    };

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
        let subs: Vec<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
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
        let subs: Vec<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
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
            },
            description: None,
            extensions: Default::default(),
        });
        let spec = make_spec(vec![op]);
        let cmd = build_operations_cmd("pets", &spec);
        let op_cmd = cmd.find_subcommand("get-pet").unwrap();
        let arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "path-petId");
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
            },
            description: None,
            extensions: Default::default(),
        });
        let spec = make_spec(vec![op]);
        let cmd = build_operations_cmd("pets", &spec);
        let op_cmd = cmd.find_subcommand("list-pets").unwrap();
        let arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "query-limit");
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
            },
            description: None,
            extensions: Default::default(),
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

        let data_arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "data");
        let form_arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "form");
        let field_arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "field");

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
            operations: vec![SpecIndexOp {
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
            }],
        };

        let cmd = build_operations_cmd_from_index("idx", &index);
        let op_cmd = cmd.find_subcommand("list").unwrap();
        let arg = op_cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "query-q");
        assert!(arg.is_some());
    }
}
