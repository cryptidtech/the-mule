package the_mule

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"time"

	"github.com/redis/go-redis/v9"
)

// MuleClientBuilder constructs a MuleClient.
type MuleClientBuilder struct {
	redisURL string
	peerName string
}

// NewBuilder creates a new builder, reading defaults from REDIS_URL and PEER_NAME env vars.
func NewBuilder() *MuleClientBuilder {
	return &MuleClientBuilder{
		redisURL: os.Getenv("REDIS_URL"),
		peerName: os.Getenv("PEER_NAME"),
	}
}

// RedisURL sets the Redis URL.
func (b *MuleClientBuilder) RedisURL(url string) *MuleClientBuilder {
	b.redisURL = url
	return b
}

// PeerName sets the peer name.
func (b *MuleClientBuilder) PeerName(name string) *MuleClientBuilder {
	b.peerName = name
	return b
}

// Build connects to Redis, installs the slog handler, and starts the command listener.
func (b *MuleClientBuilder) Build(ctx context.Context) (*MuleClient, error) {
	if b.redisURL == "" {
		return nil, fmt.Errorf("REDIS_URL not set")
	}
	if b.peerName == "" {
		return nil, fmt.Errorf("PEER_NAME not set")
	}

	opts, err := redis.ParseURL(b.redisURL)
	if err != nil {
		return nil, fmt.Errorf("invalid REDIS_URL: %w", err)
	}

	rdb := redis.NewClient(opts)
	if err := rdb.Ping(ctx).Err(); err != nil {
		return nil, fmt.Errorf("failed to connect to Redis: %w", err)
	}

	statusKey := b.peerName + "_status"
	commandKey := b.peerName + "_command"
	logKey := b.peerName + "_log"

	// Install Redis slog handler
	handler := &redisLogHandler{rdb: rdb, logKey: logKey}
	slog.SetDefault(slog.New(handler))

	commands := make(chan string, 256)
	childCtx, cancel := context.WithCancel(ctx)

	// Start command poller goroutine
	go func() {
		defer close(commands)
		for {
			select {
			case <-childCtx.Done():
				return
			default:
			}

			result, err := rdb.BLPop(childCtx, 1*time.Second, commandKey).Result()
			if err != nil {
				if err == context.Canceled {
					return
				}
				continue
			}
			if len(result) >= 2 {
				select {
				case commands <- result[1]:
				case <-childCtx.Done():
					return
				}
			}
		}
	}()

	return &MuleClient{
		rdb:       rdb,
		statusKey: statusKey,
		commands:  commands,
		cancel:    cancel,
	}, nil
}

// MuleClient communicates with The Mule orchestrator via Redis.
type MuleClient struct {
	rdb       *redis.Client
	statusKey string
	commands  chan string
	cancel    context.CancelFunc
}

// Commands returns a channel that yields command strings from the orchestrator.
func (c *MuleClient) Commands() <-chan string {
	return c.commands
}

// SendStatus sends a status update to the orchestrator.
func (c *MuleClient) SendStatus(ctx context.Context, status string) error {
	return c.rdb.Set(ctx, c.statusKey, status, 0).Err()
}

// Close stops the command listener and closes the Redis connection.
func (c *MuleClient) Close() {
	c.cancel()
	_ = c.rdb.Close()
}

// redisLogHandler implements slog.Handler, forwarding log entries to Redis.
type redisLogHandler struct {
	rdb    *redis.Client
	logKey string
}

func (h *redisLogHandler) Enabled(_ context.Context, _ slog.Level) bool {
	return true
}

func (h *redisLogHandler) Handle(ctx context.Context, r slog.Record) error {
	level := "info"
	switch {
	case r.Level >= slog.LevelError:
		level = "error"
	case r.Level >= slog.LevelWarn:
		level = "warn"
	case r.Level >= slog.LevelInfo:
		level = "info"
	default:
		level = "debug"
	}

	entry := fmt.Sprintf("%s|%s", level, r.Message)
	return h.rdb.LPush(ctx, h.logKey, entry).Err()
}

func (h *redisLogHandler) WithAttrs(_ []slog.Attr) slog.Handler {
	return h
}

func (h *redisLogHandler) WithGroup(_ string) slog.Handler {
	return h
}
