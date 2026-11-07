//! End-to-end tests against the built `roadie` binary.
//!
//! Everything else in this workspace tests a function. This runs the program a
//! person actually runs, reads what it prints, and checks the exit status a
//! script would branch on — the layer where a command wired to the wrong
//! function, or a `main` that swallows an exit code, would still pass every
//! unit test in the repository.
//!
//! # What can and cannot be asserted here
//!
//! These run on developer machines and on three CI platforms, and a CI runner
//! is not an empty desk: a macOS runner has a built-in keyboard and trackpad.
//! So anything that depends on what hardware is present is checked
//! *structurally* — the command ran, said something coherent, and exited with
//! one of the statuses it documents — while everything that does not depend on
//! hardware is checked exactly.
//!
//! That division is deliberate rather than a shortcut. The hardware-free half
//! is most of what someone actually uses this for: saving a setup, carrying it
//! to another computer, and putting it back. That path is fully exercised
//! here, end to end, through the real binary.
//!
//! Every test runs against its own configuration directory, so none of them
//! can see or damage the configuration of whoever is running them.

#![expect(
    clippy::tests_outside_test_module,
    reason = "an integration test file is already its own test-only crate"
)]
#![expect(
    clippy::expect_used,
    reason = "the sandbox helpers sit outside any `#[test]` fn, where `allow-expect-in-tests` cannot see them"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A configuration directory of this test's own.
///
/// `XDG_CONFIG_HOME` is honoured on every platform by this project — a
/// deliberate upstream decision — so one environment variable isolates a test
/// run completely, on Linux, macOS and Windows alike.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "roadie-cli-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        ));
        std::fs::create_dir_all(&root).expect("a sandbox directory");
        Self { root }
    }

    /// Run `roadie` with these arguments inside the sandbox.
    fn run(&self, arguments: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_roadie"))
            .args(arguments)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            // The agent is a separate process and none of these commands need
            // one; pointing its socket into the sandbox keeps a developer's
            // running agent out of the results.
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .output()
            .expect("the roadie binary runs");
        Run {
            arguments: arguments.iter().map(|a| (*a).to_owned()).collect(),
            output,
        }
    }

    fn path(&self, tail: &str) -> PathBuf {
        self.root.join(tail)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One run of the program, with everything a test might assert on.
struct Run {
    arguments: Vec<String>,
    output: Output,
}

impl Run {
    fn status(&self) -> i32 {
        self.output.status.code().unwrap_or(-1)
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    /// Everything the program said, for asserting without caring which stream.
    fn said(&self) -> String {
        format!("{}{}", self.stdout(), self.stderr())
    }

    fn expect_status(&self, wanted: i32) -> &Self {
        assert_eq!(
            self.status(),
            wanted,
            "`roadie {}` exited {} rather than {wanted}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            self.arguments.join(" "),
            self.status(),
            self.stdout(),
            self.stderr()
        );
        self
    }

    /// Exit with one of the statuses this command documents.
    ///
    /// For commands whose result depends on what is plugged into the machine
    /// running the test.
    fn expect_status_in(&self, allowed: &[i32]) -> &Self {
        assert!(
            allowed.contains(&self.status()),
            "`roadie {}` exited {}, which is not one of {allowed:?}\n{}",
            self.arguments.join(" "),
            self.status(),
            self.said()
        );
        self
    }

    fn expect_says(&self, wanted: &str) -> &Self {
        assert!(
            self.said().contains(wanted),
            "`roadie {}` did not say {wanted:?}\n{}",
            self.arguments.join(" "),
            self.said()
        );
        self
    }

    fn expect_never_says(&self, unwanted: &str) -> &Self {
        assert!(
            !self.said().contains(unwanted),
            "`roadie {}` said {unwanted:?} and should not have\n{}",
            self.arguments.join(" "),
            self.said()
        );
        self
    }
}

fn write(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("a parent directory");
    }
    std::fs::write(path, body).expect("a file");
}

#[test]
fn the_binary_runs_and_describes_itself() {
    let sandbox = Sandbox::new("help");
    let run = sandbox.run(&["--help"]);
    run.expect_status(0);
    for command in [
        "devices",
        "doctor",
        "streamdeck",
        "tourbox",
        "via",
        "profile",
        "mcp",
    ] {
        run.expect_says(command);
    }
}

/// `doctor` is the command someone reaches for when nothing works, so it has
/// to work on a machine where nothing else does. It must never fail outright:
/// 0 means nothing to fix, 2 means it found something.
#[test]
fn doctor_reports_on_any_machine() {
    let sandbox = Sandbox::new("doctor");
    let run = sandbox.run(&["doctor"]);
    run.expect_status_in(&[0, 2]);
    run.expect_says("Permission to open devices");
    run.expect_says("Configuration");
    // A screen reader says "thing open paren s close paren".
    run.expect_never_says("(s)");
}

#[test]
fn devices_lists_or_explains_itself() {
    let sandbox = Sandbox::new("devices");
    let run = sandbox.run(&["devices"]);
    run.expect_status_in(&[0, 2]);
    if run.status() == 2 {
        // "Nothing found" has to lead somewhere.
        run.expect_says("roadie doctor");
    } else {
        run.expect_says("Configurable now");
        run.expect_says("attached device");
    }
}

#[test]
fn via_lists_or_explains_itself() {
    let sandbox = Sandbox::new("via");
    let run = sandbox.run(&["via"]);
    run.expect_status_in(&[0, 2]);
    if run.status() == 2 {
        run.expect_says("VIA");
        run.expect_says("roadie doctor");
    }
}

/// Everything this project's own commands print has to be worth listening to.
///
/// Scoped to the commands this project authors, which is the same scoping the
/// accessibility document applies to its merge-blocking rules: it is a
/// standard we hold our own work to, not a veto on inherited code.
///
/// Deliberately a sweep rather than an assertion per command. A rule checked
/// only where someone remembered to check it is a rule that holds until the
/// next command is added.
#[test]
fn nothing_this_project_prints_is_hostile_to_a_screen_reader() {
    let sandbox = Sandbox::new("listenable");
    let file = sandbox.path("setup.toml");
    sandbox.run(&["streamdeck", "example", "streaming"]);

    let invocations: &[&[&str]] = &[
        // `list` is inherited, and is also what `roadie` with no arguments
        // runs — so it is the first thing anyone hears. Its device tree used
        // to be drawn with box characters, which a screen reader reads one
        // character at a time.
        &["list"],
        &[],
        &["doctor"],
        &["doctor", "--json"],
        &["devices"],
        &["devices", "--supported"],
        &["devices", "--json"],
        &["streamdeck"],
        &["streamdeck", "layouts"],
        &["streamdeck", "example", "streaming"],
        &["tourbox"],
        &["tourbox", "list"],
        &["via"],
        &["via", "keymap", "0"],
        &["profile", "export", file.to_str().expect("utf-8")],
        &["profile", "inspect", file.to_str().expect("utf-8")],
        &["profile", "import", file.to_str().expect("utf-8")],
    ];

    for arguments in invocations {
        let run = sandbox.run(arguments);
        let what = format!("`roadie {}`", arguments.join(" "));
        // The one list, reached through the crate, rather than a copy kept
        // here. A copy is how the two come to disagree about what the rule is.
        roadie_cli::spoken::assert_listenable(&run.said(), &what);
        roadie_cli::spoken::assert_agrees(&run.said(), &what);
    }
}

/// Anything wrong has to be said in words, not only signalled by a colour or
/// a symbol. This checks the shape rather than the absence: `doctor` labels
/// every line, so every label is a word someone can hear.
#[test]
fn doctor_labels_every_line_with_a_word() {
    let sandbox = Sandbox::new("labels");
    let run = sandbox.run(&["doctor"]);
    run.expect_status_in(&[0, 2]);
    let stdout = run.stdout();
    // The check list is everything before the first blank line; what follows
    // is prose, headings and numbered steps.
    let checks: Vec<&str> = stdout
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .collect();
    assert!(!checks.is_empty(), "doctor printed no checks:\n{stdout}");
    for line in checks {
        assert!(
            ["OK ", "NOTE ", "FIX "]
                .iter()
                .any(|label| line.starts_with(label)),
            "a check with no word saying how it came out — a colour or a symbol \
             would leave this line meaningless to anyone not looking at it: {line:?}"
        );
    }
}

/// `--json` has to be *only* JSON. A stray human sentence on stdout — a
/// warning, a "nothing found" line — makes the output unparseable, and that is
/// the kind of thing that gets added later by someone being helpful.
#[test]
fn json_output_is_nothing_but_json() {
    let sandbox = Sandbox::new("json");
    for arguments in [
        ["devices", "--json"].as_slice(),
        ["doctor", "--json"].as_slice(),
    ] {
        let run = sandbox.run(arguments);
        run.expect_status_in(&[0, 2]);
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&run.stdout());
        assert!(
            parsed.is_ok(),
            "`roadie {}` did not print parseable JSON:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            arguments.join(" "),
            run.stdout(),
            run.stderr()
        );
    }
}

/// The exit status has to mean the same thing with and without `--json`, or a
/// script that adds the flag changes its own behaviour.
#[test]
fn asking_for_json_does_not_change_the_exit_status() {
    let sandbox = Sandbox::new("json-status");
    for (plain, json) in [
        (["devices"].as_slice(), ["devices", "--json"].as_slice()),
        (["doctor"].as_slice(), ["doctor", "--json"].as_slice()),
    ] {
        assert_eq!(
            sandbox.run(plain).status(),
            sandbox.run(json).status(),
            "`roadie {}` and its --json form disagree about the exit status",
            plain.join(" ")
        );
    }
}

/// The whole layout library flow, which needs no hardware at all: create,
/// list, and refuse to overwrite.
#[test]
fn a_layout_can_be_created_listed_and_not_clobbered() {
    let sandbox = Sandbox::new("layouts");

    sandbox
        .run(&["streamdeck", "layouts"])
        .expect_status(0)
        .expect_says("No layouts saved yet");

    sandbox
        .run(&["streamdeck", "example", "streaming"])
        .expect_status(0)
        .expect_says("streaming.toml");

    sandbox
        .run(&["streamdeck", "layouts"])
        .expect_status(0)
        .expect_says("streaming");

    // The deck's own memory is not a copy of the file, so an overwrite would
    // leave nothing to restore from.
    sandbox
        .run(&["streamdeck", "example", "streaming"])
        .expect_status(4)
        .expect_says("already exists");
}

/// A linked folder inside the layout library must not be followed.
///
/// Following one is unbounded whenever it points at or above itself, and the
/// recursion runs until the operating system refuses at around forty levels —
/// leaving forty levels of rubbish in a half-written bundle and an error
/// naming a path nobody can read. A person who symlinks their layouts into a
/// synced folder is doing something ordinary, not something perverse.
#[test]
fn a_linked_folder_in_the_library_is_reported_rather_than_followed() {
    let sandbox = Sandbox::new("symlinked-layouts");
    let library = sandbox.path("config/roadie/layouts");
    std::fs::create_dir_all(library.join("deck")).expect("a library");
    std::fs::write(library.join("deck.toml"), "brightness = 50\n\nkeys = []\n").expect("a layout");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&library, library.join("deck").join("loop"))
        .expect("a link pointing back at the library it sits inside");

    let bundle = sandbox.path("setup");
    let run = sandbox.run(&["profile", "export", &bundle.to_string_lossy()]);
    run.expect_status(0);

    #[cfg(unix)]
    {
        run.expect_says("not followed");
        // The layout itself still travelled; only the link did not.
        assert!(
            bundle.join("layouts").join("deck.toml").is_file(),
            "the layout beside the link must still be carried"
        );
        // And nothing recursed: no `loop` inside a `loop`.
        assert!(
            !bundle.join("layouts/deck/loop/deck/loop").exists(),
            "the link was followed"
        );
    }
}

/// Every command the device survey points at must exist and be documented.
///
/// `roadie devices` tells someone which command configures each thing on
/// their desk — "command: roadie light" beside a Litra. That is only useful
/// if the command is real and if there is something to read about it. Both
/// halves drifted once already: the survey named `roadie light` while
/// USAGE.md never mentioned it, so the listing sent people to a command the
/// guide did not admit existed.
///
/// The driver catalogue lives in another crate and the guide is a text file,
/// so nothing but this connects the three.
#[test]
fn every_command_the_survey_names_exists_and_is_documented() {
    let sandbox = Sandbox::new("survey-commands");
    let guide = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/USAGE.md"),
    )
    .expect("the usage guide is beside the crates");

    // Every driver, not a sample. This list stood at five while the enum had
    // grown to eight, so the three newest drivers were exempt from the check
    // that their command exists and is documented — which is precisely the
    // check a new driver most needs.
    let drivers = [
        roadie_catalog::Driver::HidPlusPlus,
        roadie_catalog::Driver::Litra,
        roadie_catalog::Driver::StreamDeck,
        roadie_catalog::Driver::Uvc,
        roadie_catalog::Driver::Via,
        roadie_catalog::Driver::Ddc,
        roadie_catalog::Driver::KeyLight,
        roadie_catalog::Driver::TourBox,
    ];

    for driver in drivers {
        let named = driver.command();
        let subcommand = named
            .strip_prefix("roadie ")
            .unwrap_or_else(|| panic!("{named} does not start with the program's name"));

        // It has to be a real subcommand: --help succeeds only if clap knows it.
        sandbox.run(&[subcommand, "--help"]).expect_status(0);

        // And the guide has to say something about it.
        assert!(
            guide.contains(named),
            "{named} is what `roadie devices` tells people to run for a \
             {driver:?} device, and USAGE.md never mentions it"
        );
    }
}

/// A layouts folder that is really a file must be said out loud, not read as
/// an empty library.
///
/// The two look identical from the outside — both find no layouts — and mean
/// opposite things: one is a machine that has saved nothing, the other is
/// broken. Saying "no layouts saved yet" about the second sends someone to
/// save one and watch that fail for a reason nothing mentioned.
#[test]
fn a_layouts_path_that_is_not_a_folder_is_reported_rather_than_read_as_empty() {
    let sandbox = Sandbox::new("layouts-not-a-folder");
    let layouts = sandbox.path("config/roadie/layouts");
    std::fs::create_dir_all(layouts.parent().expect("a parent")).expect("the config directory");
    std::fs::write(&layouts, "not a folder").expect("a file where the folder belongs");

    let listed = sandbox.run(&["streamdeck", "layouts"]);
    assert_ne!(listed.status(), 0, "it must not report success");
    assert!(
        !listed.said().contains("No layouts saved yet"),
        "a broken library must not read as an empty one: {}",
        listed.said()
    );

    // And an export must not claim to have carried a setup it could not read.
    let out = sandbox.path("out");
    let exported = sandbox.run(&["profile", "export", &out.to_string_lossy()]);
    assert_ne!(exported.status(), 0, "{}", exported.said());
    assert!(
        !exported.said().contains("no saved layouts to carry"),
        "that message is for a machine with none, not a broken folder: {}",
        exported.said()
    );
}

/// A bundle carrying an action that would run a program is refused, and the
/// refusal has to leave the layouts alone too.
///
/// The guard lives in control flow — layouts are restored only inside the
/// success arm — and control flow is exactly what a later refactor reorders
/// without noticing. A layout file is a thing people send each other, so a
/// refused import that had already written half of itself is the failure worth
/// spending a test on.
#[test]
fn a_refused_bundle_import_writes_neither_configuration_nor_layouts() {
    let sandbox = Sandbox::new("refused-bundle");
    let bundle = sandbox.path("bundle");
    let layouts = bundle.join("layouts");
    std::fs::create_dir_all(&layouts).expect("a bundle to import");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 6\n\
         \n\
         [devices]\n\
         \n\
         [keyboard.bindings.f13]\n\
         RunShellCommand = \"wipe\"\n",
    )
    .expect("a configuration carrying a risky action");
    std::fs::write(
        layouts.join("theirs.toml"),
        "brightness = 10\n\nkeys = []\n",
    )
    .expect("a layout in the bundle");

    sandbox
        .run(&["profile", "import", &bundle.to_string_lossy()])
        .expect_status(3)
        .expect_says("RunShellCommand");

    // Nothing from the bundle may have landed: not the configuration, and not
    // the layout that travelled beside it.
    let library = sandbox.path("config/roadie/layouts/theirs.toml");
    assert!(
        !library.exists(),
        "a refused import left a layout behind at {}",
        library.display()
    );
    let live = sandbox.path("config/roadie/config.toml");
    assert!(
        !live.exists(),
        "a refused import left a configuration behind at {}",
        live.display()
    );
}

/// A command this program prints is a command someone copies and runs.
///
/// A layout called "my deck" echoed bare gives `roadie streamdeck apply my
/// deck`, which the shell splits in two and the program then rejects — leaving
/// the person arguing with an instruction the program itself gave them. Worse
/// than no instruction, because they doubt themselves before they doubt it.
#[test]
fn a_printed_command_is_one_that_actually_runs() {
    let sandbox = Sandbox::new("printed-commands");

    let written = sandbox.run(&["streamdeck", "example", "my deck"]);
    written.expect_status(0);
    let said = written.said();
    let instruction = said
        .lines()
        .find(|line| line.contains("streamdeck apply"))
        .expect("the command to run next is printed");

    // Take the instruction apart the way a shell would, and run exactly that.
    let arguments = shell_split(instruction.split_once(": ").expect("a command").1);
    assert_eq!(arguments.first().map(String::as_str), Some("roadie"));
    let rest: Vec<&str> = arguments[1..].iter().map(String::as_str).collect();

    // Status 2 is "no Stream Deck attached", which is the honest answer on a
    // desk without one. Status 1 is a deck that is attached and would not
    // open — Elgato's own software holds a deck exclusively the whole time it
    // runs, and the first desk this ever ran on proved it. What must not
    // happen is the argument parser rejecting the printed command.
    let run = sandbox.run(&rest);
    run.expect_status_in(&[0, 1, 2]);
    if run.status() == 1 {
        // An exit of 1 is only honest here when it is the desk that answered,
        // not the command line: a deck that would not open, or several decks
        // with none chosen, which is refused rather than guessed at.
        let said = run.said();
        assert!(
            said.contains("failed to open the") || said.contains("This acts on one Stream Deck"),
            "an exit of 1 has to be the hardware's answer, not the parser's:\n{said}"
        );
    }
    assert!(
        !run.said().contains("unexpected argument"),
        "the printed command does not parse:\n{instruction}\n{}",
        run.said()
    );
}

/// Split a command line the way a POSIX shell does, for single quotes only —
/// which is all [`roadie_cli::spoken::shell_argument`] emits.
fn shell_split(line: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut any = false;
    for character in line.trim().chars() {
        match character {
            '\'' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                    any = false;
                }
            }
            c => current.push(c),
        }
    }
    if any || !current.is_empty() {
        arguments.push(current);
    }
    arguments
}

/// A label the key font cannot draw has to be said out loud, not left to the
/// key to reveal.
///
/// The key renders each unsupported character as a hollow box. That is visible
/// if you can see the deck, and says nothing at all if you cannot — while the
/// command reports plain success. So the only signal is the one signal this
/// project cannot rely on.
#[test]
fn a_label_the_font_cannot_draw_is_reported() {
    let sandbox = Sandbox::new("undrawable-label");
    let layout = sandbox.path("deck.toml").to_string_lossy().into_owned();

    // Katakana: perfectly reasonable to try, and not in a five-by-seven font.
    sandbox
        .run(&[
            "streamdeck",
            "set",
            &layout,
            "0",
            "--label",
            "\u{30df}\u{30e5}",
        ])
        .expect_status(0)
        .expect_says("cannot draw");

    // And a label it can draw says nothing extra, or the warning becomes noise
    // that gets tuned out.
    let clean = sandbox.run(&["streamdeck", "set", &layout, "1", "--label", "MUTE MIC"]);
    clean.expect_status(0);
    assert!(
        !clean.said().contains("cannot draw"),
        "an ordinary label must not draw a warning:\n{}",
        clean.said()
    );
}

/// A layout is a file someone owns, and editing one key of it must not throw
/// away the rest of what they wrote.
///
/// End-to-end rather than only in the unit tests because the failure it guards
/// is silent: the edit reports success, the layout still works, and the only
/// thing gone is the note the person left themselves. Nobody re-reads a file
/// that said it succeeded — least of all by ear.
#[test]
fn editing_a_key_keeps_the_comments_someone_wrote() {
    let sandbox = Sandbox::new("layout-comments");
    let layout = sandbox.path("deck.toml");
    std::fs::write(
        &layout,
        "# My streaming deck.\n\
         # Key 0 is the mic mute — do not move it, muscle memory.\n\
         brightness = 80\n\
         \n\
         [[keys]]\n\
         index = 0\n\
         label = \"MUTE MIC\"\n\
         background = \"802020\"\n",
    )
    .expect("a layout to edit");
    let path = layout.to_string_lossy().into_owned();

    sandbox
        .run(&["streamdeck", "set", &path, "1", "--label", "REC"])
        .expect_status(0);

    let after = std::fs::read_to_string(&layout).expect("the layout is still there");
    assert!(after.contains("# My streaming deck."), "{after}");
    assert!(after.contains("muscle memory"), "{after}");
    assert!(after.contains("brightness = 80"), "{after}");
    assert!(after.contains("REC"), "{after}");

    // Replacing a key still clears what it no longer carries: a key left with
    // both a label and a background nobody asked for is a key nobody chose.
    sandbox
        .run(&["streamdeck", "set", &path, "0", "--label", "MUTE"])
        .expect_status(0);
    let after = std::fs::read_to_string(&layout).expect("still there");
    assert!(
        !after.contains("802020"),
        "the old background survived: {after}"
    );
    assert!(after.contains("muscle memory"), "{after}");

    // And removing a key leaves the file's own header alone.
    sandbox
        .run(&["streamdeck", "unset", &path, "1"])
        .expect_status(0);
    let after = std::fs::read_to_string(&layout).expect("still there");
    assert!(after.contains("# My streaming deck."), "{after}");
    assert!(!after.contains("REC"), "{after}");
}

/// A whole layout built from the command line, never opening a text editor.
///
/// For anyone this is convenience; for someone working by dictation it is the
/// difference between the layout feature being usable and not, which is why it
/// gets an end-to-end test rather than only unit coverage of the pieces.
#[test]
fn a_layout_can_be_built_entirely_from_the_command_line() {
    let sandbox = Sandbox::new("nokeyboard");

    // No `example` first: the first key someone sets is the layout.
    sandbox
        .run(&["streamdeck", "set", "mydeck", "0", "--label", "MUTE MIC"])
        .expect_status(0)
        .expect_says("added");
    sandbox
        .run(&[
            "streamdeck",
            "set",
            "mydeck",
            "1",
            "--label",
            "REC",
            "--colour",
            "ff4040",
            "--action",
            "Copy",
        ])
        .expect_status(0);

    sandbox
        .run(&["streamdeck", "layouts"])
        .expect_status(0)
        .expect_says("mydeck");

    // Replacement, not merge.
    sandbox
        .run(&["streamdeck", "set", "mydeck", "0", "--label", "MUTE"])
        .expect_status(0)
        .expect_says("replaced");

    sandbox
        .run(&["streamdeck", "unset", "mydeck", "1"])
        .expect_status(0)
        .expect_says("removed");

    // Removing something that is not there must not claim to have changed
    // anything: told "removed", someone believes the deck changed.
    sandbox
        .run(&["streamdeck", "unset", "mydeck", "1"])
        .expect_status(2)
        .expect_says("nothing changed");

    // And what was built is a layout the program will actually read back.
    let path = sandbox.path("config/roadie/layouts/mydeck.toml");
    let body = std::fs::read_to_string(&path).expect("the layout exists");
    assert!(body.contains("MUTE"), "{body}");
    assert!(!body.contains("REC"), "the removed key is gone: {body}");
}

/// Mistakes have to be caught while the person is still thinking about that
/// key, not at apply time when the deck is in front of them.
#[test]
fn a_bad_key_setting_is_refused_at_the_moment_it_is_made() {
    let sandbox = Sandbox::new("badset");

    let both = sandbox.run(&[
        "streamdeck",
        "set",
        "d",
        "0",
        "--label",
        "X",
        "--image",
        "y.png",
    ]);
    assert_ne!(
        both.status(),
        0,
        "words or a picture, not both:\n{}",
        both.said()
    );
    both.expect_says("not both");

    let colour = sandbox.run(&[
        "streamdeck",
        "set",
        "d",
        "0",
        "--label",
        "X",
        "--colour",
        "zzz",
    ]);
    assert_ne!(colour.status(), 0, "{}", colour.said());
    colour.expect_says("--colour");

    let action = sandbox.run(&[
        "streamdeck",
        "set",
        "d",
        "0",
        "--label",
        "X",
        "--action",
        "Frobnicate",
    ]);
    assert_ne!(action.status(), 0, "{}", action.said());
    // A rejection that does not say what the vocabulary is leaves someone
    // guessing at names.
    action.expect_says("Copy");
}

/// A malformed layout must be reported as malformed, whether or not a deck is
/// attached. Reporting "no Stream Deck found" would send someone hunting a
/// hardware problem they do not have.
#[test]
fn a_broken_layout_is_named_as_broken_rather_than_blamed_on_the_hardware() {
    let sandbox = Sandbox::new("badlayout");
    let path = sandbox.path("broken.toml");
    write(&path, "brightness = \"not a number\"\n");

    let run = sandbox.run(&["streamdeck", "apply", path.to_str().expect("utf-8")]);
    assert_ne!(
        run.status(),
        0,
        "a broken layout cannot succeed:\n{}",
        run.said()
    );
    run.expect_says("broken.toml");
    run.expect_never_says("No Stream Deck found");
}

/// The headline promise, end to end through the real binary: save a whole
/// setup, lose it, and put it back.
#[test]
fn a_whole_setup_survives_being_exported_and_imported() {
    let sandbox = Sandbox::new("bundle");
    let bundle = sandbox.path("my-setup");

    sandbox
        .run(&["streamdeck", "example", "streaming"])
        .expect_status(0);
    // An icon beside the layout, to prove pictures travel with it.
    let icons = sandbox.path("config/roadie/layouts/streaming");
    write(&icons.join("camera.png"), "pretend this is a picture");

    sandbox
        .run(&["profile", "export", bundle.to_str().expect("utf-8")])
        .expect_status(0)
        .expect_says("configuration: config.toml")
        .expect_says("streaming");

    assert!(
        bundle.join("config.toml").is_file(),
        "a bundle holds the configuration"
    );
    assert!(bundle.join("layouts/streaming.toml").is_file());
    assert!(
        bundle.join("layouts/streaming/camera.png").is_file(),
        "an icon that does not travel makes a bundle that applies to blank keys"
    );

    // Lose the layout, the way moving to a new computer loses everything.
    std::fs::remove_dir_all(sandbox.path("config/roadie/layouts")).expect("lose the layouts");
    sandbox
        .run(&["streamdeck", "layouts"])
        .expect_status(0)
        .expect_says("No layouts saved yet");

    sandbox
        .run(&["profile", "import", bundle.to_str().expect("utf-8")])
        .expect_status(0)
        .expect_says("1 layout restored: streaming");

    sandbox
        .run(&["streamdeck", "layouts"])
        .expect_status(0)
        .expect_says("streaming");
    assert!(
        sandbox
            .path("config/roadie/layouts/streaming/camera.png")
            .is_file(),
        "the icon has to come back too"
    );
}

/// Exporting to a `.toml` path is the older, narrower thing, and the command
/// has to say so — someone who wanted their layouts should find out now
/// rather than on the machine they moved to.
#[test]
fn exporting_to_a_file_says_it_is_only_the_configuration() {
    let sandbox = Sandbox::new("file-export");
    let file = sandbox.path("just-config.toml");

    sandbox
        .run(&["profile", "export", file.to_str().expect("utf-8")])
        .expect_status(0)
        .expect_says("configuration written to")
        .expect_says("export to a folder instead");

    assert!(file.is_file());
    assert!(
        !sandbox.path("just-config").exists(),
        "a file, not a folder"
    );
}

/// The safety guard, through the real binary: a profile that would run a
/// program is refused, nothing is written, and the status says which kind of
/// failure it was.
#[test]
fn a_profile_that_would_run_a_program_is_refused_with_its_own_status() {
    let sandbox = Sandbox::new("untrusted");
    let file = sandbox.path("theirs.toml");
    sandbox
        .run(&["profile", "export", file.to_str().expect("utf-8")])
        .expect_status(0);

    let mut body = std::fs::read_to_string(&file).expect("the exported profile");
    body.push_str("\n[keyboard.bindings]\nF13 = { RunShellCommand = \"echo pwned\" }\n");
    write(&file, &body);

    let refused = sandbox.run(&["profile", "import", file.to_str().expect("utf-8")]);
    refused.expect_status(3);
    refused.expect_says("Nothing has been imported");
    refused.expect_says("echo pwned");

    // And accepting it is the human's explicit decision, which must work.
    sandbox
        .run(&[
            "profile",
            "import",
            file.to_str().expect("utf-8"),
            "--accept-actions",
        ])
        .expect_status(0)
        .expect_says("accepted");
}

/// `inspect` reports without applying. A tool that changed something while
/// claiming to only look would be the worst kind of surprise here.
#[test]
fn inspecting_a_profile_changes_nothing() {
    let sandbox = Sandbox::new("inspect");
    let file = sandbox.path("setup.toml");
    sandbox
        .run(&["profile", "export", file.to_str().expect("utf-8")])
        .expect_status(0);

    let live = sandbox.path("config/roadie/config.toml");
    let before = std::fs::read(&live).ok();

    sandbox
        .run(&["profile", "inspect", file.to_str().expect("utf-8")])
        .expect_status(0)
        .expect_says("schema version");

    assert_eq!(
        std::fs::read(&live).ok(),
        before,
        "inspect must not touch the live configuration"
    );
}

/// The MCP server, driven the way a client drives it. This proves the stdio
/// framing, the JSON-RPC dispatch and the tool catalog against the real
/// binary rather than against a function call.
#[test]
fn the_mcp_server_answers_over_stdio() {
    use std::io::Write as _;
    use std::process::Stdio;

    let sandbox = Sandbox::new("mcp");
    let mut child = Command::new(env!("CARGO_BIN_EXE_roadie"))
        .arg("mcp")
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .env("XDG_DATA_HOME", sandbox.path("data"))
        .env("XDG_STATE_HOME", sandbox.path("state"))
        .env("XDG_RUNTIME_DIR", sandbox.path("run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the mcp server starts");

    {
        let stdin = child.stdin.as_mut().expect("a pipe");
        for line in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        ] {
            writeln!(stdin, "{line}").expect("the server accepts a request");
        }
    }

    let output = child.wait_with_output().expect("the server exits on EOF");
    let answered = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = answered.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 2, "one answer per request:\n{answered}");

    // Every tool the survey and the drivers expose has to reach a client.
    for tool in [
        "list_peripherals",
        "diagnose",
        "list_layouts",
        "apply_layout",
        "list_keyboards",
        "set_key",
        "list_stream_decks",
        "export_profile",
    ] {
        assert!(
            lines[1].contains(tool),
            "the catalog does not offer {tool}:\n{}",
            lines[1]
        );
    }
}

/// A layout name arriving over MCP must not be able to name a path.
///
/// The argument comes from a language model, and a model can be steered by
/// whatever it has been reading — a web page, a document, a comment on a pull
/// request. Before this was closed, `set_layout_key` with a name of
/// `../../..` wrote a TOML file wherever it pointed, truncating whatever was
/// there. The command line still takes paths, because a person who types one
/// means it; this surface does not.
#[test]
fn an_mcp_layout_name_cannot_reach_outside_the_library() {
    use std::io::Write as _;
    use std::process::Stdio;

    let sandbox = Sandbox::new("mcp-escape");
    let target = sandbox.path("must-not-be-touched.txt");
    std::fs::write(&target, "the original contents").expect("a file to try to clobber");

    let mut child = Command::new(env!("CARGO_BIN_EXE_roadie"))
        .arg("mcp")
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .env("XDG_DATA_HOME", sandbox.path("data"))
        .env("XDG_STATE_HOME", sandbox.path("state"))
        .env("XDG_RUNTIME_DIR", sandbox.path("run"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the mcp server starts");

    let escapes = [
        target.with_extension("").to_string_lossy().into_owned(),
        "../../../../etc/roadie-should-never-write-this".to_owned(),
        "..".to_owned(),
        "sub/deck".to_owned(),
    ];
    {
        let stdin = child.stdin.as_mut().expect("a pipe");
        for (id, escape) in escapes.iter().enumerate() {
            let request = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id + 1,
                "method": "tools/call",
                "params": {
                    "name": "set_layout_key",
                    "arguments": { "layout": escape, "key": 0, "label": "X" },
                },
            });
            writeln!(stdin, "{request}").expect("the server accepts a request");
        }
    }

    let output = child.wait_with_output().expect("the server exits on EOF");
    let answered = String::from_utf8_lossy(&output.stdout);
    for line in answered.lines().filter(|line| !line.is_empty()) {
        assert!(
            line.contains("is not a layout name"),
            "an escape was not refused:\n{line}"
        );
    }

    assert_eq!(
        std::fs::read_to_string(&target).expect("the file is still there"),
        "the original contents",
        "a layout name reached outside the library and overwrote a file"
    );
    assert!(
        !sandbox.path("must-not-be-touched.toml").exists(),
        "a layout name reached outside the library"
    );
}

/// A malformed request must not take the server down: a client that sends one
/// bad frame should get an error and keep its session.
#[test]
fn the_mcp_server_survives_a_malformed_request() {
    use std::io::Write as _;
    use std::process::Stdio;

    let sandbox = Sandbox::new("mcp-bad");
    let mut child = Command::new(env!("CARGO_BIN_EXE_roadie"))
        .arg("mcp")
        .env("XDG_CONFIG_HOME", sandbox.path("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the mcp server starts");

    {
        let stdin = child.stdin.as_mut().expect("a pipe");
        writeln!(stdin, "this is not json at all").expect("write");
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).expect("write");
    }

    let output = child.wait_with_output().expect("the server exits on EOF");
    let answered = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = answered.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 2, "both frames answered:\n{answered}");
    assert!(lines[0].contains("-32700"), "a parse error:\n{}", lines[0]);
    assert!(
        lines[1].contains("list_peripherals"),
        "the session survived:\n{}",
        lines[1]
    );
}
