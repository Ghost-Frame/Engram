package engram

import "context"

// BatchClient covers batch-operation execution.
//
// Access via Client.Batch.
type BatchClient struct {
	c *Client
}

// Execute runs multiple API operations in a single round-trip.
//
// POST /batch
//
// Example:
//
//	resp, err := client.Batch.Execute(ctx, engram.BatchRequest{
//	    Operations: []engram.BatchOperation{
//	        {Method: "POST", Path: "/memories", Body: map[string]interface{}{"content": "fact A"}},
//	        {Method: "POST", Path: "/memories", Body: map[string]interface{}{"content": "fact B"}},
//	    },
//	})
func (b *BatchClient) Execute(ctx context.Context, req BatchRequest) (BatchResponse, error) {
	var out BatchResponse
	err := b.c.post(ctx, "/batch", req, &out)
	return out, err
}
