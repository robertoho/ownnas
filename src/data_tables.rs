//! Data files shown as tables: Parquet, Arrow IPC and Feather (version 2), Avro, and JSON Lines.
//! The first rows are read, with each column's name and type as the header; the file is never
//! read whole.

use std::path::Path;

use crate::documents::{Sheet, View};

const MAX_ROWS: usize = 2000;
const MAX_COLUMNS: usize = 60;
/// Longest text shown in a cell.
const MAX_CELL: usize = 300;

fn short(text: String) -> String {
    if text.chars().count() > MAX_CELL {
        let mut cut: String = text.chars().take(MAX_CELL).collect();
        cut.push('…');
        cut
    } else {
        text
    }
}

pub fn view(path: &Path) -> Result<View, String> {
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let sheet = match extension.as_str() {
        "parquet" | "pq" => parquet(path, name)?,
        "arrow" | "feather" | "ipc" | "arrows" => arrow(path, name)?,
        "avro" => avro(path, name)?,
        "jsonl" | "ndjson" => json_lines(path, name)?,
        _ => return Err("This data format is not read here".into()),
    };
    Ok(View::Sheets { sheets: vec![sheet] })
}

/// Rows of Arrow record batches as text, under a header of "name (type)".
fn batches_sheet(name: String, schema: &arrow_array::RecordBatch, batches: impl Iterator<Item = Result<arrow_array::RecordBatch, String>>, known_total: Option<usize>) -> Result<Sheet, String> {
    let fields = schema.schema();
    let total_columns = fields.fields().len();
    let mut rows = vec![fields.fields().iter().take(MAX_COLUMNS).map(|f| format!("{} ({})", f.name(), f.data_type())).collect::<Vec<_>>()];
    let mut counted = 0usize;
    let options = arrow_cast::display::FormatOptions::default().with_null("");
    for batch in std::iter::once(Ok(schema.clone())).chain(batches) {
        let batch = batch?;
        if rows.len() <= MAX_ROWS {
            let formatters: Vec<_> = batch.columns().iter().take(MAX_COLUMNS).map(|column| arrow_cast::display::ArrayFormatter::try_new(column.as_ref(), &options)).collect::<Result<_, _>>().map_err(|err| err.to_string())?;
            for row in 0..batch.num_rows() {
                if rows.len() > MAX_ROWS {
                    break;
                }
                rows.push(formatters.iter().map(|f| short(f.value(row).to_string())).collect());
            }
        }
        counted += batch.num_rows();
        if known_total.is_some() && rows.len() > MAX_ROWS {
            break;
        }
    }
    Ok(Sheet { name, rows, total_rows: known_total.unwrap_or(counted) + 1, total_columns, header: true })
}

fn parquet(path: &Path, name: String) -> Result<Sheet, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
    let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file).map_err(|_| "This Parquet file could not be read".to_string())?;
    let total = builder.metadata().file_metadata().num_rows().max(0) as usize;
    let mut reader = builder.with_batch_size(1024).build().map_err(|_| "This Parquet file could not be read".to_string())?;
    let first = match reader.next() {
        Some(batch) => batch.map_err(|err| err.to_string())?,
        None => return Ok(Sheet { name, rows: Vec::new(), total_rows: 0, total_columns: 0, header: true }),
    };
    batches_sheet(name, &first, reader.map(|b| b.map_err(|err| err.to_string())), Some(total))
}

fn arrow(path: &Path, name: String) -> Result<Sheet, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
    // The file format (Feather 2, .arrow) has a footer; the stream format does not.
    match arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(file), None) {
        Ok(mut reader) => {
            let first = match reader.next() {
                Some(batch) => batch.map_err(|err| err.to_string())?,
                None => return Ok(Sheet { name, rows: Vec::new(), total_rows: 0, total_columns: 0, header: true }),
            };
            batches_sheet(name, &first, reader.map(|b| b.map_err(|err| err.to_string())), None)
        }
        Err(_) => {
            let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
            let mut reader = arrow_ipc::reader::StreamReader::try_new(std::io::BufReader::new(file), None).map_err(|_| "This Arrow file could not be read (version 1 Feather files are not supported)".to_string())?;
            let first = match reader.next() {
                Some(batch) => batch.map_err(|err| err.to_string())?,
                None => return Ok(Sheet { name, rows: Vec::new(), total_rows: 0, total_columns: 0, header: true }),
            };
            batches_sheet(name, &first, reader.map(|b| b.map_err(|err| err.to_string())), None)
        }
    }
}

fn avro_text(value: &apache_avro::types::Value) -> String {
    use apache_avro::types::Value as V;
    match value {
        V::Null => String::new(),
        V::Boolean(b) => b.to_string(),
        V::Int(n) | V::Date(n) | V::TimeMillis(n) => n.to_string(),
        V::Long(n) | V::TimeMicros(n) | V::TimestampMillis(n) | V::TimestampMicros(n) | V::TimestampNanos(n) | V::LocalTimestampMillis(n) | V::LocalTimestampMicros(n) | V::LocalTimestampNanos(n) => n.to_string(),
        V::Float(n) => n.to_string(),
        V::Double(n) => n.to_string(),
        V::String(s) | V::Enum(_, s) => s.clone(),
        V::Bytes(b) | V::Fixed(_, b) => format!("<{} bytes>", b.len()),
        V::Union(_, inner) => avro_text(inner),
        V::Array(items) => format!("[{}]", items.iter().map(avro_text).collect::<Vec<_>>().join(", ")),
        V::Map(map) => format!("{{{}}}", map.iter().map(|(k, v)| format!("{k}: {}", avro_text(v))).collect::<Vec<_>>().join(", ")),
        V::Record(fields) => format!("{{{}}}", fields.iter().map(|(k, v)| format!("{k}: {}", avro_text(v))).collect::<Vec<_>>().join(", ")),
        V::Uuid(u) => u.to_string(),
        other => format!("{other:?}"),
    }
}

fn avro(path: &Path, name: String) -> Result<Sheet, String> {
    let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
    let reader = apache_avro::Reader::new(std::io::BufReader::new(file)).map_err(|_| "This Avro file could not be read".to_string())?;
    let mut header: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut total = 0usize;
    for value in reader {
        let value = value.map_err(|_| "This Avro file is damaged".to_string())?;
        total += 1;
        if rows.len() >= MAX_ROWS {
            continue;
        }
        match value {
            apache_avro::types::Value::Record(fields) => {
                if header.is_empty() {
                    header = fields.iter().map(|(k, _)| k.clone()).collect();
                }
                rows.push(fields.iter().take(MAX_COLUMNS).map(|(_, v)| short(avro_text(v))).collect());
            }
            other => {
                if header.is_empty() {
                    header = vec!["value".into()];
                }
                rows.push(vec![short(avro_text(&other))]);
            }
        }
    }
    let total_columns = header.len();
    rows.insert(0, header.into_iter().take(MAX_COLUMNS).collect());
    Ok(Sheet { name, rows, total_rows: total + 1, total_columns, header: true })
}

/// JSON Lines: one JSON value per line; objects' keys become the columns.
fn json_lines(path: &Path, name: String) -> Result<Sheet, String> {
    use std::io::BufRead;
    let file = std::fs::File::open(path).map_err(|_| "Could not read the file".to_string())?;
    let mut keys: Vec<String> = Vec::new();
    let mut values: Vec<serde_json::Value> = Vec::new();
    let mut total = 0usize;
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|_| "Could not read the file".to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if values.len() >= MAX_ROWS {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(&line).unwrap_or(serde_json::Value::String(line));
        if let serde_json::Value::Object(map) = &value {
            for key in map.keys() {
                if !keys.contains(key) && keys.len() < MAX_COLUMNS {
                    keys.push(key.clone());
                }
            }
        }
        values.push(value);
    }
    if keys.is_empty() {
        keys.push("value".into());
    }
    let cell = |v: Option<&serde_json::Value>| match v {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => short(s.clone()),
        Some(other) => short(other.to_string()),
    };
    let mut rows = vec![keys.clone()];
    for value in &values {
        rows.push(match value {
            serde_json::Value::Object(map) => keys.iter().map(|k| cell(map.get(k))).collect(),
            other => vec![cell(Some(other))],
        });
    }
    Ok(Sheet { name, rows, total_rows: total + 1, total_columns: keys.len(), header: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn reads_parquet_arrow_and_json_lines() {
        let dir = std::env::temp_dir().join(format!("ownnas-data-{}-{}", std::process::id(), rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        let batch = arrow_array::RecordBatch::try_from_iter(vec![
            ("city", Arc::new(arrow_array::StringArray::from(vec!["Kraków", "Lima"])) as arrow_array::ArrayRef),
            ("people", Arc::new(arrow_array::Int64Array::from(vec![Some(800_000), None])) as arrow_array::ArrayRef),
        ])
        .unwrap();
        let parquet_path = dir.join("cities.parquet");
        let mut writer = parquet::arrow::ArrowWriter::try_new(std::fs::File::create(&parquet_path).unwrap(), batch.schema(), None).unwrap();
        writer.write(&batch).unwrap();
        writer.close().unwrap();
        let arrow_path = dir.join("cities.arrow");
        let mut writer = arrow_ipc::writer::FileWriter::try_new(std::fs::File::create(&arrow_path).unwrap(), &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
        for path in [&parquet_path, &arrow_path] {
            let View::Sheets { sheets } = view(path).unwrap() else { panic!() };
            assert_eq!(sheets[0].rows, [vec!["city (Utf8)".to_string(), "people (Int64)".into()], vec!["Kraków".into(), "800000".into()], vec!["Lima".into(), "".into()]]);
            assert_eq!(sheets[0].total_rows, 3);
        }
        let schema = apache_avro::Schema::parse_str(r#"{"type":"record","name":"Visit","fields":[{"name":"page","type":"string"},{"name":"seconds","type":["null","long"]}]}"#).unwrap();
        let mut writer = apache_avro::Writer::new(&schema, Vec::new()).unwrap();
        let mut record = apache_avro::types::Record::new(&schema).unwrap();
        record.put("page", "home");
        record.put("seconds", apache_avro::types::Value::Union(1, Box::new(apache_avro::types::Value::Long(42))));
        writer.append_value(record).unwrap();
        let avro_path = dir.join("visits.avro");
        std::fs::write(&avro_path, writer.into_inner().unwrap()).unwrap();
        let View::Sheets { sheets } = view(&avro_path).unwrap() else { panic!() };
        assert_eq!(sheets[0].rows, [vec!["page".to_string(), "seconds".into()], vec!["home".into(), "42".into()]]);
        let lines = dir.join("events.jsonl");
        std::fs::write(&lines, "{\"a\":1,\"b\":\"x\"}\n\n{\"a\":2,\"c\":[1,2]}\n").unwrap();
        let View::Sheets { sheets } = view(&lines).unwrap() else { panic!() };
        assert_eq!(sheets[0].rows, [vec!["a".to_string(), "b".into(), "c".into()], vec!["1".into(), "x".into(), "".into()], vec!["2".into(), "".into(), "[1,2]".into()]]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
