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

// 导入 SDK 的数据模型定义。
//
// 描述扫描得到的表/文件元数据、导入数据量估算、`IMPORT INTO` 选项，
// 以及导入作业（Import Job）与作业组（Group）的运行状态。
// TiKV 大小估算用于预估数据写入键值存储（TiKV）后的占用。

use crate::CSVConfig;
use astersql_lightning_mydump as mydump;
use chrono::NaiveDateTime;

/// 一张待导入表的元数据：库表名、数据文件列表、总大小、通配路径与 schema 文件。
#[derive(Clone, Debug, Default)]
pub struct TableMeta {
    /// 数据库（schema）名。
    pub Database: String,
    /// 表名。
    pub Table: String,
    /// 属于该表的数据文件元信息列表。
    pub DataFiles: Vec<DataFileMeta>,
    /// 数据文件总大小（字节）。
    pub TotalSize: i64,
    /// 能唯一匹配本表全部数据文件的通配路径。
    pub WildcardPath: String,
    /// 建表 DDL 所在 schema 文件路径。
    pub SchemaFile: String,
}

/// 单个数据文件的路径、大小、格式与压缩类型。
#[derive(Clone, Debug, Default)]
pub struct DataFileMeta {
    /// 文件路径（可为本地或对象存储 URL）。
    pub Path: String,
    /// 文件大小（字节）。
    pub Size: i64,
    /// 源文件类型（CSV/SQL 等，见 mydump::SourceType）。
    pub Format: mydump::SourceType,
    /// 压缩算法（无压缩/Gzip 等）。
    pub Compression: mydump::Compression,
}

/// 单表导入数据量估算：源文件大小与预估写入 TiKV 后的大小。
#[derive(Clone, Debug, Default)]
pub struct TableDataSizeEstimate {
    pub Database: String,
    pub Table: String,
    /// 源端文件总字节数。
    pub SourceSize: i64,
    /// 预估写入 TiKV（分布式键值存储）后的字节数。
    pub TiKVSize: i64,
}

/// 整次导入的数据量汇总估算。
#[derive(Clone, Debug, Default)]
pub struct ImportDataSizeEstimate {
    pub Tables: Vec<TableDataSizeEstimate>,
    pub TotalSourceSize: i64,
    pub TotalTiKVSize: i64,
}

/// 生成 `IMPORT INTO` 语句时使用的导入选项。
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// 数据格式名（如 csv、sql）。
    pub Format: String,
    /// CSV 专用解析配置；非 CSV 时为 None。
    pub CSVConfig: Option<Box<CSVConfig>>,
    /// 并行导入线程数。
    pub Thread: isize,
    /// 磁盘配额上限（字符串形式，便于直接拼进 SQL）。
    pub DiskQuota: String,
    /// 最大写入速度限制。
    pub MaxWriteSpeed: String,
    /// 是否按文件拆分导入。
    pub SplitFile: bool,
    /// 允许记录的最大错误行数。
    pub RecordErrors: i64,
    /// 是否以 detached 模式提交（异步不等待完成）。
    pub Detached: bool,
    /// 云存储 URI（用于落盘或中间结果）。
    pub CloudStorageURI: String,
    /// 作业组键，用于批量导入任务归组。
    pub GroupKey: String,
    /// 跳过的文件头行数。
    pub SkipRows: isize,
    /// 字符集。
    pub CharacterSet: String,
    /// 校验和表相关选项字符串。
    pub ChecksumTable: String,
    /// 是否禁用 TiKV import mode（导入模式可降低写放大）。
    pub DisableTiKVImportMode: bool,
    /// 是否跳过导入前预检查。
    pub DisablePrecheck: bool,
    /// 资源参数（JSON/字符串，透传给执行层）。
    pub ResourceParameters: String,
}

/// 按 GroupKey 聚合的导入作业组状态摘要。
#[derive(Clone, Debug)]
pub struct GroupStatus {
    pub GroupKey: String,
    pub TotalJobs: i64,
    pub Pending: i64,
    pub Running: i64,
    pub Completed: i64,
    pub Failed: i64,
    pub Cancelled: i64,
    pub FirstJobCreateTime: NaiveDateTime,
    pub LastJobUpdateTime: NaiveDateTime,
}

/// 单个导入作业的完整状态，对应 `SHOW IMPORT JOB` 的列布局。
#[derive(Clone, Debug)]
pub struct JobStatus {
    pub JobID: i64,
    pub GroupKey: String,
    pub DataSource: String,
    pub TargetTable: String,
    /// 目标表在元数据中的 table id。
    pub TableID: i64,
    /// 当前阶段（phase），如 import。
    pub Phase: String,
    /// 作业状态字符串：finished/failed/cancelled/running 等。
    pub Status: String,
    pub SourceFileSize: String,
    pub ImportedRows: i64,
    pub ResultMessage: String,
    pub CreateTime: NaiveDateTime,
    pub StartTime: NaiveDateTime,
    pub EndTime: NaiveDateTime,
    pub CreatedBy: String,
    pub UpdateTime: NaiveDateTime,
    pub Step: String,
    pub ProcessedSize: String,
    pub TotalSize: String,
    pub Percent: String,
    pub Speed: String,
    /// 预计剩余时间（ETA）。
    pub ETA: String,
}

impl JobStatus {
    /// 作业是否已成功结束（status == finished）。
    pub fn IsFinished(&self) -> bool {
        self.Status == "finished"
    }

    /// 作业是否失败。
    pub fn IsFailed(&self) -> bool {
        self.Status == "failed"
    }

    /// 作业是否被取消。
    pub fn IsCancelled(&self) -> bool {
        self.Status == "cancelled"
    }

    /// 作业是否已终结（成功、失败或取消任一）。
    pub fn IsCompleted(&self) -> bool {
        self.IsFinished() || self.IsFailed() || self.IsCancelled()
    }
}
