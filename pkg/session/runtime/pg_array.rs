// Copyright 2026 AsterSQL.
//! Flat PostgreSQL ARRAY analysis and values in the canonical session runtime.
use super::*;
use astersql_parser_types::PostgreSQLArrayElement as Element;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgArrayError {
    pub state: &'static str,
    pub message: String,
}
impl std::fmt::Display for PgArrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PgArrayError {}
fn error(state: &'static str, message: impl Into<String>) -> SessionError {
    let error = PgArrayError {
        state,
        message: message.into(),
    };
    SessionError::with_source(error.to_string(), astersql_errors::SharedError::new(error))
}

pub(super) fn element_type(
    expr: &ast::ExprNode,
    parameters: &HashMap<usize, u32>,
) -> SessionResult<Option<Element>> {
    Ok(match &expr.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Null | ast::ValueDatum::String(_) => None,
            ast::ValueDatum::Int64(n) => Some(if i32::try_from(*n).is_ok() {
                Element::Int4
            } else {
                Element::Int8
            }),
            ast::ValueDatum::Uint64(n) if i64::try_from(*n).is_ok() => {
                Some(if i32::try_from(*n).is_ok() {
                    Element::Int4
                } else {
                    Element::Int8
                })
            }
            _ => return Err(error("0A000", "ARRAY element type is not supported")),
        },
        ast::ExprKind::ParamMarker { Offset } => match parameters.get(Offset).copied().unwrap_or(0)
        {
            0 => None,
            23 => Some(Element::Int4),
            20 => Some(Element::Int8),
            25 => Some(Element::Text),
            _ => return Err(error("0A000", "ARRAY parameter type is not supported")),
        },
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLInt4,
            ..
        } => Some(Element::Int4),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLInt8,
            ..
        } => Some(Element::Int8),
        ast::ExprKind::Parentheses(inner) => element_type(inner, parameters)?,
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => element_type(V, parameters)?,
        ast::ExprKind::Array { .. } => {
            return Err(error("0A000", "nested ARRAY analysis is not supported"));
        }
        _ => {
            return Err(error(
                "0A000",
                "ARRAY element expression analysis is not supported",
            ));
        }
    })
}
pub(super) fn array_type(
    expr: &ast::ExprNode,
    parameters: &HashMap<usize, u32>,
) -> SessionResult<Element> {
    let ast::ExprKind::Array { Elements, .. } = &expr.Kind else {
        unreachable!()
    };
    if Elements.is_empty() {
        return Err(error("42P18", "cannot determine type of empty array"));
    }
    let mut result = None;
    for value in Elements {
        let next = element_type(value, parameters)?;
        result = match (result, next) {
            (None, next) => next,
            (previous, None) => previous,
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(Element::Int4 | Element::Int8), Some(Element::Int4 | Element::Int8)) => {
                Some(Element::Int8)
            }
            _ => return Err(error("42804", "ARRAY types cannot be matched")),
        };
    }
    Ok(result.unwrap_or(Element::Text))
}
pub(super) fn field_type(element: Element) -> astersql_parser_types::FieldType {
    let mut result =
        astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeVarString);
    result.SetCharset("utf8mb4".into());
    result.SetCollate("utf8mb4_bin".into());
    result.SetPostgreSQLArrayElement(element);
    result
}

/// Resolve every constructor before Prepare/Execute can fall back to scalar text.
/// Offsets refer to the canonical parser's SQL, after indexed-marker mapping.
pub(super) fn analyze(
    _sql: &str,
    statements: &[Box<dyn ast::Node>],
    supplied: Option<&[u32]>,
) -> SessionResult<(HashMap<usize, Element>, Vec<u32>)> {
    // This increment supplies direct flat SELECT constructors. Other expression
    // contexts require their own type constraints, rather than scalar fallbacks.
    let allowed = statements
        .iter()
        .filter_map(|node| node.as_any().downcast_ref::<ast::SelectStmt>())
        .flat_map(|select| select.Fields.Fields.iter())
        .filter_map(|field| field.Expr.as_ref())
        .filter(|expr| matches!(expr.Kind, ast::ExprKind::Array { .. }))
        .map(|expr| expr.OriginTextPosition as usize)
        .collect::<std::collections::HashSet<_>>();
    struct Markers(Vec<usize>);
    impl ast::Visitor for Markers {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(ast::ExprNode {
                Kind: ast::ExprKind::ParamMarker { Offset },
                ..
            }) = node.as_any().downcast_ref::<ast::ExprNode>()
            {
                self.0.push(*Offset);
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut markers = Markers(Vec::new());
    for statement in statements {
        statement.accept(&mut markers);
    }
    markers.0.sort_unstable();
    let markers = markers.0;
    let parameters = markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            (
                *m,
                supplied
                    .and_then(|types| types.get(i))
                    .copied()
                    .unwrap_or(0),
            )
        })
        .collect::<HashMap<_, _>>();
    struct Arrays<'a> {
        allowed: &'a std::collections::HashSet<usize>,
        parameters: &'a HashMap<usize, u32>,
        supplied: bool,
        types: HashMap<usize, Element>,
        constraints: HashMap<usize, u32>,
        failure: Option<SessionError>,
    }
    impl ast::Visitor for Arrays<'_> {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if self.failure.is_some() {
                return true;
            }
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                if let ast::ExprKind::Array { Elements, .. } = &expr.Kind {
                    if !self.allowed.contains(&(expr.OriginTextPosition as usize)) {
                        self.failure = Some(error(
                            "0A000",
                            "ARRAY analysis requires a direct SELECT constructor",
                        ));
                        return true;
                    }

                    if !self.supplied {
                        for value in Elements {
                            if let ast::ExprKind::ParamMarker { .. } = value.Kind {
                                self.failure = Some(error(
                                    "42P02",
                                    format!(
                                        "there is no parameter ${}",
                                        value.PostgreSQLParameterIndex().unwrap_or(1)
                                    ),
                                ));
                                return true;
                            }
                        }
                    }
                    match array_type(expr, self.parameters) {
                        Ok(element) => {
                            self.types.insert(expr.OriginTextPosition as usize, element);
                            for value in Elements {
                                if let ast::ExprKind::ParamMarker { Offset } = value.Kind {
                                    self.constraints.insert(Offset, element.oid());
                                }
                            }
                        }
                        Err(error) => self.failure = Some(error),
                    }
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut visitor = Arrays {
        allowed: &allowed,
        parameters: &parameters,
        supplied: supplied.is_some(),
        types: HashMap::new(),
        constraints: HashMap::new(),
        failure: None,
    };
    for statement in statements {
        statement.accept(&mut visitor);
    }
    if let Some(error) = visitor.failure {
        return Err(error);
    }
    let oids = markers
        .iter()
        .enumerate()
        .map(|(i, m)| {
            supplied
                .and_then(|types| types.get(i))
                .copied()
                .filter(|oid| *oid != 0)
                .or_else(|| visitor.constraints.get(m).copied())
                .unwrap_or(0)
        })
        .collect();
    Ok((visitor.types, oids))
}

/// SQL array text from evaluated elements, preserving element NULL and escaping.
pub(super) fn value(
    expr: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
) -> SessionResult<Option<String>> {
    let element = array_type(expr, &HashMap::new())?;
    let ast::ExprKind::Array { Elements, .. } = &expr.Kind else {
        unreachable!()
    };
    let mut values = Vec::new();
    for expr in Elements {
        let value = super::relational_value::relational_expression_value(expr, row)?;
        values.push(match value {
            None => "NULL".to_owned(),
            Some(value) => match element {
                Element::Int4 => value
                    .parse::<i32>()
                    .map_err(|_| error("22P02", "invalid input syntax for type integer"))?
                    .to_string(),
                Element::Int8 => value
                    .parse::<i64>()
                    .map_err(|_| error("22P02", "invalid input syntax for type bigint"))?
                    .to_string(),
                Element::Text => {
                    if value.is_empty()
                        || value.eq_ignore_ascii_case("NULL")
                        || value
                            .chars()
                            .any(|c| c.is_whitespace() || matches!(c, ',' | '{' | '}' | '"' | '\\'))
                    {
                        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
                    } else {
                        value
                    }
                }
            },
        });
    }
    Ok(Some(format!("{{{}}}", values.join(","))))
}
