package engram

import (
	"context"
	"fmt"
)

// SkillsClient covers skill management operations.
//
// Access via Client.Skills.
type SkillsClient struct {
	c *Client
}

// List returns available skills.
//
// GET /skills
func (s *SkillsClient) List(ctx context.Context, limit, offset *int) ([]SkillRef, error) {
	p := newParams()
	if limit != nil {
		p.addInt("limit", *limit)
	}
	if offset != nil {
		p.addInt("offset", *offset)
	}
	path := "/skills" + p.encode()

	var raw interface{}
	if err := s.c.get(ctx, path, &raw); err != nil {
		return nil, err
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[SkillRef](v)
	case map[string]interface{}:
		if data, ok := v["data"].([]interface{}); ok {
			return decodeSlice[SkillRef](data)
		}
	}
	return nil, nil
}

// Get fetches full skill detail by ID.
//
// GET /skills/{id}
func (s *SkillsClient) Get(ctx context.Context, id int64) (SkillDetail, error) {
	var out SkillDetail
	err := s.c.get(ctx, fmt.Sprintf("/skills/%d", id), &out)
	return out, err
}

// Search finds skills by query.
//
// POST /skills/search
func (s *SkillsClient) Search(ctx context.Context, query string, limit *int) ([]SkillRef, error) {
	body := map[string]interface{}{"query": query}
	if limit != nil {
		body["limit"] = *limit
	}
	var raw interface{}
	if err := s.c.post(ctx, "/skills/search", body, &raw); err != nil {
		return nil, err
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[SkillRef](v)
	case map[string]interface{}:
		if data, ok := v["data"].([]interface{}); ok {
			return decodeSlice[SkillRef](data)
		}
	}
	return nil, nil
}

// Execute records a skill execution.
//
// POST /skills/{id}/execute
func (s *SkillsClient) Execute(ctx context.Context, id int64, payload map[string]interface{}) error {
	return s.c.post(ctx, fmt.Sprintf("/skills/%d/execute", id), payload, nil)
}

// Delete removes a skill.
//
// DELETE /skills/{id}
func (s *SkillsClient) Delete(ctx context.Context, id int64) error {
	return s.c.del(ctx, fmt.Sprintf("/skills/%d", id))
}
