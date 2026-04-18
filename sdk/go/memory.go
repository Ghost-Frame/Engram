package engram

import (
	"context"
	"fmt"
)

// MemoryClient covers CRUD operations on memory records.
//
// Access via Client.Memory.
type MemoryClient struct {
	c *Client
}

// Store creates a new memory record.
//
// POST /memories
func (m *MemoryClient) Store(ctx context.Context, req StoreRequest) (StoreResult, error) {
	var out StoreResult
	err := m.c.post(ctx, "/memories", req, &out)
	return out, err
}

// Get fetches a single memory by ID.
//
// GET /memory/{id}
func (m *MemoryClient) Get(ctx context.Context, id int64) (Memory, error) {
	var out Memory
	err := m.c.get(ctx, fmt.Sprintf("/memory/%d", id), &out)
	return out, err
}

// List retrieves memories with optional filtering.
//
// GET /list
func (m *MemoryClient) List(ctx context.Context, opts ListOptions) ([]Memory, error) {
	path := "/list" + opts.queryString()
	// Server may return a list directly or wrap it in {"data": [...]}
	var raw interface{}
	if err := m.c.get(ctx, path, &raw); err != nil {
		return nil, err
	}
	return unmarshalMemoryList(raw)
}

// Update modifies fields on an existing memory.
//
// POST /memory/{id}/update
func (m *MemoryClient) Update(ctx context.Context, id int64, req UpdateRequest) (Memory, error) {
	var out Memory
	err := m.c.post(ctx, fmt.Sprintf("/memory/%d/update", id), req, &out)
	return out, err
}

// Delete hard-deletes a memory by ID.
//
// DELETE /memory/{id}
func (m *MemoryClient) Delete(ctx context.Context, id int64) error {
	return m.c.del(ctx, fmt.Sprintf("/memory/%d", id))
}

// Forget soft-deletes (forgets) a memory with an optional reason.
//
// POST /memory/{id}/forget
func (m *MemoryClient) Forget(ctx context.Context, id int64, reason string) error {
	body := map[string]interface{}{}
	if reason != "" {
		body["reason"] = reason
	}
	return m.c.post(ctx, fmt.Sprintf("/memory/%d/forget", id), body, nil)
}

// Archive archives a memory.
//
// POST /memory/{id}/archive
func (m *MemoryClient) Archive(ctx context.Context, id int64) error {
	return m.c.post(ctx, fmt.Sprintf("/memory/%d/archive", id), nil, nil)
}

// ----------------------------------------------------------------------------
// ListOptions
// ----------------------------------------------------------------------------

// ListOptions are query parameters for the List endpoint.
type ListOptions struct {
	Limit            *int
	Offset           *int
	Category         string
	Source           string
	SpaceID          *int64
	IncludeForgotten *bool
	IncludeArchived  *bool
}

func (o ListOptions) queryString() string {
	p := newParams()
	if o.Limit != nil {
		p.addInt("limit", *o.Limit)
	}
	if o.Offset != nil {
		p.addInt("offset", *o.Offset)
	}
	if o.Category != "" {
		p.add("category", o.Category)
	}
	if o.Source != "" {
		p.add("source", o.Source)
	}
	if o.SpaceID != nil {
		p.addInt64("space_id", *o.SpaceID)
	}
	if o.IncludeForgotten != nil && *o.IncludeForgotten {
		p.add("include_forgotten", "true")
	}
	if o.IncludeArchived != nil && *o.IncludeArchived {
		p.add("include_archived", "true")
	}
	return p.encode()
}

// unmarshalMemoryList handles both a bare []Memory and a {"data":[...]} envelope.
func unmarshalMemoryList(raw interface{}) ([]Memory, error) {
	if raw == nil {
		return nil, nil
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[Memory](v)
	case map[string]interface{}:
		data, ok := v["data"]
		if !ok {
			data = v["memories"]
		}
		if data == nil {
			return nil, nil
		}
		if arr, ok := data.([]interface{}); ok {
			return decodeSlice[Memory](arr)
		}
	}
	return nil, fmt.Errorf("engram: unexpected memory list shape")
}
