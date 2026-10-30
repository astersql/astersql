// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use astersql_types::datum::Datum;

pub type Result<T> = std::result::Result<T, String>;
pub const DIFF_INSERT: i64 = 1;
pub const DIFF_DELETE: i64 = 2;
pub const DIFF_UPDATE: i64 = 3;

#[derive(Clone)]
pub struct DeltaRow {
    pub op: i64,
    pub current: Vec<Datum>,
    pub recomputed: Vec<Datum>,
}

pub trait MutationBatch {
    fn insert(&mut self, row: &[Datum]) -> Result<()>;
    fn delete(&mut self, row: &[Datum]) -> Result<()>;
    fn update(&mut self, old: &[Datum], new: &[Datum], touched: &[bool]) -> Result<()>;
    /// MView merge preserves the full input for hidden row IDs and common handles.
    /// Writers keyed by the target columns can use the ordinary mutation methods.
    fn delete_from_source(&mut self, _source: &[Datum], old: &[Datum]) -> Result<()> {
        self.delete(old)
    }
    fn update_from_source(
        &mut self,
        _source: &[Datum],
        old: &[Datum],
        new: &[Datum],
        touched: &[bool],
    ) -> Result<()> {
        self.update(old, new, touched)
    }
}

/// Runtime owns transaction creation, table-context borrowing and statement rollback.
/// `run_atomic` must publish every mutation on `Ok` and none on `Err`.
/// Executors and their canonical session are thread-bound. Resources must not
/// acquire a Send/Sync promise for Rc/RefCell-owned transaction state.
pub trait MutationRuntime {
    fn run_atomic(&self, apply: &mut dyn FnMut(&mut dyn MutationBatch) -> Result<()>)
    -> Result<()>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApplyStats {
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
}

pub fn apply(runtime: &dyn MutationRuntime, rows: &[DeltaRow]) -> Result<ApplyStats> {
    let mut stats = ApplyStats::default();
    runtime.run_atomic(&mut |batch| {
        for (idx, row) in rows.iter().enumerate() {
            match row.op {
                DIFF_INSERT => {
                    batch.insert(&row.recomputed)?;
                    stats.inserted += 1;
                }
                DIFF_DELETE => {
                    batch.delete(&row.current)?;
                    stats.deleted += 1;
                }
                DIFF_UPDATE => {
                    if row.current.len() != row.recomputed.len() {
                        return Err(format!("UPDATE row {idx} column count mismatch"));
                    }
                    let touched: Vec<bool> = row
                        .current
                        .iter()
                        .zip(&row.recomputed)
                        .map(|(a, b)| !a.Equals(b))
                        .collect();
                    if touched.iter().any(|v| *v) {
                        batch.update(&row.current, &row.recomputed, &touched)?;
                        stats.updated += 1;
                    }
                }
                op => return Err(format!("invalid diff op {op} at row {idx}")),
            }
        }
        Ok(())
    })?;
    Ok(stats)
}

/// Counts successful table operations, including progress before a writer error.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RefreshWriterStats {
    pub chunks: i64,
    pub row_ops: i64,
    pub insert_rows: i64,
    pub update_rows: i64,
    pub delete_rows: i64,
    pub noop_rows: i64,
}
impl RefreshWriterStats {
    pub fn merge(&mut self, other: Self) {
        self.chunks += other.chunks;
        self.row_ops += other.row_ops;
        self.insert_rows += other.insert_rows;
        self.update_rows += other.update_rows;
        self.delete_rows += other.delete_rows;
        self.noop_rows += other.noop_rows;
    }
    pub fn affected_rows(&self) -> u64 {
        (self.insert_rows + self.update_rows + self.delete_rows) as u64
    }
    pub fn stmt_message(&self) -> String {
        format!(
            "Rows inserted: {}  Updated: {}  Deleted: {}",
            self.insert_rows, self.update_rows, self.delete_rows
        )
    }
    pub fn detail(&self, include_noop: bool) -> String {
        let noop = if include_noop {
            format!(", noop:{}", self.noop_rows)
        } else {
            String::new()
        };
        format!(
            "chunks:{}, row_ops:{}, rows:{{insert:{}, update:{}, delete:{}{}}}",
            self.chunks, self.row_ops, self.insert_rows, self.update_rows, self.delete_rows, noop
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct CompleteDeltaApplyRuntimeStats {
    pub writer_time: std::time::Duration,
    pub writer_detail: RefreshWriterStats,
}
impl CompleteDeltaApplyRuntimeStats {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
impl astersql_util_execdetails::execdetails::RuntimeStats for CompleteDeltaApplyRuntimeStats {
    fn String(&self) -> String {
        format!(
            "mview_complete_delta_apply:{{writer:{{time:{}, {}}}}}",
            astersql_util_execdetails::execdetails::FormatDuration(self.writer_time),
            self.writer_detail.detail(false)
        )
    }
    fn Tp(&self) -> i32 {
        astersql_util_execdetails::execdetails::TpMViewCompleteDeltaApplyRuntimeStats
    }
    fn CloneBox(&self) -> Box<dyn astersql_util_execdetails::execdetails::RuntimeStats> {
        Box::new(self.clone())
    }
    fn Merge(&mut self, other: &dyn astersql_util_execdetails::execdetails::RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.writer_time += other.writer_time;
            self.writer_detail.merge(other.writer_detail);
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
