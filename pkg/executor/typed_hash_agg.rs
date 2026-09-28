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

#[derive(Clone)]
enum AggregateValue {
    Count(i64),
    Sum { value: Datum, count: i64 },
    First { seen: bool, value: Datum },
    Extremum(Datum),
}

struct AggregateState {
    value: AggregateValue,
    distinct: Option<HashSet<Vec<u8>>>,
}

struct GroupState {
    aggregates: Vec<AggregateState>,
}

/// Typed HashAgg consumes child chunks incrementally and retains only one
/// aggregate state per group. Input rows and their lock keys are never retained.
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
                astersql_parser_ast::AggFuncSum
                | astersql_parser_ast::AggFuncSumInt
                | astersql_parser_ast::AggFuncFirstRow
                | astersql_parser_ast::AggFuncMax
                | astersql_parser_ast::AggFuncMin => {
                    if function.Args.is_empty() {
                        return Err(format!("typed {} requires one argument", function.Name));
                    }
                }
                name => return Err(format!("typed HashAgg does not support aggregate {name}")),
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
                        _ => unreachable!("aggregate names are validated by new"),
                    },
                    distinct: function.HasDistinct.then(HashSet::new),
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

fn update_aggregate(
    function: &AggFuncDesc,
    state: &mut AggregateState,
    eval: &dyn astersql_expression_exprctx::EvalContext,
    row: chunk::Row,
) -> AdapterResult {
    let values = evaluated_arguments(function, eval, row)?;
    if function.Name != astersql_parser_ast::AggFuncFirstRow && values.iter().any(Datum::IsNull) {
        return Ok(());
    }
    if let Some(distinct) = &mut state.distinct {
        let key = astersql_util_codec::EncodeKey(eval.Location(), Vec::new(), values.clone())
            .map_err(|error| astersql_errors::New(error.to_string()))?;
        if !distinct.insert(key) {
            return Ok(());
        }
    }
    match &mut state.value {
        AggregateValue::Count(count) => {
            if matches!(
                function.Mode,
                AggFunctionMode::FinalMode | AggFunctionMode::Partial2Mode
            ) {
                *count = count.wrapping_add(values[0].GetInt64());
            } else {
                *count = count.wrapping_add(1);
            }
        }
        AggregateValue::Sum { value, count } => {
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
        AggregateValue::Count(count) => Ok(NewIntDatum(*count)),
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
