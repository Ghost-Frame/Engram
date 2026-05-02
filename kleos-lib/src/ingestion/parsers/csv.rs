// ============================================================================
// CSV parser -- ported from parsers/csv.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

const CONTENT_COLUMN_NAMES: &[&str] = &["content", "text", "body", "message"];
const TITLE_COLUMN_NAMES: &[&str] = &["title", "name"];

/// Parse a single CSV line respecting quoted fields with escaped quotes.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Quoted field
            i += 1; // skip opening quote
            let mut field = String::new();
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                        // Escaped quote
                        field.push('"');
                        i += 2;
                    } else {
                        // End of quoted field
                        i += 1; // skip closing quote
                        break;
                    }
                } else {
                    field.push(bytes[i] as char);
                    i += 1;
                }
            }
            fields.push(field);
            if i < bytes.len() && bytes[i] == b',' {
                i += 1;
            }
        } else {
            // Unquoted field
            let start = i;
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            fields.push(String::from_utf8_lossy(&bytes[start..i]).to_string());
            if i < bytes.len() && bytes[i] == b',' {
                i += 1;
            }
        }
    }

    fields
}

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
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let lines: Vec<&str> = input.lines().filter(|l| !l.is_empty()).collect();
    if lines.len() < 2 {
        return Ok(vec![]);
    }

    let headers = parse_csv_line(lines[0]);
    let rows: Vec<Vec<String>> = lines[1..].iter().map(|l| parse_csv_line(l)).collect();

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
    fn test_parse_csv_line_simple() {
        let fields = parse_csv_line("a,b,c");
        assert_eq!(fields, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_parse_csv_line_quoted() {
        let fields = parse_csv_line("\"hello, world\",b,\"c\"\"d\"");
        assert_eq!(fields, vec!["hello, world", "b", "c\"d"]);
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
