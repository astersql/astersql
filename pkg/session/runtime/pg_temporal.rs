// Copyright 2026 AsterSQL.
//! Temporal connection settings, transaction clock and prepared constant values.
use super::session::RuntimeTimeZone;
use super::*;
use astersql_types::pg_temporal::{PgDate, PgDateContext, PgDateOrder};
use chrono::{Datelike, Timelike};

fn temporal_error(error: astersql_types::pg_temporal::PgTemporalError) -> SessionError {
    SessionError::with_source(error.to_string(), astersql_errors::SharedError::new(error))
}

impl ConcreteSession {
    /// Snapshot the statement wall clock. The native override supplies the
    /// existing deterministic clock boundary; the PG transaction freezes it.
    pub fn begin_pg_date_statement(&self) {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return;
        }
        let mut state = self.state.borrow_mut();
        state.pg_statement_time = state.timestamp_override.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
        });
    }

    pub fn pg_date_context(&self, order: Option<PgDateOrder>) -> SessionResult<PgDateContext> {
        let mut state = self.state.borrow_mut();
        if let Some(order) = order {
            state.pg_date_order = order;
        }
        let seconds = if state.transaction.is_some() {
            state.pg_transaction_time.unwrap_or(state.pg_statement_time)
        } else {
            state.pg_statement_time
        };
        let timestamp = chrono::DateTime::from_timestamp_micros((seconds * 1_000_000.0) as i64)
            .filter(|_| seconds.is_finite())
            .ok_or_else(|| temporal_error(astersql_types::pg_temporal::PgTemporalError::Range))?;
        let (year, month, day, time_micros) = match *self.time_zone.borrow() {
            RuntimeTimeZone::Named(zone) => {
                let date = timestamp.with_timezone(&zone);
                (
                    date.year(),
                    date.month(),
                    date.day(),
                    i64::from(date.num_seconds_from_midnight()) * 1_000_000
                        + i64::from(date.nanosecond() / 1000),
                )
            }
            RuntimeTimeZone::Fixed(zone) => {
                let date = timestamp.with_timezone(&zone);
                (
                    date.year(),
                    date.month(),
                    date.day(),
                    i64::from(date.num_seconds_from_midnight()) * 1_000_000
                        + i64::from(date.nanosecond() / 1000),
                )
            }
        };
        Ok(PgDateContext {
            order: state.pg_date_order,
            time_micros,
            today: PgDate::from_ymd(if year <= 0 { year - 1 } else { year }, month, day)
                .map_err(temporal_error)?,
        })
    }

    /// Analyze real cast ASTs at Parse. Only literal inputs are frozen: a text
    /// parameter cast is evaluated later using the execution context.
    pub(super) fn prepare_pg_date_constants(
        &self,
        nodes: &[Box<dyn ast::Node>],
    ) -> SessionResult<HashMap<usize, String>> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Ok(HashMap::new());
        }
        struct Constants {
            context: PgDateContext,
            values: HashMap<usize, String>,
            error: Option<SessionError>,
        }
        impl ast::Visitor for Constants {
            fn enter(&mut self, input: &dyn ast::Node) -> bool {
                self.enter_embedded(input.as_any())
            }
            fn leave(&mut self, _: &dyn ast::Node) -> bool {
                true
            }
            fn enter_embedded(&mut self, input: &dyn std::any::Any) -> bool {
                if let Some(expr) = input.downcast_ref::<ast::ExprNode>()
                    && let ast::ExprKind::Cast { Expr, Tp, .. } = &expr.Kind
                    && matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                    )
                {
                    let mut source = Expr.as_ref();
                    while let ast::ExprKind::Parentheses(inner) = &source.Kind {
                        source = inner;
                    }
                    if let ast::ExprKind::Value(value) = &source.Kind
                        && let ast::ValueDatum::String(text) = &value.Datum
                    {
                        let value = if Tp.GetType() == astersql_parser_mysql::r#type::TypeDate {
                            PgDate::parse_with_context(text, self.context).map(|v| v.to_string())
                        } else {
                            astersql_types::pg_temporal::PgTimestamp::parse_with_context(
                                text,
                                self.context,
                            )
                            .and_then(|v| v.round_precision(Tp.GetDecimal()))
                            .map(|v| v.to_string())
                        };
                        match value {
                            Ok(date) => {
                                self.values.insert(expr.OriginTextPosition as usize, date);
                            }
                            Err(error) => self.error = Some(temporal_error(error)),
                        }
                    }
                }
                false
            }
        }
        let mut constants = Constants {
            context: self.pg_date_context(None)?,
            values: HashMap::new(),
            error: None,
        };
        for node in nodes {
            node.accept(&mut constants);
        }
        if let Some(error) = constants.error {
            return Err(error);
        }
        Ok(constants.values)
    }

    /// Reinstall prepared canonical date/timestamp text in the typed AST, retaining real cast
    /// evaluation and metadata. No SQL spelling is searched or replaced.
    pub(super) fn apply_pg_date_constants(&self, nodes: &mut [Box<dyn ast::Node>]) {
        struct Apply<'a>(&'a HashMap<usize, String>);
        impl ast::InPlaceVisitor for Apply<'_> {
            fn enter(&mut self, input: &mut dyn ast::Node) -> bool {
                self.enter_embedded(input.as_any_mut())
            }
            fn leave(&mut self, _: &mut dyn ast::Node) -> bool {
                true
            }
            fn enter_embedded(&mut self, input: &mut dyn std::any::Any) -> bool {
                if let Some(expr) = input.downcast_mut::<ast::ExprNode>()
                    && let Some(date) = self.0.get(&(expr.OriginTextPosition as usize))
                    && let ast::ExprKind::Cast { Expr, Tp, .. } = &mut expr.Kind
                    && matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                    )
                {
                    **Expr = ast::ExprNode::Value(date.clone());
                }
                false
            }
        }
        let state = self.state.borrow();
        if state.pg_bound_date_constants.is_empty() {
            return;
        }
        for node in nodes {
            node.accept_in_place(&mut Apply(&state.pg_bound_date_constants));
        }
    }
}
