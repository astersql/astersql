// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Canonical-session client-local and remote `LOAD DATA` execution.

use super::*;
use astersql_lightning_mydump::{
    CsvConfig, Datum as MydumpDatum, Error as MydumpError, NewCSVParser, Parser, StringReader,
};
use astersql_objstore::azblob::StorageOptions;
use astersql_objstore::gcs::{GCSConfig, new_gcs_storage};
use astersql_objstore::parse::{ParseBackend, StorageBackend};
use astersql_objstore::storeapi::Storage as _;
use astersql_objstore::storeapi::WalkOption;
use astersql_objstore_compressedio::{CompressType, DecompressConfig, new_reader};

type LocalLoadDataReader = Box<dyn std::io::Read + Send>;
thread_local! {
    static LOCAL_LOAD_DATA_READERS: std::cell::RefCell<HashMap<usize, LocalLoadDataReader>> = std::cell::RefCell::new(HashMap::new());
}

#[derive(Clone)]
enum LoadFieldTarget {
    Column(ast::ColumnName),
    UserVariable(String),
}

fn load_data_error(code: u16, sql_state: &str, message: impl Into<String>) -> SessionError {
    SessionError::new(format!("ERROR {code} ({sql_state}): {}", message.into()))
}

fn mydump_value(value: &MydumpDatum) -> Option<String> {
    match value {
        MydumpDatum::Null => None,
        MydumpDatum::I64(value) => Some(value.to_string()),
        MydumpDatum::Bytes(value) | MydumpDatum::Binary(value) => {
            Some(String::from_utf8_lossy(value).into_owned())
        }
    }
}

fn load_data_glob_match(pattern: &str, value: &str) -> bool {
    fn matches(pattern: &[u8], value: &[u8]) -> bool {
        match pattern.first() {
            None => value.is_empty(),
            Some(b'*') => {
                matches(&pattern[1..], value)
                    || (!value.is_empty() && matches(pattern, &value[1..]))
            }
            Some(b'[') => {
                let Some(end) = pattern.iter().position(|byte| *byte == b']') else {
                    return !value.is_empty()
                        && pattern[0] == value[0]
                        && matches(&pattern[1..], &value[1..]);
                };
                !value.is_empty()
                    && pattern[1..end].contains(&value[0])
                    && matches(&pattern[end + 1..], &value[1..])
            }
            Some(byte) => {
                !value.is_empty() && *byte == value[0] && matches(&pattern[1..], &value[1..])
            }
        }
    }
    matches(pattern.as_bytes(), value.as_bytes())
}

fn decode_load_data_file(name: &str, bytes: Vec<u8>) -> SessionResult<Vec<u8>> {
    if !name.ends_with(".gz") {
        return Ok(bytes);
    }
    let mut reader = new_reader(
        CompressType::Gzip,
        DecompressConfig::default(),
        Box::new(std::io::Cursor::new(bytes)),
    )
    .map_err(|error| load_data_error(8160, "HY000", error.to_string()))?
    .expect("gzip returns a decompression reader");
    let mut decoded = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut decoded)
        .map_err(|error| load_data_error(8160, "HY000", error.to_string()))?;
    Ok(decoded)
}

fn load_numeric(value: &str) -> (f64, bool) {
    let trimmed = value.trim_start();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    for (index, character) in trimmed.char_indices() {
        let allowed_sign = index == 0 && matches!(character, '+' | '-');
        if character.is_ascii_digit() {
            seen_digit = true;
            end = index + character.len_utf8();
        } else if character == '.' && !seen_dot {
            seen_dot = true;
            end = index + 1;
        } else if allowed_sign {
            end = 1;
        } else {
            break;
        }
    }
    if !seen_digit {
        return (0.0, !value.is_empty());
    }
    let numeric = trimmed[..end].parse::<f64>().unwrap_or(0.0);
    (numeric, !trimmed[end..].trim().is_empty())
}

fn load_assignment_value(
    expression: &ast::ExprNode,
    variables: &HashMap<String, Option<String>>,
) -> SessionResult<(Option<String>, Vec<String>)> {
    match &expression.Kind {
        ast::ExprKind::Variable { Name, .. } => Ok((
            variables
                .get(&Name.trim_start_matches('@').to_ascii_lowercase())
                .cloned()
                .flatten(),
            Vec::new(),
        )),
        ast::ExprKind::Parentheses(inner) => load_assignment_value(inner, variables),
        ast::ExprKind::Cast {
            Expr,
            Tp,
            FunctionType,
            ExplicitCharSet,
        } => {
            let (value, warnings) = load_assignment_value(Expr, variables)?;
            let cast = ast::ExprNode {
                node_text: Default::default(),
                Kind: ast::ExprKind::Cast {
                    Expr: Box::new(ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin")),
                    Tp: Tp.clone(),
                    FunctionType: FunctionType.clone(),
                    ExplicitCharSet: *ExplicitCharSet,
                },
                OriginTextPosition: expression.OriginTextPosition,
                Flag: Default::default(),
            };
            crate::dml_runtime::EvalExpr(&cast, &HashMap::new(), None)
                .map(|value| (value, warnings))
        }
        ast::ExprKind::Binary { Op, L, R }
            if matches!(Op.as_str(), "+" | "-" | "*" | "/" | "%") =>
        {
            let (left, mut warnings) = load_assignment_value(L, variables)?;
            let (right, right_warnings) = load_assignment_value(R, variables)?;
            warnings.extend(right_warnings);
            let (Some(left), Some(right)) = (left, right) else {
                return Ok((None, warnings));
            };
            let (left_number, left_truncated) = load_numeric(&left);
            let (right_number, right_truncated) = load_numeric(&right);
            if left_truncated {
                warnings.push(format!("Truncated incorrect DOUBLE value: '{left}'"));
            }
            if right_truncated {
                warnings.push(format!("Truncated incorrect DOUBLE value: '{right}'"));
            }
            let value = match Op.as_str() {
                "+" => left_number + right_number,
                "-" => left_number - right_number,
                "*" => left_number * right_number,
                "/" if right_number == 0.0 => return Ok((None, warnings)),
                "/" => left_number / right_number,
                "%" if right_number == 0.0 => return Ok((None, warnings)),
                "%" => left_number % right_number,
                _ => unreachable!(),
            };
            let text = if value.fract() == 0.0 {
                format!("{value:.0}")
            } else {
                value.to_string()
            };
            Ok((Some(text), warnings))
        }
        _ => crate::dml_runtime::EvalExpr(expression, &HashMap::new(), None)
            .map(|value| (value, Vec::new())),
    }
}

fn load_data_table_refs(table: &ast::TableName) -> ast::TableRefsClause {
    ast::TableRefsClause {
        TableRefs: ast::Join {
            Left: Some(Box::new(ast::ResultSetNode::TableSource(
                ast::TableSource {
                    Source: table.clone(),
                    ..Default::default()
                },
            ))),
            ..Default::default()
        },
    }
}

impl ConcreteSession {
    /// Execute client-local LOAD DATA with a request-owned reader. Dropping the
    /// reader closes its resources on both success and error; it cannot leak
    /// into the next statement or another session.
    pub fn execute_with_load_data_reader<R: std::io::Read + Send + 'static>(
        &self,
        sql: &str,
        reader: R,
    ) -> SessionResult<Vec<ConcreteRecordSet>> {
        let ru_scope = super::typed_adapter_bridge::FileTransferStatementRUScope::new(self);
        struct Restore(usize, Option<LocalLoadDataReader>);
        impl Drop for Restore {
            fn drop(&mut self) {
                LOCAL_LOAD_DATA_READERS.with(|readers| {
                    let mut readers = readers.borrow_mut();
                    readers.remove(&self.0);
                    if let Some(previous) = self.1.take() {
                        readers.insert(self.0, previous);
                    }
                });
            }
        }
        let id = std::rc::Rc::as_ptr(&self.inner) as usize;
        let previous = LOCAL_LOAD_DATA_READERS
            .with(|readers| readers.borrow_mut().insert(id, Box::new(reader)));
        let restore = Restore(id, previous);
        let execution = self.execute(sql);
        drop(restore);
        if execution.is_ok() {
            ru_scope.finish()?;
        }
        execution
    }

    pub(super) fn has_file_transfer_reader(&self) -> bool {
        LOCAL_LOAD_DATA_READERS.with(|readers| {
            readers
                .borrow()
                .contains_key(&(std::rc::Rc::as_ptr(&self.inner) as usize))
        })
    }

    /// Return the latest MySQL OK-packet message.
    pub fn LastMessage(&self) -> String {
        self.state.borrow().last_message.clone()
    }

    /// Return the SQL text stored in Go's `sessionctx.QueryString` slot.
    pub fn QueryString(&self) -> String {
        self.state.borrow().last_query_string.clone()
    }

    /// Execute the same validation, client/remote read, row mapping and
    /// relational mutation pipeline as Go's `LoadDataExec`.
    pub(super) fn execute_load_data(&self, statement: &ast::LoadDataStmt) -> SessionResult<()> {
        let database = if statement.Table.Schema.L.is_empty() {
            self.current_database()
        } else {
            statement.Table.Schema.O.clone()
        };
        if database.is_empty() {
            return Err(load_data_error(1046, "3D000", "No database selected"));
        }

        let table_name = statement.Table.Name.O.clone();
        let table = self
            .resolve_runtime_table(&database, &statement.Table.Name.L)
            .ok_or_else(|| {
                load_data_error(
                    1146,
                    "42S02",
                    format!("Table '{database}.{table_name}' doesn't exist"),
                )
            })?;
        if table.View.is_some() || table.Sequence.is_some() {
            return Err(load_data_error(
                1288,
                "HY000",
                format!("The target table {table_name} of the LOAD is not updatable"),
            ));
        }
        let insertable_columns = table
            .Columns
            .iter()
            .filter(|column| !column.Hidden && !column.IsGenerated())
            .collect::<Vec<_>>();
        let available_columns = insertable_columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<HashSet<_>>();
        let mut seen_columns = HashSet::new();
        let mut validate_column = |column: &ast::ColumnName| -> SessionResult<()> {
            if !available_columns.contains(column.Name.L.as_str()) {
                return Err(load_data_error(
                    1054,
                    "42S22",
                    format!("Unknown column '{}' in 'field list'", column.Name.O),
                ));
            }
            if !seen_columns.insert(column.Name.L.clone()) {
                return Err(load_data_error(
                    1110,
                    "42000",
                    format!("Column '{}' specified twice", column.Name.O),
                ));
            }
            Ok(())
        };

        let mut field_targets = Vec::new();
        if !statement.ColumnsAndUserVars.is_empty() {
            for item in &statement.ColumnsAndUserVars {
                if let Some(column) = item.ColumnName.as_ref() {
                    validate_column(column)?;
                    field_targets.push(LoadFieldTarget::Column(column.clone()));
                } else if let Some(ast::ExprNode {
                    Kind: ast::ExprKind::Variable { Name, .. },
                    ..
                }) = item.UserVar.as_ref()
                {
                    field_targets.push(LoadFieldTarget::UserVariable(
                        Name.trim_start_matches('@').to_ascii_lowercase(),
                    ));
                }
            }
        } else if !statement.Columns.is_empty() {
            for column in &statement.Columns {
                validate_column(column)?;
                field_targets.push(LoadFieldTarget::Column(column.clone()));
            }
        } else {
            // Go builds the implicit field list from every visible table column,
            // including generated columns. An input field at a generated-column
            // position is consumed but never inserted; later ordinary columns
            // therefore keep their physical CSV position.
            field_targets.extend(table.Columns.iter().filter(|column| !column.Hidden).map(
                |column| {
                    LoadFieldTarget::Column(ast::ColumnName {
                        Name: column.Name.clone(),
                        ..Default::default()
                    })
                },
            ));
        }
        for assignment in &statement.ColumnAssignments {
            if !available_columns.contains(assignment.Column.Name.L.as_str()) {
                return Err(load_data_error(
                    1054,
                    "42S22",
                    format!(
                        "Unknown column '{}' in 'field list'",
                        assignment.Column.Name.O
                    ),
                ));
            }
        }

        // Keep a supplied reader alive through the transaction, including errors.
        let mut supplied_reader = LOCAL_LOAD_DATA_READERS.with(|readers| {
            readers
                .borrow_mut()
                .remove(&(std::rc::Rc::as_ptr(&self.inner) as usize))
        });
        let source_files = if matches!(statement.FileLocRef, ast::FileLocRef::Client) {
            if let Some(reader) = supplied_reader.as_mut() {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(reader, &mut bytes).map_err(|error| {
                    SessionError::new(format!(
                        "failed to read local infile {:?}: {error}",
                        statement.Path
                    ))
                })?;
                vec![(statement.Path.clone(), bytes)]
            } else {
                vec![(
                    statement.Path.clone(),
                    std::fs::read(&statement.Path).map_err(|error| {
                        SessionError::new(format!(
                            "failed to read local infile {:?}: {error}",
                            statement.Path
                        ))
                    })?,
                )]
            }
        } else {
            let backend = ParseBackend(&statement.Path, None).map_err(|error| {
                load_data_error(
                    8158,
                    "HY000",
                    format!(
                        "The URI of data source is invalid. Reason: {error}: invalid external storage config. Please provide a valid URI, such as"
                    ),
                )
            })?;
            match backend {
                StorageBackend::Gcs(gcs) => {
                    let bucket = gcs.bucket;
                    let object = gcs.prefix;
                    let storage = new_gcs_storage(
                        GCSConfig {
                            endpoint: gcs.endpoint,
                            bucket: bucket.clone(),
                            storage_class: gcs.storage_class,
                            predefined_acl: gcs.predefined_acl,
                            credentials_blob: gcs.credentials_blob,
                            ..Default::default()
                        },
                        &StorageOptions {
                            no_credentials: true,
                            send_credentials: false,
                        },
                    )
                    .map_err(|error| {
                        load_data_error(
                            8159,
                            "HY000",
                            format!("Access to the data source has been denied. Reason: {error}"),
                        )
                    })?;
                    let context = astersql_objstore::objectio::Context::default();
                    let names = if object.contains('*') || object.contains('[') {
                        let mut names = Vec::new();
                        storage
                            .WalkDir(&context, Some(&WalkOption::default()), &mut |name, _| {
                                if load_data_glob_match(&object, name) {
                                    names.push(name.to_owned());
                                }
                                Ok(())
                            })
                            .map_err(|error| {
                                load_data_error(
                                    8160,
                                    "HY000",
                                    format!("Failed to read source files. Reason: {error}"),
                                )
                            })?;
                        names.sort();
                        names
                    } else {
                        vec![object.clone()]
                    };
                    if names.is_empty() {
                        return Err(load_data_error(
                            8160,
                            "HY000",
                            format!(
                                "Failed to read source files. Reason: the object doesn't exist, file info: input.bucket='{bucket}', input.key='{object}': storage: object doesn't exist. Please check the file location is correct"
                            ),
                        ));
                    }
                    let mut files = Vec::with_capacity(names.len());
                    for name in names {
                        let bytes = storage.ReadFile(&context, &name).map_err(|_| {
                            load_data_error(8160, "HY000", format!("Failed to read source files. Reason: the object doesn't exist, file info: input.bucket='{bucket}', input.key='{name}': storage: object doesn't exist. Please check the file location is correct"))
                        })?;
                        files.push((name, bytes));
                    }
                    files
                }
                StorageBackend::S3(s3) => {
                    return Err(load_data_error(
                        8159,
                        "HY000",
                        format!(
                            "Access to the data source has been denied. Reason: failed to get region of bucket {}",
                            s3.bucket
                        ),
                    ));
                }
                StorageBackend::Local(_) => {
                    return Err(load_data_error(
                        8154,
                        "HY000",
                        "Don't support load data from tidb-server's disk.",
                    ));
                }
                other => {
                    return Err(load_data_error(
                        8158,
                        "HY000",
                        format!(
                            "The URI of data source is invalid. Reason: storage {} not support yet: invalid external storage config. Please provide a valid URI, such as",
                            other.kind()
                        ),
                    ));
                }
            }
        };

        let line_terminated_by = statement
            .LinesInfo
            .as_ref()
            .and_then(|lines| lines.Terminated.clone())
            .unwrap_or_else(|| "\n".to_owned());
        let line_starting_by = statement
            .LinesInfo
            .as_ref()
            .and_then(|lines| lines.Starting.clone())
            .unwrap_or_default();
        if line_terminated_by.is_empty() {
            return Err(load_data_error(
                8162,
                "HY000",
                "LINES TERMINATED BY is empty",
            ));
        }
        if line_starting_by.contains(&line_terminated_by) {
            return Err(load_data_error(
                8162,
                "HY000",
                format!(
                    "STARTING BY '{line_starting_by}' cannot contain LINES TERMINATED BY '{line_terminated_by}'"
                ),
            ));
        }
        let fields = statement.FieldsInfo.as_ref();
        let mut bytes = Vec::new();
        for (name, encoded) in source_files {
            let mut file = decode_load_data_file(&name, encoded)?;
            let mut skip_offset = 0;
            for _ in 0..statement.IgnoreLines.unwrap_or_default() {
                let terminator = line_terminated_by.as_bytes();
                let Some(offset) = file[skip_offset..]
                    .windows(terminator.len())
                    .position(|window| window == terminator)
                else {
                    skip_offset = file.len();
                    break;
                };
                skip_offset += offset + terminator.len();
            }
            if !bytes.is_empty()
                && !bytes.ends_with(line_terminated_by.as_bytes())
                && skip_offset < file.len()
            {
                bytes.extend_from_slice(line_terminated_by.as_bytes());
            }
            bytes.extend_from_slice(&file[skip_offset..]);
        }
        let csv = CsvConfig {
            fields_terminated_by: fields
                .and_then(|fields| fields.Terminated.clone())
                .unwrap_or_else(|| "\t".to_owned()),
            fields_enclosed_by: fields
                .and_then(|fields| fields.Enclosed.clone())
                .unwrap_or_default(),
            lines_terminated_by: line_terminated_by,
            lines_starting_by: line_starting_by,
            fields_escaped_by: fields
                .and_then(|fields| fields.Escaped.clone())
                .unwrap_or_else(|| "\\".to_owned()),
            null: fields
                .and_then(|fields| fields.DefinedNullBy.clone())
                .unwrap_or_else(|| "\\N".to_owned()),
            quoted_null_is_text: fields.is_some_and(|fields| !fields.NullValueOptEnclosed),
            allow_empty_line: true,
            ..CsvConfig::default()
        };
        let mut parser = NewCSVParser(&csv, Box::new(StringReader::from_bytes(bytes)), false, None)
            .map_err(|error| load_data_error(8160, "HY000", error.to_string()))?;

        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .unwrap_or(astersql_parser_mysql::r#const::SQLMode(0));
        let restrictive = mode.HasStrictMode()
            && statement.OnDuplicate != ast::OnDuplicateKeyHandlingType::Ignore;
        let mut insert_columns = field_targets
            .iter()
            .filter_map(|target| match target {
                LoadFieldTarget::Column(column)
                    if available_columns.contains(column.Name.L.as_str()) =>
                {
                    Some(column.clone())
                }
                LoadFieldTarget::UserVariable(_) => None,
                LoadFieldTarget::Column(_) => None,
            })
            .collect::<Vec<_>>();
        for assignment in &statement.ColumnAssignments {
            if !insert_columns
                .iter()
                .any(|column| column.Name.L == assignment.Column.Name.L)
            {
                insert_columns.push(assignment.Column.clone());
            }
        }
        let mut rows = Vec::new();
        let mut row_number = 0_u64;
        loop {
            match parser.ReadRow() {
                Ok(()) => {}
                Err(MydumpError::Eof) => break,
                Err(error) => return Err(load_data_error(8160, "HY000", error.to_string())),
            }
            row_number += 1;
            let parsed = parser.LastRow().row;
            if parsed.len() > field_targets.len() {
                let message = format!(
                    "Row {row_number} was truncated; it contained more data than there were input columns"
                );
                if restrictive {
                    return Err(load_data_error(1262, "01000", message));
                }
                self.set_warning_with_code(1262, message);
            } else if parsed.len() < field_targets.len() {
                let message = format!("Row {row_number} doesn't contain data for all columns");
                if restrictive {
                    return Err(load_data_error(1261, "01000", message));
                }
                self.set_warning_with_code(1261, message);
            }

            let mut variables = HashMap::new();
            let mut values_by_column = HashMap::new();
            for (index, target) in field_targets.iter().enumerate() {
                let value = parsed.get(index).and_then(mydump_value);
                match target {
                    LoadFieldTarget::Column(column) => {
                        values_by_column.insert(column.Name.L.clone(), value);
                    }
                    LoadFieldTarget::UserVariable(name) => {
                        variables.insert(name.clone(), value);
                    }
                }
            }
            for assignment in &statement.ColumnAssignments {
                let (value, warnings) = load_assignment_value(&assignment.Expr, &variables)?;
                if let Some(warning) = warnings.first() {
                    if restrictive {
                        return Err(load_data_error(1292, "22007", warning));
                    }
                }
                for warning in warnings {
                    self.set_warning_with_code(1292, warning);
                }
                values_by_column.insert(assignment.Column.Name.L.clone(), value);
            }

            let mut expressions = Vec::with_capacity(insert_columns.len());
            for column in &insert_columns {
                let info = table
                    .Columns
                    .iter()
                    .find(|candidate| candidate.Name.L == column.Name.L)
                    .expect("validated LOAD DATA column");
                let mut value = values_by_column.remove(&column.Name.L).unwrap_or(None);
                if value.is_none() && astersql_parser_mysql::r#type::HasNotNullFlag(info.GetFlag())
                {
                    let message = format!(
                        "Column set to default value; NULL supplied to NOT NULL column '{}' at row {row_number}",
                        info.Name.O
                    );
                    if restrictive {
                        return Err(load_data_error(1263, "22004", message));
                    }
                    self.set_warning_with_code(1263, message);
                    value = Some("0".to_owned());
                }
                if let Some(text) = value.as_mut()
                    && info.GetType() == astersql_parser_mysql::r#type::TypeString
                    && info.GetFlen() >= 0
                    && text.chars().count() > info.GetFlen() as usize
                {
                    let message = format!(
                        "Data truncated for column '{}' at row {row_number}",
                        info.Name.O
                    );
                    if restrictive {
                        return Err(load_data_error(1265, "01000", message));
                    }
                    self.set_warning_with_code(1265, message);
                    *text = text.chars().take(info.GetFlen() as usize).collect();
                }
                if let Some(text) = value.as_mut()
                    && astersql_parser_mysql::util::IsIntegerType(info.GetType())
                    && text.parse::<i128>().is_err()
                {
                    let (numeric, truncated) = load_numeric(text);
                    if truncated {
                        self.set_warning_with_code(
                            1366,
                            format!(
                                "Incorrect integer value: '{text}' for column '{}' at row {row_number}",
                                info.Name.O
                            ),
                        );
                    }
                    *text = format!("{:.0}", numeric.trunc());
                }
                if let Some(text) = value.as_mut()
                    && info.GetType() == astersql_parser_mysql::r#type::TypeDate
                {
                    let trimmed = text.trim();
                    if trimmed.is_empty()
                        || trimmed.eq_ignore_ascii_case("null")
                        || trimmed.chars().all(|character| character == '0')
                    {
                        *text = "0000-00-00".to_owned();
                    } else if trimmed.len() != text.len() {
                        *text = trimmed.to_owned();
                    }
                }
                expressions.push(ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin"));
            }
            rows.push(expressions);
        }
        if rows.is_empty() {
            return Ok(());
        }

        let warning_start = self.state.borrow().current_warnings.len();
        let insert = ast::InsertStmt {
            Priority: if statement.LowPriority {
                astersql_parser_mysql::r#const::LowPriority.0
            } else {
                astersql_parser_mysql::r#const::NoPriority.0
            },
            IsReplace: statement.OnDuplicate == ast::OnDuplicateKeyHandlingType::Replace,
            // MySQL treats LOCAL input like IGNORE for duplicate-key handling
            // because the server cannot stop a client that is already sending
            // the file. Explicit REPLACE must still replace conflicting rows.
            IgnoreErr: statement.OnDuplicate == ast::OnDuplicateKeyHandlingType::Ignore
                || (matches!(statement.FileLocRef, ast::FileLocRef::Client)
                    && statement.OnDuplicate != ast::OnDuplicateKeyHandlingType::Replace),
            Table: Some(load_data_table_refs(&statement.Table)),
            Columns: insert_columns,
            Lists: rows,
            ..Default::default()
        };
        if astersql_testkit_testfailpoint::eval_bool("executor/commitOneTaskErr") {
            return Err(SessionError::new("mock commit one task error"));
        }
        let plan = crate::dml_runtime::PlanInsert(&insert)?;
        let (copied_rows, deleted) = match self.execute_relational_insert_with_load_counts(
            &insert,
            plan,
            false,
            None,
            Some(row_number),
        ) {
            Ok(counts) => counts,
            Err(error) => {
                let message = error.to_string();
                if let Some(message) = message.strip_prefix("[kv:1062]") {
                    return Err(load_data_error(1062, "23000", message));
                }
                return Err(error);
            }
        };
        for warning in self
            .state
            .borrow_mut()
            .current_warnings
            .iter_mut()
            .skip(warning_start)
        {
            if let Some(message) = warning.message.strip_prefix("[kv:1062]") {
                warning.code = 1062;
                warning.message = message.to_owned();
            }
        }
        let mut state = self.state.borrow_mut();
        let records = row_number;
        let skipped = records.saturating_sub(copied_rows);
        state.last_message = format!(
            "Records: {records}  Deleted: {deleted}  Skipped: {skipped}  Warnings: {}",
            state.current_warnings.len()
        );
        Ok(())
    }
}
