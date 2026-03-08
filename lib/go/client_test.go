package the_mule

import (
	"context"
	"testing"
)

func TestBuilderMissingRedisURL(t *testing.T) {
	t.Setenv("REDIS_URL", "")
	t.Setenv("PEER_NAME", "test")

	builder := &MuleClientBuilder{redisURL: "", peerName: "test"}
	_, err := builder.Build(context.Background())
	if err == nil {
		t.Fatal("expected error for missing REDIS_URL")
	}
	if err.Error() != "REDIS_URL not set" {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestBuilderMissingPeerName(t *testing.T) {
	builder := &MuleClientBuilder{redisURL: "redis://localhost:6379", peerName: ""}
	_, err := builder.Build(context.Background())
	if err == nil {
		t.Fatal("expected error for missing PEER_NAME")
	}
	if err.Error() != "PEER_NAME not set" {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestNewBuilderReadsEnv(t *testing.T) {
	t.Setenv("REDIS_URL", "redis://myhost:1234")
	t.Setenv("PEER_NAME", "alice")

	b := NewBuilder()
	if b.redisURL != "redis://myhost:1234" {
		t.Fatalf("expected redis URL from env, got: %s", b.redisURL)
	}
	if b.peerName != "alice" {
		t.Fatalf("expected peer name from env, got: %s", b.peerName)
	}
}

func TestBuilderChaining(t *testing.T) {
	b := NewBuilder().RedisURL("redis://a:1").PeerName("bob")
	if b.redisURL != "redis://a:1" {
		t.Fatalf("expected chained redis URL, got: %s", b.redisURL)
	}
	if b.peerName != "bob" {
		t.Fatalf("expected chained peer name, got: %s", b.peerName)
	}
}
