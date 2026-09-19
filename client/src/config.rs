use serde::Deserialize;
use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct ConfigError {
    detail: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "config: {}", self.detail)
    }
}

impl Error for ConfigError {}

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub webrtc: WebRtcSettings,
    #[serde(default)]
    pub stream: StreamSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerSettings {
    #[serde(default = "default_public_url")]
    pub public_url: String,
    #[serde(default = "default_control_url")]
    pub control_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WebRtcSettings {
    #[serde(default = "default_stun_urls")]
    pub stun_urls: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamSettings {
    #[serde(default = "default_monitor")]
    pub monitor: u32,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_bitrate")]
    pub bitrate_kbps: u32,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_audio_enabled")]
    pub audio_enabled: bool,
}

pub fn load(path: &str) -> Result<Settings, Box<dyn Error>> {
    let raw = std::fs::read_to_string(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            format!("config {path} not found: copy viewpoint.example.yaml to {path} and retry")
        } else {
            format!("read {path}: {source}")
        }
    })?;
    let mut settings: Settings =
        serde_yaml::from_str(&raw).map_err(|source| format!("parse {path}: {source}"))?;
    settings.stream.clamp_to_safe_ranges();
    settings.validate()?;
    Ok(settings)
}

impl Settings {
    pub fn control_url(&self) -> String {
        self.server.control_url.clone()
    }

    pub fn public_url(&self) -> String {
        self.server.public_url.clone()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.server.public_url.is_empty() {
            return Err(ConfigError {
                detail: "server.public_url is empty".into(),
            });
        }
        if self.server.control_url.is_empty() {
            return Err(ConfigError {
                detail: "server.control_url is empty".into(),
            });
        }
        if self.stream.codec != "h264" && self.stream.codec != "vp8" {
            return Err(ConfigError {
                detail: format!(
                    "stream.codec must be h264 or vp8, got {}",
                    self.stream.codec
                ),
            });
        }
        Ok(())
    }
}

impl StreamSettings {
    fn clamp_to_safe_ranges(&mut self) {
        self.monitor = self.monitor.clamp(1, 4);
        self.fps = self.fps.clamp(15, 120);
        self.bitrate_kbps = self.bitrate_kbps.clamp(500, 50000);
        if self.width == 0 {
            self.width = default_width();
        }
        if self.height == 0 {
            self.height = default_height();
        }
    }
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            public_url: default_public_url(),
            control_url: default_control_url(),
        }
    }
}

impl Default for WebRtcSettings {
    fn default() -> Self {
        Self {
            stun_urls: default_stun_urls(),
        }
    }
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            monitor: default_monitor(),
            width: default_width(),
            height: default_height(),
            fps: default_fps(),
            bitrate_kbps: default_bitrate(),
            codec: default_codec(),
            audio_enabled: default_audio_enabled(),
        }
    }
}

fn default_public_url() -> String {
    "http://localhost:8080".into()
}

fn default_control_url() -> String {
    "http://127.0.0.1:8080".into()
}

fn default_monitor() -> u32 {
    1
}

fn default_width() -> u32 {
    1920
}

fn default_height() -> u32 {
    1080
}

fn default_fps() -> u32 {
    30
}

fn default_bitrate() -> u32 {
    6000
}

fn default_codec() -> String {
    "h264".into()
}

fn default_audio_enabled() -> bool {
    true
}

fn default_stun_urls() -> Vec<String> {
    vec!["stun:stun.l.google.com:19302".into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let stream = StreamSettings::default();
        assert_eq!(stream.monitor, 1);
        assert_eq!((stream.width, stream.height), (1920, 1080));
        assert_eq!(stream.fps, 30);
        assert_eq!(stream.bitrate_kbps, 6000);
        assert_eq!(stream.codec, "h264");
        assert!(stream.audio_enabled);
    }

    #[test]
    fn clamp_keeps_ranges() {
        let mut stream = StreamSettings {
            monitor: 9,
            fps: 200,
            bitrate_kbps: 100,
            width: 0,
            height: 0,
            ..StreamSettings::default()
        };
        stream.clamp_to_safe_ranges();
        assert_eq!(stream.monitor, 4);
        assert_eq!(stream.fps, 120);
        assert_eq!(stream.bitrate_kbps, 500);
        assert_eq!((stream.width, stream.height), (1920, 1080));
    }

    #[test]
    fn load_rejects_missing_file() {
        assert!(load("definitely-not-a-config.yaml").is_err());
    }

    #[test]
    fn load_parses_and_validates() {
        let path = std::env::temp_dir().join("viewpoint-test.yaml");
        std::fs::write(
            &path,
            "server:\n  public_url: http://example:8080\n  control_url: http://127.0.0.1:8080\nstream:\n  codec: h264\n",
        )
        .expect("write test config");
        let settings = load(path.to_str().expect("utf8 path")).expect("load test config");
        assert_eq!(settings.server.public_url, "http://example:8080");
        let _ = std::fs::remove_file(&path);
    }
}
