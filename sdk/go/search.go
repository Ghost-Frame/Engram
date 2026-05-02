package kleos

import (
	"context"
	"fmt"
)

// SearchClient covers memory search operations.
//
// Access via Client.Search.
type SearchClient struct {
	c *Client
}

// Search performs a hybrid vector + full-text search.
//
// POST /memories/search
func (s *SearchClient) Search(ctx context.Context, req SearchRequest) ([]SearchResult, error) {
	var raw interface{}
	if err := s.c.post(ctx, "/memories/search", req, &raw); err != nil {
		return nil, err
	}
	return unmarshalSearchResults(raw)
}

// Faceted performs a faceted search with aggregations.
//
// POST /search/faceted
func (s *SearchClient) Faceted(ctx context.Context, req SearchRequest) (map[string]interface{}, error) {
	var out map[string]interface{}
	err := s.c.post(ctx, "/search/faceted", req, &out)
	return out, err
}

// Explain performs a search with per-result score breakdown.
//
// POST /search/explain
func (s *SearchClient) Explain(ctx context.Context, req SearchRequest) ([]SearchResult, error) {
	var raw interface{}
	if err := s.c.post(ctx, "/search/explain", req, &raw); err != nil {
		return nil, err
	}
	return unmarshalSearchResults(raw)
}

// unmarshalSearchResults handles a bare []SearchResult or {"results":[...]}.
func unmarshalSearchResults(raw interface{}) ([]SearchResult, error) {
	if raw == nil {
		return nil, nil
	}
	switch v := raw.(type) {
	case []interface{}:
		return decodeSlice[SearchResult](v)
	case map[string]interface{}:
		data, ok := v["results"]
		if !ok {
			return nil, nil
		}
		if arr, ok := data.([]interface{}); ok {
			return decodeSlice[SearchResult](arr)
		}
	}
	return nil, fmt.Errorf("engram: unexpected search result shape")
}
