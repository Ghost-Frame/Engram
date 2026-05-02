package kleos

import (
	"context"
	"fmt"
)

// GraphClient covers graph/knowledge-graph operations.
//
// Access via Client.Graph.
type GraphClient struct {
	c *Client
}

// Neighborhood returns the graph neighbourhood around a memory node.
//
// GET /graph/neighborhood/{id}
func (g *GraphClient) Neighborhood(ctx context.Context, memoryID int64, depth *int) (NeighborhoodResponse, error) {
	path := fmt.Sprintf("/graph/neighborhood/%d", memoryID)
	if depth != nil {
		path += fmt.Sprintf("?depth=%d", *depth)
	}
	var out NeighborhoodResponse
	err := g.c.get(ctx, path, &out)
	return out, err
}

// Search searches the graph by query.
//
// POST /graph/search
func (g *GraphClient) Search(ctx context.Context, query string, limit *int) (map[string]interface{}, error) {
	body := map[string]interface{}{"query": query}
	if limit != nil {
		body["limit"] = *limit
	}
	var out map[string]interface{}
	err := g.c.post(ctx, "/graph/search", body, &out)
	return out, err
}

// Build triggers a graph rebuild.
//
// POST /graph/build
func (g *GraphClient) Build(ctx context.Context) error {
	return g.c.post(ctx, "/graph/build", nil, nil)
}

// EntityRelationships lists relationships for an entity.
//
// GET /entities/{id}/relationships
func (g *GraphClient) EntityRelationships(ctx context.Context, entityID int64) (map[string]interface{}, error) {
	var out map[string]interface{}
	err := g.c.get(ctx, fmt.Sprintf("/entities/%d/relationships", entityID), &out)
	return out, err
}
