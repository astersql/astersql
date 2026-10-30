// Copyright 2026 AsterSQL.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use astersql_expression_aggregation::{AggFuncDesc, AggFunctionMode};
use astersql_planner_core_base::ContextRef;
use astersql_types::datum::{
    Datum, KindFloat64, KindMysqlDecimal, NewDecimalDatum, NewFloat64Datum, NewIntDatum,
};
use astersql_types::decimal::mydecimal::{DecimalDiv, MyDecimal, NewDecFromInt};
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};

enum AggregateValue {
    Count(i64),
    FloatSum(Option<f64>),
    FloatAvg {
        count: f64,
        sum: f64,
        squares: f64,
    },
    NumericSum(Option<astersql_types::pg_numeric::PgNumeric>),
    NumericAvg {
        sum: Option<astersql_types::pg_numeric::PgNumeric>,
        count: i64,
    },
    Sum {
        value: Datum,
        count: i64,
    },
    First {
        seen: bool,
        value: Datum,
    },
    Extremum(Datum),
    CountExtrema {
        function: Box<dyn astersql_executor_aggfuncs::func_max_min_count::CountExtremaAgg>,
        state: astersql_executor_aggfuncs::aggfuncs::PartialResult,
    },
}

struct AggregateState {
    value: AggregateValue,
    distinct: Option<HashSet<Vec<u8>>>,
    ordered: Vec<(Vec<Datum>, Vec<Datum>)>,
}

struct GroupState {
    aggregates: Vec<AggregateState>,
}

/// Typed HashAgg consumes child chunks incrementally and retains only one
/// aggregate state per group. Ordered aggregates additionally retain evaluated
/// arguments and sort keys until the child is exhausted; lock keys are discarded.
pub struct TypedHashAgg {
    child: Box<dyn ExecExecutor>,
    functions: Vec<AggFuncDesc>,
    group_by: Vec<astersql_expression::ExprBox>,
    context: ContextRef,
    schema: Vec<SchemaColumn>,
    groups: Vec<GroupState>,
    group_indexes: HashMap<Vec<u8>, usize>,
    output_index: usize,
    prepared: bool,
    opened: bool,
    closed: bool,
}

impl TypedHashAgg {
    pub fn new(
        child: Box<dyn ExecExecutor>,
        functions: Vec<AggFuncDesc>,
        group_by: Vec<astersql_expression::ExprBox>,
        context: ContextRef,
    ) -> Result<Self, String> {
        for function in &functions {
            match function.Name.as_str() {
                astersql_parser_ast::AggFuncCount => {
                    if function.Args.is_empty()
                        && matches!(
                            function.Mode,
                            AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
                        )
                    {
                        return Err("final typed COUNT requires one argument".to_owned());
                    }
                }
                astersql_parser_ast::AggFuncAvg => {
                    let required = if matches!(
                        function.Mode,
                        AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
                    ) {
                        2
                    } else {
                        1
                    };
                    if function.Args.len() < required {
                        return Err(format!("typed AVG requires {required} argument(s)"));
                    }
                }
                "pg_avg_float4"
                | "pg_avg_float8"
                | "pg_sum_float4"
                | "pg_sum_float8"
                | "pg_avg_numeric"
                | "pg_sum_numeric"
                | astersql_parser_ast::AggFuncSum
                | astersql_parser_ast::AggFuncSumInt
                | astersql_parser_ast::AggFuncFirstRow
                | astersql_parser_ast::AggFuncMax
                | astersql_parser_ast::AggFuncMin
                | astersql_parser_ast::AggFuncMaxCount
                | astersql_parser_ast::AggFuncMinCount => {
                    if function.Args.is_empty() {
                        return Err(format!("typed {} requires one argument", function.Name));
                    }
                }
                name => return Err(format!("typed HashAgg does not support aggregate {name}")),
            }
            if matches!(
                function.Name.as_str(),
                astersql_parser_ast::AggFuncMaxCount | astersql_parser_ast::AggFuncMinCount
            ) {
                if function.HasDistinct
                    || astersql_executor_aggfuncs::func_max_min_count::build_count_extrema_function(
                        context.GetExprCtx().GetEvalCtx(),
                        function.Args.clone(),
                        0,
                        function.Name == astersql_parser_ast::AggFuncMaxCount,
                        matches!(
                            function.Mode,
                            AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
                        ) && function.Args.len() > 1,
                        false,
                    )
                    .is_none()
                {
                    return Err("unsupported typed count-extrema argument or DISTINCT".to_owned());
                }
            }
            if function.Mode == AggFunctionMode::DedupMode {
                return Err("typed HashAgg does not support deduplicate mode".to_owned());
            }
        }
        let schema = functions
            .iter()
            .map(|function| SchemaColumn {
                field_type: function.RetTp.clone().unwrap_or_default(),
            })
            .collect();
        Ok(Self {
            child,
            functions,
            group_by,
            context,
            schema,
            groups: Vec::new(),
            group_indexes: HashMap::new(),
            output_index: 0,
            prepared: false,
            opened: false,
            closed: false,
        })
    }

    fn new_group(&self) -> GroupState {
        GroupState {
            aggregates: self
                .functions
                .iter()
                .map(|function| AggregateState {
                    value: match function.Name.as_str() {
                        astersql_parser_ast::AggFuncCount => AggregateValue::Count(0),
                        "pg_sum_float4" | "pg_sum_float8" => AggregateValue::FloatSum(None),
                        "pg_avg_float4" | "pg_avg_float8" => AggregateValue::FloatAvg { count: 0.0, sum: 0.0, squares: 0.0 },
                        "pg_sum_numeric" => AggregateValue::NumericSum(None),
                        "pg_avg_numeric" => AggregateValue::NumericAvg { sum: None, count: 0 },
                        astersql_parser_ast::AggFuncSum
                        | astersql_parser_ast::AggFuncSumInt
                        | astersql_parser_ast::AggFuncAvg => AggregateValue::Sum {
                            value: Datum::default(),
                            count: 0,
                        },
                        astersql_parser_ast::AggFuncFirstRow => AggregateValue::First {
                            seen: false,
                            value: Datum::default(),
                        },
                        astersql_parser_ast::AggFuncMax | astersql_parser_ast::AggFuncMin => {
                            AggregateValue::Extremum(Datum::default())
                        }
                        astersql_parser_ast::AggFuncMaxCount | astersql_parser_ast::AggFuncMinCount => {
                            let evaluator = astersql_executor_aggfuncs::func_max_min_count::build_count_extrema_function(
                                self.context.GetExprCtx().GetEvalCtx(), function.Args.clone(), 0,
                                function.Name == astersql_parser_ast::AggFuncMaxCount,
                                matches!(function.Mode, AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode) && function.Args.len() > 1,
                                false,
                            ).expect("validated count-extrema argument type");
                            let (state, _) = evaluator.alloc_partial_result();
                            AggregateValue::CountExtrema { function: evaluator, state }
                        }
                        _ => unreachable!("aggregate names are validated by new"),
                    },
                    distinct: function.HasDistinct.then(HashSet::new),
                    ordered: Vec::new(),
                })
                .collect(),
        }
    }

    fn check_cancel(context: Option<&ExecutionContext>) -> AdapterResult {
        if let Some(killer) = context.and_then(|context| context.sql_killer.as_ref()) {
            killer.HandleSignal()?;
        }
        Ok(())
    }

    fn prepare(&mut self, context: Option<&ExecutionContext>) -> AdapterResult {
        if self.prepared {
            return Ok(());
        }
        let mut input = self.child.NewChunk();
        loop {
            Self::check_cancel(context)?;
            if let Some(context) = context {
                self.child.NextWithContext(context, &mut input)?;
            } else {
                self.child.Next(&mut input)?;
            }
            let rows = input.NumRows();
            // Hash aggregation owns no record identity after reduction.
            let _ = self.child.TakeLockKeys();
            if rows == 0 {
                break;
            }
            for index in 0..rows {
                Self::check_cancel(context)?;
                self.update_row(input.GetRow(index))?;
            }
        }
        if self.groups.is_empty() && self.group_by.is_empty() {
            self.groups.push(self.new_group());
        }
        let eval = self.context.GetExprCtx().GetEvalCtx();
        for group in &mut self.groups {
            for (function, state) in self.functions.iter().zip(&mut group.aggregates) {
                let mut inputs = std::mem::take(&mut state.ordered);
                let order = function
                    .OrderByItems
                    .iter()
                    .map(|item| (item.Expr.GetType(eval).clone(), item.Desc))
                    .collect::<Vec<_>>();
                SortAggregateInput(self.context.clone(), &mut inputs, &order)?;
                for (values, _) in inputs {
                    Self::check_cancel(context)?;
                    update_values(function, state, eval, values)?;
                }
            }
        }
        self.prepared = true;
        Ok(())
    }

    fn update_row(&mut self, row: chunk::Row) -> AdapterResult {
        let eval = self.context.GetExprCtx().GetEvalCtx();
        let mut group_values = Vec::with_capacity(self.group_by.len());
        for expression in &self.group_by {
            group_values.push(
                expression
                    .Eval(eval, row.clone())
                    .map_err(|error| astersql_errors::New(error.to_string()))?,
            );
        }
        let key = astersql_util_codec::EncodeKey(eval.Location(), Vec::new(), group_values)
            .map_err(|error| astersql_errors::New(error.to_string()))?;
        let group_index = if let Some(index) = self.group_indexes.get(&key) {
            *index
        } else {
            let index = self.groups.len();
            self.groups.push(self.new_group());
            self.group_indexes.insert(key, index);
            index
        };
        let group = &mut self.groups[group_index];
        for (function, state) in self.functions.iter().zip(&mut group.aggregates) {
            update_aggregate(function, state, eval, row.clone())?;
        }
        Ok(())
    }

    fn append_group(&self, index: usize, output: &mut chunk::Chunk) -> AdapterResult {
        let eval = self.context.GetExprCtx().GetEvalCtx();
        for (column, (function, state)) in self
            .functions
            .iter()
            .zip(&self.groups[index].aggregates)
            .enumerate()
        {
            let value = finish_aggregate(function, state, eval)?;
            output.AppendDatum(column, &value);
        }
        if self.functions.is_empty() {
            output.SetNumVirtualRows(output.NumRows() + 1);
        }
        Ok(())
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(astersql_errors::New(
                "hash aggregation executor is not open",
            ));
        }
        output.Reset();
        self.prepare(context)?;
        while !output.IsFull() && self.output_index < self.groups.len() {
            Self::check_cancel(context)?;
            self.append_group(self.output_index, output)?;
            self.output_index += 1;
        }
        Ok(())
    }
}

fn evaluated_arguments(
    function: &AggFuncDesc,
    eval: &dyn astersql_expression_exprctx::EvalContext,
    row: chunk::Row,
) -> AdapterResult<Vec<Datum>> {
    function
        .Args
        .iter()
        .map(|argument| {
            argument
                .Eval(eval, row.clone())
                .map_err(|error| astersql_errors::New(error.to_string()))
        })
        .collect()
}

/// Equality key for already typed PostgreSQL numeric aggregate operands.
/// Display scale and floating zero signs/NaN payloads do not affect equality.
pub fn NumericDistinctKey(value: &str, oid: u32) -> AdapterResult<Vec<u8>> {
    if oid == 1700 {
        let numeric = value
            .parse::<astersql_types::pg_numeric::PgNumeric>()
            .map_err(astersql_errors::SharedError::new)?;
        let mut bytes = numeric.to_binary();
        bytes[6..8].fill(0); // numeric_send dscale is presentation metadata.
        return Ok(bytes);
    }
    let float = if oid == 700 {
        astersql_expression::pg_cast_float4(Some(value))
            .map_err(astersql_errors::SharedError::new)?
            .map(f64::from)
    } else {
        astersql_expression::pg_cast_float8(Some(value))
            .map_err(astersql_errors::SharedError::new)?
    }
    .expect("nonnull typed operand");
    Ok((if float == 0.0 {
        0.0
    } else if float.is_nan() {
        f64::NAN
    } else {
        float
    })
    .to_bits()
    .to_be_bytes()
    .to_vec())
}

/// Sort already evaluated aggregate input with the same typed comparison used
/// by HashAgg. Relational row adapters call this after FILTER and before feeding
/// their existing typed transition executor; no SQL evaluation occurs here.
pub fn SortAggregateInput<T>(
    context: ContextRef,
    inputs: &mut [(T, Vec<Datum>)],
    order: &[(astersql_expression::types::FieldType, bool)],
) -> AdapterResult {
    let eval = context.GetExprCtx().GetEvalCtx();
    let mut comparison_error = None;
    inputs.sort_by(|left, right| {
        for (index, (tp, desc)) in order.iter().enumerate() {
            let ordering = match aggregate_order(eval, tp, &left.1[index], &right.1[index]) {
                Ok(ordering) => ordering,
                Err(error) => {
                    comparison_error = Some(error);
                    return std::cmp::Ordering::Equal;
                }
            };
            if !ordering.is_eq() {
                return if *desc { ordering.reverse() } else { ordering };
            }
        }
        std::cmp::Ordering::Equal
    });
    if let Some(error) = comparison_error {
        return Err(error);
    }
    Ok(())
}

/// Compare typed keys; PostgreSQL's default NULL position is the reverse of
/// MySQL's, while DESC reverses the complete ordering at the caller.
fn aggregate_order(
    eval: &dyn astersql_expression_exprctx::EvalContext,
    tp: &astersql_expression::types::FieldType,
    left: &Datum,
    right: &Datum,
) -> AdapterResult<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let pg = eval.SQLDialect() == astersql_parser_ast::SqlDialect::PostgreSQL;
    if left.IsNull() || right.IsNull() {
        let order = left.IsNull().cmp(&right.IsNull());
        return Ok(if pg { order } else { order.reverse() });
    }
    if pg {
        match tp.PostgreSQLTypeOid() {
            Some(1700) => {
                let left = left
                    .GetString()
                    .parse::<astersql_types::pg_numeric::PgNumeric>()
                    .map_err(astersql_errors::SharedError::new)?;
                let right = right
                    .GetString()
                    .parse::<astersql_types::pg_numeric::PgNumeric>()
                    .map_err(astersql_errors::SharedError::new)?;
                return Ok(astersql_types::pg_numrange::numeric_order(&left, &right));
            }
            Some(700 | 701) => {
                let (left, right) = if tp.PostgreSQLTypeOid() == Some(700) {
                    (f64::from(left.GetFloat32()), f64::from(right.GetFloat32()))
                } else {
                    (left.GetFloat64(), right.GetFloat64())
                };
                return Ok(match (left.is_nan(), right.is_nan()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    _ => left.partial_cmp(&right).unwrap(),
                });
            }
            _ => {}
        }
    }
    let collator = astersql_util_collate::GetCollator(tp.GetCollate());
    left.Compare(eval.TypeCtx(), right, collator.as_ref())
        .map(|order| order.cmp(&0))
        .map_err(|error| astersql_errors::New(error.to_string()))
}

fn update_aggregate(
    function: &AggFuncDesc,
    state: &mut AggregateState,
    eval: &dyn astersql_expression_exprctx::EvalContext,
    row: chunk::Row,
) -> AdapterResult {
    if let Some(filter) = &function.Filter {
        let (keep, _) = astersql_expression::EvalBool(
            eval,
            &astersql_expression::CNFExprs(vec![filter.clone()]),
            row.clone(),
        )
        .map_err(|error| astersql_errors::New(error.to_string()))?;
        if !keep {
            return Ok(());
        }
    }
    if let AggregateValue::CountExtrema { function, state } = &mut state.value {
        function
            .update_partial_result(eval, &[row], state)
            .map_err(|error| astersql_errors::New(error.to_string()))?;
        return Ok(());
    }
    let values = evaluated_arguments(function, eval, row.clone())?;
    if !function.OrderByItems.is_empty() {
        let keys = function
            .OrderByItems
            .iter()
            .map(|item| {
                item.Expr
                    .Eval(eval, row.clone())
                    .map_err(|error| astersql_errors::New(error.to_string()))
            })
            .collect::<AdapterResult<Vec<_>>>()?;
        state.ordered.push((values, keys));
        return Ok(());
    }
    update_values(function, state, eval, values)
}

fn update_values(
    function: &AggFuncDesc,
    state: &mut AggregateState,
    eval: &dyn astersql_expression_exprctx::EvalContext,
    values: Vec<Datum>,
) -> AdapterResult {
    if function.Name != astersql_parser_ast::AggFuncFirstRow && values.iter().any(Datum::IsNull) {
        return Ok(());
    }
    if let Some(distinct) = &mut state.distinct {
        let mut key_values = values.clone();
        if eval.SQLDialect() == astersql_parser_ast::SqlDialect::PostgreSQL {
            for (argument, value) in function.Args.iter().zip(&mut key_values) {
                match argument.GetType(eval).PostgreSQLTypeOid() {
                    Some(oid @ (1700 | 700 | 701)) => {
                        let text = match oid {
                            700 => value.GetFloat32().to_string(),
                            701 => value.GetFloat64().to_string(),
                            _ => value
                                .ToString()
                                .map_err(|e| astersql_errors::New(e.to_string()))?,
                        };
                        *value =
                            astersql_types::datum::NewBytesDatum(NumericDistinctKey(&text, oid)?);
                    }
                    _ => {}
                }
            }
        }
        let key = astersql_util_codec::EncodeKey(eval.Location(), Vec::new(), key_values)
            .map_err(|error| astersql_errors::New(error.to_string()))?;
        if !distinct.insert(key) {
            return Ok(());
        }
    }
    match &mut state.value {
        AggregateValue::FloatAvg {
            count,
            sum,
            squares,
        } => {
            let next = if function.Name == "pg_avg_float4" {
                f64::from(values[0].GetFloat32())
            } else {
                values[0].GetFloat64()
            };
            let previous_count = *count;
            let previous_sum = *sum;
            *count += 1.0;
            *sum += next;
            // Match PostgreSQL float[48]_accum, including the Sxx overflow
            // check even though AVG finalization only consumes N and Sx.
            if previous_count > 0.0 {
                let delta = next * *count - *sum;
                *squares += delta * delta / (*count * previous_count);
                if sum.is_infinite() || squares.is_infinite() {
                    if !previous_sum.is_infinite() && !next.is_infinite() {
                        return Err(astersql_errors::SharedError::new(FloatSumOverflow));
                    }
                    *squares = f64::NAN;
                }
            } else if !next.is_finite() {
                *squares = f64::NAN;
            }
        }
        AggregateValue::FloatSum(sum) => {
            let next = if function.Name == "pg_sum_float4" {
                f64::from(values[0].GetFloat32())
            } else {
                values[0].GetFloat64()
            };
            let previous = sum.unwrap_or(0.0);
            // PostgreSQL uses the input width for each transition. Infinity from
            // finite operands is an error; pre-existing Infinity and NaN propagate.
            let result = if sum.is_none() {
                next
            } else if function.Name == "pg_sum_float4" {
                f64::from(previous as f32 + next as f32)
            } else {
                previous + next
            };
            if result.is_infinite() && previous.is_finite() && next.is_finite() {
                return Err(astersql_errors::SharedError::new(FloatSumOverflow));
            }
            *sum = Some(result);
        }

        AggregateValue::NumericAvg { sum, count } => {
            let next = values[0]
                .GetString()
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(astersql_errors::SharedError::new)?;
            *sum = Some(match sum.as_ref() {
                Some(previous) => previous
                    .add(&next)
                    .map_err(astersql_errors::SharedError::new)?,
                None => next,
            });
            *count = count
                .checked_add(1)
                .ok_or_else(|| astersql_errors::New("AVG bigint count out of range"))?;
        }
        AggregateValue::NumericSum(sum) => {
            let next = values[0]
                .GetString()
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(astersql_errors::SharedError::new)?;
            *sum = Some(match sum.as_ref() {
                Some(previous) => previous
                    .add(&next)
                    .map_err(astersql_errors::SharedError::new)?,
                None => next,
            });
        }
        AggregateValue::CountExtrema { .. } => unreachable!(),
        AggregateValue::Count(count) => {
            let increment = if matches!(
                function.Mode,
                AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
            ) {
                values[0].GetInt64()
            } else {
                1
            };
            *count = count
                .checked_add(increment)
                .ok_or_else(|| astersql_errors::New("COUNT bigint out of range"))?;
        }
        AggregateValue::Sum { value, count } => {
            if function.Name == astersql_parser_ast::AggFuncSumInt
                && !astersql_parser_mysql::r#type::HasUnsignedFlag(
                    function.RetTp.as_ref().unwrap().GetFlag(),
                )
            {
                let previous = if value.IsNull() { 0 } else { value.GetInt64() };
                let sum = previous
                    .checked_add(values[0].GetInt64())
                    .ok_or_else(|| astersql_errors::New("SUM bigint out of range"))?;
                *value = NewIntDatum(sum);
                return Ok(());
            }
            if function.Name == astersql_parser_ast::AggFuncAvg
                && matches!(
                    function.Mode,
                    AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
                )
            {
                *count = count.wrapping_add(values[0].GetInt64());
                *value = astersql_expression_aggregation::calculateSum(
                    eval.TypeCtx(),
                    value.clone(),
                    values[1].clone(),
                )
                .map_err(|error| astersql_errors::New(error.to_string()))?;
            } else {
                *value = astersql_expression_aggregation::calculateSum(
                    eval.TypeCtx(),
                    value.clone(),
                    values[0].clone(),
                )
                .map_err(|error| astersql_errors::New(error.to_string()))?;
                *count = count.wrapping_add(1);
            }
        }
        AggregateValue::First { seen, value } => {
            if !*seen {
                *seen = true;
                *value = values[0].clone();
            }
        }
        AggregateValue::Extremum(current) => {
            let candidate = &values[0];
            if current.IsNull() {
                *current = candidate.clone();
            } else {
                let collator =
                    astersql_util_collate::GetCollator(function.Args[0].GetType(eval).GetCollate());
                let comparison = candidate
                    .Compare(eval.TypeCtx(), current, collator.as_ref())
                    .map_err(|error| astersql_errors::New(error.to_string()))?;
                let replace = if function.Name == astersql_parser_ast::AggFuncMax {
                    comparison > 0
                } else {
                    comparison < 0
                };
                if replace {
                    *current = candidate.clone();
                }
            }
        }
    }
    Ok(())
}

fn finish_aggregate(
    function: &AggFuncDesc,
    state: &AggregateState,
    eval: &dyn astersql_expression_exprctx::EvalContext,
) -> AdapterResult<Datum> {
    match &state.value {
        AggregateValue::FloatAvg { count, sum, .. } => Ok(if *count == 0.0 {
            Datum::default()
        } else {
            NewFloat64Datum(*sum / *count)
        }),
        AggregateValue::FloatSum(sum) => Ok(sum.map_or_else(Datum::default, |value| {
            if function.Name == "pg_sum_float4" {
                astersql_types::datum::NewFloat32Datum(value as f32)
            } else {
                NewFloat64Datum(value)
            }
        })),
        AggregateValue::CountExtrema {
            function: evaluator,
            state,
        } => Ok(NewIntDatum(evaluator.result_count(state))),
        AggregateValue::Count(count) => Ok(NewIntDatum(*count)),
        AggregateValue::NumericAvg { sum, count } => {
            let Some(sum) = sum else {
                return Ok(Datum::default());
            };
            let divisor = count
                .to_string()
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(astersql_errors::SharedError::new)?;
            let average = sum
                .divide(&divisor)
                .map_err(astersql_errors::SharedError::new)?;
            Ok(astersql_types::datum::NewStringDatum(average.to_string()))
        }
        AggregateValue::NumericSum(value) => Ok(value.as_ref().map_or_else(Datum::default, |v| {
            astersql_types::datum::NewStringDatum(v.to_string())
        })),
        AggregateValue::Sum { value, count }
            if function.Name == astersql_parser_ast::AggFuncAvg =>
        {
            if *count == 0 {
                return Ok(Datum::default());
            }
            match value.Kind() {
                KindFloat64 => Ok(NewFloat64Datum(value.GetFloat64() / *count as f64)),
                KindMysqlDecimal => {
                    let mut quotient = MyDecimal::default();
                    DecimalDiv(
                        &value.GetMysqlDecimal(),
                        &NewDecFromInt(*count),
                        &mut quotient,
                        eval.GetDivPrecisionIncrement() as isize,
                    )
                    .map_err(|error| astersql_errors::New(error.to_string()))?;
                    Ok(NewDecimalDatum(quotient))
                }
                _ => Ok(Datum::default()),
            }
        }
        AggregateValue::Sum { value, count }
            if matches!(function.Mode, AggFunctionMode::Partial1Mode) =>
        {
            // AVG partial output is represented by two aggregate descriptors in
            // canonical plans; SUM and SUM_INT retain their scalar partial value.
            let _ = count;
            Ok(value.clone())
        }
        AggregateValue::Sum { value, .. } => Ok(value.clone()),
        AggregateValue::First { seen, value } => Ok(if *seen {
            value.clone()
        } else {
            Datum::default()
        }),
        AggregateValue::Extremum(value) => Ok(value.clone()),
    }
}

impl ExecExecutor for TypedHashAgg {
    fn Open(&mut self) -> AdapterResult {
        self.child.Open()?;
        self.groups.clear();
        self.group_indexes.clear();
        self.output_index = 0;
        self.prepared = false;
        self.opened = true;
        self.closed = false;
        Ok(())
    }

    fn Close(&mut self) -> AdapterResult {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.groups.clear();
        self.group_indexes.clear();
        self.child.Close()
    }

    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.next_inner(None, output)
    }

    fn NextWithContext(
        &mut self,
        context: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.next_inner(Some(context), output)
    }

    fn ChunkConfig(&self) -> ChunkConfig {
        let child = self.child.ChunkConfig();
        ChunkConfig {
            fields: self
                .schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect(),
            initial_capacity: child.initial_capacity,
            maximum_chunk_size: child.maximum_chunk_size,
        }
    }

    fn NewChunk(&self) -> chunk::Chunk {
        let config = self.ChunkConfig();
        *chunk::New(
            config.fields,
            config.initial_capacity,
            config.maximum_chunk_size,
        )
    }

    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        self.child.CheckForeignKeys()
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        self.child.TakeForeignKeyCascades()
    }
    fn HasForeignKeyCascades(&self) -> bool {
        self.child.HasForeignKeyCascades()
    }
    fn PrepareFKCascadeContext(&mut self) {
        self.child.PrepareFKCascadeContext()
    }
    fn AddFKCheckLockDuration(&mut self, duration: Duration) {
        self.child.AddFKCheckLockDuration(duration)
    }
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        Vec::new()
    }
    fn ScannedRows(&self) -> usize {
        self.child.ScannedRows()
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

/// Feed the already-bound relational COUNT argument into the canonical typed
/// transition/finalization path. Only nullness is relevant to non-DISTINCT COUNT.
/// The caller evaluates every argument before handing over this owned input.
pub fn CountNonNull(context: ContextRef, present: Vec<bool>) -> AdapterResult<i64> {
    let field_type =
        astersql_expression::types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    let input = CountInput {
        present,
        index: 0,
        opened: false,
        schema: vec![SchemaColumn {
            field_type: (*field_type).clone(),
        }],
    };
    let function = astersql_expression_aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        astersql_parser_ast::AggFuncCount,
        vec![Box::new(astersql_expression::Column::new(
            *field_type,
            1,
            1,
            0,
        ))],
        false,
    )
    .map_err(|e| astersql_errors::New(e.to_string()))?;
    let mut executor = TypedHashAgg::new(Box::new(input), vec![function], vec![], context)
        .map_err(astersql_errors::New)?;
    executor.Open()?;
    let mut output = executor.NewChunk();
    let result = executor
        .Next(&mut output)
        .map(|()| output.GetRow(0).GetInt64(0));
    let close = executor.Close();
    let count = result?;
    close?;
    Ok(count)
}

// This adapter owns no SQL semantics; it pages parameter nullness into HashAgg.
struct CountInput {
    present: Vec<bool>,
    index: usize,
    opened: bool,
    schema: Vec<SchemaColumn>,
}
impl ExecExecutor for CountInput {
    fn Open(&mut self) -> AdapterResult {
        self.index = 0;
        self.opened = true;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.opened = false;
        self.present.clear();
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        if !self.opened {
            return Err(astersql_errors::New("COUNT input is not open"));
        }
        while !output.IsFull() && self.index < self.present.len() {
            if self.present[self.index] {
                output.AppendInt64(0, 1);
            } else {
                output.AppendNull(0);
            }
            self.index += 1;
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig {
            fields: self.schema.iter().map(|c| c.field_type.clone()).collect(),
            initial_capacity: 32,
            maximum_chunk_size: 1024,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        let c = self.ChunkConfig();
        *chunk::New(c.fields, c.initial_capacity, c.maximum_chunk_size)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        vec![]
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

/// Evaluate the signed small-integer SUM overload through typed HashAgg.
pub fn SumSmallInt(context: ContextRef, values: Vec<Option<i64>>) -> AdapterResult<Option<i64>> {
    let field_type =
        astersql_expression::types::NewFieldType(astersql_parser_mysql::r#type::TypeLonglong);
    let input = SumInput {
        values: values
            .into_iter()
            .map(|v| v.map(|v| v.to_string()))
            .collect(),
        index: 0,
        opened: false,
        schema: vec![SchemaColumn {
            field_type: (*field_type).clone(),
        }],
    };
    let function = astersql_expression_aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        astersql_parser_ast::AggFuncSumInt,
        vec![Box::new(astersql_expression::Column::new(
            *field_type,
            1,
            1,
            0,
        ))],
        false,
    )
    .map_err(|e| astersql_errors::New(e.to_string()))?;
    let mut executor = TypedHashAgg::new(Box::new(input), vec![function], vec![], context)
        .map_err(astersql_errors::New)?;
    executor.Open()?;
    let mut output = executor.NewChunk();
    let result = executor
        .Next(&mut output)
        .map(|()| (!output.GetRow(0).IsNull(0)).then(|| output.GetRow(0).GetInt64(0)));
    let close = executor.Close();
    let count = result?;
    close?;
    Ok(count)
}

struct SumInput {
    values: Vec<Option<String>>,
    index: usize,
    opened: bool,
    schema: Vec<SchemaColumn>,
}
impl ExecExecutor for SumInput {
    fn Open(&mut self) -> AdapterResult {
        self.index = 0;
        self.opened = true;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        self.opened = false;
        self.values.clear();
        Ok(())
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        output.Reset();
        if !self.opened {
            return Err(astersql_errors::New("SUM input is not open"));
        }
        while !output.IsFull() && self.index < self.values.len() {
            if let Some(value) = &self.values[self.index] {
                if self.schema[0].field_type.GetType()
                    == astersql_parser_mysql::r#type::TypeLonglong
                {
                    output.AppendInt64(0, value.parse().expect("signed SUM input"));
                } else if self.schema[0].field_type.GetType()
                    == astersql_parser_mysql::r#type::TypeFloat
                {
                    let value = astersql_expression::pg_cast_float4(Some(value))
                        .map_err(astersql_errors::SharedError::new)?
                        .unwrap();
                    output.AppendFloat32(0, value);
                } else if self.schema[0].field_type.GetType()
                    == astersql_parser_mysql::r#type::TypeDouble
                {
                    let value = astersql_expression::pg_cast_float8(Some(value))
                        .map_err(astersql_errors::SharedError::new)?
                        .unwrap();
                    output.AppendFloat64(0, value);
                } else {
                    output.AppendString(0, value);
                }
            } else {
                output.AppendNull(0);
            }
            self.index += 1;
        }
        Ok(())
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        ChunkConfig {
            fields: self.schema.iter().map(|c| c.field_type.clone()).collect(),
            initial_capacity: 32,
            maximum_chunk_size: 1024,
        }
    }
    fn NewChunk(&self) -> chunk::Chunk {
        let c = self.ChunkConfig();
        *chunk::New(c.fields, c.initial_capacity, c.maximum_chunk_size)
    }
    fn Schema(&self) -> &[SchemaColumn] {
        &self.schema
    }
    fn CalculateNoDelay(&self) -> bool {
        false
    }
    fn IsWriteExecutor(&self) -> bool {
        false
    }
    fn CheckForeignKeys(&mut self) -> AdapterResult {
        Ok(())
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        vec![]
    }
    fn HasForeignKeyCascades(&self) -> bool {
        false
    }
    fn PrepareFKCascadeContext(&mut self) {}
    fn AddFKCheckLockDuration(&mut self, _: Duration) {}
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

/// PostgreSQL exact SUM uses numeric values across input chunks; the string
/// chunk representation transports their lossless text through the shared executor.
pub fn SumNumeric(
    context: ContextRef,
    values: Vec<Option<String>>,
) -> AdapterResult<Option<String>> {
    NumericAggregate(context, values, "pg_sum_numeric")
}

fn NumericAggregate(
    context: ContextRef,
    values: Vec<Option<String>>,
    name: &str,
) -> AdapterResult<Option<String>> {
    let field_type =
        astersql_expression::types::NewFieldType(astersql_parser_mysql::r#type::TypeVarString);
    let input = SumInput {
        values,
        index: 0,
        opened: false,
        schema: vec![SchemaColumn {
            field_type: (*field_type).clone(),
        }],
    };
    let mut function = astersql_expression_aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        astersql_parser_ast::AggFuncFirstRow,
        vec![Box::new(astersql_expression::Column::new(
            *field_type,
            1,
            1,
            0,
        ))],
        false,
    )
    .map_err(|e| astersql_errors::New(e.to_string()))?;
    function.Name = name.into();
    let mut executor = TypedHashAgg::new(Box::new(input), vec![function], vec![], context)
        .map_err(astersql_errors::New)?;
    executor.Open()?;
    let mut output = executor.NewChunk();
    let result = executor
        .Next(&mut output)
        .map(|()| (!output.GetRow(0).IsNull(0)).then(|| output.GetRow(0).GetString(0)));
    let close = executor.Close();
    let value = result?;
    close?;
    Ok(value)
}

/// PostgreSQL exact average over the shared typed executor.
pub fn AvgNumeric(
    context: ContextRef,
    values: Vec<Option<String>>,
) -> AdapterResult<Option<String>> {
    NumericAggregate(context, values, "pg_avg_numeric")
}

/// Finite PostgreSQL SUM operands exceeded their transition type's range.
#[derive(Debug)]
pub struct FloatSumOverflow;
impl std::fmt::Display for FloatSumOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("value out of range: overflow")
    }
}
impl std::error::Error for FloatSumOverflow {}

/// Evaluate a PostgreSQL floating SUM through the shared typed aggregation chain.
pub fn SumFloat(
    context: ContextRef,
    values: Vec<Option<String>>,
    single: bool,
) -> AdapterResult<Option<f64>> {
    FloatingAggregate(context, values, single, false)
}

/// PostgreSQL floating average, with float8 transition and result for both inputs.
pub fn AvgFloat(
    context: ContextRef,
    values: Vec<Option<String>>,
    single: bool,
) -> AdapterResult<Option<f64>> {
    FloatingAggregate(context, values, single, true)
}

fn FloatingAggregate(
    context: ContextRef,
    values: Vec<Option<String>>,
    single: bool,
    average: bool,
) -> AdapterResult<Option<f64>> {
    let tp = if single {
        astersql_parser_mysql::r#type::TypeFloat
    } else {
        astersql_parser_mysql::r#type::TypeDouble
    };
    let ft = *astersql_expression::types::NewFieldType(tp);
    let input = SumInput {
        values,
        index: 0,
        opened: false,
        schema: vec![SchemaColumn {
            field_type: ft.clone(),
        }],
    };
    let mut function = astersql_expression_aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        astersql_parser_ast::AggFuncFirstRow,
        vec![Box::new(astersql_expression::Column::new(ft, 1, 1, 0))],
        false,
    )
    .map_err(|e| astersql_errors::New(e.to_string()))?;
    function.Name = if average && single {
        "pg_avg_float4"
    } else if average {
        "pg_avg_float8"
    } else if single {
        "pg_sum_float4"
    } else {
        "pg_sum_float8"
    }
    .into();
    if average {
        function.RetTp = Some(*astersql_expression::types::NewFieldType(
            astersql_parser_mysql::r#type::TypeDouble,
        ));
    }
    let mut executor = TypedHashAgg::new(Box::new(input), vec![function], vec![], context)
        .map_err(astersql_errors::New)?;
    executor.Open()?;
    let mut output = executor.NewChunk();
    let result = executor.Next(&mut output).map(|()| {
        (!output.GetRow(0).IsNull(0)).then(|| {
            if single && !average {
                f64::from(output.GetRow(0).GetFloat32(0))
            } else {
                output.GetRow(0).GetFloat64(0)
            }
        })
    });
    let close = executor.Close();
    let value = result?;
    close?;
    Ok(value)
}
