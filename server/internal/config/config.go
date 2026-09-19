package config

import (
	"fmt"
	"os"
	"time"

	"gopkg.in/yaml.v3"
)

type Config struct {
	Server struct {
		Host       string `yaml:"host"`
		Port       int    `yaml:"port"`
		PublicURL  string `yaml:"public_url"`
		ControlURL string `yaml:"control_url"`
	} `yaml:"server"`
	Auth struct {
		LinkTTLHours    int `yaml:"link_ttl_hours"`
		LinkTTLMinutes  int `yaml:"link_ttl_minutes"`
		MaxLinkTTLHours int `yaml:"max_link_ttl_hours"`
	} `yaml:"auth"`
	WebRTC struct {
		StunURLs     []string `yaml:"stun_urls"`
		TurnURL      string   `yaml:"turn_url"`
		TurnUsername string   `yaml:"turn_username"`
	} `yaml:"webrtc"`
	Stream struct {
		Monitor     int    `yaml:"monitor"`
		Width       int    `yaml:"width"`
		Height      int    `yaml:"height"`
		FPS         int    `yaml:"fps"`
		BitrateKbps int    `yaml:"bitrate_kbps"`
		Codec       string `yaml:"codec"`
	} `yaml:"stream"`

	JWTSecret  string `yaml:"-"`
	TurnSecret string `yaml:"-"`
}

func Load(path string) (Config, error) {
	if override := os.Getenv("VIEWPOINT_CONFIG"); override != "" {
		path = override
	}

	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return Config{}, fmt.Errorf("config %s not found: copy viewpoint.example.yaml to %s and retry", path, path)
		}
		return Config{}, fmt.Errorf("read config %s: %w", path, err)
	}

	var cfg Config
	applyDefaults(&cfg)
	if err := yaml.Unmarshal(data, &cfg); err != nil {
		return Config{}, fmt.Errorf("parse config %s: %w", path, err)
	}

	cfg.JWTSecret = os.Getenv("VIEWPOINT_JWT_SECRET")
	cfg.TurnSecret = os.Getenv("VIEWPOINT_TURN_SECRET")
	if cfg.JWTSecret == "" {
		return Config{}, fmt.Errorf("missing required env: VIEWPOINT_JWT_SECRET")
	}

	if err := validate(cfg); err != nil {
		return Config{}, err
	}
	return cfg, nil
}

func (c Config) LinkTTL() time.Duration {
	if c.Auth.LinkTTLMinutes >= 1 {
		return time.Duration(c.Auth.LinkTTLMinutes) * time.Minute
	}
	return time.Duration(c.Auth.LinkTTLHours) * time.Hour
}

func applyDefaults(cfg *Config) {
	cfg.Server.Host = "0.0.0.0"
	cfg.Server.Port = 8080
	cfg.Server.PublicURL = "http://localhost:8080"
	cfg.Server.ControlURL = "http://127.0.0.1:8080"
	cfg.Auth.LinkTTLHours = 24
	cfg.Auth.MaxLinkTTLHours = 168
	cfg.WebRTC.StunURLs = []string{"stun:stun.l.google.com:19302"}
	cfg.WebRTC.TurnUsername = "viewpoint"
	cfg.Stream.Monitor = 1
	cfg.Stream.Width = 1920
	cfg.Stream.Height = 1080
	cfg.Stream.FPS = 30
	cfg.Stream.BitrateKbps = 6000
	cfg.Stream.Codec = "h264"
}

func validate(cfg Config) error {
	if cfg.Server.Port < 1 || cfg.Server.Port > 65535 {
		return fmt.Errorf("server.port out of range: %d", cfg.Server.Port)
	}
	if cfg.Auth.LinkTTLHours < 1 {
		return fmt.Errorf("auth.link_ttl_hours must be >= 1")
	}
	if cfg.Auth.MaxLinkTTLHours < 1 {
		return fmt.Errorf("auth.max_link_ttl_hours must be >= 1")
	}
	if cfg.Auth.LinkTTLHours > cfg.Auth.MaxLinkTTLHours {
		return fmt.Errorf("auth.link_ttl_hours must be <= max_link_ttl_hours")
	}
	if cfg.Auth.LinkTTLMinutes < 0 || cfg.Auth.LinkTTLMinutes > cfg.Auth.MaxLinkTTLHours*60 {
		return fmt.Errorf("auth.link_ttl_minutes must be 0 (unset) or within 1..=max_link_ttl_hours in minutes")
	}
	if cfg.Stream.FPS < 15 || cfg.Stream.FPS > 120 {
		return fmt.Errorf("stream.fps out of range: %d", cfg.Stream.FPS)
	}
	if cfg.Stream.BitrateKbps < 500 || cfg.Stream.BitrateKbps > 50000 {
		return fmt.Errorf("stream.bitrate_kbps out of range: %d", cfg.Stream.BitrateKbps)
	}
	if cfg.Stream.Codec != "h264" && cfg.Stream.Codec != "vp8" {
		return fmt.Errorf("stream.codec must be h264 or vp8, got %q", cfg.Stream.Codec)
	}
	return nil
}
