use crate::signal::SignalError;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[allow(dead_code)]
pub const AUDIO_CLOCK_RATE: u32 = 8000;
#[allow(dead_code)]
pub const SAMPLES_PER_PACKET: usize = 160;

pub struct AudioPacket {
    pub bytes: Vec<u8>,
}

pub struct AudioCapturer {
    pending: VecDeque<i16>,
    #[cfg(feature = "audio")]
    _stream: Option<wasapi_stream::LoopbackStream>,
}

impl AudioCapturer {
    pub fn try_new() -> Result<Self, SignalError> {
        #[cfg(feature = "audio")]
        {
            let stream = wasapi_stream::LoopbackStream::start()
                .map_err(|source| format!("start audio loopback: {source}"))?;
            Ok(Self {
                pending: VecDeque::new(),
                _stream: Some(stream),
            })
        }
        #[cfg(not(feature = "audio"))]
        {
            Err(SignalError::failed(
                "audio capture not compiled in (rebuild with --features audio)".into(),
            ))
        }
    }

    pub async fn next_packet(&mut self) -> Result<Option<AudioPacket>, SignalError> {
        #[cfg(feature = "audio")]
        {
            self.drain_stream();
            if self.pending.len() < SAMPLES_PER_PACKET {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                self.drain_stream();
            }
            if self.pending.len() < SAMPLES_PER_PACKET {
                return Ok(None);
            }
            let mut payload = Vec::with_capacity(SAMPLES_PER_PACKET);
            for _ in 0..SAMPLES_PER_PACKET {
                if let Some(sample) = self.pending.pop_front() {
                    payload.push(linear_to_mulaw(sample));
                }
            }
            Ok(Some(AudioPacket { bytes: payload }))
        }
        #[cfg(not(feature = "audio"))]
        {
            let _ = &mut self.pending;
            Ok(None)
        }
    }

    #[cfg(feature = "audio")]
    fn drain_stream(&mut self) {
        if let Some(stream) = self._stream.as_ref() {
            for chunk in stream.drain() {
                self.pending.extend(downsample_to_8k_mono(
                    &chunk.samples,
                    chunk.channels,
                    chunk.rate,
                ));
            }
        }
    }
}

#[allow(dead_code)]
pub struct CapturedChunk {
    pub samples: Vec<f32>,
    pub channels: u32,
    pub rate: u32,
}

#[allow(dead_code)]
pub fn downsample_to_8k_mono(samples: &[f32], channels: u32, rate: u32) -> Vec<i16> {
    if samples.is_empty() || channels == 0 || rate == 0 {
        return Vec::new();
    }
    let channels = channels as usize;
    let frames = samples.len() / channels;
    if frames == 0 {
        return Vec::new();
    }
    let step = (rate / AUDIO_CLOCK_RATE).max(1) as usize;
    let mut mono = Vec::new();
    let mut index = 0;
    while index < frames {
        let mut mixed = 0.0f32;
        for channel in 0..channels {
            mixed += samples[index * channels + channel];
        }
        mixed /= channels as f32;
        let clamped = mixed.clamp(-1.0, 1.0);
        mono.push((clamped * 32767.0) as i16);
        index += step;
    }
    mono
}

#[allow(dead_code)]
pub fn linear_to_mulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;
    let mut value = sample as i32;
    let sign = if value < 0 {
        value = -value;
        0x80
    } else {
        0
    };
    if value > CLIP {
        value = CLIP;
    }
    value += BIAS;
    let mut exponent = 7;
    for shift in [14, 13, 12, 11, 10, 9, 8] {
        if value < (1 << shift) {
            exponent -= 1;
        } else {
            break;
        }
    }
    let mantissa = (value >> (exponent + 3)) & 0x0F;
    !(sign | (exponent << 4) | mantissa) as u8
}

#[allow(dead_code)]
pub type SharedAudioQueue = Arc<Mutex<VecDeque<CapturedChunk>>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mulaw_known_vectors() {
        assert_eq!(linear_to_mulaw(0), 0xFF);
        assert_eq!(linear_to_mulaw(32767), 0x80);
        assert_eq!(linear_to_mulaw(-32768), 0x00);
    }

    #[test]
    fn mulaw_symmetry() {
        assert_eq!(linear_to_mulaw(1000) ^ 0x80, linear_to_mulaw(-1000));
    }

    #[test]
    fn downsample_mono_passthrough() {
        let samples = vec![0.0, 0.5, -0.5, 1.0];
        assert_eq!(downsample_to_8k_mono(&samples, 1, 8000), vec![0, 16383, -16383, 32767]);
    }

    #[test]
    fn downsample_stereo_mixes_down() {
        let samples = vec![1.0, -1.0];
        assert_eq!(downsample_to_8k_mono(&samples, 2, 8000), vec![0]);
    }

    #[test]
    fn downsample_decimates_rate() {
        let samples = vec![0.25; 96];
        let mono = downsample_to_8k_mono(&samples, 1, 48000);
        assert_eq!(mono.len(), 16);
        assert!(mono.iter().all(|&sample| sample == 8191));
    }

    #[test]
    fn downsample_rejects_empty() {
        assert!(downsample_to_8k_mono(&[], 1, 8000).is_empty());
        assert!(downsample_to_8k_mono(&[0.5], 0, 8000).is_empty());
    }
}

#[cfg(feature = "audio")]
mod wasapi_stream {
    use super::{CapturedChunk, SharedAudioQueue};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    pub struct LoopbackStream {
        queue: SharedAudioQueue,
    }

    impl LoopbackStream {
        pub fn start() -> Result<Self, String> {
            let queue: SharedAudioQueue = Arc::new(Mutex::new(VecDeque::new()));
            spawn_loopback_thread(queue.clone())?;
            Ok(Self { queue })
        }

        pub fn drain(&self) -> Vec<CapturedChunk> {
            let mut guard = match self.queue.lock() {
                Ok(guard) => guard,
                Err(_) => return Vec::new(),
            };
            guard.drain(..).collect()
        }
    }

    fn spawn_loopback_thread(queue: SharedAudioQueue) -> Result<(), String> {
        std::thread::Builder::new()
            .name("viewpoint-audio".into())
            .spawn(move || {
                if let Err(source) = capture_loop(queue) {
                    eprintln!("[WARN] audio loopback stopped: {source}");
                }
            })
            .map_err(|source| format!("spawn audio thread: {source}"))?;
        Ok(())
    }

    fn capture_loop(queue: SharedAudioQueue) -> Result<(), String> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|source| format!("init com: {source:?}"))?;
        let enumerator =
            wasapi::DeviceEnumerator::new().map_err(|source| format!("enumerator: {source}"))?;
        let device = enumerator
            .get_default_device(&wasapi::Direction::Render)
            .map_err(|source| format!("default render device: {source}"))?;
        let mut client = device
            .get_iaudioclient()
            .map_err(|source| format!("audio client: {source}"))?;
        let format = client
            .get_mixformat()
            .map_err(|source| format!("mix format: {source}"))?;
        let channels = format.get_nchannels() as u32;
        let rate = format.get_samplespersec();
        let bits = format.get_bitspersample();
        let sample_type = format
            .get_subformat()
            .map_err(|source| format!("sample format: {source}"))?;
        let mode = wasapi::StreamMode::PollingShared {
            autoconvert: true,
            buffer_duration_hns: 200000,
        };
        client
            .initialize_client(&format, &wasapi::Direction::Capture, &mode)
            .map_err(|source| format!("init loopback: {source}"))?;
        let capture = client
            .get_audiocaptureclient()
            .map_err(|source| format!("capture client: {source}"))?;
        client
            .start_stream()
            .map_err(|source| format!("start stream: {source}"))?;
        loop {
            let frames = capture
                .get_next_packet_size()
                .map_err(|source| format!("packet size: {source}"))?
                .unwrap_or(0);
            if frames == 0 {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            }
            let mut raw: VecDeque<u8> = VecDeque::new();
            capture
                .read_from_device_to_deque(&mut raw)
                .map_err(|source| format!("read device: {source}"))?;
            let samples = bytes_to_float_samples(raw.make_contiguous(), sample_type, bits);
            if samples.is_empty() {
                continue;
            }
            if let Ok(mut guard) = queue.lock() {
                guard.push_back(CapturedChunk {
                    samples,
                    channels,
                    rate,
                });
                while guard.len() > 64 {
                    guard.pop_front();
                }
            }
        }
    }

    fn bytes_to_float_samples(raw: &[u8], sample_type: wasapi::SampleType, bits: u16) -> Vec<f32> {
        match (sample_type, bits) {
            (wasapi::SampleType::Float, 32) => {
                let (chunks, _) = raw.as_chunks::<4>();
                chunks
                    .iter()
                    .map(|chunk| f32::from_le_bytes(*chunk))
                    .collect()
            }
            (wasapi::SampleType::Int, 16) => {
                let (chunks, _) = raw.as_chunks::<2>();
                chunks
                    .iter()
                    .map(|chunk| i16::from_le_bytes(*chunk) as f32 / 32768.0)
                    .collect()
            }
            (wasapi::SampleType::Int, 32) => {
                let (chunks, _) = raw.as_chunks::<4>();
                chunks
                    .iter()
                    .map(|chunk| i32::from_le_bytes(*chunk) as f32 / 2147483648.0)
                    .collect()
            }
            _ => Vec::new(),
        }
    }
}
