// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use astersql_parser_mysql::r#type::{
    TypeBlob, TypeLongBlob, TypeMediumBlob, TypeString, TypeTinyBlob, TypeVarString, TypeVarchar,
    UnsignedFlag,
};
use astersql_parser_types::FieldType;
use astersql_planner_core::{MViewAggInfo, MViewAggKind, MViewDeltaMerge, PlanNode};
use astersql_types::datum::{Datum, NewIntDatum};
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, SchemaColumn,
};
use crate::builder::{BuildError, ExecutorBox, InfoSchema, Snapshot};
use crate::mview_complete_delta_apply::MutationRuntime;

pub struct ReaderSnapshot {
    pub ts: u64,
    pub snapshot: Mutex<Box<dyn Snapshot>>,
    pub info_schema: Arc<dyn InfoSchema>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecomputeStrategy {
    SingleRow,
    Batch,
}

/// Lookup ranges and keys are copied for each build; readers must never mutate
/// the planner's template. Result columns retain the planner's physical order.
pub struct LookupRequest {
    pub keys: Vec<Vec<Datum>>,
    pub ranges: Vec<Vec<String>>,
    pub key_to_index: Vec<usize>,
}

/// Statement resources, not an executor-building hook. Reads must use the given
/// snapshot and bind keys to the supplied index-range positions before scanning.
pub trait Resources: MutationRuntime {
    fn target_field_types(&self) -> Vec<FieldType>;
    /// Resolve actual table metadata; absence must fail closed for COMPLETE APPLY.
    fn complete_delta_target_columns(
        &self,
    ) -> Result<crate::mview_complete_delta_apply_builder::TargetColumns, BuildError> {
        Err(BuildError::new(
            "MViewCompleteDeltaApply target column metadata unavailable",
        ))
    }
    fn read_full_update(
        &self,
        source: &PlanNode,
        snapshot: &ReaderSnapshot,
        request: &LookupRequest,
    ) -> Result<ExecutorBox, BuildError>;
    fn recompute_strategy(&self) -> RecomputeStrategy {
        RecomputeStrategy::Batch
    }
}

#[derive(Clone, Debug)]
pub struct AggregateMapping {
    pub kind: MViewAggKind,
    pub output: usize,
    pub dependencies: Vec<usize>,
    pub nullable: bool,
}

fn error(message: impl Into<String>) -> BuildError {
    BuildError::new(message)
}
fn signed(field: &FieldType) -> bool {
    field.EvalType() == astersql_parser_types::ETInt && field.GetFlag() & UnsignedFlag == 0
}

/// Kahn ordering with COUNT(*) at the head, matching the Go builder. Every
/// computed dependency must name an output; duplicate edges count only once.
pub fn mappings(
    infos: &[MViewAggInfo],
    fields: &[FieldType],
    delta: usize,
    columns: usize,
) -> Result<Vec<AggregateMapping>, BuildError> {
    let mut outputs = BTreeMap::new();
    let mut result = Vec::new();
    for info in infos {
        if info.mv_offset >= columns {
            return Err(error("aggregate mview offset out of range"));
        }
        let output = delta
            .checked_add(info.mv_offset)
            .ok_or_else(|| error("aggregate output overflow"))?;
        if output >= fields.len() {
            return Err(error("aggregate output outside source schema"));
        }
        if outputs.insert(output, result.len()).is_some() {
            return Err(error("duplicate aggregate output"));
        }
        for dep in &info.dependencies {
            if *dep >= fields.len() {
                return Err(error("aggregate dependency out of range"));
            }
        }
        result.push(AggregateMapping {
            kind: info.kind,
            output,
            dependencies: info.dependencies.clone(),
            nullable: !info.arg_not_null,
        });
    }
    if result.is_empty() {
        return Ok(result);
    }
    let star = result
        .iter()
        .position(|m| m.kind == MViewAggKind::CountStar)
        .ok_or_else(|| error("aggregate mappings require COUNT(*)"))?;
    let mut edges = vec![Vec::new(); result.len()];
    let mut indegree = vec![0; result.len()];
    for (to, mapping) in result.iter().enumerate() {
        for dep in mapping
            .dependencies
            .iter()
            .copied()
            .filter(|dep| *dep >= delta)
            .collect::<BTreeSet<_>>()
        {
            let from = *outputs
                .get(&dep)
                .ok_or_else(|| error("dependency does not map to any aggregate output"))?;
            edges[from].push(to);
            indegree[to] += 1;
        }
    }
    if indegree[star] != 0 {
        return Err(error("COUNT(*) has non-zero indegree"));
    }
    let mut queue = VecDeque::from([star]);
    queue.extend((0..result.len()).filter(|i| *i != star && indegree[*i] == 0));
    let mut ordered = Vec::new();
    while let Some(i) = queue.pop_front() {
        ordered.push(result[i].clone());
        for to in &edges[i] {
            indegree[*to] -= 1;
            if indegree[*to] == 0 {
                queue.push_back(*to);
            }
        }
    }
    if ordered.len() != result.len() {
        return Err(error("aggregate dependencies are cyclic"));
    }
    for m in &ordered {
        let d = &m.dependencies;
        let expected = match m.kind {
            MViewAggKind::CountStar | MViewAggKind::Count => d.len() == 1,
            MViewAggKind::Sum => (1..=2).contains(&d.len()),
            MViewAggKind::Min | MViewAggKind::Max => d.len() == 4 || (m.nullable && d.len() == 5),
        };
        if !expected {
            return Err(error("aggregate dependency count mismatch"));
        }
        let inputs = if matches!(m.kind, MViewAggKind::Min | MViewAggKind::Max) {
            4
        } else {
            1
        };
        if d[..inputs].iter().any(|dep| *dep >= delta) {
            return Err(error("delta dependency comes from computed side"));
        }
        if d.len() > inputs {
            let dep = d[inputs];
            if dep < delta || !signed(&fields[dep]) {
                return Err(error("final count must be a signed computed dependency"));
            }
            if !matches!(
                result[*outputs
                    .get(&dep)
                    .ok_or_else(|| error("missing count dependency"))?]
                .kind,
                MViewAggKind::Count | MViewAggKind::CountStar
            ) {
                return Err(error("final count dependency is not COUNT"));
            }
        }
        match m.kind {
            MViewAggKind::CountStar | MViewAggKind::Count => {
                if !signed(&fields[d[0]])
                    || !signed(&fields[m.output])
                    || fields[m.output].GetFlag() & astersql_parser_mysql::r#type::NotNullFlag != 0
                {
                    return Err(error("COUNT requires signed integer types"));
                }
            }
            MViewAggKind::Sum => {
                let tp = fields[m.output].EvalType();
                if tp != fields[d[0]].EvalType()
                    || !matches!(
                        tp,
                        astersql_parser_types::ETInt
                            | astersql_parser_types::ETReal
                            | astersql_parser_types::ETDecimal
                    )
                {
                    return Err(error("SUM value type mismatch"));
                }
                if tp == astersql_parser_types::ETInt && fields[d[0]].GetFlag() & UnsignedFlag != 0
                {
                    return Err(error("SUM delta must be signed"));
                }
            }
            MViewAggKind::Min | MViewAggKind::Max => {
                if !matches!(
                    fields[m.output].EvalType(),
                    astersql_parser_types::ETInt
                        | astersql_parser_types::ETReal
                        | astersql_parser_types::ETDecimal
                        | astersql_parser_types::ETString
                        | astersql_parser_types::ETDatetime
                        | astersql_parser_types::ETTimestamp
                        | astersql_parser_types::ETDuration
                ) {
                    return Err(error("MIN/MAX unsupported value type"));
                }
                if fields[m.output].EvalType() == astersql_parser_types::ETString
                    && !matches!(
                        fields[m.output].GetType(),
                        TypeVarchar
                            | TypeVarString
                            | TypeString
                            | TypeBlob
                            | TypeTinyBlob
                            | TypeMediumBlob
                            | TypeLongBlob
                    )
                {
                    return Err(error("MIN/MAX unsupported string type"));
                }
                if !fields[m.output].Equal(&fields[d[0]]) || !fields[m.output].Equal(&fields[d[2]])
                {
                    return Err(error("MIN/MAX value type mismatch"));
                }
                if !signed(&fields[d[1]]) || !signed(&fields[d[3]]) {
                    return Err(error("MIN/MAX counts must be signed"));
                }
            }
        }
    }
    Ok(ordered)
}

#[derive(Clone, Debug)]
pub struct RecomputeLayout {
    pub key_inputs: Vec<usize>,
    pub key_results: Vec<usize>,
    pub results: BTreeMap<usize, usize>,
}

pub fn recompute_layout(
    plan: &MViewDeltaMerge,
    mappings: &[AggregateMapping],
) -> Result<Option<RecomputeLayout>, BuildError> {
    let minmax: BTreeSet<_> = mappings
        .iter()
        .filter(|m| matches!(m.kind, MViewAggKind::Min | MViewAggKind::Max))
        .map(|m| m.output - plan.delta_column_count)
        .collect();
    if minmax.is_empty() {
        return Ok(None);
    }
    if plan.full_update_inner_source.is_none() {
        return Err(error("MIN/MAX requires full-update inner source"));
    }
    let count = plan.full_update_inner_column_count;
    if count == 0 || count != plan.full_update_output_mv_offsets.len() {
        return Err(error("full-update output layout length mismatch"));
    }
    if plan.full_update_index_ranges.is_empty()
        || plan
            .full_update_index_ranges
            .iter()
            .any(|range| range.is_empty() || range.iter().any(String::is_empty))
    {
        return Err(error("full-update index ranges are empty"));
    }
    let keys = plan.group_key_mv_offsets.len();
    if keys == 0
        || keys != plan.full_update_key_off_to_index_off.len()
        || keys != plan.full_update_key_result_col_indexes.len()
    {
        return Err(error("full-update key mapping length mismatch"));
    }
    let mut seen = BTreeSet::new();
    for index in &plan.full_update_key_result_col_indexes {
        if *index >= count || !seen.insert(*index) {
            return Err(error("full-update key result invalid or duplicate"));
        }
    }
    seen.clear();
    for index in &plan.full_update_key_off_to_index_off {
        if !seen.insert(*index)
            || plan
                .full_update_index_ranges
                .iter()
                .any(|range| *index >= range.len())
        {
            return Err(error(
                "full-update key to index mapping invalid or duplicate",
            ));
        }
    }
    let mut results = BTreeMap::new();
    for (result, offset) in plan.full_update_output_mv_offsets.iter().enumerate() {
        if *offset >= plan.mv_column_count {
            return Err(error("full-update output mview offset out of range"));
        }
        if minmax.contains(offset)
            && results
                .insert(*offset + plan.delta_column_count, result)
                .is_some()
        {
            return Err(error("duplicate MIN/MAX result output"));
        }
    }
    if results.len() != minmax.len() {
        return Err(error("MIN/MAX result layout missing output"));
    }
    let mut key_inputs = Vec::new();
    let mut seen_keys = BTreeSet::new();
    for (i, offset) in plan.group_key_mv_offsets.iter().enumerate() {
        if *offset >= plan.mv_column_count
            || !seen_keys.insert(*offset)
            || plan.full_update_output_mv_offsets[plan.full_update_key_result_col_indexes[i]]
                != *offset
        {
            return Err(error("group key result layout mismatch"));
        }
        key_inputs.push(plan.delta_column_count + offset);
    }
    Ok(Some(RecomputeLayout {
        key_inputs,
        key_results: plan.full_update_key_result_col_indexes.clone(),
        results,
    }))
}

pub fn build(
    plan: &MViewDeltaMerge,
    child: ExecutorBox,
    resources: Arc<dyn Resources>,
    reader: ReaderSnapshot,
) -> Result<ExecutorBox, BuildError> {
    if reader.ts == 0 {
        return Err(error("snapshot TS is zero"));
    }
    let fields: Vec<_> = child
        .Schema()
        .iter()
        .map(|c| c.field_type.clone())
        .collect();
    if plan.mv_table_id == 0
        || plan.mv_column_count == 0
        || fields.len() < plan.delta_column_count + plan.mv_column_count
    {
        return Err(error("MView source/target schema mismatch"));
    }
    if plan.mv_table_pk_cols.is_empty()
        || plan
            .mv_table_pk_cols
            .iter()
            .any(|c| *c < plan.delta_column_count || *c >= fields.len())
    {
        return Err(error("MView target handle columns invalid"));
    }
    let target_fields = resources.target_field_types();
    if target_fields.len() != plan.mv_column_count
        || target_fields
            .iter()
            .zip(&fields[plan.delta_column_count..])
            .any(|(target, input)| !target.Equal(input))
    {
        return Err(error("MView target writable schema type mismatch"));
    }
    let mappings = mappings(
        &plan.agg_infos,
        &fields,
        plan.delta_column_count,
        plan.mv_column_count,
    )?;
    if mappings
        .first()
        .is_none_or(|m| m.output != plan.delta_column_count + plan.count_star_mv_offset)
    {
        return Err(error("COUNT(*) output layout mismatch"));
    }
    let layout = recompute_layout(plan, &mappings)?;
    Ok(Box::new(DeltaMergeExec {
        plan: plan.clone(),
        child,
        resources,
        reader,
        mappings,
        layout,
        opened: false,
        done: false,
        schema: Vec::new(),
    }))
}

pub struct DeltaMergeExec {
    plan: MViewDeltaMerge,
    child: ExecutorBox,
    resources: Arc<dyn Resources>,
    reader: ReaderSnapshot,
    mappings: Vec<AggregateMapping>,
    layout: Option<RecomputeLayout>,
    opened: bool,
    done: bool,
    schema: Vec<SchemaColumn>,
}

fn same(a: &[Datum], b: &[Datum]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.Equals(b))
}
fn count(value: &Datum) -> Result<i64, String> {
    if value.Kind() != astersql_types::datum::KindInt64 || value.GetInt64() < 0 {
        return Err("COUNT must be a nonnegative signed integer".into());
    }
    Ok(value.GetInt64())
}
fn compare(a: &Datum, b: &Datum, field: &FieldType) -> Result<i32, String> {
    a.Compare(
        astersql_types::DefaultStmtNoWarningContext.clone(),
        b,
        astersql_util_collate::GetCollator(field.GetCollate()).as_ref(),
    )
    .map_err(|e| e.to_string())
}

#[derive(Debug, PartialEq)]
pub(crate) enum MinMaxDecision {
    Old,
    Added,
    Recompute,
}

/// The same three-way add/remove dominance decision used by Go's typed mergers.
/// In particular a canceled extremum that outranks old requires a lookup: the
/// delta does not carry the next-best value.
pub(crate) fn minmax_decision(
    old: &Datum,
    add: &Datum,
    remove: &Datum,
    add_count: i64,
    remove_count: i64,
    kind: MViewAggKind,
    field: &FieldType,
) -> Result<MinMaxDecision, String> {
    let better = |a: &Datum, b: &Datum| -> Result<bool, String> {
        let c = compare(a, b, field)?;
        Ok(if kind == MViewAggKind::Min {
            c < 0
        } else {
            c > 0
        })
    };
    let mut equal_values = false;
    let dominance = match (add.IsNull(), remove.IsNull()) {
        (true, true) => 0,
        (false, true) => 1,
        (true, false) => -1,
        (false, false) => {
            let order = compare(add, remove, field)?;
            if order == 0 {
                equal_values = true;
                add_count.cmp(&remove_count) as i32
            } else if better(add, remove)? {
                1
            } else {
                -1
            }
        }
    };
    if dominance == 0 {
        if equal_values && (old.IsNull() || better(add, old)?) {
            Ok(MinMaxDecision::Recompute)
        } else {
            Ok(MinMaxDecision::Old)
        }
    } else if dominance > 0 {
        if old.IsNull() || better(add, old)? {
            Ok(MinMaxDecision::Added)
        } else {
            Ok(MinMaxDecision::Old)
        }
    } else {
        if old.IsNull() || better(remove, old)? {
            return Err("invalid MIN/MAX remove-dominant state".into());
        }
        if compare(remove, old, field)? == 0 {
            Ok(MinMaxDecision::Recompute)
        } else {
            Ok(MinMaxDecision::Old)
        }
    }
}

impl DeltaMergeExec {
    fn recompute(
        &self,
        rows: &mut [Vec<Datum>],
        needed: &[(usize, usize)],
        context: Option<&ExecutionContext>,
    ) -> Result<(), String> {
        if needed.is_empty() {
            return Ok(());
        }
        let layout = self
            .layout
            .as_ref()
            .ok_or("MIN/MAX recompute layout is absent")?;
        let mut keys: Vec<Vec<Datum>> = Vec::new();
        for (row, _) in needed {
            let key: Vec<_> = layout
                .key_inputs
                .iter()
                .map(|i| rows[*row][*i].clone())
                .collect();
            if !keys.iter().any(|k| same(k, &key)) {
                keys.push(key);
            }
        }
        let size = match self.resources.recompute_strategy() {
            RecomputeStrategy::Batch => keys.len(),
            RecomputeStrategy::SingleRow => 1,
        };
        let mut found: Vec<(Vec<Datum>, Vec<Datum>)> = Vec::new();
        for batch in keys.chunks(size) {
            let request = LookupRequest {
                keys: batch.to_vec(),
                ranges: self.plan.full_update_index_ranges.clone(),
                key_to_index: self.plan.full_update_key_off_to_index_off.clone(),
            };
            let mut exec = self
                .resources
                .read_full_update(
                    self.plan
                        .full_update_inner_source
                        .as_ref()
                        .ok_or("missing full-update source")?,
                    &self.reader,
                    &request,
                )
                .map_err(|e| e.to_string())?;
            let schema_valid = exec.Schema().len() == self.plan.full_update_inner_column_count
                && layout.results.iter().all(|(input, result)| {
                    self.child.Schema()[*input]
                        .field_type
                        .Equal(&exec.Schema()[*result].field_type)
                })
                && layout
                    .key_inputs
                    .iter()
                    .zip(&layout.key_results)
                    .all(|(input, result)| {
                        self.child.Schema()[*input]
                            .field_type
                            .Equal(&exec.Schema()[*result].field_type)
                    });
            if !schema_valid {
                let _ = exec.Close();
                return Err("full-update result schema mismatch".into());
            }
            let opened = exec.Open().map_err(|e| e.to_string());
            if let Err(e) = opened {
                let _ = exec.Close();
                return Err(e);
            }
            let result: Result<(), String> = (|| {
                let mut chunk = exec.NewChunk();
                loop {
                    if let Some(context) = context {
                        exec.NextWithContext(context, &mut chunk)
                    } else {
                        exec.Next(&mut chunk)
                    }
                    .map_err(|e| e.to_string())?;
                    if chunk.NumRows() == 0 {
                        break;
                    }
                    for i in 0..chunk.NumRows() {
                        let row = chunk.GetRow(i);
                        let values: Vec<_> = exec
                            .Schema()
                            .iter()
                            .enumerate()
                            .map(|(j, c)| row.GetDatum(j, &c.field_type))
                            .collect();
                        let key: Vec<_> = layout
                            .key_results
                            .iter()
                            .map(|j| values[*j].clone())
                            .collect();
                        if !batch.iter().any(|k| same(k, &key))
                            || found.iter().any(|(k, _)| same(k, &key))
                        {
                            return Err("unexpected or duplicate full-update group key".into());
                        }
                        found.push((key, values));
                    }
                }
                Ok(())
            })();
            let closed = exec.Close().map_err(|e| e.to_string());
            result?;
            closed?;
        }
        for (row, mapping) in needed {
            let m = &self.mappings[*mapping];
            let key: Vec<_> = layout
                .key_inputs
                .iter()
                .map(|i| rows[*row][*i].clone())
                .collect();
            rows[*row][m.output] = found
                .iter()
                .find(|(k, _)| same(k, &key))
                .map(|(_, v)| v[layout.results[&m.output]].clone())
                .unwrap_or_default();
        }
        Ok(())
    }

    fn merge(
        &self,
        rows: &mut [Vec<Datum>],
        context: Option<&ExecutionContext>,
    ) -> Result<(), String> {
        let fields = self.child.Schema();
        let mut needed = Vec::new();
        for (index, m) in self.mappings.iter().enumerate() {
            for (row_index, row) in rows.iter_mut().enumerate() {
                let old = row[m.output].clone();
                let d = &m.dependencies;
                row[m.output] = match m.kind {
                    MViewAggKind::CountStar | MViewAggKind::Count => {
                        let before = if old.IsNull() { 0 } else { count(&old)? };
                        if row[d[0]].Kind() != astersql_types::datum::KindInt64 {
                            return Err("count delta contains null or wrong type".into());
                        }
                        let next = before
                            .checked_add(row[d[0]].GetInt64())
                            .ok_or("COUNT overflow")?;
                        if next < 0 {
                            return Err("count becomes negative".into());
                        }
                        NewIntDatum(next)
                    }
                    MViewAggKind::Sum => {
                        if d.len() == 2 && count(&row[d[1]])? == 0 {
                            Datum::default()
                        } else if old.IsNull() {
                            if fields[m.output].field_type.GetFlag() & UnsignedFlag != 0
                                && !row[d[0]].IsNull()
                            {
                                crate::mviewdeltamergeagg::numeric_add(
                                    &astersql_types::datum::NewUintDatum(0),
                                    &row[d[0]],
                                )?
                            } else {
                                row[d[0]].clone()
                            }
                        } else if row[d[0]].IsNull() {
                            old
                        } else {
                            crate::mviewdeltamergeagg::numeric_add(&old, &row[d[0]])?
                        }
                    }
                    MViewAggKind::Min | MViewAggKind::Max => {
                        let final_count =
                            count(&row[*d.get(4).unwrap_or(&self.mappings[0].output)])?;
                        let add_count = count(&row[d[1]])?;
                        let remove_count = count(&row[d[3]])?;
                        if final_count == 0 {
                            Datum::default()
                        } else {
                            match minmax_decision(
                                &old,
                                &row[d[0]],
                                &row[d[2]],
                                add_count,
                                remove_count,
                                m.kind,
                                &fields[m.output].field_type,
                            )? {
                                MinMaxDecision::Old => old,
                                MinMaxDecision::Added => row[d[0]].clone(),
                                MinMaxDecision::Recompute => {
                                    needed.push((row_index, index));
                                    old
                                }
                            }
                        }
                    }
                };
            }
        }
        self.recompute(rows, &needed, context)
    }

    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        output.Reset();
        if !self.opened {
            return Err(astersql_errors::New("MView delta merge is not open"));
        }
        if self.done {
            return Ok(());
        }
        let result = (|| -> Result<(), String> {
            let mut chunk = self.child.NewChunk();
            loop {
                if let Some(context) = context {
                    self.child.NextWithContext(context, &mut chunk)
                } else {
                    self.child.Next(&mut chunk)
                }
                .map_err(|e| e.to_string())?;
                if chunk.NumRows() == 0 {
                    break;
                }
                let mut rows: Vec<Vec<Datum>> = (0..chunk.NumRows())
                    .map(|i| {
                        let row = chunk.GetRow(i);
                        self.child
                            .Schema()
                            .iter()
                            .enumerate()
                            .map(|(j, c)| row.GetDatum(j, &c.field_type))
                            .collect()
                    })
                    .collect();
                let before = rows.clone();
                self.merge(&mut rows, context)?;
                let start = self.plan.delta_column_count;
                let end = start + self.plan.mv_column_count;
                let count_col = start + self.plan.count_star_mv_offset;
                self.resources.run_atomic(&mut |batch| {
                    for (old, new) in before.iter().zip(&rows) {
                        let existed = !old[count_col].IsNull();
                        let remains = count(&new[count_col])? != 0;
                        let old_target = &old[start..end];
                        let new_target = &new[start..end];
                        if !existed && remains {
                            batch.insert(new_target)?;
                        } else if existed && !remains {
                            batch.delete_from_source(old, old_target)?;
                        } else if existed && remains {
                            let touched: Vec<_> = old_target
                                .iter()
                                .zip(new_target)
                                .map(|(a, b)| !a.Equals(b))
                                .collect();
                            if touched.iter().any(|v| *v) {
                                batch.update_from_source(old, old_target, new_target, &touched)?;
                            }
                        }
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })();
        // A failed write belongs to the statement rollback path; retry requires Open.
        self.done = true;
        result.map_err(astersql_errors::New)
    }
}

impl ExecExecutor for DeltaMergeExec {
    fn Open(&mut self) -> AdapterResult {
        self.child.Open()?;
        self.opened = true;
        self.done = false;
        Ok(())
    }
    fn Close(&mut self) -> AdapterResult {
        if !self.opened {
            return Ok(());
        }
        self.opened = false;
        self.child.Close()
    }
    fn Next(&mut self, output: &mut chunk::Chunk) -> AdapterResult {
        self.next_inner(None, output)
    }
    fn NextWithContext(
        &mut self,
        ctx: &ExecutionContext,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        self.next_inner(Some(ctx), output)
    }
    fn ChunkConfig(&self) -> ChunkConfig {
        let config = self.child.ChunkConfig();
        ChunkConfig {
            fields: Vec::new(),
            ..config
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
        true
    }
    fn IsWriteExecutor(&self) -> bool {
        true
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
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}

/// Table API adapter used inside the session's atomic mutation closure. Handle
/// columns here index the full source, retaining the delta prefix and hidden ID.
/// Transaction commit/rollback and context ownership remain with the session.
pub struct TableMutationBatch<'a> {
    pub table: &'a dyn astersql_table::Table,
    pub context: &'a mut dyn astersql_table::MutateContext,
    pub transaction: &'a mut dyn astersql_kv::Transaction,
    pub handles: &'a dyn astersql_planner_util::HandleCols,
    pub statement: &'a astersql_sessionctx_stmtctx::StatementContext,
    pub delta_columns: usize,
}

impl crate::mview_complete_delta_apply::MutationBatch for TableMutationBatch<'_> {
    fn insert(&mut self, row: &[Datum]) -> Result<(), String> {
        self.table
            .AddRecord(self.context, self.transaction, row, &[])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    fn delete(&mut self, row: &[Datum]) -> Result<(), String> {
        let mut source = vec![Datum::default(); self.delta_columns];
        source.extend_from_slice(row);
        self.delete_from_source(&source, row)
    }
    fn update(&mut self, old: &[Datum], new: &[Datum], touched: &[bool]) -> Result<(), String> {
        let mut source = vec![Datum::default(); self.delta_columns];
        source.extend_from_slice(old);
        self.update_from_source(&source, old, new, touched)
    }
    fn delete_from_source(&mut self, source: &[Datum], old: &[Datum]) -> Result<(), String> {
        let handle = self
            .handles
            .BuildHandleByDatums(self.statement, source)
            .map_err(|e| e.to_string())?;
        self.table
            .RemoveRecord(self.context, self.transaction, handle.as_ref(), old, &[])
            .map_err(|e| e.to_string())
    }
    fn update_from_source(
        &mut self,
        source: &[Datum],
        old: &[Datum],
        new: &[Datum],
        touched: &[bool],
    ) -> Result<(), String> {
        let handle = self
            .handles
            .BuildHandleByDatums(self.statement, source)
            .map_err(|e| e.to_string())?;
        self.table
            .UpdateRecord(
                self.context,
                self.transaction,
                handle.as_ref(),
                old,
                new,
                touched,
                &[],
            )
            .map_err(|e| e.to_string())
    }
}
