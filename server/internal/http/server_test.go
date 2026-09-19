package http

import (
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/yourpovv/viewpoint/server/internal/config"
	"github.com/yourpovv/viewpoint/server/internal/signal"
)

func testServer() *Server {
	cfg := config.Config{}
	cfg.Server.PublicURL = "http://localhost:8080"
	cfg.Server.ControlURL = "http://127.0.0.1:8080"
	cfg.Auth.LinkTTLHours = 24
	cfg.Auth.MaxLinkTTLHours = 168
	return NewServer(cfg, signal.NewRegistry(), "")
}

func TestLinkTTLCapping(t *testing.T) {
	server := testServer()
	tests := []struct {
		name string
		body string
		want time.Duration
	}{
		{"empty body uses default", "", 24 * time.Hour},
		{"invalid json uses default", "{oops", 24 * time.Hour},
		{"empty object uses default", "{}", 24 * time.Hour},
		{"minutes win over default", `{"ttl_minutes": 10}`, 10 * time.Minute},
		{"hours honored", `{"ttl_hours": 2}`, 2 * time.Hour},
		{"minutes exceed max capped", `{"ttl_minutes": 99999}`, 168 * time.Hour},
		{"hours exceed max capped", `{"ttl_hours": 999}`, 168 * time.Hour},
		{"zero hours fall back to default", `{"ttl_hours": 0}`, 24 * time.Hour},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			request := httptest.NewRequest("POST", "/api/links", strings.NewReader(tt.body))
			recorder := httptest.NewRecorder()
			if got := server.linkTTL(recorder, request); got != tt.want {
				t.Errorf("linkTTL(%q) = %v, want %v", tt.body, got, tt.want)
			}
		})
	}
}
