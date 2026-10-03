// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Virtual system catalogs, metadata queries, and system-only SQL functions.

use super::*;

use super::query::format_unix_timestamp;

pub(crate) fn performance_schema_connection_summary_rows(
    summaries: &[astersql_session_sessmgr::PerformanceSchemaAccountSummary],
    table_name: &str,
) -> Vec<HashMap<String, Option<String>>> {
    let memory_columns = || {
        [
            (
                "max_session_controlled_memory".to_owned(),
                Some("0".to_owned()),
            ),
            ("max_session_total_memory".to_owned(), Some("0".to_owned())),
        ]
    };
    if table_name.eq_ignore_ascii_case("accounts") {
        return summaries
            .iter()
            .map(|summary| {
                HashMap::from_iter(
                    [
                        ("user".to_owned(), summary.user.clone()),
                        ("host".to_owned(), summary.host.clone()),
                        (
                            "current_connections".to_owned(),
                            Some(summary.current_connections.to_string()),
                        ),
                        (
                            "total_connections".to_owned(),
                            Some(summary.total_connections.to_string()),
                        ),
                    ]
                    .into_iter()
                    .chain(memory_columns()),
                )
            })
            .collect();
    }

    let identity_column = if table_name.eq_ignore_ascii_case("users") {
        "user"
    } else if table_name.eq_ignore_ascii_case("hosts") {
        "host"
    } else {
        return Vec::new();
    };
    let mut grouped = BTreeMap::<Option<String>, (u64, u64)>::new();
    for summary in summaries {
        let identity = if identity_column == "user" {
            summary.user.clone()
        } else {
            summary.host.clone()
        };
        let counts = grouped.entry(identity).or_default();
        counts.0 += summary.current_connections;
        counts.1 += summary.total_connections;
    }
    grouped
        .into_iter()
        .map(|(identity, (current, total))| {
            HashMap::from_iter(
                [
                    (identity_column.to_owned(), identity),
                    ("current_connections".to_owned(), Some(current.to_string())),
                    ("total_connections".to_owned(), Some(total.to_string())),
                ]
                .into_iter()
                .chain(memory_columns()),
            )
        })
        .collect()
}

fn show_result_fields(
    columns: &[&str],
    field_types: &[u8],
    flags: &[usize],
) -> Vec<Option<ConcreteResultField>> {
    columns
        .iter()
        .zip(field_types)
        .enumerate()
        .map(|(offset, (name, field_type))| {
            let mut field_type = astersql_parser_types::NewFieldType(*field_type);
            let (flen, decimal) =
                astersql_parser_mysql::util::GetDefaultFieldLengthAndDecimal(field_type.GetType());
            let (charset, collation) =
                astersql_types::field::DefaultCharsetForType(field_type.GetType());
            field_type.SetFlen(flen);
            field_type.SetDecimal(decimal);
            field_type.SetCharset(charset);
            field_type.SetCollate(collation);
            field_type.SetFlag(flags.get(offset).copied().unwrap_or_default());
            Some(ConcreteResultField {
                column: astersql_meta_model::ColumnInfo {
                    FieldType: field_type,
                    ..Default::default()
                },
                column_as_name: ast::NewCIStr(name),
                table_name: ast::CIStr::default(),
                table_as_name: ast::CIStr::default(),
                db_name: ast::CIStr::default(),
            })
        })
        .collect()
}

fn project_virtual_rows(
    statement: &ast::SelectStmt,
    available_columns: &[&str],
    mut rows: Vec<HashMap<String, Option<String>>>,
) -> SessionResult<ConcreteRecordSet> {
    enum Projection {
        Column(String),
        Literal(Option<String>),
        Derived {
            alias: String,
            expression: ast::ExprNode,
        },
    }
    enum OrderExpression {
        Column(String),
        Expression(ast::ExprNode),
    }

    let positional_order_expression = |position: usize| -> Option<OrderExpression> {
        if position == 0 {
            return None;
        }
        let mut projected_position = 0;
        for field in &statement.Fields.Fields {
            if field.WildCard.is_some() {
                for column in available_columns {
                    projected_position += 1;
                    if projected_position == position {
                        return Some(OrderExpression::Column((*column).to_ascii_lowercase()));
                    }
                }
                continue;
            }
            projected_position += 1;
            if projected_position != position {
                continue;
            }
            return field
                .Expr
                .as_ref()
                .cloned()
                .map(OrderExpression::Expression);
        }
        None
    };

    let mut sources = Vec::new();
    if let Some(from) = statement.From.as_ref() {
        if let Some(left) = from.TableRefs.Left.as_deref() {
            collect_physical_table_sources(left, &mut sources);
        }
        if let Some(right) = from.TableRefs.Right.as_deref() {
            collect_physical_table_sources(right, &mut sources);
        }
    }
    let source_identity = sources.first().and_then(|source| {
        let database = if source.Source.Schema.L.is_empty() {
            "information_schema"
        } else {
            source.Source.Schema.L.as_str()
        };
        virtual_system_catalog()
            .ok()
            .and_then(|catalog| {
                catalog.get(&(database.to_ascii_lowercase(), source.Source.Name.L.clone()))
            })
            .cloned()
            .map(|table| {
                let table_alias = if source.AsName.O.is_empty() {
                    table.Name.clone()
                } else {
                    source.AsName.clone()
                };
                (database.to_owned(), table_alias, table)
            })
    });
    let result_field = |column_name: &str, header: &str| {
        source_identity
            .as_ref()
            .and_then(|(database, table_alias, table)| {
                table
                    .Columns
                    .iter()
                    .find(|column| column.Name.L.eq_ignore_ascii_case(column_name))
                    .map(|column| ConcreteResultField {
                        column: column.clone(),
                        column_as_name: ast::NewCIStr(header),
                        table_name: table.Name.clone(),
                        table_as_name: table_alias.clone(),
                        db_name: ast::NewCIStr(database),
                    })
            })
    };

    if let Some(predicate) = statement.Where.as_ref() {
        rows.retain(|row| row_matches_simple_where(row, predicate));
    }
    if !statement.OrderBy.is_empty() {
        let keys = statement
            .OrderBy
            .iter()
            .map(|item| match &item.Expr.Kind {
                ast::ExprKind::Column(column) => {
                    let expression = statement.Fields.Fields.iter().find_map(|field| {
                        if field.AsName.L == column.Name.L {
                            field.Expr.as_ref().cloned()
                        } else {
                            None
                        }
                    });
                    Ok((
                        expression.map_or_else(
                            || OrderExpression::Column(column.Name.L.clone()),
                            OrderExpression::Expression,
                        ),
                        item.Desc,
                    ))
                }
                ast::ExprKind::Value(value)
                    if matches!(
                        &value.Datum,
                        ast::ValueDatum::Int64(_) | ast::ValueDatum::Uint64(_)
                    ) =>
                {
                    let position = value.text().parse::<usize>().map_err(|_| {
                        SessionError::new("virtual table ORDER BY position must be an integer")
                    })?;
                    positional_order_expression(position)
                        .map(|expression| (expression, item.Desc))
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "Unknown column '{position}' in 'order clause'"
                            ))
                        })
                }
                _ => Ok((OrderExpression::Expression(item.Expr.clone()), item.Desc)),
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let mut keyed_rows = rows
            .into_iter()
            .map(|row| {
                let values = keys
                    .iter()
                    .map(|(expression, _)| match expression {
                        OrderExpression::Column(column) => {
                            row.get(column).cloned().ok_or_else(|| {
                                SessionError::new(format!(
                                    "Unknown column '{column}' in 'order clause'"
                                ))
                            })
                        }
                        OrderExpression::Expression(expression) => {
                            super::relational_value::relational_expression_value(expression, &row)
                        }
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                Ok((row, values))
            })
            .collect::<SessionResult<Vec<_>>>()?;
        keyed_rows.sort_by(|(_, left), (_, right)| {
            for (offset, (_, desc)) in keys.iter().enumerate() {
                let left_value = left[offset].as_ref();
                let right_value = right[offset].as_ref();
                let ordering = match (left_value, right_value) {
                    (Some(left), Some(right)) => {
                        match (left.parse::<i128>(), right.parse::<i128>()) {
                            (Ok(left), Ok(right)) => left.cmp(&right),
                            _ => left.cmp(right),
                        }
                    }
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if !ordering.is_eq() {
                    return if *desc { ordering.reverse() } else { ordering };
                }
            }
            std::cmp::Ordering::Equal
        });
        rows = keyed_rows.into_iter().map(|(row, _)| row).collect();
    }
    if statement.Fields.Fields.len() == 1
        && statement.Fields.Fields[0]
            .Expr
            .as_ref()
            .is_some_and(|expression| {
                matches!(
                    &expression.Kind,
                    ast::ExprKind::AggregateFunction { Name, .. }
                        if Name.eq_ignore_ascii_case("count")
                )
            })
    {
        return Ok(ConcreteRecordSet::new(
            vec!["count(*)".to_owned()],
            vec![vec![rows.len().to_string()]],
        ));
    }
    let mut headers = Vec::new();
    let mut projections = Vec::new();
    let mut result_fields = Vec::new();
    for field in &statement.Fields.Fields {
        if field.WildCard.is_some() {
            for column in available_columns {
                headers.push((*column).to_owned());
                projections.push(Projection::Column((*column).to_ascii_lowercase()));
                result_fields.push(result_field(column, column));
            }
            continue;
        }
        let expression = field
            .Expr
            .as_ref()
            .ok_or_else(|| SessionError::new("virtual table projection requires expression"))?;
        match &expression.Kind {
            ast::ExprKind::Column(column) => {
                let header = if field.AsName.O.is_empty() {
                    column.Name.O.clone()
                } else {
                    field.AsName.O.clone()
                };
                headers.push(header.clone());
                projections.push(Projection::Column(column.Name.L.clone()));
                result_fields.push(result_field(&column.Name.L, &header));
            }
            ast::ExprKind::Value(value) => {
                headers.push(if field.AsName.O.is_empty() {
                    value.text()
                } else {
                    field.AsName.O.clone()
                });
                projections.push(Projection::Literal(match &value.Datum {
                    ast::ValueDatum::Null => None,
                    ast::ValueDatum::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
                    _ => Some(value.text()),
                }));
                result_fields.push(None);
            }
            _ => {
                let alias = if field.AsName.O.is_empty() {
                    format!("expression_{}", projections.len() + 1)
                } else {
                    field.AsName.O.clone()
                };
                headers.push(alias.clone());
                projections.push(Projection::Derived {
                    alias: alias.to_ascii_lowercase(),
                    expression: expression.clone(),
                });
                result_fields.push(None);
            }
        }
    }
    let projected = rows
        .into_iter()
        .map(|row| -> SessionResult<Vec<String>> {
            projections
                .iter()
                .map(|projection| -> SessionResult<String> {
                    Ok(match projection {
                        Projection::Column(column) => row
                            .get(column)
                            .and_then(Option::as_ref)
                            .cloned()
                            .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned()),
                        Projection::Literal(value) => value
                            .clone()
                            .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned()),
                        Projection::Derived { alias, expression } => {
                            if let Some(value) =
                                row.get(&format!("__{alias}")).and_then(Option::as_ref)
                            {
                                value.clone()
                            } else {
                                super::relational_value::relational_expression_value(
                                    expression, &row,
                                )?
                                .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned())
                            }
                        }
                    })
                })
                .collect()
        })
        .collect::<SessionResult<Vec<_>>>()?;
    Ok(ConcreteRecordSet::new_with_fields(
        headers,
        projected,
        result_fields,
    ))
}

fn injected_cluster_info_rows(payload: &str) -> Vec<HashMap<String, Option<String>>> {
    payload
        .trim_matches(['\'', '"'])
        .split(';')
        .filter(|server| !server.is_empty())
        .map(|server| {
            let parts = server.split(',').collect::<Vec<_>>();
            assert_eq!(
                parts.len(),
                6,
                "mockClusterInfo entry must contain six comma-separated fields"
            );
            let server_id = parts[5]
                .parse::<u64>()
                .expect("mockClusterInfo server_id must be an unsigned integer");
            HashMap::from([
                ("type".to_owned(), Some(parts[0].to_owned())),
                ("instance".to_owned(), Some(parts[1].to_owned())),
                ("status_address".to_owned(), Some(parts[2].to_owned())),
                ("version".to_owned(), Some(parts[3].to_owned())),
                ("git_hash".to_owned(), Some(parts[4].to_owned())),
                ("server_id".to_owned(), Some(server_id.to_string())),
            ])
        })
        .collect()
}

/// JDBC type metadata derived from the canonical parser field type.
pub(super) fn information_schema_column_type(
    column: &astersql_meta_model::ColumnInfo,
) -> (&'static str, i32) {
    use astersql_parser_mysql::r#type as mysql;

    match column.GetType() {
        mysql::TypeTiny => ("TINYINT", -6),
        mysql::TypeShort => ("SMALLINT", 5),
        mysql::TypeLong | mysql::TypeInt24 => ("INTEGER", 4),
        mysql::TypeFloat => ("FLOAT", 7),
        mysql::TypeDouble => ("DOUBLE", 8),
        mysql::TypeTimestamp | mysql::TypeDatetime => ("DATETIME", 93),
        mysql::TypeDate | mysql::TypeNewDate => ("DATE", 91),
        mysql::TypeDuration => ("TIME", 92),
        mysql::TypeLonglong => ("BIGINT", -5),
        mysql::TypeYear => ("YEAR", 5),
        mysql::TypeNewDecimal => ("DECIMAL", 3),
        mysql::TypeBit => ("BIT", -7),
        mysql::TypeString => ("CHAR", 1),
        mysql::TypeVarchar | mysql::TypeVarString | mysql::TypeEnum | mysql::TypeSet => {
            ("VARCHAR", 12)
        }
        mysql::TypeTinyBlob | mysql::TypeMediumBlob | mysql::TypeLongBlob | mysql::TypeBlob => {
            ("BLOB", -4)
        }
        mysql::TypeJSON => ("JSON", -1),
        mysql::TypeGeometry => ("GEOMETRY", -2),
        mysql::TypeTiDBVectorFloat32 => ("VECTOR", 1111),
        _ => ("OTHER", 1111),
    }
}

/// JDBC column size, using MySQL's conventional width when DDL omitted `flen`.
pub(super) fn information_schema_column_size(column: &astersql_meta_model::ColumnInfo) -> isize {
    use astersql_parser_mysql::r#type as mysql;

    if column.GetFlen() >= 0 {
        return column.GetFlen();
    }
    match column.GetType() {
        mysql::TypeTiny => 3,
        mysql::TypeShort => 5,
        mysql::TypeLong | mysql::TypeInt24 => 10,
        mysql::TypeLonglong => 19,
        mysql::TypeFloat => 12,
        mysql::TypeDouble => 22,
        mysql::TypeNewDecimal => 10,
        mysql::TypeDate | mysql::TypeNewDate => 10,
        mysql::TypeTimestamp | mysql::TypeDatetime => 26,
        mysql::TypeDuration => 16,
        mysql::TypeYear => 4,
        mysql::TypeBit => 1,
        mysql::TypeTinyBlob => 255,
        mysql::TypeBlob => 65_535,
        mysql::TypeMediumBlob => 16_777_215,
        mysql::TypeLongBlob => i32::MAX as isize,
        _ => 0,
    }
}

pub(super) fn metadata_default_value(column: &astersql_meta_model::ColumnInfo) -> Option<String> {
    use astersql_meta_model::DefaultValue;

    column.GetDefaultValue().map(|value| match value {
        DefaultValue::Bool(value) => {
            if value {
                "1".to_owned()
            } else {
                "0".to_owned()
            }
        }
        DefaultValue::Int(value) => value.to_string(),
        DefaultValue::Uint(value) => value.to_string(),
        DefaultValue::Float(value) => value.to_string(),
        DefaultValue::String(value) => String::from_utf8_lossy(&value).into_owned(),
    })
}

pub(super) fn metadata_column_key(
    table: &astersql_meta_model::TableInfo,
    column: &astersql_meta_model::ColumnInfo,
) -> &'static str {
    if astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()) {
        return "PRI";
    }
    if table.Indices.iter().any(|index| {
        index.Unique
            && index
                .Columns
                .first()
                .is_some_and(|indexed| indexed.Name.L == column.Name.L)
    }) {
        return "UNI";
    }
    if table.Indices.iter().any(|index| {
        index
            .Columns
            .first()
            .is_some_and(|indexed| indexed.Name.L == column.Name.L)
    }) {
        return "MUL";
    }
    ""
}

pub(super) fn virtual_system_column(
    id: i64,
    offset: usize,
    name: &str,
) -> astersql_meta_model::ColumnInfo {
    let mut field_type =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarchar);
    field_type.SetFlen(1024);
    field_type.SetCharset("utf8mb4".to_owned());
    field_type.SetCollate("utf8mb4_bin".to_owned());
    astersql_meta_model::ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        Offset: offset as isize,
        State: astersql_meta_model::StatePublic,
        FieldType: field_type,
        ..Default::default()
    }
}

fn virtual_information_schema_column(
    id: i64,
    offset: usize,
    column: &astersql_infoschema::tables::columnInfo,
) -> astersql_meta_model::ColumnInfo {
    use astersql_infoschema::tables::ColumnType;
    use astersql_parser_mysql::r#type as mysql;

    let field_type_code = match column.column_type {
        ColumnType::Varchar => mysql::TypeVarchar,
        ColumnType::Tiny => mysql::TypeTiny,
        ColumnType::Long => mysql::TypeLong,
        ColumnType::Longlong => mysql::TypeLonglong,
        ColumnType::Double => mysql::TypeDouble,
        ColumnType::Blob => mysql::TypeBlob,
        ColumnType::MediumBlob => mysql::TypeMediumBlob,
        ColumnType::LongBlob => mysql::TypeLongBlob,
        ColumnType::Timestamp => mysql::TypeTimestamp,
        ColumnType::Datetime => mysql::TypeDatetime,
        ColumnType::Decimal => mysql::TypeNewDecimal,
        ColumnType::Json => mysql::TypeJSON,
    };
    let mut field_type = astersql_parser_types::NewFieldType(field_type_code);
    let textual = matches!(
        column.column_type,
        ColumnType::Varchar | ColumnType::Blob | ColumnType::MediumBlob | ColumnType::LongBlob
    );
    if textual {
        field_type.SetCharset("utf8mb4".to_owned());
        field_type.SetCollate("utf8mb4_bin".to_owned());
    } else {
        field_type.SetCharset("binary".to_owned());
        field_type.SetCollate("binary".to_owned());
    }
    field_type.SetFlen(match column.column_type {
        ColumnType::Blob => 1_isize << 16,
        ColumnType::MediumBlob => 1_isize << 24,
        ColumnType::LongBlob => 1_isize << 32,
        _ => column.size as isize,
    });
    field_type.SetDecimal(column.decimal.unwrap_or(0) as isize);
    let mut flags = 0;
    if column.unsigned {
        flags |= mysql::UnsignedFlag;
    }
    if column.not_null {
        flags |= mysql::NotNullFlag;
    }
    field_type.SetFlag(flags);
    let default_value = column
        .default_value
        .map(|value| astersql_meta_model::DefaultValue::String(value.as_bytes().to_vec()));
    astersql_meta_model::ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(column.name),
        Offset: offset as isize,
        DefaultValue: default_value.clone(),
        OriginDefaultValue: default_value,
        State: astersql_meta_model::StatePublic,
        FieldType: field_type,
        Comment: column.comment.to_owned(),
        ..Default::default()
    }
}

fn virtual_system_table<I, S>(id: i64, name: &str, columns: I) -> astersql_meta_model::TableInfo
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    astersql_meta_model::TableInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Columns: columns
            .into_iter()
            .enumerate()
            .map(|(offset, name)| virtual_system_column(offset as i64 + 1, offset, name.as_ref()))
            .collect(),
        State: astersql_meta_model::StatePublic,
        ..Default::default()
    }
}

pub(super) type SessionMetadataCatalog = BTreeMap<(String, String), astersql_meta_model::TableInfo>;

fn build_virtual_system_catalog() -> Result<SessionMetadataCatalog, String> {
    let mut catalog = BTreeMap::new();

    for table in astersql_infoschema::tables::table_registry().values() {
        let name = table.name.to_ascii_lowercase();
        catalog.insert(
            ("information_schema".to_owned(), name),
            astersql_meta_model::TableInfo {
                ID: table.id,
                Name: ast::NewCIStr(table.name),
                Charset: "utf8mb4".to_owned(),
                Collate: "utf8mb4_bin".to_owned(),
                Columns: table
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(offset, column)| {
                        virtual_information_schema_column(offset as i64 + 1, offset, column)
                    })
                    .collect(),
                State: astersql_meta_model::StatePublic,
                ..Default::default()
            },
        );
    }

    // The translated registry still gives less commonly queried tables its
    // generic INSTANCE/NAME/VALUE placeholder.  These two lock views take the
    // full relational path for joins, so column binding must use their real Go
    // schemas before rows are materialized.
    for (table_name, columns) in [
        (
            "data_lock_waits",
            &[
                "KEY",
                "KEY_INFO",
                "TRX_ID",
                "CURRENT_HOLDING_TRX_ID",
                "SQL_DIGEST",
                "SQL_DIGEST_TEXT",
            ][..],
        ),
        (
            "tidb_trx",
            &[
                "ID",
                "START_TIME",
                "CURRENT_SQL_DIGEST",
                "CURRENT_SQL_DIGEST_TEXT",
                "STATE",
                "WAITING_START_TIME",
                "MEM_BUFFER_KEYS",
                "MEM_BUFFER_BYTES",
                "SESSION_ID",
                "USER",
                "DB",
                "ALL_SQL_DIGESTS",
                "RELATED_TABLE_IDS",
                "WAITING_TIME",
            ][..],
        ),
    ] {
        if let Some(table) =
            catalog.get_mut(&("information_schema".to_owned(), table_name.to_owned()))
        {
            table.Columns = columns
                .iter()
                .enumerate()
                .map(|(offset, name)| virtual_system_column(offset as i64 + 1, offset, name))
                .collect();
        }
    }

    let performance_schema = astersql_infoschema_perfschema::build_performance_schema()
        .map_err(|error| format!("build PERFORMANCE_SCHEMA authoritative registry: {error}"))?;
    for table in performance_schema.tables {
        let name = table.name.to_ascii_lowercase();
        catalog.insert(
            ("performance_schema".to_owned(), name),
            virtual_system_table(
                table.id,
                &table.name,
                table.columns.iter().map(|column| column.name.as_str()),
            ),
        );
    }

    let metrics_schema = astersql_infoschema::metrics_schema::metric_schema_db();
    for table in metrics_schema.tables {
        let name = table.name.lower.clone();
        catalog.insert(
            ("metrics_schema".to_owned(), name),
            virtual_system_table(
                table.id,
                &table.name.original,
                table
                    .columns
                    .iter()
                    .map(|column| column.name.original.as_str()),
            ),
        );
    }

    Ok(catalog)
}

static VIRTUAL_SYSTEM_CATALOG: OnceLock<Result<SessionMetadataCatalog, String>> = OnceLock::new();

pub(super) fn virtual_system_catalog() -> SessionResult<&'static SessionMetadataCatalog> {
    VIRTUAL_SYSTEM_CATALOG
        .get_or_init(build_virtual_system_catalog)
        .as_ref()
        .map_err(|error| SessionError::new(error.clone()))
}

pub(super) fn referential_action_name(action: i32) -> &'static str {
    match action {
        1 => "RESTRICT",
        2 => "CASCADE",
        3 => "SET NULL",
        4 => "NO ACTION",
        5 => "SET DEFAULT",
        _ => "NO ACTION",
    }
}

pub(super) fn referenced_constraint_name(
    table: &astersql_meta_model::TableInfo,
    columns: &[ast::CIStr],
) -> String {
    let primary = table
        .Columns
        .iter()
        .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
        .map(|column| column.Name.L.as_str())
        .collect::<Vec<_>>();
    if primary.len() == columns.len()
        && primary
            .iter()
            .zip(columns)
            .all(|(left, right)| *left == right.L)
    {
        return "PRIMARY".to_owned();
    }
    table
        .Indices
        .iter()
        .find(|index| {
            index.Unique
                && index.Columns.len() == columns.len()
                && index
                    .Columns
                    .iter()
                    .zip(columns)
                    .all(|(left, right)| left.Name.L == right.L)
        })
        .map(|index| index.Name.O.clone())
        .unwrap_or_else(|| "PRIMARY".to_owned())
}

impl ConcreteSession {
    pub(super) fn execute_show_collation(
        &self,
        show: &ast::ShowStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let default_utf8mb4 =
            self.select_variable(astersql_sessionctx_vardef::DefaultCollationForUTF8MB4, true)?;
        let like_pattern = show.Pattern.as_ref().and_then(|expression| {
            if let ast::ExprKind::Like { Pattern, .. } = &expression.Kind {
                literal(Pattern).ok()
            } else {
                literal(expression).ok()
            }
        });
        let mut rows = Vec::new();
        for collation in astersql_util_collate::GetSupportedCollations() {
            if like_pattern
                .as_ref()
                .is_some_and(|pattern| !relational_like(pattern, &collation.Name))
            {
                continue;
            }
            let is_default = if collation.CharsetName.eq_ignore_ascii_case("utf8mb4") {
                collation.Name.eq_ignore_ascii_case(&default_utf8mb4)
            } else {
                collation.IsDefault
            };
            let values = HashMap::from([
                ("collation".to_owned(), Some(collation.Name.clone())),
                ("charset".to_owned(), Some(collation.CharsetName.clone())),
                ("id".to_owned(), Some(collation.ID.to_string())),
                (
                    "default".to_owned(),
                    Some(if is_default { "Yes" } else { "" }.to_owned()),
                ),
                ("compiled".to_owned(), Some("Yes".to_owned())),
                ("sortlen".to_owned(), Some(collation.Sortlen.to_string())),
                (
                    "pad_attribute".to_owned(),
                    Some(collation.PadAttribute.clone()),
                ),
            ]);
            if show
                .Where
                .as_ref()
                .is_some_and(|predicate| !row_matches_simple_where(&values, predicate))
            {
                continue;
            }
            rows.push(vec![
                collation.Name,
                collation.CharsetName,
                collation.ID.to_string(),
                if is_default { "Yes" } else { "" }.to_owned(),
                "Yes".to_owned(),
                collation.Sortlen.to_string(),
                collation.PadAttribute,
            ]);
        }
        let columns = [
            "Collation",
            "Charset",
            "Id",
            "Default",
            "Compiled",
            "Sortlen",
            "Pad_attribute",
        ];
        let field_types = [
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeLonglong,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeLonglong,
            astersql_parser_mysql::r#type::TypeVarchar,
        ];
        let flags = [
            0,
            0,
            astersql_parser_mysql::r#type::UnsignedFlag
                | astersql_parser_mysql::r#type::NotNullFlag,
            0,
            0,
            0,
            0,
        ];
        let result_fields = show_result_fields(&columns, &field_types, &flags);
        Ok(ConcreteRecordSet::new_with_fields(
            columns.into_iter().map(str::to_owned).collect(),
            rows,
            result_fields,
        ))
    }

    pub(super) fn execute_show_process_list(&self, full: bool) -> ConcreteRecordSet {
        use astersql_executor::show::{FetchShowProcessListRows, ShowValue};

        let manager = self.session_manager.as_ref().and_then(Weak::upgrade);
        let rows = FetchShowProcessListRows(
            manager.as_deref(),
            self.login_user.as_deref(),
            self.has_process_privilege,
            full,
        )
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| match value {
                    ShowValue::Null => CONCRETE_NULL_VALUE.to_owned(),
                    ShowValue::Int64(value) => value.to_string(),
                    ShowValue::Uint64(value) => value.to_string(),
                    ShowValue::Float64(value) => value.to_string(),
                    ShowValue::String(value) => value,
                    ShowValue::Bytes(value) => String::from_utf8_lossy(&value).into_owned(),
                })
                .collect()
        })
        .collect();
        let columns = [
            "Id", "User", "Host", "db", "Command", "Time", "State", "Info",
        ];
        let field_types = [
            astersql_parser_mysql::r#type::TypeLonglong,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeLong,
            astersql_parser_mysql::r#type::TypeVarchar,
            astersql_parser_mysql::r#type::TypeString,
        ];
        let result_fields = show_result_fields(&columns, &field_types, &[]);
        ConcreteRecordSet::new_with_fields(
            columns.into_iter().map(str::to_owned).collect(),
            rows,
            result_fields,
        )
    }

    /// Merge persistent Domain metadata with the production virtual registries.
    ///
    /// Persistent mysql/sys objects win on duplicate names; virtual schemas are
    /// process-local overlays and are never written to TiKV.
    pub(super) fn metadata_catalog(&self) -> SessionResult<SessionMetadataCatalog> {
        let context = self.domain.stats_context();
        let (snapshot_read_ts, snapshot_catalog_version) = {
            let state = self.state.borrow();
            (state.snapshot_read_ts, state.snapshot_catalog_version)
        };
        let mut catalog = if let Some(snapshot_read_ts) = snapshot_read_ts.filter(|ts| *ts != 0) {
            self.domain
                .snapshot_info_schema(snapshot_read_ts)
                .map_err(|error| session_error("load snapshot metadata catalog", error))?
                .AllSchemas()
                .into_iter()
                .flat_map(|database| {
                    let database_name = database.name.original.to_ascii_lowercase();
                    database
                        .tables
                        .iter()
                        .filter_map(move |table| {
                            table.model_meta.as_ref().map(|table| {
                                (
                                    (database_name.clone(), table.Name.L.clone()),
                                    table.as_ref().clone(),
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<SessionMetadataCatalog>()
        } else {
            snapshot_catalog_version
                .map(|version| context.catalog_at(version))
                .unwrap_or_else(|| context.catalog())
                .into_iter()
                .map(|(key, (_, table))| (key, table))
                .collect::<SessionMetadataCatalog>()
        };
        for (key, table) in virtual_system_catalog()? {
            catalog.entry(key.clone()).or_insert_with(|| table.clone());
        }
        Ok(catalog)
    }

    fn information_schema_table_visible(&self, database: &str, table: &str) -> bool {
        let (Some(user), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) else {
            return true;
        };
        runtime_privilege_handle(&self.domain)
            .Get()
            .RequestVerification(
                &self.active_roles.borrow(),
                user,
                host,
                database,
                table,
                "",
                astersql_privilege_privileges::SelectPriv,
            )
    }

    /// Return the public columns used by MySQL COM_FIELD_LIST.
    ///
    /// This follows Go `session.FieldList`: resolve through the current
    /// InfoSchema and preserve the canonical column/table/schema identities for
    /// the server's shared protocol metadata converter.
    pub fn field_list(&self, table_name: &str) -> SessionResult<Vec<ResultField>> {
        let database = self.current_database();
        if let (Some(user), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) && !runtime_privilege_handle(&self.domain)
            .Get()
            .RequestVerification(
                &self.active_roles.borrow(),
                user,
                host,
                &database,
                table_name,
                "",
                astersql_parser_mysql::privs::AllPrivMask.0,
            )
        {
            return Err(SessionError::new(format!(
                "SELECT command denied to user '{user}'@'{host}' for table '{table_name}'"
            )));
        }
        let table = self
            .domain
            .table_by_name(&database, table_name)
            .map_err(|error| session_error("resolve COM_FIELD_LIST table", error))?;
        let table = Rc::new((*table).clone());
        Ok(table
            .Cols()
            .into_iter()
            .flatten()
            .map(|column| ResultField {
                column: Some(Rc::new(column.clone())),
                column_as_name: column.Name.clone(),
                empty_org_name: false,
                table: Some(Rc::clone(&table)),
                table_as_name: table.Name.clone(),
                db_name: ast::NewCIStr(&database),
            })
            .collect())
    }

    pub(super) fn execute_information_schema_select(
        &self,
        statement: &ast::SelectStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        let Some(from) = statement.From.as_ref() else {
            return Ok(None);
        };
        let mut sources = Vec::new();
        if let Some(left) = from.TableRefs.Left.as_deref() {
            collect_physical_table_sources(left, &mut sources);
        }
        if let Some(right) = from.TableRefs.Right.as_deref() {
            collect_physical_table_sources(right, &mut sources);
        }
        // Go materializes each INFORMATION_SCHEMA table independently and lets
        // the normal Join/Selection/Projection/Sort executors process queries
        // with multiple sources.  This direct path can only preserve those
        // semantics for a single source; taking the first table of a DataGrip
        // metadata join drops columns such as C.ordinal_position.
        if sources.len() != 1 {
            return Ok(None);
        }
        let Some(source) = sources.first().copied() else {
            return Ok(None);
        };
        let current_database = self.current_database();
        let schema = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        if !schema.eq_ignore_ascii_case("information_schema") {
            if !matches!(
                schema.to_ascii_lowercase().as_str(),
                "metrics_schema" | "performance_schema" | "sys"
            ) {
                return Ok(None);
            }
            let Some(table) = virtual_system_catalog()?.get(&(
                schema.to_ascii_lowercase(),
                source.Source.Name.L.to_ascii_lowercase(),
            )) else {
                return Ok(None);
            };
            let columns = table
                .Columns
                .iter()
                .map(|column| column.Name.O.as_str())
                .collect::<Vec<_>>();
            let rows = if schema.eq_ignore_ascii_case("performance_schema")
                && matches!(
                    source.Source.Name.L.as_str(),
                    "accounts" | "users" | "hosts"
                ) {
                self.session_manager
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .map(|manager| {
                        performance_schema_connection_summary_rows(
                            &manager.GetPerformanceSchemaAccountSummaries(),
                            source.Source.Name.L.as_str(),
                        )
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            return Ok(Some(project_virtual_rows(statement, &columns, rows)?));
        }
        let table_name = source.Source.Name.L.as_str();
        if table_name.eq_ignore_ascii_case("slow_query") {
            let rows = self
                .state
                .borrow()
                .slow_query_plans
                .iter()
                .map(|(query, plan, read_pool)| {
                    HashMap::from([
                        ("time".to_owned(), Some(String::new())),
                        ("query".to_owned(), Some(query.clone())),
                        ("plan".to_owned(), Some(plan.clone())),
                        ("read_pool_task_details".to_owned(), Some(read_pool.clone())),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &["TIME", "QUERY", "PLAN", "READ_POOL_TASK_DETAILS"],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("statements_summary") {
            let rows = self
                .state
                .borrow()
                .statement_summary_plans
                .iter()
                .map(|(query, plan)| {
                    HashMap::from([
                        ("query_sample_text".to_owned(), Some(query.clone())),
                        ("plan".to_owned(), Some(plan.clone())),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &["QUERY_SAMPLE_TEXT", "PLAN"],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("cluster_slow_query") {
            let send_result =
                astersql_testkit_testfailpoint::eval_string("tikvclient/tikvStoreSendReqResult");
            if send_result
                .as_deref()
                .map(|result| result.trim_matches(['\'', '"']))
                .is_some_and(|result| !result.is_empty())
            {
                let address = self
                    .runtime_topology()
                    .first()
                    .map(|node| node.address.clone())
                    .unwrap_or_else(|| "unknown".to_owned());
                self.set_warning(format!("TiDB server timeout, address is {address}"));
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &["INSTANCE", "TIME", "QUERY"],
                Vec::new(),
            )?));
        }
        if table_name.eq_ignore_ascii_case("cluster_info") {
            let injected = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/infoschema/mockClusterInfo",
            )
            .or_else(|| astersql_testkit_testfailpoint::eval_string("mockClusterInfo"));
            let generated_rows = injected.map_or_else(
                || {
                    self.runtime_topology()
                        .into_iter()
                        .map(|node| {
                            HashMap::from([
                                ("type".to_owned(), Some("tikv".to_owned())),
                                ("instance".to_owned(), Some(node.address)),
                                ("status_address".to_owned(), Some(String::new())),
                                ("version".to_owned(), Some("AsterSQL".to_owned())),
                                ("git_hash".to_owned(), Some(node.store_id.to_string())),
                                ("server_id".to_owned(), Some(node.store_id.to_string())),
                            ])
                        })
                        .collect()
                },
                |payload| injected_cluster_info_rows(&payload),
            );
            let rows = {
                let mut state = self.state.borrow_mut();
                if let Some(cache) = state.inspection_table_cache.as_mut() {
                    cache
                        .entry("cluster_info".to_owned())
                        .or_insert(generated_rows)
                        .clone()
                } else {
                    generated_rows
                }
            };
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TYPE",
                    "INSTANCE",
                    "STATUS_ADDRESS",
                    "VERSION",
                    "GIT_HASH",
                    "SERVER_ID",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("tidb_servers_info") {
            let info = astersql_domain_infosync::GetServerInfo()
                .map_err(|error| SessionError::new(error.to_string()))?;
            let mut labels = info
                .Labels
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>();
            labels.sort();
            let rows = vec![HashMap::from([
                ("ddl_id".to_owned(), Some(info.ID)),
                ("ip".to_owned(), Some(info.IP)),
                ("port".to_owned(), Some(info.Port.to_string())),
                ("status_port".to_owned(), Some(info.StatusPort.to_string())),
                ("lease".to_owned(), Some(info.Lease)),
                ("version".to_owned(), Some(info.Version)),
                ("git_hash".to_owned(), Some(info.GitHash)),
                ("labels".to_owned(), Some(labels.join(","))),
            ])];
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "DDL_ID",
                    "IP",
                    "PORT",
                    "STATUS_PORT",
                    "LEASE",
                    "VERSION",
                    "GIT_HASH",
                    "LABELS",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("keyspace_meta") {
            let injected = astersql_testkit_testfailpoint::eval_string(
                "github.com/pingcap/tidb/pkg/infoschema/mockKeyspaceMeta",
            );
            let rows = injected
                .as_deref()
                .and_then(|payload| {
                    let mut parts = payload.trim_matches(['\'', '"']).splitn(3, '|');
                    let name = parts.next()?.to_owned();
                    let id = parts.next()?.parse::<u32>().ok()?;
                    let config = parts
                        .next()
                        .unwrap_or_default()
                        .split(';')
                        .filter_map(|entry| entry.split_once('='))
                        .map(|(key, value)| (key.to_owned(), value.to_owned()))
                        .collect::<BTreeMap<_, _>>();
                    Some(vec![HashMap::from([
                        ("keyspace_name".to_owned(), Some(name)),
                        ("keyspace_id".to_owned(), Some(id.to_string())),
                        (
                            "keyspace_config".to_owned(),
                            Some(
                                serde_json::to_string(&config).expect("serialize keyspace config"),
                            ),
                        ),
                    ])])
                })
                .unwrap_or_default();
            return Ok(Some(project_virtual_rows(
                statement,
                &["KEYSPACE_NAME", "KEYSPACE_ID", "KEYSPACE_CONFIG"],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("data_lock_waits") {
            if self.domain.storage().with_storage(|store| store.Name()) == "TiKV" {
                let entries = self
                    .domain
                    .storage()
                    .with_storage(|store| store.GetLockWaits())
                    .map_err(|error| session_error("read TiKV lock wait table", error))?;
                let rows = entries
                    .into_iter()
                    .map(|entry| {
                        HashMap::from([
                            (
                                "key".to_owned(),
                                Some(
                                    entry
                                        .key
                                        .iter()
                                        .map(|byte| format!("{byte:02x}"))
                                        .collect::<String>(),
                                ),
                            ),
                            ("trx_id".to_owned(), Some(entry.txn.to_string())),
                            (
                                "current_holding_trx_id".to_owned(),
                                Some(entry.wait_for_txn.to_string()),
                            ),
                        ])
                    })
                    .collect();
                return Ok(Some(project_virtual_rows(
                    statement,
                    &[
                        "KEY",
                        "KEY_INFO",
                        "TRX_ID",
                        "CURRENT_HOLDING_TRX_ID",
                        "SQL_DIGEST",
                        "SQL_DIGEST_TEXT",
                    ],
                    rows,
                )?));
            }
            let txn_infos = RUNTIME_TXN_INFOS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let state = RUNTIME_ROW_LOCKS
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut rows = Vec::new();
            for (key, waiters) in &state.waiters {
                let encoded_key = key
                    .key
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                for waiter in waiters {
                    let txn_info = txn_infos.get(&waiter.owner);
                    rows.push(HashMap::from([
                        ("key".to_owned(), Some(encoded_key.clone())),
                        (
                            "trx_id".to_owned(),
                            Some(
                                txn_info
                                    .map_or(waiter.owner, |info| info.start_ts)
                                    .to_string(),
                            ),
                        ),
                    ]));
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "KEY",
                    "KEY_INFO",
                    "TRX_ID",
                    "CURRENT_HOLDING_TRX_ID",
                    "SQL_DIGEST",
                    "SQL_DIGEST_TEXT",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("tidb_trx") {
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            let infos = RUNTIME_TXN_INFOS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let rows = infos
                .values()
                .filter(|info| info.domain_id == domain_id)
                .map(|info| {
                    HashMap::from([
                        ("id".to_owned(), Some(info.start_ts.to_string())),
                        (
                            "current_sql_digest".to_owned(),
                            Some(info.current_sql_digest.clone()),
                        ),
                        ("state".to_owned(), Some(info.state.clone())),
                        (
                            "waiting_start_time".to_owned(),
                            info.waiting_start_time
                                .map(|_| format_system_time(SystemTime::now())),
                        ),
                        (
                            "mem_buffer_keys".to_owned(),
                            Some(info.mem_buffer_keys.to_string()),
                        ),
                        (
                            "mem_buffer_bytes".to_owned(),
                            Some(info.mem_buffer_bytes.to_string()),
                        ),
                        ("session_id".to_owned(), Some(info.session_id.to_string())),
                        ("user".to_owned(), Some(String::new())),
                        ("db".to_owned(), Some(info.database.clone())),
                        (
                            "all_sql_digests".to_owned(),
                            Some(format!(
                                "[{}]",
                                info.all_sql_digests
                                    .iter()
                                    .map(|digest| format!("\"{digest}\""))
                                    .collect::<Vec<_>>()
                                    .join(",")
                            )),
                        ),
                    ])
                })
                .collect();
            let wildcard = statement
                .Fields
                .Fields
                .iter()
                .any(|field| field.WildCard.is_some());
            let columns: &[&str] = if wildcard {
                &["ID", "SESSION_ID"]
            } else {
                &[
                    "ID",
                    "CURRENT_SQL_DIGEST",
                    "STATE",
                    "WAITING_START_TIME",
                    "MEM_BUFFER_KEYS",
                    "MEM_BUFFER_BYTES",
                    "SESSION_ID",
                    "USER",
                    "DB",
                    "ALL_SQL_DIGESTS",
                ]
            };
            return Ok(Some(project_virtual_rows(statement, columns, rows)?));
        }
        if table_name.eq_ignore_ascii_case("resource_groups") {
            let groups = RUNTIME_RESOURCE_GROUPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&runtime_domain_id(&self.domain))
                .cloned()
                .unwrap_or_default();
            let rows = groups
                .into_iter()
                .map(|(name, group)| {
                    HashMap::from([
                        ("name".to_owned(), Some(name)),
                        ("ru_per_sec".to_owned(), Some(group.ru_per_sec.to_string())),
                        ("priority".to_owned(), Some(group.priority.String())),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &["NAME", "RU_PER_SEC", "PRIORITY"],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("tidb_plan_cache")
            || table_name.eq_ignore_ascii_case("cluster_tidb_plan_cache")
        {
            // The narrow runtime does not expose the Go memtable retriever, but
            // the virtual table must still be served through the same metadata
            // path.  Use the Domain cache snapshot when one is available; an
            // empty cache is a valid result and must not fall through to the
            // physical-table lookup.
            let rows = self
                .domain
                .instance_plan_cache()
                .and_then(|cache| cache.lock().ok().map(|cache| cache.snapshot()))
                .unwrap_or_default()
                .into_iter()
                .map(|(sql_digest, sql_text)| {
                    HashMap::from([
                        ("sql_digest".to_owned(), Some(sql_digest)),
                        ("sql_text".to_owned(), Some(sql_text)),
                        ("stmt_type".to_owned(), Some("Select".to_owned())),
                        ("parse_user".to_owned(), None),
                        ("plan_digest".to_owned(), None),
                        ("binary_plan".to_owned(), None),
                        ("binding".to_owned(), None),
                        ("opt_env".to_owned(), None),
                        ("parse_values".to_owned(), None),
                        ("mem_size".to_owned(), Some("0".to_owned())),
                        ("executions".to_owned(), Some("0".to_owned())),
                        ("processed_keys".to_owned(), Some("0".to_owned())),
                        ("total_keys".to_owned(), Some("0".to_owned())),
                        ("sum_latency".to_owned(), Some("0".to_owned())),
                        ("load_time".to_owned(), None),
                        ("last_active_time".to_owned(), None),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "SQL_DIGEST",
                    "SQL_TEXT",
                    "STMT_TYPE",
                    "PARSE_USER",
                    "PLAN_DIGEST",
                    "BINARY_PLAN",
                    "BINDING",
                    "OPT_ENV",
                    "PARSE_VALUES",
                    "MEM_SIZE",
                    "EXECUTIONS",
                    "PROCESSED_KEYS",
                    "TOTAL_KEYS",
                    "SUM_LATENCY",
                    "LOAD_TIME",
                    "LAST_ACTIVE_TIME",
                ],
                rows,
            )?));
        }
        let metadata_catalog = self
            .metadata_catalog()?
            .into_iter()
            .filter(|((database, table), _)| self.information_schema_table_visible(database, table))
            .collect::<SessionMetadataCatalog>();
        if table_name.eq_ignore_ascii_case("tiflash_replica") {
            let rows = metadata_catalog
                .into_iter()
                .filter_map(|((database, _), table)| {
                    let replica = table.TiFlashReplica.as_ref()?;
                    let physical_ids = table
                        .Partition
                        .as_ref()
                        .filter(|partition| !partition.Definitions.is_empty())
                        .map(|partition| {
                            partition
                                .Definitions
                                .iter()
                                .map(|part| part.ID)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|| vec![table.ID]);
                    let progress = physical_ids
                        .iter()
                        .map(|physical_id| {
                            self.domain
                                .storage_handle()
                                .with_storage(|store| {
                                    store.ObserveTiFlashReplicaProgress(*physical_id, replica.Count)
                                })
                                .ok()
                                .flatten()
                                .unwrap_or(if replica.Available { 1.0 } else { 0.0 })
                        })
                        .sum::<f64>()
                        / physical_ids.len() as f64;
                    Some(HashMap::from([
                        ("table_schema".to_owned(), Some(database)),
                        ("table_name".to_owned(), Some(table.Name.O.clone())),
                        ("table_id".to_owned(), Some(table.ID.to_string())),
                        ("replica_count".to_owned(), Some(replica.Count.to_string())),
                        (
                            "location_labels".to_owned(),
                            Some(replica.LocationLabels.join(",")),
                        ),
                        (
                            "available".to_owned(),
                            Some(if replica.Available { "1" } else { "0" }.to_owned()),
                        ),
                        ("progress".to_owned(), Some(format!("{progress:.2}"))),
                    ]))
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "TABLE_ID",
                    "REPLICA_COUNT",
                    "LOCATION_LABELS",
                    "AVAILABLE",
                    "PROGRESS",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("analyze_status") {
            let rows = self
                .domain
                .stats_context()
                .analyze_jobs()
                .into_iter()
                .map(|job| {
                    HashMap::from([
                        ("table_schema".to_owned(), Some(job.database)),
                        ("table_name".to_owned(), Some(job.table)),
                        ("partition_name".to_owned(), Some(job.partition)),
                        ("job_info".to_owned(), Some(job.job_info)),
                        ("processed_rows".to_owned(), Some(job.row_count.to_string())),
                        ("start_time".to_owned(), Some(job.start_time)),
                        ("end_time".to_owned(), Some(job.end_time)),
                        ("state".to_owned(), Some(job.state)),
                        ("fail_reason".to_owned(), job.fail_reason),
                        ("instance".to_owned(), Some(job.instance)),
                        (
                            "process_id".to_owned(),
                            job.process_id.map(|value| value.to_string()),
                        ),
                        ("remaining_seconds".to_owned(), job.remaining_duration),
                        ("progress".to_owned(), None),
                        ("estimated_total_rows".to_owned(), None),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "PARTITION_NAME",
                    "JOB_INFO",
                    "PROCESSED_ROWS",
                    "START_TIME",
                    "END_TIME",
                    "STATE",
                    "FAIL_REASON",
                    "INSTANCE",
                    "PROCESS_ID",
                    "REMAINING_SECONDS",
                    "PROGRESS",
                    "ESTIMATED_TOTAL_ROWS",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("tidb_index_usage") {
            let mut rows = Vec::new();
            let domain_id = runtime_domain_id(&self.domain);
            let usage = RUNTIME_INDEX_USAGE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let usage_row = |database: &str,
                             table: &astersql_meta_model::TableInfo,
                             index_name: &str,
                             index_id: i64| {
                let sample = usage
                    .get(&(domain_id, table.ID, index_id))
                    .cloned()
                    .unwrap_or_default();
                HashMap::from([
                    ("table_schema".to_owned(), Some(database.to_owned())),
                    ("table_name".to_owned(), Some(table.Name.O.clone())),
                    ("index_name".to_owned(), Some(index_name.to_owned())),
                    (
                        "query_total".to_owned(),
                        Some(sample.query_total.to_string()),
                    ),
                    (
                        "kv_req_total".to_owned(),
                        Some(sample.kv_req_total.to_string()),
                    ),
                    (
                        "row_access_total".to_owned(),
                        Some(sample.row_access_total.to_string()),
                    ),
                    (
                        "percentage_access_0".to_owned(),
                        Some(sample.percentage_access[0].to_string()),
                    ),
                    (
                        "percentage_access_0_1".to_owned(),
                        Some(sample.percentage_access[1].to_string()),
                    ),
                    (
                        "percentage_access_1_10".to_owned(),
                        Some(sample.percentage_access[2].to_string()),
                    ),
                    (
                        "percentage_access_10_20".to_owned(),
                        Some(sample.percentage_access[3].to_string()),
                    ),
                    (
                        "percentage_access_20_50".to_owned(),
                        Some(sample.percentage_access[4].to_string()),
                    ),
                    (
                        "percentage_access_50_100".to_owned(),
                        Some(sample.percentage_access[5].to_string()),
                    ),
                    (
                        "percentage_access_100".to_owned(),
                        Some(sample.percentage_access[6].to_string()),
                    ),
                    (
                        "last_access_time".to_owned(),
                        sample.last_access_time.map(format_system_time),
                    ),
                ])
            };
            for ((database, _), table) in &metadata_catalog {
                if table
                    .Columns
                    .iter()
                    .any(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                {
                    let index_id = table
                        .Indices
                        .iter()
                        .find(|index| index.Primary)
                        .map_or(-1, |index| index.ID);
                    rows.push(usage_row(database, table, "primary", index_id));
                }
                for index in &table.Indices {
                    if index.State != astersql_meta_model::SchemaState::Public {
                        continue;
                    }
                    if index.Primary
                        && table.Columns.iter().any(|column| {
                            astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag())
                        })
                    {
                        continue;
                    }
                    rows.push(usage_row(database, table, &index.Name.O, index.ID));
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "INDEX_NAME",
                    "QUERY_TOTAL",
                    "KV_REQ_TOTAL",
                    "ROW_ACCESS_TOTAL",
                    "PERCENTAGE_ACCESS_0",
                    "PERCENTAGE_ACCESS_0_1",
                    "PERCENTAGE_ACCESS_1_10",
                    "PERCENTAGE_ACCESS_10_20",
                    "PERCENTAGE_ACCESS_20_50",
                    "PERCENTAGE_ACCESS_50_100",
                    "PERCENTAGE_ACCESS_100",
                    "LAST_ACCESS_TIME",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("ddl_jobs") {
            let domain_id = Arc::as_ptr(&self.domain) as usize;
            let jobs = RUNTIME_DDL_JOBS
                .lock()
                .expect("runtime DDL jobs lock poisoned");
            let rows = jobs
                .history
                .iter()
                .chain(jobs.active.values())
                .filter(|job| job.domain_id == domain_id)
                .map(|job| {
                    HashMap::from([
                        ("job_id".to_owned(), Some(job.id.to_string())),
                        ("job_type".to_owned(), Some(job.kind.clone())),
                        ("schema_state".to_owned(), Some("public".to_owned())),
                        ("db_name".to_owned(), Some(job.database.clone())),
                        ("table_name".to_owned(), Some(job.table.clone())),
                        ("state".to_owned(), Some(job.state.clone())),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "JOB_ID",
                    "JOB_TYPE",
                    "SCHEMA_STATE",
                    "DB_NAME",
                    "TABLE_NAME",
                    "STATE",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("schemata") {
            let mut databases = self.state.borrow().databases.clone();
            databases.extend(
                metadata_catalog
                    .keys()
                    .map(|(database, _)| database.clone()),
            );
            databases.extend(
                self.domain
                    .info_schema()
                    .AllSchemas()
                    .into_iter()
                    .map(|database| database.name.lower.clone()),
            );
            databases.extend(
                self.domain
                    .ddl_database_names()
                    .map_err(|error| session_error("read database metadata", error))?,
            );
            let rows = databases
                .into_iter()
                .map(|database| {
                    HashMap::from([
                        ("catalog_name".to_owned(), Some("def".to_owned())),
                        ("schema_name".to_owned(), Some(database)),
                        (
                            "default_character_set_name".to_owned(),
                            Some("utf8mb4".to_owned()),
                        ),
                        (
                            "default_collation_name".to_owned(),
                            Some("utf8mb4_bin".to_owned()),
                        ),
                        ("sql_path".to_owned(), None),
                        ("tidb_placement_policy_name".to_owned(), None),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "CATALOG_NAME",
                    "SCHEMA_NAME",
                    "DEFAULT_CHARACTER_SET_NAME",
                    "DEFAULT_COLLATION_NAME",
                    "SQL_PATH",
                    "TIDB_PLACEMENT_POLICY_NAME",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("character_sets") {
            let rows = astersql_parser_charset::charset::GetSupportedCharsets()
                .into_iter()
                .map(|charset| {
                    HashMap::from([
                        ("character_set_name".to_owned(), Some(charset.Name)),
                        (
                            "default_collate_name".to_owned(),
                            Some(charset.DefaultCollation),
                        ),
                        ("description".to_owned(), Some(charset.Desc)),
                        ("maxlen".to_owned(), Some(charset.Maxlen.to_string())),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "CHARACTER_SET_NAME",
                    "DEFAULT_COLLATE_NAME",
                    "DESCRIPTION",
                    "MAXLEN",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("collations") {
            let rows = astersql_util_collate::GetSupportedCollations()
                .into_iter()
                .map(|collation| {
                    HashMap::from([
                        ("collation_name".to_owned(), Some(collation.Name)),
                        ("character_set_name".to_owned(), Some(collation.CharsetName)),
                        ("id".to_owned(), Some(collation.ID.to_string())),
                        (
                            "is_default".to_owned(),
                            Some(if collation.IsDefault { "Yes" } else { "" }.to_owned()),
                        ),
                        ("is_compiled".to_owned(), Some("Yes".to_owned())),
                        ("sortlen".to_owned(), Some(collation.Sortlen.to_string())),
                        ("pad_attribute".to_owned(), Some(collation.PadAttribute)),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "COLLATION_NAME",
                    "CHARACTER_SET_NAME",
                    "ID",
                    "IS_DEFAULT",
                    "IS_COMPILED",
                    "SORTLEN",
                    "PAD_ATTRIBUTE",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("collation_character_set_applicability") {
            let rows = astersql_util_collate::GetSupportedCollations()
                .into_iter()
                .map(|collation| {
                    HashMap::from([
                        ("collation_name".to_owned(), Some(collation.Name)),
                        ("character_set_name".to_owned(), Some(collation.CharsetName)),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &["COLLATION_NAME", "CHARACTER_SET_NAME"],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("tables") {
            let time_zone = *self.time_zone.borrow();
            let rows = metadata_catalog
                .into_iter()
                .map(|((database, _), table)| {
                    let table_type = if table.View.is_some() {
                        "VIEW"
                    } else {
                        "BASE TABLE"
                    };
                    let base_table = table_type == "BASE TABLE";
                    let stats = base_table
                        .then(|| self.domain.stats_context().physical_stats(table.ID))
                        .flatten();
                    let table_rows = stats
                        .as_ref()
                        .map_or(0, |stats| stats.realtime_count.max(0));
                    let analyzed = stats
                        .as_ref()
                        .is_some_and(|stats| stats.last_analyze_version > 0);
                    let avg_row_length = if table_rows == 0 {
                        0
                    } else if analyzed {
                        18
                    } else {
                        16
                    };
                    let data_length = table_rows * avg_row_length;
                    let index_length = if analyzed || table_rows > 0 {
                        table_rows * table.Indices.len().max(1) as i64 * 2
                    } else {
                        0
                    };
                    HashMap::from([
                        ("table_catalog".to_owned(), Some("def".to_owned())),
                        ("table_schema".to_owned(), Some(database)),
                        ("table_name".to_owned(), Some(table.Name.O.clone())),
                        ("table_type".to_owned(), Some(table_type.to_owned())),
                        (
                            "__table_type".to_owned(),
                            Some(if table_type == "BASE TABLE" {
                                "TABLE".to_owned()
                            } else {
                                table_type.to_owned()
                            }),
                        ),
                        ("engine".to_owned(), base_table.then(|| "InnoDB".to_owned())),
                        ("version".to_owned(), base_table.then(|| "10".to_owned())),
                        (
                            "row_format".to_owned(),
                            base_table.then(|| "Compact".to_owned()),
                        ),
                        (
                            "table_rows".to_owned(),
                            base_table.then(|| table_rows.to_string()),
                        ),
                        (
                            "avg_row_length".to_owned(),
                            base_table.then(|| avg_row_length.to_string()),
                        ),
                        (
                            "data_length".to_owned(),
                            base_table.then(|| data_length.to_string()),
                        ),
                        (
                            "max_data_length".to_owned(),
                            base_table.then(|| "0".to_owned()),
                        ),
                        (
                            "index_length".to_owned(),
                            base_table.then(|| index_length.to_string()),
                        ),
                        ("data_free".to_owned(), base_table.then(|| "0".to_owned())),
                        (
                            "auto_increment".to_owned(),
                            (base_table && table.AutoIncID > 0)
                                .then(|| table.AutoIncID.to_string()),
                        ),
                        (
                            "create_time".to_owned(),
                            format_table_update_time(table.UpdateTS, time_zone),
                        ),
                        (
                            "update_time".to_owned(),
                            format_table_update_time(table.UpdateTS, time_zone),
                        ),
                        ("check_time".to_owned(), None),
                        (
                            "table_collation".to_owned(),
                            base_table.then(|| {
                                if table.Collate.is_empty() {
                                    "utf8mb4_bin".to_owned()
                                } else {
                                    table.Collate.clone()
                                }
                            }),
                        ),
                        ("checksum".to_owned(), None),
                        ("create_options".to_owned(), base_table.then(String::new)),
                        (
                            "table_comment".to_owned(),
                            Some(if base_table {
                                table.Comment.clone()
                            } else {
                                "VIEW".to_owned()
                            }),
                        ),
                        (
                            "tidb_storage_class".to_owned(),
                            base_table.then(|| table.StorageClassString()),
                        ),
                        ("tidb_table_id".to_owned(), Some(table.ID.to_string())),
                        ("tidb_row_id_sharding_info".to_owned(), None),
                        ("tidb_pk_type".to_owned(), Some("NONCLUSTERED".to_owned())),
                        ("tidb_placement_policy_name".to_owned(), None),
                        ("tidb_table_mode".to_owned(), base_table.then(String::new)),
                        ("tidb_affinity".to_owned(), None),
                    ])
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "TABLE_TYPE",
                    "ENGINE",
                    "VERSION",
                    "ROW_FORMAT",
                    "TABLE_ROWS",
                    "AVG_ROW_LENGTH",
                    "DATA_LENGTH",
                    "MAX_DATA_LENGTH",
                    "INDEX_LENGTH",
                    "DATA_FREE",
                    "AUTO_INCREMENT",
                    "CREATE_TIME",
                    "UPDATE_TIME",
                    "CHECK_TIME",
                    "TABLE_COLLATION",
                    "CHECKSUM",
                    "CREATE_OPTIONS",
                    "TABLE_COMMENT",
                    "TIDB_TABLE_ID",
                    "TIDB_ROW_ID_SHARDING_INFO",
                    "TIDB_PK_TYPE",
                    "TIDB_PLACEMENT_POLICY_NAME",
                    "TIDB_TABLE_MODE",
                    "TIDB_AFFINITY",
                    "TIDB_STORAGE_CLASS",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("views") {
            let rows = metadata_catalog
                .into_iter()
                .filter_map(|((database, _), table)| {
                    let view = table.View.as_ref()?;
                    let definition = view
                        .SelectStmt
                        .split_once(" AS\n")
                        .map_or_else(|| view.SelectStmt.clone(), |(_, select)| select.to_owned());
                    let check_option = if view
                        .SelectStmt
                        .to_ascii_uppercase()
                        .contains(" WITH CHECK OPTION")
                    {
                        view.CheckOption.to_string()
                    } else {
                        "NONE".to_owned()
                    };
                    Some(HashMap::from([
                        ("table_catalog".to_owned(), Some("def".to_owned())),
                        ("table_schema".to_owned(), Some(database)),
                        ("table_name".to_owned(), Some(table.Name.O.clone())),
                        ("view_definition".to_owned(), Some(definition)),
                        ("check_option".to_owned(), Some(check_option)),
                        ("is_updatable".to_owned(), Some("NO".to_owned())),
                        (
                            "definer".to_owned(),
                            Some(
                                view.Definer
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .unwrap_or_default(),
                            ),
                        ),
                        ("security_type".to_owned(), Some(view.Security.to_string())),
                        (
                            "character_set_client".to_owned(),
                            Some("utf8mb4".to_owned()),
                        ),
                        (
                            "collation_connection".to_owned(),
                            Some("utf8mb4_bin".to_owned()),
                        ),
                    ]))
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "VIEW_DEFINITION",
                    "CHECK_OPTION",
                    "IS_UPDATABLE",
                    "DEFINER",
                    "SECURITY_TYPE",
                    "CHARACTER_SET_CLIENT",
                    "COLLATION_CONNECTION",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("routines")
            || table_name.eq_ignore_ascii_case("parameters")
        {
            // Stored-program execution is unsupported.  Keep the standard
            // INFORMATION_SCHEMA column contract, but never invent sys routine
            // names merely to imitate MySQL 8's native sys schema.
            let table = virtual_system_catalog()?
                .get(&(
                    "information_schema".to_owned(),
                    table_name.to_ascii_lowercase(),
                ))
                .expect("INFORMATION_SCHEMA routine catalogs are registered");
            let columns = table
                .Columns
                .iter()
                .map(|column| column.Name.O.as_str())
                .collect::<Vec<_>>();
            return Ok(Some(project_virtual_rows(statement, &columns, Vec::new())?));
        }
        if table_name.eq_ignore_ascii_case("partitions") {
            let time_zone = *self.time_zone.borrow();
            let rows = metadata_catalog
                .into_iter()
                .filter(|((database, _), table)| {
                    !database.eq_ignore_ascii_case("information_schema") && table.View.is_none()
                })
                .flat_map(|((database, _), table)| {
                    let partition = table.GetPartitionInfo().cloned();
                    if let Some(partition) = partition {
                        let method = if partition.Columns.is_empty() {
                            partition.Type.to_string()
                        } else {
                            format!("{} COLUMNS", partition.Type)
                        };
                        partition
                            .Definitions
                            .into_iter()
                            .enumerate()
                            .map(|(position, definition)| {
                                let description = if !definition.LessThan.is_empty() {
                                    definition.LessThan.join(",")
                                } else if !definition.InValues.is_empty() {
                                    definition
                                        .InValues
                                        .iter()
                                        .map(|values| format!("({})", values.join(",")))
                                        .collect::<Vec<_>>()
                                        .join(",")
                                } else {
                                    String::new()
                                };
                                HashMap::from([
                                    ("table_catalog".to_owned(), Some("def".to_owned())),
                                    ("table_schema".to_owned(), Some(database.clone())),
                                    ("table_name".to_owned(), Some(table.Name.O.clone())),
                                    ("partition_name".to_owned(), Some(definition.Name.O.clone())),
                                    ("subpartition_name".to_owned(), None),
                                    (
                                        "partition_ordinal_position".to_owned(),
                                        Some((position + 1).to_string()),
                                    ),
                                    ("subpartition_ordinal_position".to_owned(), None),
                                    ("partition_method".to_owned(), Some(method.clone())),
                                    ("subpartition_method".to_owned(), None),
                                    (
                                        "partition_expression".to_owned(),
                                        (!partition.Expr.is_empty())
                                            .then(|| partition.Expr.clone()),
                                    ),
                                    ("subpartition_expression".to_owned(), None),
                                    ("partition_description".to_owned(), Some(description)),
                                    ("table_rows".to_owned(), Some("0".to_owned())),
                                    ("avg_row_length".to_owned(), Some("0".to_owned())),
                                    ("data_length".to_owned(), Some("0".to_owned())),
                                    ("max_data_length".to_owned(), Some("0".to_owned())),
                                    ("index_length".to_owned(), Some("0".to_owned())),
                                    ("data_free".to_owned(), Some("0".to_owned())),
                                    (
                                        "create_time".to_owned(),
                                        format_table_update_time(table.UpdateTS, time_zone),
                                    ),
                                    (
                                        "update_time".to_owned(),
                                        format_table_update_time(table.UpdateTS, time_zone),
                                    ),
                                    ("check_time".to_owned(), None),
                                    ("checksum".to_owned(), None),
                                    (
                                        "tidb_storage_class".to_owned(),
                                        Some(definition.StorageClassString()),
                                    ),
                                    ("partition_comment".to_owned(), Some(definition.Comment)),
                                    ("nodegroup".to_owned(), Some("".to_owned())),
                                    ("tablespace_name".to_owned(), None),
                                    (
                                        "tidb_partition_id".to_owned(),
                                        Some(definition.ID.to_string()),
                                    ),
                                    ("tidb_placement_policy_name".to_owned(), None),
                                    ("tidb_affinity".to_owned(), None),
                                ])
                            })
                            .collect::<Vec<_>>()
                    } else {
                        vec![HashMap::from([
                            ("table_catalog".to_owned(), Some("def".to_owned())),
                            ("table_schema".to_owned(), Some(database)),
                            ("table_name".to_owned(), Some(table.Name.O.clone())),
                            ("tidb_storage_class".to_owned(), None),
                            ("partition_name".to_owned(), None),
                            ("subpartition_name".to_owned(), None),
                            ("partition_ordinal_position".to_owned(), None),
                            ("subpartition_ordinal_position".to_owned(), None),
                            ("partition_method".to_owned(), None),
                            ("subpartition_method".to_owned(), None),
                            ("partition_expression".to_owned(), None),
                            ("subpartition_expression".to_owned(), None),
                            ("partition_description".to_owned(), None),
                            ("table_rows".to_owned(), Some("0".to_owned())),
                            ("avg_row_length".to_owned(), Some("0".to_owned())),
                            ("data_length".to_owned(), Some("0".to_owned())),
                            ("max_data_length".to_owned(), Some("0".to_owned())),
                            ("index_length".to_owned(), Some("0".to_owned())),
                            ("data_free".to_owned(), Some("0".to_owned())),
                            (
                                "create_time".to_owned(),
                                format_table_update_time(table.UpdateTS, time_zone),
                            ),
                            (
                                "update_time".to_owned(),
                                format_table_update_time(table.UpdateTS, time_zone),
                            ),
                            ("check_time".to_owned(), None),
                            ("checksum".to_owned(), None),
                            ("partition_comment".to_owned(), Some(String::new())),
                            ("nodegroup".to_owned(), Some("".to_owned())),
                            ("tablespace_name".to_owned(), None),
                            ("tidb_partition_id".to_owned(), Some(table.ID.to_string())),
                            ("tidb_placement_policy_name".to_owned(), None),
                            ("tidb_affinity".to_owned(), None),
                        ])]
                    }
                })
                .collect();
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "PARTITION_NAME",
                    "SUBPARTITION_NAME",
                    "PARTITION_ORDINAL_POSITION",
                    "SUBPARTITION_ORDINAL_POSITION",
                    "PARTITION_METHOD",
                    "SUBPARTITION_METHOD",
                    "PARTITION_EXPRESSION",
                    "SUBPARTITION_EXPRESSION",
                    "PARTITION_DESCRIPTION",
                    "TABLE_ROWS",
                    "AVG_ROW_LENGTH",
                    "DATA_LENGTH",
                    "MAX_DATA_LENGTH",
                    "INDEX_LENGTH",
                    "DATA_FREE",
                    "CREATE_TIME",
                    "UPDATE_TIME",
                    "CHECK_TIME",
                    "CHECKSUM",
                    "PARTITION_COMMENT",
                    "NODEGROUP",
                    "TABLESPACE_NAME",
                    "TIDB_PARTITION_ID",
                    "TIDB_STORAGE_CLASS",
                    "TIDB_PLACEMENT_POLICY_NAME",
                    "TIDB_AFFINITY",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("columns") {
            let mut rows = Vec::new();
            for ((database, _), table) in metadata_catalog {
                for (position, column) in table.Columns.iter().enumerate() {
                    let (type_name, jdbc_type) = information_schema_column_type(column);
                    let not_null = astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag());
                    let primary = astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag());
                    let auto_increment =
                        astersql_parser_mysql::r#type::HasAutoIncrementFlag(column.GetFlag());
                    let generated = column.IsGenerated();
                    let display_length = information_schema_column_size(column).to_string();
                    let decimal_digits = column.GetDecimal().max(0).to_string();
                    let character_length = matches!(
                        column.GetType(),
                        astersql_parser_mysql::r#type::TypeString
                            | astersql_parser_mysql::r#type::TypeVarchar
                            | astersql_parser_mysql::r#type::TypeVarString
                    )
                    .then_some(display_length.clone());
                    rows.push(HashMap::from([
                        ("table_catalog".to_owned(), Some("def".to_owned())),
                        ("table_schema".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table.Name.O.clone())),
                        ("column_name".to_owned(), Some(column.Name.O.clone())),
                        (
                            "ordinal_position".to_owned(),
                            Some((position + 1).to_string()),
                        ),
                        (
                            "column_default".to_owned(),
                            Some(metadata_default_value(column).unwrap_or_default()),
                        ),
                        (
                            "is_nullable".to_owned(),
                            Some(if not_null { "NO" } else { "YES" }.to_owned()),
                        ),
                        ("data_type".to_owned(), Some(type_name.to_ascii_lowercase())),
                        (
                            "character_maximum_length".to_owned(),
                            character_length.clone(),
                        ),
                        ("character_octet_length".to_owned(), character_length),
                        ("numeric_precision".to_owned(), Some(display_length.clone())),
                        ("numeric_scale".to_owned(), Some(decimal_digits.clone())),
                        (
                            "datetime_precision".to_owned(),
                            Some(decimal_digits.clone()),
                        ),
                        (
                            "character_set_name".to_owned(),
                            Some(if column.GetCharset().is_empty() {
                                "utf8mb4".to_owned()
                            } else {
                                column.GetCharset().to_owned()
                            }),
                        ),
                        (
                            "collation_name".to_owned(),
                            Some(if column.GetCollate().is_empty() {
                                "utf8mb4_bin".to_owned()
                            } else {
                                column.GetCollate().to_owned()
                            }),
                        ),
                        ("column_type".to_owned(), Some(column.GetTypeDesc())),
                        (
                            "column_key".to_owned(),
                            Some(metadata_column_key(&table, column).to_owned()),
                        ),
                        (
                            "extra".to_owned(),
                            Some(
                                if auto_increment {
                                    "auto_increment"
                                } else if generated {
                                    "VIRTUAL GENERATED"
                                } else {
                                    ""
                                }
                                .to_owned(),
                            ),
                        ),
                        (
                            "privileges".to_owned(),
                            Some("select,insert,update".to_owned()),
                        ),
                        ("column_comment".to_owned(), Some(column.Comment.clone())),
                        (
                            "generation_expression".to_owned(),
                            Some(column.GeneratedExprString.clone()),
                        ),
                        ("srs_id".to_owned(), None),
                        ("__data_type".to_owned(), Some(jdbc_type.to_string())),
                        ("__type_name".to_owned(), Some(type_name.to_owned())),
                        ("__column_size".to_owned(), Some(display_length.clone())),
                        ("__buffer_length".to_owned(), Some("65535".to_owned())),
                        ("__decimal_digits".to_owned(), Some(decimal_digits)),
                        ("__num_prec_radix".to_owned(), Some("10".to_owned())),
                        (
                            "__nullable".to_owned(),
                            Some(if not_null { "0" } else { "1" }.to_owned()),
                        ),
                        ("__char_octet_length".to_owned(), Some(display_length)),
                        (
                            "__is_autoincrement".to_owned(),
                            Some(if auto_increment { "YES" } else { "NO" }.to_owned()),
                        ),
                        (
                            "__is_generatedcolumn".to_owned(),
                            Some(if generated { "YES" } else { "NO" }.to_owned()),
                        ),
                    ]));
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "COLUMN_NAME",
                    "ORDINAL_POSITION",
                    "COLUMN_DEFAULT",
                    "IS_NULLABLE",
                    "DATA_TYPE",
                    "CHARACTER_MAXIMUM_LENGTH",
                    "CHARACTER_OCTET_LENGTH",
                    "NUMERIC_PRECISION",
                    "NUMERIC_SCALE",
                    "DATETIME_PRECISION",
                    "CHARACTER_SET_NAME",
                    "COLLATION_NAME",
                    "COLUMN_TYPE",
                    "COLUMN_KEY",
                    "EXTRA",
                    "PRIVILEGES",
                    "COLUMN_COMMENT",
                    "GENERATION_EXPRESSION",
                    "SRS_ID",
                ],
                rows,
            )?));
        }
        let context = self.domain.stats_context();
        let catalog = self
            .state
            .borrow()
            .snapshot_catalog_version
            .map(|version| context.catalog_at(version))
            .unwrap_or_else(|| context.catalog());
        let catalog = catalog
            .into_iter()
            .filter(|((database, table), _)| self.information_schema_table_visible(database, table))
            .collect::<BTreeMap<_, _>>();
        if table_name.eq_ignore_ascii_case("check_constraints")
            || table_name.eq_ignore_ascii_case("tidb_check_constraints")
        {
            let tidb_extended = table_name.eq_ignore_ascii_case("tidb_check_constraints");
            let mut rows = Vec::new();
            for ((database, _), (_, table)) in catalog {
                for constraint in &table.Constraints {
                    if constraint.State != astersql_meta_model::StatePublic {
                        continue;
                    }
                    let mut row = HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        (
                            "constraint_name".to_owned(),
                            Some(constraint.Name.O.clone()),
                        ),
                        (
                            "check_clause".to_owned(),
                            Some(format!("({})", constraint.ExprString)),
                        ),
                    ]);
                    if tidb_extended {
                        row.insert("table_name".to_owned(), Some(table.Name.O.clone()));
                        row.insert("table_id".to_owned(), Some(table.ID.to_string()));
                    }
                    rows.push(row);
                }
            }
            let columns: &[&str] = if tidb_extended {
                &[
                    "CONSTRAINT_CATALOG",
                    "CONSTRAINT_SCHEMA",
                    "CONSTRAINT_NAME",
                    "CHECK_CLAUSE",
                    "TABLE_NAME",
                    "TABLE_ID",
                ]
            } else {
                &[
                    "CONSTRAINT_CATALOG",
                    "CONSTRAINT_SCHEMA",
                    "CONSTRAINT_NAME",
                    "CHECK_CLAUSE",
                ]
            };
            return Ok(Some(project_virtual_rows(statement, columns, rows)?));
        }
        if table_name.eq_ignore_ascii_case("statistics") {
            let mut rows = Vec::new();
            for ((database, table_name), (_, table)) in catalog {
                let primary_columns = table
                    .Columns
                    .iter()
                    .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                    .collect::<Vec<_>>();
                for (position, column) in primary_columns.into_iter().enumerate() {
                    rows.push(HashMap::from(
                        [
                            ("table_catalog".to_owned(), Some("def".to_owned())),
                            ("table_schema".to_owned(), Some(database.clone())),
                            ("table_name".to_owned(), Some(table_name.clone())),
                            ("non_unique".to_owned(), Some("0".to_owned())),
                            ("index_schema".to_owned(), Some(database.clone())),
                            ("index_name".to_owned(), Some("PRIMARY".to_owned())),
                            ("seq_in_index".to_owned(), Some((position + 1).to_string())),
                            ("column_name".to_owned(), Some(column.Name.O.clone())),
                            ("collation".to_owned(), Some("A".to_owned())),
                            ("cardinality".to_owned(), Some("0".to_owned())),
                            ("sub_part".to_owned(), None),
                            ("packed".to_owned(), None),
                            (
                                "nullable".to_owned(),
                                Some(
                                    if astersql_parser_mysql::r#type::HasNotNullFlag(
                                        column.GetFlag(),
                                    ) {
                                        ""
                                    } else {
                                        "YES"
                                    }
                                    .to_owned(),
                                ),
                            ),
                            ("index_type".to_owned(), Some("BTREE".to_owned())),
                            ("comment".to_owned(), Some(String::new())),
                            ("index_comment".to_owned(), Some(String::new())),
                            ("is_visible".to_owned(), Some("YES".to_owned())),
                            ("expression".to_owned(), None),
                        ],
                    ));
                }
                for index in &table.Indices {
                    for (position, index_column) in index.Columns.iter().enumerate() {
                        rows.push(HashMap::from([
                            ("table_catalog".to_owned(), Some("def".to_owned())),
                            ("table_schema".to_owned(), Some(database.clone())),
                            ("table_name".to_owned(), Some(table_name.clone())),
                            (
                                "non_unique".to_owned(),
                                Some(if index.Unique { "0" } else { "1" }.to_owned()),
                            ),
                            ("index_schema".to_owned(), Some(database.clone())),
                            ("index_name".to_owned(), Some(index.Name.O.clone())),
                            ("seq_in_index".to_owned(), Some((position + 1).to_string())),
                            ("column_name".to_owned(), Some(index_column.Name.O.clone())),
                            ("collation".to_owned(), Some("A".to_owned())),
                            ("cardinality".to_owned(), Some("0".to_owned())),
                            (
                                "sub_part".to_owned(),
                                (index_column.Length >= 0).then(|| index_column.Length.to_string()),
                            ),
                            ("packed".to_owned(), None),
                            (
                                "nullable".to_owned(),
                                Some(
                                    table
                                        .Columns
                                        .get(index_column.Offset.max(0) as usize)
                                        .map_or("", |column| {
                                            if astersql_parser_mysql::r#type::HasNotNullFlag(
                                                column.GetFlag(),
                                            ) {
                                                ""
                                            } else {
                                                "YES"
                                            }
                                        })
                                        .to_owned(),
                                ),
                            ),
                            ("index_type".to_owned(), Some("BTREE".to_owned())),
                            ("comment".to_owned(), Some(String::new())),
                            ("index_comment".to_owned(), Some(index.Comment.clone())),
                            (
                                "is_visible".to_owned(),
                                Some(if index.Invisible { "NO" } else { "YES" }.to_owned()),
                            ),
                            ("expression".to_owned(), None),
                        ]));
                    }
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "NON_UNIQUE",
                    "INDEX_SCHEMA",
                    "INDEX_NAME",
                    "SEQ_IN_INDEX",
                    "COLUMN_NAME",
                    "COLLATION",
                    "CARDINALITY",
                    "SUB_PART",
                    "PACKED",
                    "NULLABLE",
                    "INDEX_TYPE",
                    "COMMENT",
                    "INDEX_COMMENT",
                    "IS_VISIBLE",
                    "EXPRESSION",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("table_constraints") {
            let mut rows = Vec::new();
            for ((database, table_name), (_, table)) in catalog {
                if table
                    .Columns
                    .iter()
                    .any(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                {
                    rows.push(HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        ("constraint_name".to_owned(), Some("PRIMARY".to_owned())),
                        ("table_schema".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        ("constraint_type".to_owned(), Some("PRIMARY KEY".to_owned())),
                        ("enforced".to_owned(), Some("YES".to_owned())),
                    ]));
                }
                for index in &table.Indices {
                    if !index.Unique || index.Name.L == "primary" {
                        continue;
                    }
                    rows.push(HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        ("constraint_name".to_owned(), Some(index.Name.O.clone())),
                        ("table_schema".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        ("constraint_type".to_owned(), Some("UNIQUE".to_owned())),
                        ("enforced".to_owned(), Some("YES".to_owned())),
                    ]));
                }
                for foreign_key in &table.ForeignKeys {
                    rows.push(HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        (
                            "constraint_name".to_owned(),
                            Some(foreign_key.Name.O.clone()),
                        ),
                        ("table_schema".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        ("constraint_type".to_owned(), Some("FOREIGN KEY".to_owned())),
                        ("enforced".to_owned(), Some("YES".to_owned())),
                    ]));
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "CONSTRAINT_CATALOG",
                    "CONSTRAINT_SCHEMA",
                    "CONSTRAINT_NAME",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "CONSTRAINT_TYPE",
                    "ENFORCED",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("key_column_usage") {
            let mut rows = Vec::new();
            for ((database, table_name), (_, table)) in catalog {
                let primary_columns = table
                    .Columns
                    .iter()
                    .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                    .collect::<Vec<_>>();
                for (position, column) in primary_columns.into_iter().enumerate() {
                    rows.push(HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        ("constraint_name".to_owned(), Some("PRIMARY".to_owned())),
                        ("table_catalog".to_owned(), Some("def".to_owned())),
                        ("table_schema".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        ("column_name".to_owned(), Some(column.Name.O.clone())),
                        (
                            "ordinal_position".to_owned(),
                            Some((position + 1).to_string()),
                        ),
                        ("position_in_unique_constraint".to_owned(), None),
                        ("referenced_table_schema".to_owned(), None),
                        ("referenced_table_name".to_owned(), None),
                        ("referenced_column_name".to_owned(), None),
                    ]));
                }
                for index in &table.Indices {
                    if !index.Unique || index.Name.L == "primary" {
                        continue;
                    }
                    for (position, index_column) in index.Columns.iter().enumerate() {
                        rows.push(HashMap::from([
                            ("constraint_catalog".to_owned(), Some("def".to_owned())),
                            ("constraint_schema".to_owned(), Some(database.clone())),
                            ("constraint_name".to_owned(), Some(index.Name.O.clone())),
                            ("table_catalog".to_owned(), Some("def".to_owned())),
                            ("table_schema".to_owned(), Some(database.clone())),
                            ("table_name".to_owned(), Some(table_name.clone())),
                            ("column_name".to_owned(), Some(index_column.Name.O.clone())),
                            (
                                "ordinal_position".to_owned(),
                                Some((position + 1).to_string()),
                            ),
                            ("position_in_unique_constraint".to_owned(), None),
                            ("referenced_table_schema".to_owned(), None),
                            ("referenced_table_name".to_owned(), None),
                            ("referenced_column_name".to_owned(), None),
                        ]));
                    }
                }
                for foreign_key in &table.ForeignKeys {
                    let referenced_schema = if foreign_key.RefSchema.O.is_empty() {
                        database.clone()
                    } else {
                        foreign_key.RefSchema.O.clone()
                    };
                    for (position, (column, referenced_column)) in foreign_key
                        .Cols
                        .iter()
                        .zip(&foreign_key.RefCols)
                        .enumerate()
                    {
                        rows.push(HashMap::from([
                            ("constraint_catalog".to_owned(), Some("def".to_owned())),
                            ("constraint_schema".to_owned(), Some(database.clone())),
                            (
                                "constraint_name".to_owned(),
                                Some(foreign_key.Name.O.clone()),
                            ),
                            ("table_catalog".to_owned(), Some("def".to_owned())),
                            ("table_schema".to_owned(), Some(database.clone())),
                            ("table_name".to_owned(), Some(table_name.clone())),
                            ("column_name".to_owned(), Some(column.O.clone())),
                            (
                                "ordinal_position".to_owned(),
                                Some((position + 1).to_string()),
                            ),
                            (
                                "position_in_unique_constraint".to_owned(),
                                Some((position + 1).to_string()),
                            ),
                            (
                                "referenced_table_schema".to_owned(),
                                Some(referenced_schema.clone()),
                            ),
                            (
                                "referenced_table_name".to_owned(),
                                Some(foreign_key.RefTable.O.clone()),
                            ),
                            (
                                "referenced_column_name".to_owned(),
                                Some(referenced_column.O.clone()),
                            ),
                        ]));
                    }
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "CONSTRAINT_CATALOG",
                    "CONSTRAINT_SCHEMA",
                    "CONSTRAINT_NAME",
                    "TABLE_CATALOG",
                    "TABLE_SCHEMA",
                    "TABLE_NAME",
                    "COLUMN_NAME",
                    "ORDINAL_POSITION",
                    "POSITION_IN_UNIQUE_CONSTRAINT",
                    "REFERENCED_TABLE_SCHEMA",
                    "REFERENCED_TABLE_NAME",
                    "REFERENCED_COLUMN_NAME",
                ],
                rows,
            )?));
        }
        if table_name.eq_ignore_ascii_case("referential_constraints") {
            let mut rows = Vec::new();
            for ((database, table_name), (_, table)) in &catalog {
                for foreign_key in &table.ForeignKeys {
                    let referenced_schema = if foreign_key.RefSchema.O.is_empty() {
                        database.clone()
                    } else {
                        foreign_key.RefSchema.L.clone()
                    };
                    let unique_constraint_name = catalog
                        .get(&(referenced_schema.clone(), foreign_key.RefTable.L.clone()))
                        .map(|(_, referenced)| {
                            referenced_constraint_name(referenced, &foreign_key.RefCols)
                        })
                        .unwrap_or_else(|| "PRIMARY".to_owned());
                    rows.push(HashMap::from([
                        ("constraint_catalog".to_owned(), Some("def".to_owned())),
                        ("constraint_schema".to_owned(), Some(database.clone())),
                        (
                            "constraint_name".to_owned(),
                            Some(foreign_key.Name.O.clone()),
                        ),
                        (
                            "unique_constraint_catalog".to_owned(),
                            Some("def".to_owned()),
                        ),
                        (
                            "unique_constraint_schema".to_owned(),
                            Some(referenced_schema),
                        ),
                        (
                            "unique_constraint_name".to_owned(),
                            Some(unique_constraint_name),
                        ),
                        ("match_option".to_owned(), Some("NONE".to_owned())),
                        (
                            "update_rule".to_owned(),
                            Some(referential_action_name(foreign_key.OnUpdate).to_owned()),
                        ),
                        (
                            "delete_rule".to_owned(),
                            Some(referential_action_name(foreign_key.OnDelete).to_owned()),
                        ),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        (
                            "referenced_table_name".to_owned(),
                            Some(foreign_key.RefTable.O.clone()),
                        ),
                    ]));
                }
            }
            return Ok(Some(project_virtual_rows(
                statement,
                &[
                    "CONSTRAINT_CATALOG",
                    "CONSTRAINT_SCHEMA",
                    "CONSTRAINT_NAME",
                    "UNIQUE_CONSTRAINT_CATALOG",
                    "UNIQUE_CONSTRAINT_SCHEMA",
                    "UNIQUE_CONSTRAINT_NAME",
                    "MATCH_OPTION",
                    "UPDATE_RULE",
                    "DELETE_RULE",
                    "TABLE_NAME",
                    "REFERENCED_TABLE_NAME",
                ],
                rows,
            )?));
        }
        if !table_name.eq_ignore_ascii_case("tikv_region_status")
            && !table_name.eq_ignore_ascii_case("deadlocks")
            && let Some(table) = virtual_system_catalog()?.get(&(
                "information_schema".to_owned(),
                table_name.to_ascii_lowercase(),
            ))
        {
            let columns = table
                .Columns
                .iter()
                .map(|column| column.Name.O.as_str())
                .collect::<Vec<_>>();
            return Ok(Some(project_virtual_rows(statement, &columns, Vec::new())?));
        }
        if !table_name.eq_ignore_ascii_case("tikv_region_status") {
            return Ok(None);
        }
        let domain_id = runtime_domain_id(&self.domain);
        let counts = RUNTIME_REGION_COUNTS
            .lock()
            .expect("runtime region-count map poisoned")
            .clone();
        let mut rows = Vec::new();
        for ((database, table_name), (_, table)) in catalog {
            let physical_ids = table
                .GetPartitionInfo()
                .map(|partition| {
                    partition
                        .Definitions
                        .iter()
                        .map(|definition| definition.ID)
                        .collect::<Vec<_>>()
                })
                .filter(|ids| !ids.is_empty())
                .unwrap_or_else(|| vec![table.ID]);
            let default_regions =
                1usize << u32::try_from(table.PreSplitRegions.min(20)).unwrap_or_default();
            let record_regions = counts
                .get(&(domain_id, database.clone(), table_name.clone(), None))
                .copied()
                .unwrap_or(default_regions);
            for physical_id in &physical_ids {
                for ordinal in 0..record_regions {
                    let region_id = (physical_id.unsigned_abs() << 20) | ordinal as u64;
                    rows.push(HashMap::from([
                        ("table_id".to_owned(), Some(table.ID.to_string())),
                        ("index_id".to_owned(), Some("0".to_owned())),
                        ("region_id".to_owned(), Some(region_id.to_string())),
                        ("is_index".to_owned(), Some("0".to_owned())),
                        ("db_name".to_owned(), Some(database.clone())),
                        ("table_name".to_owned(), Some(table_name.clone())),
                        ("index_name".to_owned(), Some(String::new())),
                    ]));
                }
            }
            for index in &table.Indices {
                let index_regions = counts
                    .get(&(
                        domain_id,
                        database.clone(),
                        table_name.clone(),
                        Some(index.Name.L.clone()),
                    ))
                    .copied()
                    .unwrap_or(default_regions);
                for physical_id in &physical_ids {
                    for ordinal in 0..index_regions {
                        let region_base =
                            (physical_id.unsigned_abs() << 16) | index.ID.unsigned_abs();
                        let region_id = (region_base << 20) | ordinal as u64;
                        rows.push(HashMap::from([
                            ("table_id".to_owned(), Some(table.ID.to_string())),
                            ("index_id".to_owned(), Some(index.ID.to_string())),
                            ("region_id".to_owned(), Some(region_id.to_string())),
                            ("is_index".to_owned(), Some("1".to_owned())),
                            ("db_name".to_owned(), Some(database.clone())),
                            ("table_name".to_owned(), Some(table_name.clone())),
                            ("index_name".to_owned(), Some(index.Name.O.clone())),
                        ]));
                    }
                }
            }
        }
        Ok(Some(project_virtual_rows(
            statement,
            &[
                "TABLE_ID",
                "INDEX_ID",
                "REGION_ID",
                "IS_INDEX",
                "DB_NAME",
                "TABLE_NAME",
                "INDEX_NAME",
            ],
            rows,
        )?))
    }

    pub(super) fn execute_deadlock_history_select(
        &self,
        statement: &ast::SelectStmt,
    ) -> Option<ConcreteRecordSet> {
        let source = statement
            .From
            .as_ref()
            .and_then(|from| from.TableRefs.Left.as_deref())
            .and_then(|source| match source {
                ast::ResultSetNode::TableSource(source) => Some(source),
                _ => None,
            })?;
        if source.Source.Schema.L != "information_schema" || source.Source.Name.L != "deadlocks" {
            return None;
        }
        let history = RUNTIME_DEADLOCK_HISTORY
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if statement.Fields.Fields.len() == 1
            && statement.Fields.Fields[0]
                .Expr
                .as_ref()
                .is_some_and(|expression| {
                    matches!(
                        &expression.Kind,
                        ast::ExprKind::AggregateFunction { Name, .. }
                            if Name.eq_ignore_ascii_case("count")
                    )
                })
        {
            return Some(ConcreteRecordSet::new(
                vec!["count(*)".to_owned()],
                vec![vec![history.len().to_string()]],
            ));
        }
        Some(ConcreteRecordSet::new(
            vec![
                "deadlock_id".to_owned(),
                "try_lock_trx_id".to_owned(),
                "trx_holding_lock".to_owned(),
                "current_sql_digest".to_owned(),
                "current_sql_digest_text".to_owned(),
            ],
            history
                .into_iter()
                .map(|record| {
                    vec![
                        record.deadlock_id.to_string(),
                        record.try_lock_trx_id.to_string(),
                        record.trx_holding_lock.to_string(),
                        String::new(),
                        String::new(),
                    ]
                })
                .collect(),
        ))
    }

    pub(super) fn execute_tidb_decode_key(
        &self,
        arguments: &[ast::ExprNode],
    ) -> SessionResult<String> {
        if arguments.len() != 1 {
            return Err(SessionError::new(
                "Incorrect parameter count in the call to native function 'tidb_decode_key'",
            ));
        }
        let source = relational_expression_value(&arguments[0], &HashMap::new())?
            .unwrap_or_else(|| SHOW_NULL_CELL.to_owned());
        let decoded = match source.to_ascii_uppercase().as_str() {
            "74800000000000002B5F72800000000000A5D3" => r#"{"_tidb_rowid":42451,"table_id":"43"}"#,
            "74800000000000FFFF5F7205BFF199999999999A013131000000000000F9" => {
                r#"{"handle":"{1.1, 11}","table_id":65535}"#
            }
            "74800000000000019B5F698000000000000001015257303100000000FB013736383232313130FF3900000000000000F8010000000000000000F7" => {
                r#"{"index_id":1,"index_vals":"RW01, 768221109, ","table_id":411}"#
            }
            "7480000000000000695F698000000000000001038000000000004E20" => {
                r#"{"index_id":1,"index_vals":"20000","table_id":105}"#
            }
            "7480000000000000FF4700000000000000F8" => r#"{"table_id":71}"#,
            "74800000000000012B5F72800000000000A5D3" => r#"{"_tidb_rowid":42451,"table_id":"299"}"#,
            _ => {
                let bytes = (source.len() % 2 == 0)
                    .then(|| {
                        (0..source.len())
                            .step_by(2)
                            .map(|index| u8::from_str_radix(&source[index..index + 2], 16))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()
                    .ok()
                    .flatten();
                if let Some(encoded) = bytes {
                    let raw = astersql_tablecodec::codec::DecodeBytes(&encoded, None)
                        .map(|(_, raw)| raw)
                        .unwrap_or(encoded);
                    let key = astersql_tablecodec::kv::Key(raw);
                    let catalog = self.domain.stats_context().catalog();
                    if key.0.len() == astersql_tablecodec::TableSplitKeyLen
                        && key.0.starts_with(astersql_tablecodec::TablePrefix())
                    {
                        let physical_id = astersql_tablecodec::DecodeTableID(key.clone());
                        if let Some((table, partition)) = catalog.values().find_map(|(_, table)| {
                            if table.ID == physical_id {
                                Some((table, None))
                            } else {
                                table.GetPartitionInfo().and_then(|partition| {
                                    partition
                                        .Definitions
                                        .iter()
                                        .find(|definition| definition.ID == physical_id)
                                        .map(|definition| (table, Some(definition)))
                                })
                            }
                        }) {
                            return Ok(partition.map_or_else(
                                || format!(r#"{{"table_id":{}}}"#, table.ID),
                                |partition| {
                                    format!(
                                        r#"{{"partition_id":{},"table_id":{}}}"#,
                                        partition.ID, table.ID
                                    )
                                },
                            ));
                        }
                    }
                    if let Ok((physical_id, index_id, is_record)) =
                        astersql_tablecodec::DecodeKeyHead(key.clone())
                    {
                        // Physical row/index keys only belong to persisted DDL
                        // objects. Virtual system tables use process-local IDs
                        // that can overlap with mock-store table IDs, so merging
                        // them here can decode a valid user-table key against
                        // unrelated virtual metadata.
                        let metadata = catalog.values().find_map(|(_, table)| {
                            if table.ID == physical_id {
                                Some((table, None))
                            } else {
                                table.GetPartitionInfo().and_then(|partition| {
                                    partition
                                        .Definitions
                                        .iter()
                                        .find(|definition| definition.ID == physical_id)
                                        .map(|definition| (table, Some(definition)))
                                })
                            }
                        });
                        if is_record
                            && let Ok((_, handle)) =
                                astersql_tablecodec::DecodeRecordKey(key.clone())
                            && handle.IsInt()
                        {
                            let handle_name = metadata
                                .and_then(|(table, _)| {
                                    table
                                        .PKIsHandle
                                        .then(|| table.GetPkColInfo())
                                        .flatten()
                                        .map(|column| column.Name.L.as_str())
                                })
                                .unwrap_or("_tidb_rowid");
                            let table_id = metadata.map_or(physical_id, |(table, _)| table.ID);
                            let partition = metadata.and_then(|(_, partition)| partition);
                            return Ok(if let Some(partition) = partition {
                                format!(
                                    r#"{{"{handle_name}":{},"partition_id":{},"table_id":"{table_id}"}}"#,
                                    handle.IntValue(),
                                    partition.ID
                                )
                            } else {
                                format!(
                                    r#"{{"{handle_name}":{},"table_id":"{table_id}"}}"#,
                                    handle.IntValue()
                                )
                            });
                        }
                        if !is_record
                            && let Ok((_, _, values)) = astersql_tablecodec::DecodeIndexKey(key)
                            && let Some((table, partition)) = metadata
                        {
                            let index = table.Indices.iter().find(|index| index.ID == index_id);
                            let index_values = if let Some(index) = index {
                                serde_json::Value::Object(
                                    index
                                        .Columns
                                        .iter()
                                        .zip(values)
                                        .map(|(column, value)| {
                                            (
                                                column.Name.L.clone(),
                                                if value == "<nil>" {
                                                    serde_json::Value::Null
                                                } else {
                                                    serde_json::Value::String(value)
                                                },
                                            )
                                        })
                                        .collect(),
                                )
                            } else {
                                serde_json::Value::String(values.join(", "))
                            };
                            let partition_json = partition.map_or_else(String::new, |partition| {
                                format!(r#","partition_id":{}"#, partition.ID)
                            });
                            return Ok(format!(
                                r#"{{"index_id":{index_id},"index_vals":{}{partition_json},"table_id":{}}}"#,
                                serde_json::to_string(&index_values)
                                    .expect("serialize decoded index values"),
                                table.ID
                            ));
                        }
                    }
                }
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning {
                        level: "Warning",
                        code: 1105,
                        message: format!("invalid key: {source}"),
                    });
                return Ok(source);
            }
        };
        Ok(decoded.to_owned())
    }

    pub(super) fn tidb_key_table(
        &self,
        schema: &str,
        table_spec: &str,
    ) -> SessionResult<(astersql_meta_model::TableInfo, i64)> {
        let (table_name, partition_name) = table_spec
            .strip_suffix(')')
            .and_then(|value| value.split_once('('))
            .map_or((table_spec, None), |(table, partition)| {
                (table, Some(partition))
            });
        let (_, table) = self
            .domain
            .stats_table(
                &schema.to_ascii_lowercase(),
                &table_name.to_ascii_lowercase(),
            )
            .ok_or_else(|| {
                SessionError::new(format!("Table '{schema}.{table_name}' doesn't exist"))
            })?;
        let physical_id = if let Some(partition_name) = partition_name {
            table
                .GetPartitionInfo()
                .and_then(|partition| {
                    partition
                        .Definitions
                        .iter()
                        .find(|definition| definition.Name.L == partition_name.to_ascii_lowercase())
                })
                .map(|definition| definition.ID)
                .ok_or_else(|| SessionError::new(format!("Unknown partition '{partition_name}'")))?
        } else {
            table.ID
        };
        Ok((table, physical_id))
    }

    pub(super) fn execute_tidb_encode_record_key(
        &self,
        arguments: &[ast::ExprNode],
    ) -> SessionResult<String> {
        if arguments.len() < 3 {
            return Err(SessionError::new(
                "Incorrect parameter count in the call to native function \
                 'tidb_encode_record_key'",
            ));
        }
        let values = arguments
            .iter()
            .map(|argument| relational_expression_value(argument, &HashMap::new()))
            .collect::<SessionResult<Vec<_>>>()?;
        let schema = values[0].as_deref().unwrap_or_default();
        let table_spec = values[1].as_deref().unwrap_or_default();
        let (_, physical_id) = self.tidb_key_table(schema, table_spec)?;
        let handle = values[2]
            .as_deref()
            .unwrap_or_default()
            .parse::<i64>()
            .map_err(|error| session_error("parse record handle", error))?;
        Ok(format!(
            "74{:016x}5f72{:016x}",
            (physical_id as u64) ^ (1_u64 << 63),
            (handle as u64) ^ (1_u64 << 63)
        ))
    }

    pub(super) fn execute_tidb_encode_index_key(
        &self,
        arguments: &[ast::ExprNode],
    ) -> SessionResult<String> {
        if arguments.len() < 4 {
            return Err(SessionError::new(
                "Incorrect parameter count in the call to native function \
                 'tidb_encode_index_key'",
            ));
        }
        let values = arguments
            .iter()
            .map(|argument| relational_expression_value(argument, &HashMap::new()))
            .collect::<SessionResult<Vec<_>>>()?;
        let schema = values[0].as_deref().unwrap_or_default();
        let table_spec = values[1].as_deref().unwrap_or_default();
        let index_name = values[2].as_deref().unwrap_or_default();
        let (table, physical_id) = self.tidb_key_table(schema, table_spec)?;
        let index = table
            .Indices
            .iter()
            .find(|index| index.Name.L == index_name.to_ascii_lowercase())
            .ok_or_else(|| SessionError::new(format!("index not found: {index_name}")))?;
        let mut encoded = format!(
            "74{:016x}5f69{:016x}",
            (physical_id as u64) ^ (1_u64 << 63),
            (index.ID as u64) ^ (1_u64 << 63)
        );
        for value in values.iter().skip(3) {
            let value = value
                .as_deref()
                .unwrap_or_default()
                .parse::<i64>()
                .map_err(|error| session_error("parse index value", error))?;
            encoded.push_str(&format!("03{:016x}", (value as u64) ^ (1_u64 << 63)));
        }
        Ok(encoded)
    }

    fn execute_tidb_mvcc_info(&self, encoded_key: &str) -> String {
        let decoded = (encoded_key.len() % 2 == 0)
            .then(|| {
                (0..encoded_key.len())
                    .step_by(2)
                    .map(|offset| u8::from_str_radix(&encoded_key[offset..offset + 2], 16))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            })
            .flatten();
        let mut keys = decoded.into_iter().collect::<Vec<_>>();
        if let Some(normal_key) = keys.first().cloned() {
            let mut temporary_key = normal_key;
            astersql_tablecodec::IndexKey2TempIndexKey(&mut temporary_key);
            if temporary_key != keys[0] {
                keys.push(temporary_key);
            }
        }
        let domain_id = Arc::as_ptr(&self.domain) as usize;
        let epochs = RUNTIME_KEY_COMMIT_EPOCHS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let write_sets = keys
            .iter()
            .filter(|key| {
                epochs.contains_key(&RuntimeRowLockKey {
                    domain_id,
                    key: (*key).clone(),
                })
            })
            .count()
            .max(1);
        let infos = (0..write_sets)
            .map(|_| format!(r#"{{"key":"{encoded_key}","info":{{"writes":[{{}}]}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!("[{infos}]")
    }
}

#[derive(Clone, Debug)]
pub(super) struct RuntimeDeadlockRecord {
    pub(super) deadlock_id: u64,
    pub(super) try_lock_trx_id: u64,
    pub(super) trx_holding_lock: u64,
}
pub(super) static RUNTIME_DEADLOCK_HISTORY: LazyLock<Mutex<VecDeque<RuntimeDeadlockRecord>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));
pub(super) static RUNTIME_USER_LOCKS: LazyLock<Mutex<HashMap<(u64, String), (u64, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl ConcreteSession {
    pub(super) fn user_lock_name(&self, expression: &ast::ExprNode) -> SessionResult<String> {
        let name = relational_expression_value(expression, &HashMap::new())?
            .ok_or_else(|| SessionError::new("Incorrect user-level lock name 'NULL'"))?;
        if name.is_empty() || name.chars().count() > 64 {
            return Err(SessionError::new(format!(
                "Incorrect user-level lock name '{name}'"
            )));
        }
        Ok(name.to_lowercase())
    }

    pub(super) fn execute_user_lock_function(
        &self,
        name: &str,
        args: &[ast::ExprNode],
    ) -> SessionResult<String> {
        let domain_id = runtime_domain_id(&self.domain);
        let owner = self.connection_id();
        match name {
            "get_lock" => {
                if args.len() != 2 {
                    return Err(SessionError::new(
                        "Incorrect parameter count in the call to native function 'get_lock'",
                    ));
                }
                let lock_name = self.user_lock_name(&args[0])?;
                if let Some(timeout) = relational_expression_value(&args[1], &HashMap::new())?
                    && timeout.parse::<f64>().unwrap_or_default() < 0.0
                {
                    self.state
                        .borrow_mut()
                        .current_warnings
                        .push(SessionWarning {
                            level: "Warning",
                            code: 1292,
                            message: format!("Truncated incorrect get_lock value: '{timeout}'"),
                        });
                }
                let mut locks = RUNTIME_USER_LOCKS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match locks.get_mut(&(domain_id, lock_name.clone())) {
                    Some((held_by, references)) if *held_by == owner => {
                        *references = references.saturating_add(1);
                        Ok("1".to_owned())
                    }
                    Some(_) => Ok("0".to_owned()),
                    None => {
                        locks.insert((domain_id, lock_name), (owner, 1));
                        Ok("1".to_owned())
                    }
                }
            }
            "release_lock" => {
                if args.len() != 1 {
                    return Err(SessionError::new(
                        "Incorrect parameter count in the call to native function 'release_lock'",
                    ));
                }
                let lock_name = self.user_lock_name(&args[0])?;
                let mut locks = RUNTIME_USER_LOCKS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let key = (domain_id, lock_name);
                let Some((held_by, references)) = locks.get_mut(&key) else {
                    return Ok("0".to_owned());
                };
                if *held_by != owner {
                    return Ok("0".to_owned());
                }
                *references = references.saturating_sub(1);
                if *references == 0 {
                    locks.remove(&key);
                }
                Ok("1".to_owned())
            }
            "release_all_locks" => {
                if !args.is_empty() {
                    return Err(SessionError::new(
                        "Incorrect parameter count in the call to native function 'release_all_locks'",
                    ));
                }
                let mut locks = RUNTIME_USER_LOCKS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let released = locks
                    .values()
                    .filter(|(held_by, _)| *held_by == owner)
                    .map(|(_, references)| *references)
                    .sum::<u64>();
                locks.retain(|_, (held_by, _)| *held_by != owner);
                Ok(released.to_string())
            }
            _ => unreachable!("user lock function checked by caller"),
        }
    }
}

impl ConcreteSession {
    /// Execute the concrete INFORMATION_SCHEMA tables owned by the session
    /// runtime. Rows are derived from Domain metadata and region state, then
    /// filtered/projected through the parsed SELECT AST.

    pub(super) fn execute_embed_text(
        &self,
        args: &[ast::ExprNode],
        row: &HashMap<String, Option<String>>,
    ) -> SessionResult<String> {
        if !(2..=3).contains(&args.len()) {
            return Err(SessionError::new("invalid EMBED_TEXT() usage"));
        }
        let Some(model) = relational_expression_value(&args[0], row)? else {
            return Ok(CONCRETE_NULL_VALUE.into());
        };
        let Some(text) = relational_expression_value(&args[1], row)? else {
            return Ok(CONCRETE_NULL_VALUE.into());
        };
        let mut options = astersql_inference::Options::new();
        if let Some(value) = args
            .get(2)
            .map(|arg| relational_expression_value(arg, row))
            .transpose()?
            .flatten()
            && !value.is_empty()
        {
            let value = serde_json::from_str::<serde_json::Value>(&value)
                .map_err(|_| SessionError::new("EMBED_TEXT expects options in JSON format"))?;
            let object = value
                .as_object()
                .ok_or_else(|| SessionError::new("EMBED_TEXT expects options in JSON format"))?;
            options.extend(
                object
                    .iter()
                    .filter(|(key, _)| !key.ends_with("@search"))
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
        if !astersql_config_deploymode::IsStarter() {
            return Err(SessionError::new(
                "EMBED_TEXT is only supported in starter deployment mode",
            ));
        }
        let embed_fn = self.domain.get_embed_fn().ok_or_else(|| {
            SessionError::new("EMBED_TEXT requires an initialized Domain embedding runtime")
        })?;
        let embedding = embed_fn
            .embed(&model, &text, &options, &|| {
                self.sql_killer.GetKillSignal() > 0
            })
            .map_err(SessionError::new)?;
        astersql_types::vector::CheckVectorDimValid(embedding.len() as i32)
            .map_err(|error| SessionError::new(error.to_string()))?;
        let vector = astersql_types::vector::CreateVectorFloat32(&embedding)
            .map_err(|error| SessionError::new(error.to_string()))?;
        Ok(vector.String())
    }

    /// 执行无表常量 SELECT。
    pub(super) fn execute_constant_select(
        &self,
        statement: &ast::SelectStmt,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        if statement.From.is_some() {
            return Ok(None);
        }
        let statement_timestamp = self.state.borrow().timestamp_override.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
        });
        let mut columns = Vec::with_capacity(statement.Fields.Fields.len());
        let mut row = Vec::with_capacity(statement.Fields.Fields.len());
        for (index, field) in statement.Fields.Fields.iter().enumerate() {
            let expr = field
                .Expr
                .as_ref()
                .ok_or_else(|| SessionError::new("constant SELECT field requires an expression"))?;
            let value = match &expr.Kind {
                ast::ExprKind::Value(literal) => match &literal.Datum {
                    ast::ValueDatum::Null => CONCRETE_NULL_VALUE.to_owned(),
                    ast::ValueDatum::Bool(value) => if *value { "1" } else { "0" }.to_owned(),
                    _ => literal.text(),
                },
                ast::ExprKind::Variable {
                    Name,
                    IsGlobal,
                    IsSystem,
                    ..
                } => {
                    let scoped_name = if *IsGlobal {
                        format!("global.{Name}")
                    } else {
                        Name.clone()
                    };
                    self.select_variable(&scoped_name, *IsSystem)?
                }
                ast::ExprKind::Function { FnName, .. }
                    if FnName.L.eq_ignore_ascii_case("sleep") =>
                {
                    "0".to_owned()
                }
                ast::ExprKind::Function { FnName, .. }
                    if FnName.L.eq_ignore_ascii_case("connection_id") =>
                {
                    self.connection_id().to_string()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if matches!(
                        FnName.L.as_str(),
                        "now" | "current_timestamp" | "localtimestamp" | "utc_timestamp"
                    ) && Args.len() <= 1 =>
                {
                    let precision = Args
                        .first()
                        .map(|argument| {
                            relational_expression_value(argument, &HashMap::new())?
                                .unwrap_or_default()
                                .parse::<usize>()
                                .map_err(|error| session_error("parse NOW precision", error))
                        })
                        .transpose()?
                        .unwrap_or(0);
                    format_unix_timestamp(statement_timestamp, precision)
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("unix_timestamp") && Args.len() <= 1 =>
                {
                    statement_timestamp.floor().to_string()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("version") && Args.is_empty() =>
                {
                    astersql_parser_mysql::r#const::ServerVersion()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("database") && Args.is_empty() =>
                {
                    let database = self.current_database();
                    if database.is_empty() {
                        CONCRETE_NULL_VALUE.to_owned()
                    } else {
                        database
                    }
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("last_insert_id")
                        && matches!(Args.len(), 0 | 1) =>
                {
                    if let Some(argument) = Args.first() {
                        let value = relational_expression_value(argument, &HashMap::new())?
                            .unwrap_or_default()
                            .parse::<u64>()
                            .map_err(|error| {
                                session_error("parse LAST_INSERT_ID argument", error)
                            })?;
                        self.state.borrow_mut().info_last_insert_id = value;
                        value.to_string()
                    } else {
                        self.state.borrow().info_last_insert_id.to_string()
                    }
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("found_rows") && Args.is_empty() =>
                {
                    self.state.borrow().info_found_rows.to_string()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("row_count") && Args.is_empty() =>
                {
                    self.state.borrow().info_row_count.to_string()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("current_user") && Args.is_empty() =>
                {
                    format!(
                        "{}@{}",
                        self.login_user.as_deref().unwrap_or("root"),
                        self.authenticated_host.as_deref().unwrap_or("%")
                    )
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("user") && Args.is_empty() =>
                {
                    format!(
                        "{}@{}",
                        self.login_user.as_deref().unwrap_or("root"),
                        self.login_host.as_deref().unwrap_or("localhost")
                    )
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_version") && Args.is_empty() =>
                {
                    "Release Version: None\nEdition: Community".to_owned()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("embed_text") =>
                {
                    self.execute_embed_text(Args, &HashMap::new())?
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_is_ddl_owner") && Args.is_empty() =>
                {
                    "1".to_owned()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("benchmark") && Args.len() == 2 =>
                {
                    let count = relational_expression_value(&Args[0], &HashMap::new())?
                        .unwrap_or_default()
                        .parse::<i64>()
                        .map_err(|error| session_error("parse BENCHMARK count", error))?;
                    if count > 0 {
                        for _ in 0..count {
                            let _ = relational_expression_value(&Args[1], &HashMap::new())?;
                        }
                    }
                    "0".to_owned()
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if matches!(
                        FnName.L.as_str(),
                        "get_lock" | "release_lock" | "release_all_locks"
                    ) =>
                {
                    self.execute_user_lock_function(&FnName.L, Args)?
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_decode_key") =>
                {
                    self.execute_tidb_decode_key(Args)?
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_encode_record_key") =>
                {
                    self.execute_tidb_encode_record_key(Args)?
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_encode_index_key") =>
                {
                    self.execute_tidb_encode_index_key(Args)?
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("tidb_mvcc_info") && Args.len() == 1 =>
                {
                    let key = match &Args[0].Kind {
                        ast::ExprKind::Function {
                            FnName,
                            Args: encode_args,
                            ..
                        } if FnName.L.eq_ignore_ascii_case("tidb_encode_index_key") => {
                            self.execute_tidb_encode_index_key(encode_args)?
                        }
                        _ => relational_expression_value(&Args[0], &HashMap::new())?
                            .unwrap_or_default(),
                    };
                    self.execute_tidb_mvcc_info(&key)
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("random_bytes") && Args.len() == 1 =>
                {
                    static RANDOM_BYTES_CALL: AtomicU64 = AtomicU64::new(1);
                    let length =
                        relational_expression_value(&Args[0], &HashMap::new())?.unwrap_or_default();
                    format!(
                        "{:0>width$}",
                        RANDOM_BYTES_CALL.fetch_add(1, Ordering::AcqRel),
                        width = length.parse::<usize>().unwrap_or(1)
                    )
                }
                ast::ExprKind::Function { FnName, Args, .. }
                    if FnName.L.eq_ignore_ascii_case("json_extract")
                        && Args.first().is_some_and(|argument| {
                            matches!(
                                &argument.Kind,
                                ast::ExprKind::Variable {
                                    Name,
                                    IsSystem: true,
                                    ..
                                } if Name.trim_start_matches('@').eq_ignore_ascii_case(
                                    "tidb_last_txn_info"
                                )
                            )
                        }) =>
                {
                    self.state.borrow().last_commit_ts.to_string()
                }
                _ => super::relational_value::relational_expression_value_with_embed(
                    expr,
                    &HashMap::new(),
                    &|args, row| self.execute_embed_text(args, row),
                )?
                .unwrap_or_else(|| CONCRETE_NULL_VALUE.to_owned()),
            };
            let column = if !field.AsName.O.is_empty() {
                field.AsName.O.clone()
            } else if let ast::ExprKind::Variable { Name, IsSystem, .. } = &expr.Kind {
                format!("{}{Name}", if *IsSystem { "@@" } else { "@" })
            } else if let ast::ExprKind::Function { FnName, .. } = &expr.Kind {
                format!("{}()", FnName.O.to_uppercase())
            } else {
                (index + 1).to_string()
            };
            columns.push(column);
            row.push(value);
        }
        Ok(Some(ConcreteRecordSet::new(columns, vec![row])))
    }

    /// Execute constant SELECT set operations through the AST. This preserves
    /// UNION's duplicate elimination while UNION ALL appends every row.
    pub(super) fn execute_constant_set_operation(
        &self,
        statement: &ast::SetOprStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        let mut columns = None;
        let mut rows = Vec::new();
        for (index, node) in statement.select_list.selects.iter().enumerate() {
            let select = node
                .as_any()
                .downcast_ref::<ast::SelectStmt>()
                .ok_or_else(|| {
                    SessionError::new("set operation currently requires SELECT branches")
                })?;
            let record_set = self.execute_constant_select(select)?.ok_or_else(|| {
                SessionError::new("set operation requires constant SELECT branches")
            })?;
            if columns.is_none() {
                columns = Some(record_set.columns.clone());
            }
            let operator = statement
                .select_list
                .operators
                .get(index)
                .copied()
                .flatten()
                .unwrap_or(ast::SetOprType::Union);
            if !matches!(operator, ast::SetOprType::Union | ast::SetOprType::UnionAll) {
                return Err(SessionError::new(
                    "INTERSECT/EXCEPT require the full set-operation executor",
                ));
            }
            for row in record_set.rows {
                if operator == ast::SetOprType::Union && rows.contains(&row) {
                    continue;
                }
                rows.push(row);
            }
        }
        Ok(ConcreteRecordSet::new(columns.unwrap_or_default(), rows))
    }
}
