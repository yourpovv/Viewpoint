package signal

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"sync"
	"time"
)

const MaxSDPBytes = 1024 * 1024

const MaxSessionsPerLink = 16

var ErrSessionLimit = errors.New("session limit reached")

type Session struct {
	ID     string
	Offer  string
	Answer string
	Leaves uint64
}

type Link struct {
	ID      string
	Expires time.Time

	sessions map[string]*Session
}

type Registry struct {
	mu    sync.RWMutex
	links map[string]Link
}

func NewRegistry() *Registry {
	return &Registry{links: make(map[string]Link)}
}

func (r *Registry) Create(ttl time.Duration) (Link, error) {
	id, err := randomID()
	if err != nil {
		return Link{}, err
	}
	now := time.Now()
	link := Link{ID: id, Expires: now.Add(ttl), sessions: make(map[string]*Session)}
	r.mu.Lock()
	defer r.mu.Unlock()
	r.removeExpired(now)
	r.links[id] = link
	return link, nil
}

func (r *Registry) removeExpired(now time.Time) {
	for id, link := range r.links {
		if now.After(link.Expires) {
			delete(r.links, id)
		}
	}
}

func (r *Registry) Sweep(now time.Time) int {
	r.mu.Lock()
	defer r.mu.Unlock()
	before := len(r.links)
	r.removeExpired(now)
	return before - len(r.links)
}

func (r *Registry) Count() int {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return len(r.links)
}

func (r *Registry) Resolve(id string) (Link, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return liveLink(r.links, id, time.Now())
}

func (r *Registry) CreateSession(linkID string) (Session, error) {
	id, err := randomID()
	if err != nil {
		return Session{}, err
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	link, err := liveLink(r.links, linkID, time.Now())
	if err != nil {
		return Session{}, err
	}
	if len(link.sessions) >= MaxSessionsPerLink {
		return Session{}, ErrSessionLimit
	}
	session := &Session{ID: id}
	link.sessions[id] = session
	r.links[linkID] = link
	return *session, nil
}

func (r *Registry) ListSessions(linkID string) ([]string, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	link, err := liveLink(r.links, linkID, time.Now())
	if err != nil {
		return nil, err
	}
	ids := make([]string, 0, len(link.sessions))
	for id := range link.sessions {
		ids = append(ids, id)
	}
	return ids, nil
}

func (r *Registry) DropSession(linkID, sessionID string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if link, err := liveLink(r.links, linkID, time.Now()); err == nil {
		delete(link.sessions, sessionID)
		r.links[linkID] = link
	}
}

func (r *Registry) PublishOffer(linkID, sessionID, offer string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return err
	}
	session.Offer = offer
	session.Answer = ""
	return nil
}

func (r *Registry) PublishAnswer(linkID, sessionID, answer string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return err
	}
	session.Answer = answer
	return nil
}

func (r *Registry) TakeOffer(linkID, sessionID string) (string, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return "", err
	}
	return session.Offer, nil
}

func (r *Registry) TakeAnswer(linkID, sessionID string) (string, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return "", err
	}
	return session.Answer, nil
}

func (r *Registry) Leave(linkID, sessionID string) error {
	r.mu.Lock()
	defer r.mu.Unlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return err
	}
	session.Leaves++
	return nil
}

func (r *Registry) LeaveCount(linkID, sessionID string) (uint64, error) {
	r.mu.RLock()
	defer r.mu.RUnlock()
	session, err := liveSession(r.links, linkID, sessionID, time.Now())
	if err != nil {
		return 0, err
	}
	return session.Leaves, nil
}

func liveLink(links map[string]Link, id string, now time.Time) (Link, error) {
	link, exists := links[id]
	if !exists {
		return Link{}, errors.New("link not found")
	}
	if now.After(link.Expires) {
		return Link{}, errors.New("link expired")
	}
	return link, nil
}

func liveSession(links map[string]Link, linkID, sessionID string, now time.Time) (*Session, error) {
	link, err := liveLink(links, linkID, now)
	if err != nil {
		return nil, err
	}
	session, exists := link.sessions[sessionID]
	if !exists {
		return nil, errors.New("session not found")
	}
	return session, nil
}

func (r *Registry) Revoke(id string) {
	r.mu.Lock()
	defer r.mu.Unlock()
	delete(r.links, id)
}

func randomID() (string, error) {
	var raw [16]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", errors.New("generate link id")
	}
	return hex.EncodeToString(raw[:]), nil
}
