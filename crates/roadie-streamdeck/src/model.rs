//! Which Stream Deck is on the other end, and what it can do.
//!
//! Pure identity and layout data — the counterpart to
//! `roadie_device_registry` for Elgato hardware. Nothing here performs I/O;
//! a host layer matches an enumerated HID node's vendor and product id
//! against [`identify`] and carries the resulting [`Model`] alongside its
//! open handle.

use crate::ProtocolError;

/// Elgato's USB vendor ID. Every Stream Deck enumerates under it.
pub const ELGATO_VENDOR_ID: u16 = 0x0fd9;

/// Which revision of the Stream Deck wire protocol a device speaks.
///
/// The split is the single most load-bearing fact about a model: it selects
/// the feature-report layouts in [`crate::report`] and the image framing in
/// [`crate::image`]. Everything else is presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Generation {
    /// The original 2015-era protocol: BMP key images, 8191-byte image
    /// packets, and its own brightness and reset reports.
    Gen1,
    /// The protocol introduced with the Stream Deck V2 and used by every
    /// model since: JPEG key images, 1024-byte image packets, and the
    /// `0x03`-prefixed control reports.
    Gen2,
}

/// Codec a model's key screens accept for uploaded images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    /// Windows bitmap, used by [`Generation::Gen1`] hardware.
    Bmp,
    /// Baseline JPEG, used by [`Generation::Gen2`] hardware.
    Jpeg,
}

/// How an image must be rotated before upload, because the key screens are
/// not all mounted in the same orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageRotation {
    /// Upload the image as-is.
    None,
    /// Mirror across the diagonal from top left to bottom right, so the
    /// pixel at column x, row y lands at column y, row x.
    ///
    /// The Mini's panel. This was catalogued as a plain quarter turn, which
    /// would have shown every key mirrored; two independent drivers agree on
    /// the transpose, reached by different routes — a quarter turn clockwise
    /// then a left-right flip in one, a quarter turn anticlockwise then a
    /// top-bottom flip in the other.
    Transpose,
    /// Rotate a half turn — equivalent to flipping both axes.
    Half,
}

/// How an encoded key image is cut into output reports.
///
/// Not derivable from [`Generation`]: the two gen 1 models share a codec and
/// a header shape but page differently, and a page counter one off is an
/// image that never appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFraming {
    /// 1024-byte reports, an 8-byte header, a 16-bit page counted from 0 —
    /// every gen 2 model.
    Gen2,
    /// 1024-byte reports, a 16-byte header, an 8-bit page counted from 0,
    /// and the key counted from 1 — the Mini.
    Gen1Mini,
    /// 8191-byte reports carrying the image in two halves, pages counted
    /// from 1 — the original Stream Deck. Not encoded: there is none on this
    /// project's desk to confirm it against.
    Gen1Original,
}

/// The order in which a model reports its keys in an input report.
///
/// Index 0 is always the key a user would call "top left". Some hardware
/// scans each row in the opposite direction, so the reported position has to
/// be mirrored within its row before it means anything to a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyOrder {
    /// Reported left-to-right within each row; reported index == key index.
    LeftToRight,
    /// Reported right-to-left within each row; the index needs mirroring.
    ///
    /// This is the documented behavior of the original Stream Deck and is
    /// the one fact in this catalog most in need of hardware confirmation
    /// (see the crate-level note on verification status).
    RightToLeftRows,
}

/// The physical arrangement of a model's keys.
///
/// Carried so a key can be described by where it *is* — "row 2, column 3" —
/// rather than only by an opaque index. That matters for screen-reader
/// output and for a spoken instruction like "the top left key", neither of
/// which can be served by an index alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyGrid {
    /// Keys per row.
    pub columns: u8,
    /// Number of rows.
    pub rows: u8,
}

impl KeyGrid {
    /// Total keys in the grid.
    #[must_use]
    pub const fn count(self) -> u16 {
        self.columns as u16 * self.rows as u16
    }
}

/// A key's place on the front of the device, one-based so it reads naturally
/// when spoken aloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPosition {
    /// Row, counting from 1 at the top.
    pub row: u8,
    /// Column, counting from 1 at the left.
    pub column: u8,
}

/// What a model's key screens accept, for models that have them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyScreens {
    /// Codec the device accepts.
    pub format: ImageFormat,
    /// Width and height in pixels; key screens are square on every model.
    pub size_px: u16,
    /// Rotation to apply before upload.
    pub rotation: ImageRotation,
    /// How the encoded image is cut into reports.
    pub framing: ImageFraming,
}

/// One Stream Deck model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    /// USB product ID, unique within [`ELGATO_VENDOR_ID`].
    pub product_id: u16,
    /// Marketing name, suitable for display and for speech.
    pub name: &'static str,
    /// Wire protocol revision.
    pub generation: Generation,
    /// Physical key arrangement.
    pub grid: KeyGrid,
    /// Reported key ordering within a row.
    pub key_order: KeyOrder,
    /// Key screen properties, or `None` for a model whose keys have no
    /// screens (the Stream Deck Pedal).
    pub screens: Option<KeyScreens>,
    /// Rotary encoders, which the Stream Deck Plus has and no other model
    /// does. Their reports are not decoded yet.
    pub dials: u8,
}

impl Model {
    /// How many keys this model has.
    #[must_use]
    pub const fn key_count(&self) -> u16 {
        self.grid.count()
    }

    /// Where `index` sits on the device.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::KeyOutOfRange`] when `index` is not a key on
    /// this model.
    pub fn key_position(&self, index: u16) -> Result<KeyPosition, ProtocolError> {
        if index >= self.key_count() {
            return Err(ProtocolError::KeyOutOfRange {
                index,
                count: self.key_count(),
            });
        }
        // Bounds-checked above, and every catalogued model has far fewer
        // than 255 keys — so this conversion cannot fail today, and a future
        // model that broke the assumption would error rather than wrap.
        let index = u8::try_from(index).map_err(|_| ProtocolError::KeyOutOfRange {
            index,
            count: self.key_count(),
        })?;
        Ok(KeyPosition {
            row: index / self.grid.columns + 1,
            column: index % self.grid.columns + 1,
        })
    }

    /// Translate a hardware-reported key position into a key index, undoing
    /// the per-row mirroring on models that scan rows right-to-left.
    ///
    /// # Errors
    ///
    /// Returns [`ProtocolError::KeyOutOfRange`] when `reported` is not a key
    /// on this model.
    pub fn key_index_from_reported(&self, reported: u16) -> Result<u16, ProtocolError> {
        if reported >= self.key_count() {
            return Err(ProtocolError::KeyOutOfRange {
                index: reported,
                count: self.key_count(),
            });
        }
        Ok(match self.key_order {
            KeyOrder::LeftToRight => reported,
            KeyOrder::RightToLeftRows => {
                let columns = u16::from(self.grid.columns);
                let row = reported / columns;
                let column = reported % columns;
                row * columns + (columns - 1 - column)
            }
        })
    }
}

/// Every Stream Deck model this crate knows how to talk to.
///
/// Adding a model that speaks an already-implemented generation is an entry
/// here and nothing else — the deliberate "no code" path for growing device
/// support.
pub const MODELS: &[Model] = &[
    Model {
        product_id: 0x0060,
        name: "Stream Deck",
        generation: Generation::Gen1,
        grid: KeyGrid {
            columns: 5,
            rows: 3,
        },
        key_order: KeyOrder::RightToLeftRows,
        screens: Some(KeyScreens {
            format: ImageFormat::Bmp,
            size_px: 72,
            rotation: ImageRotation::Half,
            framing: ImageFraming::Gen1Original,
        }),
        dials: 0,
    },
    Model {
        product_id: 0x0063,
        name: "Stream Deck Mini",
        generation: Generation::Gen1,
        grid: KeyGrid {
            columns: 3,
            rows: 2,
        },
        key_order: KeyOrder::LeftToRight,
        screens: Some(KeyScreens {
            format: ImageFormat::Bmp,
            size_px: 80,
            rotation: ImageRotation::Transpose,
            framing: ImageFraming::Gen1Mini,
        }),
        dials: 0,
    },
    Model {
        product_id: 0x006d,
        name: "Stream Deck V2",
        generation: Generation::Gen2,
        grid: KeyGrid {
            columns: 5,
            rows: 3,
        },
        key_order: KeyOrder::LeftToRight,
        screens: Some(KeyScreens {
            format: ImageFormat::Jpeg,
            size_px: 72,
            rotation: ImageRotation::Half,
            framing: ImageFraming::Gen2,
        }),
        dials: 0,
    },
    Model {
        product_id: 0x0080,
        name: "Stream Deck MK.2",
        generation: Generation::Gen2,
        grid: KeyGrid {
            columns: 5,
            rows: 3,
        },
        key_order: KeyOrder::LeftToRight,
        screens: Some(KeyScreens {
            format: ImageFormat::Jpeg,
            size_px: 72,
            rotation: ImageRotation::Half,
            framing: ImageFraming::Gen2,
        }),
        dials: 0,
    },
    Model {
        product_id: 0x006c,
        name: "Stream Deck XL",
        generation: Generation::Gen2,
        grid: KeyGrid {
            columns: 8,
            rows: 4,
        },
        key_order: KeyOrder::LeftToRight,
        screens: Some(KeyScreens {
            format: ImageFormat::Jpeg,
            size_px: 96,
            rotation: ImageRotation::Half,
            framing: ImageFraming::Gen2,
        }),
        dials: 0,
    },
    Model {
        product_id: 0x0084,
        name: "Stream Deck Plus",
        generation: Generation::Gen2,
        grid: KeyGrid {
            columns: 4,
            rows: 2,
        },
        key_order: KeyOrder::LeftToRight,
        screens: Some(KeyScreens {
            format: ImageFormat::Jpeg,
            size_px: 120,
            rotation: ImageRotation::None,
            framing: ImageFraming::Gen2,
        }),
        dials: 4,
    },
    Model {
        product_id: 0x0086,
        name: "Stream Deck Pedal",
        generation: Generation::Gen2,
        grid: KeyGrid {
            columns: 3,
            rows: 1,
        },
        key_order: KeyOrder::LeftToRight,
        screens: None,
        dials: 0,
    },
];

/// Look up the model behind an enumerated HID node.
///
/// Returns `None` for any device that is not a Stream Deck this crate knows,
/// including other Elgato hardware — an unknown product ID is never guessed
/// at, because the generation it would imply decides how every subsequent
/// byte is framed.
#[must_use]
pub fn identify(vendor_id: u16, product_id: u16) -> Option<&'static Model> {
    if vendor_id != ELGATO_VENDOR_ID {
        return None;
    }
    MODELS.iter().find(|model| model.product_id == product_id)
}

#[cfg(test)]
mod tests {
    use super::{ELGATO_VENDOR_ID, KeyOrder, MODELS, identify};
    use crate::ProtocolError;

    #[test]
    fn every_product_id_is_listed_once() {
        let mut ids: Vec<u16> = MODELS.iter().map(|model| model.product_id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "product IDs must be unique");
    }

    #[test]
    fn a_screenless_model_is_still_a_model() {
        let pedal = identify(ELGATO_VENDOR_ID, 0x0086).expect("the Pedal is catalogued");
        assert!(pedal.screens.is_none());
        assert_eq!(pedal.key_count(), 3);
    }

    #[test]
    fn unknown_hardware_is_never_guessed_at() {
        assert!(identify(ELGATO_VENDOR_ID, 0xffff).is_none());
        // A Logitech mouse must not match a Stream Deck on product ID alone.
        assert!(identify(0x046d, 0x0060).is_none());
    }

    #[test]
    fn key_positions_are_one_based_and_row_major() {
        let xl = identify(ELGATO_VENDOR_ID, 0x006c).expect("the XL is catalogued");
        let first = xl.key_position(0).expect("key 0 exists");
        assert_eq!((first.row, first.column), (1, 1));
        // The XL is 8 wide: index 8 opens the second row.
        let second_row = xl.key_position(8).expect("key 8 exists");
        assert_eq!((second_row.row, second_row.column), (2, 1));
        let last = xl.key_position(31).expect("key 31 exists");
        assert_eq!((last.row, last.column), (4, 8));
    }

    #[test]
    fn a_key_past_the_end_is_an_error_not_a_wrapped_position() {
        let mini = identify(ELGATO_VENDOR_ID, 0x0063).expect("the Mini is catalogued");
        let error = mini.key_position(6).expect_err("the Mini has only 6 keys");
        assert!(matches!(
            error,
            ProtocolError::KeyOutOfRange { index: 6, count: 6 }
        ));
    }

    #[test]
    fn left_to_right_models_report_their_keys_unchanged() {
        let mk2 = identify(ELGATO_VENDOR_ID, 0x0080).expect("the MK.2 is catalogued");
        assert_eq!(mk2.key_order, KeyOrder::LeftToRight);
        for reported in 0..mk2.key_count() {
            let index = mk2
                .key_index_from_reported(reported)
                .expect("every reported key is in range");
            assert_eq!(index, reported);
        }
    }

    #[test]
    fn mirrored_models_have_their_rows_flipped_back() {
        let original = identify(ELGATO_VENDOR_ID, 0x0060).expect("the original is catalogued");
        assert_eq!(original.key_order, KeyOrder::RightToLeftRows);
        // 5 columns: the first reported key of a row is that row's last key.
        assert_eq!(original.key_index_from_reported(0).expect("in range"), 4);
        assert_eq!(original.key_index_from_reported(4).expect("in range"), 0);
        assert_eq!(original.key_index_from_reported(5).expect("in range"), 9);
        assert_eq!(original.key_index_from_reported(9).expect("in range"), 5);
    }

    #[test]
    fn mirroring_is_its_own_inverse_and_never_collides() {
        let original = identify(ELGATO_VENDOR_ID, 0x0060).expect("the original is catalogued");
        let mut seen: Vec<u16> = (0..original.key_count())
            .map(|reported| {
                let index = original
                    .key_index_from_reported(reported)
                    .expect("in range");
                assert_eq!(
                    original.key_index_from_reported(index).expect("in range"),
                    reported,
                    "mirroring twice must return the original index"
                );
                index
            })
            .collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), total, "no two reported keys may share an index");
    }

    #[test]
    fn grid_dimensions_agree_with_key_counts() {
        for model in MODELS {
            assert!(
                model.grid.columns > 0 && model.grid.rows > 0,
                "{}",
                model.name
            );
            let last = model.key_count() - 1;
            let position = model.key_position(last).expect("the last key exists");
            assert_eq!(position.row, model.grid.rows, "{}", model.name);
            assert_eq!(position.column, model.grid.columns, "{}", model.name);
        }
    }
}
