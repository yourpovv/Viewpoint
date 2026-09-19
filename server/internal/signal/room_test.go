package signal

import (
	"testing"
	"time"
)

func TestCreateResolve(t *testing.T) {
	registry := NewRegistry()
	link, err := registry.Create(time.Hour)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	if link.ID == "" {
		t.Fatal("expected non-empty id")
	}
	resolved, err := registry.Resolve(link.ID)
	if err != nil {
		t.Fatalf("Resolve: %v", err)
	}
	if resolved.ID != link.ID {
		t.Fatalf("got %q want %q", resolved.ID, link.ID)
	}
}

func TestExpiry(t *testing.T) {
	registry := NewRegistry()
	link, err := registry.Create(time.Millisecond)
	if err != nil {
		t.Fatalf("Create: %v", err)
	}
	time.Sleep(5 * time.Millisecond)
	if _, err := registry.Resolve(link.ID); err == nil {
		t.Fatal("expected expired link to fail")
	}
	if swept := registry.Sweep(time.Now()); swept != 1 {
		t.Fatalf("Sweep = %d want 1", swept)
	}
	if count := registry.Count(); count != 0 {
		t.Fatalf("Count = %d want 0", count)
	}
}

func TestSessionOfferAnswer(t *testing.T) {
	registry := NewRegistry()
	link, _ := registry.Create(time.Hour)
	first, err := registry.CreateSession(link.ID)
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	second, err := registry.CreateSession(link.ID)
	if err != nil {
		t.Fatalf("CreateSession: %v", err)
	}
	if first.ID == second.ID {
		t.Fatal("expected distinct session ids")
	}
	ids, err := registry.ListSessions(link.ID)
	if err != nil {
		t.Fatalf("ListSessions: %v", err)
	}
	if len(ids) != 2 {
		t.Fatalf("ListSessions = %d ids, want 2", len(ids))
	}
	if err := registry.PublishOffer(link.ID, first.ID, "offer-a"); err != nil {
		t.Fatalf("PublishOffer: %v", err)
	}
	if err := registry.PublishOffer(link.ID, second.ID, "offer-b"); err != nil {
		t.Fatalf("PublishOffer: %v", err)
	}
	if got, _ := registry.TakeOffer(link.ID, first.ID); got != "offer-a" {
		t.Fatalf("TakeOffer first = %q", got)
	}
	if got, _ := registry.TakeOffer(link.ID, second.ID); got != "offer-b" {
		t.Fatalf("TakeOffer second = %q", got)
	}
	if err := registry.PublishAnswer(link.ID, first.ID, "answer-a"); err != nil {
		t.Fatalf("PublishAnswer: %v", err)
	}
	if got, _ := registry.TakeAnswer(link.ID, first.ID); got != "answer-a" {
		t.Fatalf("TakeAnswer = %q", got)
	}
	if got, _ := registry.TakeAnswer(link.ID, second.ID); got != "" {
		t.Fatalf("TakeAnswer second = %q, want empty", got)
	}
	if err := registry.PublishOffer(link.ID, first.ID, "offer-a2"); err != nil {
		t.Fatalf("PublishOffer: %v", err)
	}
	if got, _ := registry.TakeAnswer(link.ID, first.ID); got != "" {
		t.Fatalf("TakeAnswer after re-offer = %q, want empty", got)
	}
	registry.DropSession(link.ID, first.ID)
	if ids, _ := registry.ListSessions(link.ID); len(ids) != 1 {
		t.Fatalf("ListSessions after drop = %d ids, want 1", len(ids))
	}
}

func TestSessionLeaveCount(t *testing.T) {
	registry := NewRegistry()
	link, _ := registry.Create(time.Hour)
	session, _ := registry.CreateSession(link.ID)
	for i := uint64(1); i <= 3; i++ {
		if err := registry.Leave(link.ID, session.ID); err != nil {
			t.Fatalf("Leave: %v", err)
		}
		count, err := registry.LeaveCount(link.ID, session.ID)
		if err != nil {
			t.Fatalf("LeaveCount: %v", err)
		}
		if count != i {
			t.Fatalf("LeaveCount = %d want %d", count, i)
		}
	}
	if err := registry.Leave(link.ID, "nope"); err == nil {
		t.Fatal("expected leave of missing session to fail")
	}
	if err := registry.Leave("nope", session.ID); err == nil {
		t.Fatal("expected leave of missing link to fail")
	}
}

func TestRevoke(t *testing.T) {
	registry := NewRegistry()
	link, _ := registry.Create(time.Hour)
	registry.Revoke(link.ID)
	if _, err := registry.Resolve(link.ID); err == nil {
		t.Fatal("expected revoked link to fail")
	}
}

func TestMissingLink(t *testing.T) {
	registry := NewRegistry()
	if _, err := registry.Resolve("nope"); err == nil {
		t.Fatal("expected missing link to fail")
	}
	if _, err := registry.CreateSession("nope"); err == nil {
		t.Fatal("expected session on missing link to fail")
	}
}
