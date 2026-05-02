package kleos

import (
	"context"
	"fmt"
)

// AuthClient covers API key management.
//
// Access via Client.Auth.
type AuthClient struct {
	c *Client
}

// ListKeys returns all API keys for the current user.
//
// GET /keys
func (a *AuthClient) ListKeys(ctx context.Context) ([]APIKey, error) {
	var raw interface{}
	if err := a.c.get(ctx, "/keys", &raw); err != nil {
		return nil, err
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[APIKey](v)
	case map[string]interface{}:
		if data, ok := v["data"].([]interface{}); ok {
			return decodeSlice[APIKey](data)
		}
	}
	return nil, nil
}

// CreateKey creates a new API key. The plaintext key is returned once.
//
// POST /keys
func (a *AuthClient) CreateKey(ctx context.Context, req CreateKeyRequest) (CreateKeyResponse, error) {
	var out CreateKeyResponse
	err := a.c.post(ctx, "/keys", req, &out)
	return out, err
}

// RevokeKey permanently revokes an API key.
//
// DELETE /keys/{id}
func (a *AuthClient) RevokeKey(ctx context.Context, id int64) error {
	return a.c.del(ctx, fmt.Sprintf("/keys/%d", id))
}

// RotateKey rotates an existing key and returns the new value.
//
// POST /keys/rotate
func (a *AuthClient) RotateKey(ctx context.Context, id int64) (CreateKeyResponse, error) {
	var out CreateKeyResponse
	body := map[string]interface{}{"key_id": id}
	err := a.c.post(ctx, "/keys/rotate", body, &out)
	return out, err
}
