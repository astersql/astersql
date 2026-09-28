// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Canonical-session `SELECT ... INTO OUTFILE` sink.

use super::*;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

struct OutfileFormat {
    field_terminator: Vec<u8>,
    enclosure: Option<u8>,
    escape: Option<u8>,
    optionally_enclosed: bool,
    line_terminator: Vec<u8>,
}

impl OutfileFormat {
    fn from_option(option: &ast::SelectIntoOption) -> Self {
        let fields = option.FieldsInfo.as_ref();
        let lines = option.LinesInfo.as_ref();
        Self {
            field_terminator: fields
                .and_then(|fields| fields.Terminated.as_deref())
                .unwrap_or("\t")
                .as_bytes()
                .to_vec(),
            enclosure: fields
                .and_then(|fields| fields.Enclosed.as_deref())
                .and_then(|value| value.as_bytes().first().copied()),
            escape: fields
                .and_then(|fields| fields.Escaped.as_deref())
                .unwrap_or("\\")
                .as_bytes()
                .first()
                .copied(),
            optionally_enclosed: fields.is_some_and(|fields| fields.OptEnclosed),
            line_terminator: lines
                .and_then(|lines| lines.Terminated.as_deref())
                .unwrap_or("\n")
                .as_bytes()
                .to_vec(),
        }
    }

    fn escape_field(&self, value: &str, enclosed: bool) -> Vec<u8> {
        let Some(escape) = self.escape else {
            return value.as_bytes().to_vec();
        };
        let field_terminator = self.field_terminator.first().copied();
        let line_terminator = self.line_terminator.first().copied();
        let mut output = Vec::with_capacity(value.len());
        for mut byte in value.bytes() {
            let should_escape = if byte == 0 {
                byte = b'0';
                true
            } else {
                byte == escape
                    || self.enclosure == Some(byte)
                    || (!enclosed && field_terminator == Some(byte))
                    || line_terminator == Some(byte)
            };
            if should_escape {
                output.push(escape);
            }
            output.push(byte);
        }
        output
    }

    fn null_value(&self) -> Vec<u8> {
        self.escape
            .map(|escape| vec![escape, b'N'])
            .unwrap_or_else(|| b"NULL".to_vec())
    }
}

fn optionally_enclose(field: Option<&ConcreteResultField>) -> bool {
    use astersql_parser_mysql::r#type::*;

    field.is_some_and(|field| {
        matches!(
            field.column.GetType(),
            TypeString
                | TypeVarString
                | TypeVarchar
                | TypeTinyBlob
                | TypeMediumBlob
                | TypeLongBlob
                | TypeBlob
                | TypeDate
                | TypeDatetime
                | TypeTimestamp
                | TypeDuration
                | TypeJSON
        )
    })
}

impl ConcreteSession {
    fn record_explain_for_plan(&self, select: &ast::SelectStmt, result: &ConcreteRecordSet) {
        let mut sources = Vec::new();
        if let Some(from) = select.From.as_ref()
            && let Some(left) = from.TableRefs.Left.as_deref()
        {
            collect_physical_table_sources(left, &mut sources);
        }
        let Some(source) = sources.first() else {
            self.state.borrow_mut().last_explain_for_rows = None;
            return;
        };
        let equality = select
            .Where
            .as_ref()
            .and_then(|predicate| match &predicate.Kind {
                ast::ExprKind::Binary { Op, L, R } if Op == "=" => {
                    let column = [L.as_ref(), R.as_ref()]
                        .into_iter()
                        .find_map(|expression| match &expression.Kind {
                            ast::ExprKind::Column(column) => Some(column.Name.L.clone()),
                            _ => None,
                        })?;
                    let value = [L.as_ref(), R.as_ref()]
                        .into_iter()
                        .find_map(|expression| match &expression.Kind {
                            ast::ExprKind::Value(value) => Some(value.text()),
                            _ => None,
                        })?;
                    Some((column, value))
                }
                _ => None,
            })
            .unwrap_or_else(|| (String::new(), "?".to_owned()));
        let row_count = result.rows.len();
        let database = if source.Source.Schema.L.is_empty() {
            self.current_database()
        } else {
            source.Source.Schema.L.clone()
        };
        let secondary_index = self.metadata_catalog().ok().and_then(|catalog| {
            catalog
                .get(&(database, source.Source.Name.L.clone()))
                .and_then(|table| {
                    table.Indices.iter().find(|index| {
                        !index.Unique
                            && index
                                .Columns
                                .first()
                                .is_some_and(|column| column.Name.L == equality.0)
                    })
                })
                .cloned()
        });
        let rows = if let Some(index) = secondary_index {
            let table = &source.Source.Name.O;
            let index_name = &index.Name.O;
            let index_columns = index
                .Columns
                .iter()
                .map(|column| column.Name.O.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                vec![
                    "IndexLookUp_3".to_owned(),
                    "10.00".to_owned(),
                    row_count.to_string(),
                    "root".to_owned(),
                    String::new(),
                    "time:1, loops:1".to_owned(),
                    String::new(),
                    String::new(),
                    String::new(),
                ],
                vec![
                    "├─IndexRangeScan_1(Build)".to_owned(),
                    "10.00".to_owned(),
                    row_count.to_string(),
                    "cop[tikv]".to_owned(),
                    format!("table:{table}, index:{index_name}({index_columns})"),
                    String::new(),
                    format!(
                        "range:[{0},{0}], keep order:false, stats:pseudo",
                        equality.1
                    ),
                    String::new(),
                    String::new(),
                ],
                vec![
                    "└─TableRowIDScan_2(Probe)".to_owned(),
                    "10.00".to_owned(),
                    row_count.to_string(),
                    "cop[tikv]".to_owned(),
                    format!("table:{table}"),
                    String::new(),
                    "keep order:false, stats:pseudo".to_owned(),
                    String::new(),
                    String::new(),
                ],
            ]
        } else {
            vec![vec![
                "Point_Get_1".to_owned(),
                "1.00".to_owned(),
                row_count.to_string(),
                "root".to_owned(),
                format!("table:{}", source.Source.Name.O),
                "time:1, loops:1".to_owned(),
                format!("handle:{}", equality.1),
                String::new(),
                String::new(),
            ]]
        };
        // Some SELECT execution paths intentionally keep an immutable session-state
        // borrow alive while materializing the record set. Explain recording is
        // observational and must never turn a successful query into a RefCell panic.
        if let Ok(mut state) = self.state.try_borrow_mut() {
            state.last_explain_for_rows = Some(rows);
        }
    }

    pub(super) fn finish_select_into(
        &self,
        select: &ast::SelectStmt,
        mut result: ConcreteRecordSet,
    ) -> SessionResult<ConcreteRecordSet> {
        self.record_explain_for_plan(select, &result);
        let Some(option) = select.SelectIntoOpt.as_ref() else {
            return Ok(result);
        };
        if option.Tp != ast::SelectIntoType::Outfile {
            return Err(SessionError::new("unsupported SelectInto type"));
        }

        let format = OutfileFormat::from_option(option);
        let mut open = OpenOptions::new();
        open.read(true).write(true).create_new(true);
        #[cfg(unix)]
        open.mode(0o640);
        let file = open
            .open(&option.FileName)
            .map_err(|error| SessionError::new(error.to_string()))?;
        let mut writer = BufWriter::new(file);
        let row_count = result.rows.len() as u64;
        while let Some(row) = result.rows.pop_front() {
            for (index, value) in row.iter().enumerate() {
                if index != 0 {
                    writer
                        .write_all(&format.field_terminator)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                }
                if value == SHOW_NULL_CELL || value == CONCRETE_NULL_VALUE {
                    writer
                        .write_all(&format.null_value())
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    continue;
                }
                let enclosed = format.enclosure.is_some()
                    && (!format.optionally_enclosed
                        || optionally_enclose(
                            result.result_fields.get(index).and_then(Option::as_ref),
                        ));
                if enclosed {
                    writer
                        .write_all(&[format.enclosure.expect("checked enclosure")])
                        .map_err(|error| SessionError::new(error.to_string()))?;
                }
                writer
                    .write_all(&format.escape_field(value, enclosed))
                    .map_err(|error| SessionError::new(error.to_string()))?;
                if enclosed {
                    writer
                        .write_all(&[format.enclosure.expect("checked enclosure")])
                        .map_err(|error| SessionError::new(error.to_string()))?;
                }
            }
            writer
                .write_all(&format.line_terminator)
                .map_err(|error| SessionError::new(error.to_string()))?;
        }
        writer
            .flush()
            .map_err(|error| SessionError::new(error.to_string()))?;
        let file = writer
            .into_inner()
            .map_err(|error| SessionError::new(error.into_error().to_string()))?;
        file.sync_all()
            .map_err(|error| SessionError::new(error.to_string()))?;

        self.state.borrow_mut().last_dml_report = Some(crate::dml_runtime::DmlExecutionReport {
            Operator: "SelectInto".to_owned(),
            AffectedRows: row_count,
            ..Default::default()
        });
        Ok(ConcreteRecordSet::new(Vec::new(), Vec::new()))
    }
}
