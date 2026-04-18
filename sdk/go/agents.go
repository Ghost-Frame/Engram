package engram

import (
	"context"
	"fmt"
)

// AgentsClient covers agent management operations.
//
// Access via Client.Agents.
type AgentsClient struct {
	c *Client
}

// List returns all registered agents.
//
// GET /agents
func (a *AgentsClient) List(ctx context.Context) ([]Agent, error) {
	var raw interface{}
	if err := a.c.get(ctx, "/agents", &raw); err != nil {
		return nil, err
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[Agent](v)
	case map[string]interface{}:
		if data, ok := v["data"].([]interface{}); ok {
			return decodeSlice[Agent](data)
		}
	}
	return nil, nil
}

// Create registers a new agent.
//
// POST /agents
func (a *AgentsClient) Create(ctx context.Context, req CreateAgentRequest) (Agent, error) {
	var out Agent
	err := a.c.post(ctx, "/agents", req, &out)
	return out, err
}

// Get fetches an agent by ID.
//
// GET /agents/{id}
func (a *AgentsClient) Get(ctx context.Context, id int64) (Agent, error) {
	var out Agent
	err := a.c.get(ctx, fmt.Sprintf("/agents/%d", id), &out)
	return out, err
}

// Revoke deactivates an agent.
//
// POST /agents/{id}/revoke
func (a *AgentsClient) Revoke(ctx context.Context, id int64) error {
	return a.c.post(ctx, fmt.Sprintf("/agents/%d/revoke", id), nil, nil)
}

// Passport issues a signed token for an agent.
//
// GET /agents/{id}/passport
func (a *AgentsClient) Passport(ctx context.Context, id int64) (AgentPassport, error) {
	var out AgentPassport
	err := a.c.get(ctx, fmt.Sprintf("/agents/%d/passport", id), &out)
	return out, err
}

// LinkKey associates an API key with an agent.
//
// POST /agents/{id}/link-key
func (a *AgentsClient) LinkKey(ctx context.Context, agentID int64, keyID int64) error {
	body := map[string]interface{}{"key_id": keyID}
	return a.c.post(ctx, fmt.Sprintf("/agents/%d/link-key", agentID), body, nil)
}
