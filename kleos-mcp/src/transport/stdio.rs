/// Stdio JSON-RPC transport with auto-detected framing.
///
/// Supports both NDJSON (one JSON object per line, used by Claude Code
/// 2025-03-26+ spec) and Content-Length/LSP-style framing (2024-11-05 spec).
/// The framing mode is detected from the very first byte of the session and
/// then used consistently for both reads and writes.
use crate::{handle_jsonrpc, App};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};

/// Which wire framing the peer is speaking.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Framing {
    /// One JSON object per line, newline-delimited.
    Ndjson,
    /// `Content-Length: N\r\n\r\n<body>` (LSP-style).
    ContentLength,
}

/// Maximum message body size (10 MiB) to prevent OOM from a malicious or
/// buggy Content-Length value.
const MAX_MCP_MSG_SIZE: usize = 10 * 1024 * 1024;

/// Reads the first message and determines the session framing.
///
/// Peeks at the first non-empty line: if it starts with `{` it's NDJSON,
/// if it starts with `Content-Length:` it's LSP framing.
fn read_first_message<R: BufRead>(reader: &mut R) -> Result<Option<(Value, Framing)>, String> {
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return read_first_message(reader);
    }
    if trimmed.starts_with('{') {
        let value: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
        Ok(Some((value, Framing::Ndjson)))
    } else if trimmed.starts_with("Content-Length:") {
        let len = parse_content_length(trimmed)?;
        // Consume remaining headers until the empty separator line.
        loop {
            let mut hdr = String::new();
            reader.read_line(&mut hdr).map_err(|e| e.to_string())?;
            if hdr.trim().is_empty() {
                break;
            }
        }
        let value = read_content_length_body(reader, len)?;
        Ok(Some((value, Framing::ContentLength)))
    } else {
        Err(format!("unexpected first line: {trimmed}"))
    }
}

/// Reads the next NDJSON message, skipping blank lines.
fn read_ndjson<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|e| e.to_string())?;
        return Ok(Some(value));
    }
}

/// Reads the next Content-Length-framed message.
fn read_content_length_msg<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if trimmed.starts_with("Content-Length:") {
            content_length = Some(parse_content_length(trimmed)?);
        }
    }
    let len = content_length.ok_or_else(|| "missing Content-Length header".to_string())?;
    let value = read_content_length_body(reader, len)?;
    Ok(Some(value))
}

/// Parses a `Content-Length: N` header line into `N`.
fn parse_content_length(header: &str) -> Result<usize, String> {
    let value = header
        .strip_prefix("Content-Length:")
        .ok_or_else(|| "not a Content-Length header".to_string())?;
    value.trim().parse::<usize>().map_err(|e| e.to_string())
}

/// Reads exactly `len` bytes and deserialises as JSON, with a 10 MiB cap.
fn read_content_length_body<R: BufRead>(reader: &mut R, len: usize) -> Result<Value, String> {
    if len > MAX_MCP_MSG_SIZE {
        return Err(format!(
            "Content-Length {} exceeds max {}",
            len, MAX_MCP_MSG_SIZE
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    serde_json::from_slice(&buf).map_err(|e| e.to_string())
}

/// Writes a single NDJSON message (compact JSON + newline).
fn write_ndjson<W: Write>(writer: &mut W, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    writer.write_all(&body).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Writes a single Content-Length-framed message.
fn write_content_length<W: Write>(writer: &mut W, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len()).map_err(|e| e.to_string())?;
    writer.write_all(&body).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Runs the stdio JSON-RPC loop against the given `App` until EOF.
///
/// Auto-detects the wire framing from the first message and then uses
/// the same framing for all subsequent reads and writes in the session.
pub async fn serve(app: App) -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    let (first_msg, framing) = match read_first_message(&mut reader)? {
        Some(pair) => pair,
        None => return Ok(()),
    };

    tracing::debug!(?framing, "stdio framing negotiated");

    // Process the first message.
    if let Some(response) = handle_jsonrpc(&app, first_msg).await {
        match framing {
            Framing::Ndjson => write_ndjson(&mut writer, &response)?,
            Framing::ContentLength => write_content_length(&mut writer, &response)?,
        }
    }

    // Main loop using the detected framing.
    loop {
        let message = match framing {
            Framing::Ndjson => read_ndjson(&mut reader)?,
            Framing::ContentLength => read_content_length_msg(&mut reader)?,
        };
        match message {
            Some(msg) => {
                if let Some(response) = handle_jsonrpc(&app, msg).await {
                    match framing {
                        Framing::Ndjson => write_ndjson(&mut writer, &response)?,
                        Framing::ContentLength => write_content_length(&mut writer, &response)?,
                    }
                }
            }
            None => break,
        }
    }

    Ok(())
}
