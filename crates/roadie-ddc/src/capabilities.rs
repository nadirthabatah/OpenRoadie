//! The capability string: what a monitor says it can do.
//!
//! Ask a monitor for its capabilities and it answers with one long
//! parenthesised string, delivered in fragments:
//!
//! ```text
//! (prot(monitor)type(lcd)model(U2723QE)cmds(01 02 03 07 0C E3 F3)
//!  vcp(02 04 10 12 14(01 05 08 0B) 60(0F 10 11 1B) D6(01 04 05) DF)
//!  mccs_ver(2.1))
//! ```
//!
//! That is the difference between guessing and knowing. Without it a host has
//! to probe features one at a time and interpret silence; with it, the monitor
//! has already listed its features and — for discrete ones like input source —
//! the exact values it accepts. The `1B` in the `60(...)` list above is how a
//! host learns that *this* panel calls its USB-C port `0x1B`, which no
//! specification would have told it.
//!
//! # Why this parser forgives
//!
//! Capability strings are famously badly formed. Monitors ship them with
//! unbalanced parentheses, missing outer parentheses, stray whitespace inside
//! hex tokens, and segments that were clearly meant to be something else. A
//! strict parser rejects working monitors, which is the wrong trade for a tool
//! whose job is to control the hardware someone already owns.
//!
//! So this one recovers wherever the meaning survives, and records what it had
//! to work around in [`Capabilities::warnings`] rather than swallowing it. A
//! caller that wants to be strict can check that list; `roadie doctor` prints
//! it, which turns a vague "this monitor is weird" into a specific sentence.
//! Parsing fails only when there is nothing at all to salvage.

use crate::vcp::{Feature, InputSource};

/// A parsed capability string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// The `model(...)` segment, if the monitor sent one.
    pub model: Option<String>,
    /// The `type(...)` segment — `lcd`, `crt`, and so on.
    pub display_type: Option<String>,
    /// The `mccs_ver(...)` segment.
    pub mccs_version: Option<String>,
    /// The opcodes from `cmds(...)`: which DDC commands the monitor answers,
    /// as opposed to which features it has.
    pub commands: Vec<u8>,
    /// Every feature from `vcp(...)`, in the order the monitor listed them.
    pub features: Vec<FeatureEntry>,
    /// What the parser had to work around. Empty for a well-formed string.
    pub warnings: Vec<String>,
}

/// One entry from the `vcp(...)` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureEntry {
    /// The feature.
    pub feature: Feature,
    /// The values the monitor listed for it.
    ///
    /// Empty for a continuous feature like brightness, where the range comes
    /// from reading the feature rather than from this list. Non-empty for a
    /// discrete one, and then it is authoritative: these are the only values
    /// worth offering, whatever any specification says.
    pub values: Vec<u8>,
}

impl Capabilities {
    /// Parse a capability string.
    ///
    /// # Errors
    ///
    /// Returns [`CapabilitiesError`] only when the input is empty, is not
    /// text, or contains no recognisable segment at all. Anything short of
    /// that is recovered and recorded in [`Self::warnings`].
    pub fn parse(bytes: &[u8]) -> Result<Self, CapabilitiesError> {
        let text = core::str::from_utf8(bytes).map_err(|_| CapabilitiesError::NotText)?;
        Self::parse_str(text)
    }

    /// Parse a capability string that is already text.
    ///
    /// # Errors
    ///
    /// As [`Self::parse`].
    pub fn parse_str(text: &str) -> Result<Self, CapabilitiesError> {
        let trimmed = text.trim().trim_end_matches('\0').trim();
        if trimmed.is_empty() {
            return Err(CapabilitiesError::Empty);
        }
        let mut caps = Self::default();
        // Some monitors wrap the whole string in parentheses and some do not,
        // so peel one layer if it wraps everything rather than requiring it.
        let body = strip_outer_parens(trimmed).unwrap_or(trimmed);

        let mut found_any = false;
        for (key, value) in segments(body, &mut caps.warnings) {
            found_any = true;
            match key {
                "model" => caps.model = Some(clean(value)),
                "type" => caps.display_type = Some(clean(value)),
                "mccs_ver" => caps.mccs_version = Some(clean(value)),
                "cmds" => caps.commands = hex_list(value, "cmds", &mut caps.warnings),
                "vcp" => caps.features = parse_vcp(value, &mut caps.warnings),
                // prot, mswhql, asset_eep, vcpname and vendor inventions all
                // land here. Not warned about: an unrecognised segment is
                // normal, not a defect.
                _ => {}
            }
        }
        if !found_any {
            return Err(CapabilitiesError::NoSegments);
        }
        Ok(caps)
    }

    /// Whether the monitor listed this feature.
    #[must_use]
    pub fn supports(&self, feature: Feature) -> bool {
        self.entry(feature).is_some()
    }

    /// The entry for a feature, if the monitor listed it.
    #[must_use]
    pub fn entry(&self, feature: Feature) -> Option<&FeatureEntry> {
        self.features.iter().find(|entry| entry.feature == feature)
    }

    /// The inputs this monitor will actually switch to.
    ///
    /// Drawn from the monitor's own `60(...)` list, which is the only source
    /// that knows about vendor values — a USB-C port that no standard names.
    /// Empty when the monitor listed input source without enumerating values,
    /// which some do; then the standard table is the best a caller has.
    #[must_use]
    pub fn inputs(&self) -> Vec<InputSource> {
        self.entry(Feature::InputSource).map_or_default(|entry| {
            entry
                .values
                .iter()
                .copied()
                .map(InputSource::from_code)
                .collect()
        })
    }
}

/// Trim a segment's text of whitespace and of the NUL padding monitors use.
///
/// `str::trim` does not touch NUL — it is not whitespace — so a monitor that
/// pads a short read leaves them inside the value, where they reach a screen
/// reader as silence in the middle of a model name and a terminal as nothing
/// visible at all. Neither is debuggable by looking at it.
fn clean(value: &str) -> String {
    value
        .trim_matches(|ch: char| ch.is_whitespace() || ch == '\0')
        .to_owned()
}

/// Peel one pair of parentheses if they wrap the entire string.
fn strip_outer_parens(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('(')?.strip_suffix(')')?;
    // Only peel when that first parenthesis is closed by that last one; a
    // string like `(a)(b)` is two segments, not one wrapped in parentheses.
    let mut depth = 1_usize;
    for ch in inner.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    (depth == 1).then_some(inner)
}

/// Split a capability body into its top-level `key(value)` pairs.
fn segments<'a>(body: &'a str, warnings: &mut Vec<String>) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        // Skip anything that cannot start a key.
        if !bytes[index].is_ascii_alphanumeric() && bytes[index] != b'_' {
            index += 1;
            continue;
        }
        let key_start = index;
        while index < bytes.len() && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
        {
            index += 1;
        }
        let key = &body[key_start..index];
        if index >= bytes.len() || bytes[index] != b'(' {
            warnings.push(format!(
                "ignored `{key}`, which has no value in parentheses"
            ));
            continue;
        }
        index += 1;
        let value_start = index;
        let mut depth = 1_usize;
        while index < bytes.len() && depth > 0 {
            match bytes[index] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            index += 1;
        }
        if depth > 0 {
            // The monitor ran out of string before closing this segment. Take
            // what arrived: a truncated `vcp(...)` list is still most of a
            // feature list, and dropping it would lose the whole monitor.
            warnings.push(format!("`{key}` was never closed; used what arrived"));
            out.push((key, &body[value_start..]));
            break;
        }
        out.push((key, &body[value_start..index - 1]));
    }
    out
}

/// Parse a whitespace-separated list of hex bytes.
fn hex_list(text: &str, what: &str, warnings: &mut Vec<String>) -> Vec<u8> {
    let mut out = Vec::new();
    for token in text.split_ascii_whitespace() {
        match u8::from_str_radix(token, 16) {
            Ok(byte) => out.push(byte),
            Err(_) => warnings.push(format!(
                "ignored `{token}` in `{what}`, which is not a hex byte"
            )),
        }
    }
    out
}

/// Parse the `vcp(...)` list: hex codes, each optionally followed by its own
/// parenthesised list of accepted values.
fn parse_vcp(text: &str, warnings: &mut Vec<String>) -> Vec<FeatureEntry> {
    let mut out: Vec<FeatureEntry> = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if bytes[index] == b'(' {
            // A value list with no code in front of it. Attach it to the
            // previous code if there is one — some monitors put a space
            // between a code and its list — and drop it otherwise.
            let (values, next) = take_parenthesised(text, index);
            index = next;
            match out.last_mut() {
                Some(entry) => entry.values.extend(hex_list(values, "vcp", warnings)),
                None => warnings.push("ignored a value list with no feature before it".to_owned()),
            }
            continue;
        }
        let start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'('
            && bytes[index] != b')'
        {
            index += 1;
        }
        let token = &text[start..index];
        let Ok(code) = u8::from_str_radix(token, 16) else {
            warnings.push(format!(
                "ignored `{token}` in `vcp`, which is not a hex byte"
            ));
            continue;
        };
        let mut values = Vec::new();
        if index < bytes.len() && bytes[index] == b'(' {
            let (inner, next) = take_parenthesised(text, index);
            index = next;
            values = hex_list(inner, "vcp", warnings);
        }
        out.push(FeatureEntry {
            feature: Feature::from_code(code),
            values,
        });
    }
    out
}

/// Take the contents of the parenthesised group starting at `open`, and the
/// index just past it. An unclosed group runs to the end of the text.
fn take_parenthesised(text: &str, open: usize) -> (&str, usize) {
    let bytes = text.as_bytes();
    let mut depth = 1_usize;
    let mut index = open + 1;
    while index < bytes.len() && depth > 0 {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        index += 1;
    }
    if depth > 0 {
        (&text[open + 1..], index)
    } else {
        (&text[open + 1..index - 1], index)
    }
}

/// Why a capability string could not be parsed at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CapabilitiesError {
    /// The monitor sent no capability string.
    #[error("the monitor sent an empty capability string")]
    Empty,
    /// The bytes were not text.
    #[error("the capability string is not valid text")]
    NotText,
    /// Text arrived, but nothing in it looked like `key(value)`.
    #[error("the capability string contains no recognisable segment")]
    NoSegments,
}

#[cfg(test)]
mod tests;
