// Copyright 2026 AsterSQL.
//! PostgreSQL ARRAY analysis and values in the canonical session runtime.
use super::*;
use astersql_parser_ast::Node;
use astersql_parser_types::PostgreSQLArrayElement as Element;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgArrayError {
    pub state: &'static str,
    pub message: String,
    /// Zero-based byte offset in the original SQL, when the semantic error is tied to a value.
    pub position: Option<usize>,
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
        position: None,
    };
    SessionError::with_source(error.to_string(), astersql_errors::SharedError::new(error))
}
fn error_at(state: &'static str, message: impl Into<String>, position: usize) -> SessionError {
    let error = PgArrayError {
        state,
        message: message.into(),
        position: Some(position),
    };
    SessionError::with_source(error.to_string(), astersql_errors::SharedError::new(error))
}
fn element_name(element: Element) -> &'static str {
    match element {
        Element::Int4 => "integer",
        Element::Int8 => "bigint",
        Element::Numeric => "numeric",
        Element::Bool => "boolean",
        Element::Text => "text",
    }
}

pub(super) fn element_type(
    expr: &ast::ExprNode,
    parameters: &HashMap<usize, u32>,
) -> SessionResult<Option<Element>> {
    Ok(match &expr.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Null | ast::ValueDatum::String(_) => None,
            ast::ValueDatum::Bool(_) => Some(Element::Bool),
            ast::ValueDatum::Decimal(_) => Some(Element::Numeric),
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
            16 => Some(Element::Bool),
            1700 => Some(Element::Numeric),
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
            return Err(error("42804", "multidimensional ARRAY type mismatch"));
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
    if Elements
        .iter()
        .any(|value| matches!(value.Kind, ast::ExprKind::Array { .. }))
        && Elements
            .iter()
            .any(|value| !matches!(value.Kind, ast::ExprKind::Array { .. }))
    {
        let position = Elements
            .iter()
            .find(|value| !matches!(value.Kind, ast::ExprKind::Array { .. }))
            .map(|value| value.OriginTextPosition as usize)
            .unwrap_or(expr.OriginTextPosition as usize);
        return Err(error_at("42804", "ARRAY types cannot be matched", position));
    }
    fn leaves<'a>(
        expr: &'a ast::ExprNode,
        parameters: &HashMap<usize, u32>,
        values: &mut Vec<(Option<Element>, usize)>,
    ) -> SessionResult<()> {
        if let ast::ExprKind::Array { Elements, .. } = &expr.Kind {
            for value in Elements {
                leaves(value, parameters, values)?;
            }
        } else {
            values.push((
                element_type(expr, parameters)?,
                expr.OriginTextPosition as usize,
            ));
        }
        Ok(())
    }
    let mut values = Vec::new();
    leaves(expr, parameters, &mut values)?;
    let mut result = None;
    for (next, position) in values {
        result = match (result, next) {
            (None, next) => next,
            (previous, None) => previous,
            (Some(a), Some(b)) if a == b => Some(a),
            (Some(Element::Int4 | Element::Int8), Some(Element::Int4 | Element::Int8)) => {
                Some(Element::Int8)
            }
            (
                Some(Element::Int4 | Element::Int8 | Element::Numeric),
                Some(Element::Numeric | Element::Int4 | Element::Int8),
            ) => Some(Element::Numeric),
            (Some(a), Some(b)) => {
                return Err(error_at(
                    "42804",
                    format!(
                        "ARRAY types {} and {} cannot be matched",
                        element_name(a),
                        element_name(b)
                    ),
                    position,
                ));
            }
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
    // This increment supplies constructors beneath directly projected SELECT
    // roots. Other expression contexts need their own type constraints.
    let roots = statements
        .iter()
        .filter_map(|node| node.as_any().downcast_ref::<ast::SelectStmt>())
        .flat_map(|select| select.Fields.Fields.iter())
        .filter_map(|field| field.Expr.as_ref())
        .filter(|expr| matches!(expr.Kind, ast::ExprKind::Array { .. }))
        .collect::<Vec<_>>();
    struct ArrayPositions(std::collections::HashSet<usize>);
    impl ast::Visitor for ArrayPositions {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(ast::ExprNode {
                Kind: ast::ExprKind::Array { .. },
                OriginTextPosition,
                ..
            }) = node.as_any().downcast_ref::<ast::ExprNode>()
            {
                self.0.insert(*OriginTextPosition as usize);
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut allowed = std::collections::HashSet::new();
    for root in roots {
        let mut positions = ArrayPositions(std::collections::HashSet::new());
        root.accept(&mut positions);
        allowed.extend(positions.0);
    }
    struct Markers(Vec<(usize, Option<usize>)>);
    impl ast::Visitor for Markers {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<ast::ExprNode>() {
                if let ast::ExprKind::ParamMarker { Offset } = expr.Kind {
                    self.0
                        .push((Offset, expr.PostgreSQLParameterIndex().map(usize::from)));
                }
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
    markers.0.sort_unstable_by_key(|(offset, _)| *offset);
    let markers = markers.0;
    let parameters = markers
        .iter()
        .enumerate()
        .map(|(i, (offset, parameter_index))| {
            (
                *offset,
                supplied
                    .and_then(|types| {
                        types.get(
                            parameter_index
                                .and_then(|index| index.checked_sub(1))
                                .unwrap_or(i),
                        )
                    })
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
                            struct ArrayParameters(Vec<usize>);
                            impl ast::Visitor for ArrayParameters {
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
                            let mut params = ArrayParameters(Vec::new());
                            expr.accept(&mut params);
                            for offset in params.0 {
                                self.constraints.insert(offset, element.oid());
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
        .map(|(i, (offset, parameter_index))| {
            supplied
                .and_then(|types| {
                    types.get(
                        parameter_index
                            .and_then(|index| index.checked_sub(1))
                            .unwrap_or(i),
                    )
                })
                .copied()
                .filter(|oid| *oid != 0)
                .or_else(|| visitor.constraints.get(offset).copied())
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
    fn render(
        expr: &ast::ExprNode,
        row: &HashMap<String, Option<String>>,
        element: Element,
    ) -> SessionResult<(String, Vec<usize>)> {
        let ast::ExprKind::Array { Elements, .. } = &expr.Kind else {
            unreachable!()
        };
        let has_arrays = Elements
            .iter()
            .any(|value| matches!(value.Kind, ast::ExprKind::Array { .. }));
        let has_scalars = Elements
            .iter()
            .any(|value| !matches!(value.Kind, ast::ExprKind::Array { .. }));
        if has_arrays && has_scalars {
            return Err(error(
                "2202E",
                "multidimensional arrays must have array expressions with matching dimensions",
            ));
        }
        let mut values = Vec::with_capacity(Elements.len());
        let mut child_shape: Option<Vec<usize>> = None;
        for child in Elements {
            if has_arrays {
                let (value, shape) = render(child, row, element)?;
                if child_shape
                    .as_ref()
                    .is_some_and(|expected| expected != &shape)
                {
                    return Err(error(
                        "2202E",
                        "multidimensional arrays must have array expressions with matching dimensions",
                    ));
                }
                child_shape = Some(shape);
                values.push(value);
                continue;
            }
            let value = super::relational_value::relational_expression_value(child, row)?;
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
                    Element::Numeric => value,
                    Element::Bool => match value.to_ascii_lowercase().as_str() {
                        "1" | "true" | "t" => "t".to_owned(),
                        "0" | "false" | "f" => "f".to_owned(),
                        _ => return Err(error("22P02", "invalid input syntax for type boolean")),
                    },
                    Element::Text => {
                        if value.is_empty()
                            || value.eq_ignore_ascii_case("NULL")
                            || value.chars().any(|c| {
                                c.is_whitespace() || matches!(c, ',' | '{' | '}' | '"' | '\\')
                            })
                        {
                            format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
                        } else {
                            value
                        }
                    }
                },
            });
        }
        let mut shape = vec![Elements.len()];
        if let Some(child) = child_shape {
            shape.extend(child);
        }
        Ok((format!("{{{}}}", values.join(",")), shape))
    }
    render(expr, row, element).map(|(value, _)| Some(value))
}
