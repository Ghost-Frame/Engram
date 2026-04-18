# engram (Go SDK)

Go client for the [Engram](https://github.com/Ghost-Frame/engram-rust) memory server.

Uses only the Go standard library -- no third-party dependencies.

## Install

```bash
go get github.com/Ghost-Frame/engram-rust/sdk/go
```

## Quick start

```go
package main

import (
    "context"
    "fmt"
    "log"

    engram "github.com/Ghost-Frame/engram-rust/sdk/go"
)

func main() {
    c := engram.NewClient("http://localhost:4200", "ek-your-key")
    ctx := context.Background()

    // Store a memory
    result, err := c.Memory.Store(ctx, engram.StoreRequest{
        Content:    "User prefers dark mode",
        Category:   "preference",
        Importance: engram.Ptr(7),
    })
    if err != nil {
        log.Fatal(err)
    }
    fmt.Println("Stored ID:", result.ID)

    // Search
    hits, err := c.Search.Search(ctx, engram.SearchRequest{
        Query: "user preferences",
        Limit: engram.Ptr(10),
    })
    if err != nil {
        log.Fatal(err)
    }
    for _, h := range hits {
        fmt.Printf("%.3f  %s\n", h.Score, h.Memory.Content)
    }

    // Assemble context for an LLM prompt
    ctx2, err := c.Context.Build(ctx, engram.ContextRequest{
        Query:     "What are the user's preferences?",
        MaxTokens: engram.Ptr(2000),
    })
    if err != nil {
        log.Fatal(err)
    }
    for _, block := range ctx2.Blocks {
        fmt.Println(block.Content)
    }
}
```

## Covered endpoints

| Group | Client field | Endpoints |
|-------|-------------|-----------|
| Memory | `c.Memory` | Store, Get, List, Update, Delete, Forget, Archive |
| Search | `c.Search` | Search, Faceted, Explain |
| Context | `c.Context` | Build, Stream (SSE) |
| Agents | `c.Agents` | List, Create, Get, Revoke, Passport, LinkKey |
| Graph | `c.Graph` | Neighborhood, Search, Build, EntityRelationships |
| Skills | `c.Skills` | List, Get, Search, Execute, Delete |
| Auth | `c.Auth` | ListKeys, CreateKey, RevokeKey, RotateKey |
| Batch | `c.Batch` | Execute |

For unlisted endpoints use the passthrough methods:

```go
var out map[string]interface{}
err := c.RawPost(ctx, "/intelligence/consolidate", map[string]interface{}{"mode": "full"}, &out)
```

## Error handling

```go
result, err := c.Memory.Get(ctx, 99999)
if err != nil {
    if engram.IsNotFound(err) {
        fmt.Println("memory not found")
    } else if engram.IsUnauthorized(err) {
        fmt.Println("check your API key")
    } else if e, ok := engram.IsEngramError(err); ok {
        fmt.Printf("HTTP %d: %v\n", e.StatusCode, e)
    }
}
```

## Pointer helpers

Optional numeric and boolean fields use `*T`. Use `engram.Ptr(v)` to create
a pointer inline:

```go
req := engram.SearchRequest{
    Query: "facts",
    Limit: engram.Ptr(20),
}
```

## OpenAPI

The server exposes its full OpenAPI spec at `GET /openapi.json`. These SDKs
are hand-maintained; see the top-level `sdk/README.md` for notes on a future
codegen path.
