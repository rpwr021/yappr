use std::io::Cursor;
use std::sync::OnceLock;

use tray_icon::Icon;

use crate::ui::{
    ANSWERING, ERROR, NOTICE, PROVISIONING_ENGINE, PROVISIONING_MODEL, RECORDING_CHAT,
    RECORDING_DICTATE, SPEAKING, STARTING, TRANSCRIBING,
};

const ICON_SIZE: usize = 32;

const IDLE_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-01-idle.png");
const CHAT_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-03-recording-chat.png");
const TRANSCRIBING_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-04-transcribing.png");
const ANSWERING_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-06-answering.png");
const SPEAKING_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-07-speaking.png");
const ERROR_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-08-error.png");

// Mouth/sound-dot animation frames for the dictate state.
const DICTATE_FRAMES: [&[u8]; 4] = [
    include_bytes!("../resources/assets/logos/yappr-logo-02-recording-dictate-frame-01.png"),
    include_bytes!("../resources/assets/logos/yappr-logo-02-recording-dictate-frame-02.png"),
    include_bytes!("../resources/assets/logos/yappr-logo-02-recording-dictate-frame-03.png"),
    include_bytes!("../resources/assets/logos/yappr-logo-02-recording-dictate-frame-04.png"),
];

pub fn icon_for_state(state: u8, frame: usize) -> Result<Icon, Box<dyn std::error::Error>> {
    let png = match state {
        RECORDING_DICTATE => DICTATE_FRAMES[frame % DICTATE_FRAMES.len()],
        RECORDING_CHAT => CHAT_ICON,
        TRANSCRIBING => TRANSCRIBING_ICON,
        ANSWERING => ANSWERING_ICON,
        SPEAKING => SPEAKING_ICON,
        ERROR => ERROR_ICON,
        NOTICE => IDLE_ICON,
        PROVISIONING_MODEL | PROVISIONING_ENGINE | STARTING => TRANSCRIBING_ICON,
        _ => IDLE_ICON,
    };
    // Dictate frames share one crop box so the head stays put while the mouth
    // and sound dots animate; other states crop to their own content.
    let crop = if state == RECORDING_DICTATE {
        Some(dictate_crop())
    } else {
        None
    };
    let rgba = decode_icon(png, crop)?;
    Icon::from_rgba(rgba, ICON_SIZE as u32, ICON_SIZE as u32).map_err(Into::into)
}

/// Union of the content bounds across all dictate frames, computed once.
fn dictate_crop() -> Rect {
    static CROP: OnceLock<Rect> = OnceLock::new();
    *CROP.get_or_init(|| {
        DICTATE_FRAMES
            .iter()
            .filter_map(|png| decode_rgba(png).ok())
            .map(|img| content_bounds(&img.pixels, img.width, img.height))
            .reduce(|a, b| a.union(b))
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                w: 1,
                h: 1,
            })
    })
}

/// States whose icon cycles through frames, driving a per-tick redraw in `ui`.
/// Only dictate has frame art today; other active states render a static icon.
pub fn is_animated(state: u8) -> bool {
    state == RECORDING_DICTATE
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let r = (self.x + self.w).max(other.x + other.w);
        let b = (self.y + self.h).max(other.y + other.h);
        Rect {
            x,
            y,
            w: r - x,
            h: b - y,
        }
    }
}

struct Rgba {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

fn decode_rgba(bytes: &[u8]) -> Result<Rgba, Box<dyn std::error::Error>> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info()?;
    let mut decoded = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("invalid png buffer size")?
    ];
    let info = reader.next_frame(&mut decoded)?;
    decoded.truncate(info.buffer_size());

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("expected 8-bit RGBA png".into());
    }
    Ok(Rgba {
        pixels: decoded,
        width: info.width as usize,
        height: info.height as usize,
    })
}

/// Crop the icon to visible content so it fills the menu-bar canvas instead of
/// the source's transparent padding, then scale that region to `ICON_SIZE`.
/// `crop` overrides the per-image content bounds (used to share one box across
/// animation frames).
fn decode_icon(bytes: &[u8], crop: Option<Rect>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let img = decode_rgba(bytes)?;
    let bounds = crop.unwrap_or_else(|| content_bounds(&img.pixels, img.width, img.height));
    Ok(scale_region(&img, bounds, ICON_SIZE))
}

/// Bounding box of pixels with alpha above a small threshold (keeps soft halos).
fn content_bounds(pixels: &[u8], width: usize, height: usize) -> Rect {
    const ALPHA_THRESHOLD: u8 = 4;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (width, height, 0usize, 0usize);
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if pixels[(y * width + x) * 4 + 3] > ALPHA_THRESHOLD {
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !found {
        return Rect {
            x: 0,
            y: 0,
            w: width,
            h: height,
        };
    }
    Rect {
        x: min_x,
        y: min_y,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
    }
}

/// Nearest-neighbour scale of `rect` within `img` into a centred `size`x`size`
/// transparent canvas, preserving aspect ratio.
fn scale_region(img: &Rgba, rect: Rect, size: usize) -> Vec<u8> {
    let mut out = vec![0; size * size * 4];
    let scale = (size as f32 / rect.w.max(rect.h) as f32).max(f32::MIN_POSITIVE);
    let dw = ((rect.w as f32 * scale).round() as usize).clamp(1, size);
    let dh = ((rect.h as f32 * scale).round() as usize).clamp(1, size);
    let off_x = (size - dw) / 2;
    let off_y = (size - dh) / 2;
    for y in 0..dh {
        let sy = rect.y + y * rect.h / dh;
        for x in 0..dw {
            let sx = rect.x + x * rect.w / dw;
            let src_i = (sy * img.width + sx) * 4;
            let dst_i = ((y + off_y) * size + (x + off_x)) * 4;
            out[dst_i..dst_i + 4].copy_from_slice(&img.pixels[src_i..src_i + 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        content_bounds, icon_for_state, is_animated, scale_region, Rect, Rgba, DICTATE_FRAMES,
        ICON_SIZE,
    };
    use crate::ui::{ERROR, IDLE, NOTICE, RECORDING_CHAT, RECORDING_DICTATE, TRANSCRIBING};

    #[test]
    fn only_dictate_animates() {
        assert!(is_animated(RECORDING_DICTATE));
        for state in [IDLE, ERROR, NOTICE, RECORDING_CHAT, TRANSCRIBING] {
            assert!(!is_animated(state), "state {state} should be static");
        }
    }

    #[test]
    fn every_dictate_frame_decodes() {
        // Frame index wraps and each underlying frame is a valid 32x32 icon.
        for frame in 0..DICTATE_FRAMES.len() * 2 {
            assert!(icon_for_state(RECORDING_DICTATE, frame).is_ok());
        }
    }

    #[test]
    fn static_states_decode() {
        assert!(icon_for_state(IDLE, 0).is_ok());
        assert!(icon_for_state(TRANSCRIBING, 7).is_ok());
    }

    // 4x4 image with a single opaque pixel at (1,2); rest transparent.
    fn sparse_image() -> Rgba {
        let mut pixels = vec![0u8; 4 * 4 * 4];
        let i = (2 * 4 + 1) * 4;
        pixels[i..i + 4].copy_from_slice(&[10, 20, 30, 255]);
        Rgba {
            pixels,
            width: 4,
            height: 4,
        }
    }

    #[test]
    fn content_bounds_ignores_transparent_padding() {
        let img = sparse_image();
        let b = content_bounds(&img.pixels, img.width, img.height);
        assert_eq!((b.x, b.y, b.w, b.h), (1, 2, 1, 1));
    }

    #[test]
    fn content_bounds_falls_back_to_full_when_empty() {
        let pixels = vec![0u8; 2 * 2 * 4];
        let b = content_bounds(&pixels, 2, 2);
        assert_eq!((b.x, b.y, b.w, b.h), (0, 0, 2, 2));
    }

    #[test]
    fn rect_union_covers_both() {
        let a = Rect {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        }; // covers 1..3
        let b = Rect {
            x: 4,
            y: 0,
            w: 1,
            h: 5,
        }; // covers x 4..5, y 0..5
        let u = a.union(b);
        assert_eq!((u.x, u.y, u.w, u.h), (1, 0, 4, 5));
    }

    #[test]
    fn scale_region_fills_canvas_from_cropped_content() {
        // The single opaque pixel should expand to fill the whole 32x32 canvas,
        // i.e. cropping then scaling removes the transparent padding.
        let img = sparse_image();
        let bounds = content_bounds(&img.pixels, img.width, img.height);
        let out = scale_region(&img, bounds, ICON_SIZE);
        assert_eq!(out.len(), ICON_SIZE * ICON_SIZE * 4);
        let center = ((ICON_SIZE / 2) * ICON_SIZE + ICON_SIZE / 2) * 4;
        assert_eq!(&out[center..center + 4], &[10, 20, 30, 255]);
    }
}
