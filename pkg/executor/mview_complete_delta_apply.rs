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
}

/// Runtime owns transaction creation, table-context borrowing and statement rollback.
/// `run_atomic` must publish every mutation on `Ok` and none on `Err`.
pub trait MutationRuntime: Send + Sync {
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
