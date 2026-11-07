//! `roadie via` — QMK keyboards and macro pads.
//!
//! One implementation reaches the hundreds of boards running QMK with VIA
//! enabled, which is why this is worth more per line than any single-vendor
//! driver. The commands are deliberately few and deliberately careful: a wrong
//! keycode takes a key away from whoever is using the board, and the tool that
//! did it is then the tool they have to use to fix it.

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use roadie_hid::via::{self, Attached, Session};
use roadie_via::keycode;

use crate::choose;
use crate::spoken::counted;

/// Exit status for "the scan succeeded, but no VIA device is attached".
const NOTHING_FOUND: u8 = 2;

#[derive(Debug, Args)]
pub struct ViaArgs {
    /// Case-insensitive part of the board's name or serial number, as
    /// `roadie via list` says it. Not needed when only one is attached.
    #[arg(long, global = true, value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub device: Option<String>,
    #[command(subcommand)]
    pub cmd: Option<ViaCmd>,
}

#[derive(Debug, Subcommand)]
pub enum ViaCmd {
    /// List attached VIA devices and what each reports about itself.
    List,
    /// Print a whole keymap layer, key by key.
    Keymap(KeymapArgs),
    /// Read the keycode at one position.
    Get(PositionArgs),
    /// Write a keycode to one position, then read it back to confirm.
    Set(SetArgs),
}

#[derive(Debug, Args)]
pub struct KeymapArgs {
    /// Which layer, counting from 0.
    #[arg(default_value_t = 0)]
    pub layer: u8,
    /// How many matrix rows to read.
    #[arg(long, default_value_t = 6)]
    pub rows: u8,
    /// How many matrix columns to read.
    #[arg(long, default_value_t = 16)]
    pub columns: u8,
}

#[derive(Debug, Args)]
pub struct PositionArgs {
    /// Which layer, counting from 0.
    pub layer: u8,
    /// Matrix row, counting from 0.
    pub row: u8,
    /// Matrix column, counting from 0.
    pub column: u8,
}

#[derive(Debug, Args)]
pub struct SetArgs {
    /// Which layer, counting from 0.
    pub layer: u8,
    /// Matrix row, counting from 0.
    pub row: u8,
    /// Matrix column, counting from 0.
    pub column: u8,
    /// The key to assign: a name like `F13` or `KC_F13`, or a number.
    pub keycode: String,
}

impl ViaCmd {
    /// Run the subcommand.
    ///
    /// # Errors
    ///
    /// Fails when HID enumeration fails, when the device does not answer, or
    /// when a keycode name cannot be resolved.
    pub async fn run(self, device: Option<&str>) -> Result<ExitCode> {
        // Resolved before the device is opened: a name we cannot resolve is
        // wrong whether or not a keyboard is attached, and "no VIA device
        // found" would send someone hunting the wrong problem.
        let wanted = match &self {
            Self::Set(args) => Some(resolve(&args.keycode)?),
            _ => None,
        };

        let attached = via::attached()
            .await
            .context("failed to enumerate HID devices")?;
        if attached.is_empty() {
            println!("No VIA device found.");
            println!();
            println!(
                "A QMK board only answers here if its firmware was built with VIA \
                 enabled. If yours has it, the next thing to check is whether this \
                 process can open raw HID devices at all — `roadie doctor` answers \
                 that and says what to do about it."
            );
            return Ok(ExitCode::from(NOTHING_FOUND));
        }
        let attached = choose::narrow(attached, device, &BOARDS)?;

        match self {
            Self::List => list(&attached).await,
            Self::Keymap(args) => keymap(&attached, &args).await,
            Self::Get(args) => get(&attached, &args).await,
            Self::Set(args) => set(&attached, &args, wanted.unwrap_or_default()).await,
        }
    }
}

/// Turn a keycode argument into a number.
///
/// Accepts a name (`F13`, `KC_F13`, case-insensitive), hex (`0x0068`) or plain
/// decimal. Names are what people say and remember; the numeric forms are what
/// firmware references print, and refusing either would make someone translate
/// by hand between two things that mean the same key.
///
/// A name wins over the same text read as a number, which matters for the
/// digits: `1` is the "1" key, not keycode 1. Someone typing it means the key
/// they can see on their board, and the numeric reading would give them a
/// pass-through — a key that appears to do nothing.
fn resolve(argument: &str) -> Result<u16> {
    let text = argument.trim();
    if let Some(keycode) = keycode::parse(text) {
        return Ok(keycode);
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u16::from_str_radix(hex, 16).with_context(|| {
            format!("{argument} is not a keycode this build knows, nor a number")
        });
    }
    text.parse::<u16>()
        .with_context(|| format!("{argument} is not a keycode this build knows, nor a number"))
}

/// Open the one board in reach.
///
/// `attached` is already narrowed by `--device`. More than one left is
/// refused rather than resolved by enumeration order, because a keycode
/// written to the wrong board takes a key away from whoever is using it (see
/// [`crate::choose`]).
async fn open_one(attached: &[Attached]) -> Result<Session> {
    let board = choose::only(attached.iter().collect(), &BOARDS)?;
    Ok(Session::open(board).await?)
}

/// What `--device` and its refusals call these.
const BOARDS: choose::Kind = choose::Kind {
    one: "VIA board",
    many: "VIA boards",
};

impl choose::Choosable for Attached {
    fn spoken(&self) -> String {
        match &self.serial_number {
            Some(serial) => format!("{}, serial {serial}", self.name),
            None => self.name.clone(),
        }
    }

    fn names(&self) -> Vec<&str> {
        vec![
            &self.name,
            self.serial_number.as_deref().unwrap_or_default(),
        ]
    }
}

/// `roadie via list`.
async fn list(attached: &[Attached]) -> Result<ExitCode> {
    println!("VIA devices ({}):", attached.len());
    for device in attached {
        println!(
            "  {} ({:04x}:{:04x})",
            device.name, device.vendor_id, device.product_id
        );
        if let Some(serial) = &device.serial_number {
            println!("    serial: {serial}");
        }
        // Opening is what turns a usage-page match into a fact. Reported
        // rather than fatal: one unresponsive board should not hide the rest.
        match Session::open(device).await {
            Ok(session) => println!(
                "    speaks VIA protocol {}, with {}",
                session.protocol(),
                counted(session.layers().into(), "keymap layer", "keymap layers")
            ),
            Err(error) => println!("    did not answer as a VIA device: {error}"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `roadie via keymap`.
async fn keymap(attached: &[Attached], args: &KeymapArgs) -> Result<ExitCode> {
    let (rows, rows_capped) = via::scan_edge(args.rows);
    let (columns, columns_capped) = via::scan_edge(args.columns);
    if rows_capped || columns_capped {
        // Said before the read, not after: it takes a moment per position and
        // someone who asked for more should know now why they are not getting
        // it, rather than after waiting.
        println!(
            "Reading {rows} rows by {columns} columns; more than that was asked for and \
             every real keyboard matrix is smaller."
        );
    }
    let mut session = open_one(attached).await?;
    println!(
        "Layer {} of {} (rows 0 to {}, columns 0 to {}):",
        args.layer,
        session.layers(),
        rows.saturating_sub(1),
        columns.saturating_sub(1)
    );
    let mut quiet = 0_u32;
    let mut shown = 0_u32;
    for row in 0..rows {
        for column in 0..columns {
            let code = session.keycode(args.layer, row, column).await?;
            // Unassigned and pass-through positions are the great majority of
            // a matrix read blind — on any layer above the first, nearly all
            // of it — and printing them would bury the keys that exist. They
            // are counted rather than dropped, so nothing goes missing
            // silently.
            if code == keycode::NONE || code == keycode::TRANSPARENT {
                quiet += 1;
                continue;
            }
            shown += 1;
            println!("  row {row}, column {column}: {}", keycode::describe(code));
        }
    }
    if shown == 0 {
        println!("  (nothing assigned on this layer)");
    }
    println!();
    println!("{shown} assigned, {quiet} unassigned or passed through to the layer below.");
    if args.layer == 0 && shown == 0 {
        println!(
            "Layer 0 with nothing on it usually means the matrix is larger than the \
             {rows} rows by {columns} columns read here; pass --rows and --columns to \
             widen it."
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `roadie via get`.
async fn get(attached: &[Attached], args: &PositionArgs) -> Result<ExitCode> {
    let mut session = open_one(attached).await?;
    let code = session.keycode(args.layer, args.row, args.column).await?;
    println!(
        "layer {}, row {}, column {}: {}",
        args.layer,
        args.row,
        args.column,
        keycode::describe(code)
    );
    Ok(ExitCode::SUCCESS)
}

/// `roadie via set`.
async fn set(attached: &[Attached], args: &SetArgs, keycode_value: u16) -> Result<ExitCode> {
    let mut session = open_one(attached).await?;
    let was = session.keycode(args.layer, args.row, args.column).await?;
    session
        .set_keycode(args.layer, args.row, args.column, keycode_value)
        .await?;
    println!(
        "layer {}, row {}, column {}: {} -> {}",
        args.layer,
        args.row,
        args.column,
        keycode::describe(was),
        keycode::describe(keycode_value)
    );
    println!("Confirmed by reading the position back.");
    // Said every time, not only when it looks risky: someone who has just
    // changed a key needs to know how to change it back, and the moment they
    // need that is after they have closed the terminal.
    println!(
        "To undo: roadie via set {} {} {} {}",
        args.layer,
        args.row,
        args.column,
        keycode::describe(was)
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use roadie_via::keycode;

    use super::resolve;

    /// Names are what people say and remember.
    #[test]
    fn a_name_resolves_however_it_is_spelled() {
        for spelling in ["F13", "f13", "KC_F13"] {
            assert_eq!(
                resolve(spelling).expect("a known key"),
                0x0068,
                "{spelling}"
            );
        }
    }

    /// Firmware references print hex; refusing it would make someone
    /// translate by hand between two spellings of the same key.
    #[test]
    fn hex_and_decimal_both_resolve() {
        assert_eq!(resolve("0x0068").expect("hex"), 0x0068);
        assert_eq!(resolve("0X0068").expect("upper-case hex"), 0x0068);
        assert_eq!(resolve("104").expect("decimal"), 104);
    }

    /// A subtle one worth pinning: `1` is the name of the "1" key, not the
    /// number 1. Someone typing `set 0 0 0 1` means the key they can see on
    /// their board, and a numeric reading would make it a pass-through
    /// instead — a key that appears to do nothing.
    #[test]
    fn a_name_wins_over_the_same_text_read_as_a_number() {
        assert_eq!(resolve("1").expect("the 1 key"), 0x001e);
        assert_eq!(resolve("0").expect("the 0 key"), 0x0027);
        // Anything with no name still reads as a number.
        assert_eq!(resolve("200").expect("a number"), 200);
    }

    #[test]
    fn something_that_is_neither_is_refused_with_a_message_that_says_so() {
        let error = resolve("SUPERKEY").expect_err("not a key");
        let text = format!("{error}");
        assert!(text.contains("SUPERKEY"), "{text}");
        assert!(text.contains("nor a number"), "{text}");
    }

    /// The undo line the `set` command prints has to be a command that runs.
    /// Printing a name the parser would then reject would be worse than
    /// printing nothing, because it looks like a way back.
    #[test]
    fn every_name_the_undo_line_could_print_resolves_again() {
        for code in [0x0000_u16, 0x0004, 0x0045, 0x0068, 0x00e0, 0x00ff] {
            let printed = keycode::describe(code);
            assert_eq!(
                resolve(&printed).expect("the undo line must be runnable"),
                code,
                "{printed} did not round trip"
            );
        }
    }
}
