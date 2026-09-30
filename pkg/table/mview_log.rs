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

//! Statement scoped materialized-view log wrapper. Every successful base-table
//! mutation synchronously writes its corresponding log row in the same transaction.

use std::sync::{Arc, Mutex};

use autoid_dependency::Allocators;
use errors_dependency::New;
use kv_dependency::{Handle, Key, Transaction};
use model_dependency::group_4 as model;
use tblctx_dependency::AllocatorContext;
use types_dependency::datum::{self, Datum};

use crate::{
    AddRecordOption, Column, Constraint, Index, MutateContext, NewAddRecordOpt, NewUpdateRecordOpt,
    PartitionedTable, RemoveRecordOption, Table, TableResult, Type, UpdateRecordOption, WithCtx,
    columnAPI,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MLogSourceStmt {
    Insert,
    Update,
    Delete,
    Replace,
    LoadData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MLogDMLType {
    Insert,
    Update,
    Delete,
}

impl MLogDMLType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "I",
            Self::Update => "U",
            Self::Delete => "D",
        }
    }
}

pub fn classify_add_record(
    source: MLogSourceStmt,
    is_update: bool,
    removed_conflict: bool,
) -> MLogDMLType {
    match source {
        MLogSourceStmt::Update => MLogDMLType::Update,
        MLogSourceStmt::Insert | MLogSourceStmt::Replace | MLogSourceStmt::LoadData
            if is_update || removed_conflict =>
        {
            MLogDMLType::Update
        }
        _ => MLogDMLType::Insert,
    }
}

pub fn validate_meta(base: &model::TableInfo, mlog: &model::TableInfo) -> TableResult<Vec<usize>> {
    let base_log_id = base
        .MaterializedViewBase
        .as_ref()
        .map(|info| info.MLogID)
        .unwrap_or(0);
    if base_log_id == 0 {
        return Err(New("wrap table with mlog: base table has no mlog info"));
    }
    if base_log_id != mlog.ID {
        return Err(New(format!(
            "wrap table with mlog: mlog id mismatch, base mlog id={base_log_id}, mlog table id={}",
            mlog.ID
        )));
    }
    let log_info = mlog
        .MaterializedViewLog
        .as_ref()
        .ok_or_else(|| New("wrap table with mlog: mlog table has no MaterializedViewLog info"))?;
    if log_info.BaseTableID != base.ID {
        return Err(New(format!(
            "wrap table with mlog: base table id mismatch, mlog base id={}, base id={}",
            log_info.BaseTableID, base.ID
        )));
    }
    let public: Vec<_> = mlog
        .Columns
        .iter()
        .filter(|column| column.State == model::StatePublic)
        .collect();
    if public.len() != log_info.Columns.len() + 2 {
        return Err(New(format!(
            "wrap table with mlog: invalid mlog meta columns, expect {}, got {}",
            log_info.Columns.len() + 2,
            public.len()
        )));
    }
    for (position, tracked) in log_info.Columns.iter().enumerate() {
        if public[position].Name.L != tracked.L {
            return Err(New(format!(
                "wrap table with mlog: invalid mlog tracked columns order at position {position}, expect {}, got {}",
                tracked.O, public[position].Name.O
            )));
        }
    }
    let n = public.len();
    if public[n - 2].Name.L != model::MaterializedViewLogDMLTypeColumnName.to_ascii_lowercase()
        || public[n - 1].Name.L != model::MaterializedViewLogOldNewColumnName.to_ascii_lowercase()
    {
        return Err(New(format!(
            "wrap table with mlog: invalid mlog meta columns, expect {},{}, got {},{}",
            model::MaterializedViewLogDMLTypeColumnName,
            model::MaterializedViewLogOldNewColumnName,
            public[n - 2].Name.O,
            public[n - 1].Name.O
        )));
    }
    log_info
        .Columns
        .iter()
        .map(|tracked| {
            let column = base
                .Columns
                .iter()
                .find(|column| column.Name.L == tracked.L)
                .ok_or_else(|| {
                    New(format!(
                        "wrap table with mlog: base column {} not found",
                        tracked.O
                    ))
                })?;
            usize::try_from(column.Offset).map_err(|_| {
                New(format!(
                    "wrap table with mlog: invalid base column offset {}",
                    column.Offset
                ))
            })
        })
        .collect()
}

pub struct MLogTable {
    base: Box<dyn Table>,
    mlog: Box<dyn Table>,
    source: MLogSourceStmt,
    tracked_offsets: Vec<usize>,
    removed_conflict: Mutex<bool>,
}

pub fn WrapTableWithMaterializedViewLog(
    base: Box<dyn Table>,
    mlog: Box<dyn Table>,
    source: MLogSourceStmt,
) -> TableResult<Box<dyn Table>> {
    let tracked_offsets = validate_meta(base.Meta(), mlog.Meta())?;
    Ok(Box::new(MLogTable {
        base,
        mlog,
        source,
        tracked_offsets,
        removed_conflict: Mutex::new(false),
    }))
}

pub(crate) fn should_log_update(offsets: &[usize], touched: &[bool]) -> bool {
    offsets
        .iter()
        .any(|&offset| touched.get(offset).copied().unwrap_or(true))
}

pub(crate) fn project_log_row(
    offsets: &[usize],
    row: &[Datum],
    dml: MLogDMLType,
    marker: i64,
) -> TableResult<Vec<Datum>> {
    let mut log_row = Vec::with_capacity(offsets.len() + 2);
    for &offset in offsets {
        log_row.push(row.get(offset).cloned().ok_or_else(|| {
            New(format!(
                "write mlog row: column at offset {offset} is missing from the base row (len {})",
                row.len()
            ))
        })?);
    }
    log_row.push(datum::NewStringDatum(dml.as_str().to_owned()));
    log_row.push(datum::NewIntDatum(marker));
    Ok(log_row)
}

impl MLogTable {
    fn should_log_update(&self, touched: &[bool]) -> bool {
        should_log_update(&self.tracked_offsets, touched)
    }

    fn write_log_row(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        row: &[Datum],
        dml: MLogDMLType,
        marker: i64,
        lazy: crate::PessimisticLazyDupKeyCheckMode,
        go_ctx: Option<kv_dependency::Context>,
    ) -> TableResult<()> {
        let log_row = project_log_row(&self.tracked_offsets, row, dml, marker)?;
        let reserved = {
            let (allocator, available) = context.GetReservedRowIDAlloc();
            if available {
                allocator.map(|allocator| {
                    let original = allocator.Current();
                    allocator.Reset(0, 0);
                    original
                })
            } else {
                None
            }
        };
        let ctx_option = go_ctx.map(WithCtx);
        let mut options: Vec<&dyn AddRecordOption> =
            vec![&crate::DupKeyCheckMode::DupKeyCheckSkip, &lazy];
        if let Some(ref option) = ctx_option {
            options.push(option);
        }
        let result = self
            .mlog
            .AddRecord(context, transaction, &log_row, &options);
        if let Some((base, maxv)) = reserved {
            let (allocator, available) = context.GetReservedRowIDAlloc();
            if available {
                if let Some(allocator) = allocator {
                    allocator.Reset(base, maxv);
                }
            }
        }
        result.map(|_| ())
    }
}

impl columnAPI for MLogTable {
    fn Cols(&self) -> Vec<Arc<Column>> {
        self.base.Cols()
    }
    fn VisibleCols(&self) -> Vec<Arc<Column>> {
        self.base.VisibleCols()
    }
    fn HiddenCols(&self) -> Vec<Arc<Column>> {
        self.base.HiddenCols()
    }
    fn WritableCols(&self) -> Vec<Arc<Column>> {
        self.base.WritableCols()
    }
    fn DeletableCols(&self) -> Vec<Arc<Column>> {
        self.base.DeletableCols()
    }
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<Column>> {
        self.base.FullHiddenColsAndVisibleCols()
    }
}

impl Table for MLogTable {
    fn Indices(&self) -> Vec<Arc<dyn Index>> {
        self.base.Indices()
    }
    fn DeletableIndices(&self) -> Vec<Arc<dyn Index>> {
        self.base.DeletableIndices()
    }
    fn WritableConstraint(&self) -> Vec<Arc<Constraint>> {
        self.base.WritableConstraint()
    }
    fn RecordPrefix(&self) -> Key {
        self.base.RecordPrefix()
    }
    fn IndexPrefix(&self) -> Key {
        self.base.IndexPrefix()
    }
    fn AddRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        row: &[Datum],
        options: &[&dyn AddRecordOption],
    ) -> TableResult<Box<dyn Handle>> {
        let opt = NewAddRecordOpt(options);
        let had_removed = {
            let mut marker = self
                .removed_conflict
                .lock()
                .map_err(|_| New("mlog conflict marker lock poisoned"))?;
            let previous = *marker;
            *marker = false;
            previous
        };
        let handle = self.base.AddRecord(context, transaction, row, options)?;
        self.write_log_row(
            context,
            transaction,
            row,
            classify_add_record(self.source, opt.IsUpdate(), had_removed),
            1,
            opt.PessimisticLazyDupKeyCheck(),
            opt.Ctx(),
        )?;
        Ok(handle)
    }
    fn UpdateRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        handle: &dyn Handle,
        current_data: &[Datum],
        new_data: &[Datum],
        touched: &[bool],
        options: &[&dyn UpdateRecordOption],
    ) -> TableResult<()> {
        self.base.UpdateRecord(
            context,
            transaction,
            handle,
            current_data,
            new_data,
            touched,
            options,
        )?;
        if !self.should_log_update(touched) {
            return Ok(());
        }
        let opt = NewUpdateRecordOpt(options);
        self.write_log_row(
            context,
            transaction,
            current_data,
            MLogDMLType::Update,
            -1,
            opt.PessimisticLazyDupKeyCheck(),
            opt.Ctx(),
        )?;
        self.write_log_row(
            context,
            transaction,
            new_data,
            MLogDMLType::Update,
            1,
            opt.PessimisticLazyDupKeyCheck(),
            opt.Ctx(),
        )
    }
    fn RemoveRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        handle: &dyn Handle,
        row: &[Datum],
        options: &[&dyn RemoveRecordOption],
    ) -> TableResult<()> {
        self.base
            .RemoveRecord(context, transaction, handle, row, options)?;
        if matches!(
            self.source,
            MLogSourceStmt::Insert | MLogSourceStmt::Replace | MLogSourceStmt::LoadData
        ) {
            *self
                .removed_conflict
                .lock()
                .map_err(|_| New("mlog conflict marker lock poisoned"))? = true;
        }
        let dml = if self.source == MLogSourceStmt::Delete {
            MLogDMLType::Delete
        } else {
            MLogDMLType::Update
        };
        self.write_log_row(
            context,
            transaction,
            row,
            dml,
            -1,
            crate::PessimisticLazyDupKeyCheckMode::default(),
            None,
        )
    }
    fn Allocators(&self, context: &mut dyn AllocatorContext) -> Allocators {
        self.base.Allocators(context)
    }
    fn Meta(&self) -> &model::TableInfo {
        self.base.Meta()
    }
    fn UseNewCollate(&self) -> bool {
        self.base.UseNewCollate()
    }
    fn Type(&self) -> Type {
        self.base.Type()
    }
    fn GetPartitionedTable(&self) -> Option<&dyn PartitionedTable> {
        self.base.GetPartitionedTable()
    }
}
