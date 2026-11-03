//! Editing a layout file without throwing away what the person wrote in it.
//!
//! A layout is a file someone owns. It carries their comments — "key 0 is the
//! mic mute, do not move it" — their blank lines, and the order they chose.
//! Reading it into a struct and serializing that struct back produces a
//! *correct* file that has silently lost all of it, and the person least able
//! to notice is the one this project is for: nothing on screen changes, and a
//! screen reader has no reason to re-read a file that reported success.
//!
//! So edits are applied to the document, not to a re-rendering of it. This is
//! the same choice `roadie-core` already made for `config.toml`, and for the
//! same reason; layouts were simply missed.
//!
//! Everything here is text in, text out, so what survives an edit is checked
//! against files nobody had to write by hand at the time.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value, value};

use super::{Key, LayoutError};

/// A byte-order mark, which some Windows editors put at the front of a file.
const BOM: &str = "\u{feff}";

/// Give an edited document the line endings and byte-order mark it arrived
/// with.
///
/// The parser accepts both kinds of line ending and hands back only the one,
/// so a file written on Windows came back with every line changed — which is
/// exactly what this module exists to prevent, and shows up as a whole-file
/// diff in the git repository the documentation suggests keeping layouts in.
///
/// The mark is put back for the same reason. It parsed with it, so removing it
/// is a change to someone's file made for no benefit to them.
fn keep_file_shape(source: &str, edited: String) -> String {
    let edited = if source.contains("\r\n") {
        // Flattened before being expanded, so a line that already ends CRLF
        // cannot come out as CRCRLF.
        edited.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        edited
    };
    if source.starts_with(BOM) && !edited.starts_with(BOM) {
        return format!("{BOM}{edited}");
    }
    edited
}

/// Parse a layout document, keeping its formatting.
fn document(source: &str) -> Result<DocumentMut, LayoutError> {
    source
        .parse::<DocumentMut>()
        .map_err(|error| LayoutError::Unwritable {
            detail: error.to_string(),
        })
}

/// Make sure `keys` is a form that can be edited, and say which it is.
///
/// TOML writes a list of tables two ways and both are valid layouts. A person
/// writes `[[keys]]` blocks; a serializer writes `keys = []` for an empty list
/// and `keys = [{ index = 0 }]` for a full one. Handling only the first meant
/// a layout that had been through a serializer — including one this program
/// wrote itself, and the empty-list shape used in its own fixtures — could not
/// be edited at all.
///
/// An empty inline array is promoted to `[[keys]]` blocks, which loses nothing
/// because there is nothing in it and gives the more readable form. A
/// non-empty one is edited where it is, because rewriting the style of a file
/// someone wrote is not this function's business.
fn prepare_keys(document: &mut DocumentMut) -> Result<KeysForm, LayoutError> {
    match document.get("keys") {
        None => {
            document["keys"] = Item::ArrayOfTables(ArrayOfTables::new());
            Ok(KeysForm::Blocks)
        }
        Some(Item::ArrayOfTables(_)) => Ok(KeysForm::Blocks),
        Some(Item::Value(Value::Array(array))) if array.is_empty() => {
            // Removed before being put back, so the new `[[keys]]` header does
            // not inherit the spacing that surrounded `keys = []`. Assigning
            // over it carries that decoration into the header and produces
            // `[[\nkeys ]]` — valid TOML, and alarming to read.
            document.remove("keys");
            document["keys"] = Item::ArrayOfTables(ArrayOfTables::new());
            Ok(KeysForm::Blocks)
        }
        Some(Item::Value(Value::Array(array)))
            if array.iter().all(|entry| entry.as_inline_table().is_some()) =>
        {
            Ok(KeysForm::Inline)
        }
        Some(_) => Err(LayoutError::Unwritable {
            detail: "`keys` is in this file but is not a list of keys".to_owned(),
        }),
    }
}

/// Which of TOML's two spellings of a list of tables this file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeysForm {
    /// `[[keys]]` blocks.
    Blocks,
    /// `keys = [{ ... }]`.
    Inline,
}

/// Which entry of `keys` holds `index`.
fn position_of(keys: &ArrayOfTables, index: u16) -> Option<usize> {
    keys.iter().position(|table| {
        table
            .get("index")
            .and_then(Item::as_integer)
            .is_some_and(|held| held == i64::from(index))
    })
}

/// Write `key` into a layout's text, returning the new text.
///
/// Only the fields that actually change are touched, so a comment sitting
/// beside a value the edit does not concern survives it.
///
/// # Errors
///
/// [`LayoutError::Unwritable`] if the source is not valid TOML, or has a
/// `keys` entry that is not a list of key tables.
pub fn set_key(source: &str, key: &Key) -> Result<String, LayoutError> {
    let mut document = document(source)?;
    if prepare_keys(&mut document)? == KeysForm::Inline {
        return Ok(keep_file_shape(source, set_key_inline(document, key)?));
    }
    let keys =
        document["keys"]
            .as_array_of_tables_mut()
            .ok_or_else(|| LayoutError::Unwritable {
                detail: "`keys` stopped being a list of keys while it was being edited".to_owned(),
            })?;
    if let Some(at) = position_of(keys, key.index) {
        let table = keys.get_mut(at).ok_or_else(|| LayoutError::Unwritable {
            detail: "the key vanished between finding it and editing it".to_owned(),
        })?;
        apply(table, key)?;
    } else {
        let mut table = Table::new();
        apply(&mut table, key)?;
        // Appended rather than sorted into index order. Sorting would move
        // whole blocks around the file, and a comment written above the first
        // `[[keys]]` — which is where a file's own header comment ends up —
        // would travel with whichever key sorted first. A file someone
        // arranged stays arranged; nothing downstream cares about the order.
        keys.push(table);
    }
    Ok(keep_file_shape(source, document.to_string()))
}

/// Remove `index` from a layout's text, reporting whether it was there.
///
/// # Errors
///
/// As [`set_key`].
pub fn remove_key(source: &str, index: u16) -> Result<(String, bool), LayoutError> {
    let mut document = document(source)?;
    if prepare_keys(&mut document)? == KeysForm::Inline {
        let (edited, removed) = remove_key_inline(document, index)?;
        return Ok((keep_file_shape(source, edited), removed));
    }
    let keys =
        document["keys"]
            .as_array_of_tables_mut()
            .ok_or_else(|| LayoutError::Unwritable {
                detail: "`keys` stopped being a list of keys while it was being edited".to_owned(),
            })?;
    let Some(at) = position_of(keys, index) else {
        return Ok((keep_file_shape(source, document.to_string()), false));
    };
    keys.remove(at);
    // An emptied array of tables would otherwise render as nothing at all,
    // which is fine, but leaving the empty `keys` key behind is tidier to
    // diff than having it appear and disappear as the last key comes and goes.
    Ok((keep_file_shape(source, document.to_string()), true))
}

/// [`set_key`] for a file that spells its keys inline.
///
/// The same edit, over `Value::InlineTable` rather than `Table`. Kept apart
/// rather than abstracted over the two: the shapes differ in enough small ways
/// that a shared version would be mostly branching, and this pair is short
/// enough to read.
fn set_key_inline(mut document: DocumentMut, key: &Key) -> Result<String, LayoutError> {
    let rendered = rendered_key(key)?;
    let array = document["keys"]
        .as_array_mut()
        .ok_or_else(|| LayoutError::Unwritable {
            detail: "`keys` stopped being a list of keys while it was being edited".to_owned(),
        })?;
    let at = array.iter().position(|entry| {
        entry
            .as_inline_table()
            .and_then(|table| table.get("index"))
            .and_then(Value::as_integer)
            .is_some_and(|held| held == i64::from(key.index))
    });
    match at {
        Some(at) => {
            array.replace(at, Value::InlineTable(rendered));
        }
        None => array.push(Value::InlineTable(rendered)),
    }
    Ok(document.to_string())
}

/// [`remove_key`] for a file that spells its keys inline.
fn remove_key_inline(mut document: DocumentMut, index: u16) -> Result<(String, bool), LayoutError> {
    let array = document["keys"]
        .as_array_mut()
        .ok_or_else(|| LayoutError::Unwritable {
            detail: "`keys` stopped being a list of keys while it was being edited".to_owned(),
        })?;
    let at = array.iter().position(|entry| {
        entry
            .as_inline_table()
            .and_then(|table| table.get("index"))
            .and_then(Value::as_integer)
            .is_some_and(|held| held == i64::from(index))
    });
    let Some(at) = at else {
        return Ok((document.to_string(), false));
    };
    array.remove(at);
    Ok((document.to_string(), true))
}

/// One key as an inline table, built through the serializer.
fn rendered_key(key: &Key) -> Result<toml_edit::InlineTable, LayoutError> {
    let mut table = Table::new();
    apply(&mut table, key)?;
    Ok(table.into_inline_table())
}

/// Write one key's fields into `table`, clearing the ones it no longer has.
///
/// Clearing matters: relabelling a key that used to show a picture has to take
/// the `image` line away, or the layout now says both and is refused the next
/// time it is applied — a failure that would arrive long after the edit that
/// caused it.
fn apply(table: &mut Table, key: &Key) -> Result<(), LayoutError> {
    table["index"] = value(i64::from(key.index));
    set_or_clear(table, "label", key.label.as_deref().map(value));
    set_or_clear(
        table,
        "image",
        key.image
            .as_deref()
            .map(|path| value(path.to_string_lossy().into_owned())),
    );
    set_or_clear(table, "colour", key.colour.as_deref().map(value));
    set_or_clear(table, "background", key.background.as_deref().map(value));
    set_or_clear(table, "action", action_item(key)?);
    Ok(())
}

/// Set a field, or remove it when the key does not carry one.
fn set_or_clear(table: &mut Table, field: &str, item: Option<Item>) {
    match item {
        Some(item) => table[field] = item,
        None => {
            table.remove(field);
        }
    }
}

/// An action rendered as the TOML it serializes to.
///
/// Round-tripped through the serializer rather than matched on by hand: the
/// action vocabulary is long and grows, and a hand-written renderer would fall
/// behind it silently — writing a valid-looking action that is not the one
/// asked for.
fn action_item(key: &Key) -> Result<Option<Item>, LayoutError> {
    let Some(action) = &key.action else {
        return Ok(None);
    };
    let rendered =
        toml::to_string(&ActionOnly { action }).map_err(|error| LayoutError::Unwritable {
            detail: error.to_string(),
        })?;
    let fragment = document(&rendered)?;
    fragment
        .get("action")
        .cloned()
        .map(Some)
        .ok_or_else(|| LayoutError::Unwritable {
            detail: "the action did not serialize to an `action` field".to_owned(),
        })
}

/// A shim so one action can be serialized on its own.
#[derive(serde::Serialize)]
struct ActionOnly<'a> {
    action: &'a roadie_core::binding::Action,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use roadie_core::binding::Action;

    use super::super::{Key, Layout};
    use super::{remove_key, set_key};

    fn key(index: u16, label: &str) -> Key {
        Key {
            index,
            label: Some(label.to_owned()),
            image: None,
            colour: None,
            background: None,
            action: None,
        }
    }

    fn parsed(source: &str) -> Layout {
        Layout::parse(std::path::Path::new("t.toml"), source).expect("the result is a layout")
    }

    /// The whole reason this module exists. A layout is a file someone owns,
    /// and the note they left themselves about which key is the mic mute is
    /// the part a re-serialization silently destroys — with nothing on screen
    /// to notice, which is worst for the person this project is for.
    #[test]
    fn a_comment_someone_wrote_survives_an_edit() {
        let source = "\
# My streaming deck.
# Key 0 is the mic mute — do not move it, muscle memory.
brightness = 80

[[keys]]
index = 0
label = \"MUTE MIC\"
";
        let edited = set_key(source, &key(1, "REC")).expect("edited");
        assert!(edited.contains("# My streaming deck."), "{edited}");
        assert!(edited.contains("muscle memory"), "{edited}");
        assert!(edited.contains("brightness = 80"), "{edited}");
    }

    #[test]
    fn setting_a_new_key_adds_it() {
        let edited = set_key("", &key(2, "REC")).expect("edited");
        let layout = parsed(&edited);
        assert_eq!(layout.keys.len(), 1);
        assert_eq!(layout.keys[0].index, 2);
        assert_eq!(layout.keys[0].label.as_deref(), Some("REC"));
    }

    /// Replacement, not merge. A command that quietly kept the old picture
    /// underneath new words would produce a key nobody asked for, and saying
    /// `--label` twice should give the second label and nothing else.
    #[test]
    fn setting_an_existing_key_replaces_it_rather_than_merging() {
        let source = "\
[[keys]]
index = 0
label = \"MUTE MIC\"
background = \"802020\"
";
        let edited = set_key(source, &key(0, "MUTE")).expect("edited");
        let layout = parsed(&edited);
        assert_eq!(layout.keys.len(), 1, "one key, not two");
        assert_eq!(layout.keys[0].label.as_deref(), Some("MUTE"));
        assert_eq!(
            layout.keys[0].background, None,
            "the old background must not survive under the new label"
        );
    }

    /// The specific way a merge would bite later: a key left carrying both a
    /// label and an image is refused when the layout is next applied, long
    /// after the edit that caused it.
    #[test]
    fn relabelling_a_picture_key_takes_the_picture_away() {
        let source = "\
[[keys]]
index = 0
image = \"icons/camera.png\"
";
        let edited = set_key(source, &key(0, "CAMERA")).expect("edited");
        assert!(!edited.contains("image"), "{edited}");
        let layout = parsed(&edited);
        assert_eq!(layout.keys[0].image, None);
        // The proof that it matters: a key with both is refused on apply.
        assert!(
            Layout::parse(std::path::Path::new("t.toml"), &edited).is_ok(),
            "and the file still parses"
        );
    }

    #[test]
    fn removing_a_key_reports_whether_it_was_there() {
        let source = "\
[[keys]]
index = 0
label = \"MUTE\"
";
        let (edited, removed) = remove_key(source, 0).expect("edited");
        assert!(removed, "the key was there");
        assert_eq!(parsed(&edited).keys.len(), 0);

        // Told "removed", someone believes something changed, and the next
        // thing they do is wonder why the deck looks the same.
        let (_, again) = remove_key(&edited, 0).expect("edited");
        assert!(!again, "it is no longer there");
    }

    #[test]
    fn removing_one_key_leaves_the_others_alone() {
        let source = "\
[[keys]]
index = 0
label = \"A\"

[[keys]]
index = 1
label = \"B\"
";
        let (edited, removed) = remove_key(source, 0).expect("edited");
        assert!(removed);
        let layout = parsed(&edited);
        assert_eq!(layout.keys.len(), 1);
        assert_eq!(layout.keys[0].label.as_deref(), Some("B"));
    }

    /// Removing a key must not take the comment belonging to another one.
    #[test]
    fn removing_a_key_leaves_another_keys_comment_behind() {
        let source = "\
[[keys]]
index = 0
label = \"A\"

# This one is the camera toggle.
[[keys]]
index = 1
label = \"B\"
";
        let (edited, _) = remove_key(source, 0).expect("edited");
        assert!(edited.contains("camera toggle"), "{edited}");
    }

    /// Every field has to survive being written and read back. Anything lost
    /// here is a setting someone made that the next edit silently discards.
    #[test]
    fn every_field_survives_being_written_and_read_back() {
        let full = Key {
            index: 0,
            label: Some("MUTE MIC".to_owned()),
            image: None,
            colour: Some("ffffff".to_owned()),
            background: Some("802020".to_owned()),
            action: Some(Action::Copy),
        };
        let picture = Key {
            index: 2,
            label: None,
            image: Some(PathBuf::from("icons/camera.png")),
            colour: None,
            background: None,
            action: None,
        };
        let edited = set_key("brightness = 80\n", &full).expect("edited");
        let edited = set_key(&edited, &picture).expect("edited again");

        let layout = parsed(&edited);
        assert_eq!(layout.brightness, Some(80));
        assert_eq!(layout.keys.len(), 2);
        assert!(layout.keys.contains(&full), "{edited}");
        assert!(layout.keys.contains(&picture), "{edited}");
    }

    /// An action that serializes as a table, not a bare string. Rendering it
    /// by hand is what this avoids: the action vocabulary grows, and a
    /// hand-written renderer falls behind it silently.
    #[test]
    fn an_action_with_a_payload_round_trips() {
        let mut key = key(3, "BUILD");
        key.action = Some(Action::RunShellCommand("make -C ~/project".to_owned()));
        let edited = set_key("", &key).expect("edited");
        let layout = parsed(&edited);
        assert_eq!(
            layout.keys[0].action,
            Some(Action::RunShellCommand("make -C ~/project".to_owned())),
            "{edited}"
        );
    }

    /// An edit is applied to whatever the file already says, so a file that
    /// is not a layout has to be refused rather than half-rewritten.
    #[test]
    fn a_file_that_is_not_toml_is_refused_rather_than_overwritten() {
        set_key("this is not toml {{{", &key(0, "X")).expect_err("not a layout");
        remove_key("this is not toml {{{", 0).expect_err("not a layout");
    }

    /// `keys` present but holding something else is a file we must not write
    /// a key into — it is someone's file and we do not know what it is.
    #[test]
    fn a_keys_field_that_is_not_a_list_of_keys_is_refused() {
        set_key("keys = 3\n", &key(0, "X")).expect_err("`keys` is not a list of keys");
    }

    /// Editing the same key twice must not accumulate anything.
    #[test]
    fn editing_the_same_key_twice_leaves_one_entry() {
        let once = set_key("", &key(0, "A")).expect("edited");
        let twice = set_key(&once, &key(0, "B")).expect("edited again");
        let layout = parsed(&twice);
        assert_eq!(layout.keys.len(), 1);
        assert_eq!(layout.keys[0].label.as_deref(), Some("B"));
    }

    /// TOML spells a list of tables two ways and both are valid layouts. A
    /// person writes `[[keys]]`; a serializer writes `keys = []` for an empty
    /// list. Handling only the first meant a layout that had been through a
    /// serializer — including the empty-list shape used in this project's own
    /// fixtures — could not be edited at all, and the error blamed the file.
    #[test]
    fn a_layout_written_as_an_empty_inline_list_can_still_be_edited() {
        let source = "# keep me\nbrightness = 80\n\nkeys = []\n";
        let edited = set_key(source, &key(0, "NEW")).expect("an empty list is a layout");
        assert!(edited.contains("# keep me"), "{edited}");
        assert!(edited.contains("[[keys]]"), "{edited}");
        // The promoted header must not inherit the spacing that surrounded
        // `keys = []`, which produces `[[\nkeys ]]` — valid, and alarming.
        assert!(!edited.contains("[[\n"), "{edited}");
        let layout = parsed(&edited);
        assert_eq!(layout.brightness, Some(80));
        assert_eq!(layout.keys[0].label.as_deref(), Some("NEW"));
    }

    /// A file that spells its keys inline is edited where it is, rather than
    /// rewritten into the other style. How someone laid their file out is not
    /// this code's business.
    #[test]
    fn a_layout_written_inline_is_edited_in_place() {
        let source = "# inline style\nkeys = [{ index = 0, label = \"OLD\" }]\n";
        let edited = set_key(source, &key(0, "NEW")).expect("inline is a layout too");
        assert!(edited.contains("# inline style"), "{edited}");
        assert!(
            edited.contains("keys = ["),
            "the inline style must survive: {edited}"
        );
        assert!(!edited.contains("[[keys]]"), "{edited}");
        let layout = parsed(&edited);
        assert_eq!(layout.keys.len(), 1, "replaced, not appended: {edited}");
        assert_eq!(layout.keys[0].label.as_deref(), Some("NEW"));
    }

    #[test]
    fn a_key_can_be_added_to_and_removed_from_an_inline_list() {
        let source = "keys = [{ index = 0, label = \"A\" }]\n";
        let edited = set_key(source, &key(1, "B")).expect("added");
        assert_eq!(parsed(&edited).keys.len(), 2, "{edited}");

        let (edited, removed) = remove_key(&edited, 0).expect("removed");
        assert!(removed);
        let layout = parsed(&edited);
        assert_eq!(layout.keys.len(), 1);
        assert_eq!(layout.keys[0].label.as_deref(), Some("B"));

        let (_, again) = remove_key(&edited, 0).expect("not there");
        assert!(!again, "removing what is gone reports so");
    }

    /// `keys` holding something that is neither shape is still refused: it is
    /// someone's file and we do not know what it is.
    #[test]
    fn a_keys_field_of_the_wrong_kind_is_still_refused() {
        set_key("keys = 3\n", &key(0, "X")).expect_err("a number is not a list of keys");
        set_key("keys = [1, 2]\n", &key(0, "X")).expect_err("a list of numbers is not either");
    }

    /// A layout written on Windows has CRLF line endings, and the parser
    /// accepts both kinds while handing back only one — so an edit came back
    /// with every line changed. That is exactly what this module exists to
    /// prevent, and it shows up as a whole-file diff in the git repository the
    /// documentation suggests keeping layouts in.
    #[test]
    fn a_file_written_on_windows_keeps_its_line_endings() {
        let source =
            "# windows file\r\nbrightness = 80\r\n\r\n[[keys]]\r\nindex = 0\r\nlabel = \"OLD\"\r\n";
        let edited = set_key(source, &key(1, "NEW")).expect("edited");
        assert!(edited.contains("# windows file\r\n"), "{edited:?}");
        assert!(
            !edited.contains("\r\r\n"),
            "a line that already ended CRLF must not come out CRCRLF: {edited:?}"
        );
        // The lines the edit added get the file's endings too, not the
        // serializer's.
        assert!(edited.contains("label = \"NEW\"\r\n"), "{edited:?}");
        assert!(
            !edited.replace("\r\n", "").contains('\n'),
            "a stray bare newline: {edited:?}"
        );
        // And it still reads back as the layout it is.
        assert_eq!(parsed(&edited).keys.len(), 2);
    }

    /// A file that uses plain newlines must not acquire carriage returns.
    #[test]
    fn a_file_written_anywhere_else_keeps_plain_newlines() {
        let edited = set_key("brightness = 80\n", &key(0, "X")).expect("edited");
        assert!(!edited.contains('\r'), "{edited:?}");
    }

    /// Some Windows editors put a byte-order mark at the front. The file
    /// parsed with it, so taking it away is a change made to someone's file
    /// for no benefit to them.
    #[test]
    fn a_byte_order_mark_is_left_where_it_was() {
        let source = "\u{feff}brightness = 80\n";
        let edited = set_key(source, &key(0, "X")).expect("edited");
        assert!(edited.starts_with('\u{feff}'), "{edited:?}");
        assert_eq!(
            edited.matches('\u{feff}').count(),
            1,
            "and only one: {edited:?}"
        );
    }

    /// Removing a key preserves the file's shape too, not only setting one.
    #[test]
    fn removing_a_key_keeps_the_files_line_endings() {
        let source = "[[keys]]\r\nindex = 0\r\nlabel = \"A\"\r\n\r\n[[keys]]\r\nindex = 1\r\nlabel = \"B\"\r\n";
        let (edited, removed) = remove_key(source, 0).expect("removed");
        assert!(removed);
        assert!(edited.contains("\r\n"), "{edited:?}");
        assert!(!edited.replace("\r\n", "").contains('\n'), "{edited:?}");
    }
}
