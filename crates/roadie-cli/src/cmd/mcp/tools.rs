//! The MCP tool surface, executed against the running agent over IPC.
//!
//! Two error channels, deliberately distinct: an unknown tool name is a
//! protocol-level failure (the caller maps it to a JSON-RPC error), while
//! everything that can go wrong *doing* the work — no agent running, a
//! malformed `route` argument, a device write failure — is reported as
//! tool-result text with `isError` set, because that text is what the
//! language model reads to correct itself and retry.
//!
//! Each submodule owns one domain: its tool descriptors and the code that
//! runs them sit together, so adding a tool touches one file. Dispatch stays
//! here as a single match, which keeps the whole surface greppable from one
//! place.

mod audio;
mod bindings;
mod camera;
mod decks;
mod displays;
mod health;
mod inventory;
mod keyboards;
mod keylights;
mod layouts;
mod lighting;
mod monitor;
mod peripherals;
mod pointer;
mod profiles;
mod tourboxes;

use std::time::Duration;

use roadie_core::hid::DeviceRoute;
use roadie_ipc::{AgentClient, ClientKind, PROTOCOL_VERSION, client};
use serde_json::{Value, json};
use tarpc::context;

/// How long to wait for the agent's socket and handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait for any single RPC to answer.
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// Wording every tool that takes a `route` shares, teaching the model where
/// route objects come from instead of letting it invent one.
const ROUTE_HELP: &str = "Pass the `route` object exactly as returned by list_devices; \
     do not construct one by hand.";

/// The schema fragment for a `route` argument.
fn route_schema() -> Value {
    json!({ "type": "object", "description": ROUTE_HELP })
}

/// A tool whose only argument is a device route.
fn route_only_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "route": route_schema() },
        "required": ["route"],
        "additionalProperties": false,
    })
}

/// A tool that takes no arguments at all.
fn no_arguments_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The tool catalog served by `tools/list`.
pub fn catalog() -> Value {
    let mut tools = Vec::new();
    tools.extend(inventory::tools());
    tools.extend(peripherals::tools());
    tools.extend(camera::tools());
    tools.extend(pointer::tools());
    tools.extend(lighting::tools());
    tools.extend(keylights::tools());
    tools.extend(audio::tools());
    tools.extend(monitor::tools());
    tools.extend(displays::tools());
    tools.extend(profiles::tools());
    tools.extend(decks::tools());
    tools.extend(tourboxes::tools());
    tools.extend(keyboards::tools());
    tools.extend(health::tools());
    tools.extend(layouts::tools());
    tools.extend(bindings::tools());
    Value::Array(tools)
}

/// Execute one `tools/call`.
///
/// # Errors
///
/// Only an unrecognized tool name (or a missing one) is an `Err`; the caller
/// turns it into a JSON-RPC invalid-params error. Execution failures come
/// back as `Ok` tool results with `isError` set.
pub async fn call(params: &Value) -> Result<Value, String> {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err("tools/call params carry no tool name".to_string());
    };
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let outcome = match name {
        "list_devices" => inventory::list_devices().await,
        "list_peripherals" => peripherals::list_peripherals().await,
        "reload_config" => inventory::reload_config().await,
        "read_dpi" => pointer::read_dpi(&arguments).await,
        "set_dpi" => pointer::set_dpi(&arguments).await,
        "read_smartshift" => pointer::read_smartshift(&arguments).await,
        "set_smartshift" => pointer::set_smartshift(&arguments).await,
        "set_lighting" => lighting::set_lighting(&arguments).await,
        "set_light" => lighting::set_light(&arguments).await,
        "list_audio_interfaces" => audio::list_audio_interfaces(),
        "set_audio_input" => audio::set_audio_input(&arguments),
        "set_phantom_power" => audio::set_phantom_power(&arguments),
        "list_network_lights" => keylights::list_network_lights().await,
        "set_network_light" => keylights::set_network_light(&arguments).await,
        "watch_input" => monitor::watch_input(&arguments).await,
        "list_cameras" => camera::list_cameras(),
        "list_tourboxes" => tourboxes::list_tourboxes(),
        "watch_tourbox" => tourboxes::watch_tourbox(&arguments),
        "list_displays" => displays::list_displays(),
        "read_display_settings" => displays::read_display_settings(&arguments),
        "set_display_setting" => displays::set_display_setting(&arguments),
        "read_camera_controls" => camera::read_camera_controls(&arguments),
        "set_camera_control" => camera::set_camera_control(&arguments),
        "export_profile" => profiles::export_profile(&arguments),
        "inspect_profile" => profiles::inspect_profile(&arguments),
        "import_profile" => profiles::import_profile(&arguments),
        "config_location" => profiles::config_location(),
        "list_stream_decks" => decks::list_stream_decks().await,
        "set_stream_deck_brightness" => decks::set_stream_deck_brightness(&arguments).await,
        "set_stream_deck_key_colour" => decks::set_stream_deck_key_colour(&arguments).await,
        "set_stream_deck_key_label" => decks::set_stream_deck_key_label(&arguments).await,
        "clear_stream_deck" => decks::clear_stream_deck(&arguments).await,
        "list_keyboards" => keyboards::list_keyboards().await,
        "read_keymap" => keyboards::read_keymap(&arguments).await,
        "set_key" => keyboards::set_key(&arguments).await,
        "diagnose" => health::diagnose().await,
        "list_layouts" => layouts::list_layouts(),
        "apply_layout" => layouts::apply_layout(&arguments).await,
        "set_layout_key" => layouts::set_layout_key(&arguments),
        "unset_layout_key" => layouts::unset_layout_key(&arguments),
        "list_device_keys" => bindings::list_device_keys(),
        "list_actions" => bindings::list_actions(),
        "read_button_bindings" => bindings::read_button_bindings(&arguments),
        "set_button_action" => bindings::set_button_action(&arguments).await,
        "set_action_ring_slot" => bindings::set_action_ring_slot(&arguments).await,
        other => return Err(format!("unknown tool: {other}")),
    };
    Ok(match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    })
}

/// Connect to the agent, complete the handshake, and declare this process a
/// CLI client so a dormant agent serves it without arming its input stack.
async fn agent() -> Result<AgentClient, String> {
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, client::connect())
        .await
        .map_err(|_| no_agent("the agent's socket did not answer in time"))?
        .map_err(|error| no_agent(&error.to_string()))?;
    if conn.version != PROTOCOL_VERSION {
        return Err(format!(
            "the running agent speaks IPC protocol v{}, this CLI expects v{PROTOCOL_VERSION}; \
             they normally ship together, so one side is a stale install",
            conn.version
        ));
    }
    rpc(conn
        .client
        .declare_client(context::current(), ClientKind::Cli))
    .await?;
    Ok(conn.client)
}

/// Phrase an unreachable-agent failure so the model can tell the user what
/// to actually do about it.
fn no_agent(detail: &str) -> String {
    format!(
        "no running OpenRoadie agent is reachable ({detail}); start the OpenRoadie app \
         or the roadie-agent process on this machine, then retry"
    )
}

/// Await one RPC under [`RPC_TIMEOUT`], flattening timeout and transport
/// failures into tool-result text.
async fn rpc<T>(
    call: impl Future<Output = Result<T, tarpc::client::RpcError>>,
) -> Result<T, String> {
    tokio::time::timeout(RPC_TIMEOUT, call)
        .await
        .map_err(|_| "the agent did not answer the request in time".to_string())?
        .map_err(|error| format!("the agent connection failed: {error}"))
}

/// Decode the `route` argument every per-device tool takes.
fn route_argument(arguments: &Value) -> Result<DeviceRoute, String> {
    let Some(route) = arguments.get("route") else {
        return Err(format!("the `route` argument is missing. {ROUTE_HELP}"));
    };
    serde_json::from_value(route.clone()).map_err(|error| {
        format!("the `route` argument is not a device route ({error}). {ROUTE_HELP}")
    })
}

/// Render a value as pretty JSON for a tool result.
fn rendered(value: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to render a result: {error}"))
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::catalog;

    /// The dispatch arms in [`super::call`], as the catalog must advertise
    /// them. Kept here so a tool added to one and not the other fails a test
    /// rather than surfacing as "unknown tool" at run time.
    const DISPATCHED: [&str; 45] = [
        "list_device_keys",
        "list_actions",
        "read_button_bindings",
        "set_button_action",
        "set_action_ring_slot",
        "list_tourboxes",
        "watch_tourbox",
        "list_devices",
        "list_peripherals",
        "reload_config",
        "read_dpi",
        "set_dpi",
        "read_smartshift",
        "set_smartshift",
        "set_lighting",
        "set_light",
        "list_audio_interfaces",
        "set_audio_input",
        "set_phantom_power",
        "list_network_lights",
        "set_network_light",
        "watch_input",
        "list_cameras",
        "read_camera_controls",
        "set_camera_control",
        "list_displays",
        "read_display_settings",
        "set_display_setting",
        "export_profile",
        "inspect_profile",
        "import_profile",
        "config_location",
        "list_stream_decks",
        "set_stream_deck_brightness",
        "set_stream_deck_key_colour",
        "set_stream_deck_key_label",
        "clear_stream_deck",
        "list_keyboards",
        "read_keymap",
        "set_key",
        "diagnose",
        "list_layouts",
        "apply_layout",
        "set_layout_key",
        "unset_layout_key",
    ];

    fn names() -> Vec<String> {
        let Value::Array(tools) = catalog() else {
            panic!("the catalog is an array");
        };
        tools
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .expect("every tool is named")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_catalog_matches_the_dispatch_table_exactly() {
        let mut advertised = names();
        advertised.sort();
        let mut dispatched: Vec<String> = DISPATCHED.iter().map(|s| (*s).to_string()).collect();
        dispatched.sort();
        assert_eq!(advertised, dispatched);
    }

    #[test]
    fn tool_names_are_unique() {
        let mut advertised = names();
        let total = advertised.len();
        advertised.sort();
        advertised.dedup();
        assert_eq!(advertised.len(), total);
    }

    #[test]
    fn every_tool_carries_a_description_and_an_object_schema() {
        let Value::Array(tools) = catalog() else {
            panic!("the catalog is an array");
        };
        for tool in &tools {
            let name = tool["name"].as_str().expect("named");
            assert!(
                tool["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "{name} has no description"
            );
            assert_eq!(
                tool["inputSchema"]["type"], "object",
                "{name} has a non-object input schema"
            );
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{name} accepts undeclared arguments"
            );
        }
    }

    #[test]
    fn every_declared_required_argument_exists_in_its_schema() {
        let Value::Array(tools) = catalog() else {
            panic!("the catalog is an array");
        };
        for tool in &tools {
            let name = tool["name"].as_str().expect("named");
            let schema = &tool["inputSchema"];
            let Some(required) = schema["required"].as_array() else {
                continue;
            };
            for field in required {
                let field = field.as_str().expect("required names are strings");
                assert!(
                    schema["properties"].get(field).is_some(),
                    "{name} requires `{field}` but does not declare it"
                );
            }
        }
    }
}
