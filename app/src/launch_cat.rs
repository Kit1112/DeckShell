//! Launch cover look (task 331): the Custom Loader "Cat" GIF the owner's Steam shows while a game loads.
//!
//! The GIF is built into the binary from `assets/css-loader/themes/Custom Loader/images/cat.gif`, a copy of the
//! owner's CSS Loader theme. The owner confirmed the theme may be used. Nothing is read from the Deck's home folder.
use std::io::{BufRead, Seek};
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::imageops::{self, FilterType};
use image::AnimationDecoder;

/// The theme's CSS forces the throbber to 250 x 250 px (`height: 250px; width: 250px`).
pub const SIZE: u32 = 250;

/// The theme's animated GIF, compiled into the app.
pub const CAT_GIF: &[u8] = include_bytes!("../../assets/css-loader/themes/Custom Loader/images/cat.gif");

/// One decoded frame: RGBA8, `SIZE` x `SIZE`. The theme stretches the GIF to that box rather than keeping its aspect.
pub struct Frame {
    pub rgba: Vec<u8>,
    pub delay: Duration,
}

/// Every frame of the built-in cat GIF.
pub fn decode_embedded() -> Result<Vec<Frame>, String> {
    decode_reader(std::io::Cursor::new(CAT_GIF))
}

/// Every frame of the GIF, composited by the decoder, scaled to `SIZE` x `SIZE`, with browser-style delays.
pub fn decode_reader<R: BufRead + Seek>(reader: R) -> Result<Vec<Frame>, String> {
    let decoder = GifDecoder::new(reader).map_err(|e| e.to_string())?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| e.to_string())?;
        let delay = browser_delay(Duration::from(frame.delay()));
        let scaled = imageops::resize(frame.buffer(), SIZE, SIZE, FilterType::Triangle);
        frames.push(Frame { rgba: scaled.into_raw(), delay });
    }
    if frames.is_empty() {
        return Err("GIF has no frames".into());
    }
    Ok(frames)
}

/// Browsers (and so Steam's CEF) show frames with a delay of 10 ms or less at 100 ms.
fn browser_delay(delay: Duration) -> Duration {
    if delay <= Duration::from_millis(10) { Duration::from_millis(100) } else { delay }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame as GifFrame, Rgba, RgbaImage};

    fn gif_bytes(delays_ms: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            let frames = delays_ms.iter().enumerate().map(|(i, ms)| {
                let img = RgbaImage::from_pixel(8, 6, Rgba([i as u8 * 60, 10, 20, 255]));
                GifFrame::from_parts(img, 0, 0, Delay::from_saturating_duration(Duration::from_millis(*ms)))
            });
            enc.encode_frames(frames).unwrap();
        }
        out
    }

    #[test]
    fn decodes_every_frame_scaled_with_its_delay() {
        let frames = decode_reader(std::io::Cursor::new(gif_bytes(&[30, 50, 30]))).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.rgba.len() == (SIZE * SIZE * 4) as usize));
        let delays: Vec<u64> = frames.iter().map(|f| f.delay.as_millis() as u64).collect();
        assert_eq!(delays, vec![30, 50, 30]);
    }

    #[test]
    fn a_tiny_delay_is_shown_at_one_tenth_of_a_second() {
        let frames = decode_reader(std::io::Cursor::new(gif_bytes(&[0]))).unwrap();
        assert_eq!(frames[0].delay, Duration::from_millis(100));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_reader(std::io::Cursor::new(b"not a gif".to_vec())).is_err());
    }

    #[test]
    fn the_built_in_cat_gif_decodes() {
        let frames = decode_embedded().unwrap();
        assert!(frames.len() > 1, "the cat is an animation");
        assert!(frames.iter().all(|f| f.rgba.len() == (SIZE * SIZE * 4) as usize));
    }
}
