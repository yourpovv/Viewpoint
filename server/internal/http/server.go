package http

import (
	"crypto/hmac"
	"crypto/sha1"
	"crypto/subtle"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/yourpovv/viewpoint/server/internal/config"
	"github.com/yourpovv/viewpoint/server/internal/signal"
)

type Server struct {
	cfg    config.Config
	links  *signal.Registry
	mux    *http.ServeMux
	web    http.Handler
	webDir string
}

func NewServer(cfg config.Config, links *signal.Registry, webDir string) *Server {
	s := &Server{cfg: cfg, links: links, mux: http.NewServeMux()}
	if webDir == "" {
		webDir = "web"
	}
	s.web = http.FileServer(http.Dir(webDir))
	s.webDir = webDir
	s.routes()
	return s
}

func (s *Server) routes() {
	s.mux.HandleFunc("/healthz", s.handleHealth)
	s.mux.HandleFunc("POST /api/links", s.handleLinks)
	s.mux.HandleFunc("DELETE /api/links/{id}", s.handleRevokeLink)
	s.mux.HandleFunc("GET /api/session/{id}/ice", s.handleIce)
	s.mux.HandleFunc("POST /api/links/{id}/sessions", s.handleCreateSession)
	s.mux.HandleFunc("GET /api/links/{id}/sessions", s.handleListSessions)
	s.mux.HandleFunc("DELETE /api/session/{link}/{session}", s.handleDropSession)
	s.mux.HandleFunc("POST /api/session/{link}/{session}/offer", s.handlePublishOffer)
	s.mux.HandleFunc("GET /api/session/{link}/{session}/offer", s.handleTakeOffer)
	s.mux.HandleFunc("POST /api/session/{link}/{session}/answer", s.handlePublishAnswer)
	s.mux.HandleFunc("GET /api/session/{link}/{session}/answer", s.handleTakeAnswer)
	s.mux.HandleFunc("POST /api/session/{link}/{session}/leave", s.handleLeave)
	s.mux.HandleFunc("GET /api/session/{link}/{session}/leave", s.handleLeaveCount)
	s.mux.HandleFunc("/v/", s.handleViewer)
	s.mux.HandleFunc("/", s.serveViewer)
}

func (s *Server) Start() error {
	go reapExpired(s.links)
	server := &http.Server{
		Addr:         s.cfg.Server.Host + ":" + strconv.Itoa(s.cfg.Server.Port),
		Handler:      s.mux,
		ReadTimeout:  10 * time.Second,
		WriteTimeout: 10 * time.Second,
	}
	return server.ListenAndServe()
}

func reapExpired(links *signal.Registry) {
	ticker := time.NewTicker(time.Minute)
	defer ticker.Stop()
	for now := range ticker.C {
		links.Sweep(now)
	}
}

func (s *Server) handleHealth(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
}

func (s *Server) handleLinks(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	link, err := s.links.Create(s.linkTTL(w, r))
	if err != nil {
		http.Error(w, "create link", http.StatusInternalServerError)
		return
	}
	writeJSON(w, http.StatusCreated, map[string]string{
		"id":         link.ID,
		"url":        s.cfg.Server.PublicURL + "/v/" + link.ID,
		"expires_at": link.Expires.Format(time.RFC3339),
	})
}

func (s *Server) linkTTL(w http.ResponseWriter, r *http.Request) time.Duration {
	var body struct {
		TTLHours   *int `json:"ttl_hours"`
		TTLMinutes *int `json:"ttl_minutes"`
	}
	r.Body = http.MaxBytesReader(w, r.Body, signal.MaxSDPBytes)
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		return s.cfg.LinkTTL()
	}
	maxMinutes := s.cfg.Auth.MaxLinkTTLHours * 60
	if body.TTLMinutes != nil && *body.TTLMinutes >= 1 {
		minutes := *body.TTLMinutes
		if minutes > maxMinutes {
			minutes = maxMinutes
		}
		return time.Duration(minutes) * time.Minute
	}
	if body.TTLHours == nil || *body.TTLHours < 1 {
		return s.cfg.LinkTTL()
	}
	if *body.TTLHours > s.cfg.Auth.MaxLinkTTLHours {
		return time.Duration(s.cfg.Auth.MaxLinkTTLHours) * time.Hour
	}
	return time.Duration(*body.TTLHours) * time.Hour
}

func (s *Server) handleViewer(w http.ResponseWriter, r *http.Request) {
	id := r.URL.Path[len("/v/"):]
	if _, err := s.links.Resolve(id); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	http.ServeFile(w, r, filepath.Join(s.webDir, "index.html"))
}

func (s *Server) serveViewer(w http.ResponseWriter, r *http.Request) {
	s.web.ServeHTTP(w, r)
}

func authorized(r *http.Request, secret string) bool {
	token, found := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer ")
	if !found || token == "" {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(token), []byte(secret)) == 1
}

func (s *Server) handleCreateSession(w http.ResponseWriter, r *http.Request) {
	link, err := s.links.Resolve(r.PathValue("id"))
	if err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	session, err := s.links.CreateSession(link.ID)
	if err != nil {
		if errors.Is(err, signal.ErrSessionLimit) {
			http.Error(w, "session limit reached", http.StatusTooManyRequests)
			return
		}
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	writeJSON(w, http.StatusCreated, map[string]string{
		"session_id": session.ID,
		"expires_at": link.Expires.Format(time.RFC3339),
	})
}

func (s *Server) handleDropSession(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	s.links.DropSession(link, session)
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleListSessions(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	ids, err := s.links.ListSessions(r.PathValue("id"))
	if err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	writeJSON(w, http.StatusOK, map[string][]string{"sessions": ids})
}

func (s *Server) handlePublishOffer(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	offer, ok := readSDP(w, r)
	if !ok {
		return
	}
	if err := s.links.PublishOffer(r.PathValue("link"), r.PathValue("session"), offer); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleTakeOffer(w http.ResponseWriter, r *http.Request) {
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	offer, err := s.links.TakeOffer(link, session)
	if err != nil {
		http.Error(w, "session not found", http.StatusNotFound)
		return
	}
	serveSDP(w, offer)
}

func (s *Server) handlePublishAnswer(w http.ResponseWriter, r *http.Request) {
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	answer, ok := readSDP(w, r)
	if !ok {
		return
	}
	if err := s.links.PublishAnswer(link, session, answer); err != nil {
		http.Error(w, "session not found", http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleTakeAnswer(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	answer, err := s.links.TakeAnswer(link, session)
	if err != nil {
		http.Error(w, "session not found", http.StatusNotFound)
		return
	}
	serveSDP(w, answer)
}

func readSDP(w http.ResponseWriter, r *http.Request) (string, bool) {
	var body struct {
		SDP string `json:"sdp"`
	}
	r.Body = http.MaxBytesReader(w, r.Body, signal.MaxSDPBytes)
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		http.Error(w, "invalid sdp", http.StatusBadRequest)
		return "", false
	}
	if body.SDP == "" {
		http.Error(w, "invalid sdp", http.StatusBadRequest)
		return "", false
	}
	return body.SDP, true
}

func (s *Server) handleRevokeLink(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	id := r.PathValue("id")
	if _, err := s.links.Resolve(id); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	s.links.Revoke(id)
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleIce(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	link, err := s.links.Resolve(id)
	if err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	writeJSON(w, http.StatusOK, s.iceResponse(link))
}

func (s *Server) iceResponse(link signal.Link) map[string]any {
	response := map[string]any{"stun_urls": s.cfg.WebRTC.StunURLs}
	if s.cfg.WebRTC.TurnURL == "" || s.cfg.TurnSecret == "" {
		return response
	}
	expiry := time.Now().Add(24 * time.Hour)
	if link.Expires.Before(expiry) {
		expiry = link.Expires
	}
	username := turnUsername(s.cfg.WebRTC.TurnUsername, expiry)
	response["turn_url"] = s.cfg.WebRTC.TurnURL
	response["turn_username"] = username
	response["turn_password"] = turnPassword(s.cfg.TurnSecret, username)
	response["expires_at"] = expiry.Format(time.RFC3339)
	return response
}

func turnUsername(base string, expiry time.Time) string {
	if base == "" {
		base = "viewpoint"
	}
	return fmt.Sprintf("%d:%s", expiry.Unix(), base)
}

func (s *Server) handleLeave(w http.ResponseWriter, r *http.Request) {
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	if err := s.links.Leave(link, session); err != nil {
		http.Error(w, "session not found", http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleLeaveCount(w http.ResponseWriter, r *http.Request) {
	if !authorized(r, s.cfg.JWTSecret) {
		http.Error(w, "unauthorized", http.StatusUnauthorized)
		return
	}
	link, session := r.PathValue("link"), r.PathValue("session")
	if _, err := s.links.Resolve(link); err != nil {
		http.Error(w, "link invalid or expired", http.StatusNotFound)
		return
	}
	count, err := s.links.LeaveCount(link, session)
	if err != nil {
		http.Error(w, "session not found", http.StatusNotFound)
		return
	}
	writeJSON(w, http.StatusOK, map[string]uint64{"leaves": count})
}

func turnPassword(secret, username string) string {
	mac := hmac.New(sha1.New, []byte(secret))
	_, _ = mac.Write([]byte(username))
	return base64.StdEncoding.EncodeToString(mac.Sum(nil))
}

func serveSDP(w http.ResponseWriter, sdp string) {
	if sdp == "" {
		w.WriteHeader(http.StatusNoContent)
		return
	}
	writeJSON(w, http.StatusOK, map[string]string{"sdp": sdp})
}

func writeJSON(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}
