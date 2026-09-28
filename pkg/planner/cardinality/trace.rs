// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 语句级已用统计加载状态追踪。
//
// 在用户查询规划期间，把非 FullLoad（未完全加载）的列/索引统计项写入 StmtCtx，
// 便于诊断异步统计加载缺口；忽略 `_tidb_rowid`（id <= 0）这类必然无统计的项。

use crate::*;

/// UsedStatsItem 对应 Go `any` 参数经类型断言后支持的两种统计对象。
/// `None` 显式保留 Go 接口中“类型已知但指针为 nil”的状态，避免把缺失统计误作未知类型。
pub enum UsedStatsItem<'a> {
    Column(Option<&'a statistics::Column>),
    Index(Option<&'a statistics::Index>),
}

/// recordUsedItemStatsStatus 对应 Go 同名函数，只在用户查询期间记录非 FullLoad 项的加载状态。
pub fn recordUsedItemStatsStatus(
    sctx: &dyn planctx::PlanContext,
    stats: UsedStatsItem<'_>,
    table_id: i64,
    id: i64,
) {
    // Go 有时会尝试读取 `_tidb_rowid`（id == -1）的统计信息；该项必为空，因此直接忽略。
    if id <= 0 {
        return;
    }

    // 类型分支同时得出索引标记、缺失标记和可用的加载状态引用。
    let (is_index, missing, load_status) = match stats {
        UsedStatsItem::Column(Some(column)) => (false, false, Some(&column.StatsLoadedStatus)),
        UsedStatsItem::Column(None) => (false, true, None),
        UsedStatsItem::Index(Some(index)) => (true, false, Some(&index.StatsLoadedStatus)),
        UsedStatsItem::Index(None) => (true, true, None),
    };

    // 已经完全加载的统计项无需写入语句级追踪信息。
    if !missing && load_status.is_some_and(|status| status.IsFullLoad()) {
        return;
    }

    if missing {
        // Go distinguishes a genuinely absent item from an analyzed item whose
        // statistics object is not currently present. Preserve that distinction
        // when the table-level existence map has already been attached to the
        // statement's used-stats record.
        let analyzed = sctx
            .GetSessionVars()
            .StmtCtx
            .GetUsedStatsInfo(false)
            .and_then(|used| used.GetUsedInfo(table_id))
            .and_then(|table| table.ColAndIdxStatus.clone())
            .is_some_and(|status| {
                stmtctx::cache_downcast_ref::<statistics::ColAndIdxExistenceMap>(&status)
                    .is_some_and(|existence| existence.HasAnalyzed(id, is_index))
                    || stmtctx::cache_downcast_ref::<Box<statistics::ColAndIdxExistenceMap>>(
                        &status,
                    )
                    .is_some_and(|existence| existence.HasAnalyzed(id, is_index))
            });
        let status = if analyzed {
            statistics::StatsLoadedStatus::default()
                .StatusToString()
                .to_owned()
        } else {
            "missing".to_owned()
        };
        sctx.GetSessionVars()
            .StmtCtx
            .RecordUsedStatsLoadStatus(table_id, id, is_index, status);
        return;
    }

    // 前面的 FullLoad 分支已返回；此处记录剩余统计项的实际加载状态。
    if let Some(status) = load_status {
        sctx.GetSessionVars().StmtCtx.RecordUsedStatsLoadStatus(
            table_id,
            id,
            is_index,
            status.StatusToString().to_owned(),
        );
    }
}
