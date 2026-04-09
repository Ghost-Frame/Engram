# Worktree G: Ingestion Parsers (PDF, DOCX, ZIP)

## Worktree Path
`/home/zan/Projects/engram-rust-wt-G` -- branch `feat/ingestion-parsers`

## Goal
Replace the stub parsers with real implementations that extract text from PDF, DOCX, and ZIP files. The TypeScript source uses npm packages for these; we need Rust crate equivalents.

## Source of Truth
- `C:\Users\Zan\Projects\engram\src\ingestion\parsers\` -- TS parser implementations
- `C:\Users\Zan\Projects\engram-rust\engram-lib\src\ingestion\parsers\` -- current Rust stubs

## Files to Modify
- `engram-lib/src/ingestion/parsers/pdf.rs` (currently 32 lines, stub)
- `engram-lib/src/ingestion/parsers/docx.rs` (currently 28 lines, stub)
- `engram-lib/src/ingestion/parsers/zip.rs` (currently 39 lines, stub)
- `engram-lib/Cargo.toml` (add parser crate dependencies)
- `Cargo.toml` (workspace deps if needed)

## Implementation

### PDF Parser
- Use `pdf-extract` or `lopdf` crate for text extraction
- Read the TS implementation first to match the output format
- Must handle multi-page documents
- Return extracted text chunks compatible with the existing `IngestResult` type
- Handle errors gracefully (corrupted PDFs, password-protected, etc.)

### DOCX Parser
- Use `docx-rs` or `zip` + XML parsing (DOCX is a ZIP of XML files)
- Extract paragraph text, preserving basic structure
- Match TS output format

### ZIP Parser
- Use `zip` crate
- Recursively extract and parse supported file types within the archive
- Delegate to existing parsers (markdown, html, csv, etc.) based on file extension
- Handle nested ZIPs if the TS version does

## Constraints
- Read each existing stub file BEFORE editing
- Read the TS parser equivalents to understand expected behavior
- Match the existing parser function signatures (`parse_*` functions returning `Result<Vec<IngestChunk>>` or similar)
- Add crate dependencies to workspace Cargo.toml
- Run `cargo check --workspace` after every change
- Run `cargo clippy --workspace` before committing
- No em dashes in commits or comments

## Verification
1. `cargo check --workspace` passes
2. `cargo test -p engram-lib ingestion` passes (if tests exist)
3. Each parser handles both valid input and error cases
