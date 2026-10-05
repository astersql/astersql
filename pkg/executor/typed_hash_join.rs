// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::time::Duration;

use astersql_planner_core_base::{ContextRef, JoinType};
use astersql_types::datum::{Datum, NewIntDatum};
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, Key, SchemaColumn,
};

struct StoredRow {
    values: Vec<Datum>,
    matched: bool,
}

/// A typed hash join which materializes only its build child. The probe child
/// remains chunk-driven, and pending matches are resumed across output pages.
pub struct TypedHashJoin {
    left: Box<dyn ExecExecutor>,
    right: Box<dyn ExecExecutor>,
    left_keys: Vec<astersql_expression::Column>,
    right_keys: Vec<astersql_expression::Column>,
    null_equal: Vec<bool>,
    left_conditions: Vec<astersql_expression::ExprBox>,
    right_conditions: Vec<astersql_expression::ExprBox>,
    other_conditions: Vec<astersql_expression::ExprBox>,
    context: ContextRef,
    join_type: JoinType,
    schema: Vec<SchemaColumn>,
    build_rows: Vec<StoredRow>,
    buckets: HashMap<Vec<u8>, Vec<usize>>,
    probe_chunk: chunk::Chunk,
    probe_index: usize,
    pending_probe: Option<Vec<Datum>>,
    pending_matches: Vec<usize>,
    pending_index: usize,
    pending_matched: bool,
    pending_null: bool,
    unmatched_build_index: usize,
    probe_done: bool,
    page_keys: Vec<Key>,
    opened: bool,
    closed: bool,
}

impl TypedHashJoin {
    pub fn new(
        left: Box<dyn ExecExecutor>,
        right: Box<dyn ExecExecutor>,
        left_keys: Vec<astersql_expression::Column>,
        right_keys: Vec<astersql_expression::Column>,
        null_equal: Vec<bool>,
        left_conditions: Vec<astersql_expression::ExprBox>,
        right_conditions: Vec<astersql_expression::ExprBox>,
        other_conditions: Vec<astersql_expression::ExprBox>,
        context: ContextRef,
        join_type: JoinType,
    ) -> Result<Self, String> {
        if left_keys.len() != right_keys.len() {
            return Err("typed HashJoin key counts differ".into());
        }
        if !null_equal.is_empty() && null_equal.len() != left_keys.len() {
            return Err("typed HashJoin NULL equality count differs from key count".into());
        }
        let left_schema = left.Schema().to_vec();
        let right_schema = right.Schema().to_vec();
        let schema = match join_type {
            JoinType::SemiJoin | JoinType::AntiSemiJoin => left_schema.clone(),
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin => {
                let mut output = left_schema.clone();
                output.push(SchemaColumn {
                    field_type: astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeLonglong,
                    ),
                });
                output
            }
            _ => left_schema.iter().chain(&right_schema).cloned().collect(),
        };
        let probe_chunk = left.NewChunk();
        Ok(Self {
            left,
            right,
            left_keys,
            right_keys,
            null_equal,
            left_conditions,
            right_conditions,
            other_conditions,
            context,
            join_type,
            schema,
            build_rows: Vec::new(),
            buckets: HashMap::new(),
            probe_chunk,
            probe_index: 0,
            pending_probe: None,
            pending_matches: Vec::new(),
            pending_index: 0,
            pending_matched: false,
            pending_null: false,
            unmatched_build_index: 0,
            probe_done: false,
            page_keys: Vec::new(),
            opened: false,
            closed: false,
        })
    }

    fn check_cancel(context: Option<&ExecutionContext>) -> AdapterResult {
        if let Some(killer) = context.and_then(|context| context.sql_killer.as_ref()) {
            killer.HandleSignal()?;
        }
        Ok(())
    }

    fn row_values(row: chunk::Row, schema: &[SchemaColumn]) -> Vec<Datum> {
        row.GetDatumRow(
            &schema
                .iter()
                .map(|column| column.field_type.clone())
                .collect::<Vec<_>>(),
        )
    }

    fn key(
        &self,
        row: chunk::Row,
        keys: &[astersql_expression::Column],
    ) -> AdapterResult<Option<Vec<u8>>> {
        let eval = self.context.GetExprCtx().GetEvalCtx();
        let mut values = Vec::with_capacity(keys.len());
        for (index, key) in keys.iter().enumerate() {
            let value = key
                .Eval(eval, row.clone())
                .map_err(|error| astersql_errors::New(error.to_string()))?;
            if value.IsNull() && !self.null_equal.get(index).copied().unwrap_or(false) {
                return Ok(None);
            }
            values.push(value);
        }
        astersql_util_codec::EncodeKey(eval.Location(), Vec::new(), values)
            .map(Some)
            .map_err(|error| astersql_errors::New(error.to_string()))
    }

    fn values_chunk(&self, values: &[Datum]) -> chunk::Chunk {
        let fields = self
            .left
            .Schema()
            .iter()
            .chain(self.right.Schema())
            .map(|column| column.field_type.clone())
            .collect();
        let mut result = *chunk::New(fields, 1, 1);
        for (index, value) in values.iter().enumerate() {
            result.AppendDatum(index, value);
        }
        result
    }

    fn conditions_match(&self, values: &[Datum]) -> AdapterResult<(bool, bool)> {
        if self.other_conditions.is_empty() {
            return Ok((true, false));
        }
        let joined = self.values_chunk(values);
        let expressions = astersql_expression::CNFExprs(
            self.other_conditions
                .iter()
                .map(|expression| expression.CloneExpr())
                .collect(),
        );
        astersql_expression::EvalBool(
            self.context.GetExprCtx().GetEvalCtx(),
            &expressions,
            joined.GetRow(0),
        )
        .map_err(|error| astersql_errors::New(error.to_string()))
    }

    fn side_conditions_match(
        &self,
        row: chunk::Row,
        conditions: &[astersql_expression::ExprBox],
    ) -> AdapterResult<bool> {
        if conditions.is_empty() {
            return Ok(true);
        }
        let expressions = astersql_expression::CNFExprs(
            conditions
                .iter()
                .map(|expression| expression.CloneExpr())
                .collect(),
        );
        astersql_expression::EvalBool(self.context.GetExprCtx().GetEvalCtx(), &expressions, row)
            .map(|(matched, _)| matched)
            .map_err(|error| astersql_errors::New(error.to_string()))
    }

    fn append(&self, output: &mut chunk::Chunk, values: &[Datum]) {
        for (index, value) in values.iter().enumerate() {
            output.AppendDatum(index, value);
        }
    }

    fn joined_values(&self, left: &[Datum], right: &[Datum]) -> Vec<Datum> {
        left.iter().chain(right).cloned().collect()
    }

    fn build(&mut self, context: Option<&ExecutionContext>) -> AdapterResult {
        let mut input = self.right.NewChunk();
        loop {
            Self::check_cancel(context)?;
            if let Some(context) = context {
                self.right.NextWithContext(context, &mut input)?;
            } else {
                self.right.Next(&mut input)?;
            }
            let _ = self.right.TakeLockKeys();
            if input.NumRows() == 0 {
                break;
            }
            for index in 0..input.NumRows() {
                let row = input.GetRow(index);
                let key = self.key(row.clone(), &self.right_keys)?;
                let selected = self.side_conditions_match(row.clone(), &self.right_conditions)?;
                let stored = self.build_rows.len();
                self.build_rows.push(StoredRow {
                    values: Self::row_values(row, self.right.Schema()),
                    matched: false,
                });
                if selected {
                    if let Some(key) = key {
                        self.buckets.entry(key).or_default().push(stored);
                    }
                }
            }
        }
        Ok(())
    }

    fn finish_probe(&mut self, output: &mut chunk::Chunk) {
        let Some(probe) = self.pending_probe.take() else {
            return;
        };
        match self.join_type {
            JoinType::LeftOuterJoin | JoinType::FullOuterJoin if !self.pending_matched => {
                let mut values = probe;
                values.extend((0..self.right.Schema().len()).map(|_| Datum::default()));
                self.append(output, &values);
            }
            JoinType::SemiJoin if self.pending_matched => self.append(output, &probe),
            JoinType::AntiSemiJoin if !self.pending_matched && !self.pending_null => {
                self.append(output, &probe)
            }
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin => {
                let value = match (self.pending_matched, self.pending_null) {
                    (true, _) => Some(self.join_type == JoinType::LeftOuterSemiJoin),
                    (false, true) => None,
                    (false, false) => Some(self.join_type == JoinType::AntiLeftOuterSemiJoin),
                };
                let mut values = probe;
                values.push(value.map_or_else(Datum::default, |value| NewIntDatum(value as i64)));
                self.append(output, &values);
            }
            _ => {}
        }
        self.pending_matches.clear();
        self.pending_index = 0;
        self.pending_matched = false;
        self.pending_null = false;
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        if !self.opened || self.closed {
            return Err(astersql_errors::New("hash join executor is not open"));
        }
        output.Reset();
        self.page_keys.clear();
        while !output.IsFull() {
            Self::check_cancel(context)?;
            if self.pending_probe.is_some() {
                while self.pending_index < self.pending_matches.len() && !output.IsFull() {
                    let build_index = self.pending_matches[self.pending_index];
                    self.pending_index += 1;
                    let joined = self.joined_values(
                        self.pending_probe.as_ref().expect("pending probe"),
                        &self.build_rows[build_index].values,
                    );
                    let (matched, is_null) = self.conditions_match(&joined)?;
                    self.pending_null |= is_null;
                    if !matched {
                        continue;
                    }
                    self.pending_matched = true;
                    self.build_rows[build_index].matched = true;
                    if matches!(
                        self.join_type,
                        JoinType::InnerJoin
                            | JoinType::LeftOuterJoin
                            | JoinType::RightOuterJoin
                            | JoinType::FullOuterJoin
                    ) {
                        self.append(output, &joined);
                    }
                    if matches!(
                        self.join_type,
                        JoinType::SemiJoin
                            | JoinType::AntiSemiJoin
                            | JoinType::LeftOuterSemiJoin
                            | JoinType::AntiLeftOuterSemiJoin
                    ) {
                        self.pending_index = self.pending_matches.len();
                    }
                }
                if self.pending_index == self.pending_matches.len() && !output.IsFull() {
                    self.finish_probe(output);
                }
                if output.IsFull() {
                    break;
                }
                continue;
            }
            if !self.probe_done {
                if self.probe_index >= self.probe_chunk.NumRows() {
                    if let Some(context) = context {
                        self.left.NextWithContext(context, &mut self.probe_chunk)?;
                    } else {
                        self.left.Next(&mut self.probe_chunk)?;
                    }
                    self.probe_index = 0;
                    let _ = self.left.TakeLockKeys();
                    if self.probe_chunk.NumRows() == 0 {
                        self.probe_done = true;
                        continue;
                    }
                }
                let row = self.probe_chunk.GetRow(self.probe_index);
                self.probe_index += 1;
                let key = self.key(row.clone(), &self.left_keys)?;
                let probe_selected =
                    self.side_conditions_match(row.clone(), &self.left_conditions)?;
                self.pending_probe = Some(Self::row_values(row, self.left.Schema()));
                self.pending_matches = if probe_selected {
                    key.as_ref()
                        .and_then(|key| self.buckets.get(key))
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                continue;
            }
            if matches!(
                self.join_type,
                JoinType::RightOuterJoin | JoinType::FullOuterJoin
            ) {
                while self.unmatched_build_index < self.build_rows.len() && !output.IsFull() {
                    let index = self.unmatched_build_index;
                    self.unmatched_build_index += 1;
                    if self.build_rows[index].matched {
                        continue;
                    }
                    let mut values = vec![Datum::default(); self.left.Schema().len()];
                    values.extend(self.build_rows[index].values.clone());
                    self.append(output, &values);
                }
            }
            break;
        }
        Ok(())
    }
}

impl ExecExecutor for TypedHashJoin {
    fn Open(&mut self) -> AdapterResult {
        self.left.Open()?;
        if let Err(error) = self.right.Open() {
            let _ = self.left.Close();
            return Err(error);
        }
        self.build_rows.clear();
        self.buckets.clear();
        self.probe_chunk.Reset();
        self.probe_index = 0;
        self.pending_probe = None;
        self.pending_matches.clear();
        self.pending_index = 0;
        self.unmatched_build_index = 0;
        self.probe_done = false;
        self.opened = true;
        self.closed = false;
        if let Err(error) = self.build(None) {
            let _ = self.Close();
            return Err(error);
        }
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let left = self.left.Close();
        let right = self.right.Close();
        self.build_rows.clear();
        self.buckets.clear();
        left.and(right)
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
        let child = self.left.ChunkConfig();
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
        self.left.CheckForeignKeys()?;
        self.right.CheckForeignKeys()
    }
    fn TakeForeignKeyCascades(&mut self) -> Vec<Box<dyn CascadeBatch>> {
        let mut result = self.left.TakeForeignKeyCascades();
        result.extend(self.right.TakeForeignKeyCascades());
        result
    }
    fn HasForeignKeyCascades(&self) -> bool {
        self.left.HasForeignKeyCascades() || self.right.HasForeignKeyCascades()
    }
    fn PrepareFKCascadeContext(&mut self) {
        self.left.PrepareFKCascadeContext();
        self.right.PrepareFKCascadeContext();
    }
    fn AddFKCheckLockDuration(&mut self, duration: Duration) {
        self.left.AddFKCheckLockDuration(duration);
        self.right.AddFKCheckLockDuration(duration);
    }
    fn TakeLockKeys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.page_keys)
    }
    fn ScannedRows(&self) -> usize {
        self.left.ScannedRows() + self.right.ScannedRows()
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
