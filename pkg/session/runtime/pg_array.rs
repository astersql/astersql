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
        Element::Date => "date",
        Element::Time => "time",
        Element::Int2 => "smallint",
        Element::Int4 => "integer",
        Element::Int8 => "bigint",
        Element::Numeric => "numeric",
        Element::Float4 => "real",
        Element::Float8 => "double precision",
        Element::Bool => "boolean",
        Element::Text => "text",
        Element::Varchar => "character varying",
        Element::Bpchar => "character",
        Element::Bytea => "bytea",
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
            1082 => Some(Element::Date),
            1083 => Some(Element::Time),
            21 => Some(Element::Int2),
            23 => Some(Element::Int4),
            20 => Some(Element::Int8),
            16 => Some(Element::Bool),
            1700 => Some(Element::Numeric),
            700 => Some(Element::Float4),
            701 => Some(Element::Float8),
            25 => Some(Element::Text),
            1043 => Some(Element::Varchar),
            1042 => Some(Element::Bpchar),
            17 => Some(Element::Bytea),
            _ => return Err(error("0A000", "ARRAY parameter type is not supported")),
        },
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLFloat4,
            ..
        } => Some(Element::Float4),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLFloat8,
            ..
        } => Some(Element::Float8),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLNumeric,
            ..
        } => Some(Element::Numeric),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLInt2,
            ..
        } => Some(Element::Int2),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLInt4,
            ..
        } => Some(Element::Int4),
        ast::ExprKind::Cast {
            FunctionType: ast::CastFunctionType::PostgreSQLInt8,
            ..
        } => Some(Element::Int8),
        // The existing OID17 binder emits a binary literal cast. Retain that
        // typed identity for ARRAY elements, including the all-NULL rebind.
        ast::ExprKind::Cast { Expr, Tp, .. }
            if Tp.GetCharset() == "binary"
                && matches!(
                    Tp.GetType(),
                    astersql_parser_mysql::r#type::TypeString
                        | astersql_parser_mysql::r#type::TypeVarString
                )
                && (super::relational_value::binary_cast_literal_value(Expr, Tp).is_some()
                    || matches!(&Expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::Null))) =>
        {
            Some(Element::Bytea)
        }
        ast::ExprKind::Cast { Tp, .. }
            if Tp.GetFlag() & astersql_parser_mysql::r#type::IsBooleanFlag != 0 =>
        {
            Some(Element::Bool)
        }
        ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLTextOid() == Some(1043) => {
            Some(Element::Varchar)
        }
        ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLTextOid() == Some(1042) => {
            Some(Element::Bpchar)
        }
        ast::ExprKind::Cast { Tp, .. }
            if Tp.PostgreSQLArrayElement().is_none()
                && Tp.GetType() == astersql_parser_mysql::r#type::TypeDate =>
        {
            Some(Element::Date)
        }
        ast::ExprKind::Cast { Tp, .. }
            if Tp.PostgreSQLArrayElement().is_none()
                && Tp.GetType() == astersql_parser_mysql::r#type::TypeDuration =>
        {
            Some(Element::Time)
        }
        ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLTextOid() == Some(25) => Some(Element::Text),
        ast::ExprKind::Parentheses(inner) => element_type(inner, parameters)?,
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => {
            // The positive token of i64::MIN is Uint64; use the scalar rule
            // on the signed expression before looking at that token alone.
            match super::query::pg_integer_literal_type(expr).map(|tp| tp.GetType()) {
                Some(astersql_parser_mysql::r#type::TypeLonglong) => Some(Element::Int8),
                _ => element_type(V, parameters)?,
            }
        }
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
            (
                Some(Element::Int2 | Element::Int4 | Element::Int8),
                Some(Element::Int2 | Element::Int4 | Element::Int8),
            ) => Some(
                if result == Some(Element::Int8) || next == Some(Element::Int8) {
                    Element::Int8
                } else {
                    Element::Int4
                },
            ),
            (
                Some(Element::Int2 | Element::Int4 | Element::Int8 | Element::Numeric),
                Some(Element::Numeric | Element::Int2 | Element::Int4 | Element::Int8),
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

/// A bpchar constructor keeps a modifier only when every leaf declares the same length.
pub(super) fn constructor_field_type(
    expr: &ast::ExprNode,
    element: Element,
) -> astersql_parser_types::FieldType {
    fn length(expr: &ast::ExprNode) -> isize {
        match &expr.Kind {
            ast::ExprKind::Parentheses(inner) => length(inner),
            ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLTextOid() == Some(1042) => Tp.GetFlen(),
            ast::ExprKind::Array { Elements, .. } => {
                let Some(first) = Elements.first().map(length) else {
                    return -1;
                };
                if Elements.iter().all(|expr| length(expr) == first) {
                    first
                } else {
                    -1
                }
            }
            _ => -1,
        }
    }
    let mut tp = field_type(element);
    if element == Element::Bpchar {
        tp.SetFlen(length(expr));
    }
    tp
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
        .filter(|expr| {
            matches!(expr.Kind, ast::ExprKind::Array { .. })
                || matches!(&expr.Kind, ast::ExprKind::Cast { Tp, .. }
                    if matches!(Tp.PostgreSQLArrayElement(), Some(Element::Int2 | Element::Int4 | Element::Int8 | Element::Bool | Element::Text | Element::Varchar | Element::Bpchar | Element::Bytea | Element::Numeric | Element::Float4 | Element::Float8 | Element::Date | Element::Time)))
        })
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
    struct Contexts(HashMap<usize, Element>);
    impl ast::Visitor for Contexts {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(ast::ExprNode {
                Kind: ast::ExprKind::Cast { Expr, Tp, .. },
                ..
            }) = node.as_any().downcast_ref::<ast::ExprNode>()
            {
                if let Some(
                    element @ (Element::Int2
                    | Element::Int4
                    | Element::Int8
                    | Element::Bool
                    | Element::Text
                    | Element::Varchar
                    | Element::Bpchar
                    | Element::Bytea
                    | Element::Numeric
                    | Element::Float4
                    | Element::Float8
                    | Element::Date
                    | Element::Time),
                ) = Tp.PostgreSQLArrayElement()
                {
                    let mut positions = ArrayPositions(std::collections::HashSet::new());
                    Expr.accept(&mut positions);
                    self.0
                        .extend(positions.0.into_iter().map(|offset| (offset, element)));
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut contexts = Contexts(HashMap::new());
    for statement in statements {
        statement.accept(&mut contexts);
    }
    struct Arrays<'a> {
        allowed: &'a std::collections::HashSet<usize>,
        parameters: &'a HashMap<usize, u32>,
        contexts: &'a HashMap<usize, Element>,
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
                    match self
                        .contexts
                        .get(&(expr.OriginTextPosition as usize))
                        .copied()
                        .map(Ok)
                        .unwrap_or_else(|| array_type(expr, self.parameters))
                    {
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
        contexts: &contexts.0,
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
    value_as(expr, row, array_type(expr, &HashMap::new())?, None)
}
fn value_as(
    expr: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    element: Element,
    context: Option<astersql_types::pg_temporal::PgDateContext>,
) -> SessionResult<Option<String>> {
    if matches!(element, Element::Float4 | Element::Float8) {
        use astersql_types::pg_array::{PgArray, PgArrayElement};
        fn collect(
            expr: &ast::ExprNode,
            row: &HashMap<String, Option<String>>,
            values: &mut Vec<Option<PgArrayElement>>,
            element: Element,
        ) -> SessionResult<Vec<i32>> {
            if let ast::ExprKind::Array { Elements, .. } = &expr.Kind {
                let nested = Elements
                    .iter()
                    .any(|v| matches!(v.Kind, ast::ExprKind::Array { .. }));
                let mut child_shape = None;
                for child in Elements {
                    if nested != matches!(child.Kind, ast::ExprKind::Array { .. }) {
                        return Err(error(
                            "2202E",
                            "multidimensional arrays must have matching dimensions",
                        ));
                    }
                    let shape = collect(child, row, values, element)?;
                    if child_shape.as_ref().is_some_and(|s| s != &shape) {
                        return Err(error(
                            "2202E",
                            "multidimensional arrays must have matching dimensions",
                        ));
                    }
                    child_shape = Some(shape);
                }
                let mut shape = vec![
                    i32::try_from(Elements.len()).map_err(|_| error("54000", "array too large"))?,
                ];
                if let Some(child) = child_shape {
                    shape.extend(child);
                }
                return Ok(shape);
            }
            let value = super::relational_value::relational_expression_value(expr, row)?;
            let value = value
                .map(|text| {
                    if element_type(expr, &HashMap::new())? == Some(element) {
                        if let Some(bytes) = super::row_codec::binary_runtime_bytes(&text) {
                            return match element {
                                Element::Float4 => {
                                    astersql_types::pg_array::float4_binary_input(&bytes)
                                        .map(PgArrayElement::Float4)
                                }
                                Element::Float8 => {
                                    astersql_types::pg_array::float8_binary_input(&bytes)
                                        .map(PgArrayElement::Float8)
                                }
                                _ => unreachable!(),
                            }
                            .map_err(|(s, m)| error(s, m));
                        }
                    }
                    match element {
                        Element::Float4 => astersql_expression::pg_cast_float4(Some(&text))
                            .map(|v| PgArrayElement::Float4(v.unwrap().to_bits()))
                            .map_err(|e| error(e.sqlstate(), e.to_string())),
                        Element::Float8 => astersql_expression::pg_cast_float8(Some(&text))
                            .map(|v| PgArrayElement::Float8(v.unwrap().to_bits()))
                            .map_err(|e| error(e.sqlstate(), e.to_string())),
                        _ => unreachable!(),
                    }
                })
                .transpose()?;
            values.push(value);
            Ok(vec![])
        }
        let mut values = vec![];
        let shape = collect(expr, row, &mut values, element)?;
        let shape = if shape.contains(&0) { vec![] } else { shape };
        let lower = vec![1; shape.len()];
        let array = PgArray::new(element.oid(), -1, shape, lower, values)
            .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))?;
        return runtime_array(&array).map(Some);
    }
    fn render(
        expr: &ast::ExprNode,
        row: &HashMap<String, Option<String>>,
        element: Element,
        context: Option<astersql_types::pg_temporal::PgDateContext>,
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
                let (value, shape) = render(child, row, element, context)?;
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
            // Parse has frozen scalar date casts. Keep their full calendar
            // domain out of the shared MySQL temporal evaluator.
            let source = if matches!(element, Element::Date | Element::Time)
                && let ast::ExprKind::Cast { Expr, Tp, .. } = &child.Kind
                && matches!(
                    Tp.GetType(),
                    astersql_parser_mysql::r#type::TypeDate
                        | astersql_parser_mysql::r#type::TypeDuration
                ) {
                Expr.as_ref()
            } else {
                child
            };
            let value = super::relational_value::relational_expression_value(source, row)?;
            values.push(match value {
                None => "NULL".to_owned(),
                Some(value) => match element {
                    Element::Int2 => astersql_expression::pg_cast_int2(Some(&value))
                        .map_err(|e| error(e.sqlstate(), e.to_string()))?
                        .unwrap()
                        .to_string(),
                    Element::Int4 => astersql_expression::pg_cast_int4(Some(&value))
                        .map_err(|e| error(e.sqlstate(), e.to_string()))?
                        .unwrap()
                        .to_string(),
                    Element::Int8 => astersql_expression::pg_cast_int8(Some(&value))
                        .map_err(|e| error(e.sqlstate(), e.to_string()))?
                        .unwrap()
                        .to_string(),
                    Element::Float4 | Element::Float8 => {
                        unreachable!("floating-point arrays use canonical bit construction")
                    }
                    Element::Time => {
                        let precision = if let ast::ExprKind::Cast { Tp, .. } = &child.Kind {
                            Tp.GetDecimal()
                        } else {
                            -1
                        };
                        value
                            .parse::<astersql_types::pg_temporal::PgTime>()
                            .map_err(|e| error(e.sqlstate(), e.to_string()))?
                            .round_precision(precision)
                            .to_string()
                    }
                    Element::Date => {
                        let date = context
                            .map_or_else(
                                || value.parse::<astersql_types::pg_temporal::PgDate>(),
                                |context| {
                                    astersql_types::pg_temporal::PgDate::parse_with_context(
                                        &value, context,
                                    )
                                },
                            )
                            .map_err(|e| error(e.sqlstate(), e.to_string()))?;
                        format!("\"{date}\"")
                    }
                    Element::Numeric => value,
                    Element::Bool => {
                        let value = astersql_expression::pg_boolean_cast(
                            Some(&value),
                            astersql_expression::PgBooleanCast::TextToBool,
                        )
                        .map_err(|e| error(e.sqlstate(), e.to_string()))?
                        .unwrap();
                        if value == "1" {
                            "t".to_owned()
                        } else {
                            "f".to_owned()
                        }
                    }
                    Element::Bytea => {
                        let mut source = child;
                        while let ast::ExprKind::Parentheses(inner) = &source.Kind {
                            source = inner;
                        }
                        let binary = match &source.Kind {
                            ast::ExprKind::Cast { Expr, Tp, .. } => {
                                super::relational_value::binary_cast_literal_value(Expr, Tp)
                                    .and_then(|value| {
                                        super::row_codec::binary_runtime_bytes(&value)
                                    })
                            }
                            _ => None,
                        };
                        let bytes = match binary {
                            Some(bytes) => bytes,
                            None => astersql_types::pg_array::bytea_text_input(value.as_bytes())
                                .map_err(|e| error(e.0, e.1))?,
                        };
                        let text = astersql_types::pg_array::bytea_text_output(&bytes);
                        format!("\"{}\"", text.replace('\\', "\\\\"))
                    }
                    Element::Text | Element::Varchar | Element::Bpchar => {
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
    let (value, _) = render(expr, row, element, context)?;
    if matches!(
        element,
        Element::Int2
            | Element::Int4
            | Element::Int8
            | Element::Bool
            | Element::Text
            | Element::Varchar
            | Element::Bpchar
            | Element::Bytea
            | Element::Numeric
            | Element::Float4
            | Element::Float8
            | Element::Date
            | Element::Time
    ) {
        typed_array(value.as_bytes(), element)?
            .to_text(b',')
            .map(Some)
            .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))
    } else {
        Ok(Some(value))
    }
}

/// Lossless text bridge for the shared runtime; identity stays in FieldType.
pub(super) fn int4_text(bytes: &[u8]) -> SessionResult<String> {
    typed_array(bytes, Element::Int4)?
        .to_text(b',')
        .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))
}

fn typed_array(bytes: &[u8], element: Element) -> SessionResult<astersql_types::pg_array::PgArray> {
    typed_array_with_modifier(bytes, element, -1, -1)
}
fn typed_array_with_modifier(
    bytes: &[u8],
    element: Element,
    precision: isize,
    scale: isize,
) -> SessionResult<astersql_types::pg_array::PgArray> {
    use astersql_types::pg_array::{PgArray, PgArrayElement, PgArrayTextError};
    if element == Element::Numeric {
        astersql_types::pg_numeric::PgNumeric::validate_modifier(precision, scale)
            .map_err(|e| error(e.sqlstate(), e.to_string()))?;
    }
    if matches!(element, Element::Varchar | Element::Bpchar) {
        astersql_expression::pg_varchar_length(precision)
            .map_err(|e| error(e.sqlstate(), e.to_string()))?;
    }
    let typmod = if element == Element::Time {
        scale as i32
    } else if matches!(element, Element::Varchar | Element::Bpchar) && precision >= 0 {
        precision as i32 + 4
    } else if element == Element::Numeric && precision >= 0 {
        ((precision as i32) << 16 | (scale as i32 & 0x7ff)) + 4
    } else {
        -1
    };
    PgArray::from_text_with(
        bytes,
        element.oid(),
        typmod,
        b',',
        |text, _| match element {
            Element::Float4 => astersql_expression::pg_cast_float4(Some(text))
                .map(|v| PgArrayElement::Float4(v.unwrap().to_bits()))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Float8 => astersql_expression::pg_cast_float8(Some(text))
                .map(|v| PgArrayElement::Float8(v.unwrap().to_bits()))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Time => text
                .parse::<astersql_types::pg_temporal::PgTime>()
                .map(|v| PgArrayElement::Time(v.round_precision(scale)))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Date => text
                .parse::<astersql_types::pg_temporal::PgDate>()
                .map(PgArrayElement::Date)
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Numeric => text
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .and_then(|v| v.apply_modifier(precision, scale))
                .map(PgArrayElement::Numeric)
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Int2 => astersql_expression::pg_cast_int2(Some(text))
                .map(|v| PgArrayElement::Int2(v.unwrap()))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Int4 => astersql_expression::pg_cast_int4(Some(text))
                .map(|v| PgArrayElement::Int4(v.unwrap()))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Int8 => astersql_expression::pg_cast_int8(Some(text))
                .map(|v| PgArrayElement::Int8(v.unwrap()))
                .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Bytea => astersql_types::pg_array::bytea_text_input(text.as_bytes())
                .map(PgArrayElement::Bytea)
                .map_err(|e| error(e.0, e.1)),
            Element::Bool => astersql_expression::pg_boolean_cast(
                Some(text),
                astersql_expression::PgBooleanCast::TextToBool,
            )
            .map(|v| PgArrayElement::Bool(v.unwrap() == "1"))
            .map_err(|e| error(e.sqlstate(), e.to_string())),
            Element::Text | Element::Varchar | Element::Bpchar => {
                astersql_expression::pg_text_cast(
                    Some(text),
                    false,
                    element.oid(),
                    if matches!(element, Element::Varchar | Element::Bpchar) {
                        precision
                    } else {
                        -1
                    },
                    true,
                )
                .map(|v| PgArrayElement::Text(v.unwrap()))
                .map_err(|e| error(e.sqlstate(), e.to_string()))
            }
        },
    )
    .map_err(|failure| match &failure {
        PgArrayTextError::Element(e) => e.clone(),
        _ => error(failure.sqlstate().unwrap_or("XX000"), failure.to_string()),
    })
}

/// Explicit integer and text array inputs; cross-family conversions remain gated.
pub(super) fn validate_cast(
    expr: &ast::ExprNode,
    target: &astersql_parser_types::FieldType,
) -> SessionResult<()> {
    if matches!(
        target.PostgreSQLArrayElement(),
        Some(Element::Varchar | Element::Bpchar)
    ) {
        astersql_expression::pg_varchar_length(target.GetFlen())
            .map_err(|e| error(e.sqlstate(), e.to_string()))?;
    }
    if target.PostgreSQLArrayElement() == Some(Element::Numeric) {
        astersql_types::pg_numeric::PgNumeric::validate_modifier(
            target.GetFlen(),
            target.GetDecimal(),
        )
        .map_err(|e| error(e.sqlstate(), e.to_string()))?;
    }
    if target.PostgreSQLArrayElement() == Some(Element::Float4)
        && matches!(&expr.Kind, ast::ExprKind::Value(v) if v.Type.PostgreSQLArrayElement() == Some(Element::Float4) && matches!(v.Datum, ast::ValueDatum::Bytes(_)))
    {
        return Ok(());
    }
    if target.PostgreSQLArrayElement() == Some(Element::Float8)
        && matches!(&expr.Kind, ast::ExprKind::Value(v) if v.Type.PostgreSQLArrayElement() == Some(Element::Float8) && matches!(v.Datum, ast::ValueDatum::Bytes(_)))
    {
        return Ok(());
    }
    if !matches!(
        target.PostgreSQLArrayElement(),
        Some(
            Element::Int2
                | Element::Int4
                | Element::Int8
                | Element::Bool
                | Element::Text
                | Element::Varchar
                | Element::Bpchar
                | Element::Bytea
                | Element::Numeric
                | Element::Float4
                | Element::Float8
                | Element::Date
                | Element::Time
        )
    ) {
        return Ok(());
    }
    fn supported(expr: &ast::ExprNode, target: Element) -> bool {
        match &expr.Kind {
            ast::ExprKind::Parentheses(inner) => supported(inner, target),
            ast::ExprKind::Value(v) => matches!(v.Datum, ast::ValueDatum::Null | ast::ValueDatum::String(_)),
            ast::ExprKind::ParamMarker { .. } => true,
            ast::ExprKind::Array { Elements, .. } => Elements.iter().all(|child| {
                matches!(&child.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::Null))
                || supported_array_element(child, target)
            }),
            ast::ExprKind::Cast { Tp, .. } => match Tp.PostgreSQLArrayElement() {
                Some(source) if matches!(source, Element::Bytea | Element::Bool | Element::Int2 | Element::Numeric | Element::Float4 | Element::Float8 | Element::Date | Element::Time) || matches!(target, Element::Bytea | Element::Bool | Element::Int2 | Element::Numeric | Element::Float4 | Element::Float8 | Element::Date | Element::Time) => source == target,
                Some(Element::Int2 | Element::Int4 | Element::Int8 | Element::Text | Element::Varchar | Element::Bpchar) => true,
                _ => Tp.PostgreSQLTextOid() == Some(25),
            },
            _ => false,
        }
    }
    fn supported_array_element(expr: &ast::ExprNode, target: Element) -> bool {
        match &expr.Kind {
            ast::ExprKind::Array { .. } => supported(expr, target),
            ast::ExprKind::ParamMarker { .. } => true,
            _ => {
                let source = element_type(expr, &HashMap::new()).ok().flatten();
                if matches!(target, Element::Float4 | Element::Float8) {
                    return source == Some(target)
                        || matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)));
                }
                if target == Element::Numeric {
                    return matches!(
                        source,
                        Some(Element::Numeric | Element::Int2 | Element::Int4 | Element::Int8)
                    ) || matches!(&expr.Kind,ast::ExprKind::Value(v) if matches!(v.Datum,ast::ValueDatum::String(_)));
                }
                if target == Element::Time {
                    return source == Some(Element::Time)
                        || matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)));
                }
                if target == Element::Date {
                    return source == Some(Element::Date)
                        || matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)));
                }
                if target == Element::Bytea {
                    return source == Some(Element::Bytea)
                        || matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)));
                }
                if target == Element::Bool {
                    return source == Some(Element::Bool)
                        || matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_)));
                }
                matches!(
                    source,
                    Some(
                        Element::Int2
                            | Element::Int4
                            | Element::Int8
                            | Element::Text
                            | Element::Varchar
                            | Element::Bpchar
                    )
                ) || (matches!(target, Element::Text | Element::Varchar | Element::Bpchar)
                    && matches!(&expr.Kind, ast::ExprKind::Value(v) if matches!(v.Datum, ast::ValueDatum::String(_))))
            }
        }
    }
    if matches!(
        target.PostgreSQLArrayElement(),
        Some(
            Element::Text
                | Element::Varchar
                | Element::Bpchar
                | Element::Bytea
                | Element::Bool
                | Element::Int2
                | Element::Numeric
                | Element::Float4
                | Element::Float8
                | Element::Date
                | Element::Time
        )
    ) && matches!(&expr.Kind, ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLArrayElement().is_some_and(|e| Some(e) != target.PostgreSQLArrayElement()))
    {
        return Err(error("0A000", "array cast source is not supported"));
    }
    if supported(expr, target.PostgreSQLArrayElement().unwrap()) {
        Ok(())
    } else {
        Err(error("0A000", "array cast source is not supported"))
    }
}

pub(super) fn cast_value(
    expr: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    target_type: &astersql_parser_types::FieldType,
    value: impl FnOnce() -> SessionResult<Option<String>>,
) -> SessionResult<Option<String>> {
    cast_value_with_context(expr, row, target_type, None, value)
}
pub(super) fn cast_value_with_context(
    expr: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    target_type: &astersql_parser_types::FieldType,
    context: Option<astersql_types::pg_temporal::PgDateContext>,
    value: impl FnOnce() -> SessionResult<Option<String>>,
) -> SessionResult<Option<String>> {
    use astersql_types::pg_array::{PgArray, PgArrayElement};
    let target = target_type.PostgreSQLArrayElement().unwrap();
    if target == Element::Float4 {
        if let ast::ExprKind::Value(v) = &expr.Kind
            && v.Type.PostgreSQLArrayElement() == Some(Element::Float4)
            && let ast::ValueDatum::Bytes(bytes) = &v.Datum
        {
            return float4_binary(bytes)
                .and_then(|array| runtime_array(&array))
                .map(Some);
        }
    }
    if target == Element::Float8 {
        if let ast::ExprKind::Value(v) = &expr.Kind
            && v.Type.PostgreSQLArrayElement() == Some(Element::Float8)
            && let ast::ValueDatum::Bytes(bytes) = &v.Datum
        {
            return float8_binary(bytes)
                .and_then(|array| runtime_array(&array))
                .map(Some);
        }
    }
    let mut expr = expr;
    while let ast::ExprKind::Parentheses(inner) = &expr.Kind {
        expr = inner;
    }
    if matches!(target, Element::Float4 | Element::Float8)
        && matches!(expr.Kind, ast::ExprKind::Array { .. })
    {
        return value_as(expr, row, target, context);
    }
    if matches!(expr.Kind, ast::ExprKind::Array { .. }) {
        let value = value_as(expr, row, target, context)?;
        return value
            .map(|s| {
                typed_array_with_modifier(
                    s.as_bytes(),
                    target,
                    target_type.GetFlen(),
                    target_type.GetDecimal(),
                )?
                .to_text(b',')
                .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))
            })
            .transpose();
    }
    let Some(text) = value()? else {
        return Ok(None);
    };
    let text = if target == Element::Date
        && let Some(context) = context
    {
        canonical_date_text(text.as_bytes(), context)?
    } else {
        text
    };
    let source = match &expr.Kind {
        ast::ExprKind::Cast { Tp, .. } => Tp.PostgreSQLArrayElement().unwrap_or(target),
        _ => target,
    };
    if source != target
        && (matches!(
            source,
            Element::Text
                | Element::Varchar
                | Element::Bpchar
                | Element::Bytea
                | Element::Bool
                | Element::Int2
                | Element::Numeric
                | Element::Float4
                | Element::Float8
                | Element::Date
                | Element::Time
        ) || matches!(
            target,
            Element::Text
                | Element::Varchar
                | Element::Bpchar
                | Element::Bytea
                | Element::Bool
                | Element::Int2
                | Element::Numeric
                | Element::Float4
                | Element::Float8
                | Element::Date
                | Element::Time
        ))
    {
        return Err(error("0A000", "array cast source is not supported"));
    }
    if target == Element::Float4 && source == target {
        if matches!(&expr.Kind, ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLArrayElement() == Some(Element::Float4))
        {
            if let Some(bytes) = super::row_codec::binary_runtime_bytes(&text) {
                return float4_binary(&bytes)
                    .and_then(|array| runtime_array(&array))
                    .map(Some);
            }
        }
        return typed_array(text.as_bytes(), target)
            .and_then(|array| runtime_array(&array))
            .map(Some);
    }
    if target == Element::Float8 && source == target {
        if matches!(&expr.Kind, ast::ExprKind::Cast { Tp, .. } if Tp.PostgreSQLArrayElement() == Some(Element::Float8))
        {
            if let Some(bytes) = super::row_codec::binary_runtime_bytes(&text) {
                return float8_binary(&bytes)
                    .and_then(|array| runtime_array(&array))
                    .map(Some);
            }
        }
        return typed_array(text.as_bytes(), target)
            .and_then(|array| runtime_array(&array))
            .map(Some);
    }
    let source = if matches!(
        target,
        Element::Time | Element::Numeric | Element::Varchar | Element::Bpchar
    ) && source == target
    {
        typed_array_with_modifier(
            text.as_bytes(),
            source,
            target_type.GetFlen(),
            target_type.GetDecimal(),
        )?
    } else {
        typed_array(text.as_bytes(), source)?
    };
    // Collect the entire conversion before constructing/publishing a result.
    let elements = source
        .elements()
        .iter()
        .map(|value| {
            Ok(match (target, value) {
                (_, None) => None,
                (Element::Int8, Some(PgArrayElement::Int4(v))) => {
                    Some(PgArrayElement::Int8(i64::from(*v)))
                }
                (Element::Int4, Some(PgArrayElement::Int8(v))) => Some(PgArrayElement::Int4(
                    i32::try_from(*v).map_err(|_| error("22003", "integer out of range"))?,
                )),
                (_, Some(v)) => Some(v.clone()),
            })
        })
        .collect::<SessionResult<Vec<_>>>()?;
    PgArray::new(
        target.oid(),
        source.element_typmod(),
        source.shape().to_vec(),
        source.lower_bounds().to_vec(),
        elements,
    )
    .and_then(|value| value.to_text(b','))
    .map(Some)
    .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))
}

fn float4_binary(bytes: &[u8]) -> SessionResult<astersql_types::pg_array::PgArray> {
    use astersql_types::pg_array::PgArrayBinaryError;
    astersql_types::pg_array::PgArray::float4_from_binary(bytes).map_err(|e| match &e {
        PgArrayBinaryError::Element((s, m)) => error(s, *m),
        _ => error(e.sqlstate().unwrap_or("XX000"), e.to_string()),
    })
}

fn float8_binary(bytes: &[u8]) -> SessionResult<astersql_types::pg_array::PgArray> {
    use astersql_types::pg_array::PgArrayBinaryError;
    astersql_types::pg_array::PgArray::float8_from_binary(bytes).map_err(|e| match &e {
        PgArrayBinaryError::Element((s, m)) => error(s, *m),
        _ => error(e.sqlstate().unwrap_or("XX000"), e.to_string()),
    })
}

fn runtime_array(array: &astersql_types::pg_array::PgArray) -> SessionResult<String> {
    let bytes = array
        .to_binary()
        .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))?;
    Ok(runtime_float4_bytes(&bytes))
}

pub(super) fn runtime_float4_bytes(bytes: &[u8]) -> String {
    format!(
        "{}{}",
        super::row_codec::BINARY_RUNTIME_PREFIX,
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    )
}

impl ConcreteSession {
    /// Restore only private prepared binary32/binary64 values at their original bound AST locations.
    pub(super) fn apply_pg_float4_constants(&self, nodes: &mut [Box<dyn ast::Node>]) {
        struct Apply<'a>(&'a HashMap<usize, ast::ExprNode>);
        impl ast::InPlaceVisitor for Apply<'_> {
            fn enter(&mut self, node: &mut dyn ast::Node) -> bool {
                self.enter_embedded(node.as_any_mut())
            }
            fn leave(&mut self, _: &mut dyn ast::Node) -> bool {
                true
            }
            fn enter_embedded(&mut self, node: &mut dyn std::any::Any) -> bool {
                if let Some(node) = node.downcast_mut::<ast::ExprNode>()
                    && let Some(value) = self.0.get(&(node.OriginTextPosition as usize))
                    && let ast::ExprKind::Cast {
                        Expr,
                        Tp,
                        FunctionType,
                        ..
                    } = &mut node.Kind
                    && (matches!(
                        *FunctionType,
                        ast::CastFunctionType::PostgreSQLFloat4
                            | ast::CastFunctionType::PostgreSQLFloat8
                    ) || matches!(
                        Tp.PostgreSQLArrayElement(),
                        Some(Element::Float4 | Element::Float8)
                    ))
                {
                    **Expr = value.clone();
                }
                false
            }
        }
        let state = self.state.borrow();
        if !state.pg_bound_float4_constants.is_empty() {
            for node in nodes {
                node.accept_in_place(&mut Apply(&state.pg_bound_float4_constants));
            }
        }
    }
}

/// Resolve each date element with the same immutable scalar input context.
pub(super) fn canonical_date_text(
    bytes: &[u8],
    context: astersql_types::pg_temporal::PgDateContext,
) -> SessionResult<String> {
    use astersql_types::pg_array::{PgArray, PgArrayElement, PgArrayTextError};
    PgArray::from_text_with(bytes, 1082, -1, b',', |text, _| {
        astersql_types::pg_temporal::PgDate::parse_with_context(text, context)
            .map(PgArrayElement::Date)
            .map_err(|e| error(e.sqlstate(), e.to_string()))
    })
    .map_err(|e| match e {
        PgArrayTextError::Element(e) => e,
        e => error(e.sqlstate().unwrap_or("XX000"), e.to_string()),
    })?
    .to_text(b',')
    .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), e.to_string()))
}
