//! Choosing the one device a command acts on when several of a kind are in
//! reach.
//!
//! The rule is the one `roadie audio` set first: **ambiguity is an error, not
//! a guess.** Two Stream Decks, two macro pads or two interfaces share a
//! backend, and the OS lists them in an order that says nothing about which
//! was meant. Picking the first means a keymap written to the wrong board or
//! presses listened for on the wrong deck, and neither announces itself to
//! someone working by ear. So a command that acts on one device refuses when
//! more than one is in reach, names them all, and gives the `--device` to
//! type back.

use anyhow::{Result, anyhow};

use crate::spoken::{counted, shell_argument};

/// Something `--device` can pick out.
pub(crate) trait Choosable {
    /// How a person tells it apart from the others: model, then serial.
    fn spoken(&self) -> String;
    /// Every string a `--device` query may match part of, most specific
    /// last — the last one is what an example suggests typing.
    fn names(&self) -> Vec<&str>;
}

impl<T: Choosable> Choosable for &T {
    fn spoken(&self) -> String {
        (**self).spoken()
    }

    fn names(&self) -> Vec<&str> {
        (**self).names()
    }
}

/// What the devices are called, for the sentences below.
pub(crate) struct Kind {
    /// One of them: "Stream Deck".
    pub one: &'static str,
    /// Several: "Stream Decks".
    pub many: &'static str,
}

/// Whether `query` names a device described by `fields`: a case-insensitive
/// part of any one of them, so "xl" finds a Stream Deck XL and the last few
/// characters of a serial find that unit.
pub(crate) fn names(query: &str, fields: &[&str]) -> bool {
    let wanted = query.trim().to_lowercase();
    // `contains("")` is true of every string, so without this a stray
    // `--device " "` would quietly mean "all of them".
    !wanted.is_empty()
        && fields
            .iter()
            .any(|field| field.to_lowercase().contains(&wanted))
}

/// Keep what `--device` names, or refuse a query that names nothing.
///
/// No query keeps everything: narrowing is the caller's choice, and `list`
/// should still show every device.
///
/// # Errors
///
/// When a query matches none of `found`, saying what is attached instead.
pub(crate) fn narrow<T: Choosable>(
    found: Vec<T>,
    query: Option<&str>,
    kind: &Kind,
) -> Result<Vec<T>> {
    let Some(query) = query else {
        return Ok(found);
    };
    let attached = spoken_list(&found);
    let kept: Vec<T> = found
        .into_iter()
        .filter(|device| names(query, &device.names()))
        .collect();
    if kept.is_empty() {
        return Err(anyhow!(
            "No attached {} matches {query:?}. Attached: {attached}.",
            kind.one
        ));
    }
    Ok(kept)
}

/// The one device a command acts on.
///
/// # Errors
///
/// When there is none, or when there are several — the second names them all
/// and how to choose.
pub(crate) fn only<T: Choosable>(mut found: Vec<T>, kind: &Kind) -> Result<T> {
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(anyhow!("No {} is attached.", kind.one)),
        many => {
            let example = found
                .get(1)
                .map(|device| shell_argument(&shortest_unique(device, &found)))
                .unwrap_or_default();
            Err(anyhow!(
                "This acts on one {}, and {} are in reach: {}. Choose with --device and part \
                 of a name or serial number, for example: --device {example}",
                kind.one,
                counted(many, kind.one, kind.many),
                spoken_list(&found),
            ))
        }
    }
}

/// The shortest thing to type that picks `device` out of `found`.
///
/// A word from its names that no other device shares — "Mini", "XL" — because
/// the example is read aloud and typed back, and a fourteen-character serial
/// read letter by letter is the worst thing to hand someone to repeat. The
/// serial (the last name) only when no word is unique to it, as with two of
/// the same model.
fn shortest_unique<T: Choosable>(device: &T, found: &[T]) -> String {
    let others: Vec<Vec<&str>> = found
        .iter()
        .filter(|other| other.names() != device.names())
        .map(Choosable::names)
        .collect();
    let mut words: Vec<&str> = device
        .names()
        .into_iter()
        .flat_map(str::split_whitespace)
        .filter(|word| !others.iter().any(|names_of| names(word, names_of)))
        .collect();
    words.sort_by_key(|word| word.len());
    words.first().map_or_else(
        || {
            device
                .names()
                .into_iter()
                .rfind(|name| !name.is_empty())
                .unwrap_or_default()
                .to_owned()
        },
        |word| (*word).to_owned(),
    )
}

/// Each device once, in the order found. One Stream Deck can present several
/// HID collections, and a sentence naming it three times reads aloud as
/// three decks.
fn spoken_list<T: Choosable>(found: &[T]) -> String {
    let mut spoken: Vec<String> = Vec::new();
    for device in found {
        let said = device.spoken();
        if !spoken.contains(&said) {
            spoken.push(said);
        }
    }
    spoken.join("; ")
}

#[cfg(test)]
mod tests {
    use super::{Choosable, Kind, names, narrow, only};

    /// A device as this desk's two Stream Decks described themselves on
    /// Windows, 2026-11-03.
    #[derive(Debug, PartialEq)]
    struct Deck(&'static str, &'static str);

    impl Choosable for Deck {
        fn spoken(&self) -> String {
            format!("{}, serial {}", self.0, self.1)
        }

        fn names(&self) -> Vec<&str> {
            vec![self.0, self.1]
        }
    }

    const KIND: Kind = Kind {
        one: "Stream Deck",
        many: "Stream Decks",
    };

    fn desk() -> Vec<Deck> {
        vec![
            Deck("Stream Deck XL", "CL11L2A08430"),
            Deck("Stream Deck Mini", "A00DA43117P7VB"),
        ]
    }

    #[test]
    fn a_model_word_or_part_of_a_serial_names_one_device_whatever_its_case() {
        assert!(names("xl", &["Stream Deck XL", "CL11L2A08430"]));
        assert!(!names("xl", &["Stream Deck Mini", "A00DA43117P7VB"]));
        assert!(names("MINI", &["Stream Deck Mini", "A00DA43117P7VB"]));
        assert!(names("2a084", &["Stream Deck XL", "CL11L2A08430"]));
    }

    #[test]
    fn a_blank_query_names_nothing_rather_than_everything() {
        assert!(!names("  ", &["Stream Deck XL"]));
        assert!(!names("", &["Stream Deck Mini"]));
    }

    #[test]
    fn two_in_reach_is_refused_with_both_named_and_a_choice_to_type() {
        let error = only(desk(), &KIND).expect_err("two decks is ambiguous");
        assert_eq!(
            error.to_string(),
            "This acts on one Stream Deck, and 2 Stream Decks are in reach: Stream Deck XL, \
             serial CL11L2A08430; Stream Deck Mini, serial A00DA43117P7VB. Choose with \
             --device and part of a name or serial number, for example: --device Mini"
        );
    }

    #[test]
    fn two_of_the_same_model_are_told_apart_by_serial() {
        let twins = vec![
            Deck("Stream Deck XL", "CL11L2A08430"),
            Deck("Stream Deck XL", "CL22B9Z00001"),
        ];
        let error = only(twins, &KIND).expect_err("two decks is ambiguous");
        assert!(
            error
                .to_string()
                .ends_with("for example: --device CL22B9Z00001"),
            "{error}"
        );
    }

    #[test]
    fn a_query_narrows_to_the_device_it_names() {
        let kept = narrow(desk(), Some("mini"), &KIND).expect("one matches");
        assert_eq!(
            only(kept, &KIND).expect("one left"),
            Deck("Stream Deck Mini", "A00DA43117P7VB")
        );
    }

    #[test]
    fn a_query_both_share_still_leaves_the_choice_to_be_made() {
        let kept = narrow(desk(), Some("deck"), &KIND).expect("both match");
        assert_eq!(kept.len(), 2);
        only(kept, &KIND).expect_err("still ambiguous");
    }

    #[test]
    fn a_query_naming_nothing_says_what_is_attached() {
        let error = narrow(desk(), Some("pedal"), &KIND).expect_err("nothing matches");
        assert_eq!(
            error.to_string(),
            "No attached Stream Deck matches \"pedal\". Attached: Stream Deck XL, serial \
             CL11L2A08430; Stream Deck Mini, serial A00DA43117P7VB."
        );
    }

    #[test]
    fn a_device_seen_through_several_collections_is_named_once() {
        let collections = vec![
            Deck("Stream Deck XL", "CL11L2A08430"),
            Deck("Stream Deck XL", "CL11L2A08430"),
        ];
        let error = narrow(collections, Some("pedal"), &KIND).expect_err("nothing matches");
        assert_eq!(
            error.to_string(),
            "No attached Stream Deck matches \"pedal\". Attached: Stream Deck XL, serial \
             CL11L2A08430."
        );
    }

    #[test]
    fn no_query_keeps_every_device() {
        assert_eq!(narrow(desk(), None, &KIND).expect("no query").len(), 2);
    }
}
