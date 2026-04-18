package engram

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const defaultTimeout = 30 * time.Second

// Client is the Engram API client.
//
// All methods accept a context.Context as their first argument so callers can
// apply deadlines or cancellation.
//
// Example:
//
//	c := engram.NewClient("http://localhost:4200", "ek-your-key")
//	result, err := c.Memory.Store(ctx, engram.StoreRequest{Content: "a fact"})
type Client struct {
	// BaseURL is the root URL of the Engram server (no trailing slash).
	BaseURL string
	// APIKey is the Bearer token used for all requests.
	APIKey string
	// HTTP is the underlying transport. Override for testing or custom timeouts.
	HTTP *http.Client

	// Endpoint groups -- all wired to this Client.
	Memory  *MemoryClient
	Search  *SearchClient
	Context *ContextClient
	Agents  *AgentsClient
	Graph   *GraphClient
	Skills  *SkillsClient
	Auth    *AuthClient
	Batch   *BatchClient
}

// NewClient creates a Client with sensible defaults.
func NewClient(baseURL, apiKey string) *Client {
	c := &Client{
		BaseURL: strings.TrimRight(baseURL, "/"),
		APIKey:  apiKey,
		HTTP:    &http.Client{Timeout: defaultTimeout},
	}
	c.Memory = &MemoryClient{c}
	c.Search = &SearchClient{c}
	c.Context = &ContextClient{c}
	c.Agents = &AgentsClient{c}
	c.Graph = &GraphClient{c}
	c.Skills = &SkillsClient{c}
	c.Auth = &AuthClient{c}
	c.Batch = &BatchClient{c}
	return c
}

// NewClientWithHTTP creates a Client using a custom http.Client.
func NewClientWithHTTP(baseURL, apiKey string, httpClient *http.Client) *Client {
	c := NewClient(baseURL, apiKey)
	c.HTTP = httpClient
	return c
}

// ----------------------------------------------------------------------------
// Internal request helpers
// ----------------------------------------------------------------------------

func (c *Client) do(ctx context.Context, method, path string, body interface{}) (*http.Response, error) {
	var bodyReader io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, fmt.Errorf("engram: marshal request body: %w", err)
		}
		bodyReader = bytes.NewReader(b)
	}

	req, err := http.NewRequestWithContext(ctx, method, c.BaseURL+path, bodyReader)
	if err != nil {
		return nil, fmt.Errorf("engram: build request: %w", err)
	}
	req.Header.Set("Authorization", "Bearer "+c.APIKey)
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}

	return c.HTTP.Do(req)
}

// decode reads the HTTP response body into dst, and returns an EngramError for
// non-2xx responses. dst may be nil if the caller does not need the body.
func (c *Client) decode(resp *http.Response, dst interface{}) error {
	defer resp.Body.Close()
	raw, err := io.ReadAll(resp.Body)
	if err != nil {
		return fmt.Errorf("engram: read response body: %w", err)
	}

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		var apiErr APIError
		_ = json.Unmarshal(raw, &apiErr)
		return &EngramError{
			StatusCode: resp.StatusCode,
			Message:    http.StatusText(resp.StatusCode),
			Body:       &apiErr,
		}
	}

	if dst == nil || len(raw) == 0 {
		return nil
	}
	if err := json.Unmarshal(raw, dst); err != nil {
		return fmt.Errorf("engram: decode response: %w", err)
	}
	return nil
}

// get is a convenience wrapper for GET requests that decode into dst.
func (c *Client) get(ctx context.Context, path string, dst interface{}) error {
	resp, err := c.do(ctx, http.MethodGet, path, nil)
	if err != nil {
		return err
	}
	return c.decode(resp, dst)
}

// post is a convenience wrapper for POST requests.
func (c *Client) post(ctx context.Context, path string, body, dst interface{}) error {
	resp, err := c.do(ctx, http.MethodPost, path, body)
	if err != nil {
		return err
	}
	return c.decode(resp, dst)
}

// patch is a convenience wrapper for PATCH requests.
func (c *Client) patch(ctx context.Context, path string, body, dst interface{}) error {
	resp, err := c.do(ctx, http.MethodPatch, path, body)
	if err != nil {
		return err
	}
	return c.decode(resp, dst)
}

// del is a convenience wrapper for DELETE requests.
func (c *Client) del(ctx context.Context, path string) error {
	resp, err := c.do(ctx, http.MethodDelete, path, nil)
	if err != nil {
		return err
	}
	return c.decode(resp, nil)
}

// RawGet issues an authenticated GET to any path and decodes the response.
// Use this for endpoints not covered by the high-level methods.
func (c *Client) RawGet(ctx context.Context, path string, dst interface{}) error {
	return c.get(ctx, path, dst)
}

// RawPost issues an authenticated POST to any path.
func (c *Client) RawPost(ctx context.Context, path string, body, dst interface{}) error {
	return c.post(ctx, path, body, dst)
}

// RawDelete issues an authenticated DELETE to any path.
func (c *Client) RawDelete(ctx context.Context, path string) error {
	return c.del(ctx, path)
}
