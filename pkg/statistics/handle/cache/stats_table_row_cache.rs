// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 表行数与列/索引数据长度的内存缓存。
//
// 缓存各表的近似行数，以及按 `(tableID, histID)` 索引的列/索引长度，
// 用于估算表数据与索引占用的字节数（如磁盘空间估算路径）。

use crate::CacheError;
use std::collections::HashMap;
use std::sync::RwLock;
/// 表 ID 与直方图/列/索引 ID 的复合键，用于查找列长度缓存。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct tableHistID {
    pub tableID: i64,
    pub histID: i64,
}
/// 列元信息：固定长度列可直接用 `FixedLength * 行数` 估算。
#[derive(Clone, Debug, Default)]
pub struct ColumnMeta {
    pub ID: i64,
    pub FixedLength: Option<u64>,
    /// Whether the column is in Go's `model.StatePublic`.
    pub Public: bool,
}
/// Index column metadata. `None` means Go's `types.UnspecifiedLength`.
#[derive(Clone, Debug, Default)]
pub struct IndexColumnMeta {
    pub Offset: usize,
    pub Length: Option<u64>,
}
/// 索引元信息：记录可见性、全局属性及组成列。
#[derive(Clone, Debug, Default)]
pub struct IndexMeta {
    pub ID: i64,
    pub Public: bool,
    pub Global: bool,
    pub Columns: Vec<IndexColumnMeta>,
}
/// 表结构摘要，供长度估算时遍历列与索引。
#[derive(Clone, Debug, Default)]
pub struct TableMeta {
    pub ID: i64,
    pub Columns: Vec<ColumnMeta>,
    pub Indices: Vec<IndexMeta>,
    pub Partitions: Vec<i64>,
    pub IsSequence: bool,
}
/// 行数与列长度数据源接口，由底层存储或测试桩实现。
pub trait RowStatsProvider {
    /// 批量查询表 ID 到行数的映射。
    fn RowCounts(&self, ids: &[i64]) -> Result<HashMap<i64, u64>, CacheError>;
    /// 批量查询 `(tableID, histID)` 到列/索引长度的映射。
    fn ColumnLengths(&self, ids: &[i64]) -> Result<HashMap<(i64, i64), u64>, CacheError>;
}
#[derive(Default)]
struct StatsTableRowCacheState {
    tableRows: HashMap<i64, u64>,
    colLength: HashMap<tableHistID, u64>,
}

/// 线程安全的表行数与列长度缓存（单个读写锁保护同一快照）。
#[derive(Default)]
pub struct StatsTableRowCache {
    state: RwLock<StatsTableRowCacheState>,
}
impl StatsTableRowCache {
    /// 读取缓存中的表行数；未命中返回 0。
    pub fn GetTableRows(&self, id: i64) -> u64 {
        *self.state.read().unwrap().tableRows.get(&id).unwrap_or(&0)
    }
    /// 读取缓存中的列/索引长度；未命中返回 0。
    pub fn GetColLength(&self, id: tableHistID) -> u64 {
        *self.state.read().unwrap().colLength.get(&id).unwrap_or(&0)
    }
    /// 通过 Provider 拉取指定表 ID 的行数与列长度并合并进缓存。
    pub fn UpdateByID(&self, p: &dyn RowStatsProvider, ids: &[i64]) -> Result<(), CacheError> {
        let rows = p.RowCounts(ids)?;
        let cols = p.ColumnLengths(ids)?;
        let mut state = self.state.write().unwrap();
        state.tableRows.extend(rows);
        // 将 (tableID, histID) 元组键转为 tableHistID 结构体键后写入。
        state.colLength.extend(
            cols.into_iter()
                .map(|((tableID, histID), v)| (tableHistID { tableID, histID }, v)),
        );
        Ok(())
    }
    /// 按表元信息估算行数、平均行长、数据长度与索引长度。
    pub fn EstimateDataLength(&self, t: &TableMeta) -> (u64, u64, u64, u64) {
        let mut row_count = self.GetTableRows(t.ID);
        let (mut data_length, mut index_length) = self.GetDataAndIndexLength(t, t.ID, row_count);

        if !t.Partitions.is_empty() {
            // Partition data and local indexes live at partition level. Keep the
            // table-level contribution because it contains global indexes.
            row_count = 0;
            data_length = 0;
            for partition_id in &t.Partitions {
                let partition_rows = self.GetTableRows(*partition_id);
                row_count = row_count.wrapping_add(partition_rows);
                let (partition_data, partition_index) =
                    self.GetDataAndIndexLength(t, *partition_id, partition_rows);
                data_length = data_length.wrapping_add(partition_data);
                index_length = index_length.wrapping_add(partition_index);
            }
        }

        let avg_row_length = if row_count == 0 {
            0
        } else {
            data_length / row_count
        };
        if t.IsSequence {
            row_count = 1;
        }
        (row_count, avg_row_length, data_length, index_length)
    }
    /// 给定物理 ID 与行数，分别累加列数据长度与索引长度。
    ///
    /// 固定长度列用 `FixedLength * rows`；变长列与索引从 `colLength` 缓存读取。
    pub fn GetDataAndIndexLength(&self, t: &TableMeta, pid: i64, rows: u64) -> (u64, u64) {
        let mut column_length = vec![0; t.Columns.len()];
        let mut data: u64 = 0;
        for (offset, column) in t.Columns.iter().enumerate() {
            if !column.Public {
                continue;
            }
            let length = column
                .FixedLength
                .map(|n| n.wrapping_mul(rows))
                .unwrap_or_else(|| {
                    self.GetColLength(tableHistID {
                        tableID: pid,
                        histID: column.ID,
                    })
                });
            data = data.wrapping_add(length);
            column_length[offset] = length;
        }

        let mut index: u64 = 0;
        for idx in &t.Indices {
            if !idx.Public {
                continue;
            }
            if !t.Partitions.is_empty() {
                if idx.Global && t.ID != pid {
                    continue;
                }
                if !idx.Global && t.ID == pid {
                    continue;
                }
            }
            for column in &idx.Columns {
                index = index.wrapping_add(
                    column
                        .Length
                        .map(|length| rows.wrapping_mul(length))
                        .unwrap_or(column_length[column.Offset]),
                );
            }
        }
        (data, index)
    }
}
/// 将表 ID 列表格式化为逗号分隔字符串，供 SQL `IN (...)` 等场景使用。
pub fn buildInTableIDsString(ids: &[i64]) -> String {
    format!(
        "table_id in ({})",
        ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
    )
}
