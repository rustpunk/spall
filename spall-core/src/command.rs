use crate::extensions::CliExtensions;
use crate::ir::{ParameterLocation, ResolvedSpec, SpecIndex};
use crate::value::SpallValue;
use clap::{Arg, ArgAction, ArgGroup, Command};
use indexmap::IndexMap;

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
    schema: Option<&'a crate::ir::ResolvedSchema>,
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
                .help("Multipart form field (e.g., file=@image.png)")
                .conflicts_with("data")
                .conflicts_with("field"),
        );

        cmd = cmd.arg(
            Arg::new("field")
                .long("field")
                .action(ArgAction::Append)
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

fn apply_schema_parsing(mut arg: Arg, schema: &crate::ir::ResolvedSchema) -> Arg {
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
