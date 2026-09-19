use crate::capture::Frame;
use crate::config::StreamSettings;
use openh264::encoder::{
    BitRate, Complexity, EncoderConfig, FrameRate, IntraFramePeriod, UsageType,
};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};
use openh264::OpenH264API;

#[derive(Clone)]
pub struct Packet {
    pub bytes: bytes::Bytes,
}

pub struct Encoder {
    inner: openh264::encoder::Encoder,
    target_width: u32,
    target_height: u32,
}

impl Encoder {
    pub fn from_stream(stream: &StreamSettings) -> Result<Self, String> {
        if stream.codec != "h264" {
            return Err(format!(
                "stream.codec {} not supported in this build, set codec to h264",
                stream.codec
            ));
        }
        if stream.width == 0 || stream.height == 0 {
            return Err("stream.width and stream.height must be >= 1".into());
        }
        let api = OpenH264API::from_source();
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(stream.bitrate_kbps * 1000))
            .max_frame_rate(FrameRate::from_hz(stream.fps as f32))
            .usage_type(UsageType::ScreenContentRealTime)
            .complexity(Complexity::Low)
            .intra_frame_period(IntraFramePeriod::from_num_frames(stream.fps.max(1) * 2));
        let inner = openh264::encoder::Encoder::with_api_config(api, config)
            .map_err(|source| format!("start encoder: {source}"))?;
        Ok(Self {
            inner,
            target_width: stream.width,
            target_height: stream.height,
        })
    }

    pub fn encode_frame(&mut self, frame: Frame) -> Result<Option<Packet>, String> {
        let scaled = scale_to_target(frame, self.target_width, self.target_height);
        let width = scaled.width as usize;
        let height = scaled.height as usize;
        let source = RgbaSliceU8::new(&scaled.pixels, (width, height));
        let mut yuv = YUVBuffer::new(width, height);
        yuv.read_rgba8(source);
        let bitstream = self
            .inner
            .encode(&yuv)
            .map_err(|source| format!("encode frame: {source}"))?;
        let mut annexb = Vec::new();
        for layer in 0..bitstream.num_layers() {
            let layer = bitstream.layer(layer).ok_or("missing layer")?;
            for unit in 0..layer.nal_count() {
                let nal = layer.nal_unit(unit).ok_or("missing nal")?;
                annexb.extend_from_slice(nal);
            }
        }
        if annexb.is_empty() {
            return Ok(None);
        }
        save_debug_roundtrip(&bitstream, &annexb);
        Ok(Some(Packet {
            bytes: bytes::Bytes::from(annexb),
        }))
    }
}

fn save_debug_roundtrip(bitstream: &openh264::encoder::EncodedBitStream<'_>, annexb: &[u8]) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) && std::env::var("VIEWPOINT_DEBUG_SNAPSHOT").is_ok() {
        let mut inventory = format!("layers={}", bitstream.num_layers());
        for layer in 0..bitstream.num_layers() {
            if let Some(entry) = bitstream.layer(layer) {
                inventory.push_str(&format!(" L{}(nals={}", layer, entry.nal_count()));
                for unit in 0..entry.nal_count() {
                    if let Some(nal) = entry.nal_unit(unit) {
                        let kind = nal.get(4).map(|byte| byte & 0x1F).unwrap_or(255);
                        inventory.push_str(&format!(" [t={kind} len={}]", nal.len()));
                    }
                }
                inventory.push(')');
            }
        }
        println!("[INFO] first packet {inventory} total={}", annexb.len());
    }
    use std::sync::atomic::AtomicU32;
    use std::sync::Mutex;
    static DONE: AtomicBool = AtomicBool::new(false);
    static TRIES: AtomicU32 = AtomicU32::new(0);
    static DECODER: Mutex<Option<openh264::decoder::Decoder>> = Mutex::new(None);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    if std::env::var("VIEWPOINT_DEBUG_SNAPSHOT").is_err() {
        return;
    }
    if TRIES.fetch_add(1, Ordering::Relaxed) > 120 {
        eprintln!("[WARN] roundtrip: no picture in first 120 packets");
        DONE.store(true, Ordering::Relaxed);
        return;
    }
    let mut guard = match DECODER.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if guard.is_none() {
        match openh264::decoder::Decoder::new() {
            Ok(decoder) => *guard = Some(decoder),
            Err(source) => {
                eprintln!("[WARN] roundtrip decoder: {source}");
                DONE.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
    let decoder = match guard.as_mut() {
        Some(decoder) => decoder,
        None => return,
    };
    match decoder.decode(annexb) {
        Ok(Some(decoded)) => {
            let (width, height) = decoded.dimensions();
            let mut pixels = vec![0u8; width * height * 4];
            decoded.write_rgba8(&mut pixels);
            let frame = Frame {
                width: width as u32,
                height: height as u32,
                pixels,
            };
            match crate::capture::write_bitmap("viewpoint-roundtrip.bmp", &frame) {
                Ok(()) => println!("[INFO] roundtrip saved to viewpoint-roundtrip.bmp"),
                Err(source) => eprintln!("[WARN] roundtrip snapshot failed: {source}"),
            }
            DONE.store(true, Ordering::Relaxed);
        }
        Ok(None) => {}
        Err(source) => {
            eprintln!("[WARN] roundtrip decode failed: {source}");
            DONE.store(true, Ordering::Relaxed);
        }
    }
}

fn scale_to_target(frame: Frame, target_width: u32, target_height: u32) -> Frame {
    if frame.width == target_width && frame.height == target_height {
        return frame;
    }
    if frame.width == 0 || frame.height == 0 {
        return frame;
    }
    let source_width = frame.width as usize;
    let source_height = frame.height as usize;
    let destination_width = target_width as usize;
    let destination_height = target_height as usize;
    let mut scaled = vec![0u8; destination_width * destination_height * 4];
    for destination_y in 0..destination_height {
        let source_y = destination_y * source_height / destination_height;
        for destination_x in 0..destination_width {
            let source_x = destination_x * source_width / destination_width;
            let source_offset = (source_y * source_width + source_x) * 4;
            let destination_offset = (destination_y * destination_width + destination_x) * 4;
            scaled[destination_offset..destination_offset + 4]
                .copy_from_slice(&frame.pixels[source_offset..source_offset + 4]);
        }
    }
    Frame {
        width: target_width,
        height: target_height,
        pixels: scaled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_frame(width: u32, height: u32, pixel: [u8; 4]) -> Frame {
        Frame {
            width,
            height,
            pixels: vec![pixel[0], pixel[1], pixel[2], pixel[3]]
                .repeat(width as usize * height as usize),
        }
    }

    #[test]
    fn scale_identity_keeps_pixels() {
        let frame = solid_frame(2, 2, [10, 20, 30, 255]);
        let scaled = scale_to_target(frame, 2, 2);
        assert_eq!((scaled.width, scaled.height), (2, 2));
        assert_eq!(scaled.pixels.len(), 2 * 2 * 4);
        assert!(scaled.pixels.chunks_exact(4).all(|pixel| pixel == [10, 20, 30, 255]));
    }

    #[test]
    fn scale_down_samples_top_left() {
        let mut pixels = Vec::new();
        for row in 0..4u8 {
            for column in 0..4u8 {
                pixels.extend_from_slice(&[row * 4 + column, 0, 0, 255]);
            }
        }
        let frame = Frame {
            width: 4,
            height: 4,
            pixels,
        };
        let scaled = scale_to_target(frame, 2, 2);
        assert_eq!((scaled.width, scaled.height), (2, 2));
        let reds: Vec<u8> = scaled.pixels.chunks_exact(4).map(|pixel| pixel[0]).collect();
        assert_eq!(reds, vec![0, 2, 8, 10]);
    }

    #[test]
    fn scale_empty_frame_passes_through() {
        let frame = Frame {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        let scaled = scale_to_target(frame, 1920, 1080);
        assert_eq!((scaled.width, scaled.height), (0, 0));
    }
}
