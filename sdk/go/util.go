package engram

import (
	"encoding/json"
	"fmt"
	"strconv"
	"strings"
)

// ----------------------------------------------------------------------------
// Query-string builder
// ----------------------------------------------------------------------------

type params struct {
	pairs []string
}

func newParams() *params { return &params{} }

func (p *params) add(key, value string) {
	p.pairs = append(p.pairs, key+"="+value)
}

func (p *params) addInt(key string, value int) {
	p.pairs = append(p.pairs, key+"="+strconv.Itoa(value))
}

func (p *params) addInt64(key string, value int64) {
	p.pairs = append(p.pairs, key+"="+strconv.FormatInt(value, 10))
}

func (p *params) encode() string {
	if len(p.pairs) == 0 {
		return ""
	}
	return "?" + strings.Join(p.pairs, "&")
}

// ----------------------------------------------------------------------------
// Generic JSON round-trip for interface{} slices
// ----------------------------------------------------------------------------

// decodeSlice re-encodes each element of a []interface{} into T by marshalling
// and unmarshalling through JSON. This avoids reflection gymnastics and keeps
// the code simple at the cost of a small allocation per item.
func decodeSlice[T any](src []interface{}) ([]T, error) {
	out := make([]T, 0, len(src))
	for i, item := range src {
		b, err := json.Marshal(item)
		if err != nil {
			return nil, fmt.Errorf("engram: marshal item %d: %w", i, err)
		}
		var t T
		if err := json.Unmarshal(b, &t); err != nil {
			return nil, fmt.Errorf("engram: decode item %d: %w", i, err)
		}
		out = append(out, t)
	}
	return out, nil
}

// Ptr returns a pointer to v. Useful for optional fields:
//
//	req := SearchRequest{Limit: engram.Ptr(10)}
func Ptr[T any](v T) *T { return &v }
