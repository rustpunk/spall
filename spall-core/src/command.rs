use crate::ir::{ParameterLocation, ResolvedOperation, ResolvedSpec};
use clap::{Arg, ArgAction, ArgGroup, Command};
use indexmap::IndexMap;

/// Build a clap `Command` tree from a resolved spec for a given API.
///
/// Operations are grouped by their first tag. If the spec contains only a
/// single tag (or the default fallback), operations are registered directly
/// under the API root to avoid forcing users to type a redundant tag name.
pub fn build_operations_cmd(api_name: &str, spec: &ResolvedSpec) -> Command {
    let mut root = Command::new(api_name.to_string())
        .about(format!("{} API ({})", spec.title, spec.version));

    let groups = group_by_tag(&spec.operations);

    // Single-tag flatten: register operations directly under root.
    if groups.len() == 1 {
        for op in groups.values().next().unwrap() {
            root = root.subcommand(build_operation_cmd(op));
        }
        return root;
    }

    // Multi-tag: create tag subcommands and also register ops under root
    // for direct access.
    let mut seen_root: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (tag, ops) in &groups {
        let mut tag_cmd = Command::new(tag.clone())
            .about(format!("{} operations", tag));

        for op in ops {
            tag_cmd = tag_cmd.subcommand(build_operation_cmd(op));
            if seen_root.insert(&op.operation_id) {
                root = root.subcommand(build_operation_cmd(op));
            }
        }

        root = root.subcommand(tag_cmd);
    }

    root
}

/// Build a single operation subcommand with its arguments.
fn build_operation_cmd(op: &ResolvedOperation) -> Command {
    let mut cmd = Command::new(op.operation_id.clone())
        .about(op.summary.clone().unwrap_or_default());

    if op.deprecated {
        cmd = cmd.before_help("[DEPRECATED] This operation is deprecated.");
    }

    // Path params → positional args (internal ID: path-{name})
    for param in &op.parameters {
        if param.location == ParameterLocation::Path {
            let id = format!("path-{}", param.name);
            let mut arg = Arg::new(id.clone())
                .value_name(param.name.clone())
                .required(true)
                .help(
                    param
                        .description
                        .clone()
                        .unwrap_or_default(),
                );

            arg = apply_schema_parsing(arg, &param.schema);
            cmd = cmd.arg(arg);
        }
    }

    // Query params → --flags (internal ID: query-{name})
    for param in &op.parameters {
        if param.location == ParameterLocation::Query {
            let id = format!("query-{}", param.name);
            let mut arg = Arg::new(id.clone())
                .long(param.name.clone())
                .required(param.required)
                .help(
                    param
                        .description
                        .clone()
                        .unwrap_or_default(),
                );

            arg = apply_schema_parsing(arg, &param.schema);
            cmd = cmd.arg(arg);
        }
    }

    // Header params → --header-{name}
    for param in &op.parameters {
        if param.location == ParameterLocation::Header {
            let id = format!("header-{}", param.name);
            let kebab = param.name.to_ascii_lowercase().replace('_', "-");
            let mut arg = Arg::new(id.clone())
                .long(format!("header-{}", kebab))
                .required(param.required)
                .help(
                    param
                        .description
                        .clone()
                        .unwrap_or_default(),
                );

            arg = apply_schema_parsing(arg, &param.schema);
            cmd = cmd.arg(arg);
        }
    }

    // Cookie params → --cookie-{name}
    for param in &op.parameters {
        if param.location == ParameterLocation::Cookie {
            let id = format!("cookie-{}", param.name);
            let kebab = param.name.to_ascii_lowercase().replace('_', "-");
            let mut arg = Arg::new(id.clone())
                .long(format!("cookie-{}", kebab))
                .required(param.required)
                .help(
                    param
                        .description
                        .clone()
                        .unwrap_or_default(),
                );

            arg = apply_schema_parsing(arg, &param.schema);
            cmd = cmd.arg(arg);
        }
    }

    // Request body args
    if let Some(body) = &op.request_body {
        let mut data_arg = Arg::new("data")
            .long("data")
            .short('d')
            .action(ArgAction::Append)
            .help("Request body (JSON). Use @file.json or - for stdin.");

        if body.required {
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

        // Form uploads (Wave 1)
        cmd = cmd.arg(
            Arg::new("form")
                .long("form")
                .action(ArgAction::Append)
                .help("Multipart form field (e.g., file=@image.png)")
                .conflicts_with("data")
                .conflicts_with("field"),
        );

        // Form-encoded fields (Wave 1)
        cmd = cmd.arg(
            Arg::new("field")
                .long("field")
                .action(ArgAction::Append)
                .help("URL-encoded form field (e.g., grant_type=client_credentials)")
                .conflicts_with("data")
                .conflicts_with("form"),
        );

        // Ensure only one body mechanism is used
        cmd = cmd.group(
            ArgGroup::new("body")
                .args(["data", "form", "field"])
                .multiple(false),
        );
    }

    cmd
}

/// Apply schema-aware parsing hints to a clap Arg.
fn apply_schema_parsing(mut arg: Arg, schema: &crate::ir::ResolvedSchema) -> Arg {
    if !schema.enum_values.is_empty() {
        let values: Vec<String> = schema
            .enum_values
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !values.is_empty() {
            arg = arg.value_parser(clap::builder::PossibleValuesParser::new(values));
        }
    }

    if let Some(default) = &schema.default {
        let s = match default.as_str() {
            Some(s) => s.to_string(),
            None => default.to_string(),
        };
        arg = arg.default_value(s);
    }

    arg
}

/// Group operations by their first tag.
fn group_by_tag(
    operations: &[ResolvedOperation],
) -> IndexMap<String, Vec<&ResolvedOperation>> {
    let mut map: IndexMap<String, Vec<&ResolvedOperation>> = IndexMap::new();

    for op in operations {
        let tag = op
            .tags
            .first()
            .cloned()
            .unwrap_or_else(|| "default".to_string());
        map.entry(tag).or_default().push(op);
    }

    map
}
