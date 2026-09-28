// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 逻辑 Lock 算子：对应 SELECT ... FOR UPDATE/SHARE 行锁语义。
//
// 在计划树中保留锁元数据，并确保裁剪时不丢失执行器所需的句柄列
// （Handle，用于定位行）与物理表 ID 列。

use crate::{
    BaseLogicalPlan, Column, HandleCols, LogicalPlan, LogicalPlanRef, NewBaseLogicalPlan, Result,
    SelectLockInfo, SelectLockType,
};
use std::any::Any;
use std::collections::HashMap;

/// SELECT lock metadata plus the hidden handle columns required by executors.
/// SELECT 锁元数据，以及执行器所需的隐藏句柄列。
pub struct LogicalLock {
    pub BaseLogicalPlan: BaseLogicalPlan,
    pub Lock: SelectLockInfo,
    pub TblID2Handle: HashMap<i64, Vec<Box<dyn HandleCols>>>,
    pub TblID2PhysTblIDCol: HashMap<i64, Column>,
}

/// 默认无锁类型、无句柄映射。
impl Default for LogicalLock {
    fn default() -> Self {
        Self {
            BaseLogicalPlan: BaseLogicalPlan::default(),
            Lock: SelectLockInfo {
                lock_type: SelectLockType::None,
                LockType: SelectLockType::None,
                WaitSec: 0,
                Tables: Vec::new(),
            },
            TblID2Handle: HashMap::new(),
            TblID2PhysTblIDCol: HashMap::new(),
        }
    }
}

impl LogicalLock {
    /// 初始化基类逻辑计划，算子名为 Lock。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Lock", 0);
        self
    }

    /// 列裁剪：支持的锁类型必须额外保留各表句柄与物理表 ID 列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let mut used = parent_used_cols.to_vec();
        if !IsSupportedSelectLockType(self.Lock.LockType) {
            return self.BaseLogicalPlan.PruneColumns(&used);
        }

        // 锁执行需要句柄列定位行，以及分区场景下的物理表 ID。
        for (table_id, handles) in &self.TblID2Handle {
            for handle in handles {
                used.extend(handle.IterColumns().cloned());
            }
            if let Some(physical_id_col) = self.TblID2PhysTblIDCol.get(table_id) {
                used.push(physical_id_col.clone());
            }
        }
        if let Some(child) = self.BaseLogicalPlan.Children_mut().first_mut() {
            child.PruneColumns(&used)?;
        }
        Ok(())
    }

    /// LogicalLock remains in the tree; an upper TopN is pushed into its child.
    /// Lock 留在树中；上层 TopN 继续下推到其子节点。
    pub fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) {
        let Some(top_n) = top_n else {
            return;
        };
        let Some(child) = self.BaseLogicalPlan.Children_mut().first_mut() else {
            return;
        };
        if let Some(pushed) = child.PushDownTopN(Some(top_n)) {
            *child = pushed;
        }
    }
}

/// LogicalPlan trait 委托；PushDownTopN 返回 None 表示本节点不下沉。
impl LogicalPlan for LogicalLock {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.BaseLogicalPlan
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.BaseLogicalPlan
    }

    fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        LogicalLock::PruneColumns(self, parent_used_cols)
    }

    fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        LogicalLock::PushDownTopN(self, top_n);
        None
    }
}

/// 是否为 FOR UPDATE 系列锁（含 NoWait / WaitN）。
pub fn isSelectForUpdateLockType(lock_type: SelectLockType) -> bool {
    matches!(
        lock_type,
        SelectLockType::ForUpdate
            | SelectLockType::ForUpdateNoWait
            | SelectLockType::ForUpdateWaitN
    )
}

/// 是否为 FOR SHARE 系列锁（含 NoWait）。
pub fn isSelectForShareLockType(lock_type: SelectLockType) -> bool {
    matches!(
        lock_type,
        SelectLockType::ForShare | SelectLockType::ForShareNoWait
    )
}

/// 是否为规划器支持的 SELECT 锁类型。
pub fn IsSupportedSelectLockType(lock_type: SelectLockType) -> bool {
    isSelectForUpdateLockType(lock_type) || isSelectForShareLockType(lock_type)
}
