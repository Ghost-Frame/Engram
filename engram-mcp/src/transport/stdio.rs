use crate::{handle_jsonrpc, App};
use engram_lib::{EngError, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};

fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| EngError::Internal(e.to_string()))?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|e| EngError::Internal(e.to_string()))?;
            content_length = Some(parsed);
        }
    }

    let len = content_length.ok_or_else(|| EngError::Internal("missing Content-Length header".into()))?;
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .map_err(|e| EngError::Internal(e.to_string()))?;
    let value = serde_json::from_slice(&buf)?;
    Ok(Some(value))
}

fn write_message<W: Write>(writer: &mut W, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .map_err(|e| EngError::Internal(e.to_string()))?;
    writer
        .write_all(&body)
        .map_err(|e| EngError::Internal(e.to_string()))?;
    writer
        .flush()
        .map_err(|e| EngError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn serve(app: App) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    while let Some(message) = read_message(&mut reader)? {
        if let Some(response) = handle_jsonrpc(&app, message).await {
            write_message(&mut writer, &response)?;
        }
    }

    Ok(())
}
