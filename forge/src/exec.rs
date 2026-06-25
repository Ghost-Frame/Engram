//! The `forge exec` command implementation.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::client::KleosClient;
use crate::dispatch::{ConfigSummary, DispatchConfig, ParamSpec};
use crate::error::{ForgeError, Result};
use crate::output::{self, Format};

/// List available skills by fetching dispatch configs from the server.
pub async fn list_skills(client: &KleosClient) -> Result<()> {
    let resp = client.get("/dispatch/configs").await?;
    let configs: Vec<ConfigSummary> = resp
        .get("configs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    if configs.is_empty() {
        println!("No skills available.");
        return Ok(());
    }

    println!("Available skills:");
    for cfg in &configs {
        println!("  {:<20} {}", cfg.skill_name, cfg.description);
    }
    Ok(())
}

/// Print dynamic help for a skill based on its dispatch config.
pub fn print_skill_help(config: &DispatchConfig) {
    println!("{}: {}\n", config.skill_name, config.description);
    println!("USAGE:");
    println!("    forge exec {} [OPTIONS]\n", config.skill_name);

    let mut required: Vec<(&String, &ParamSpec)> = Vec::new();
    let mut optional: Vec<(&String, &ParamSpec)> = Vec::new();

    for (name, spec) in &config.params_schema {
        if spec.required {
            &mut required
        } else {
            &mut optional
        }
        .push((name, spec));
    }

    if !required.is_empty() {
        println!("REQUIRED:");
        for (name, spec) in &required {
            let flag = spec.cli_name.as_deref().unwrap_or(name);
            let desc = spec.description.as_deref().unwrap_or("");
            println!("    --{flag:<20} {desc}");
        }
        println!();
    }

    if !optional.is_empty() {
        println!("OPTIONS:");
        for (name, spec) in &optional {
            let flag = spec.cli_name.as_deref().unwrap_or(name);
            let desc = spec.description.as_deref().unwrap_or("");
            let suffix = spec
                .default
                .as_ref()
                .map(|d| format!(" [default: {d}]"))
                .unwrap_or_default();
            println!("    --{flag:<20} {desc}{suffix}");
        }
        println!();
    }

    if let Some(fields) = &config.output_hints.summary_fields {
        println!("OUTPUT FIELDS: {}", fields.join(", "));
    }
}

/// Execute a skill: validate params, send request, format output.
pub async fn execute_skill(
    client: &KleosClient,
    config: &DispatchConfig,
    raw_args: &[String],
    format: Format,
) -> Result<()> {
    let params = parse_args(raw_args, &config.params_schema)?;
    let body = build_body(params, &config.params_schema)?;

    let resp = match config.method.to_uppercase().as_str() {
        "POST" => client.post(&config.endpoint, &body).await?,
        // FORGE-3 fix: GET skills must pass their parameters as query-string values.
        // Previously body was built and then discarded, silently sending no params.
        "GET" => client.get_with_query(&config.endpoint, &body).await?,
        other => {
            return Err(ForgeError::InvalidParam(
                "method".into(),
                format!("unsupported HTTP method: {other}"),
            ))
        }
    };

    let output = output::format_output(&resp, format, &config.output_hints, &config.skill_name);
    print!("{output}");
    Ok(())
}

/// Fetch a dispatch config by skill name from the server.
pub async fn fetch_config(client: &KleosClient, skill_name: &str) -> Result<DispatchConfig> {
    let path = format!("/dispatch/configs/{skill_name}");
    let resp = client.get(&path).await;

    match resp {
        Ok(val) => {
            let config: DispatchConfig = serde_json::from_value(val)?;
            if !config.enabled {
                return Err(ForgeError::DisabledSkill(skill_name.into()));
            }
            Ok(config)
        }
        Err(ForgeError::Server(404, _)) => Err(ForgeError::UnknownSkill(skill_name.into())),
        Err(e) => Err(e),
    }
}

/// Parse `--key value` pairs from raw CLI args into a map.
fn parse_args(
    args: &[String],
    schema: &HashMap<String, ParamSpec>,
) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    let mut iter = args.iter();

    while let Some(arg) = iter.next() {
        if !arg.starts_with("--") {
            return Err(ForgeError::InvalidParam(
                arg.clone(),
                "expected --key or --key value".into(),
            ));
        }
        let key = arg.trim_start_matches('-');

        // Resolve cli_name aliases back to the canonical param name.
        let canonical = schema
            .iter()
            .find(|(_, spec)| spec.cli_name.as_deref() == Some(key))
            .map(|(k, _)| k.as_str())
            .unwrap_or(key);

        if !schema.contains_key(canonical) {
            return Err(ForgeError::UnknownParam(key.into()));
        }

        let spec = &schema[canonical];
        if spec.param_type == "boolean" {
            result.insert(canonical.to_string(), "true".into());
        } else {
            let val = iter
                .next()
                .ok_or_else(|| ForgeError::InvalidParam(key.into(), "expected a value".into()))?;
            result.insert(canonical.to_string(), val.clone());
        }
    }

    Ok(result)
}

/// Build a JSON request body from validated params and schema defaults.
fn build_body(
    mut params: HashMap<String, String>,
    schema: &HashMap<String, ParamSpec>,
) -> Result<Value> {
    let mut body = serde_json::Map::new();

    for (name, spec) in schema {
        if let Some(raw) = params.remove(name) {
            let val = coerce_value(&raw, spec, name)?;
            body.insert(name.clone(), val);
        } else if spec.required {
            if let Some(default) = &spec.default {
                body.insert(name.clone(), default.clone());
            } else {
                return Err(ForgeError::MissingParam(name.clone()));
            }
        } else if let Some(default) = &spec.default {
            body.insert(name.clone(), default.clone());
        }
    }

    Ok(Value::Object(body))
}

/// Coerce a string value to the type declared in the param spec.
fn coerce_value(raw: &str, spec: &ParamSpec, name: &str) -> Result<Value> {
    // Enum validation
    if let Some(allowed) = &spec.allowed_values {
        if !allowed.iter().any(|a| a == raw) {
            return Err(ForgeError::InvalidParam(
                name.into(),
                format!("'{}' is not one of: {}", raw, allowed.join(", ")),
            ));
        }
    }

    match spec.param_type.as_str() {
        "string" => Ok(Value::String(raw.into())),
        "integer" => {
            let n: i64 = raw.parse().map_err(|_| {
                ForgeError::InvalidParam(name.into(), format!("'{raw}' is not an integer"))
            })?;
            Ok(json!(n))
        }
        "boolean" => {
            let b: bool = raw.parse().map_err(|_| {
                ForgeError::InvalidParam(name.into(), format!("'{raw}' is not a boolean"))
            })?;
            Ok(json!(b))
        }
        other => Err(ForgeError::InvalidParam(
            name.into(),
            format!("unknown type '{other}'"),
        )),
    }
}
