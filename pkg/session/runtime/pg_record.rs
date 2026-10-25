// Copyright 2026 AsterSQL.
//! Anonymous PostgreSQL record types and values in the normal session runtime.
use super::*;
use ast::Node;
use astersql_parser_types::PostgreSQLRecordField as Field;

fn error(state: &'static str, message: impl Into<String>, position: Option<usize>) -> SessionError {
    // Constructor errors share the existing typed SQLSTATE/source-position bridge.
    let value = super::pg_array::PgArrayError {
        state,
        message: message.into(),
        position,
    };
    SessionError::with_source(value.to_string(), astersql_errors::SharedError::new(value))
}
fn scalar_type(expr: &ast::ExprNode, parameters: &HashMap<usize, u32>) -> SessionResult<Field> {
    let oid = match &expr.Kind {
        ast::ExprKind::ExplicitRow { .. } => {
            return Ok(Field {
                oid: 2249,
                fields: Some(record_type(expr, parameters)?),
            });
        }
        // Unknown fields remain unknown inside an anonymous record (record_send
        // uses OID705), unlike a top-level unknown SELECT projection.
        ast::ExprKind::Value(v)
            if matches!(v.Datum, ast::ValueDatum::Null | ast::ValueDatum::String(_)) =>
        {
            705
        }
        ast::ExprKind::ParamMarker { Offset } => {
            let oid = parameters.get(Offset).copied().unwrap_or(0);
            if oid == 0 {
                return Err(error(
                    "42P18",
                    "could not determine data type of parameter",
                    None,
                ));
            }
            match oid {
                16 | 20 | 23 | 25 | 1700 => oid,
                _ => {
                    return Err(error(
                        "0A000",
                        "record parameter type is not supported",
                        None,
                    ));
                }
            }
        }
        ast::ExprKind::Parentheses(inner) => return scalar_type(inner, parameters),
        _ => super::pg_array::element_type(expr, parameters)?
            .map(|element| element.oid())
            .ok_or_else(|| {
                error(
                    "0A000",
                    "record field expression type is not supported",
                    None,
                )
            })?,
    };
    Ok(Field { oid, fields: None })
}
pub(super) fn record_type(
    expr: &ast::ExprNode,
    parameters: &HashMap<usize, u32>,
) -> SessionResult<Vec<Field>> {
    let ast::ExprKind::ExplicitRow { Fields, .. } = &expr.Kind else {
        unreachable!()
    };
    Fields
        .iter()
        .map(|field| scalar_type(field, parameters))
        .collect()
}
pub(super) fn field_type(fields: Vec<Field>) -> astersql_parser_types::FieldType {
    // The legacy scalar code is only the shared transport carrier; record
    // identity, equality, serialization and wire metadata use this typed layout.
    let mut result =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarString);
    result.SetCharset("utf8mb4".into());
    result.SetCollate("utf8mb4_bin".into());
    result.SetPostgreSQLRecordFields(fields);
    result
}

pub(super) fn analyze(
    statements: &[Box<dyn ast::Node>],
    supplied: Option<&[u32]>,
) -> SessionResult<(HashMap<usize, Vec<Field>>, Vec<u32>)> {
    struct Markers {
        markers: Vec<(usize, Option<usize>)>,
        casts: HashMap<usize, u32>,
    }
    impl ast::Visitor for Markers {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                if let ast::ExprKind::ParamMarker { Offset } = expr.Kind {
                    self.markers
                        .push((Offset, expr.PostgreSQLParameterIndex().map(usize::from)));
                }
                if let ast::ExprKind::Cast {
                    Expr, FunctionType, ..
                } = &expr.Kind
                {
                    let oid = match FunctionType {
                        ast::CastFunctionType::PostgreSQLInt4 => 23,
                        ast::CastFunctionType::PostgreSQLInt8 => 20,
                        _ => 0,
                    };
                    let mut inner = Expr.as_ref();
                    while let ast::ExprKind::Parentheses(value) = &inner.Kind {
                        inner = value;
                    }
                    if let ast::ExprKind::ParamMarker { Offset } = inner.Kind {
                        if oid != 0 {
                            self.casts.insert(Offset, oid);
                        }
                    }
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut markers = Markers {
        markers: vec![],
        casts: HashMap::new(),
    };
    for stmt in statements {
        stmt.accept(&mut markers);
    }
    markers.markers.sort_unstable_by_key(|(offset, _)| *offset);
    let mut by_index = HashMap::new();
    for (i, (offset, index)) in markers.markers.iter().enumerate() {
        let index = index.unwrap_or(i + 1) - 1;
        if let Some(oid) = supplied
            .and_then(|types| types.get(index))
            .copied()
            .filter(|oid| *oid != 0)
            .or_else(|| markers.casts.get(offset).copied())
        {
            by_index.insert(index, oid);
        }
    }
    let parameters = markers
        .markers
        .iter()
        .enumerate()
        .map(|(i, (offset, index))| {
            (
                *offset,
                by_index
                    .get(&(index.unwrap_or(i + 1) - 1))
                    .copied()
                    .unwrap_or(0),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut types = HashMap::new();
    for statement in statements {
        // Register the original constructor positions, so a record in DML or
        // scalar operator context cannot acquire scalar-row behavior by fallback.
        let roots = statement
            .as_any()
            .downcast_ref::<ast::SelectStmt>()
            .into_iter()
            .flat_map(|select| select.Fields.Fields.iter())
            .filter_map(|field| field.Expr.as_ref())
            .filter(|expr| matches!(expr.Kind, ast::ExprKind::ExplicitRow { .. }))
            .collect::<Vec<_>>();
        struct Positions(std::collections::HashSet<usize>);
        impl ast::Visitor for Positions {
            fn enter(&mut self, node: &dyn ast::Node) -> bool {
                if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                    if matches!(expr.Kind, ast::ExprKind::ExplicitRow { .. }) {
                        self.0.insert(expr.OriginTextPosition as usize);
                    }
                }
                false
            }
            fn leave(&mut self, _: &dyn ast::Node) -> bool {
                true
            }
        }
        let mut all = Positions(Default::default());
        statement.accept(&mut all);
        let mut permitted = Positions(Default::default());
        for root in &roots {
            root.accept(&mut permitted);
        }
        if all.0 != permitted.0 {
            return Err(error(
                "0A000",
                "record analysis requires a direct SELECT constructor",
                None,
            ));
        }
        for root in roots {
            if supplied.is_none()
                && let Some((offset, _)) = markers.markers.first()
            {
                // Simple Query has no bound parameter namespace, even below CAST.
                return Err(error("42P02", "there is no parameter", Some(*offset)));
            }
            types.insert(
                root.OriginTextPosition as usize,
                record_type(root, &parameters)?,
            );
        }
    }
    let oids = markers
        .markers
        .iter()
        .map(|(offset, _)| parameters[offset])
        .collect();
    Ok((types, oids))
}

pub(super) fn value(
    expr: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
) -> SessionResult<Option<String>> {
    let ast::ExprKind::ExplicitRow { Fields, .. } = &expr.Kind else {
        unreachable!()
    };
    let mut values = Vec::with_capacity(Fields.len());
    for field in Fields {
        let value = if matches!(field.Kind, ast::ExprKind::ExplicitRow { .. }) {
            value(field, row)?
        } else {
            super::relational_value::relational_expression_value(field, row)?
        };
        values.push(match value {
            None => String::new(),
            Some(mut value) => {
                if scalar_type(field, &HashMap::new())?.oid == 16 {
                    value = match value.as_str() {
                        "1" | "t" | "true" => "t".into(),
                        "0" | "f" | "false" => "f".into(),
                        _ => return Err(error("22P02", "invalid boolean record field", None)),
                    };
                }
                if value.is_empty()
                    || value
                        .chars()
                        .any(|ch| ch.is_whitespace() || matches!(ch, '(' | ')' | ',' | '"' | '\\'))
                {
                    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\"\""))
                } else {
                    value
                }
            }
        });
    }
    Ok(Some(format!("({})", values.join(","))))
}
