use std::io::Cursor;

use tray_icon::Icon;

use crate::ui::{
    ANSWERING, ERROR, NOTICE, PROVISIONING_ENGINE, PROVISIONING_MODEL, RECORDING_CHAT,
    RECORDING_DICTATE, SPEAKING, STARTING, TRANSCRIBING,
};

const ICON_SIZE: usize = 32;

const IDLE_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-01-idle.png");
const DICTATE_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-02-recording-dictate.png");
const CHAT_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-03-recording-chat.png");
const TRANSCRIBING_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-04-transcribing.png");
const ANSWERING_ICON: &[u8] =
    include_bytes!("../resources/assets/logos/yappr-logo-06-answering.png");
const SPEAKING_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-07-speaking.png");
const ERROR_ICON: &[u8] = include_bytes!("../resources/assets/logos/yappr-logo-08-error.png");

pub fn icon_for_state(state: u8, _frame: usize) -> Result<Icon, Box<dyn std::error::Error>> {
    let png = match state {
        RECORDING_DICTATE => DICTATE_ICON,
        RECORDING_CHAT => CHAT_ICON,
        TRANSCRIBING => TRANSCRIBING_ICON,
        ANSWERING => ANSWERING_ICON,
        SPEAKING => SPEAKING_ICON,
        ERROR => ERROR_ICON,
        NOTICE => IDLE_ICON,
        PROVISIONING_MODEL | PROVISIONING_ENGINE | STARTING => TRANSCRIBING_ICON,
        _ => IDLE_ICON,
    };
    let rgba = decode_icon(png)?;
    Icon::from_rgba(rgba, ICON_SIZE as u32, ICON_SIZE as u32).map_err(Into::into)
}

fn decode_icon(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info()?;
    let mut decoded = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("invalid png buffer size")?
    ];
    let info = reader.next_frame(&mut decoded)?;
    let decoded = &decoded[..info.buffer_size()];

    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("expected 8-bit RGBA png".into());
    }

    Ok(scale_nearest(
        decoded,
        info.width as usize,
        info.height as usize,
        ICON_SIZE,
    ))
}

fn scale_nearest(src: &[u8], width: usize, height: usize, size: usize) -> Vec<u8> {
    let mut out = vec![0; size * size * 4];
    for y in 0..size {
        let sy = y * height / size;
        for x in 0..size {
            let sx = x * width / size;
            let src_i = (sy * width + sx) * 4;
            let dst_i = (y * size + x) * 4;
            out[dst_i..dst_i + 4].copy_from_slice(&src[src_i..src_i + 4]);
        }
    }
    out
}
