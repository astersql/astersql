// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// tipb Executor 树的最小模型与 TableID / 分区 ID 改写。
//
// TiDB 执行计划下推到 TiKV 时以 tipb（TiDB Protocol Buffer）描述算子树。
// 分区表（Partition Table）将逻辑表拆成多个物理分区；动态裁剪或切换分区后
// 需要沿算子树更新 TableScan / PartitionTableScan / IndexScan 上的表或分区 ID。
// `UpdateExecutorTableID` 对齐 Go 版路径：可递归下降，Join 只改写非内表侧。

use std::collections::HashSet;

/// tipb 执行器类型枚举（覆盖本模块改写逻辑关心的算子）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecType {
    /// 表扫描。
    TableScan,
    /// 分区表扫描（携带多个分区 ID）。
    PartitionTableScan,
    /// 索引扫描。
    IndexScan,
    /// 过滤（Selection）。
    Selection,
    /// Hash 聚合。
    Aggregation,
    /// 流式聚合（要求输入按分组键有序）。
    StreamAgg,
    /// TopN（排序 + Limit 的组合）。
    TopN,
    /// Limit。
    Limit,
    /// MPP/分布式交换发送端。
    ExchangeSender,
    /// MPP/分布式交换接收端。
    ExchangeReceiver,
    /// CTE（公用表表达式）写入端。
    CteSink,
    /// CTE 读取端。
    CteSource,
    /// Join（连接）。
    Join,
    /// 投影（列裁剪/表达式计算）。
    Projection,
    /// 窗口函数。
    Window,
    /// 排序。
    Sort,
    /// Expand（CUBE/ROLLUP 等展开）。
    Expand,
    /// Expand 的第二种协议变体。
    Expand2,
    /// 未知 tipb 协议类型码。
    Unknown(i32),
}

/// 与 `ExecType` 配套的载荷：扫描 ID、一元子树、Join 双孩或终端节点。
#[derive(Clone, Debug, Eq, PartialEq)]
enum ExecutorData {
    TableScan(i64),
    PartitionTableScan(Vec<i64>),
    IndexScan(i64),
    Unary(Box<Executor>),
    Join {
        children: [Box<Executor>; 2],
        /// 内表（inner）在 children 中的下标；改写时更新 `1 - inner_idx` 一侧。
        inner_idx: usize,
    },
    Terminal,
}

/// Minimal, strongly typed representation of the tipb executor fields used by
/// `UpdateExecutorTableID`.
/// 供 `UpdateExecutorTableID` 使用的 tipb 执行器字段的最小强类型表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Executor {
    tp: ExecType,
    data: ExecutorData,
}

impl Executor {
    /// 构造指定 table_id 的表扫描节点。
    pub fn table_scan(table_id: i64) -> Self {
        Self {
            tp: ExecType::TableScan,
            data: ExecutorData::TableScan(table_id),
        }
    }

    /// 构造携带分区 ID 列表的分区表扫描节点。
    pub fn partition_table_scan(partition_ids: Vec<i64>) -> Self {
        Self {
            tp: ExecType::PartitionTableScan,
            data: ExecutorData::PartitionTableScan(partition_ids),
        }
    }

    /// 构造指定 table_id 的索引扫描节点。
    pub fn index_scan(table_id: i64) -> Self {
        Self {
            tp: ExecType::IndexScan,
            data: ExecutorData::IndexScan(table_id),
        }
    }

    /// 构造一元算子（Selection/Agg/Limit 等），子节点为 `child`。
    pub fn unary(tp: ExecType, child: Executor) -> Self {
        debug_assert!(matches!(
            tp,
            ExecType::Selection
                | ExecType::Aggregation
                | ExecType::StreamAgg
                | ExecType::TopN
                | ExecType::Limit
                | ExecType::ExchangeSender
                | ExecType::CteSink
                | ExecType::Projection
                | ExecType::Window
                | ExecType::Sort
                | ExecType::Expand
                | ExecType::Expand2
        ));
        Self {
            tp,
            data: ExecutorData::Unary(Box::new(child)),
        }
    }

    /// 构造无子节点的终端算子（ExchangeReceiver / CteSource）。
    pub fn terminal(tp: ExecType) -> Self {
        debug_assert!(matches!(
            tp,
            ExecType::ExchangeReceiver | ExecType::CteSource
        ));
        Self {
            tp,
            data: ExecutorData::Terminal,
        }
    }

    /// 构造 Join：`inner_idx` 标明内表侧（0 或 1）。
    pub fn join(children: [Executor; 2], inner_idx: usize) -> Self {
        assert!(inner_idx < 2, "join inner index must be 0 or 1");
        let [left, right] = children;
        Self {
            tp: ExecType::Join,
            data: ExecutorData::Join {
                children: [Box::new(left), Box::new(right)],
                inner_idx,
            },
        }
    }

    /// 构造未知协议类型节点（用于错误路径测试）。
    pub fn unknown(protocol_type: i32) -> Self {
        Self {
            tp: ExecType::Unknown(protocol_type),
            data: ExecutorData::Terminal,
        }
    }

    /// 若为一元算子则返回子节点引用。
    pub fn child(&self) -> Option<&Executor> {
        match &self.data {
            ExecutorData::Unary(child) => Some(child),
            _ => None,
        }
    }

    /// 若为 Join 则返回左右子节点引用。
    pub fn join_children(&self) -> Option<[&Executor; 2]> {
        match &self.data {
            ExecutorData::Join { children, .. } => {
                Some([children[0].as_ref(), children[1].as_ref()])
            }
            _ => None,
        }
    }
}

/// 改写过程可选上下文：记录下一批被更新的分区 ID。
#[derive(Default)]
pub struct UpdateExecutorTableIDContext<'a> {
    next_partition_updates: Option<&'a mut HashSet<i64>>,
}

impl<'a> UpdateExecutorTableIDContext<'a> {
    /// 创建会把更新过的分区 ID 记入 `next_partition_updates` 的上下文。
    pub fn recording(next_partition_updates: &'a mut HashSet<i64>) -> Self {
        Self {
            next_partition_updates: Some(next_partition_updates),
        }
    }
}

/// `UpdateExecutorTableID` 可能返回的错误。
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum UpdateExecutorTableIDError {
    /// tipb 协议类型码未识别。
    #[error("unknown new tipb protocol {0}")]
    UnknownProtocol(i32),
    /// ExecType 与内部载荷不一致。
    #[error("executor type and payload do not match")]
    MalformedExecutor,
}

/// Updates the table or partition IDs along the same executor path as the Go
/// implementation. As in Go, scan variants require at least one partition ID.
/// 沿与 Go 相同的执行器路径更新表 ID 或分区 ID；扫描类节点至少需要一个分区 ID。
pub fn UpdateExecutorTableID(
    mut ctx: UpdateExecutorTableIDContext<'_>,
    exec: Option<&mut Executor>,
    recursive: bool,
    partition_ids: &[i64],
) -> Result<(), UpdateExecutorTableIDError> {
    let Some(exec) = exec else {
        return Ok(());
    };

    // 按算子类型就地改写 ID，并决定是否继续向下传给子节点。
    let child = match exec.tp {
        ExecType::TableScan => {
            let ExecutorData::TableScan(table_id) = &mut exec.data else {
                return Err(UpdateExecutorTableIDError::MalformedExecutor);
            };
            *table_id = partition_ids[0];
            if let Some(updates) = &mut ctx.next_partition_updates {
                updates.insert(partition_ids[0]);
            }
            None
        }
        ExecType::PartitionTableScan => {
            let ExecutorData::PartitionTableScan(ids) = &mut exec.data else {
                return Err(UpdateExecutorTableIDError::MalformedExecutor);
            };
            ids.clone_from(&partition_ids.to_vec());
            None
        }
        ExecType::IndexScan => {
            let ExecutorData::IndexScan(table_id) = &mut exec.data else {
                return Err(UpdateExecutorTableIDError::MalformedExecutor);
            };
            *table_id = partition_ids[0];
            None
        }
        ExecType::Selection
        | ExecType::Aggregation
        | ExecType::StreamAgg
        | ExecType::TopN
        | ExecType::Limit
        | ExecType::ExchangeSender
        | ExecType::CteSink
        | ExecType::Projection
        | ExecType::Window
        | ExecType::Sort
        | ExecType::Expand
        | ExecType::Expand2 => {
            let ExecutorData::Unary(child) = &mut exec.data else {
                return Err(UpdateExecutorTableIDError::MalformedExecutor);
            };
            Some(child.as_mut())
        }
        ExecType::ExchangeReceiver | ExecType::CteSource => None,
        ExecType::Join => {
            // Go 路径只更新非内表侧（outer），inner_idx 指向内表。
            let ExecutorData::Join {
                children,
                inner_idx,
            } = &mut exec.data
            else {
                return Err(UpdateExecutorTableIDError::MalformedExecutor);
            };
            Some(children[1 - *inner_idx].as_mut())
        }
        ExecType::Unknown(protocol_type) => {
            return Err(UpdateExecutorTableIDError::UnknownProtocol(protocol_type));
        }
    };

    if recursive {
        UpdateExecutorTableID(ctx, child, true, partition_ids)?;
    }
    Ok(())
}
