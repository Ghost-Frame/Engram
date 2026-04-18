package engram

import (
	"bufio"
	"context"
	"fmt"
	"net/http"
	"strings"
)

// ContextClient covers context-assembly operations.
//
// Access via Client.Context.
type ContextClient struct {
	c *Client
}

// Build assembles ranked context blocks for injection into an LLM prompt.
//
// POST /context
func (cc *ContextClient) Build(ctx context.Context, req ContextRequest) (ContextResponse, error) {
	var out ContextResponse
	err := cc.c.post(ctx, "/context", req, &out)
	return out, err
}

// Stream assembles context and streams results as server-sent events.
//
// The returned channel receives raw SSE lines (e.g. "data: {...}"). The
// channel is closed when the stream ends or ctx is cancelled. The returned
// error channel receives any transport-level error.
//
// POST /context/stream
func (cc *ContextClient) Stream(ctx context.Context, req ContextRequest) (<-chan string, <-chan error) {
	lines := make(chan string, 64)
	errs := make(chan error, 1)

	go func() {
		defer close(lines)
		defer close(errs)

		resp, err := cc.c.do(ctx, http.MethodPost, "/context/stream", req)
		if err != nil {
			errs <- err
			return
		}
		defer resp.Body.Close()

		if resp.StatusCode < 200 || resp.StatusCode >= 300 {
			errs <- &EngramError{
				StatusCode: resp.StatusCode,
				Message:    fmt.Sprintf("stream request failed: %s", http.StatusText(resp.StatusCode)),
			}
			return
		}

		scanner := bufio.NewScanner(resp.Body)
		for scanner.Scan() {
			line := scanner.Text()
			if strings.HasPrefix(line, "data:") {
				select {
				case lines <- strings.TrimPrefix(line, "data:"):
				case <-ctx.Done():
					return
				}
			}
		}
		if err := scanner.Err(); err != nil {
			errs <- fmt.Errorf("engram: stream read: %w", err)
		}
	}()

	return lines, errs
}
