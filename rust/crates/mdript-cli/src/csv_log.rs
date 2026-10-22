use chrono::Local;
use serde_json::Value;
use std::{fs, fs::OpenOptions, io::Write, path::Path};

const HEADER: [&str; 9] = [
    "日時",
    "状態",
    "カード名",
    "カードID",
    "距離",
    "2位との差",
    "出力ファイル",
    "備考",
    "入力ファイル",
];

fn safe_cell(value: &str) -> String {
    if matches!(
        value.chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    ) {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

fn field(value: &str) -> String {
    let value = safe_cell(value);
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

fn row(fields: &[String]) -> String {
    let mut line = fields
        .iter()
        .map(|v| field(v))
        .collect::<Vec<_>>()
        .join(",");
    line.push_str("\r\n");
    line
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

pub fn write_csv(output: &Path, results: &[Value]) -> std::io::Result<()> {
    if results.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(output)?;
    let path = output.join("log.csv");
    let new = !path.exists();
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut bytes = Vec::new();
    if new {
        bytes.extend_from_slice(b"\xef\xbb\xbf");
        bytes.extend_from_slice(row(&HEADER.map(str::to_owned)).as_bytes());
    }
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for result in results {
        let values = [
            now.clone(),
            text(&result["status"]),
            text(&result["name"]),
            text(&result["card_id"]),
            text(&result["distance"]),
            text(&result["margin"]),
            text(&result["saved_path"]),
            text(&result["note"]),
            text(&result["source"]),
        ];
        bytes.extend_from_slice(row(&values).as_bytes());
    }
    file.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn csv_appends_with_one_bom_header_and_escaped_cells() {
        let dir = std::env::temp_dir().join(format!("mdript-csv-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let result = json!({
            "status":"確定", "name":"=SUM(1,2)", "card_id":"42", "distance":3,
            "margin":7, "saved_path":"C:\\cards\\a.png", "note":"a\"b\nline",
            "source":"@source.png"
        });
        write_csv(&dir, std::slice::from_ref(&result)).unwrap();
        write_csv(&dir, &[result]).unwrap();
        let bytes = fs::read(dir.join("log.csv")).unwrap();
        assert!(bytes.starts_with(b"\xef\xbb\xbf"));
        let content = String::from_utf8(bytes[3..].to_vec()).unwrap();
        assert_eq!(content.matches("日時,状態").count(), 1);
        assert_eq!(content.matches("'=SUM(1,2)").count(), 2);
        assert_eq!(content.matches("'@source.png").count(), 2);
        assert_eq!(content.matches("\"a\"\"b\nline\"").count(), 2);
        assert_eq!(content.matches("\r\n").count(), 3);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_results_do_not_create_output() {
        let dir =
            std::env::temp_dir().join(format!("mdript-empty-csv-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_csv(&dir, &[]).unwrap();
        assert!(!dir.exists());
    }
}
