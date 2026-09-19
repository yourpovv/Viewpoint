use std::time::Duration;

use tokio::sync::mpsc;

use crate::signal::SignalError;

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub struct DesktopCapturer {
    receiver: Option<mpsc::Receiver<Result<Frame, String>>>,
}

impl DesktopCapturer {
    pub fn for_monitor(monitor: u32) -> Self {
        let (sender, receiver) = mpsc::channel(1);
        match std::thread::Builder::new()
            .name("viewpoint-capture".into())
            .spawn(move || capture_loop(monitor, sender))
        {
            Ok(_) => Self {
                receiver: Some(receiver),
            },
            Err(source) => {
                eprintln!("[ERROR] capture thread failed to start: {source}");
                Self { receiver: None }
            }
        }
    }

    pub async fn next_frame(&mut self) -> Result<Frame, SignalError> {
        let receiver = self
            .receiver
            .as_mut()
            .ok_or("capture thread failed to start")?;
        receiver
            .recv()
            .await
            .ok_or("capture thread stopped")?
            .map_err(SignalError::failed)
    }
}

fn capture_loop(monitor: u32, frames: mpsc::Sender<Result<Frame, String>>) {
    let index = monitor.saturating_sub(1) as usize;
    loop {
        let frame = find_monitor(index).and_then(|found| capture_monitor(&found, index));
        let failed = frame.is_err();
        if frames.blocking_send(frame).is_err() {
            break;
        }
        if failed {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn find_monitor(index: usize) -> Result<xcap::Monitor, String> {
    let monitors = xcap::Monitor::all().map_err(|source| format!("list monitors: {source}"))?;
    monitors
        .into_iter()
        .nth(index)
        .ok_or_else(|| format!("monitor {} not found", index + 1))
}

fn capture_monitor(monitor: &xcap::Monitor, index: usize) -> Result<Frame, String> {
    let shot = monitor
        .capture_image()
        .map_err(|source| format!("grab frame {}: {source}", index + 1))?;
    let frame = Frame {
        width: shot.width(),
        height: shot.height(),
        pixels: shot.into_raw(),
    };
    save_debug_snapshot(&frame);
    Ok(frame)
}

fn save_debug_snapshot(frame: &Frame) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SAVED: AtomicBool = AtomicBool::new(false);
    if SAVED.swap(true, Ordering::Relaxed) {
        return;
    }
    if std::env::var("VIEWPOINT_DEBUG_SNAPSHOT").is_err() {
        return;
    }
    if let Err(source) = write_bitmap("viewpoint-snapshot.bmp", frame) {
        eprintln!("[WARN] snapshot failed: {source}");
    } else {
        println!("[INFO] snapshot saved to viewpoint-snapshot.bmp");
    }
}

pub(crate) fn write_bitmap(path: &str, frame: &Frame) -> Result<(), String> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    if width == 0 || height == 0 || frame.pixels.len() != width * height * 4 {
        return Err("bad frame dimensions".into());
    }
    let stride = (width * 3 + 3) & !3;
    let mut data = vec![0u8; 54 + stride * height];
    data[0..2].copy_from_slice(b"BM");
    let file_size = data.len() as u32;
    data[2..6].copy_from_slice(&file_size.to_le_bytes());
    data[10..14].copy_from_slice(&54u32.to_le_bytes());
    data[14..18].copy_from_slice(&40u32.to_le_bytes());
    data[18..22].copy_from_slice(&(width as u32).to_le_bytes());
    data[22..26].copy_from_slice(&(height as u32).to_le_bytes());
    data[26..28].copy_from_slice(&1u16.to_le_bytes());
    data[28..30].copy_from_slice(&24u16.to_le_bytes());
    for row in 0..height {
        let source_row = row * width * 4;
        let target_row = 54 + (height - 1 - row) * stride;
        for column in 0..width {
            let source = source_row + column * 4;
            let target = target_row + column * 3;
            data[target] = frame.pixels[source + 2];
            data[target + 1] = frame.pixels[source + 1];
            data[target + 2] = frame.pixels[source];
        }
    }
    std::fs::write(path, data).map_err(|source| format!("write {path}: {source}"))?;
    Ok(())
}
