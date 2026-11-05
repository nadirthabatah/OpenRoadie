//! Mouse and keyboard button actions and the Actions Ring, edited in the
//! config the way the settings window edits them.
//!
//! Actions are taken by name only, the way `set_layout_key` takes them: an
//! action that runs a program or types text carries a value, so it cannot be
//! named here, and binding one stays the person's to write.

use roadie_core::binding::{Action, ActionRingSlot, Binding, ButtonId, RingAction};
use roadie_core::config::{Config, ConfigFile};
use serde_json::{Value, json};

use super::{no_arguments_schema, rendered};

/// Wording every tool here shares about where a `device_key` comes from.
const KEY_HELP: &str = "Pass a `device_key` exactly as list_device_keys returns it.";

/// Tool descriptors owned by this module.
pub fn tools() -> Vec<Value> {
    let device_key = json!({ "type": "string", "description": KEY_HELP });
    vec![
        json!({
            "name": "list_device_keys",
            "description": "List the devices whose buttons can be set, by the \
                `device_key` the other binding tools take, with each one's name and \
                kind. A device appears once OpenRoadie has seen it.",
            "inputSchema": no_arguments_schema(),
        }),
        json!({
            "name": "list_actions",
            "description": "List every action a button or an Actions Ring slot can be \
                set to, by the name set_button_action and set_action_ring_slot take, \
                with a readable label. Actions that run a program or type text are not \
                listed: those are the person's to write in the config.",
            "inputSchema": no_arguments_schema(),
        }),
        json!({
            "name": "read_button_bindings",
            "description": "Read what each button on one device has been set to, and its \
                Actions Ring. A button not listed does its default action.",
            "inputSchema": {
                "type": "object",
                "properties": { "device_key": device_key },
                "required": ["device_key"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "set_button_action",
            "description": "Set what one button does, save it to the config, and have \
                the agent apply it. Replaces whatever the button did, gesture directions \
                included. If the OpenRoadie window is open, reopen it to see the change.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_key": device_key,
                    "button": {
                        "type": "string",
                        "description": "The button, by name: \"MiddleClick\", \"Back\", \
                            \"Forward\", \"DpiToggle\", \"GestureButton\", \
                            \"ThumbwheelScrollUp\" and so on, as read_button_bindings \
                            names them.",
                    },
                    "action": {
                        "type": "string",
                        "description": "An action name from list_actions.",
                    },
                },
                "required": ["device_key", "button", "action"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "set_action_ring_slot",
            "description": "Set or clear one of the eight Actions Ring slots on a device, \
                save it to the config, and have the agent apply it. Leave `action` out to \
                empty the slot.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "device_key": device_key,
                    "slot": {
                        "type": "string",
                        "description": "The slot, clockwise from twelve o'clock: \"Top\", \
                            \"TopRight\", \"Right\", \"BottomRight\", \"Bottom\", \
                            \"BottomLeft\", \"Left\", \"TopLeft\".",
                    },
                    "action": {
                        "type": "string",
                        "description": "An action name from list_actions.",
                    },
                },
                "required": ["device_key", "slot"],
                "additionalProperties": false,
            },
        }),
    ]
}

/// Every device the config knows, by the key the binding tools take.
pub fn list_device_keys() -> Result<String, String> {
    let config = load()?.0;
    let devices: Vec<Value> = config
        .known_identities()
        .map(|(key, identity)| {
            json!({
                "device_key": key,
                "name": config.device_custom_name(key).unwrap_or(&identity.display_name),
                "kind": identity.kind,
            })
        })
        .collect();
    rendered(&Value::Array(devices))
}

/// The pickable actions that are nothing but a name.
pub fn list_actions() -> Result<String, String> {
    rendered(&Value::Array(
        named_actions()
            .map(|(name, action)| json!({ "name": name, "label": action.label() }))
            .collect(),
    ))
}

/// One device's button bindings and Actions Ring.
pub fn read_button_bindings(arguments: &Value) -> Result<String, String> {
    let key = text(arguments, "device_key")?;
    let config = load()?.0;
    known(&config, &key)?;
    rendered(&json!({
        "bindings": config.bindings_for(&key),
        "action_ring": config.action_ring(&key),
    }))
}

/// Bind one button to one named action.
pub async fn set_button_action(arguments: &Value) -> Result<String, String> {
    let key = text(arguments, "device_key")?;
    let button = button(&text(arguments, "button")?)?;
    let action = action(&text(arguments, "action")?)?;

    let (mut config, mut file) = load()?;
    known(&config, &key)?;
    let was = config.bindings_for(&key).remove(&button);
    config.set_binding(&key, button, Binding::Single(action.clone()));
    file.save(&config)
        .map_err(|error| format!("the config was not saved: {error}"))?;
    rendered(&json!({
        "button": button,
        "was": was,
        "now": action,
        "applied": applied().await,
    }))
}

/// Set or clear one Actions Ring slot.
pub async fn set_action_ring_slot(arguments: &Value) -> Result<String, String> {
    let key = text(arguments, "device_key")?;
    let slot = slot(&text(arguments, "slot")?)?;
    let action = match arguments.get("action").and_then(Value::as_str) {
        Some(name) => Some(
            RingAction::new(action(name)?)
                .map_err(|error| format!("{name} cannot go in an Actions Ring: {error}"))?,
        ),
        None => None,
    };

    let (mut config, mut file) = load()?;
    known(&config, &key)?;
    config.set_action_ring_slot(&key, slot, action);
    file.save(&config)
        .map_err(|error| format!("the config was not saved: {error}"))?;
    rendered(&json!({
        "slot": slot,
        "now": config.action_ring(&key).default,
        "applied": applied().await,
    }))
}

/// The catalogue's actions that serialise as a bare name, with that name.
///
/// The same filter decides what [`action`] accepts, so the list and the
/// setter cannot disagree about what may be named.
fn named_actions() -> impl Iterator<Item = (String, Action)> {
    Action::catalog()
        .into_iter()
        .filter_map(|action| match serde_json::to_value(&action) {
            Ok(Value::String(name)) => Some((name, action)),
            _ => None,
        })
}

/// A named action from [`named_actions`].
fn action(name: &str) -> Result<Action, String> {
    named_actions()
        .find(|(known, _)| known == name)
        .map(|(_, action)| action)
        .ok_or_else(|| {
            format!(
                "{name} is not an action that can be set here. list_actions lists them; \
                 an action that runs a program or types text has to be written in the \
                 config by the person."
            )
        })
}

/// A button, by the name the config uses for it.
fn button(name: &str) -> Result<ButtonId, String> {
    serde_json::from_value(Value::String(name.to_owned())).map_err(|_| {
        format!("{name} is not a button name. read_button_bindings shows the names in use.")
    })
}

/// An Actions Ring slot, by the name the config uses for it.
fn slot(name: &str) -> Result<ActionRingSlot, String> {
    serde_json::from_value(Value::String(name.to_owned())).map_err(|_| {
        format!(
            "{name} is not a slot. The slots are Top, TopRight, Right, BottomRight, \
             Bottom, BottomLeft, Left and TopLeft."
        )
    })
}

/// A required string argument.
fn text(arguments: &Value, name: &str) -> Result<String, String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("the `{name}` argument is missing"))
}

/// The config, and the handle that saves it back only if nothing else changed
/// it in between.
fn load() -> Result<(Config, ConfigFile), String> {
    ConfigFile::load_or_default().map_err(|error| format!("the config could not be read: {error}"))
}

/// Refuse a key the config has no device for, rather than start a new entry
/// under a mistyped one.
fn known(config: &Config, key: &str) -> Result<(), String> {
    if config.devices.contains_key(key) {
        Ok(())
    } else {
        Err(format!(
            "there is no device {key} in the config. {KEY_HELP}"
        ))
    }
}

/// Have the agent re-read the saved config, and say whether it did.
async fn applied() -> String {
    match super::inventory::reload_config().await {
        Ok(_) => "the agent is using it now".to_owned(),
        Err(why) => format!(
            "saved, but the agent did not reload ({why}); it applies the next time \
             the agent starts or reload_config succeeds"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_actions_are_exactly_the_ones_that_can_be_set() {
        let names: Vec<String> = named_actions().map(|(name, _)| name).collect();
        for name in &names {
            action(name).expect("every listed action can be set");
        }
        assert!(names.iter().any(|name| name == "BrowserBack"), "{names:?}");
    }

    #[test]
    fn an_action_that_carries_a_program_cannot_be_named() {
        let error = action("RunShellCommand").expect_err("a program action is refused");
        assert!(error.contains("by the person"), "{error}");
        assert!(named_actions().all(|(name, _)| name != "TypeText"));
    }

    #[test]
    fn buttons_and_slots_are_read_by_their_config_names() {
        assert_eq!(button("MiddleClick"), Ok(ButtonId::MiddleClick));
        assert_eq!(slot("TopRight"), Ok(ActionRingSlot::TopRight));
        assert!(
            button("Wheel")
                .expect_err("unknown")
                .contains("not a button")
        );
        assert!(slot("Middle").expect_err("unknown").contains("not a slot"));
    }

    #[test]
    fn a_missing_or_blank_argument_is_named() {
        let error = text(&json!({ "device_key": "  " }), "device_key").expect_err("blank");
        assert!(error.contains("device_key"), "{error}");
    }
}
