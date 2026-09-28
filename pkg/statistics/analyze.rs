// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// ANALYZE（分析统计信息）结果的数据结构。
//
// ANALYZE 扫描表/索引采样，构建直方图（Histogram）、CMSketch、TopN、FMSketch 等，
// 供优化器估算选择率。本文件描述单次分析任务产出的表标识与聚合结果容器。

use crate::{AnalyzeJob, CMSketch, FMSketch, Histogram, TopN};

/// 非分区表使用的伪分区 ID。
pub const NonPartitionTableID: i64 = -1;

/// 表 ID 与分区 ID 的组合；分区表统计使用 PartitionID，普通表使用 TableID。
pub struct AnalyzeTableID {
    pub TableID: i64,
    pub PartitionID: i64,
}

impl AnalyzeTableID {
    /// 返回构建统计信息实际使用的 ID。
    pub fn GetStatisticsID(&self) -> i64 {
        if self.PartitionID != NonPartitionTableID {
            self.PartitionID
        } else {
            self.TableID
        }
    }
    /// 判断当前 ID 是否表示分区表。
    pub fn IsPartitionTable(&self) -> bool {
        self.PartitionID != NonPartitionTableID
    }
    /// 保留 Go fmt.Sprintf 的展示格式，便于日志和调试输出保持一致。
    pub fn String(&self) -> String {
        format!("{} => {}", self.PartitionID, self.TableID)
    }
    /// 比较两个可空指针语义的表 ID；同一地址视为相等。
    pub fn Equals(left: Option<&AnalyzeTableID>, right: Option<&AnalyzeTableID>) -> bool {
        if left.map(|v| v as *const _) == right.map(|v| v as *const _) {
            return true;
        }
        match (left, right) {
            (Some(a), Some(b)) => a.TableID == b.TableID && a.PartitionID == b.PartitionID,
            _ => false,
        }
    }
}

/// AnalyzeResult 保存一列或一个索引的直方图、CMSketch、TopN 和 FMSketch。
pub struct AnalyzeResult {
    pub Hist: Vec<Histogram>,
    pub Cms: Vec<CMSketch>,
    pub TopNs: Vec<TopN>,
    pub Fms: Vec<FMSketch>,
    pub IsIndex: i32,
}

impl AnalyzeResult {
    /// 释放 FMSketch，并逐个把直方图归还对象池；顺序对应 Go 的 GC 释放逻辑。
    pub fn DestroyAndPutToPool(&mut self) {
        // Go assigns nil here so the slice backing storage can be reclaimed.
        // `Vec::clear` would drop the sketches but retain the allocation.
        drop(std::mem::take(&mut self.Fms));
        for histogram in &mut self.Hist {
            histogram.DestroyAndPutToPool();
        }
    }
}

/// AnalyzeResults 是一次 analyze task 的聚合结果，包含表级计数和快照元数据。
pub struct AnalyzeResults {
    pub Err: Option<astersql_errors::SharedError>,
    pub Job: Option<AnalyzeJob>,
    pub Ars: Vec<AnalyzeResult>,
    pub TableID: AnalyzeTableID,
    pub Count: i64,
    pub StatsVer: i32,
    /// 开始分析任务时的快照时间戳，用于并发 analyze 检查。
    pub Snapshot: u64,
    /// analyze 开始时 mysql.stats_meta 中的原始行数。
    pub BaseCount: i64,
    /// analyze 开始时 mysql.stats_meta 中的原始 modify_count。
    pub BaseModifyCnt: i64,
    /// 多值索引或全局索引只更新索引统计及版本，不更新表级行数和 snapshot。
    pub ForMVIndexOrGlobalIndex: bool,
}

impl AnalyzeResults {
    /// 逐个销毁聚合结果并归还其内部统计对象。
    pub fn DestroyAndPutToPool(&mut self) {
        for result in &mut self.Ars {
            result.DestroyAndPutToPool();
        }
    }
}
