// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Hash Join 运行时统计（runtime stats）。
//
// 汇总 build/probe 耗时、哈希冲突次数与 spill（落盘）分区/字节统计，
// 供 `EXPLAIN ANALYZE` 等诊断输出。含 v1 与 v2 两套结构。

// Hash Join runtime stats 的字符串格式、Clone/Merge 语义和 spill 统计格式化。
//
// use std::sync::atomic::{AtomicI64, Ordering};
// use std::time::Duration;
//
// writeSpilledPartitionNumStatsToString 对应 Go：输出每轮 spill 分区数量与上一轮分区总数的比例。
// pub fn writeSpilledPartitionNumStatsToString(
//     buf: &mut bytes::Buffer,
//     partitionNum: i32,
//     spilledPartitionNumPerRound: &[i32],
// ) {
//     buf.WriteString("[");
//     fmt::Fprintf(buf, format_args!("{}/{}", spilledPartitionNumPerRound[0], partitionNum));
//     for i in 1..spilledPartitionNumPerRound.len() {
//         fmt::Fprintf(
//             buf,
//             format_args!(
//                 " {}/{}",
//                 spilledPartitionNumPerRound[i],
//                 spilledPartitionNumPerRound[i - 1] * partitionNum
//             ),
//         );
//     }
//     buf.WriteString("]");
// }
//
// writeBytesStatsToString 对应 Go：把 byte 统计转换成 GiB 并保留两位小数。
// pub fn writeBytesStatsToString(buf: &mut bytes::Buffer, convertedBytes: &[i64]) {
//     buf.WriteString("[");
//     for (i, byte) in convertedBytes.iter().enumerate() {
//         if i == 0 {
//             fmt::Fprintf(buf, format_args!("{:.2}", util::ByteToGiB(*byte as f64)));
//         } else {
//             fmt::Fprintf(buf, format_args!(" {:.2}", util::ByteToGiB(*byte as f64)));
//         }
//     }
//     buf.WriteString("]");
// }
//
// hashJoinRuntimeStats 对应 Go 旧版 hash join runtime stats。
// pub struct hashJoinRuntimeStats {
//     pub fetchAndBuildHashTable: Duration,
//     pub hashStat: hashStatistic,
//     pub fetchAndProbe: i64,
//     pub probe: i64,
//     pub concurrent: i32,
//     pub maxFetchAndProbe: AtomicI64,
// }
//
// impl hashJoinRuntimeStats {
// Tp implements the RuntimeStats interface.
//     pub fn Tp(&self) -> i32 {
//         execdetails::TpHashJoinRuntimeStats
//     }
//
// String 对应 Go：按 build/probe 两段拼接 RuntimeStats 文本。
//     pub fn String(&self) -> String {
//         let mut buf = bytes::NewBuffer(Vec::with_capacity(128));
//         if self.fetchAndBuildHashTable > Duration::from_nanos(0) {
//             buf.WriteString("build_hash_table:{total:");
//             buf.WriteString(execdetails::FormatDuration(self.fetchAndBuildHashTable));
//             buf.WriteString(", fetch:");
//             buf.WriteString(execdetails::FormatDuration(
//                 self.fetchAndBuildHashTable - self.hashStat.buildTableElapse,
//             ));
//             buf.WriteString(", build:");
//             buf.WriteString(execdetails::FormatDuration(self.hashStat.buildTableElapse));
//             buf.WriteString("}");
//         }
//         if self.probe > 0 {
//             buf.WriteString(", probe:{concurrency:");
//             buf.WriteString(self.concurrent.to_string());
//             buf.WriteString(", total:");
//             buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                 self.fetchAndProbe as u64,
//             )));
//             buf.WriteString(", max:");
//             buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                 self.maxFetchAndProbe.load(Ordering::SeqCst) as u64,
//             )));
//             buf.WriteString(", probe:");
//             buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(self.probe as u64)));
// fetch time 是等子 executor 取数，wait time 是等父 executor 拉取 join 结果。
//             buf.WriteString(", fetch and wait:");
//             buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                 (self.fetchAndProbe - self.probe) as u64,
//             )));
//             if self.hashStat.probeCollision > 0 {
//                 buf.WriteString(", probe_collision:");
//                 buf.WriteString(self.hashStat.probeCollision.to_string());
//             }
//             buf.WriteString("}");
//         }
//         buf.String()
//     }
//
//     pub fn Clone(&self) -> hashJoinRuntimeStats {
//         hashJoinRuntimeStats {
//             fetchAndBuildHashTable: self.fetchAndBuildHashTable,
//             hashStat: self.hashStat.clone(),
//             fetchAndProbe: self.fetchAndProbe,
//             probe: self.probe,
//             concurrent: self.concurrent,
//             maxFetchAndProbe: AtomicI64::new(self.maxFetchAndProbe.load(Ordering::SeqCst)),
//         }
//     }
//
// Merge 对应 Go：只合并同类型 stats，并保留 maxFetchAndProbe 最大值。
//     pub fn Merge(&mut self, rs: execdetails::RuntimeStats) {
//         let Some(tmp) = rs.downcast_ref::<hashJoinRuntimeStats>() else {
//             return;
//         };
//         self.fetchAndBuildHashTable += tmp.fetchAndBuildHashTable;
//         self.hashStat.buildTableElapse += tmp.hashStat.buildTableElapse;
//         self.hashStat.probeCollision += tmp.hashStat.probeCollision;
//         self.fetchAndProbe += tmp.fetchAndProbe;
//         self.probe += tmp.probe;
//         let tmp_max = tmp.maxFetchAndProbe.load(Ordering::SeqCst);
//         if self.maxFetchAndProbe.load(Ordering::SeqCst) < tmp_max {
//             self.maxFetchAndProbe.store(tmp_max, Ordering::SeqCst);
//         }
//     }
// }
//
// hashStatistic 对应 Go 内部 hash 统计；probeCollision 可能被多个 goroutine 并发访问。
// #[derive(Clone)]
// pub struct hashStatistic {
//     pub probeCollision: i64,
//     pub buildTableElapse: Duration,
// }
//
// impl hashStatistic {
//     pub fn String(&self) -> String {
//         format!(
//             "probe_collision:{}, build:{}",
//             self.probeCollision,
//             execdetails::FormatDuration(self.buildTableElapse)
//         )
//     }
// }
//
// spillStats 对应 Go spill 统计，按轮记录 spill byte 和分区数。
// pub struct spillStats {
//     pub round: i32,
//     pub totalSpillBytesPerRound: Vec<i64>,
//     pub spilledPartitionNumPerRound: Vec<i32>,
//     pub spillBuildRowTableBytesPerRound: Vec<i64>,
//     pub spillBuildHashTableBytesPerRound: Vec<i64>,
//     pub partitionNum: i32,
// }
//
// hashJoinRuntimeStatsV2 对应 Go 新版 hash join stats，拆分 partition/build/probe/fetch 等阶段。
// pub struct hashJoinRuntimeStatsV2 {
//     pub concurrent: i32,
//     pub probeCollision: i64,
//     pub fetchAndBuildHashTable: i64,
//     pub partitionData: i64,
//     pub buildHashTable: i64,
//     pub probe: i64,
//     pub fetchAndProbe: i64,
//     pub workerFetchAndProbe: i64,
//     pub maxPartitionData: i64,
//     pub maxBuildHashTable: i64,
//     pub maxProbe: i64,
//     pub maxWorkerFetchAndProbe: AtomicI64,
//     pub maxPartitionDataForCurrentRound: i64,
//     pub maxBuildHashTableForCurrentRound: i64,
//     pub maxProbeForCurrentRound: i64,
//     pub maxWorkerFetchAndProbeForCurrentRound: i64,
//     pub spill: spillStats,
//     pub isHashJoinGA: bool,
// }
//
// setMaxValue 对应 Go CAS 循环：只有 currentValue 更大时才原子更新目标地址。
// pub fn setMaxValue(addr: &AtomicI64, currentValue: i64) {
//     loop {
//         let value = addr.load(Ordering::SeqCst);
//         if currentValue <= value {
//             return;
//         }
//         if addr
//             .compare_exchange(value, currentValue, Ordering::SeqCst, Ordering::SeqCst)
//             .is_ok()
//         {
//             return;
//         }
//     }
// }
//
// impl hashJoinRuntimeStatsV2 {
//     pub fn reset(&mut self) {
//         self.probeCollision = 0;
//         self.fetchAndBuildHashTable = 0;
//         self.partitionData = 0;
//         self.buildHashTable = 0;
//         self.probe = 0;
//         self.fetchAndProbe = 0;
//         self.workerFetchAndProbe = 0;
//         self.maxPartitionData = 0;
//         self.maxBuildHashTable = 0;
//         self.maxProbe = 0;
//         self.maxWorkerFetchAndProbe.store(0, Ordering::SeqCst);
//         self.maxPartitionDataForCurrentRound = 0;
//         self.maxBuildHashTableForCurrentRound = 0;
//         self.maxProbeForCurrentRound = 0;
//         self.maxWorkerFetchAndProbeForCurrentRound = 0;
//     }
//
//     pub fn resetCurrentRound(&mut self) {
//         self.maxPartitionData += self.maxPartitionDataForCurrentRound;
//         self.maxBuildHashTable += self.maxBuildHashTableForCurrentRound;
//         self.maxProbe += self.maxProbeForCurrentRound;
//         self.maxWorkerFetchAndProbe.fetch_add(
//             self.maxWorkerFetchAndProbeForCurrentRound,
//             Ordering::SeqCst,
//         );
//         self.maxPartitionDataForCurrentRound = 0;
//         self.maxBuildHashTableForCurrentRound = 0;
//         self.maxProbeForCurrentRound = 0;
//         self.maxWorkerFetchAndProbeForCurrentRound = 0;
//     }
//
// Tp implements the RuntimeStats interface.
//     pub fn Tp(&self) -> i32 {
//         execdetails::TpHashJoinRuntimeStatsV2
//     }
//
// String 对应 Go V2 文本输出，GA 模式和非 GA 模式展示字段不同。
//     pub fn String(&self) -> String {
//         let mut buf = bytes::NewBuffer(Vec::with_capacity(128));
//         if self.fetchAndBuildHashTable > 0 {
//             if self.isHashJoinGA {
//                 buf.WriteString("build_hash_table:{concurrency:");
//                 buf.WriteString(self.concurrent.to_string());
//                 buf.WriteString(", time:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.fetchAndBuildHashTable as u64,
//                 )));
//                 buf.WriteString(", fetch:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     (self.fetchAndBuildHashTable - self.maxBuildHashTable - self.maxPartitionData)
//                         as u64,
//                 )));
//                 buf.WriteString(", max_partition:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxPartitionData as u64,
//                 )));
//                 buf.WriteString(", total_partition:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.partitionData as u64,
//                 )));
//                 buf.WriteString(", max_build:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxBuildHashTable as u64,
//                 )));
//                 buf.WriteString(", total_build:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.buildHashTable as u64,
//                 )));
//                 buf.WriteString("}");
//             } else {
//                 buf.WriteString("build_hash_table:{total:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.fetchAndBuildHashTable as u64,
//                 )));
//                 buf.WriteString(", fetch:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     (self.fetchAndBuildHashTable - self.maxBuildHashTable - self.maxPartitionData)
//                         as u64,
//                 )));
//                 buf.WriteString(", build:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     (self.maxBuildHashTable + self.maxPartitionData) as u64,
//                 )));
//                 buf.WriteString("}");
//             }
//         }
//
//         if self.probe > 0 {
//             buf.WriteString(", probe:{concurrency:");
//             buf.WriteString(self.concurrent.to_string());
//             if self.isHashJoinGA {
//                 buf.WriteString(", time:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.fetchAndProbe as u64,
//                 )));
//                 buf.WriteString(", fetch_and_wait:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     (self.fetchAndProbe - self.maxProbe) as u64,
//                 )));
//                 buf.WriteString(", max_worker_time:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxWorkerFetchAndProbe.load(Ordering::SeqCst) as u64,
//                 )));
//                 buf.WriteString(", total_worker_time:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.workerFetchAndProbe as u64,
//                 )));
//                 buf.WriteString(", max_probe:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxProbe as u64,
//                 )));
//                 buf.WriteString(", total_probe:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.probe as u64,
//                 )));
//             } else {
//                 buf.WriteString(", total:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.fetchAndProbe as u64,
//                 )));
//                 buf.WriteString(", max:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxWorkerFetchAndProbe.load(Ordering::SeqCst) as u64,
//                 )));
//                 buf.WriteString(", probe:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     self.maxProbe as u64,
//                 )));
//                 buf.WriteString(", fetch_and_wait:");
//                 buf.WriteString(execdetails::FormatDuration(Duration::from_nanos(
//                     (self.fetchAndProbe - self.maxProbe) as u64,
//                 )));
//             }
//
//             if self.probeCollision > 0 {
//                 buf.WriteString(", probe_collision:");
//                 buf.WriteString(self.probeCollision.to_string());
//             }
//             buf.WriteString("}");
//         }
//
//         if self.spill.round > 0 {
//             buf.WriteString(", spill:{round:");
//             buf.WriteString(self.spill.round.to_string());
//             buf.WriteString(", spilled_partition_num_per_round:");
//             writeSpilledPartitionNumStatsToString(
//                 &mut buf,
//                 self.spill.partitionNum,
//                 &self.spill.spilledPartitionNumPerRound,
//             );
//             buf.WriteString(", total_spill_GiB_per_round:");
//             writeBytesStatsToString(&mut buf, &self.spill.totalSpillBytesPerRound);
//             buf.WriteString(", build_spill_row_table_GiB_per_round:");
//             writeBytesStatsToString(&mut buf, &self.spill.spillBuildRowTableBytesPerRound);
//             buf.WriteString(", build_spill_hash_table_per_round:");
//             writeBytesStatsToString(&mut buf, &self.spill.spillBuildHashTableBytesPerRound);
//             buf.WriteString("}");
//         }
//         buf.String()
//     }
//
//     pub fn Clone(&self) -> hashJoinRuntimeStatsV2 {
//         hashJoinRuntimeStatsV2 {
//             concurrent: self.concurrent,
//             probeCollision: self.probeCollision,
//             fetchAndBuildHashTable: self.fetchAndBuildHashTable,
//             partitionData: self.partitionData,
//             buildHashTable: self.buildHashTable,
//             probe: self.probe,
//             fetchAndProbe: self.fetchAndProbe,
//             workerFetchAndProbe: self.workerFetchAndProbe,
//             maxPartitionData: self.maxPartitionData,
//             maxBuildHashTable: self.maxBuildHashTable,
//             maxProbe: self.maxProbe,
//             maxWorkerFetchAndProbe: AtomicI64::new(
//                 self.maxWorkerFetchAndProbe.load(Ordering::SeqCst),
//             ),
//             maxPartitionDataForCurrentRound: self.maxPartitionDataForCurrentRound,
//             maxBuildHashTableForCurrentRound: self.maxBuildHashTableForCurrentRound,
//             maxProbeForCurrentRound: self.maxProbeForCurrentRound,
//             maxWorkerFetchAndProbeForCurrentRound: self.maxWorkerFetchAndProbeForCurrentRound,
// Go Clone 未复制 spill/isHashJoinGA 字段，这里保留同样的缺省迁移形状。
//             spill: spillStats::default(),
//             isHashJoinGA: false,
//         }
//     }
//
//     pub fn Merge(&mut self, rs: execdetails::RuntimeStats) {
//         let Some(tmp) = rs.downcast_ref::<hashJoinRuntimeStatsV2>() else {
//             return;
//         };
//         self.fetchAndBuildHashTable += tmp.fetchAndBuildHashTable;
//         self.buildHashTable += tmp.buildHashTable;
//         if self.maxBuildHashTable < tmp.maxBuildHashTable {
//             self.maxBuildHashTable = tmp.maxBuildHashTable;
//         }
//         self.partitionData += tmp.partitionData;
//         if self.maxPartitionData < tmp.maxPartitionData {
//             self.maxPartitionData = tmp.maxPartitionData;
//         }
//         self.probeCollision += tmp.probeCollision;
//         self.fetchAndProbe += tmp.fetchAndProbe;
//         self.probe += tmp.probe;
//         let tmp_worker_max = tmp.maxWorkerFetchAndProbe.load(Ordering::SeqCst);
//         if self.maxWorkerFetchAndProbe.load(Ordering::SeqCst) < tmp_worker_max {
//             self.maxWorkerFetchAndProbe
//                 .store(tmp_worker_max, Ordering::SeqCst);
//         }
//     }
// }
// */
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

pub fn write_spilled_partition_num_stats(
    output: &mut String,
    partition_num: usize,
    rounds: &[usize],
) {
    output.push('[');
    if let Some(first) = rounds.first() {
        output.push_str(&format!("{first}/{partition_num}"));
        for (index, count) in rounds.iter().enumerate().skip(1) {
            output.push_str(&format!(" {count}/{}", rounds[index - 1] * partition_num));
        }
    }
    output.push(']');
}

pub fn write_bytes_stats(output: &mut String, bytes: &[i64]) {
    output.push('[');
    for (index, count) in bytes.iter().enumerate() {
        if index > 0 {
            output.push(' ');
        }
        output.push_str(&format!("{:.2}", *count as f64 / 1_073_741_824.0));
    }
    output.push(']');
}

/// Hash Join v1 运行时统计：build/probe 耗时与冲突次数。
#[derive(Debug, Default)]
pub struct HashJoinRuntimeStats {
    /// 取构建侧数据并建哈希表的总时间。
    pub fetch_and_build: Duration,
    /// 纯建表耗时（不含取数）。
    pub build_hash_table: Duration,
    /// 取探测侧数据并做 probe 的总时间。
    pub fetch_and_probe: Duration,
    /// 纯 probe 匹配耗时。
    pub probe: Duration,
    /// Probe 时哈希冲突（同桶多条需逐条比较）次数。
    pub probe_collision: u64,
    /// Worker 并发度。
    pub concurrency: usize,
    /// 单 worker 取数与 probe 的最大耗时（纳秒）。
    pub max_fetch_and_probe_ns: AtomicI64,
}

impl Clone for HashJoinRuntimeStats {
    fn clone(&self) -> Self {
        Self {
            fetch_and_build: self.fetch_and_build,
            build_hash_table: self.build_hash_table,
            fetch_and_probe: self.fetch_and_probe,
            probe: self.probe,
            probe_collision: self.probe_collision,
            concurrency: self.concurrency,
            max_fetch_and_probe_ns: AtomicI64::new(
                self.max_fetch_and_probe_ns.load(Ordering::Acquire),
            ),
        }
    }
}

impl HashJoinRuntimeStats {
    /// RuntimeStats 类型标识（对应 Go `TpHashJoinRuntimeStats`）。
    pub const TYPE: u8 = 4;
    /// 返回类型标识。
    pub fn tp(&self) -> u8 {
        Self::TYPE
    }
    /// 合并 Go `Merge` 所累加的计数；并发度保持接收者原值。
    pub fn merge(&mut self, other: &Self) {
        self.fetch_and_build += other.fetch_and_build;
        self.build_hash_table += other.build_hash_table;
        self.fetch_and_probe += other.fetch_and_probe;
        self.probe += other.probe;
        self.probe_collision += other.probe_collision;
        set_max_value(
            &self.max_fetch_and_probe_ns,
            other.max_fetch_and_probe_ns.load(Ordering::Acquire),
        );
    }
}

impl std::fmt::Display for HashJoinRuntimeStats {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = String::new();
        if self.fetch_and_build > Duration::ZERO {
            output.push_str(&format!(
                "build_hash_table:{{total:{}, fetch:{}, build:{}}}",
                format_duration(self.fetch_and_build),
                format_duration(self.fetch_and_build.saturating_sub(self.build_hash_table)),
                format_duration(self.build_hash_table)
            ));
        }
        if self.probe > Duration::ZERO {
            output.push_str(&format!(
                ", probe:{{concurrency:{}, total:{}, max:{}, probe:{}, fetch and wait:{}",
                self.concurrency,
                format_duration(self.fetch_and_probe),
                format_duration(nanos_to_duration(
                    self.max_fetch_and_probe_ns.load(Ordering::Acquire)
                )),
                format_duration(self.probe),
                format_duration(self.fetch_and_probe.saturating_sub(self.probe))
            ));
            if self.probe_collision > 0 {
                output.push_str(&format!(", probe_collision:{}", self.probe_collision));
            }
            output.push('}');
        }
        formatter.write_str(&output)
    }
}

/// 哈希表构建/探测的细粒度统计。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashStatistic {
    /// 构建哈希表耗时。
    pub build_table_elapsed: Duration,
    /// Probe 冲突次数。
    pub probe_collision: u64,
}
impl HashStatistic {
    /// 清零统计。
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl std::fmt::Display for HashStatistic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "probe_collision:{}, build:{}",
            self.probe_collision,
            format_duration(self.build_table_elapsed)
        )
    }
}

/// Spill 相关统计：每轮落盘分区数、写出字节与恢复字节。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpillStats {
    pub round: usize,
    pub partition_num: usize,
    pub total_spill_bytes_per_round: Vec<i64>,
    pub spilled_partition_num_per_round: Vec<usize>,
    pub spill_build_row_table_bytes_per_round: Vec<i64>,
    pub spill_build_hash_table_bytes_per_round: Vec<i64>,
    /// Rust 执行器采集接口；与 Go 字段接线前保留。
    pub spilled_partition_num: Vec<usize>,
    pub spilled_bytes: Vec<i64>,
    pub restored_bytes: Vec<i64>,
}
impl SpillStats {
    /// 清空各轮向量。
    pub fn reset(&mut self) {
        self.spilled_partition_num.clear();
        self.spilled_bytes.clear();
        self.restored_bytes.clear();
        self.round = 0;
        self.partition_num = 0;
        self.total_spill_bytes_per_round.clear();
        self.spilled_partition_num_per_round.clear();
        self.spill_build_row_table_bytes_per_round.clear();
        self.spill_build_hash_table_bytes_per_round.clear();
    }
}

/// Hash Join v2 运行时统计：拆分 partition/build/probe，并含 spill。
#[derive(Debug, Default)]
pub struct HashJoinRuntimeStatsV2 {
    /// 取数并建表总时间。
    pub fetch_and_build: Duration,
    /// 各 worker 建表耗时的最大值。
    pub max_build_hash_table: Duration,
    /// 分区数据总耗时。
    pub partition_data: Duration,
    /// 各 worker 分区耗时的最大值。
    pub max_partition_data: Duration,
    pub build_hash_table: Duration,
    /// 取探测侧并 probe 的总时间。
    pub fetch_and_probe: Duration,
    /// 纯 probe 耗时。
    pub probe: Duration,
    /// 各 worker probe 耗时的最大值，跨 spill 轮累加。
    pub max_probe: Duration,
    pub worker_fetch_and_probe: Duration,
    /// Probe 哈希冲突次数。
    pub probe_collision: u64,
    /// Worker 取数+probe 最大耗时（纳秒，原子更新）。
    pub max_worker_fetch_and_probe_ns: AtomicI64,
    /// Spill 分区与字节统计。
    pub spill: SpillStats,
    /// Worker 并发度。
    pub concurrency: usize,
    pub max_partition_data_for_current_round: Duration,
    pub max_build_hash_table_for_current_round: Duration,
    pub max_probe_for_current_round: Duration,
    pub max_worker_fetch_and_probe_for_current_round: Duration,
    pub is_hash_join_ga: bool,
}

impl Clone for HashJoinRuntimeStatsV2 {
    fn clone(&self) -> Self {
        Self {
            fetch_and_build: self.fetch_and_build,
            max_build_hash_table: self.max_build_hash_table,
            partition_data: self.partition_data,
            max_partition_data: self.max_partition_data,
            build_hash_table: self.build_hash_table,
            fetch_and_probe: self.fetch_and_probe,
            probe: self.probe,
            max_probe: self.max_probe,
            worker_fetch_and_probe: Duration::ZERO,
            probe_collision: self.probe_collision,
            max_worker_fetch_and_probe_ns: AtomicI64::new(
                self.max_worker_fetch_and_probe_ns.load(Ordering::Acquire),
            ),
            spill: SpillStats::default(),
            concurrency: self.concurrency,
            max_partition_data_for_current_round: self.max_partition_data_for_current_round,
            max_build_hash_table_for_current_round: self.max_build_hash_table_for_current_round,
            max_probe_for_current_round: self.max_probe_for_current_round,
            max_worker_fetch_and_probe_for_current_round: self
                .max_worker_fetch_and_probe_for_current_round,
            is_hash_join_ga: false,
        }
    }
}

impl HashJoinRuntimeStatsV2 {
    /// RuntimeStats 类型标识（对应 Go `TpHashJoinRuntimeStatsV2`）。
    pub const TYPE: u8 = 5;
    /// 返回类型标识。
    pub fn tp(&self) -> u8 {
        Self::TYPE
    }
    /// 用 CAS 更新 worker 取数+probe 的最大耗时。
    pub fn set_max_worker_fetch_and_probe(&self, elapsed: Duration) {
        set_max_value(
            &self.max_worker_fetch_and_probe_ns,
            elapsed.as_nanos().min(i64::MAX as u128) as i64,
        );
    }
    /// 整体清零。
    pub fn reset(&mut self) {
        self.probe_collision = 0;
        self.fetch_and_build = Duration::ZERO;
        self.partition_data = Duration::ZERO;
        self.build_hash_table = Duration::ZERO;
        self.probe = Duration::ZERO;
        self.fetch_and_probe = Duration::ZERO;
        self.worker_fetch_and_probe = Duration::ZERO;
        self.max_partition_data = Duration::ZERO;
        self.max_build_hash_table = Duration::ZERO;
        self.max_probe = Duration::ZERO;
        self.max_worker_fetch_and_probe_ns
            .store(0, Ordering::Release);
        self.max_partition_data_for_current_round = Duration::ZERO;
        self.max_build_hash_table_for_current_round = Duration::ZERO;
        self.max_probe_for_current_round = Duration::ZERO;
        self.max_worker_fetch_and_probe_for_current_round = Duration::ZERO;
    }
    /// 将当前轮最大值累加到跨轮最大耗时，再清空当前轮。
    pub fn reset_round(&mut self) {
        self.max_partition_data += self.max_partition_data_for_current_round;
        self.max_build_hash_table += self.max_build_hash_table_for_current_round;
        self.max_probe += self.max_probe_for_current_round;
        self.max_worker_fetch_and_probe_ns.fetch_add(
            duration_to_nanos(self.max_worker_fetch_and_probe_for_current_round),
            Ordering::AcqRel,
        );
        self.max_partition_data_for_current_round = Duration::ZERO;
        self.max_build_hash_table_for_current_round = Duration::ZERO;
        self.max_probe_for_current_round = Duration::ZERO;
        self.max_worker_fetch_and_probe_for_current_round = Duration::ZERO;
    }
    /// 合并另一份 v2 统计；max 字段取较大值。
    pub fn merge(&mut self, other: &Self) {
        self.fetch_and_build += other.fetch_and_build;
        self.build_hash_table += other.build_hash_table;
        self.max_build_hash_table = self.max_build_hash_table.max(other.max_build_hash_table);
        self.partition_data += other.partition_data;
        self.max_partition_data = self.max_partition_data.max(other.max_partition_data);
        self.fetch_and_probe += other.fetch_and_probe;
        self.probe += other.probe;
        self.max_probe = self.max_probe.max(other.max_probe);
        self.probe_collision += other.probe_collision;
        set_max_value(
            &self.max_worker_fetch_and_probe_ns,
            other.max_worker_fetch_and_probe_ns.load(Ordering::Acquire),
        );
    }
}

impl std::fmt::Display for HashJoinRuntimeStatsV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut output = String::new();
        if self.fetch_and_build > Duration::ZERO {
            let fetch = self
                .fetch_and_build
                .saturating_sub(self.max_build_hash_table + self.max_partition_data);
            if self.is_hash_join_ga {
                output.push_str(&format!("build_hash_table:{{concurrency:{}, time:{}, fetch:{}, max_partition:{}, total_partition:{}, max_build:{}, total_build:{}}}", self.concurrency, format_duration(self.fetch_and_build), format_duration(fetch), format_duration(self.max_partition_data), format_duration(self.partition_data), format_duration(self.max_build_hash_table), format_duration(self.build_hash_table)));
            } else {
                output.push_str(&format!(
                    "build_hash_table:{{total:{}, fetch:{}, build:{}}}",
                    format_duration(self.fetch_and_build),
                    format_duration(fetch),
                    format_duration(self.max_build_hash_table + self.max_partition_data)
                ));
            }
        }
        if self.probe > Duration::ZERO {
            output.push_str(&format!(", probe:{{concurrency:{}", self.concurrency));
            let max_probe = self.max_probe;
            if self.is_hash_join_ga {
                output.push_str(&format!(", time:{}, fetch_and_wait:{}, max_worker_time:{}, total_worker_time:{}, max_probe:{}, total_probe:{}", format_duration(self.fetch_and_probe), format_duration(self.fetch_and_probe.saturating_sub(max_probe)), format_duration(nanos_to_duration(self.max_worker_fetch_and_probe_ns.load(Ordering::Acquire))), format_duration(self.worker_fetch_and_probe), format_duration(max_probe), format_duration(self.probe)));
            } else {
                output.push_str(&format!(
                    ", total:{}, max:{}, probe:{}, fetch_and_wait:{}",
                    format_duration(self.fetch_and_probe),
                    format_duration(nanos_to_duration(
                        self.max_worker_fetch_and_probe_ns.load(Ordering::Acquire)
                    )),
                    format_duration(max_probe),
                    format_duration(self.fetch_and_probe.saturating_sub(max_probe))
                ));
            }
            if self.probe_collision > 0 {
                output.push_str(&format!(", probe_collision:{}", self.probe_collision));
            }
            output.push('}');
        }
        if self.spill.round > 0 {
            let mut partitions = String::new();
            write_spilled_partition_num_stats(
                &mut partitions,
                self.spill.partition_num,
                &self.spill.spilled_partition_num_per_round,
            );
            let mut total = String::new();
            write_bytes_stats(&mut total, &self.spill.total_spill_bytes_per_round);
            let mut rows = String::new();
            write_bytes_stats(&mut rows, &self.spill.spill_build_row_table_bytes_per_round);
            let mut hashes = String::new();
            write_bytes_stats(
                &mut hashes,
                &self.spill.spill_build_hash_table_bytes_per_round,
            );
            output.push_str(&format!(", spill:{{round:{}, spilled_partition_num_per_round:{partitions}, total_spill_GiB_per_round:{total}, build_spill_row_table_GiB_per_round:{rows}, build_spill_hash_table_per_round:{hashes}}}", self.spill.round));
        }
        formatter.write_str(&output)
    }
}

/// CAS 循环：仅当 `candidate` 更大时才原子更新目标最大值。
pub fn set_max_value(value: &AtomicI64, candidate: i64) {
    let mut current = value.load(Ordering::Acquire);
    while candidate > current {
        match value.compare_exchange_weak(current, candidate, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn duration_to_nanos(duration: Duration) -> i64 {
    duration.as_nanos().min(i64::MAX as u128) as i64
}

fn nanos_to_duration(nanos: i64) -> Duration {
    Duration::from_nanos(nanos.max(0) as u64)
}

fn format_duration(duration: Duration) -> String {
    if duration <= Duration::from_micros(1) {
        return format_go_duration(duration);
    }
    let unit = if duration >= Duration::from_secs(1) {
        Duration::from_secs(1)
    } else if duration >= Duration::from_millis(1) {
        Duration::from_millis(1)
    } else {
        Duration::from_micros(1)
    };
    let unit_ns = unit.as_nanos();
    let duration_ns = duration.as_nanos();
    let integer_ns = duration_ns / unit_ns * unit_ns;
    let scale = if duration < unit * 10 { 100 } else { 10 };
    let rounded_fraction = ((duration_ns % unit_ns) * scale + unit_ns / 2) / unit_ns;
    format_go_duration(Duration::from_nanos(
        (integer_ns + rounded_fraction * (unit_ns / scale)).min(u64::MAX as u128) as u64,
    ))
}

fn format_go_duration(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos == 0 {
        return "0s".to_owned();
    }
    if nanos < 1_000 {
        return format!("{nanos}ns");
    }
    if nanos < 1_000_000 {
        return format_decimal_duration(nanos / 1_000, nanos % 1_000, 3, "µs");
    }
    if nanos < 1_000_000_000 {
        return format_decimal_duration(nanos / 1_000_000, nanos % 1_000_000, 6, "ms");
    }
    let total_seconds = nanos / 1_000_000_000;
    let hours = total_seconds / 3_600;
    let minutes = total_seconds % 3_600 / 60;
    let seconds = total_seconds % 60;
    let seconds = format_decimal_duration(seconds, nanos % 1_000_000_000, 9, "s");
    if hours > 0 {
        format!("{hours}h{minutes}m{seconds}")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}")
    } else {
        seconds
    }
}

fn format_decimal_duration(whole: u128, fraction: u128, width: usize, suffix: &str) -> String {
    if fraction == 0 {
        return format!("{whole}{suffix}");
    }
    let mut fraction = format!("{fraction:0width$}");
    while fraction.ends_with('0') {
        fraction.pop();
    }
    format!("{whole}.{fraction}{suffix}")
}
