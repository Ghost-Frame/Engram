// ============================================================================
// CSV parser -- ported from parsers/csv.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

const CONTENT_COLUMN_NAMES: &[&str] = &["content", "text", "body", "message"];
const TITLE_COLUMN_NAMES: &[&str] = &["title", "name"];

/// Detect which column contains the main content by name, then by average length.
fn detect_content_column(headers: &[String], rows: &[Vec<String>]) -> usize {
    for name in CONTENT_COLUMN_NAMES {
        if let Some(idx) = headers.iter().position(|h| h.to_lowercase() == *name) {
            return idx;
        }
    }

    if headers.is_empty() {
        return 0;
    }

    let averages: Vec<f64> = (0..headers.len())
        .map(|col_idx| {
            let total: usize = rows
                .iter()
                .map(|row| row.get(col_idx).map(|s| s.len()).unwrap_or(0))
                .sum();
            total as f64 / rows.len().max(1) as f64
        })
        .collect();

    let mut max_idx = 0;
    for i in 1..averages.len() {
        if averages[i] > averages[max_idx] {
            max_idx = i;
        }
    }
    max_idx
}

/// Detect which column contains the title.
fn detect_title_column(headers: &[String]) -> Option<usize> {
    for name in TITLE_COLUMN_NAMES {
        if let Some(idx) = headers.iter().position(|h| h.to_lowercase() == *name) {
            return Some(idx);
        }
    }
    None
}

/// Parse CSV text into parsed documents (one per row).
///
/// Reads through the `csv` crate so quoted fields containing embedded
/// newlines, commas, and escaped quotes parse per RFC 4180. The previous
/// hand-rolled per-line splitter broke any quoted multi-line field into
/// separate phantom rows. Headers are consumed manually (`has_headers(false)`
/// plus a first-record take) because the column-detection heuristics below
/// need them as an ordinary row; `flexible(true)` keeps the old tolerance
/// for rows with a different field count, and malformed records are skipped
/// rather than failing the whole document.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(input.as_bytes());

    let mut records = reader.records();
    let headers: Vec<String> = match records.next() {
        Some(Ok(record)) => record.iter().map(|s| s.to_string()).collect(),
        _ => return Ok(vec![]),
    };
    let rows: Vec<Vec<String>> = records
        .filter_map(|r| r.ok())
        .map(|record| record.iter().map(|s| s.to_string()).collect())
        .collect();
    if rows.is_empty() {
        return Ok(vec![]);
    }

    let content_idx = detect_content_column(&headers, &rows);
    let title_idx = detect_title_column(&headers);

    let mut docs = Vec::new();
    for (row_num, row) in rows.iter().enumerate() {
        let content_text = row.get(content_idx).map(|s| s.as_str()).unwrap_or("");

        let title = match title_idx {
            Some(idx) if row.get(idx).map(|s| !s.is_empty()).unwrap_or(false) => row[idx].clone(),
            _ => format!("Row {}", row_num + 1),
        };

        let mut metadata = HashMap::new();
        for (col_idx, header) in headers.iter().enumerate() {
            if col_idx == content_idx {
                continue;
            }
            let value = row.get(col_idx).map(|s| s.as_str()).unwrap_or("");
            metadata.insert(header.clone(), serde_json::Value::String(value.to_string()));
        }

        docs.push(ParsedDocument {
            title,
            text: content_text.to_string(),
            metadata,
            source: "csv".to_string(),
            timestamp: None,
        });
    }

    Ok(docs)
}

/// Detect if input has .csv extension.
pub fn detect(extension: Option<&str>) -> bool {
    extension
        .map(|e| e.to_lowercase() == ".csv")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_csv_quoted_fields() {
        // Quoted commas and escaped quotes stay inside their field.
        let csv = "title,content\n\"hello, world\",\"she said \"\"hi\"\" twice\"";
        let docs = parse(csv).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "hello, world");
        assert_eq!(docs[0].text, "she said \"hi\" twice");
    }

    #[test]
    fn test_parse_csv_quoted_embedded_newline() {
        // A quoted field containing a newline is ONE row, not two. The old
        // hand-rolled line splitter broke this into phantom rows.
        let csv = "title,content\nnote,\"first line\nsecond line\"\nother,plain";
        let docs = parse(csv).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].text, "first line\nsecond line");
        assert_eq!(docs[1].title, "other");
        assert_eq!(docs[1].text, "plain");
    }

    #[test]
    fn test_parse_csv_basic() {
        let csv = "name,content,category\nAlice,Hello world,general\nBob,Goodbye,task";
        let docs = parse(csv).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].title, "Alice");
        assert_eq!(docs[0].text, "Hello world");
        assert_eq!(docs[1].title, "Bob");
        assert_eq!(docs[1].text, "Goodbye");
    }

    #[test]
    fn test_parse_csv_content_detection_by_name() {
        let csv = "id,body,extra\n1,some long text here,x\n2,another body text,y";
        let docs = parse(csv).unwrap();
        assert_eq!(docs[0].text, "some long text here");
    }

    #[test]
    fn test_parse_csv_content_detection_by_length() {
        let csv = "a,b,c\n1,short,this is a much longer piece of content\n2,tiny,another long piece of content here";
        let docs = parse(csv).unwrap();
        assert!(docs[0].text.contains("much longer"));
    }

    #[test]
    fn test_parse_csv_empty() {
        let csv = "header\n";
        let docs = parse(csv).unwrap();
        assert!(docs.is_empty());
    }
}
