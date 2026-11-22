//! Цветные эмодзи клавиатуры. Steam рисует их системным шрифтом Noto Color Emoji (CBDT, PNG-глифы); Slint с femtovg
//! цветные глифы не рисует, поэтому PNG берётся из того же шрифта напрямую (ttf-parser), последовательности
//! (флаги, ZWJ, оттенки) собираются по лигатурам GSUB. Ничего не рисуется и не генерируется.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use ttf_parser::{gsub::SubstitutionSubtable, Face, GlyphId};

const CANDIDATES: [&str; 5] = [
    "/usr/share/fonts/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    "/usr/share/fonts/google-noto-emoji/NotoColorEmoji.ttf",
    "/usr/share/fonts/noto-color-emoji/NotoColorEmoji.ttf",
    "/usr/share/fonts/NotoColorEmoji.ttf",
];

fn font_data() -> Option<&'static [u8]> {
    static D: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    D.get_or_init(|| {
        let path = CANDIDATES.iter().map(PathBuf::from).find(|p| p.exists()).or_else(|| {
            let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "Noto Color Emoji"]).output().ok()?;
            let p = PathBuf::from(String::from_utf8(out.stdout).ok()?);
            p.to_string_lossy().contains("Emoji").then_some(p)
        })?;
        std::fs::read(path).ok()
    })
    .as_deref()
}

/// Ищет лигатуру для последовательности глифов (GSUB тип 4), результат — один глиф.
fn ligature(face: &Face, seq: &[GlyphId]) -> Option<GlyphId> {
    let gsub = face.tables().gsub?;
    for lookup in gsub.lookups {
        for sub in lookup.subtables.into_iter::<SubstitutionSubtable>() {
            let SubstitutionSubtable::Ligature(l) = sub else { continue };
            let Some(ci) = l.coverage.get(seq[0]) else { continue };
            let Some(set) = l.ligature_sets.get(ci) else { continue };
            for lig in set {
                if lig.components.len() as usize + 1 == seq.len() && lig.components.into_iter().zip(&seq[1..]).all(|(a, b)| a == *b) {
                    return Some(lig.glyph);
                }
            }
        }
    }
    None
}

fn glyph_for(face: &Face, s: &str) -> Option<GlyphId> {
    let try_seq = |chars: Vec<char>| -> Option<GlyphId> {
        let ids: Vec<GlyphId> = chars.iter().filter_map(|c| face.glyph_index(*c)).collect();
        if ids.len() != chars.len() || ids.is_empty() {
            return None;
        }
        if ids.len() == 1 { Some(ids[0]) } else { ligature(face, &ids) }
    };
    let all: Vec<char> = s.chars().collect();
    try_seq(all.clone()).or_else(|| {
        let no_vs: Vec<char> = all.iter().copied().filter(|c| *c != '\u{FE0F}').collect();
        try_seq(no_vs.clone()).or_else(|| try_seq(vec![*no_vs.first()?]))
    })
}

type Pixels = SharedPixelBuffer<Rgba8Pixel>;
const CACHE_LIMIT: usize = 256;

fn decode(face: &Face<'_>, text: &str) -> Option<Pixels> {
    let glyph = face.glyph_raster_image(glyph_for(face, text)?, 128)?;
    if glyph.format != ttf_parser::RasterImageFormat::PNG {
        return None;
    }
    let rgba = image::load_from_memory_with_format(glyph.data, image::ImageFormat::Png).ok()?.to_rgba8();
    Some(Pixels::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}

#[derive(Default)]
struct Cache {
    wanted: Vec<String>,
    pixels: HashMap<String, Option<Pixels>>,
    order: VecDeque<String>,
}

impl Cache {
    fn insert(&mut self, text: String, pixels: Option<Pixels>) {
        if self.pixels.contains_key(&text) {
            return;
        }
        self.order.push_back(text.clone());
        self.pixels.insert(text, pixels);
        if self.order.len() > CACHE_LIMIT {
            if let Some(index) = self.order.iter().position(|text| !self.wanted.contains(text)) {
                let key = self.order.remove(index).unwrap();
                self.pixels.remove(&key);
            }
        }
    }
}

static CACHE: LazyLock<(Mutex<Cache>, Condvar)> = LazyLock::new(Default::default);

pub fn prewarm() {
    static WORKER: OnceLock<()> = OnceLock::new();
    WORKER.get_or_init(|| {
        std::thread::spawn(|| {
            super::osk_model::warm_layouts();
            super::osk_model::emoji_list();
            let face = font_data().and_then(|data| Face::parse(data, 0).ok());
            let (cache, wake) = &*CACHE;
            loop {
                let mut state = cache.lock().unwrap();
                let mut batch: Vec<String>;
                loop {
                    batch = state.wanted.iter().filter(|text| !state.pixels.contains_key(*text)).cloned().collect();
                    if !batch.is_empty() {
                        break;
                    }
                    state = wake.wait(state).unwrap();
                }
                drop(state);
                let decoded: Vec<_> = batch.into_iter().map(|text| {
                    let pixels = face.as_ref().and_then(|face| decode(face, &text));
                    (text, pixels)
                }).collect();
                let mut state = cache.lock().unwrap();
                let mut refresh = false;
                for (text, pixels) in decoded {
                    refresh |= pixels.is_some() && state.wanted.contains(&text);
                    state.insert(text, pixels);
                }
                drop(state);
                if refresh {
                    super::defer(|a| a.osk_emoji_ready());
                }
            }
        });
    });
}

/// Один запрос на видимые клавиши и открытый ряд вариантов. Новый запрос заменяет ещё не начатый.
pub fn request_visible(text: Vec<String>) {
    let (cache, wake) = &*CACHE;
    let mut state = cache.lock().unwrap();
    if state.wanted != text {
        state.wanted = text;
        wake.notify_one();
    }
}

pub fn emoji_image(s: &str) -> Option<Image> {
    CACHE.0.lock().unwrap().pixels.get(s).and_then(|p| p.clone()).map(Image::from_rgba8)
}

#[cfg(test)]
pub fn prepare_fixture(text: &[String]) {
    if text.is_empty() {
        return;
    }
    let face = font_data().and_then(|data| Face::parse(data, 0).ok());
    let decoded: Vec<_> = text.iter()
        .map(|text| (text.clone(), face.as_ref().and_then(|face| decode(face, text))))
        .collect();
    let mut cache = CACHE.0.lock().unwrap();
    for (text, pixels) in decoded {
        cache.insert(text, pixels);
    }
}

#[cfg(test)]
#[path = "osk_emoji_img_tests.rs"]
mod tests;
