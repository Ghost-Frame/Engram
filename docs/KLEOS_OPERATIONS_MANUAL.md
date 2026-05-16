# Kleos Operations Manual

Canonical operator reference for the command-line tools that ship with Kleos.
This is the source of truth for what each command is for, how it authenticates,
what side effects it has, and when an agent should use it.

## Scope

This manual covers the operator-facing binaries installed by the `agent-host`
and `full` profiles in [dist/install.sh](../dist/install.sh):

- `kleos-cli`
- `kleos-sh`
- `kr`, `kw`, `ke`
- `agent-forge`
- `cred`
- `credd`
- `kleos-sidecar`
- `kleos-server`
- `kleos-mcp`
- `eidolon-supervisor`

## Quick routing

Use this table when choosing the right command:

| Need | Command |
|---|---|
| Store, search, recall, or hand off knowledge | `kleos-cli` |
| Report a task lifecycle event (started/progress/completed/blocked/error) | `kleos-cli activity` |
| Run a shell command through Kleos gate checks | `kleos-sh` |
| Read code or files safely | `kr` |
| Write file contents from stdin inside approved roots | `kw` |
| Check whether an edit is allowed for a file | `ke` |
| Create a spec, log a bug hypothesis, verify a change, or map code | `agent-forge` |
| Read or write secrets in the local vault | `cred` |
| Broker per-agent Kleos bearers and resolve secrets for clients | `credd` |
| Batch observations from hooks or sessions | `kleos-sidecar` |
| Run the HTTP API server | `kleos-server` |
| Expose Kleos as an MCP server | `kleos-mcp` |
| Watch Claude session logs for drift and violations | `eidolon-supervisor` |

## Shared conventions

- `KLEOS_URL` defaults to `http://127.0.0.1:4200` for most client tools.
- Kleos auth usually resolves in this order: explicit CLI key, signing identity,
  then per-agent bearer bootstrap via `credd`.
- Secrets should be injected into child processes with `cred exec` or
  `kleos-cli cred exec`. Do not print secrets into stdout.
- `kr`, `kw`, and `ke` are wrappers for agent use. They are not general shell
  replacements.
- Claude hook handlers fail open on transport failures by design. Direct shell
  execution paths default to fail closed unless configured otherwise.

## `kleos-cli`

Synopsis:

```bash
kleos-cli [--server URL] [--credd-url URL] [--key API_KEY] COMMAND ...
```

Purpose:

- Main human and agent client for the Kleos HTTP API.
- Covers memory, search, ingestion, jobs, skills, credentials, handoffs,
  Claude hooks, and identity enrollment.

Authentication model:

- Request signing identity, if configured, takes precedence for normal requests.
- `--key` overrides everything for bearer auth.
- If no key is passed, `kleos-cli` resolves the current agent slot and asks the
  bootstrap path for a per-agent Kleos bearer.
- `hook` commands are special: they use direct env-based auth instead of the
  normal bootstrap flow.

### Core memory commands

#### `kleos-cli store CONTENT [--category CAT] [--importance N] [--tags csv] [--source SRC]`

- Stores one memory via `POST /store`.
- `tags` are split on commas and trimmed.
- If the server returns `existing_id`, the CLI reports it as a duplicate.

Use for:

- Durable facts, decisions, issues, state, references, and task notes.

#### `kleos-cli search QUERY [--limit N]`

- Searches via `POST /search`.
- Prints each hit with final score plus channel breakdown when returned:
  final score, cosine score, and BM25/FTS score.

Use for:

- Fast recall before asking a human, opening a broad investigation, or editing.

#### `kleos-cli context QUERY [--limit N]`

- Calls `POST /recall` with both `query` and `context`.
- Returns richer JSON context than `search`.

Use for:

- Turn-level context injection or deeper retrieval where surrounding detail
  matters more than a flat result list.

#### `kleos-cli recall ID`

- Fetches one memory via `GET /memory/{id}`.

#### `kleos-cli list [--limit N] [--offset N]`

- Lists memories via `GET /list`.

#### `kleos-cli delete ID`

- Deletes one memory via `DELETE /memory/{id}`.

#### `kleos-cli guard CONTENT`

- Evaluates a proposed action through `POST /guard`.
- Prints matched rules and exits `2` on `warn` or `block`.

Use for:

- Preflight checks when you want a direct rule evaluation without running
  through `kleos-sh`.

#### `kleos-cli recall-due TOPIC [--limit N] [--session ID]`

- Calls `GET /fsrs/recall-due`.
- Surfaces memories with low retrievability that should be reinforced.

Use for:

- Review loops and reinforcement passes.

#### `kleos-cli ingest [--text TEXT | --file PATH] [--mode raw|extract] [--source SRC] [--category CAT]`

How it works:

- Text input goes directly to `POST /ingest`.
- Binary-like files go through the chunked upload flow:
  `POST /ingest/upload/init`, repeated `POST /ingest/upload/chunk`,
  then `POST /ingest/upload/complete`.
- Upload chunks are 1 MiB and hashed with SHA-256.

Use for:

- Markdown, text, HTML, CSV, JSONL, PDF, DOCX, ZIP, and similar source imports.

#### `kleos-cli health`

- Fetches `GET /health`.

#### `kleos-cli activity --action ACT --summary TEXT [--project P] [--agent A] [--metadata JSON]`

- Posts a lifecycle event to `POST /activity`.
- Signed with the local PIV or Ed25519 identity through the standard `Client`
  auth path -- no bearer token or `cred exec` wrapper required.
- The single call fans out to Chiasm (tasks), Axon (events), Broca (actions),
  Thymus (metrics), Skills, and Memory.
- `--action` is one of `task.started`, `task.progress`, `task.completed`,
  `task.blocked`, `error.raised`, or any custom event name.
- `--agent` defaults to `claude-code`.
- `--metadata` accepts a JSON object string and is mapped onto the
  `metadata`/`details` field server-side.
- This is the recommended path for activity reporting; `cred exec curl`
  remains as a fallback for hosts without an enrolled identity.

Use for:

- Per sub-task lifecycle reporting from agent sessions.

#### `kleos-cli bootstrap`

- Calls `POST /bootstrap`.
- Used once to claim the initial API key after server bootstrap is enabled.

### Jobs

#### `kleos-cli jobs stats`

- Calls `GET /jobs/stats`.

#### `kleos-cli jobs list [--status pending|running|failed] [--limit N] [--offset N]`

- Calls `GET /jobs`.

#### `kleos-cli jobs retry ID`

- Calls `POST /jobs/{id}/retry`.

#### `kleos-cli jobs retry --all`

- There is no server-side retry-all endpoint.
- The CLI emulates it by listing failed jobs and retrying each one individually.

#### `kleos-cli jobs purge [--older-than-days N]`

- Calls `POST /jobs/purge`.
- Removes old failed jobs.

#### `kleos-cli jobs cleanup [--older-than-days N]`

- Calls `POST /jobs/cleanup`.
- Removes old completed jobs.

### Skills

#### `kleos-cli skill search QUERY [--limit N]`

- Calls `POST /skills/search`.

#### `kleos-cli skill list [--limit N] [--offset N] [--agent NAME]`

- Calls `GET /skills`.

#### `kleos-cli skill get ID`

- Calls `GET /skills/{id}`.

#### `kleos-cli skill execute ID --success [--duration-ms N] [--error-type T] [--error-message MSG]`

- Calls `POST /skills/{id}/execute`.
- Records whether a skill run worked and how long it took.

#### `kleos-cli skill capture DESCRIPTION [--agent NAME]`

- Calls `POST /skills/capture`.
- Used to turn a description into a new skill record.

#### `kleos-cli skill fix ID [--direction TEXT] [--agent NAME]`

- Calls `POST /skills/{id}/fix`.

#### `kleos-cli skill derive PARENT_ID ... --direction TEXT [--agent NAME]`

- Calls `POST /skills/derive`.

#### `kleos-cli skill stats`

- Calls `GET /skills/dashboard/overview`.

#### `kleos-cli skill lineage ID`

- Calls `GET /skills/{id}/lineage`.

#### `kleos-cli skill evolve [--hours N] [--limit N]`

- Calls `GET /skills/evolution/recent`.

### Credentials through `credd`

These commands talk to `credd`, not directly to the main Kleos server.

Token resolution:

- `CREDD_AGENT_KEY`
- `~/.config/cred/credd-agent-key.token`
- fallback to the current Kleos bearer if neither exists

#### `kleos-cli cred get CATEGORY NAME [--raw]`

- Calls `GET /secret/{category}/{name}` on `credd`.
- `--raw` extracts the primary value field from the secret object.

#### `kleos-cli cred set CATEGORY NAME [--secret-type TYPE] [--value V] [--username U] [--url URL]`

- Calls `POST /secret/{category}/{name}` on `credd`.
- If `--value` is omitted, the CLI prompts on stdin without echoing.

Supported types:

- `api_key`
- `login`
- `oauth_app`
- `ssh_key`
- `note`

#### `kleos-cli cred list [--category CAT]`

- Calls `GET /secrets`.

#### `kleos-cli cred delete CATEGORY NAME`

- Calls `DELETE /secret/{category}/{name}`.

#### `kleos-cli cred agent-create NAME [--categories csv] [--allow-raw]`

- Calls `POST /agents`.
- Prints the generated agent key once. It cannot be re-read later.

#### `kleos-cli cred agent-list`

- Calls `GET /agents`.

#### `kleos-cli cred agent-revoke NAME`

- Calls `POST /agents/{name}/revoke`.

#### `kleos-cli cred exec CATEGORY NAME --env VAR -- COMMAND ...`

How it works:

- For `CATEGORY` equal to `kleos` or `engram-rust`, this uses the bootstrap
  bearer path to fetch a per-agent Kleos key for `NAME`.
- For all other categories, it reads the secret object from `credd` and
  extracts either the requested `--field` or the primary value.
- The secret is injected into the child environment and never printed by the CLI.

Use for:

- `curl`, SDKs, deployment tools, or any child process that needs a secret.

### Handoffs

#### `kleos-cli handoff dump [--project P] [--branch B] [--agent A] [--handoff-type T] [--session S] [--model M] [--host H] [--content TEXT] [--dir PATH]`

- Stores a handoff via `POST /handoffs`.
- If `--content` is omitted, reads stdin.
- Auto-detects project from `SESSION_HANDOFF_PROJECT`, git origin, or cwd name.

#### `kleos-cli handoff restore [filters...]`

- Calls `GET /handoffs`.
- Prints only the handoff content bodies.

#### `kleos-cli handoff latest [--project P] [--dir PATH]`

- Calls `GET /handoffs/latest`.

#### `kleos-cli handoff mechanical [--project P] [--agent A] [--dir PATH] [--session S] [--model M] [--host H]`

How it works:

- Collects git status, recent commits, diff stats, stashes, and recently
  modified files from the working tree.
- Stores that bundle as a `mechanical` handoff via `POST /handoffs`.

Use for:

- Mechanical state capture at session boundaries.

#### `kleos-cli handoff list [--limit N] [--project P] [--agent A] [--handoff-type T]`

- Calls `GET /handoffs`.
- Prints a summary table.

#### `kleos-cli handoff search QUERY [--project P] [--limit N]`

- Calls `GET /handoffs/search`.

#### `kleos-cli handoff stats`

- Calls `GET /handoffs/stats`.

#### `kleos-cli handoff gc [--tiered] [--keep N]`

- Calls `POST /handoffs/gc`.

### Claude hook handlers

#### `kleos-cli hook session-start`

- Registers session start with `POST /activity`.
- Fetches growth context from `/growth/materialize`.
- Emits Claude hook JSON on stdout.

#### `kleos-cli hook user-prompt`

- Reads Claude hook JSON from stdin.
- Checks supervisor pending injections through `/supervisor/pending`.
- Emits deny if a supervisor violation is pending; otherwise no output.

#### `kleos-cli hook stop`

- Records session end via `POST /activity`.

#### `kleos-cli hook pre-tool`

- Reads the proposed tool use from stdin.
- Derives a normalized command string.
- Sends it to `POST /gate/check`.
- Emits deny or additional context in Claude hook format.
- Fails open if the gate cannot be reached.

#### `kleos-cli hook post-tool`

- Reports completion to `/activity`.
- Closes the latest gate with `POST /gate/complete-latest`.

#### `kleos-cli hook post-bash`

- Back-compat alias for `post-tool`.

### Identity keys

#### `kleos-cli identity init [--label TEXT] [--software]`

How it works:

- Prefers a PIV YubiKey signer if present.
- Falls back to generating a software Ed25519 key if no YubiKey is detected,
  or immediately uses software when `--software` is passed.
- Signs an enrollment proof and posts it to `/identity-keys/enroll`.

#### `kleos-cli identity status`

- Prints the active local signing identity.

#### `kleos-cli identity list`

- Calls `GET /identity-keys/mine`.

#### `kleos-cli identity revoke ID [--reason TEXT]`

- Calls `POST /identity-keys/{id}/revoke`.

## `kleos-sh`

Synopsis:

```bash
kleos-sh -c "command"
kleos-sh --gate-only -c "command"
kleos-sh --claude-hook
```

Purpose:

- Universal shell gate for agent commands.
- Runs command proposals through Kleos policy before execution.
- Can act as a normal executor or as a Claude `PreToolUse` hook adapter.

Authentication resolution:

1. `KLEOS_API_KEY`
2. `EIDOLON_KEY`
3. `credd` bootstrap flow using `CREDD_SOCKET` and `CREDD_AGENT_KEY`
4. legacy fallback: `cred get kleos <slot> --raw`

Execution model:

- Normal mode: checks a command, optionally enriches it, then executes it.
- `--gate-only`: checks and exits without execution.
- `--claude-hook`: reads hook JSON from stdin, emits hook JSON to stdout, and
  always exits `0`; the allow or deny decision is carried in JSON.

Failure policy:

- Hook mode defaults to fail open.
- Direct execution mode defaults to fail closed.
- Override with `KLEOS_SH_FAIL_OPEN=1` or `0`.

Important options:

- `-c`: command string
- `--agent`: agent identity label, default `claude-code`
- `--tool-name`: reported tool name, default `Bash`
- `--gate-only`
- `--claude-hook`

Important env:

- `KLEOS_SERVER_URL`, `KLEOS_URL`, `ENGRAM_EIDOLON_URL`, `EIDOLON_URL`
- `KLEOS_SIDECAR_URL`
- `KLEOS_SH_TIMEOUT_SECS`
- `KLEOS_CRED_KEY`
- `KLEOS_AGENT_SLOT`
- `CREDD_SOCKET`
- `CREDD_AGENT_KEY`

Side effects:

- On allow, executes the resolved command and reports completion to the sidecar.
- On deny in normal mode, exits `2`.
- On deny in hook mode, emits Claude hook deny JSON.

## `kr`, `kw`, `ke`

These are the `kleos-fs` binaries. The behavior depends on the invoked binary name.

### `kr`

Synopsis:

```bash
kr PATH [--symbol NAME]
```

Purpose:

- Read files for agents.
- For large code files, prefer structure-aware output through `agent-forge`.

How it works:

- Resolves `~/` and canonicalizes the path.
- Small files and non-code files are read directly.
- Code files larger than 8192 bytes are routed through `agent-forge`:
  - `repo-map` when reading a file generally
  - `search-code` when `--symbol` is passed
- If `agent-forge` fails, `kr` falls back to raw file output unless
  `KLEOS_FS_NO_FALLBACK` disables that fallback.

Important env:

- `AGENT_FORGE_BIN`
- `KLEOS_FS_TRUST_PATH`

### `kw`

Synopsis:

```bash
kw [--mkdir] PATH < content
```

Purpose:

- Write exact stdin content to a file under approved roots.

How it works:

- Allowed roots come from `KLEOS_FS_ALLOWED_ROOTS`.
- If that env var is unset, the current working directory is the only allowed root.
- New parent directories are created only with `--mkdir`.
- Writes outside allowed roots are blocked.
- On success, reports an observation event.

Use for:

- Direct file replacement from controlled stdin content.

### `ke`

Synopsis:

```bash
ke PATH
```

Purpose:

- Edit permission gate, not an editor.
- Confirms that a file has a matching spec-task ledger entry before editing.

How it works:

- Uses the same root allowlist as `kw`.
- Builds a ledger key from `KLEOS_SESSION_ID` or `CLAUDE_SESSION_ID` plus path.
- Checks the scratchpad ledger for a matching spec-task entry.
- If the ledger is missing, it blocks and instructs you to run
  `agent-forge ... spec-task`.
- If the server is unavailable, it fails closed unless
  `KLEOS_FS_ALLOW_OFFLINE_EDIT=1`.

Use for:

- Pre-edit validation in guarded workflows.

## `agent-forge`

Synopsis:

```bash
agent-forge --input in.json --output out.json [--db ~/.agent-forge/forge.db] COMMAND
```

Purpose:

- Structured reasoning, code review, and workflow enforcement tool.
- All commands read JSON input and write JSON output.

Output contract:

- The output file always receives a structured result.
- Successful commands usually return `success=true`, a message, and optional `data`.

### Workflow commands

#### `agent-forge spec-task`

Required input:

- `task_description`
- `task_type` in `feature`, `bugfix`, `refactor`, `enhancement`, `test`, `docs`
- at least 2 `acceptance_criteria`
- `interface_contract`
- at least 3 `edge_cases`

What it does:

- Creates a new active spec record.
- Marks the session active against that spec.

Use before:

- New code, non-trivial edits, refactors, and test additions.

#### `agent-forge consider-approaches`

Required input:

- `problem`
- `approaches`: array of 2+ items, each with `name`, `description`, optional `pros`, `cons`, `score`
- optional `spec_id` (links to existing spec)
- optional `chosen_index` (which approach was selected)

What it does:

- Records evaluated approaches in the forge database.
- If `chosen_index` is set, marks that approach as chosen.
- Returns a comparison prompt for the agent to reason through.

#### `agent-forge log-hypothesis`

Required input:

- `bug_description`
- `hypothesis`
- optional `confidence` from `0.0` to `1.0`
- optional `spec_id` (links hypothesis to a spec)

What it does:

- Records a debugging hypothesis.
- If `spec_id` is set, the hypothesis appears in `get-spec` output.

#### `agent-forge log-outcome`

Required input:

- `hypothesis_id`
- `outcome` in `correct`, `incorrect`, `partial`

What it does:

- Marks whether a previous hypothesis was right.

#### `agent-forge recall-errors`

Input:

- optional `query`
- optional `limit`

What it does:

- Searches past hypotheses and outcomes.

#### `agent-forge verify`

Required input (one of):

- `command` -- single command to run
- `steps` -- array of `{command, expected_exit_code?, label?}` for multi-step verification
- Both may be provided; `command` runs first, then `steps`

Optional:

- `expected_exit_code`, default `0` (for single `command`)
- `timeout_secs` -- kill the process if it exceeds this duration
- `spec_id` -- link verification results to a spec (records to `verifications` table)
- `criteria_index` -- which acceptance criterion this verification covers
- `skill_id` -- record pass/fail to Kleos skill execution tracking

What it does:

- Executes each step without a shell (SEC-C1).
- Tracks duration_ms per step.
- Records results to the `verifications` table when `spec_id` is provided.
- Returns per-step results with exit code, stdout, stderr, duration.

Use after:

- Code changes or repair attempts.
- Multi-step: run tests AND check types AND lint in one call.

#### `agent-forge challenge-code`

Required input:

- `file_path`
- optional `focus_areas`

What it does:

- Reads a file and returns an adversarial review prompt plus metadata.

#### `agent-forge checkpoint`

Required input:

- `name`

What it does:

- Records a checkpoint and current git `HEAD`.

#### `agent-forge rollback`

Required input:

- `checkpoint_name`

What it does:

- Looks up the checkpoint git ref and performs `git checkout` to that ref.

Use with care:

- This is operationally destructive to the current checkout state.

#### `agent-forge session-learn`

Required input:

- `discovery`

Optional:

- `context`
- `tags`
- `spec_id` (links learning to a spec)
- `capture_as_skill` (boolean) -- if true, also captures the discovery as a Kleos skill

What it does:

- Stores a session learning note in the forge database.
- If `spec_id` is set, the learning appears in `get-spec` output.
- If `capture_as_skill` is true and Kleos is reachable, creates a skill from the discovery.

#### `agent-forge session-recall`

Input:

- optional `query`
- optional `limit`

What it does:

- Recalls prior session learnings.

#### `agent-forge session-diff`

Input:

- optional `base`, default `HEAD~10`

What it does:

- Runs `git diff --stat` and `git diff --name-only` against the base ref.

#### `agent-forge think`

Required input:

- `problem`

Optional:

- `constraints`
- `context`

What it does:

- Produces a structured reasoning prompt.

#### `agent-forge declare-unknowns`

Required input:

- `unknowns`: one or more `{description, blocking, resolution_hint?}`

What it does:

- Separates blocking from non-blocking unknowns.
- Explicitly tells the operator to stop if blocking unknowns exist.

### Code-structure commands

#### `agent-forge repo-map`

Required input:

- `path`

Optional:

- `focus`
- `max_tokens`

What it does:

- Walks the repo respecting `.gitignore`.
- Parses supported source files with Tree-sitter.
- Emits a ranked symbol map within a token budget.

#### `agent-forge search-code`

Required input:

- `query`

Optional:

- `path`
- `symbol_type`
- `limit`

What it does:

- Searches symbol names across supported source files.
- Returns file, line, column, symbol kind, and line context.

### Spec lifecycle commands

#### `agent-forge update-spec`

Required input:

- `spec_id`
- `status` in `active`, `completed`, `failed`, `blocked`

Optional:

- `note` -- reason for the status change

What it does:

- Updates the spec status. Sets `completed_at` timestamp for `completed` and `failed`.

Use when:

- A task is done, abandoned, or blocked. Closes the protocol loop.

#### `agent-forge list-specs`

Optional input:

- `status` -- filter by status
- `limit` -- max results, default 20

What it does:

- Lists specs with id, description, type, status, timestamps.

#### `agent-forge get-spec`

Required input:

- `spec_id`

What it does:

- Returns the full spec plus all related hypotheses, approaches, learnings, and verifications.
- This is the cross-reference view -- everything linked to a single task.

### Stats command

#### `agent-forge stats`

Optional input:

- `days` -- look-back window, default 30

What it does:

- Queries across all tables and returns a protocol health dashboard:
  - Spec completion rate (completed/total)
  - Hypothesis accuracy (correct/resolved)
  - Verification pass rate (passed/total)
  - Average hypothesis confidence
  - Average verification duration
  - Top error patterns (most common bug descriptions)
  - Task type distribution
  - Learning, approach, and checkpoint counts

### Skill integration commands

These commands connect agent-forge to the Kleos skill evolution system.
They require a running Kleos server (`KLEOS_URL` and `KLEOS_API_KEY` env vars).

#### `agent-forge skill-search`

Required input:

- `query`

Optional:

- `limit`

What it does:

- Searches Kleos skills by keyword/semantic query.
- Use before `spec-task` to find relevant existing skills.

#### `agent-forge skill-capture`

Required input:

- `description` (max 2000 chars)

Optional:

- `agent`

What it does:

- Creates a new Kleos skill from a freeform workflow description.
- The Kleos LLM generates name, description, and reusable code.

#### `agent-forge skill-record-exec`

Required input:

- `skill_id`
- `success` (boolean)

Optional:

- `duration_ms`
- `error_type`
- `error_message`

What it does:

- Records an execution result against a Kleos skill.
- Builds trust scores over time.

#### `agent-forge skill-fix`

Required input:

- `skill_id`

Optional:

- `hint`

What it does:

- Triggers fix evolution on a failing skill.
- The Kleos LLM analyzes recent failures and generates a fixed version.

#### `agent-forge skill-derive`

Required input:

- `parent_ids` (array of skill IDs, at least one)
- `direction` (max 2000 chars)

Optional:

- `agent`

What it does:

- Combines parent skills into a new derived skill guided by the direction hint.

#### `agent-forge skill-lineage`

Required input:

- `skill_id`

What it does:

- Returns the evolution chain (parent IDs) for a skill.

## `cred`

Synopsis:

```bash
cred COMMAND ...
```

Purpose:

- Local YubiKey-backed credential vault and bootstrap helper.

Core commands:

- `cred init`: first-time YubiKey challenge-response setup and recovery kit creation.
- `cred store SERVICE KEY [-t TYPE]`: interactive secret storage.
- `cred get SERVICE KEY [-f FIELD] [--raw]`: retrieve one secret or field.
- `cred list [--service NAME]`: list redacted secrets.
- `cred delete SERVICE KEY [-y]`: delete one secret.
- `cred import [-n]`: bulk import from stdin.
- `cred export`: export vault data.
- `cred recover [--from PATH]`: recover onto a replacement YubiKey.
- `cred exec SERVICE KEY [--field FIELD] [--env VAR|--stdin] -- COMMAND ...`: inject a secret into a child process.
- `cred agent-key ...`: manage DB-backed and bootstrap-scoped agent keys.
- `cred bootstrap wrap|unwrap`: manage `bootstrap.enc` for `credd`.
- `cred piv ...`: manage YubiKey PIV bootstrap keys.
- `cred ssh-ca ...`: mint or sign short-lived SSH certificates.
- `cred tui`: interactive terminal UI.

How it works:

- Derives the vault master key from a persisted challenge plus YubiKey
  HMAC-SHA1 slot 2 response.
- Uses encrypted local storage plus optional bootstrap blobs and agent-key files.

Deep reference:

- [kleos-cred/MANUAL.md](../kleos-cred/MANUAL.md)

## `credd`

Synopsis:

```bash
credd [--listen ADDR] [--db-path PATH] [--auth-mode yubikey|password]
```

Purpose:

- Credential daemon that brokers per-agent Kleos bearers and serves secrets.

How it works:

- Derives its master key from YubiKey or password.
- Loads `bootstrap.enc` if present.
- Loads bootstrap-scoped file-backed agent keys.
- Opens the credential DB and serves HTTP or socket-based clients.

Important surfaces:

- `GET /bootstrap/kleos-bearer?agent=<slot>`
- `POST /resolve/text`
- `POST /resolve/proxy`
- `POST /resolve/raw`
- `GET /secret/{category}/{name}`
- `GET/POST/DELETE` agent-key management endpoints

Deep reference:

- [kleos-credd/MANUAL.md](../kleos-credd/MANUAL.md)

## `kleos-sidecar`

Synopsis:

```bash
kleos-sidecar [--config sidecar.toml] [--port N] [--host HOST] [--watch]
```

Purpose:

- Local batching proxy for observations, session traffic, and optional
  Claude session-file watching.

Config precedence:

- CLI flag
- env var
- config file
- built-in default

Important options:

- `--config`
- `--port`, default `7711`
- `--host`, default `127.0.0.1`
- `--session-id`
- `--source`
- `--user-id`
- `--token`
- `--kleos-url`
- `--kleos-api-key`
- `--watch`
- `--watch-dir`, default `~/.claude/projects`
- batch sizing, compression, idle TTL, and log format controls

How it works:

- Generates a bearer token at startup if one is not supplied.
- Queues observations per session and flushes them in batches.
- Can watch Claude session JSONL files directly and ingest those changes.
- Uses a local LLM path for `/compress` when configured.

## `kleos-server`

Synopsis:

```bash
kleos-server
```

Purpose:

- Main Kleos HTTP API server and background worker host.

How it works at startup:

1. Loads env-based config.
2. Resolves database encryption mode.
3. Connects to SQLite and tenant shards.
4. Starts metrics.
5. Loads embeddings and reranker in the background.
6. Probes local LLM availability.
7. Starts background jobs, dreamer, cleanup, and replay workers.
8. Serves the HTTP API, GUI, and related routes.

Operational notes:

- Default bind is `127.0.0.1:4200`.
- Initial bootstrap key flow is separate from normal auth.
- Multi-user tenant sharding is enabled by default.

## `kleos-mcp`

Synopsis:

```bash
kleos-mcp [--transport stdio|http] [--listen ADDR]
```

Purpose:

- Exposes Kleos capabilities to LLM clients over MCP.

How it works:

- Loads app configuration from env.
- Starts stdio transport by default.
- When compiled with the `http` feature, can also serve over HTTP.

Tool families exposed:

- memory
- context
- graph
- intelligence
- services
- structural
- skills
- admin

## `eidolon-supervisor`

Synopsis:

```bash
eidolon-supervisor
```

Purpose:

- Watches session logs for policy drift, rule violations, and retry loops.

How it works:

- Watches `CLAUDE_SESSIONS_DIR`, default `~/.claude/projects`.
- Loads rules from `EIDOLON_SUPERVISOR_CONFIG` or uses built-in defaults.
- Posts alerts and supervisor injections back to Kleos.

Important env:

- `CLAUDE_SESSIONS_DIR`
- `KLEOS_SERVER_URL`
- `KLEOS_API_KEY`
- `EIDOLON_SUPERVISOR_CONFIG`

## Recommended agent workflow

For day-to-day agent work, default to this sequence:

1. `kleos-cli search` or `kleos-cli context` before asking a human.
2. `agent-forge spec-task` before non-trivial code changes.
3. `agent-forge consider-approaches` when multiple paths exist.
4. `kr` to read code and `kw` only for bounded direct writes.
5. `ke` before an edit path that is supposed to be spec-gated.
6. `kleos-sh` or provider hook integration for shell actions that must go
   through the gate.
7. `agent-forge verify` after code changes (use `steps` for multi-step).
8. `agent-forge update-spec` to mark the spec completed/failed/blocked.
9. `agent-forge session-learn` for reusable discoveries (with `capture_as_skill: true` when appropriate).
10. `kleos-cli store` or `kleos-cli handoff dump/mechanical` at task boundaries.
11. `agent-forge stats` periodically to review protocol health.

## Rule of precedence

If command behavior here and ad hoc prompt instructions disagree, prefer:

1. Actual command implementation
2. This manual
3. Ad hoc prompt text

When in doubt, inspect the source before inventing a new usage pattern.
