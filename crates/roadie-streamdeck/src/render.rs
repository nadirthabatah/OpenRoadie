//! Turning a picture into the bytes a key screen accepts.
//!
//! The counterpart of [`crate::image`], which frames an already-encoded image
//! into packets: this produces the encoding. Kept behind the `render` feature
//! so the rest of the crate stays dependency-light and portable — the wasm
//! job checks it with default features, and an image codec has no business in
//! that build.
//!
//! Two model facts decide the output, and both come from the catalogue rather
//! than from assumption: the key screen's pixel size, and the rotation its
//! panel is mounted at. Get the rotation wrong and every key is upside down,
//! which is the most likely visible symptom of a wrong catalogue entry.

use image::codecs::bmp::BmpEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder as _, Rgb, RgbImage};

use crate::ProtocolError;
use crate::font;
use crate::model::{ImageFormat as KeyFormat, ImageRotation, Model};

/// JPEG quality for key images.
///
/// High enough that flat colour and text edges stay clean, low enough that a
/// key image is a few kilobytes — which matters, because every image is
/// chopped into 1016-byte packets and written one at a time.
const QUALITY: u8 = 90;

/// Encode `picture` for `model`'s key screens.
///
/// Scales to the model's key size, applies its panel rotation, and encodes in
/// the format it accepts. The result is what [`crate::image::key_image_packets`]
/// expects.
///
/// # Errors
///
/// - [`ProtocolError::ScreenlessModel`] if the model has no key screens.
/// - [`ProtocolError::ImageEncoding`] if the encoder itself fails.
pub fn key_image(model: &Model, picture: &DynamicImage) -> Result<Vec<u8>, ProtocolError> {
    let screens = model
        .screens
        .ok_or(ProtocolError::ScreenlessModel { model: model.name })?;

    let size = u32::from(screens.size_px);
    let scaled = fit(picture, size);
    let oriented = match screens.rotation {
        ImageRotation::None => scaled,
        ImageRotation::Transpose => scaled.rotate90().fliph(),
        ImageRotation::Half => scaled.rotate180(),
    };

    let rgb = oriented.to_rgb8();
    let mut encoded = Vec::new();
    let written = match screens.format {
        KeyFormat::Jpeg => JpegEncoder::new_with_quality(&mut encoded, QUALITY).write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        ),
        // 24-bit, bottom-up, a 54-byte header: what both reference drivers
        // hand a gen 1 deck, since the image crate writes RGB bitmaps exactly
        // as Pillow does.
        KeyFormat::Bmp => BmpEncoder::new(&mut encoded).write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            ExtendedColorType::Rgb8,
        ),
    };
    written.map_err(|error| ProtocolError::ImageEncoding {
        detail: error.to_string(),
    })?;
    Ok(encoded)
}

/// Scale `picture` to fit a square key without distorting it.
///
/// Key screens are square and pictures usually are not. Stretching to fill
/// would squash a wide logo into something the sender did not choose and
/// cannot predict, so the picture is scaled to fit *inside* the key and
/// centred on black. Nothing is distorted and nothing is cropped away — the
/// image that arrives is the image that was sent, just smaller.
///
/// Triangle filtering: key screens are small and pictures are usually
/// downscaled a long way onto them, where nearest-neighbour aliases badly and
/// a slower filter buys nothing visible at 72 pixels.
fn fit(picture: &DynamicImage, size: u32) -> DynamicImage {
    // `resize` preserves the aspect ratio and fits within the bounds, unlike
    // `resize_exact`, which stretches to them.
    let scaled =
        flatten_onto_black(&picture.resize(size, size, image::imageops::FilterType::Triangle));
    if scaled.width() == size && scaled.height() == size {
        return DynamicImage::ImageRgb8(scaled);
    }
    let mut canvas = RgbImage::from_pixel(size, size, Rgb([0, 0, 0]));
    // Integer halves: an odd leftover pixel lands on the right or bottom,
    // which is invisible and keeps this total.
    let x = (size - scaled.width()) / 2;
    let y = (size - scaled.height()) / 2;
    image::imageops::replace(&mut canvas, &scaled, i64::from(x), i64::from(y));
    DynamicImage::ImageRgb8(canvas)
}

/// Composite a picture onto black, rather than discarding what it says about
/// transparency.
///
/// `to_rgb8` drops the alpha channel and keeps the colour underneath it. For
/// an icon that is the usual case rather than an unusual one — icon sets are
/// overwhelmingly PNGs with a transparent surround — and what is stored under
/// full transparency is whatever the tool that exported it happened to leave
/// there. Often black, so it looks right by accident; often white, in which
/// case the key comes out a solid white block with the artwork lost in it.
///
/// Black because that is what the key is: the surround of every other picture
/// this module produces, and what a cleared key shows.
fn flatten_onto_black(picture: &DynamicImage) -> RgbImage {
    let source = picture.to_rgba8();
    let mut flattened = RgbImage::new(source.width(), source.height());
    for (x, y, pixel) in source.enumerate_pixels() {
        let [red, green, blue, alpha] = pixel.0;
        let over = |channel: u8| {
            // Rounded rather than truncated: an eighth of a shade per channel
            // is invisible, but truncation biases every blend towards black
            // and takes the edge off antialiased artwork.
            u8::try_from((u32::from(channel) * u32::from(alpha) + 127) / 255).unwrap_or(u8::MAX)
        };
        flattened.put_pixel(x, y, Rgb([over(red), over(green), over(blue)]));
    }
    flattened
}

/// Blank rows between lines of a label, in glyph-scale units.
const LINE_GAP: usize = 2;

/// The margin a label keeps on every side, as a fraction of the key: a
/// tenth, which is 8 pixels on a Mini's 80 and 9 on a 96-pixel key.
const MARGIN_DIVISOR: usize = 10;

/// Largest scale a label is drawn at, so a one-character label does not
/// become a single enormous letter filling the whole key.
const MAX_SCALE: usize = 6;

/// Draw `text` on a key, as large as it will fit.
///
/// This is the point of the whole image path for an accessible tool: a key
/// that *says* what it does. The label is text the system holds — it can be
/// read back, searched, and spoken — and the picture on the key is a
/// rendering of it, not its identity.
///
/// Words wrap, and the scale is chosen to be the largest at which the wrapped
/// text fits, so short labels are big and long ones stay legible rather than
/// running off the key.
///
/// # Errors
///
/// [`ProtocolError::ScreenlessModel`] if the model has no key screens.
pub fn label(
    model: &Model,
    text: &str,
    foreground: (u8, u8, u8),
    background: (u8, u8, u8),
) -> Result<DynamicImage, ProtocolError> {
    let screens = model
        .screens
        .ok_or(ProtocolError::ScreenlessModel { model: model.name })?;
    let size = usize::from(screens.size_px);

    let (scale, lines) = layout(text, size);
    let mut canvas = RgbImage::from_pixel(
        u32::try_from(size).unwrap_or(u32::MAX),
        u32::try_from(size).unwrap_or(u32::MAX),
        Rgb([background.0, background.1, background.2]),
    );

    let line_height = (font::GLYPH_HEIGHT + LINE_GAP) * scale;
    let block_height = lines.len() * line_height - LINE_GAP * scale;
    let top = size.saturating_sub(block_height) / 2;

    for (row, line) in lines.iter().enumerate() {
        let width = font::text_width(line) * scale;
        let left = size.saturating_sub(width) / 2;
        draw_line(
            &mut canvas,
            line,
            left,
            top + row * line_height,
            scale,
            foreground,
        );
    }
    Ok(DynamicImage::ImageRgb8(canvas))
}

/// Pick the largest scale at which `text` wraps into `size` pixels, and the
/// lines it wraps into.
///
/// Two rules, both from a photograph of a Mini on 2026-11-03. **A word is
/// split only when it fits whole at no scale at all**: "MUTE" drawn as "MU"
/// over "TE" at the larger size read worse than "MUTE" at the smaller one,
/// so every scale that keeps words whole is tried before any that breaks
/// one. And **text keeps a margin** of a tenth of the key on every side,
/// because a keycap's curved edge hides the outermost pixels — the tops of
/// "MUTE" and "NEXT" were lost to it.
///
/// Falls back to scale 1 when nothing fits, so an over-long label is drawn
/// small rather than not at all — showing something legible-if-cramped beats
/// showing a blank key.
fn layout(text: &str, size: usize) -> (usize, Vec<String>) {
    let room = size - 2 * (size / MARGIN_DIVISOR);
    let fits = |scale: usize, lines: &[String]| {
        let line_height = (font::GLYPH_HEIGHT + LINE_GAP) * scale;
        let height = lines.len() * line_height - LINE_GAP * scale;
        let widest = lines
            .iter()
            .map(|line| font::text_width(line) * scale)
            .max()
            .unwrap_or(0);
        height <= room && widest <= room
    };
    let longest_word = text
        .split_whitespace()
        .map(|word| word.chars().count())
        .max()
        .unwrap_or(0);
    for whole_words in [true, false] {
        for scale in (1..=MAX_SCALE).rev() {
            let columns = columns(room, scale);
            if whole_words && longest_word > columns {
                continue;
            }
            let lines = wrap(text, columns);
            if fits(scale, &lines) {
                return (scale, lines);
            }
        }
    }
    (1, wrap(text, columns(room, 1)))
}

/// How many glyphs fit across a key at `scale`.
fn columns(size: usize, scale: usize) -> usize {
    let per_glyph = (font::GLYPH_WIDTH + font::GLYPH_SPACING) * scale;
    // The last glyph needs no trailing gap, so one extra fits when the
    // remainder covers a glyph without its spacing.
    (size + font::GLYPH_SPACING * scale) / per_glyph
}

/// Greedy word wrap. A word longer than a line is hard-broken rather than
/// allowed to overflow.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        for chunk in hard_break(word, columns) {
            if current.is_empty() {
                current = chunk;
            } else if current.chars().count() + 1 + chunk.chars().count() <= columns {
                current.push(' ');
                current.push_str(&chunk);
            } else {
                lines.push(std::mem::replace(&mut current, chunk));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Split a word too long for a line into line-sized pieces.
fn hard_break(word: &str, columns: usize) -> Vec<String> {
    if word.chars().count() <= columns {
        return vec![word.to_string()];
    }
    word.chars()
        .collect::<Vec<_>>()
        .chunks(columns)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Draw one line of glyphs at `scale`, top-left at `(left, top)`.
fn draw_line(
    canvas: &mut RgbImage,
    line: &str,
    left: usize,
    top: usize,
    scale: usize,
    colour: (u8, u8, u8),
) {
    let ink = Rgb([colour.0, colour.1, colour.2]);
    let mut x = left;
    for character in line.chars() {
        for (row, pixels) in font::glyph(character).iter().enumerate() {
            for (column, pixel) in pixels.chars().enumerate() {
                if pixel != '#' {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = x + column * scale + dx;
                        let py = top + row * scale + dy;
                        let (Ok(px), Ok(py)) = (u32::try_from(px), u32::try_from(py)) else {
                            continue;
                        };
                        if px < canvas.width() && py < canvas.height() {
                            canvas.put_pixel(px, py, ink);
                        }
                    }
                }
            }
        }
        x += (font::GLYPH_WIDTH + font::GLYPH_SPACING) * scale;
    }
}

/// A key filled with one colour, ready to encode.
///
/// Separate from [`key_image`] so a caller that just wants a coloured key does
/// not have to construct an image to get one — and so the size is taken from
/// the model rather than guessed.
///
/// # Errors
///
/// [`ProtocolError::ScreenlessModel`] if the model has no key screens.
pub fn solid(model: &Model, red: u8, green: u8, blue: u8) -> Result<DynamicImage, ProtocolError> {
    let screens = model
        .screens
        .ok_or(ProtocolError::ScreenlessModel { model: model.name })?;
    let size = u32::from(screens.size_px);
    Ok(DynamicImage::ImageRgb8(RgbImage::from_pixel(
        size,
        size,
        Rgb([red, green, blue]),
    )))
}

#[cfg(test)]
mod tests {

    use image::{
        DynamicImage, GenericImageView as _, ImageEncoder as _, Rgb, RgbImage, Rgba, RgbaImage,
    };

    use super::{flatten_onto_black, key_image, solid};
    use crate::ProtocolError;
    use crate::model::{ELGATO_VENDOR_ID, identify};

    fn model(product_id: u16) -> &'static crate::model::Model {
        identify(ELGATO_VENDOR_ID, product_id).expect("catalogued")
    }

    #[test]
    fn a_short_word_stays_whole_on_a_mini_rather_than_split_to_be_bigger() {
        // Every label photographed split on the Mini on 2026-11-03.
        for word in ["MUTE", "PLAY", "PREV", "NEXT"] {
            let (_, lines) = super::layout(word, 80);
            assert_eq!(lines, [word], "{word} was split");
        }
        let (_, lines) = super::layout("VOL DOWN", 80);
        assert_eq!(lines, ["VOL", "DOWN"]);
    }

    #[test]
    fn a_word_too_long_to_fit_whole_is_still_broken_rather_than_overflowing() {
        let (scale, lines) = super::layout("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 80);
        assert_eq!(scale, 1);
        assert!(lines.len() > 1, "{lines:?}");
    }

    #[test]
    fn a_label_keeps_clear_of_the_edge_a_keycap_hides() {
        let mini = model(0x0063);
        for text in ["MUTE", "NEXT", "VOL UP", "W", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"] {
            let drawn = super::label(mini, text, (255, 255, 255), (0, 0, 0))
                .expect("labels")
                .to_rgb8();
            for (x, y, pixel) in drawn.enumerate_pixels() {
                let in_margin = x < 8 || y < 8 || x >= 72 || y >= 72;
                assert!(
                    !(in_margin && pixel.0[0] > 0),
                    "{text:?} draws at ({x}, {y}), inside the 8-pixel margin"
                );
            }
        }
    }

    #[test]
    fn a_solid_key_is_the_models_own_size() {
        // The MK.2 is 72px, the XL 96px, the Plus 120px: the size must come
        // from the catalogue, not from a constant.
        for (product_id, expected) in [(0x0080, 72), (0x006c, 96), (0x0084, 120)] {
            let filled = solid(model(product_id), 1, 2, 3).expect("has screens");
            assert_eq!(filled.dimensions(), (expected, expected));
        }
    }

    #[test]
    fn a_screenless_model_cannot_be_filled_or_encoded() {
        let pedal = model(0x0086);
        assert!(matches!(
            solid(pedal, 0, 0, 0).expect_err("the Pedal has no screens"),
            ProtocolError::ScreenlessModel { .. }
        ));
        let any = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([0, 0, 0])));
        assert!(matches!(
            key_image(pedal, &any).expect_err("the Pedal has no screens"),
            ProtocolError::ScreenlessModel { .. }
        ));
    }

    #[test]
    fn gen1_keys_are_encoded_as_bitmaps_never_as_jpeg() {
        // The original's bitmap is encoded; it is its framing that is still
        // refused, in `crate::image`, which is where its unknown lives.
        let original = model(0x0060);
        let any = DynamicImage::ImageRgb8(RgbImage::from_pixel(8, 8, Rgb([0, 0, 0])));
        let encoded = key_image(original, &any).expect("gen 1 encodes a bitmap");
        assert_eq!(&encoded[..2], b"BM");
        assert_eq!(encoded.len(), 54 + 72 * 72 * 3);
    }

    #[test]
    fn an_encoded_key_image_is_a_jpeg_of_the_models_size() {
        let mk2 = model(0x0080);
        let filled = solid(mk2, 200, 40, 40).expect("has screens");
        let encoded = key_image(mk2, &filled).expect("encodes");

        // JPEG's own magic, so this asserts the format rather than trusting
        // the call that asked for it.
        assert_eq!(
            &encoded[..2],
            &[0xff, 0xd8],
            "starts with a JPEG SOI marker"
        );
        assert_eq!(
            &encoded[encoded.len() - 2..],
            &[0xff, 0xd9],
            "ends with EOI"
        );

        let decoded = image::load_from_memory(&encoded).expect("re-reads as an image");
        assert_eq!(decoded.dimensions(), (72, 72));
    }

    #[test]
    fn an_oversized_picture_is_scaled_down_to_the_key() {
        let mk2 = model(0x0080);
        let big = DynamicImage::ImageRgb8(RgbImage::from_pixel(1000, 400, Rgb([10, 20, 30])));
        let encoded = key_image(mk2, &big).expect("encodes");
        let decoded = image::load_from_memory(&encoded).expect("re-reads");
        assert_eq!(decoded.dimensions(), (72, 72));
    }

    /// A half turn is the difference between a legible key and an upside-down
    /// one, so it is asserted rather than assumed: a picture with a distinct
    /// top-left corner must come back with that corner at bottom-right.
    /// A wide picture must not be squashed into the square key. Stretching
    /// would change the picture into something the sender did not choose,
    /// which is worse than showing it smaller.
    /// Render a label and return which pixels are ink, as a grid of bools —
    /// enough to reason about placement without decoding a JPEG.
    fn inked(text: &str) -> Vec<Vec<bool>> {
        let picture = super::label(model(0x0080), text, (255, 255, 255), (0, 0, 0))
            .expect("has screens")
            .to_rgb8();
        (0..picture.height())
            .map(|y| {
                (0..picture.width())
                    .map(|x| picture.get_pixel(x, y).0[0] > 128)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_label_draws_something_and_stays_inside_the_key() {
        let grid = inked("OK");
        assert_eq!(grid.len(), 72);
        assert!(
            grid.iter().flatten().any(|lit| *lit),
            "a label must actually draw"
        );
        // Nothing may be written outside the canvas; `put_pixel` is bounds
        // checked by the caller, so this asserts the arithmetic rather than
        // the guard.
        for row in &grid {
            assert_eq!(row.len(), 72);
        }
    }

    #[test]
    fn an_empty_label_leaves_the_key_blank() {
        assert!(
            !inked("").iter().flatten().any(|lit| *lit),
            "nothing to say means nothing drawn"
        );
        assert!(!inked("   ").iter().flatten().any(|lit| *lit));
    }

    #[test]
    fn different_words_produce_different_pictures() {
        // The failure this catches is a renderer that draws the same blob
        // whatever it is given — every other test here would still pass.
        assert_ne!(inked("YES"), inked("NO"));
        assert_ne!(inked("A"), inked("B"));
    }

    /// Scale, not total ink: twelve small letters light more pixels than one
    /// large one, so counting ink measures the wrong thing. The property that
    /// matters is how big each glyph is drawn.
    #[test]
    fn a_short_label_is_drawn_at_a_larger_scale_than_a_long_one() {
        let (short, _) = super::layout("A", 72);
        let (long, _) = super::layout("AAAAAAAAAAAA", 72);
        let (longer, _) = super::layout("MUTE THE MICROPHONE AND PAUSE THE MUSIC NOW", 72);
        assert!(
            short > long,
            "one letter ({short}) should be drawn larger than twelve ({long})"
        );
        assert!(
            long >= longer,
            "and twelve ({long}) no smaller than a sentence ({longer})"
        );
        assert!(longer >= 1, "even a long label is drawn at some scale");
    }

    #[test]
    fn a_chosen_layout_always_fits_the_key() {
        // The scale search must never return one whose text overflows: an
        // overflowing label is clipped at the edge, which reads as a typo.
        for text in [
            "A",
            "OK",
            "MUTE",
            "MUTE MIC",
            "MUTE THE MICROPHONE",
            "SUPERCALIFRAGILISTICEXPIALIDOCIOUS",
            "",
        ] {
            let (scale, lines) = super::layout(text, 72);
            let line_height = (super::font::GLYPH_HEIGHT + super::LINE_GAP) * scale;
            let height = lines.len() * line_height - super::LINE_GAP * scale;
            let widest = lines
                .iter()
                .map(|line| super::font::text_width(line) * scale)
                .max()
                .unwrap_or(0);
            assert!(height <= 72, "{text:?} is {height} tall at scale {scale}");
            assert!(widest <= 72, "{text:?} is {widest} wide at scale {scale}");
        }
    }

    #[test]
    fn a_long_label_wraps_onto_several_lines() {
        // Two words that cannot sit side by side must stack, which shows up
        // as ink in both the upper and lower halves of the key.
        let grid = inked("MUTE MICROPHONE");
        let upper = grid[..36].iter().flatten().filter(|lit| **lit).count();
        let lower = grid[36..].iter().flatten().filter(|lit| **lit).count();
        assert!(
            upper > 0 && lower > 0,
            "the label should occupy both halves"
        );
    }

    #[test]
    fn a_label_is_centred_rather_than_pinned_to_a_corner() {
        let grid = inked("HI");
        let lit_columns: Vec<usize> = (0..72).filter(|x| grid.iter().any(|row| row[*x])).collect();
        let left = *lit_columns.first().expect("something is drawn");
        let right = *lit_columns.last().expect("something is drawn");
        let margin_left = left;
        let margin_right = 71 - right;
        assert!(
            margin_left.abs_diff(margin_right) <= 2,
            "left margin {margin_left} and right margin {margin_right} should match"
        );
    }

    #[test]
    fn a_word_longer_than_the_key_is_broken_rather_than_lost() {
        // A single unbroken word wider than the key must still appear.
        let grid = inked("SUPERCALIFRAGILISTIC");
        assert!(
            grid.iter().flatten().any(|lit| *lit),
            "an over-long word must still be drawn"
        );
    }

    #[test]
    fn a_label_encodes_for_the_device_like_any_other_picture() {
        let mk2 = model(0x0080);
        let picture = super::label(mk2, "REC", (255, 0, 0), (0, 0, 0)).expect("has screens");
        let encoded = key_image(mk2, &picture).expect("encodes");
        assert_eq!(&encoded[..2], &[0xff, 0xd8]);
        let decoded = image::load_from_memory(&encoded).expect("re-reads");
        assert_eq!(decoded.dimensions(), (72, 72));
    }

    #[test]
    fn a_screenless_model_cannot_be_labelled() {
        assert!(matches!(
            super::label(model(0x0086), "X", (255, 255, 255), (0, 0, 0))
                .expect_err("the Pedal has no screens"),
            ProtocolError::ScreenlessModel { .. }
        ));
    }

    #[test]
    fn a_wide_picture_keeps_its_shape_and_is_padded_rather_than_squashed() {
        let mk2 = model(0x0080);
        // A 2:1 red block. Fitted into 72x72 it becomes 72 wide by 36 tall,
        // centred, with black above and below.
        let wide = DynamicImage::ImageRgb8(RgbImage::from_pixel(200, 100, Rgb([255, 0, 0])));
        let encoded = key_image(mk2, &wide).expect("encodes");
        let decoded = image::load_from_memory(&encoded)
            .expect("re-reads")
            .to_rgb8();

        let red_at = |x: u32, y: u32| u32::from(decoded.get_pixel(x, y).0[0]);
        assert!(red_at(36, 36) > 200, "the centre band carries the picture");
        assert!(
            red_at(36, 4) < 60,
            "the top must be padding, not a stretched picture"
        );
        assert!(red_at(36, 68) < 60, "and so must the bottom");
    }

    #[test]
    fn a_square_picture_fills_the_key_edge_to_edge() {
        // The padding path must not shrink a picture that already fits.
        let mk2 = model(0x0080);
        let square = DynamicImage::ImageRgb8(RgbImage::from_pixel(300, 300, Rgb([0, 255, 0])));
        let encoded = key_image(mk2, &square).expect("encodes");
        let decoded = image::load_from_memory(&encoded)
            .expect("re-reads")
            .to_rgb8();
        for (x, y) in [(2, 2), (69, 2), (2, 69), (69, 69), (36, 36)] {
            assert!(
                u32::from(decoded.get_pixel(x, y).0[1]) > 200,
                "corner ({x}, {y}) should be the picture, not padding"
            );
        }
    }

    #[test]
    fn the_models_rotation_is_actually_applied() {
        let mk2 = model(0x0080); // rotation: Half
        assert_eq!(
            mk2.screens.expect("has screens").rotation,
            crate::model::ImageRotation::Half
        );

        let mut marked = RgbImage::from_pixel(72, 72, Rgb([0, 0, 0]));
        // A bright block in the top-left quarter.
        for y in 0..20 {
            for x in 0..20 {
                marked.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let encoded = key_image(mk2, &DynamicImage::ImageRgb8(marked)).expect("encodes");
        let decoded = image::load_from_memory(&encoded)
            .expect("re-reads")
            .to_rgb8();

        let corner = |x: u32, y: u32| u32::from(decoded.get_pixel(x, y).0[0]);
        assert!(
            corner(61, 61) > 200,
            "the marked corner must land at bottom-right after a half turn"
        );
        assert!(
            corner(10, 10) < 60,
            "and the original top-left must now be dark"
        );
    }

    #[test]
    fn a_mini_key_is_an_80_pixel_24_bit_bitmap_of_the_size_the_drivers_send() {
        let mini = model(0x0063);
        let encoded = key_image(mini, &DynamicImage::ImageRgb8(RgbImage::new(10, 10)))
            .expect("the Mini encodes");
        assert_eq!(&encoded[..2], b"BM");
        assert_eq!(encoded.len(), 19_254, "54-byte header and 80 × 80 × 3");
        assert_eq!(
            u32::from_le_bytes([encoded[10], encoded[11], encoded[12], encoded[13]]),
            54
        );
        let width = i32::from_le_bytes([encoded[18], encoded[19], encoded[20], encoded[21]]);
        let height = i32::from_le_bytes([encoded[22], encoded[23], encoded[24], encoded[25]]);
        assert_eq!((width, height), (80, 80), "positive height: bottom-up rows");
        assert_eq!(u16::from_le_bytes([encoded[28], encoded[29]]), 24);
    }

    #[test]
    fn the_mini_is_mirrored_across_its_diagonal_not_turned() {
        // A mark at the top right must land at the bottom left. A plain
        // quarter turn — what the catalogue said before — puts it at the
        // bottom right, which on the device is every key shown mirrored.
        let mini = model(0x0063);
        let mut marked = RgbImage::from_pixel(80, 80, Rgb([0, 0, 0]));
        for y in 0..20 {
            for x in 60..80 {
                marked.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        let encoded = key_image(mini, &DynamicImage::ImageRgb8(marked)).expect("encodes");
        let decoded = image::load_from_memory(&encoded)
            .expect("re-reads")
            .to_rgb8();
        let bright = |x: u32, y: u32| decoded.get_pixel(x, y).0[0] > 200;
        assert!(
            bright(10, 70),
            "the top-right mark lands at the bottom left"
        );
        assert!(
            !bright(70, 70),
            "not at the bottom right, as a quarter turn would put it"
        );
        assert!(!bright(70, 10), "and no longer at the top right");
    }

    /// The quality setting must actually reach the encoder. It did not once:
    /// the constant was defined and then ignored, which is invisible except
    /// as a file size.
    #[test]
    fn the_quality_setting_reaches_the_encoder() {
        // A photographic gradient, so quality has something to act on — a
        // flat fill compresses to nearly the same size at any setting.
        let mut noisy = RgbImage::new(72, 72);
        for (x, y, pixel) in noisy.enumerate_pixels_mut() {
            let v = u8::try_from((x * 7 + y * 13) % 256).unwrap_or(0);
            *pixel = Rgb([v, v.wrapping_mul(3), v.wrapping_add(90)]);
        }
        let picture = DynamicImage::ImageRgb8(noisy);
        let at_configured = key_image(model(0x0080), &picture).expect("encodes");

        let mut low = Vec::new();
        let rgb = picture.to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut low, 10)
            .write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("encodes");

        assert!(
            at_configured.len() > low.len(),
            "quality {} produced {} bytes, no larger than quality 10's {} — \
             the setting is not reaching the encoder",
            super::QUALITY,
            at_configured.len(),
            low.len()
        );
    }

    #[test]
    fn an_encoded_key_image_frames_into_packets() {
        // The two halves of the image path meet here: what render produces
        // must be what the framer accepts.
        let mk2 = model(0x0080);
        let filled = solid(mk2, 0, 128, 255).expect("has screens");
        let encoded = key_image(mk2, &filled).expect("encodes");
        let packets = crate::image::key_image_packets(mk2, 0, &encoded).expect("frames");
        assert_ne!(packets.len(), 0);
        assert_eq!(
            packets.last().expect("at least one packet")[3],
            1,
            "the final packet is flagged last"
        );
    }

    /// An icon is almost always a PNG with a transparent surround, and what is
    /// stored *under* full transparency is whatever the exporting tool left
    /// there. Dropping the alpha channel keeps that colour, so an icon whose
    /// surround is transparent white came out as a solid white key with the
    /// artwork lost in it.
    #[test]
    fn a_transparent_surround_becomes_the_key_colour_not_what_is_under_it() {
        let transparent_white =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 0])));
        let flattened = flatten_onto_black(&transparent_white);
        for pixel in flattened.pixels() {
            assert_eq!(
                pixel.0,
                [0, 0, 0],
                "transparent white must composite to black"
            );
        }
    }

    /// And an opaque picture must come through untouched, or every icon that
    /// was fine before is now darker than the person drew it.
    #[test]
    fn an_opaque_picture_is_unchanged() {
        let opaque =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, Rgba([200, 100, 50, 255])));
        let flattened = flatten_onto_black(&opaque);
        for pixel in flattened.pixels() {
            assert_eq!(pixel.0, [200, 100, 50]);
        }
    }

    /// Half transparent is half the colour, rounded rather than truncated:
    /// truncation biases every blend towards black and takes the edge off
    /// antialiased artwork.
    #[test]
    fn a_half_transparent_pixel_is_half_the_colour() {
        let half =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 128])));
        assert_eq!(flatten_onto_black(&half).get_pixel(0, 0).0, [128, 128, 128]);
    }

    /// The whole path, through the encoder a device actually receives.
    #[test]
    fn a_fully_transparent_icon_reaches_the_key_as_black() {
        let model = identify(ELGATO_VENDOR_ID, 0x0080).expect("the MK.2 is catalogued");
        let transparent_white =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(72, 72, Rgba([255, 255, 255, 0])));
        let encoded = key_image(model, &transparent_white).expect("encodes");
        let decoded = image::load_from_memory(&encoded)
            .expect("decodes")
            .to_rgb8();
        let centre = decoded
            .get_pixel(decoded.width() / 2, decoded.height() / 2)
            .0;
        assert!(
            centre.iter().all(|channel| *channel < 32),
            "a transparent icon reached the key as {centre:?}, not black"
        );
    }

    /// A picture with a degenerate shape must come back as a key, not a panic.
    ///
    /// The centring arithmetic subtracts the scaled size from the key size, so
    /// anything that scaled *larger* than the key would underflow and abort
    /// the program. Aspect ratios this extreme also round one dimension to
    /// zero, which is the other way an image pipeline falls over. Someone will
    /// point this at a banner, a 1-pixel spacer, or a file that is not the
    /// shape they think it is, and getting a black key beats getting a crash.
    #[test]
    fn a_picture_of_any_shape_produces_a_key_rather_than_a_panic() {
        let mk2 = model(0x0080);
        for (width, height) in [
            (1_u32, 1_u32),
            (10_000, 1),
            (1, 10_000),
            (3000, 2),
            (2, 3000),
            (1, 72),
            (72, 1),
        ] {
            let picture = DynamicImage::ImageRgba8(RgbaImage::new(width, height));
            let encoded = key_image(mk2, &picture)
                .unwrap_or_else(|error| panic!("{width}x{height} failed: {error}"));
            assert!(
                !encoded.is_empty(),
                "{width}x{height} encoded to nothing at all"
            );
            // Still a JPEG of the key's own size, not a picture of the shape
            // that went in.
            let decoded = image::load_from_memory(&encoded).expect("a JPEG comes back");
            assert_eq!(
                (decoded.width(), decoded.height()),
                (72, 72),
                "{width}x{height} did not come back as a key-sized image"
            );
        }
    }
}
