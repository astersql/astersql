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
        struct Constants<'a> {
            session: &'a ConcreteSession,
            context: PgDateContext,
            values: HashMap<usize, String>,
            error: Option<SessionError>,
        }
        impl ast::Visitor for Constants<'_> {
            fn enter(&mut self, input: &dyn ast::Node) -> bool {
                self.enter_embedded(input.as_any())
            }
            fn leave(&mut self, _: &dyn ast::Node) -> bool {
                true
            }
            fn enter_embedded(&mut self, input: &dyn std::any::Any) -> bool {
                if let Some(expr) = input.downcast_ref::<ast::ExprNode>()
                    && let ast::ExprKind::Cast { Expr, Tp, .. } = &expr.Kind
                    && (matches!(
                        Tp.PostgreSQLArrayElement(),
                        Some(
                            astersql_parser_types::PostgreSQLArrayElement::Date
                                | astersql_parser_types::PostgreSQLArrayElement::Timestamptz
                                | astersql_parser_types::PostgreSQLArrayElement::Interval
                        )
                    ) || matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypePgInterval
                            | astersql_parser_mysql::r#type::TypePgTimestampTz
                    ))
                {
                    let mut source = Expr.as_ref();
                    while let ast::ExprKind::Parentheses(inner) = &source.Kind {
                        source = inner;
                    }
                    if let ast::ExprKind::Value(value) = &source.Kind
                        && let ast::ValueDatum::String(text) = &value.Datum
                    {
                        if Tp.PostgreSQLArrayElement()
                            == Some(astersql_parser_types::PostgreSQLArrayElement::Interval)
                        {
                            match super::pg_array::canonical_interval_text(
                                text.as_bytes(),
                                self.session.pg_interval_style(None),
                                Tp.GetFlen(),
                                Tp.GetDecimal(),
                            ) {
                                Ok(value) => {
                                    self.values.insert(expr.OriginTextPosition as usize, value);
                                }
                                Err(error) => self.error = Some(error),
                            }
                            return false;
                        }
                        if Tp.PostgreSQLArrayElement()
                            == Some(astersql_parser_types::PostgreSQLArrayElement::Timestamptz)
                        {
                            match self
                                .session
                                .pg_timestamptz_array_input(text.as_bytes(), Tp.GetDecimal())
                            {
                                Ok(value) => {
                                    self.values.insert(expr.OriginTextPosition as usize, value);
                                }
                                Err(error) => self.error = Some(error),
                            }
                            return false;
                        }
                        if Tp.PostgreSQLArrayElement()
                            == Some(astersql_parser_types::PostgreSQLArrayElement::Date)
                        {
                            match super::pg_array::canonical_date_text(
                                text.as_bytes(),
                                self.context,
                            ) {
                                Ok(value) => {
                                    self.values.insert(expr.OriginTextPosition as usize, value);
                                }
                                Err(error) => self.error = Some(error),
                            }
                            return false;
                        }
                        let value = if Tp.GetType()
                            == astersql_parser_mysql::r#type::TypePgTimestampTz
                        {
                            match self.session.pg_timestamptz_input(text).and_then(|v| {
                                v.round_precision(Tp.GetDecimal()).map_err(temporal_error)
                            }) {
                                Ok(v) => {
                                    self.values
                                        .insert(expr.OriginTextPosition as usize, v.to_string());
                                }
                                Err(error) => self.error = Some(error),
                            }
                            return false;
                        } else if Tp.GetType() == astersql_parser_mysql::r#type::TypePgInterval {
                            astersql_types::pg_temporal::PgInterval::parse_with_style(
                                text,
                                self.session.pg_interval_style(None),
                            )
                            .and_then(|v| v.apply_typmod(Tp.GetFlen(), Tp.GetDecimal()))
                            .map(|v| v.to_string())
                        } else if Tp.GetType() == astersql_parser_mysql::r#type::TypeDate {
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
            session: self,
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
                    && (matches!(
                        Tp.PostgreSQLArrayElement(),
                        Some(
                            astersql_parser_types::PostgreSQLArrayElement::Date
                                | astersql_parser_types::PostgreSQLArrayElement::Timestamptz
                                | astersql_parser_types::PostgreSQLArrayElement::Interval
                        )
                    ) || matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypePgInterval
                            | astersql_parser_mysql::r#type::TypePgTimestampTz
                    ))
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

impl ConcreteSession {
    /// Extract time only after resolving the instant in the session timezone.
    /// Avoid timestamp's narrower date range: the local date may cross its edge.
    pub(super) fn pg_timestamptz_to_time_value(
        &self,
        value: Option<String>,
        precision: isize,
    ) -> SessionResult<Option<String>> {
        let Some(local) = self.pg_timestamptz_relational_value(value, -1)? else {
            return Ok(None);
        };
        if matches!(local.as_str(), "infinity" | "-infinity") {
            return Ok(None);
        }
        let time = local
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| temporal_error(astersql_types::pg_temporal::PgTemporalError::Syntax))?;
        let time = time.split(['+', '-']).next().unwrap_or(time);
        time.parse::<astersql_types::pg_temporal::PgTime>()
            .map(|time| Some(time.round_precision(precision).to_string()))
            .map_err(temporal_error)
    }

    /// Resolve the instant in this session before dropping its zone. Parse the
    /// canonical ISO wall clock to enforce timestamp's range, then round the
    /// target precision (which can carry across midnight or overflow).
    pub(super) fn pg_timestamptz_to_timestamp_value(
        &self,
        value: Option<String>,
        precision: isize,
    ) -> SessionResult<Option<String>> {
        self.pg_timestamptz_relational_value(value, -1)?
            .map(|local| {
                local
                    .parse::<astersql_types::pg_temporal::PgTimestamp>()
                    .and_then(|wall| wall.round_precision(precision))
                    .map(|wall| wall.to_string())
                    .map_err(temporal_error)
            })
            .transpose()
    }

    /// Extract the calendar date after resolving the instant in this session.
    /// The display is canonical ISO, so DateStyle cannot reinterpret it. Keep
    /// infinities and BC eras, including local dates outside timestamp's range.
    pub(super) fn pg_timestamptz_to_date_value(
        &self,
        value: Option<String>,
    ) -> SessionResult<Option<String>> {
        self.pg_timestamptz_relational_value(value, -1)?
            .map(|local| {
                let date = if let Some((date, _)) = local.split_once(' ') {
                    if local.ends_with(" BC") {
                        format!("{date} BC")
                    } else {
                        date.to_owned()
                    }
                } else {
                    local
                };
                date.parse::<PgDate>()
                    .map(|date| date.to_string())
                    .map_err(temporal_error)
            })
            .transpose()
    }

    /// Convert the typed instantaneous value to the active session's display.
    pub(super) fn pg_timestamptz_relational_value(
        &self,
        value: Option<String>,
        precision: isize,
    ) -> SessionResult<Option<String>> {
        use astersql_types::pg_temporal::PgTemporalError;
        use chrono::Offset;
        value
            .map(|text| {
                let instant = self
                    .pg_timestamptz_input(&text)?
                    .round_precision(precision)
                    .map_err(temporal_error)?;
                if matches!(instant.micros(), i64::MIN | i64::MAX) {
                    return Ok(instant.to_string());
                }
                let offset = match *self.time_zone.borrow() {
                    RuntimeTimeZone::Fixed(zone) => zone.local_minus_utc(),
                    RuntimeTimeZone::Named(chrono_tz::UTC) => 0,
                    RuntimeTimeZone::Named(zone) => {
                        let unix = instant
                            .micros()
                            .checked_add(946_684_800_000_000)
                            .ok_or_else(|| temporal_error(PgTemporalError::Range))?;
                        let date = chrono::DateTime::from_timestamp_micros(unix)
                            .ok_or_else(|| temporal_error(PgTemporalError::Unsupported))?;
                        date.with_timezone(&zone).offset().fix().local_minus_utc()
                    }
                };
                instant.format_with_offset(offset).map_err(temporal_error)
            })
            .transpose()
    }
}

impl ConcreteSession {
    /// Resolve text only at a real Parse/Bind/cast boundary. Canonical UTC text
    /// subsequently travels through the existing typed parameter/row carrier.
    pub fn pg_timestamptz_input(
        &self,
        text: &str,
    ) -> SessionResult<astersql_types::pg_temporal::PgTimestampTz> {
        use astersql_types::pg_temporal::{PgTemporalError, PgTimestamp, PgTimestampTz};
        use chrono::{Offset, TimeZone};
        if !matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "now" | "today" | "yesterday" | "tomorrow"
        ) {
            match text.parse::<PgTimestampTz>() {
                Ok(value) => return Ok(value),
                Err(PgTemporalError::Unsupported) => {}
                Err(error) => return Err(temporal_error(error)),
            }
        }
        if text.trim().eq_ignore_ascii_case("now") {
            let state = self.state.borrow();
            let seconds = if state.transaction.is_some() {
                state.pg_transaction_time.unwrap_or(state.pg_statement_time)
            } else {
                state.pg_statement_time
            };
            let micros = (seconds * 1_000_000.0) as i64;
            return PgTimestampTz::from_micros(
                micros
                    .checked_sub(946_684_800_000_000)
                    .ok_or_else(|| temporal_error(PgTemporalError::Range))?,
            )
            .map_err(temporal_error);
        }
        let wall = PgTimestamp::parse_with_context(text, self.pg_date_context(None)?)
            .map_err(temporal_error)?;
        let offset = match *self.time_zone.borrow() {
            RuntimeTimeZone::Fixed(zone) => zone.local_minus_utc(),
            RuntimeTimeZone::Named(chrono_tz::UTC) => 0,
            RuntimeTimeZone::Named(zone) => {
                let unix = wall
                    .micros()
                    .checked_add(946_684_800_000_000)
                    .ok_or_else(|| temporal_error(PgTemporalError::Range))?;
                let local = chrono::DateTime::from_timestamp_micros(unix)
                    .ok_or_else(|| temporal_error(PgTemporalError::Unsupported))?
                    .naive_utc();
                // PostgreSQL resolves a forward gap with the pre-transition
                // offset and a backward overlap with the post-transition one.
                // Use transition data rather than assuming a one-hour change.
                match zone.from_local_datetime(&local) {
                    chrono::LocalResult::Single(value) => value.offset().fix().local_minus_utc(),
                    chrono::LocalResult::Ambiguous(_, after) => {
                        after.offset().fix().local_minus_utc()
                    }
                    chrono::LocalResult::None => chrono_tz::GapInfo::new(&local, &zone)
                        .and_then(|gap| gap.begin)
                        .map(|(_, before)| before.fix().local_minus_utc())
                        .ok_or_else(|| temporal_error(PgTemporalError::Unsupported))?,
                }
            }
        };
        PgTimestampTz::from_micros(
            wall.micros()
                .checked_sub(i64::from(offset) * 1_000_000)
                .ok_or_else(|| temporal_error(PgTemporalError::Range))?,
        )
        .map_err(temporal_error)
    }
}

/// IntervalStyle uses the same state in casts and the protocol worker. The
/// transaction snapshot restores SET on abort; LOCAL expires at either end.
#[derive(Clone, Default)]
pub(super) struct IntervalSettings {
    default: astersql_types::pg_temporal::PgIntervalStyle,
    session: astersql_types::pg_temporal::PgIntervalStyle,
    local: Option<astersql_types::pg_temporal::PgIntervalStyle>,
    transaction: Option<astersql_types::pg_temporal::PgIntervalStyle>,
}
impl IntervalSettings {
    pub(super) fn begin(&mut self) {
        self.transaction = Some(self.session);
    }
    pub(super) fn finish(&mut self, commit: bool) {
        if let Some(initial) = self.transaction.take() {
            if !commit {
                self.session = initial;
            }
        }
        self.local = None;
    }
}
impl ConcreteSession {
    /// Character coercion uses interval_out with the current session style.
    pub(super) fn pg_interval_output(
        &self,
        value: Option<String>,
    ) -> SessionResult<Option<String>> {
        value
            .map(|text| {
                text.parse::<astersql_types::pg_temporal::PgInterval>()
                    .map(|value| value.format_with_style(self.pg_interval_style(None)))
                    .map_err(temporal_error)
            })
            .transpose()
    }

    pub fn pg_interval_style(
        &self,
        setting: Option<astersql_types::pg_temporal::PgIntervalSetting>,
    ) -> astersql_types::pg_temporal::PgIntervalStyle {
        use astersql_types::pg_temporal::PgIntervalSetting;
        let mut state = self.state.borrow_mut();
        let active = state.transaction.is_some();
        let values = &mut state.pg_interval_settings;
        match setting {
            Some(PgIntervalSetting::Startup(style)) => {
                values.default = style;
                values.session = style;
                values.local = None;
            }
            Some(PgIntervalSetting::ResetLocal) => {
                if active {
                    values.local = Some(values.default);
                }
            }
            Some(PgIntervalSetting::Reset) => {
                values.session = values.default;
                values.local = None;
            }
            Some(PgIntervalSetting::Set { style, local: true }) => {
                if active {
                    values.local = Some(style);
                }
            }
            Some(PgIntervalSetting::Set {
                style,
                local: false,
            }) => {
                values.session = style;
                values.local = None;
            }
            None => {}
        }
        values.local.unwrap_or(values.session)
    }
}
