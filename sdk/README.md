# Engram SDKs

Client libraries for the [Engram](https://github.com/Ghost-Frame/engram-rust) memory server.

## Available SDKs

| SDK | Directory | Language |
|-----|-----------|----------|
| TypeScript | `sdk/typescript/` | TypeScript / Node.js 18+ |
| Python | `sdk/python/` | Python 3.10+ |
| Go | `sdk/go/` | Go 1.22+ |

All three SDKs cover the same core surface area:

- Memory CRUD (store, get, list, update, delete, forget)
- Hybrid search and faceted search
- Context assembly (sync and SSE streaming)
- Agent management (register, revoke, passport)
- Graph neighbourhood queries
- Skill listing and lookup
- API key management
- Batch operations

## Relation to the OpenAPI spec

The server publishes a machine-readable OpenAPI 3.1 spec at:

```
GET /openapi.json
GET /docs/openapi.json
```

A Swagger UI is available at `GET /docs`.

These SDKs are **hand-maintained** at this scaffolding stage. The spec and the
SDK methods are kept in sync by hand; they do not diverge from each other by
design, but there is no automated codegen step yet.

## Future codegen path

When the API surface stabilises, generation from the spec is straightforward:

```bash
# Example using openapi-generator-cli (not yet wired up)
openapi-generator-cli generate \
  -i http://localhost:4200/openapi.json \
  -g python \
  -o sdk/python-generated/

# Or for Go:
openapi-generator-cli generate \
  -i http://localhost:4200/openapi.json \
  -g go \
  -o sdk/go-generated/
```

The hand-written SDKs in this directory are the canonical reference until that
story is completed. The codegen output would live in separate directories to
avoid overwriting the hand-written ergonomic wrappers.

## Quick install

**TypeScript**

```bash
npm install @ghost_frame/engram
```

**Python**

```bash
pip install engram-client
```

**Go**

```bash
go get github.com/Ghost-Frame/engram-rust/sdk/go
```

## Authentication

All SDKs use Bearer token authentication. Pass your API key when constructing
the client. Keys are managed via the `/keys` endpoints or the Engram web UI.

```
Authorization: Bearer ek-your-key-here
```
