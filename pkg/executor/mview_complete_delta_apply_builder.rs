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

use std::sync::Arc;
use std::time::{Duration, Instant};

use astersql_parser_types::{ETInt, FieldType};
use astersql_planner_core::MViewCompleteDeltaApply;
use astersql_types::datum::Datum;
use astersql_util_chunk as chunk;

use crate::adapter::{
    AdapterResult, CascadeBatch, ChunkConfig, ExecExecutor, ExecutionContext, SchemaColumn,
};
use crate::builder::{BuildError, ExecutorBox};
use crate::mview_complete_delta_apply::{DIFF_DELETE, DIFF_INSERT, DIFF_UPDATE};
use crate::mview_delta_merge_builder::Resources;

/// Metadata resolved from the target table, in its writable-column order.
/// Missing columns retain their slot so malformed metadata cannot silently shift mappings.
#[derive(Clone, Debug)]
pub struct TargetColumns {
    pub public_count: usize,
    pub writable: Vec<Option<TargetColumn>>,
}
#[derive(Clone, Debug)]
pub struct TargetColumn {
    pub name: String,
    pub offset: isize,
    pub public: bool,
    pub field_type: FieldType,
}
impl TargetColumns {
    pub fn from_table(table: &dyn astersql_table::Table) -> Self {
        Self {
            public_count: table.Cols().len(),
            writable: table
                .WritableCols()
                .iter()
                .map(|col| {
                    Some(TargetColumn {
                        name: col.ColumnInfo.Name.O.clone(),
                        offset: col.ColumnInfo.Offset,
                        public: col.ColumnInfo.State == astersql_meta_model::StatePublic,
                        field_type: col.ColumnInfo.FieldType.clone(),
                    })
                })
                .collect(),
        }
    }
}
fn error(message: impl Into<String>) -> BuildError {
    BuildError::new(format!("MViewCompleteDeltaApply {}", message.into()))
}
#[derive(Clone, Debug, PartialEq)]
pub struct Mappings {
    pub current: Vec<usize>,
    pub recomputed: Vec<usize>,
    pub compare: Vec<usize>,
}
/// Compile the Go public-offset to writable-position mapping before any writes.
pub fn mappings(
    plan: &MViewCompleteDeltaApply,
    source: &[FieldType],
    target: &TargetColumns,
) -> Result<Mappings, BuildError> {
    if source
        .get(plan.op_col_offset)
        .is_none_or(|tp| tp.EvalType() != ETInt)
    {
        return Err(error(format!(
            "op column {} is absent or not integer typed",
            plan.op_col_offset
        )));
    }
    if plan.current_handle_cols.is_empty()
        || plan.current_handle_cols.iter().any(|i| *i >= source.len())
    {
        return Err(error(
            "target handle columns are absent or outside source range",
        ));
    }
    if target.public_count != plan.mv_column_count {
        return Err(error(format!(
            "target public column count {} != plan MV column count {}",
            target.public_count, plan.mv_column_count
        )));
    }
    if target.writable.is_empty() {
        return Err(error("target table has no writable columns"));
    }
    if target.writable.len() != target.public_count
        || target.writable.iter().flatten().any(|c| !c.public)
    {
        return Err(error(
            "does not support target table with non-public writable columns",
        ));
    }
    for ids in [
        &plan.current_row_input_col_ids,
        &plan.recomputed_row_input_col_ids,
    ] {
        if ids.len() != target.public_count {
            return Err(error(format!(
                "row input column mapping length {} != target public column count {}",
                ids.len(),
                target.public_count
            )));
        }
    }
    if plan.group_key_mv_offsets.is_empty()
        || plan
            .group_key_mv_offsets
            .iter()
            .any(|i| *i >= target.public_count)
    {
        return Err(error(
            "group key offsets are empty or outside target public column range",
        ));
    }
    let mut result = Mappings {
        current: vec![],
        recomputed: vec![],
        compare: vec![],
    };
    for (i, column) in target.writable.iter().enumerate() {
        let col = column
            .as_ref()
            .ok_or_else(|| error(format!("target writable column at offset {i} is nil")))?;
        if col.offset < 0 || col.offset as usize >= target.public_count {
            return Err(error(format!(
                "target writable column `{}` offset {} out of range",
                col.name, col.offset
            )));
        }
        for (ids, mapped) in [
            (&plan.current_row_input_col_ids, &mut result.current),
            (&plan.recomputed_row_input_col_ids, &mut result.recomputed),
        ] {
            let input = ids[col.offset as usize];
            let tp = source.get(input).ok_or_else(|| {
                error(format!(
                    "writable input col id {input} at writable offset {i} out of source range"
                ))
            })?;
            if !col.field_type.Equal(tp) {
                return Err(error(format!(
                    "writable input col id {input} type mismatch for target column `{}`",
                    col.name
                )));
            }
            mapped.push(input);
        }
        if !plan.group_key_mv_offsets.contains(&(col.offset as usize)) {
            result.compare.push(i);
        }
    }
    Ok(result)
}

pub fn build(
    plan: &MViewCompleteDeltaApply,
    child: ExecutorBox,
    resources: Arc<dyn Resources>,
) -> Result<ExecutorBox, BuildError> {
    let fields = child
        .Schema()
        .iter()
        .map(|c| c.field_type.clone())
        .collect::<Vec<_>>();
    let target = resources.complete_delta_target_columns()?;
    let mapping = mappings(plan, &fields, &target)?;
    let compare_fields = target
        .writable
        .iter()
        .map(|c| c.as_ref().unwrap().field_type.clone())
        .collect();
    Ok(Box::new(CompleteDeltaApplyExec {
        child,
        resources,
        op: plan.op_col_offset,
        mapping,
        compare_fields,
        opened: false,
        done: false,
        schema: vec![],
    }))
}

pub struct CompleteDeltaApplyExec {
    child: ExecutorBox,
    resources: Arc<dyn Resources>,
    op: usize,
    mapping: Mappings,
    compare_fields: Vec<FieldType>,
    opened: bool,
    done: bool,
    schema: Vec<SchemaColumn>,
}
impl CompleteDeltaApplyExec {
    fn next_inner(
        &mut self,
        context: Option<&ExecutionContext>,
        output: &mut chunk::Chunk,
    ) -> AdapterResult {
        output.Reset();
        if !self.opened {
            return Err(astersql_errors::New("MViewCompleteDeltaApply is not open"));
        }
        if self.done {
            return Ok(());
        }
        let accounting = self.resources.refresh_context();
        let enabled = accounting
            .as_ref()
            .is_some_and(|(_, ctx)| ctx.RuntimeStatsColl.is_some());
        let mut stats =
            crate::mview_complete_delta_apply::CompleteDeltaApplyRuntimeStats::default();
        let result = (|| -> Result<(), String> {
            let mut input = self.child.NewChunk();
            loop {
                if let Some(ctx) = context {
                    self.child.NextWithContext(ctx, &mut input)
                } else {
                    self.child.Next(&mut input)
                }
                .map_err(|e| e.to_string())?;
                if input.NumRows() == 0 {
                    break;
                }
                let rows = (0..input.NumRows())
                    .map(|i| {
                        let row = input.GetRow(i);
                        self.child
                            .Schema()
                            .iter()
                            .enumerate()
                            .map(|(j, c)| row.GetDatum(j, &c.field_type))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                let write_start = enabled.then(Instant::now);
                stats.writer_detail.chunks += 1;
                stats.writer_detail.row_ops += rows.len() as i64;
                self.resources.run_atomic(&mut |batch| {
                    for (idx, row) in rows.iter().enumerate() {
                        let op = &row[self.op];
                        if op.IsNull() {
                            return Err(format!("MViewCompleteDeltaApply op is NULL at row {idx}"));
                        }
                        let old = self
                            .mapping
                            .current
                            .iter()
                            .map(|i| row[*i].clone())
                            .collect::<Vec<Datum>>();
                        let mut new = old.clone();
                        match op.GetInt64() {
                            DIFF_INSERT => {
                                let values = self
                                    .mapping
                                    .recomputed
                                    .iter()
                                    .map(|i| row[*i].clone())
                                    .collect::<Vec<_>>();
                                batch.insert(&values)?;
                                stats.writer_detail.insert_rows += 1;
                            }
                            DIFF_DELETE => {
                                batch.delete_from_source(row, &old)?;
                                stats.writer_detail.delete_rows += 1;
                            }
                            DIFF_UPDATE => {
                                let mut touched = vec![false; old.len()];
                                for i in &self.mapping.compare {
                                    let candidate = &row[self.mapping.recomputed[*i]];
                                    let field = &self.compare_fields[*i];
                                    let equal = old[*i]
                                        .Compare(
                                            astersql_types::DefaultStmtNoWarningContext.clone(),
                                            candidate,
                                            astersql_util_collate::GetCollator(field.GetCollate())
                                                .as_ref(),
                                        )
                                        .map_err(|e| e.to_string())?
                                        == 0;
                                    if !equal {
                                        touched[*i] = true;
                                        new[*i] = candidate.clone();
                                    }
                                }
                                if touched.iter().any(|v| *v) {
                                    batch.update_from_source(row, &old, &new, &touched)?;
                                    stats.writer_detail.update_rows += 1;
                                }
                            }
                            other => {
                                return Err(format!(
                                    "MViewCompleteDeltaApply invalid diff op {other} at row {idx}"
                                ));
                            }
                        }
                    }
                    Ok(())
                })?;
                if let Some(start) = write_start {
                    stats.writer_time += start.elapsed();
                }
            }
            Ok(())
        })();
        self.done = true;
        if let Some((id, statement)) = accounting {
            if result.is_ok() {
                statement.SetAffectedRows(stats.writer_detail.affected_rows());
                statement.SetMessage(stats.writer_detail.stmt_message());
            }
            if let Some(collector) = &statement.RuntimeStatsColl {
                collector.RegisterStatsShared(id, Box::new(stats));
            }
        }
        result.map_err(astersql_errors::New)
    }
}
impl ExecExecutor for CompleteDeltaApplyExec {
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
        ChunkConfig {
            fields: vec![],
            ..self.child.ChunkConfig()
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
    fn AddFKCheckLockDuration(&mut self, d: Duration) {
        self.child.AddFKCheckLockDuration(d)
    }
    fn Detach(&mut self) -> Option<Box<dyn ExecExecutor>> {
        None
    }
}
