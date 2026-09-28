// Copyright 2026 AsterSQL.

//! File import through canonical dump parsers and session KV writes. Only the
//! object-storage boundary is injected; compressed bytes are decoded here.
use super::*;
use astersql_lightning_mydump as dump;
use astersql_objstore_compressedio::{CompressType, DecompressConfig, new_reader};
use std::io::Read;

fn import_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::new(error.to_string())
}

fn source_matches(pattern: &str, source: &str) -> bool {
    let (pattern, query) = pattern.split_once('?').unwrap_or((pattern, ""));
    let (source, source_query) = source.split_once('?').unwrap_or((source, ""));
    if query != source_query {
        return false;
    }
    let mut previous = vec![false; source.len() + 1];
    previous[0] = true;
    for byte in pattern.bytes() {
        let mut next = vec![false; source.len() + 1];
        next[0] = byte == b'*' && previous[0];
        for (index, value) in source.bytes().enumerate() {
            next[index + 1] = if byte == b'*' {
                previous[index + 1] || next[index]
            } else {
                byte == value && previous[index]
            };
        }
        previous = next;
    }
    previous[source.len()]
}

impl ConcreteSession {
    pub(super) fn execute_import_compression(
        &self,
        statement: &ast::ImportIntoStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        if statement.Select.is_some()
            || !statement.ColumnAssignments.is_empty()
            || !statement.ColumnsAndUserVars.is_empty()
        {
            return Err(import_error("file import column mapping is not configured"));
        }
        let mut skip = 0_u64;
        let mut csv = dump::CsvConfig::default();
        for option in &statement.Options {
            let value = option
                .Value
                .as_ref()
                .map(|expr| crate::dml_runtime::EvalExpr(expr, &HashMap::new(), None))
                .transpose()?
                .flatten();
            let required = || {
                value
                    .clone()
                    .ok_or_else(|| import_error(format!("{} requires a value", option.Name)))
            };
            match option.Name.to_ascii_lowercase().as_str() {
                "thread" => {
                    let threads: u64 = required()?.parse().map_err(import_error)?;
                    if threads == 0 {
                        return Err(import_error("thread must be positive"));
                    }
                }
                "skip_rows" => skip = required()?.parse().map_err(import_error)?,
                "lines_terminated_by" => csv.lines_terminated_by = required()?,
                "fields_terminated_by" => csv.fields_terminated_by = required()?,
                "fields_enclosed_by" => csv.fields_enclosed_by = required()?,
                "fields_escaped_by" => csv.fields_escaped_by = required()?,
                other => {
                    return Err(import_error(format!(
                        "unsupported file import option {other}"
                    )));
                }
            }
        }
        let storage = self
            .import_files
            .borrow()
            .storage
            .clone()
            .ok_or_else(|| import_error("import object storage is not configured"))?;
        let mut sources = storage.list().map_err(import_error)?;
        sources.retain(|(path, _)| source_matches(&statement.Path, path));
        sources.sort_by(|left, right| left.0.cmp(&right.0));
        if sources.is_empty() {
            return Err(import_error(format!(
                "source file {} not found",
                statement.Path
            )));
        }
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.O.clone()
        };
        let table = self
            .resolve_runtime_table(&database, &statement.Table.Name.L)
            .ok_or_else(|| import_error("import target table not found"))?;
        let mut lists = Vec::new();
        for (path, _) in sources {
            let name = path.split('?').next().unwrap_or(&path).to_ascii_lowercase();
            let (kind, plain) = if let Some(plain) = name
                .strip_suffix(".gz")
                .or_else(|| name.strip_suffix(".gzip"))
            {
                (Some(CompressType::Gzip), plain)
            } else if let Some(plain) = name
                .strip_suffix(".zst")
                .or_else(|| name.strip_suffix(".zstd"))
            {
                (Some(CompressType::Zstd), plain)
            } else if let Some(plain) = name.strip_suffix(".snappy") {
                (Some(CompressType::Snappy), plain)
            } else {
                (None, name.as_str())
            };
            let raw = storage
                .open(&path, dump::Compression::None)
                .map_err(import_error)?;
            let mut reader: Box<dyn Read + Send> = if let Some(kind) = kind {
                new_reader(
                    kind,
                    DecompressConfig {
                        zstd_decode_concurrency: 1,
                    },
                    raw,
                )
                .map_err(|e| import_error(format!("decompress {path}: {e}")))?
                .unwrap()
            } else {
                raw
            };
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .map_err(|e| import_error(format!("decompress {path}: {e}")))?;
            let format = statement
                .Format
                .as_deref()
                .unwrap_or(if plain.ends_with(".sql") {
                    "sql"
                } else {
                    "csv"
                });
            let input = Box::new(dump::StringReader::from_bytes(bytes));
            let mut parser: Box<dyn dump::Parser> = match format.to_ascii_lowercase().as_str() {
                "csv" => {
                    Box::new(dump::NewCSVParser(&csv, input, false, None).map_err(import_error)?)
                }
                "sql" => Box::new(dump::NewChunkParser(input, 64 * 1024, None, false)),
                other => return Err(import_error(format!("unsupported import format {other}"))),
            };
            let parsed = (|| -> SessionResult<()> {
                let mut skipped = 0;
                loop {
                    match parser.ReadRow() {
                        Ok(()) => {}
                        Err(dump::Error::Eof) => break,
                        Err(error) => return Err(import_error(error)),
                    }
                    if skipped < skip {
                        skipped += 1;
                        continue;
                    }
                    let row = parser.LastRow();
                    if row.row.len() != table.Columns.iter().filter(|column| !column.Hidden).count()
                    {
                        return Err(import_error(
                            "import field count does not match target table",
                        ));
                    }
                    let values: Result<Vec<_>, _> = row
                        .row
                        .iter()
                        .map(|value| {
                            let value = match value {
                                dump::Datum::Null => None,
                                dump::Datum::I64(value) => Some(value.to_string()),
                                dump::Datum::Bytes(value) | dump::Datum::Binary(value) => {
                                    Some(String::from_utf8(value.clone()).map_err(import_error)?)
                                }
                            };
                            Ok::<_, SessionError>(ast::NewValueExpr(
                                value,
                                "utf8mb4",
                                "utf8mb4_bin",
                            ))
                        })
                        .collect();
                    lists.push(values?);
                }
                Ok(())
            })();
            let closed = parser.Close().map_err(import_error);
            parsed?;
            closed?;
        }
        let count = lists.len();
        if count > 0 {
            self.execute_insert(&ast::InsertStmt {
                Table: Some(ast::TableRefsClause {
                    TableRefs: ast::Join {
                        Left: Some(Box::new(ast::ResultSetNode::TableSource(
                            ast::TableSource {
                                Source: statement.Table.clone(),
                                ..Default::default()
                            },
                        ))),
                        ..Default::default()
                    },
                }),
                Lists: lists,
                ..Default::default()
            })?;
        }
        Ok(ConcreteRecordSet::new(
            vec!["Imported_Rows".into()],
            vec![vec![count.to_string()]],
        ))
    }
}

#[cfg(test)]
#[path = "import_compression_test.rs"]
mod tests;
