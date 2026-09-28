// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 逻辑算子杂项辅助函数的单元测试。
//
// 覆盖 TableDual 判定、单行统计（Singleton Stats）、Limit/Selection 哈希、
// SELECT 锁类型分类、排序项剪枝与 Projection 消除等逻辑算子公共工具。

use crate::*;
use std::any::Any;

struct LogicalMock {
    base: BaseLogicalPlan,
}

impl Default for LogicalMock {
    fn default() -> Self {
        Self {
            base: BaseLogicalPlan::default(),
        }
    }
}

impl LogicalPlan for LogicalMock {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }

    fn DeriveStats(&mut self, _reload: bool) -> Result<(StatsInfo, bool)> {
        Ok((self.StatsInfo().cloned().unwrap_or_default(), false))
    }
}

/// 构造仅设置 UniqueID 的测试列（UniqueID 为优化器内列的唯一标识）。
fn column(unique_id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = unique_id;
    column
}

#[test]
/// 验证 Conds2TableDual：假/空条件可折叠为空表（TableDual）。
fn table_dual_detection_matches_false_and_null_filter_semantics() {
    assert!(!Conds2TableDual(&[]));
    assert!(Conds2TableDual(&[Box::new(expression::NewZero())]));
    assert!(!Conds2TableDual(&[Box::new(expression::NewOne())]));
}

#[test]
/// 验证 getSingletonStats：行数为 1，且每列 NDV（Distinct 值估计）为 1。
fn singleton_statistics_keep_one_row_and_one_ndv_per_column() {
    let schema = expression::NewSchema(vec![column(11), column(29)]);
    let stats = getSingletonStats(&schema);
    assert_eq!(stats.RowCount, 1.0);
    assert_eq!(stats.ColNDVs.get(&11), Some(&1.0));
    assert_eq!(stats.ColNDVs.get(&29), Some(&1.0));
}

#[test]
/// 验证 LogicalLimit::HashCode 的 24 字节布局含 offset/count。
fn limit_hash_covers_offset_count_and_query_block_layout() {
    let first = LogicalLimit {
        Offset: 3,
        Count: 7,
        ..LogicalLimit::default()
    };
    let second = LogicalLimit {
        Offset: 3,
        Count: 8,
        ..LogicalLimit::default()
    };
    assert_eq!(first.HashCode().len(), 24);
    assert_ne!(first.HashCode(), second.HashCode());
    assert_eq!(&first.HashCode()[4..8], &0_u32.to_be_bytes());
    assert_eq!(&first.HashCode()[8..16], &3_u64.to_be_bytes());
    assert_eq!(&first.HashCode()[16..24], &7_u64.to_be_bytes());
}

#[test]
/// 验证经 LogicalPlan trait 调用时，Limit 仍把输出基数截断到 Count。
fn limit_trait_statistics_respect_count() {
    let mut child = LogicalMock::default();
    child.SetStats(StatsInfo {
        RowCount: 10_000.0,
        ..StatsInfo::default()
    });
    let mut limit = LogicalLimit {
        Count: 10,
        ..LogicalLimit::default()
    };
    limit.SetChildren(vec![Box::new(child)]);
    let mut plan: LogicalPlanRef = Box::new(limit);

    let (stats, _) = plan.DeriveStats(true).expect("derive limit statistics");

    assert_eq!(stats.RowCount, 10.0);
}

#[test]
/// 验证 SELECT FOR UPDATE/SHARE 锁类型判定与 Go 侧支持集合一致。
fn lock_type_classification_matches_go_supported_modes() {
    for lock_type in [
        SelectLockType::ForUpdate,
        SelectLockType::ForUpdateNoWait,
        SelectLockType::ForUpdateWaitN,
    ] {
        assert!(isSelectForUpdateLockType(lock_type));
        assert!(IsSupportedSelectLockType(lock_type));
    }
    for lock_type in [SelectLockType::ForShare, SelectLockType::ForShareNoWait] {
        assert!(isSelectForShareLockType(lock_type));
        assert!(IsSupportedSelectLockType(lock_type));
    }
    for lock_type in [
        SelectLockType::None,
        SelectLockType::ForUpdateSkipLocked,
        SelectLockType::ForShareSkipLocked,
    ] {
        assert!(!isSelectForUpdateLockType(lock_type));
        assert!(!isSelectForShareLockType(lock_type));
        assert!(!IsSupportedSelectLockType(lock_type));
    }
}

#[test]
/// 验证 Selection 哈希与条件顺序无关，但与条件个数有关。
fn selection_hash_is_order_independent_but_cardinality_sensitive() {
    let first = LogicalSelection {
        Conditions: vec![
            Box::new(expression::NewZero()),
            Box::new(expression::NewOne()),
        ],
        ..LogicalSelection::default()
    };
    let reversed = LogicalSelection {
        Conditions: vec![
            Box::new(expression::NewOne()),
            Box::new(expression::NewZero()),
        ],
        ..LogicalSelection::default()
    };
    let shorter = LogicalSelection {
        Conditions: vec![Box::new(expression::NewOne())],
        ..LogicalSelection::default()
    };
    assert_eq!(first.HashCode(), reversed.HashCode());
    assert_ne!(first.HashCode(), shorter.HashCode());
}

#[test]
/// 验证 pruneSortByItems：去重排序键并丢弃运行时常量表达式。
fn sort_pruning_deduplicates_and_drops_runtime_constants() {
    let repeated = column(7);
    let items = vec![
        ByItems {
            Expr: Box::new(repeated.Clone()),
            Desc: false,
        },
        ByItems {
            Expr: Box::new(repeated),
            Desc: true,
        },
        ByItems {
            Expr: Box::new(expression::NewOne()),
            Desc: false,
        },
    ];
    let (kept, used) = pruneSortByItems(items);
    assert_eq!(kept.len(), 1);
    assert_eq!(used.len(), 1);
    assert_eq!(used[0].UniqueID, 7);
}

#[test]
/// 验证松散 Projection 消除仅允许直接列引用，不允许计算表达式。
fn projection_elimination_requires_only_direct_columns() {
    let direct = LogicalProjection {
        Exprs: vec![Box::new(column(1)), Box::new(column(2))],
        ..LogicalProjection::default()
    };
    let computed = LogicalProjection {
        Exprs: vec![Box::new(expression::NewOne())],
        ..LogicalProjection::default()
    };
    let expand_projection = LogicalProjection {
        Exprs: vec![Box::new(column(1))],
        Proj4Expand: true,
        ..LogicalProjection::default()
    };
    assert!(canProjectionBeEliminatedLoose(&direct));
    assert!(!canProjectionBeEliminatedLoose(&computed));
    assert!(!canProjectionBeEliminatedLoose(&expand_projection));
}

#[test]
/// 验证 splitSetGetVarFunc：普通常量仍可下推，不保留在上层。
fn selection_variable_split_keeps_plain_constants_pushable() {
    let (pushable, retained) = splitSetGetVarFunc(vec![
        Box::new(expression::NewZero()),
        Box::new(expression::NewOne()),
    ]);
    assert_eq!(pushable.len(), 2);
    assert!(retained.is_empty());
}
