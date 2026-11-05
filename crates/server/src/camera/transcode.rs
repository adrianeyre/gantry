//! Camera quality levels, and turning a camera frame into a smaller one.
//!
//! A remote viewer watches over home upload bandwidth (PRD §9.3), so the
//! relay can send a downscaled, frame-rate-limited copy of a camera. This is
//! the CPU work behind that: decode a JPEG, shrink it with an area filter,
//! and encode it again. It is pure, so the relay runs it off the async
//! runtime with `spawn_blocking`.
use std::time::Duration;

use bytes::Bytes;
use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

/// How much of a camera a viewer is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    /// The camera's own frames, as they come.
    Full,
    /// At most 960 pixels wide and 5 frames a second.
    Medium,
    /// At most 480 pixels wide and 2 frames a second.
    Low,
}

impl Quality {
    pub const ALL: [Self; 3] = [Self::Full, Self::Medium, Self::Low];

    /// The level a viewer gets when they don't ask for one: remote viewers
    /// get a reduced stream, local ones the full camera.
    pub fn default_for(remote: bool) -> Self {
        if remote { Self::Medium } else { Self::Full }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|q| q.as_str() == value)
    }

    /// The widest frame this level sends, or `None` for no limit.
    pub fn max_width(self) -> Option<usize> {
        match self {
            Self::Full => None,
            Self::Medium => Some(960),
            Self::Low => Some(480),
        }
    }

    /// The shortest gap between two frames, or `None` for no limit.
    pub fn min_interval(self) -> Option<Duration> {
        match self {
            Self::Full => None,
            Self::Medium => Some(Duration::from_millis(200)),
            Self::Low => Some(Duration::from_millis(500)),
        }
    }

    /// The JPEG quality a downscaled frame is encoded at.
    fn jpeg_quality(self) -> u8 {
        match self {
            Self::Full => 85,
            Self::Medium => 70,
            Self::Low => 60,
        }
    }
}

/// Why a frame couldn't be transcoded.
#[derive(Debug, thiserror::Error)]
pub enum TranscodeError {
    #[error("the frame isn't a JPEG we can decode: {0}")]
    Decode(String),
    #[error("the frame couldn't be encoded: {0}")]
    Encode(String),
}

/// `frame` at `quality`. A frame already no wider than the level allows is
/// passed through unchanged: it is never upscaled.
pub fn transcode(frame: &Bytes, quality: Quality) -> Result<Bytes, TranscodeError> {
    let Some(max_width) = quality.max_width() else {
        return Ok(frame.clone());
    };
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(frame.as_ref()), options);
    decoder
        .decode_headers()
        .map_err(|e| TranscodeError::Decode(e.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| TranscodeError::Decode("no image header".into()))?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    if width <= max_width {
        return Ok(frame.clone());
    }
    let pixels = decoder
        .decode()
        .map_err(|e| TranscodeError::Decode(e.to_string()))?;
    let channels = pixels.len() / (width * height).max(1);
    let colour = match channels {
        3 => jpeg_encoder::ColorType::Rgb,
        1 => jpeg_encoder::ColorType::Luma,
        n => return Err(TranscodeError::Decode(format!("{n} channels per pixel"))),
    };
    let image = Image {
        pixels: &pixels,
        width,
        height,
        channels,
    };
    let (out_width, out_height) = scaled_size(width, height, max_width);
    let small = downscale(&image, out_width, out_height);
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, quality.jpeg_quality())
        .encode(
            &small,
            u16::try_from(out_width).expect("smaller than the source"),
            u16::try_from(out_height).expect("smaller than the source"),
            colour,
        )
        .map_err(|e| TranscodeError::Encode(e.to_string()))?;
    Ok(Bytes::from(jpeg))
}

/// The size of a `width` by `height` frame scaled to `max_width` wide,
/// keeping its shape.
fn scaled_size(width: usize, height: usize, max_width: usize) -> (usize, usize) {
    let out_width = max_width.min(width).max(1);
    let out_height = ((height * out_width + width / 2) / width).max(1);
    (out_width, out_height)
}

/// Interleaved pixels, `channels` bytes each.
struct Image<'a> {
    pixels: &'a [u8],
    width: usize,
    height: usize,
    channels: usize,
}

/// Shrinks `image` with an area (box) filter: each output pixel is the mean
/// of the source pixels it covers.
///
/// It is one plain pass over the source bytes, so it stays quick on a
/// Raspberry Pi (and in unoptimised test builds).
fn downscale(image: &Image<'_>, out_width: usize, out_height: usize) -> Vec<u8> {
    let channels = image.channels;
    let row_bytes = image.width * channels;
    // For each source byte in a row, the output byte it adds to.
    let target: Vec<usize> = (0..row_bytes)
        .map(|i| (i / channels) * out_width / image.width * channels + i % channels)
        .collect();
    // How many source columns each output column covers.
    let mut widths = vec![0u32; out_width];
    for x in 0..image.width {
        widths[x * out_width / image.width] += 1;
    }
    let mut out = vec![0u8; out_width * out_height * channels];
    let mut sums = vec![0u32; out_width * channels];
    let mut row = 0;
    for y in 0..out_height {
        // The source rows this output row covers.
        let bottom = ((y + 1) * image.height / out_height).max(row + 1);
        let rows = u32::try_from(bottom - row).expect("a small height");
        sums.fill(0);
        for line in image.pixels[row * row_bytes..bottom * row_bytes].chunks_exact(row_bytes) {
            for (i, &byte) in line.iter().enumerate() {
                sums[target[i]] += u32::from(byte);
            }
        }
        row = bottom;
        let out_line = &mut out[y * out_width * channels..][..out_width * channels];
        for (i, slot) in out_line.iter_mut().enumerate() {
            let count = widths[i / channels] * rows;
            let mean = (sums[i] + count / 2) / count;
            *slot = u8::try_from(mean).expect("a mean of bytes is a byte");
        }
    }
    out
}

/// The width and height of a JPEG, from its header.
#[cfg(test)]
pub fn dimensions(frame: &[u8]) -> Option<(usize, usize)> {
    let mut decoder = JpegDecoder::new(ZCursor::new(frame));
    decoder.decode_headers().ok()?;
    let info = decoder.info()?;
    Some((usize::from(info.width), usize::from(info.height)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A `width` by `height` JPEG: a left half of one colour and a right half
    /// of another, so scaling can be checked.
    pub fn jpeg(width: u16, height: u16) -> Bytes {
        let (w, h) = (usize::from(width), usize::from(height));
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let pixel = if x < w / 2 {
                    [200, 40, 40]
                } else {
                    [40, 40, 200]
                };
                let at = (y * w + x) * 3;
                rgb[at..at + 3].copy_from_slice(&pixel);
            }
        }
        let mut jpeg = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg, 80)
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        Bytes::from(jpeg)
    }

    #[test]
    fn levels_parse_and_default_by_where_the_viewer_is() {
        for q in Quality::ALL {
            assert_eq!(Quality::parse(q.as_str()), Some(q));
        }
        assert_eq!(Quality::parse("ultra"), None);
        assert_eq!(Quality::parse("Medium"), None);
        assert_eq!(Quality::default_for(true), Quality::Medium);
        assert_eq!(Quality::default_for(false), Quality::Full);
    }

    #[test]
    fn a_large_frame_is_shrunk_keeping_its_shape() {
        let source = jpeg(1280, 720);
        let medium = transcode(&source, Quality::Medium).unwrap();
        assert_eq!(dimensions(&medium), Some((960, 540)));
        let low = transcode(&source, Quality::Low).unwrap();
        assert_eq!(dimensions(&low), Some((480, 270)));
        assert!(low.len() < source.len());

        // The picture survives: the left is still red, the right still blue.
        let mut decoder = JpegDecoder::new(ZCursor::new(low.as_ref()));
        let pixels = decoder.decode().unwrap();
        let at = |x: usize, y: usize| &pixels[(y * 480 + x) * 3..][..3];
        assert!(at(100, 135)[0] > 150 && at(100, 135)[2] < 90);
        assert!(at(380, 135)[2] > 150 && at(380, 135)[0] < 90);
    }

    #[test]
    fn full_and_small_frames_pass_through_untouched() {
        let source = jpeg(1280, 720);
        assert_eq!(transcode(&source, Quality::Full).unwrap(), source);
        let small = jpeg(320, 240);
        assert_eq!(
            transcode(&small, Quality::Low).unwrap(),
            small,
            "not upscaled"
        );
        let exact = jpeg(480, 360);
        assert_eq!(transcode(&exact, Quality::Low).unwrap(), exact);
    }

    #[test]
    fn a_frame_that_isnt_a_jpeg_is_an_error() {
        let junk = Bytes::from_static(b"frame 1");
        assert!(matches!(
            transcode(&junk, Quality::Medium),
            Err(TranscodeError::Decode(_))
        ));
        // Full doesn't look at the frame at all.
        assert_eq!(transcode(&junk, Quality::Full).unwrap(), junk);
    }

    #[test]
    fn the_area_filter_averages_what_each_pixel_covers() {
        // 4x2 grey levels, halved to 2x1.
        let pixels = [0, 100, 200, 250, 50, 150, 0, 30];
        let image = Image {
            pixels: &pixels,
            width: 4,
            height: 2,
            channels: 1,
        };
        assert_eq!(downscale(&image, 2, 1), vec![75, 120]);
        assert_eq!(scaled_size(1280, 720, 960), (960, 540));
        assert_eq!(scaled_size(1001, 3, 480), (480, 1));
    }
}
