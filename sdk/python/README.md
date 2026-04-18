# engram-client (Python SDK)

Python client for the [Engram](https://github.com/Ghost-Frame/engram-rust) memory server.

## Install

```bash
pip install engram-client
# or with Poetry
poetry add engram-client
```

Requires Python 3.10+ and depends on `httpx` and `pydantic`.

## Quick start

```python
from engram_client import EngramClient

client = EngramClient("http://localhost:4200", api_key="ek-your-key")

# Store a memory
result = client.store_memory("User prefers dark mode", category="preference", importance=7)
print(result.id)

# Search
hits = client.search("user preferences", limit=10)
for h in hits:
    print(f"{h.score:.3f}  {h.memory.content}")

# Assemble context for an LLM prompt
ctx = client.build_context("What are the user's preferences?", max_tokens=2000)
for block in ctx.blocks:
    print(block.content)

# Batch multiple operations in one round-trip
resp = client.batch([
    {"method": "POST", "path": "/memories", "body": {"content": "fact A"}},
    {"method": "POST", "path": "/memories", "body": {"content": "fact B"}},
])
```

## Async

```python
import asyncio
from engram_client import AsyncEngramClient

async def main():
    async with AsyncEngramClient("http://localhost:4200", api_key="ek-your-key") as c:
        result = await c.store_memory("Important decision", category="decision")
        hits = await c.search("decisions", limit=5)

asyncio.run(main())
```

## Covered endpoints

| Group | Endpoints |
|-------|-----------|
| Memory | POST /memories, GET /memory/{id}, GET /list, POST /memory/{id}/update, DELETE /memory/{id}, POST /memory/{id}/forget |
| Search | POST /memories/search, POST /search/faceted |
| Context | POST /context, POST /context/stream (SSE) |
| Agents | GET/POST /agents, GET /agents/{id}, POST /agents/{id}/revoke, GET /agents/{id}/passport |
| Graph | GET /graph/neighborhood/{id} |
| Skills | GET /skills, GET /skills/{id} |
| Auth | GET/POST /keys, DELETE /keys/{id} |
| Batch | POST /batch |

For endpoints not listed, use the low-level passthrough:

```python
raw = client.raw_post("/intelligence/consolidate", {"mode": "full"})
```

## Error handling

```python
from engram_client import EngramError, NotFoundError, AuthError

try:
    mem = client.get_memory(99999)
except NotFoundError:
    print("Memory does not exist")
except AuthError:
    print("Check your API key")
except EngramError as e:
    print(f"HTTP {e.status_code}: {e}")
```

## OpenAPI

The server exposes its full OpenAPI spec at `GET /openapi.json`. The SDK is
hand-maintained at this scaffolding stage; a codegen path from the spec can be
added later -- see the top-level `sdk/README.md` for notes on that story.
