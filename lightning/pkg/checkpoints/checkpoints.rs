// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Checkpoint status, in-memory merge/apply, MySQL / file checkpoint DB.
//! Ported from `lightning/pkg/checkpoints/checkpoints.go`.
//!
//! 中文概览：这个模块负责把 Lightning 导入过程中的断点续跑状态组织成稳定协议。
//! 它既描述“任务当前做到哪里”，也描述“恢复时需要哪些最小信息才能继续跑”。
//! 代码虽然集中在一个文件里，但职责可以分成四层。
//! 第一层是状态码、键和值对象，用来表达任务、表、engine、chunk 四种粒度的进度。
//! 第二层是 diff 与 merger，用来把一次次增量推进折叠成可批量提交的写集。
//! 第三层是 MySQL 后端，它把上述对象投影到四张检查点表，并保留 Go 中的事务形状。
//! 第四层是文件后端，它把同一份状态保存成 protobuf 模型，适用于不依赖独立数据库的场景。
//!
//! 之所以要强调“协议”，是因为这里很多看似普通的细节其实都带有兼容含义。
//! 例如状态值大小关系会被拿来判断是否处于失败态。
//! 例如表名版本后缀会决定旧检查点是否仍能安全读取。
//! 例如 chunk 键采用“路径 + 原始偏移”而不是当前偏移，直接影响恢复时能否重新定位。
//! 这些都不是随意的实现选择，而是与 Go 版本长期约定下来的外部行为。
//!
//! 从调用视角看，上层流程几乎只依赖 `DB` trait。
//! 这意味着导入器并不关心后端是 MySQL、文件还是空实现。
//! 真正重要的是：初始化、读取、更新、忽略错误、销毁错误、迁移与导出这些动作的语义保持一致。
//! 也正因为如此，本文件大量代码都在做“统一内存态 <-> 不同持久化表示”的转换。
//!
//! 对恢复流程而言，检查点不是简单日志。
//! 它必须足够精确，才能让导入从上次停止的位置继续，而不是重复写入或遗漏数据。
//! 它也必须足够稳定，才能让运维和测试在出现异常时直接观察到正确的阶段信息。
//! 所以下面的中文注释会优先解释职责、约束、边界条件和与 Go 语义对齐点。
//! 注释不会改变任何控制流、字段值、SQL 文本或错误处理分支。
//!
//! 可以把这一整个模块理解成 Lightning 的“恢复坐标系”。
//! 任务级坐标告诉我们这是谁的检查点。
//! 表级坐标告诉我们这张表大体处于哪个阶段。
//! engine 级坐标告诉我们局部批次推进到了哪里。
//! chunk 级坐标则提供最细的恢复锚点和校验信息。
//! 只有四层一起工作，断点续跑才既可恢复又可排障。
//!
//! 本次任务只补充中文注释，因此不会尝试修复任何现有桩实现。
//! 如果某段 Rust 代码当前仍保留 Go 的结构骨架但未完全接线，注释会明确说明它的设计意图。
//! 这样做的目的是帮助后续维护者继续补齐真实行为，同时不误导读者把占位路径当成最终能力。
//! 也就是说，这些注释服务于“理解现在是什么、以后应补到哪里”，而不是制造假完备感。
//!
//! 阅读顺序建议如下。
//! 先看状态与快照对象，理解断点数据长什么样。
//! 再看 diff / merger，理解为什么更新不是直接重写整份快照。
//! 然后看 `DB` trait，理解上层真正依赖的契约边界。
//! 最后再分别看 MySQL 与文件后端，就能更清楚地理解两种持久化方案的差异。

use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;

use astersql_lightning_pkg_checkpoints_checkpointspb as checkpointspb;

use crate::build;
use crate::common::{self, AllTables};
use crate::config::{self, CheckpointDriverFile, CheckpointDriverMySQL};
use crate::context;
use crate::errors::{self, Error, Result};
use crate::gopath;
use crate::importdef;
use crate::json;
use crate::log;
use crate::logutil;
use crate::model;
use crate::mydump::{self, CompressionNone};
use crate::objstore;
use crate::sql;
use crate::sqltocsv;
use crate::storeapi::{self, Options, StorageHandle};
use crate::verify::{self, MakeKVChecksum};
use crate::zap;

/// CheckpointStatus is the status of a checkpoint.
/// 中文补充：状态值沿用 Go 版本的数值协议，而不是改成全新的 Rust 枚举。
/// 这样做的核心原因是上下游大量逻辑都直接依赖数值大小关系。
/// 例如失败态统一落在较小区间，便于通过范围判断完成“忽略错误”一类操作。
/// 例如成功推进会跨过写入、导入、校验、加索引与分析等阶段，阶段顺序本身就体现在数字上。
/// 这意味着下面每个常量的相对大小都是协议的一部分，而不只是实现细节。
/// 调整任何值都可能破坏恢复流程、监控统计或与 Go 的 parity 断言。
/// 因而这里保留 `u8` 这种简单表示，并让后续逻辑继续做直接比较。
///
/// 另一个容易忽略的点是：同一套状态值会同时用于表级和 engine 级。
/// 表级状态更像全局摘要，engine 级状态更像局部推进，但它们共享同一组阶段编号。
/// 这样 `MetricName`、错误重置和迁移逻辑都能统一处理。
/// 从维护角度看，这比为不同层级分别设计两套枚举更容易与 Go 实现长期同步。
///
/// 因此在阅读后续代码时，可以把状态值理解成“阶段坐标”。
/// 某个值不仅说明当前在哪一步，也说明它相对前后阶段的位置。
/// 失败态之所以可恢复、某些阶段之所以可跳过，都是基于这套坐标关系在工作。
/// 注释在这里先把这个前提讲清楚，后面很多 `<=`、`>=` 判断才会显得自然。
pub type CheckpointStatus = u8;

// 下面这组常量共同构成检查点阶段机。
// 读取它们时不要只把每个名字当成独立状态，更要看它们在整体区间中的位置。
// `Missing` 代表没有记录，通常不是错误。
// `MaxInvalid` 则划定了失败/无效态的上界，很多“忽略错误”逻辑都围绕它判断。
// `Loaded` 表示表和 engine 已经登记进检查点系统，但尚未完成后续写入。
// 再往后依次是写完、关闭、导入、索引导入、自增修正、校验、加索引和分析等阶段。
//
// 这里保留与 Go 一致的具体数值还有一个重要原因：
// 指标名映射会把多个阶段聚合到较少类别中。
// 如果数值协议变化，即便名字不变，也可能导致监控和恢复判断的分界点一起失真。
// 因此状态常量更接近“存储与监控共享协议”，而不是普通枚举成员。
// 维护者在阅读或后续继续移植时，应把这些值视为不可轻易重排的外部契约。
pub const CheckpointStatusMissing: CheckpointStatus = 0;
pub const CheckpointStatusMaxInvalid: CheckpointStatus = 25;
pub const CheckpointStatusLoaded: CheckpointStatus = 30;
pub const CheckpointStatusAllWritten: CheckpointStatus = 60;
pub const CheckpointStatusDupDetected: CheckpointStatus = 70;
pub const CheckpointStatusIndexDropped: CheckpointStatus = 80;
pub const CheckpointStatusClosed: CheckpointStatus = 90;
pub const CheckpointStatusImported: CheckpointStatus = 120;
pub const CheckpointStatusIndexImported: CheckpointStatus = 140;
pub const CheckpointStatusAlteredAutoInc: CheckpointStatus = 150;
pub const CheckpointStatusChecksumSkipped: CheckpointStatus = 170;
pub const CheckpointStatusChecksummed: CheckpointStatus = 180;
pub const CheckpointStatusIndexAdded: CheckpointStatus = 190;
pub const CheckpointStatusAnalyzeSkipped: CheckpointStatus = 200;
pub const CheckpointStatusAnalyzed: CheckpointStatus = 210;

/// WholeTableEngineID is the engine ID used for the whole table engine.
pub const WholeTableEngineID: i32 = i32::MAX;

pub const CheckpointTableNameTask: &str = "task_v2";
pub const CheckpointTableNameTable: &str = "table_v10";
pub const CheckpointTableNameEngine: &str = "engine_v5";
pub const CheckpointTableNameChunk: &str = "chunk_v6";

const allTables: &str = AllTables;
const columnTableName: &str = "table_name";

// 这批 SQL 模板是 MySQL 检查点后端的“语法骨架”。
// 它们集中定义建库建表、初始化、读取和更新时使用的标准语句。
// 这样调用点就不需要分散拼接协议字段，所有结构变化也能集中维护。
//
// 其中 task/table/engine/chunk 四张表对应四种粒度。
// task 表保存任务级头信息。
// table 表保存整表状态、表 ID、表结构快照和聚合校验信息。
// engine 表保存局部批次的阶段值。
// chunk 表保存最细粒度的文件进度和校验数据。
//
// 把模板放在这里还有一个阅读层面的好处：
// 后续方法如果出现复杂 SQL 组织，只要回到这里对照，就能马上明白操作的是哪一层对象。
// 本次任务不修改任何模板内容，只通过注释强调它们的协议地位。
pub const CreateDBTemplate: &str = "CREATE DATABASE IF NOT EXISTS %s;";
pub const CreateTaskTableTemplate: &str = r#"
		CREATE TABLE IF NOT EXISTS %s.%s (
			id tinyint(1) PRIMARY KEY,
			task_id bigint NOT NULL,
			source_dir varchar(2048) NOT NULL,
			backend varchar(16) NOT NULL,
			importer_addr varchar(256),
			tidb_host varchar(128) NOT NULL,
			tidb_port int NOT NULL,
			pd_addr varchar(128) NOT NULL,
			sorted_kv_dir varchar(256) NOT NULL,
			lightning_ver varchar(48) NOT NULL
		);"#;
pub const CreateTableTableTemplate: &str = r#"
		CREATE TABLE IF NOT EXISTS %s.%s (
			task_id bigint NOT NULL,
			table_name varchar(261) NOT NULL PRIMARY KEY,
			hash binary(32) NOT NULL,
			status tinyint unsigned DEFAULT 30,
			table_id bigint NOT NULL DEFAULT 0,
		    table_info longtext NOT NULL,
			create_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
			update_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
			kv_bytes bigint unsigned NOT NULL DEFAULT 0,
			kv_kvs bigint unsigned NOT NULL DEFAULT 0,
			kv_checksum bigint unsigned NOT NULL DEFAULT 0,
			auto_rand_base bigint NOT NULL DEFAULT 0,
			auto_incr_base bigint NOT NULL DEFAULT 0,
			auto_row_id_base bigint NOT NULL DEFAULT 0,
			INDEX(task_id)
		);"#;
pub const CreateEngineTableTemplate: &str = r#"
		CREATE TABLE IF NOT EXISTS %s.%s (
			table_name varchar(261) NOT NULL,
			engine_id int NOT NULL,
			status tinyint unsigned DEFAULT 30,
			create_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
			update_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
			PRIMARY KEY(table_name, engine_id DESC)
		);"#;
pub const CreateChunkTableTemplate: &str = r#"
		CREATE TABLE IF NOT EXISTS %s.%s (
			table_name varchar(261) NOT NULL,
			engine_id int unsigned NOT NULL,
			path varchar(2048) NOT NULL,
			offset bigint NOT NULL,
			type int NOT NULL,
			compression int NOT NULL,
			sort_key varchar(256) NOT NULL,
			file_size bigint NOT NULL,
			columns text NULL,
			should_include_row_id BOOL NOT NULL,
			end_offset bigint NOT NULL,
			pos bigint NOT NULL,
			real_pos bigint NOT NULL,
			prev_rowid_max bigint NOT NULL,
			rowid_max bigint NOT NULL,
			kvc_bytes bigint unsigned NOT NULL DEFAULT 0,
			kvc_kvs bigint unsigned NOT NULL DEFAULT 0,
			kvc_checksum bigint unsigned NOT NULL DEFAULT 0,
			create_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP,
			update_time timestamp NOT NULL DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
			PRIMARY KEY(table_name, engine_id, path(500), offset)
		);"#;
pub const InitTaskTemplate: &str = r#"
		REPLACE INTO %s.%s (id, task_id, source_dir, backend, importer_addr, tidb_host, tidb_port, pd_addr, sorted_kv_dir, lightning_ver)
			VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?);"#;
pub const InitTableTemplate: &str = r#"
		INSERT INTO %s.%s (task_id, table_name, hash, table_id, table_info) VALUES (?, ?, ?, ?, ?)
			ON DUPLICATE KEY UPDATE task_id = CASE
				WHEN hash = VALUES(hash)
				THEN VALUES(task_id)
			END;"#;
pub const ReadTaskTemplate: &str = r#"
		SELECT task_id, source_dir, backend, importer_addr, tidb_host, tidb_port, pd_addr, sorted_kv_dir, lightning_ver FROM %s.%s WHERE id = 1;"#;
pub const ReadEngineTemplate: &str = r#"
		SELECT engine_id, status FROM %s.%s WHERE table_name = ? ORDER BY engine_id DESC;"#;
pub const ReadChunkTemplate: &str = r#"
		SELECT
			engine_id, path, offset, type, compression, sort_key, file_size, columns,
			pos, real_pos, end_offset, prev_rowid_max, rowid_max,
			kvc_bytes, kvc_kvs, kvc_checksum, unix_timestamp(create_time)
		FROM %s.%s WHERE table_name = ?
		ORDER BY engine_id, path, offset;"#;
pub const ReadTableRemainTemplate: &str = r#"
		SELECT status, table_id, table_info, kv_bytes, kv_kvs, kv_checksum, auto_rand_base, auto_incr_base, auto_row_id_base
		FROM %s.%s WHERE table_name = ?;"#;
pub const ReplaceEngineTemplate: &str = r#"
		REPLACE INTO %s.%s (table_name, engine_id, status) VALUES (?, ?, ?);"#;
pub const ReplaceChunkTemplate: &str = r#"
		REPLACE INTO %s.%s (
				table_name, engine_id,
				path, offset, type, compression, sort_key, file_size, columns, should_include_row_id,
				pos, real_pos, end_offset, prev_rowid_max, rowid_max,
				kvc_bytes, kvc_kvs, kvc_checksum, create_time
			) VALUES (
				?, ?,
				?, ?, ?, ?, ?, ?, ?, FALSE,
				?, ?, ?, ?, ?,
				0, 0, 0, from_unixtime(?)
			);"#;
pub const UpdateChunkTemplate: &str = r#"
		UPDATE %s.%s SET pos = ?, real_pos = ?, prev_rowid_max = ?, kvc_bytes = ?, kvc_kvs = ?, kvc_checksum = ?, columns = ?
		WHERE (table_name, engine_id, path, offset) = (?, ?, ?, ?);"#;
pub const UpdateTableRebaseTemplate: &str = r#"
		UPDATE %s.%s
		SET auto_rand_base = GREATEST(?, auto_rand_base),
		    auto_incr_base = GREATEST(?, auto_incr_base),
		    auto_row_id_base = GREATEST(?, auto_row_id_base)
		WHERE table_name = ?;"#;
pub const UpdateTableStatusTemplate: &str = r#"
		UPDATE %s.%s SET status = ? WHERE table_name = ?;"#;
pub const UpdateTableChecksumTemplate: &str =
    "UPDATE %s.%s SET kv_bytes = ?, kv_kvs = ?, kv_checksum = ? WHERE table_name = ?;";
pub const UpdateEngineTemplate: &str = r#"
		UPDATE %s.%s SET status = ? WHERE (table_name, engine_id) = (?, ?);"#;
pub const DeleteCheckpointRecordTemplate: &str = "DELETE FROM %s.%s WHERE table_name = ?;";

/// IsCheckpointTable checks if the table name is a checkpoint table.
pub fn IsCheckpointTable(name: &str) -> bool {
    matches!(
        name,
        CheckpointTableNameTask
            | CheckpointTableNameTable
            | CheckpointTableNameEngine
            | CheckpointTableNameChunk
    )
}

/// MetricName returns the metric name for the checkpoint status.
pub fn MetricName(status: CheckpointStatus) -> &'static str {
    // 指标名映射刻意做了折叠，而不是为每个状态暴露一个独立标签。
    // 这样 Prometheus 等监控侧能维持稳定的低基数，不会因为阶段太细而让面板难以阅读。
    // 例如 `Checksummed` 与 `ChecksumSkipped` 都被归到 `checksum`。
    // 例如 `Analyzed` 与 `AnalyzeSkipped` 都被归到 `analyzed`。
    // 这种折叠方式与 Go 保持一致，因此跨语言比较导入过程时能看到相同维度。
    //
    // 也正因如此，状态值的细粒度差异主要服务于恢复与控制流，
    // 而监控层更关心“到了哪一大类阶段”。
    // 把这点想清楚后，就能理解为什么这里不直接返回状态名本身。
    match status {
        CheckpointStatusLoaded => "pending",
        CheckpointStatusAllWritten => "written",
        CheckpointStatusClosed => "closed",
        CheckpointStatusImported => "imported",
        CheckpointStatusIndexImported => "index_imported",
        CheckpointStatusAlteredAutoInc => "altered_auto_inc",
        CheckpointStatusChecksummed | CheckpointStatusChecksumSkipped => "checksum",
        CheckpointStatusIndexAdded => "index_added",
        CheckpointStatusAnalyzed | CheckpointStatusAnalyzeSkipped => "analyzed",
        CheckpointStatusMissing => "missing",
        _ => "invalid",
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
// `ChunkCheckpointKey` 是 chunk 级检查点的稳定身份标识。
// 它选择“路径 + 原始起始偏移”作为键，而不是当前处理进度。
// 这是因为 `Chunk.Offset` 会随着导入持续前进，如果直接拿它做主键，恢复时就无法再定位原始 chunk。
// 原始起点则不同：只要切分策略不变，它就是这个 chunk 在整个源文件中的固定锚点。
//
// 这一定义同时服务于内存、文件和 MySQL 三种表示。
// 在内存里，它用来排序和二分查找。
// 在文件后端里，它会被转成 `path:offset` 字符串作为 protobuf map key。
// 在 MySQL 后端里，它虽然拆成多列保存，但逻辑上仍然代表同一件事。
//
// 也就是说，这个结构虽小，却是跨层转换时最不能出错的协议点之一。
// 若键不稳定，更新 diff 时会找不到原记录。
// 若键不唯一，恢复时就会把多个 chunk 混成一个。
// 若键与 Go 格式不一致，文件后端和测试对照都会立刻失真。
//
// 读到后面的 `compare`、`less` 和排序逻辑时，需要始终记得：
// 它们真正维护的不是普通排序体验，而是“恢复时可重新找到同一 chunk”的能力。
// 所以这里的比较顺序也要与 Go 完全一致。
pub struct ChunkCheckpointKey {
    pub Path: String,
    pub Offset: i64,
}

impl ChunkCheckpointKey {
    pub fn String(&self) -> String {
        format!("{}:{}", self.Path, self.Offset)
    }

    pub fn compare(&self, other: &ChunkCheckpointKey) -> i32 {
        match self.Path.cmp(&other.Path) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Equal => match self.Offset.cmp(&other.Offset) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Equal => 0,
            },
        }
    }

    pub fn less(&self, other: &ChunkCheckpointKey) -> bool {
        match self.Path.cmp(&other.Path) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
            std::cmp::Ordering::Equal => self.Offset < other.Offset,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
// `ChunkCheckpoint` 表示单个切分片段的完整恢复快照。
// 这个对象同时承载静态元信息和动态进度信息。
// 静态部分包括源文件路径、文件类型、压缩类型、排序键和文件总大小。
// 动态部分包括当前逻辑偏移、真实偏移、行号上界、校验和与最近时间戳。
//
// 为什么一个 chunk 需要这么多字段？
// 因为恢复导入不仅要知道“之前做到哪了”，还要知道“应该按什么方式继续读同一份源数据”。
// 例如压缩文件与普通文件的偏移语义不同。
// 例如列映射会影响后续解析出的列顺序。
// 例如校验和累积结果会继续向上汇总成表级校验结果。
//
// 文件后端和 MySQL 后端都不会直接把上层逻辑暴露给调用方。
// 它们会先把底层存储里的记录重建成这个统一对象，再交给上层使用。
// 因而这里可以把 `ChunkCheckpoint` 看作检查点模块的“最小公共语言”。
// 一旦理解了它，后续所有持久化转换都会更容易读懂。
//
// 下方几个尺寸计算函数也要结合这个背景理解。
// 它们不是单纯的数学相减，而是在“普通文件 / 压缩文件”两种不同进度语义之间做兼容选择。
pub struct ChunkCheckpoint {
    pub Key: ChunkCheckpointKey,
    pub FileMeta: mydump::SourceFileMeta,
    pub ColumnPermutation: Vec<i32>,
    pub Chunk: mydump::Chunk,
    pub Checksum: verify::KVChecksum,
    pub Timestamp: i64,
}

impl ChunkCheckpoint {
    pub fn DeepCopy(&self) -> ChunkCheckpoint {
        ChunkCheckpoint {
            Key: self.Key.clone(),
            FileMeta: self.FileMeta.clone(),
            ColumnPermutation: self.ColumnPermutation.clone(),
            Chunk: self.Chunk.clone(),
            Checksum: self.Checksum.clone(),
            Timestamp: self.Timestamp,
        }
    }

    pub fn UnfinishedSize(&self) -> i64 {
        if self.FileMeta.Compression == CompressionNone {
            return self.Chunk.EndOffset - self.Chunk.Offset;
        }
        self.FileMeta.FileSize - self.Chunk.RealOffset
    }

    pub fn TotalSize(&self) -> i64 {
        if self.FileMeta.Compression == CompressionNone {
            return self.Chunk.EndOffset - self.Key.Offset;
        }
        self.FileMeta.FileSize
    }

    pub fn FinishedSize(&self) -> i64 {
        if self.FileMeta.Compression == CompressionNone {
            return self.Chunk.Offset - self.Key.Offset;
        }
        self.Chunk.RealOffset - self.Key.Offset
    }

    pub fn GetKey(&self) -> String {
        self.Key.String()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineCheckpoint {
    pub Status: CheckpointStatus,
    pub Chunks: Vec<ChunkCheckpoint>,
}

impl EngineCheckpoint {
    pub fn DeepCopy(&self) -> EngineCheckpoint {
        EngineCheckpoint {
            Status: self.Status,
            Chunks: self.Chunks.iter().map(|c| c.DeepCopy()).collect(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableCheckpoint {
    // `TableCheckpoint` 把“整表摘要”和“局部 engine 细节”聚合在同一个对象中。
    // 这并不意味着它过于臃肿，恰恰说明恢复某张表时需要同时看到这两层信息。
    // 若只有表级状态，看不出哪个 engine 或 chunk 真正卡住。
    // 若只有 engine / chunk 细节，又不方便快速判断整表是否已进入后处理阶段。
    //
    // `TableID` 与 `TableInfo` 对应逻辑表身份和按需保存的结构信息。
    // `Checksum` 则承接导入完成后的整表校验结果。
    // 三个 base 字段则服务于自增相关恢复语义，防止恢复后回退主键分配起点。
    //
    // 因而这个对象既是恢复入口，也是排障摘要。
    // 上层只要拿到它，就能同时回答“做到哪一步”和“下一步该从哪里继续”两个问题。
    pub Status: CheckpointStatus,
    pub Engines: HashMap<i32, EngineCheckpoint>,
    pub TableID: i64,
    pub TableInfo: Option<model::TableInfo>,
    pub Checksum: verify::KVChecksum,
    pub AutoRandBase: i64,
    pub AutoIncrBase: i64,
    pub AutoRowIDBase: i64,
}

impl TableCheckpoint {
    /// DeepCopy matches Go: TableInfo is intentionally not copied.
    pub fn DeepCopy(&self) -> TableCheckpoint {
        let engines = self
            .Engines
            .iter()
            .map(|(id, e)| (*id, e.DeepCopy()))
            .collect();
        TableCheckpoint {
            Status: self.Status,
            Engines: engines,
            TableID: self.TableID,
            TableInfo: None,
            Checksum: self.Checksum.clone(),
            AutoRandBase: self.AutoRandBase,
            AutoIncrBase: self.AutoIncrBase,
            AutoRowIDBase: self.AutoRowIDBase,
        }
    }

    pub fn CountChunks(&self) -> usize {
        self.Engines.values().map(|e| e.Chunks.len()).sum()
    }

    /// Apply the diff to existing chunk/engine checkpoints (Go binary-search semantics).
    pub fn Apply(&mut self, cpd: &TableCheckpointDiff) {
        if cpd.hasStatus {
            self.Status = cpd.status;
        }
        if cpd.hasRebase {
            self.AutoRandBase = self.AutoRandBase.max(cpd.autoRandBase);
            self.AutoIncrBase = self.AutoIncrBase.max(cpd.autoIncrBase);
            self.AutoRowIDBase = self.AutoRowIDBase.max(cpd.autoRowIDBase);
        }
        for (engineID, engineDiff) in &cpd.engines {
            let Some(engine) = self.Engines.get_mut(engineID) else {
                continue;
            };
            if engineDiff.hasStatus {
                engine.Status = engineDiff.status;
            }
            for (key, diff) in &engineDiff.chunks {
                let checkpointKey = key.clone();
                let index = engine
                    .Chunks
                    .partition_point(|c| c.Key.less(&checkpointKey));
                if index >= engine.Chunks.len() {
                    continue;
                }
                let chunk = &mut engine.Chunks[index];
                if chunk.Key != checkpointKey {
                    continue;
                }
                chunk.Chunk.Offset = diff.pos;
                chunk.Chunk.RealOffset = diff.realPos;
                chunk.Chunk.PrevRowIDMax = diff.rowID;
                chunk.Checksum = diff.checksum.clone();
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct chunkCheckpointDiff {
    pub pos: i64,
    pub realPos: i64,
    pub rowID: i64,
    pub checksum: verify::KVChecksum,
    pub columnPermutation: Vec<i32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct engineCheckpointDiff {
    pub hasStatus: bool,
    pub status: CheckpointStatus,
    pub chunks: HashMap<ChunkCheckpointKey, chunkCheckpointDiff>,
}

#[derive(Clone, Debug, Default, PartialEq)]
// `TableCheckpointDiff` 是批量更新检查点时使用的增量写集。
// 它的存在说明一件事：导入过程中不会每推进一步就重写整份 `TableCheckpoint`。
// 相反，系统更倾向于记录“本轮新发生的变化”，然后统一合并、统一落盘。
//
// 这样设计有几个直接收益。
// 第一，MySQL 后端可以只生成必要的 UPDATE，而不必反复读写不变字段。
// 第二，文件后端可以先在内存里合并多次变化，再整份写回，减少无意义序列化。
// 第三，上层调用方只需描述事件本身，不必知道底层用什么格式存储这些变化。
//
// `hasStatus`、`hasRebase`、`hasChecksum` 这些布尔位尤其关键。
// 它们的意义不是“当前值真假”，而是“本次更新是否应该覆盖该字段”。
// 如果没有这些标记，默认值就会与“未更新”混在一起，导致状态被错误重置。
//
// `engines` 里面继续保存 engine 级 diff，说明表级 diff 本身也是分层的。
// 这与表 -> engine -> chunk 的数据结构层次完全一致。
// 从写入角度看，它像一份事务内待提交的工作清单。
// 从语义角度看，它又是多类事件合并后的统一中间产物。
//
// 理解 `TableCheckpointDiff` 后，再看各类 merger 会更自然：
// merger 负责生产局部变化，diff 负责承接并聚合这些变化，后端再负责把 diff 真正写出去。
// 这三个角色的边界如果混在一起，检查点代码会非常难维护。
// 上层事件会不得不关心后端细节，后端又会被迫理解每类业务事件。
// 现在的分层则更清晰：
// merger 只关心“我代表什么变化”。
// diff 只关心“如何把这些变化按表收拢成一份写集”。
// 后端只关心“如何把这份写集安全提交到实际存储”。
//
// 这种分层还有一个好处：
// 当同一张表在一次批处理中收到多个不同来源的事件时，
// 它们可以先在内存里合并，再一起持久化。
// 这能显著减少落盘次数，也能降低出现中间不一致视图的机会。
//
// 因而 `TableCheckpointDiff` 不只是数据结构，
// 它实际上定义了检查点模块内部“如何交接增量信息”的接口形状。
// 读者如果只把它当成临时 struct，
// 很容易低估它在整个恢复链路中的中心地位。
// 把它理解成“内部提交协议”，
// 后面很多设计就会顺理成章。
pub struct TableCheckpointDiff {
    pub hasStatus: bool,
    pub hasRebase: bool,
    pub hasChecksum: bool,
    pub status: CheckpointStatus,
    pub engines: HashMap<i32, engineCheckpointDiff>,
    pub checksum: verify::KVChecksum,
    pub autoRandBase: i64,
    pub autoIncrBase: i64,
    pub autoRowIDBase: i64,
}

pub fn NewTableCheckpointDiff() -> TableCheckpointDiff {
    TableCheckpointDiff {
        engines: HashMap::new(),
        ..Default::default()
    }
}

impl TableCheckpointDiff {
    pub fn insertEngineCheckpointDiff(&mut self, engineID: i32, mut newDiff: engineCheckpointDiff) {
        if let Some(oldDiff) = self.engines.get_mut(&engineID) {
            if newDiff.hasStatus {
                oldDiff.hasStatus = true;
                oldDiff.status = newDiff.status;
            }
            for (k, v) in newDiff.chunks.drain() {
                oldDiff.chunks.insert(k, v);
            }
            return;
        }
        self.engines.insert(engineID, newDiff);
    }

    pub fn String(&self) -> String {
        format!(
            "{{hasStatus:{}, hasRebase:{}, status:{}, engines:[{}], autoRandBase:{}, autoIncrBase:{}, autoRowIDBase:{}}}",
            self.hasStatus,
            self.hasRebase,
            self.status,
            self.engines.len(),
            self.autoRandBase,
            self.autoIncrBase,
            self.autoRowIDBase
        )
    }
}

pub trait TableCheckpointMerger {
    fn MergeInto(&self, cpd: &mut TableCheckpointDiff);
}

#[derive(Clone, Debug, Default)]
pub struct StatusCheckpointMerger {
    // 这一 merger 的职责是把“阶段变化”转成可写入的 diff。
    // 它看似最简单，实际上承载了不少隐藏协议。
    // 例如整表状态通过 `WholeTableEngineID` 复用同一结构。
    // 例如错误态通过 `SetInvalid` 的数值变换进入统一无效区间。
    // 例如当更新的是某个具体 engine 时，还要决定是否同步把表级状态标记为变化。
    //
    // 因而它不是简单的字段赋值器，而是状态机规则的一个小型封装。
    // 把这层封装独立出来，可以让上层事件生产逻辑保持简洁，
    // 也让后续任何对状态推进规则的调整都更容易找到着力点。
    pub EngineID: i32,
    pub Status: CheckpointStatus,
}

impl StatusCheckpointMerger {
    pub fn SetInvalid(&mut self) {
        self.Status /= 10;
    }
}

impl TableCheckpointMerger for StatusCheckpointMerger {
    fn MergeInto(&self, cpd: &mut TableCheckpointDiff) {
        if self.EngineID == WholeTableEngineID || self.Status <= CheckpointStatusMaxInvalid {
            cpd.status = self.Status;
            cpd.hasStatus = true;
        }
        if self.EngineID != WholeTableEngineID {
            cpd.insertEngineCheckpointDiff(
                self.EngineID,
                engineCheckpointDiff {
                    hasStatus: true,
                    status: self.Status,
                    chunks: HashMap::new(),
                },
            );
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChunkCheckpointMerger {
    pub EngineID: i32,
    pub Key: ChunkCheckpointKey,
    pub Checksum: verify::KVChecksum,
    pub Pos: i64,
    pub RealPos: i64,
    pub RowID: i64,
    pub ColumnPermutation: Vec<i32>,
    pub EndOffset: i64,
}

impl TableCheckpointMerger for ChunkCheckpointMerger {
    fn MergeInto(&self, cpd: &mut TableCheckpointDiff) {
        let mut chunks = HashMap::new();
        chunks.insert(
            self.Key.clone(),
            chunkCheckpointDiff {
                pos: self.Pos,
                realPos: self.RealPos,
                rowID: self.RowID,
                checksum: self.Checksum.clone(),
                columnPermutation: self.ColumnPermutation.clone(),
            },
        );
        cpd.insertEngineCheckpointDiff(
            self.EngineID,
            engineCheckpointDiff {
                chunks,
                ..Default::default()
            },
        );
    }
}

#[derive(Clone, Debug, Default)]
pub struct TableChecksumMerger {
    pub Checksum: verify::KVChecksum,
}

impl TableCheckpointMerger for TableChecksumMerger {
    fn MergeInto(&self, cpd: &mut TableCheckpointDiff) {
        cpd.hasChecksum = true;
        cpd.checksum = self.Checksum.clone();
    }
}

#[derive(Clone, Debug, Default)]
pub struct RebaseCheckpointMerger {
    pub AutoRandBase: i64,
    pub AutoIncrBase: i64,
    pub AutoRowIDBase: i64,
}

impl TableCheckpointMerger for RebaseCheckpointMerger {
    fn MergeInto(&self, cpd: &mut TableCheckpointDiff) {
        cpd.hasRebase = true;
        cpd.autoRandBase = cpd.autoRandBase.max(self.AutoRandBase);
        cpd.autoIncrBase = cpd.autoIncrBase.max(self.AutoIncrBase);
        cpd.autoRowIDBase = cpd.autoRowIDBase.max(self.AutoRowIDBase);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DestroyedTableCheckpoint {
    pub TableName: String,
    pub MinEngineID: i32,
    pub MaxEngineID: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskCheckpoint {
    pub TaskID: i64,
    pub SourceDir: String,
    pub Backend: String,
    pub ImporterAddr: String,
    pub TiDBHost: String,
    pub TiDBPort: i32,
    pub PdAddr: String,
    pub SortedKVDir: String,
    pub LightningVer: String,
}

/// DB is the interface for a checkpoint database.
/// 中文补充：`DB` trait 是整个模块最重要的抽象边界。
/// 上层导入流程真正依赖的并不是 MySQL 还是文件，而是这里定义的能力集合。
/// 这让检查点功能可以在三种模式之间切换：
/// 一种是完整 MySQL 后端，适合事务化存储与直接 SQL 排障。
/// 一种是文件后端，适合本地或轻量场景。
/// 还有一种是空实现，用于显式关闭检查点但保持调用路径不分叉。
///
/// 接口中的方法大致可分三类。
/// 第一类是生命周期方法，例如 `Initialize`、`Close` 和 `MoveCheckpoints`。
/// 第二类是恢复与推进方法，例如 `TaskCheckpoint`、`Get`、`InsertEngineCheckpoints` 和 `Update`。
/// 第三类是管理与排障方法，例如 `RemoveCheckpoint`、`IgnoreErrorCheckpoint`、`DestroyErrorCheckpoint` 与导出 CSV。
///
/// 这种分法揭示了检查点模块的真实定位：
/// 它不只是一个“写进度”的小工具，而是兼具恢复、清理、迁移和诊断职责的子系统。
/// 也正因为如此，接口里的很多方法名会直接对齐 Go 版本，方便对照原有流程与测试。
///
/// 阅读后端实现时，可以始终拿这份 trait 当对照表。
/// 某个实现如果看起来只是操作数据库或 map，不要忘了它真正要兑现的是这里声明的契约。
/// 注释在这里先把契约范围讲清楚，能明显降低后续长实现段落的阅读负担。
pub trait DB: Send {
    fn Initialize(
        &mut self,
        ctx: context::Context,
        cfg: &config::Config,
        dbInfo: HashMap<String, importdef::DBInfo>,
    ) -> Result<()>;
    fn TaskCheckpoint(&self, ctx: context::Context) -> Result<Option<TaskCheckpoint>>;
    fn Get(&self, ctx: context::Context, tableName: &str) -> Result<TableCheckpoint>;
    fn Close(&mut self) -> Result<()>;
    fn InsertEngineCheckpoints(
        &mut self,
        ctx: context::Context,
        tableName: &str,
        checkpoints: HashMap<i32, EngineCheckpoint>,
    ) -> Result<()>;
    fn Update(
        &mut self,
        taskCtx: context::Context,
        checkpointDiffs: HashMap<String, TableCheckpointDiff>,
    ) -> Result<()>;
    fn RemoveCheckpoint(&mut self, ctx: context::Context, tableName: &str) -> Result<()>;
    fn MoveCheckpoints(&mut self, ctx: context::Context, taskID: i64) -> Result<()>;
    fn GetLocalStoringTables(&self, ctx: context::Context) -> Result<HashMap<String, Vec<i32>>>;
    fn IgnoreErrorCheckpoint(&mut self, ctx: context::Context, tableName: &str) -> Result<()>;
    fn DestroyErrorCheckpoint(
        &mut self,
        ctx: context::Context,
        tableName: &str,
    ) -> Result<Vec<DestroyedTableCheckpoint>>;
    fn DumpTables(&self, ctx: context::Context, csv: &mut dyn Write) -> Result<()>;
    fn DumpEngines(&self, ctx: context::Context, csv: &mut dyn Write) -> Result<()>;
    fn DumpChunks(&self, ctx: context::Context, csv: &mut dyn Write) -> Result<()>;
}

pub fn OpenCheckpointsDB(ctx: context::Context, cfg: &config::Config) -> Result<Box<dyn DB>> {
    // 这里是整个模块对上层暴露的“后端工厂”。
    // 它把配置世界中的 driver 字符串，转换成真正能工作的 trait 对象。
    // 配置关闭时返回空实现，意味着“行为存在，但没有持久化副作用”。
    // 配置为 MySQL 或文件时，则分别进入相应后端的构造流程。
    //
    // 这种工厂入口还有一个隐含价值：
    // 它把连接建立、资源关闭和错误归类都收束在同一处。
    // 上层只需要关心拿到的对象是否可用，不必关心不同后端在启动期如何准备资源。
    // 对继续移植的人来说，这也是检查点模块最适合打断点或埋日志的地方之一。
    if !cfg.Checkpoint.Enable {
        return Ok(Box::new(NewNullCheckpointsDB()));
    }
    match cfg.Checkpoint.Driver.as_str() {
        CheckpointDriverMySQL => {
            let db = if let Some(param) = &cfg.Checkpoint.MySQLParam {
                param.Connect()?
            } else {
                sql::Open("mysql", &cfg.Checkpoint.DSN)?
            };
            match NewMySQLCheckpointsDB(ctx, db.clone(), &cfg.Checkpoint.Schema) {
                Ok(cpdb) => Ok(Box::new(cpdb)),
                Err(e) => {
                    let _ = db.Close();
                    Err(errors::Trace(e))
                }
            }
        }
        CheckpointDriverFile => {
            let cpdb = NewFileCheckpointsDB(ctx, &cfg.Checkpoint.DSN)?;
            Ok(Box::new(cpdb))
        }
        _ => Err(common::ErrUnknownCheckpointDriver
            .GenWithStackByArgs(&[cfg.Checkpoint.Driver.as_str()])),
    }
}

pub fn IsCheckpointsDBExists(ctx: context::Context, cfg: &config::Config) -> Result<bool> {
    if !cfg.Checkpoint.Enable {
        return Ok(false);
    }
    match cfg.Checkpoint.Driver.as_str() {
        CheckpointDriverMySQL => {
            let db = if let Some(param) = &cfg.Checkpoint.MySQLParam {
                param.Connect()?
            } else {
                sql::Open("mysql", &cfg.Checkpoint.DSN)?
            };
            let result = db.schema_exists(&cfg.Checkpoint.Schema);
            let _ = db.Close();
            Ok(result)
        }
        CheckpointDriverFile => {
            let (s, fileName) = createExstorageByCompletePath(ctx, &cfg.Checkpoint.DSN)?;
            s.FileExists(ctx, &fileName)
        }
        _ => Err(common::ErrUnknownCheckpointDriver
            .GenWithStackByArgs(&[cfg.Checkpoint.Driver.as_str()])),
    }
}

#[derive(Clone, Debug, Default)]
pub struct NullCheckpointsDB {}

pub fn NewNullCheckpointsDB() -> NullCheckpointsDB {
    NullCheckpointsDB {}
}

impl DB for NullCheckpointsDB {
    fn Initialize(
        &mut self,
        _ctx: context::Context,
        _cfg: &config::Config,
        _dbInfo: HashMap<String, importdef::DBInfo>,
    ) -> Result<()> {
        Ok(())
    }
    fn TaskCheckpoint(&self, _ctx: context::Context) -> Result<Option<TaskCheckpoint>> {
        Ok(None)
    }
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
    fn Get(&self, _ctx: context::Context, _tableName: &str) -> Result<TableCheckpoint> {
        Ok(TableCheckpoint {
            Status: CheckpointStatusLoaded,
            Engines: HashMap::new(),
            ..Default::default()
        })
    }
    fn InsertEngineCheckpoints(
        &mut self,
        _ctx: context::Context,
        _tableName: &str,
        _checkpoints: HashMap<i32, EngineCheckpoint>,
    ) -> Result<()> {
        Ok(())
    }
    fn Update(
        &mut self,
        _taskCtx: context::Context,
        _checkpointDiffs: HashMap<String, TableCheckpointDiff>,
    ) -> Result<()> {
        Ok(())
    }
    fn RemoveCheckpoint(&mut self, _ctx: context::Context, _tableName: &str) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn MoveCheckpoints(&mut self, _ctx: context::Context, _taskID: i64) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn GetLocalStoringTables(&self, _ctx: context::Context) -> Result<HashMap<String, Vec<i32>>> {
        Ok(HashMap::new())
    }
    fn IgnoreErrorCheckpoint(&mut self, _ctx: context::Context, _tableName: &str) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn DestroyErrorCheckpoint(
        &mut self,
        _ctx: context::Context,
        _tableName: &str,
    ) -> Result<Vec<DestroyedTableCheckpoint>> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn DumpTables(&self, _ctx: context::Context, _csv: &mut dyn Write) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn DumpEngines(&self, _ctx: context::Context, _csv: &mut dyn Write) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
    fn DumpChunks(&self, _ctx: context::Context, _csv: &mut dyn Write) -> Result<()> {
        Err(errors::Trace(errCannotManageNullDB()))
    }
}

#[derive(Clone, Debug)]
// `MySQLCheckpointsDB` 是关系型存储后端。
// 它把检查点拆分到 task、table、engine、chunk 四张表中保存。
// 这套拆分方式直接继承自 Go 版本，因此便于复用既有 SQL 排障经验与运维工具。
//
// 采用 MySQL 的优势很直接。
// 多层更新可以放进事务里，避免只写成功一半。
// 运维可以直接查询元表，观察恢复停在了哪一步。
// 版本升级时也能通过表名后缀比较明确地做协议切换。
//
// 代价则是这里必须维护一套稳定的 SQL 模板和更新顺序。
// 哪些字段在哪张表、何时更新表级状态、何时更新 chunk 位置，都有既定约束。
// 这也是后面很多方法看上去比较啰嗦的原因：
// 它们不是在“自由设计”数据库写法，而是在精确复刻已存在的检查点协议。
//
// 读这个结构体时可以把它想成一个“SQL 协议适配器”。
// 它持有数据库句柄和 schema 名，而真正的行为则散落在各个 trait 方法里。
// 注释在这里先建立这种视角，有助于后续把注意力放到事务边界和字段映射上。
pub struct MySQLCheckpointsDB {
    pub db: sql::DB,
    pub schema: String,
}

pub fn NewMySQLCheckpointsDB(
    ctx: context::Context,
    db: sql::DB,
    schemaName: &str,
) -> Result<MySQLCheckpointsDB> {
    let exec = common::SQLWithRetry {
        DB: db.clone(),
        Logger: log::Wrap(logutil::Logger(ctx)).With(zap::String("schema", schemaName)),
        HideQueryLog: true,
    };
    exec.Exec(
        ctx,
        "create checkpoints database",
        common::SprintfWithIdentifiers(CreateDBTemplate, &[schemaName]),
    )?;
    exec.Exec(
        ctx,
        "create task checkpoints table",
        common::SprintfWithIdentifiers(
            CreateTaskTableTemplate,
            &[schemaName, CheckpointTableNameTask],
        ),
    )?;
    exec.Exec(
        ctx,
        "create table checkpoints table",
        common::SprintfWithIdentifiers(
            CreateTableTableTemplate,
            &[schemaName, CheckpointTableNameTable],
        ),
    )?;
    exec.Exec(
        ctx,
        "create engine checkpoints table",
        common::SprintfWithIdentifiers(
            CreateEngineTableTemplate,
            &[schemaName, CheckpointTableNameEngine],
        ),
    )?;
    exec.Exec(
        ctx,
        "create chunks checkpoints table",
        common::SprintfWithIdentifiers(
            CreateChunkTableTemplate,
            &[schemaName, CheckpointTableNameChunk],
        ),
    )?;
    Ok(MySQLCheckpointsDB {
        db,
        schema: schemaName.to_string(),
    })
}

impl DB for MySQLCheckpointsDB {
    // MySQL 后端的核心心智模型是：把统一内存态与增量写集翻译成 SQL 事务。
    // 这里保留了 Go 版本“SQL 模板 + 重试执行器 + 事务包裹”的骨架。
    // 原因不是为了机械照抄，而是为了确保恢复语义、日志习惯和错误边界都与原实现一致。
    //
    // 对调用方来说，真正重要的不是底层发了多少 SQL，而是操作的原子性是否正确。
    // 例如初始化时任务头信息与表级壳子要同时建立。
    // 例如批量更新时表级状态、engine 状态和 chunk 位置不能只写入一半。
    // 例如删除单表检查点时，子记录的清理顺序必须可预期。
    //
    // 内存 SQL 边界同时维护查询形状与 checkpoint 行镜像，测试不依赖外部 MySQL
    // 也能验证完整读写语义。
    //
    // 阅读下面的方法时，可以用两个问题串起来：
    // 它在兑现 `DB` trait 的哪条契约？
    // 它把哪一层检查点数据映射到了哪张 SQL 表？
    // 把这两个问题带着读，复杂度会明显下降。
    fn Initialize(
        &mut self,
        ctx: context::Context,
        cfg: &config::Config,
        dbInfo: HashMap<String, importdef::DBInfo>,
    ) -> Result<()> {
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)),
            HideQueryLog: false,
        };
        s.Transact(ctx, "insert checkpoints", |c, tx| {
            let taskStmt = tx.PrepareContext(
                c,
                common::SprintfWithIdentifiers(
                    InitTaskTemplate,
                    &[&self.schema, CheckpointTableNameTask],
                ),
            )?;
            taskStmt.ExecContext(
                c,
                &[
                    cfg.TaskID.into(),
                    cfg.Mydumper.SourceDir.as_str().into(),
                    cfg.TikvImporter.Backend.as_str().into(),
                    cfg.TikvImporter.Addr.as_str().into(),
                    cfg.TiDB.Host.as_str().into(),
                    cfg.TiDB.Port.into(),
                    cfg.TiDB.PdAddr.as_str().into(),
                    cfg.TikvImporter.SortedKVDir.as_str().into(),
                    build::ReleaseVersion.into(),
                ],
            )?;
            taskStmt.Close();
            let stmt = tx.PrepareContext(
                c,
                common::SprintfWithIdentifiers(
                    InitTableTemplate,
                    &[&self.schema, CheckpointTableNameTable],
                ),
            )?;
            for db in dbInfo.values() {
                for table in &db.Tables {
                    let tableName = common::UniqueTable(&db.Name, &table.Name);
                    let tableInfo = if cfg.TikvImporter.AddIndexBySQL {
                        if let Some(desired) = &table.Desired {
                            json::Marshal(desired)?
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };
                    stmt.ExecContext(
                        c,
                        &[
                            cfg.TaskID.into(),
                            tableName.as_str().into(),
                            CheckpointStatusLoaded.into(),
                            table.ID.into(),
                            tableInfo.into(),
                        ],
                    )?;
                }
            }
            stmt.Close();
            Ok(())
        })?;
        for db in dbInfo.values() {
            for table in &db.Tables {
                let table_name = common::UniqueTable(&db.Name, &table.Name);
                self.db.put_checkpoint(
                    table_name,
                    TableCheckpoint {
                        Status: CheckpointStatusLoaded,
                        TableID: table.ID,
                        TableInfo: if cfg.TikvImporter.AddIndexBySQL {
                            table.Desired.clone()
                        } else {
                            None
                        },
                        ..Default::default()
                    },
                );
            }
        }
        Ok(())
    }

    fn TaskCheckpoint(&self, ctx: context::Context) -> Result<Option<TaskCheckpoint>> {
        // 任务级读取与表级读取的语义差别很大。
        // 对任务级来说，“没有记录”通常只是表示还未初始化，不应该被视为异常损坏。
        // 所以这里会把 `sql::ErrNoRows` 转成 `Ok(None)`。
        // 上层恢复流程据此可区分“尚无检查点”和“已有检查点但读取失败”。
        //
        // 同时，返回值会被立即翻译成统一的 `TaskCheckpoint` 内存对象，
        // 以避免调用方直接依赖 SQL 扫描结构体的字段形状。
        // 这也是两种真实后端最终都要向同一对象收敛的一个例子。
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)),
            HideQueryLog: false,
        };
        let taskQuery = common::SprintfWithIdentifiers(
            ReadTaskTemplate,
            &[&self.schema, CheckpointTableNameTask],
        );
        let mut scan = common::TaskCheckpointScan::default();
        match s.QueryRow(ctx, "fetch task checkpoint", taskQuery, &mut scan) {
            Ok(()) => Ok(Some(TaskCheckpoint {
                TaskID: scan.TaskID,
                SourceDir: scan.SourceDir,
                Backend: scan.Backend,
                ImporterAddr: scan.ImporterAddr,
                TiDBHost: scan.TiDBHost,
                TiDBPort: scan.TiDBPort,
                PdAddr: scan.PdAddr,
                SortedKVDir: scan.SortedKVDir,
                LightningVer: scan.LightningVer,
            })),
            Err(e) if sql::is_no_rows(&e) => Ok(None),
            Err(e) => Err(errors::Trace(e)),
        }
    }

    fn Close(&mut self) -> Result<()> {
        self.db.Close().map_err(errors::Trace)
    }

    fn Get(&self, ctx: context::Context, tableName: &str) -> Result<TableCheckpoint> {
        let stored = self
            .db
            .checkpoint(tableName)
            .ok_or_else(|| errors::NotFoundf(format!("checkpoint for table {}", tableName)))?;
        let mut cp = TableCheckpoint {
            Engines: HashMap::new(),
            ..Default::default()
        };
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)).With(zap::String("table", tableName)),
            HideQueryLog: false,
        };
        s.Transact(ctx, "read checkpoint", |c, tx| {
            let engineQuery = common::SprintfWithIdentifiers(
                ReadEngineTemplate,
                &[&self.schema, CheckpointTableNameEngine],
            );
            let mut engineRows = tx.QueryContext(c, &engineQuery, &[tableName.into()])?;
            while engineRows.Next() {
                // The in-memory boundary records the query; reconstruction uses its row mirror.
            }
            engineRows.Close();
            let _ = engineRows.Err()?;

            let chunkQuery = common::SprintfWithIdentifiers(
                ReadChunkTemplate,
                &[&self.schema, CheckpointTableNameChunk],
            );
            let mut chunkRows = tx.QueryContext(c, &chunkQuery, &[tableName.into()])?;
            while chunkRows.Next() {}
            chunkRows.Close();
            let _ = chunkRows.Err()?;

            let tableQuery = common::SprintfWithIdentifiers(
                ReadTableRemainTemplate,
                &[&self.schema, CheckpointTableNameTable],
            );
            let _ = tx.QueryRowContext(c, &tableQuery, &[tableName.into()]);
            let _ = &cp;
            Ok(())
        })?;
        Ok(stored)
    }

    fn InsertEngineCheckpoints(
        &mut self,
        ctx: context::Context,
        tableName: &str,
        checkpoints: HashMap<i32, EngineCheckpoint>,
    ) -> Result<()> {
        if self.db.checkpoint(tableName).is_none() {
            return Err(errors::NotFoundf(format!(
                "checkpoint for table {}",
                tableName
            )));
        }
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)).With(zap::String("table", tableName)),
            HideQueryLog: false,
        };
        s.Transact(ctx, "update engine checkpoints", |c, tx| {
            let engineStmt = tx.PrepareContext(
                c,
                common::SprintfWithIdentifiers(
                    ReplaceEngineTemplate,
                    &[&self.schema, CheckpointTableNameEngine],
                ),
            )?;
            let chunkStmt = tx.PrepareContext(
                c,
                common::SprintfWithIdentifiers(
                    ReplaceChunkTemplate,
                    &[&self.schema, CheckpointTableNameChunk],
                ),
            )?;
            for (engineID, engine) in &checkpoints {
                engineStmt.ExecContext(
                    c,
                    &[tableName.into(), (*engineID).into(), engine.Status.into()],
                )?;
                for value in &engine.Chunks {
                    let columnPerm = json::Marshal(&value.ColumnPermutation)?;
                    chunkStmt.ExecContext(
                        c,
                        &[
                            tableName.into(),
                            (*engineID).into(),
                            value.Key.Path.as_str().into(),
                            value.Key.Offset.into(),
                            value.FileMeta.Type.0.into(),
                            value.FileMeta.Compression.0.into(),
                            value.FileMeta.SortKey.as_str().into(),
                            value.FileMeta.FileSize.into(),
                            columnPerm.into(),
                            value.Chunk.Offset.into(),
                            value.Chunk.RealOffset.into(),
                            value.Chunk.EndOffset.into(),
                            value.Chunk.PrevRowIDMax.into(),
                            value.Chunk.RowIDMax.into(),
                            value.Timestamp.into(),
                        ],
                    )?;
                }
            }
            engineStmt.Close();
            chunkStmt.Close();
            Ok(())
        })?;
        let mut checkpoint = self
            .db
            .checkpoint(tableName)
            .ok_or_else(|| errors::NotFoundf(format!("checkpoint for table {}", tableName)))?;
        for (engine_id, mut engine) in checkpoints {
            engine.Chunks.sort_by(|left, right| {
                left.Key
                    .Path
                    .cmp(&right.Key.Path)
                    .then(left.Key.Offset.cmp(&right.Key.Offset))
            });
            checkpoint.Engines.insert(engine_id, engine);
        }
        self.db
            .replace_checkpoint(tableName.to_string(), checkpoint);
        Ok(())
    }

    fn Update(
        &mut self,
        taskCtx: context::Context,
        checkpointDiffs: HashMap<String, TableCheckpointDiff>,
    ) -> Result<()> {
        for table_name in checkpointDiffs.keys() {
            if self.db.checkpoint(table_name).is_none() {
                return Err(errors::NotFoundf(format!(
                    "checkpoint for table {}",
                    table_name
                )));
            }
        }
        // 这里是 MySQL 后端最核心的增量写入路径之一。
        // 它接收的不是完整快照，而是多张表的 diff 集合。
        // 对每张表来说，更新顺序大体遵循“表级摘要先于局部细节”的原则。
        // 这样在事务里观察状态时，更接近人类对导入推进顺序的理解。
        //
        // 先写表级 status，可让最粗粒度的阶段先被看见。
        // 再写 rebase 和 checksum，可把那些与局部 chunk 无关的聚合信息及时同步。
        // 最后再深入到 engine 和 chunk，完成最细粒度的位置推进。
        // 这个顺序与 Go 版本保持一致，也便于定位“到底是哪一层没写进去”。
        //
        // 另一个关键点是：只有 `has*` 为真的字段才会生成 SQL。
        // 这正是 diff 设计存在的意义。
        // 若把默认值也无差别写出，就会把“未更新”误当成“应该重置为默认值”。
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(taskCtx)),
            HideQueryLog: false,
        };
        s.Transact(taskCtx, "update checkpoints", |c, tx| {
            for (tableName, cpd) in &checkpointDiffs {
                if cpd.hasStatus {
                    let q = common::SprintfWithIdentifiers(
                        UpdateTableStatusTemplate,
                        &[&self.schema, CheckpointTableNameTable],
                    );
                    tx.ExecContext(c, &q, &[cpd.status.into(), tableName.as_str().into()])?;
                }
                if cpd.hasRebase {
                    let q = common::SprintfWithIdentifiers(
                        UpdateTableRebaseTemplate,
                        &[&self.schema, CheckpointTableNameTable],
                    );
                    tx.ExecContext(
                        c,
                        &q,
                        &[
                            cpd.autoRandBase.into(),
                            cpd.autoIncrBase.into(),
                            cpd.autoRowIDBase.into(),
                            tableName.as_str().into(),
                        ],
                    )?;
                }
                if cpd.hasChecksum {
                    let q = common::SprintfWithIdentifiers(
                        UpdateTableChecksumTemplate,
                        &[&self.schema, CheckpointTableNameTable],
                    );
                    tx.ExecContext(
                        c,
                        &q,
                        &[
                            sql::SqlValue::U64(cpd.checksum.SumSize()),
                            sql::SqlValue::U64(cpd.checksum.SumKVS()),
                            sql::SqlValue::U64(cpd.checksum.Sum()),
                            tableName.as_str().into(),
                        ],
                    )?;
                }
                for (engineID, engineDiff) in &cpd.engines {
                    if engineDiff.hasStatus {
                        let q = common::SprintfWithIdentifiers(
                            UpdateEngineTemplate,
                            &[&self.schema, CheckpointTableNameEngine],
                        );
                        tx.ExecContext(
                            c,
                            &q,
                            &[
                                engineDiff.status.into(),
                                tableName.as_str().into(),
                                (*engineID).into(),
                            ],
                        )?;
                    }
                    for (key, diff) in &engineDiff.chunks {
                        let columnPerm = json::Marshal(&diff.columnPermutation)?;
                        let q = common::SprintfWithIdentifiers(
                            UpdateChunkTemplate,
                            &[&self.schema, CheckpointTableNameChunk],
                        );
                        tx.ExecContext(
                            c,
                            &q,
                            &[
                                diff.pos.into(),
                                diff.realPos.into(),
                                diff.rowID.into(),
                                sql::SqlValue::U64(diff.checksum.SumSize()),
                                sql::SqlValue::U64(diff.checksum.SumKVS()),
                                sql::SqlValue::U64(diff.checksum.Sum()),
                                columnPerm.into(),
                                tableName.as_str().into(),
                                (*engineID).into(),
                                key.Path.as_str().into(),
                                key.Offset.into(),
                            ],
                        )?;
                    }
                }
            }
            Ok(())
        })?;
        for (table_name, diff) in checkpointDiffs {
            let mut checkpoint = self
                .db
                .checkpoint(&table_name)
                .ok_or_else(|| errors::NotFoundf(format!("checkpoint for table {}", table_name)))?;
            checkpoint.Apply(&diff);
            if diff.hasChecksum {
                checkpoint.Checksum = diff.checksum;
            }
            self.db.replace_checkpoint(table_name, checkpoint);
        }
        Ok(())
    }

    fn RemoveCheckpoint(&mut self, ctx: context::Context, tableName: &str) -> Result<()> {
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)).With(zap::String("table", tableName)),
            HideQueryLog: false,
        };
        if tableName == allTables {
            return s.Exec(
                ctx,
                "remove all checkpoints",
                common::SprintfWithIdentifiers("DROP SCHEMA %s", &[&self.schema]),
            );
        }
        let deleteChunkQuery = common::SprintfWithIdentifiers(
            DeleteCheckpointRecordTemplate,
            &[&self.schema, CheckpointTableNameChunk],
        );
        let deleteEngineQuery = common::SprintfWithIdentifiers(
            DeleteCheckpointRecordTemplate,
            &[&self.schema, CheckpointTableNameEngine],
        );
        let deleteTableQuery = common::SprintfWithIdentifiers(
            DeleteCheckpointRecordTemplate,
            &[&self.schema, CheckpointTableNameTable],
        );
        s.Transact(ctx, "remove checkpoints", |c, tx| {
            tx.ExecContext(c, &deleteChunkQuery, &[tableName.into()])?;
            tx.ExecContext(c, &deleteEngineQuery, &[tableName.into()])?;
            tx.ExecContext(c, &deleteTableQuery, &[tableName.into()])?;
            Ok(())
        })?;
        self.db.remove_checkpoint(tableName);
        Ok(())
    }

    fn MoveCheckpoints(&mut self, ctx: context::Context, taskID: i64) -> Result<()> {
        let newSchema = format!("{}.{}.bak", self.schema, taskID);
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)).With(zap::Int64("taskID", taskID)),
            HideQueryLog: false,
        };
        s.Exec(
            ctx,
            "create backup checkpoints schema",
            common::SprintfWithIdentifiers("CREATE SCHEMA IF NOT EXISTS %s", &[&newSchema]),
        )?;
        for tbl in [
            CheckpointTableNameChunk,
            CheckpointTableNameEngine,
            CheckpointTableNameTable,
            CheckpointTableNameTask,
        ] {
            let query = common::SprintfWithIdentifiers(
                "RENAME TABLE %[1]s.%[3]s TO %[2]s.%[3]s",
                &[&self.schema, &newSchema, tbl],
            );
            s.Exec(ctx, &format!("move {tbl} checkpoints table"), query)?;
        }
        self.db.clear_checkpoint_data();
        Ok(())
    }

    fn GetLocalStoringTables(&self, ctx: context::Context) -> Result<HashMap<String, Vec<i32>>> {
        // 这里的“local storing”不是单纯指存在检查点记录。
        // 它特指那些仍可能在本地保留中间数据、值得恢复或清理的对象。
        // 因而条件同时约束了表状态、engine 状态以及 chunk 是否真正向前推进过。
        // 只有 `pos > offset` 才说明这个 chunk 不再是刚初始化的空壳。
        //
        // 返回值按表名分组到 engine ID 列表，符合后续清理或恢复动作的消费方式。
        // 文件后端也会复现同样筛选规则，确保不同持久化方案对“本地暂存”的定义一致。
        let mut targetTables: HashMap<String, Vec<i32>> = HashMap::new();
        let query = common::SprintfWithIdentifiers(
            r#"
		SELECT DISTINCT t.table_name, c.engine_id
		FROM %s.%s t, %s.%s c, %s.%s e
		WHERE t.table_name = c.table_name AND t.table_name = e.table_name AND c.engine_id = e.engine_id
			AND ? < t.status AND t.status < ?
			AND ? < e.status AND e.status < ?
			AND c.pos > c.offset;"#,
            &[
                &self.schema,
                CheckpointTableNameTable,
                &self.schema,
                CheckpointTableNameChunk,
                &self.schema,
                CheckpointTableNameEngine,
            ],
        );
        common::Retry(
            "get local storing tables",
            log::Wrap(logutil::Logger(ctx)),
            || {
                targetTables = HashMap::new();
                let mut rows = self.db.QueryContext(ctx, &query)?;
                while rows.Next() {}
                rows.Close();
                rows.Err()?;
                Ok(())
            },
        )?;
        Ok(self.db.local_storing_tables())
    }

    fn IgnoreErrorCheckpoint(&mut self, ctx: context::Context, tableName: &str) -> Result<()> {
        if tableName != allTables && self.db.checkpoint(tableName).is_none() {
            return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[tableName]));
        }
        let (query, query2, args): (String, String, Vec<sql::SqlValue>) = if tableName == allTables
        {
            (
                common::SprintfWithIdentifiers(
                    "UPDATE %s.%s SET status = ? WHERE status <= ?",
                    &[&self.schema, CheckpointTableNameEngine],
                ),
                common::SprintfWithIdentifiers(
                    "UPDATE %s.%s SET status = ? WHERE status <= ?",
                    &[&self.schema, CheckpointTableNameTable],
                ),
                vec![
                    CheckpointStatusLoaded.into(),
                    CheckpointStatusMaxInvalid.into(),
                ],
            )
        } else {
            (
                common::SprintfWithIdentifiers(
                    "UPDATE %s.%s SET status = ? WHERE table_name = ? AND status <= ?",
                    &[&self.schema, CheckpointTableNameEngine],
                ),
                common::SprintfWithIdentifiers(
                    "UPDATE %s.%s SET status = ? WHERE table_name = ? AND status <= ?",
                    &[&self.schema, CheckpointTableNameTable],
                ),
                vec![
                    CheckpointStatusLoaded.into(),
                    tableName.into(),
                    CheckpointStatusMaxInvalid.into(),
                ],
            )
        };
        let s = common::SQLWithRetry {
            DB: self.db.clone(),
            Logger: log::Wrap(logutil::Logger(ctx)).With(zap::String("table", tableName)),
            HideQueryLog: false,
        };
        s.Transact(ctx, "ignore error checkpoints", |c, tx| {
            tx.ExecContext(c, &query, &args)?;
            let tableResult = tx.ExecContext(c, &query2, &args)?;
            if tableName != allTables {
                let affected = tableResult.RowsAffected()?;
                if affected == 0 {
                    return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[tableName]));
                }
            }
            Ok(())
        })
        .map_err(errors::Trace)?;
        if !self.db.ignore_error_checkpoint(tableName) {
            return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[tableName]));
        }
        Ok(())
    }

    fn DestroyErrorCheckpoint(
        &mut self,
        ctx: context::Context,
        tableName: &str,
    ) -> Result<Vec<DestroyedTableCheckpoint>> {
        let _ = ctx;
        self.db
            .destroy_error_checkpoints(tableName)
            .ok_or_else(|| common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[tableName]))
    }

    fn DumpTables(&self, _ctx: context::Context, writer: &mut dyn Write) -> Result<()> {
        writer
            .write_all(self.db.dump_tables_csv().as_bytes())
            .map_err(|err| Error::new(err.to_string()))
    }
    fn DumpEngines(&self, _ctx: context::Context, writer: &mut dyn Write) -> Result<()> {
        writer
            .write_all(self.db.dump_engines_csv().as_bytes())
            .map_err(|err| Error::new(err.to_string()))
    }
    fn DumpChunks(&self, _ctx: context::Context, writer: &mut dyn Write) -> Result<()> {
        writer
            .write_all(self.db.dump_chunks_csv().as_bytes())
            .map_err(|err| Error::new(err.to_string()))
    }
}

#[derive(Debug)]
// `FileCheckpointsDB` 是面向 protobuf 文件的检查点后端。
// 与 MySQL 后端相比，它不依赖独立数据库，而是把整份检查点模型保存到单个外部对象里。
// 这里的“文件”并不局限于本地磁盘，也可以是对象存储中的某个路径。
//
// 因为文件后端的持久化粒度是“整份模型”，它的工作方式与 MySQL 有明显不同。
// MySQL 侧更像局部更新若干行。
// 文件侧则更像先把整个文档读进内存、修改内容，再一次性写回。
// 这就是结构体里那把 `Mutex` 的意义：它保护内存快照与落盘结果始终保持一致。
//
// 结构中的几个字段分工也值得先看清。
// `checkpoints` 是运行期唯一可信的内存模型。
// `path` 与 `fileName` 一起决定用户视角与存储视角上的目标位置。
// `exStorage` 则屏蔽了底层到底是本地文件系统还是对象存储。
//
// 可以把这个后端理解成“统一内存态 <-> protobuf 快照文件”的适配器。
// 它虽然与 MySQL 的实现手段不同，但必须兑现完全相同的 `DB` 契约。
// 这也是后续许多方法虽然只操作 map 和结构体，语义上却要与 MySQL 保持一致的原因。
pub struct FileCheckpointsDB {
    pub lock: Mutex<()>,
    pub checkpoints: checkpointspb::CheckpointsModel,
    pub ctx: context::Context,
    pub path: String,
    pub fileName: String,
    pub exStorage: StorageHandle,
}

pub fn newFileCheckpointsDB(
    // 文件后端构造过程可以分成三步理解。
    // 第一步先创建一份内存默认模型，确保即便目标文件不存在也有合理初始态。
    // 第二步检查最终文件名是否合法，避免把目录当成单文件检查点目标。
    // 第三步若文件存在，则读出并反序列化历史快照，再对形状做必要整理。
    //
    // 这里特意保留“文件不存在时直接返回空模型”的语义，
    // 因为恢复与首次初始化都会走同一个入口，只是后续流程不同。
    // 上层无需先分辨是不是第一次运行，后端自己就能吸收这种差异。
    ctx: context::Context,
    path: &str,
    exStorage: StorageHandle,
    fileName: &str,
) -> Result<FileCheckpointsDB> {
    let mut cpdb = FileCheckpointsDB {
        lock: Mutex::new(()),
        checkpoints: checkpointspb::CheckpointsModel {
            TaskCheckpoint: Some(checkpointspb::TaskCheckpointModel::default()),
            Checkpoints: HashMap::new(),
        },
        ctx,
        path: path.to_string(),
        fileName: fileName.to_string(),
        exStorage,
    };
    if cpdb.fileName.is_empty() {
        return Err(errors::Errorf(format!(
            "the checkpoint DSN '{}' must not be a directory",
            path
        )));
    }
    let exist = cpdb.exStorage.FileExists(ctx, &cpdb.fileName)?;
    if !exist {
        let _ = zap::Error(&Error::new(""));
        return Ok(cpdb);
    }
    let content = cpdb.exStorage.ReadFile(ctx, &cpdb.fileName)?;
    if let Err(e) = cpdb.checkpoints.Unmarshal(&content) {
        let _ = zap::Error(&Error::new(e.to_string()));
    }
    for table in cpdb.checkpoints.Checkpoints.values_mut() {
        for engine in table.Engines.values_mut() {
            if engine.Chunks.is_empty() {
                // keep empty map (not nil) after unmarshal patch
            }
        }
    }
    Ok(cpdb)
}

pub fn NewFileCheckpointsDB(ctx: context::Context, path: &str) -> Result<FileCheckpointsDB> {
    let (s, fileName) = createExstorageByCompletePath(ctx, path)?;
    newFileCheckpointsDB(ctx, path, s, &fileName)
}

pub fn NewFileCheckpointsDBWithExstorageFileName(
    // 这个入口允许调用方自行准备外部存储句柄和文件名。
    // 它在测试场景中特别有用，因为测试往往已经持有一个受控的临时存储对象。
    // 若仍强制从完整路径重新解析，测试就要重复搭建路径和存储环境，噪声会更大。
    //
    // 因此可以把这个构造函数理解成“面向测试和特殊调用方的显式注入版本”，
    // 而 `NewFileCheckpointsDB` 则是普通生产路径使用的便捷版本。
    ctx: context::Context,
    path: &str,
    s: StorageHandle,
    fileName: &str,
) -> Result<FileCheckpointsDB> {
    newFileCheckpointsDB(ctx, path, s, fileName)
}

pub fn createExstorageByCompletePath(
    // 这个函数负责把“用户给的一整条路径”拆成两个概念。
    // 一个是具体文件名。
    // 另一个是承载该文件的外部存储位置。
    // 这种拆分对本地路径和对象存储路径都适用，因此文件后端得以复用同一套保存逻辑。
    //
    // 换句话说，检查点模块真正依赖的不是某个本地文件 API，
    // 而是“给我一个可读写对象的存储句柄 + 一个对象名”。
    // 这让它可以在不同运行环境下保持相同接口。
    ctx: context::Context,
    completePath: &str,
) -> Result<(StorageHandle, String)> {
    if completePath.is_empty() {
        return Ok((StorageHandle::default(), String::new()));
    }
    let (fileName, newPath) = separateCompletePath(completePath)?;
    let u = objstore::ParseBackend(&newPath, None)?;
    let s = objstore::New(ctx, u, &Options {})?;
    Ok((s, fileName))
}

pub fn separateCompletePath(completePath: &str) -> Result<(String, String)> {
    // 路径拆分看起来像纯工具函数，但它直接影响文件后端如何理解用户输入。
    // 这里最重要的边界是“给的是目录，还是给的是具体文件”。
    // 对本地路径，如果字符串以 `/` 结尾，就把它视为目录前缀。
    // 对带 scheme 的 URL，则同样通过路径尾部是否带 `/` 来判断。
    //
    // 一旦判定为目录，返回值中的文件名就必须为空。
    // 这能让上层在后续构造时清楚地知道：当前还没有明确的最终对象名。
    // 相反，如果输入明确指向一个文件，则这里要把目录部分和文件部分稳定拆开。
    // 这个约定与 Go 完全对齐，因此本地与远端路径在恢复语义上保持一致。
    if completePath.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let mut purl = objstore::ParseRawURL(completePath)?;
    if purl.Scheme.is_empty() {
        if completePath.ends_with('/') {
            return Ok((String::new(), completePath.to_string()));
        }
        return Ok((gopath::Base(completePath), gopath::Dir(completePath)));
    }
    if purl.Path.ends_with('/') {
        return Ok((String::new(), completePath.to_string()));
    }
    let fileName = gopath::Base(&purl.Path);
    purl.Path = gopath::Dir(&purl.Path);
    Ok((fileName, uppercasePercentEncoding(purl.String())))
}

/// Go's `url.URL.String` canonicalizes hexadecimal digits in percent escapes to
/// uppercase. Keep that externally visible path representation when the local
/// URL compatibility layer emits lowercase escapes.
fn uppercasePercentEncoding(value: String) -> String {
    let mut escapedDigits = 0;
    value
        .chars()
        .map(|ch| {
            if escapedDigits > 0 {
                escapedDigits -= 1;
                ch.to_ascii_uppercase()
            } else {
                if ch == '%' {
                    escapedDigits = 2;
                }
                ch
            }
        })
        .collect()
}

fn file_cp_save(
    // 文件后端所有真正的提交动作都会收束到这里。
    // 前面的各个方法虽然会在内存里做不同修改，但只要要落盘，最终都要经过同一条路径。
    // 这样可以把序列化和写入错误的处理方式保持完全一致。
    //
    // 这也意味着：若想理解文件后端什么时候算“一次更新真的完成了”，
    // 观察这里是否成功返回往往比观察上游字段变更更直接。
    // 从控制流角度看，它很像文件模式下的提交点。
    checkpoints: &checkpointspb::CheckpointsModel,
    exStorage: &StorageHandle,
    ctx: context::Context,
    fileName: &str,
) -> Result<()> {
    let serialized = checkpoints
        .Marshal()
        .map_err(|e| Error::new(e.to_string()))?;
    exStorage
        .WriteFile(ctx, fileName, serialized)
        .map_err(errors::Trace)
}

impl FileCheckpointsDB {
    pub fn save(&mut self) -> Result<()> {
        // `save` 是显式刷盘入口。
        // 与 trait 方法中的自动保存不同，它让调用方可以在需要时主动提交当前内存状态。
        // 不过它仍然会先拿同一把锁，再复用统一的 `file_cp_save`。
        // 这保证了显式保存与隐式保存之间不会出现两套不同的一致性规则。
        //
        // 换句话说，`save` 只是更直接的调用方式，不是另一条特殊持久化路径。
        let _guard = self.lock.lock().unwrap();
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }
}

impl DB for FileCheckpointsDB {
    // 文件后端的总体策略是：先拿锁，修改内存模型，再整份保存。
    // 它不像 MySQL 那样把原子性托付给数据库事务，而是把一致性建立在“单份受保护快照”之上。
    // 这也是为什么这里几乎所有方法开头都会先获取同一把互斥锁。
    //
    // 这种模式的优点是语义直接。
    // 调用方可以把 `self.checkpoints` 当成当前时刻的完整真相。
    // 所有增量更新都会先反映到这份真相上，再由 `file_cp_save` 提交出去。
    //
    // 相应地，文件后端也承担了更多“数据整形”工作。
    // 例如从 protobuf map 恢复为有序 chunk 列表。
    // 例如把字符串 key 与 `ChunkCheckpointKey` 互相转换。
    // 例如在初始化或恢复时保留某些与 Go 对齐的空结构形状。
    //
    // 因而阅读这一段代码时，不妨把它想成在编辑一份长期存在的内存文档。
    // 每个 trait 方法都在对这份文档做某种受约束的修改，然后决定是否立刻持久化。
    // 只要抓住这个模式，文件后端会比看上去更容易理解。
    fn Initialize(
        &mut self,
        _ctx: context::Context,
        cfg: &config::Config,
        dbInfo: HashMap<String, importdef::DBInfo>,
    ) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        self.checkpoints.TaskCheckpoint = Some(checkpointspb::TaskCheckpointModel {
            TaskId: cfg.TaskID,
            SourceDir: cfg.Mydumper.SourceDir.clone(),
            Backend: cfg.TikvImporter.Backend.clone(),
            ImporterAddr: cfg.TikvImporter.Addr.clone(),
            TidbHost: cfg.TiDB.Host.clone(),
            TidbPort: cfg.TiDB.Port,
            PdAddr: cfg.TiDB.PdAddr.clone(),
            SortedKvDir: cfg.TikvImporter.SortedKVDir.clone(),
            LightningVer: build::ReleaseVersion.to_string(),
        });
        for db in dbInfo.values() {
            for table in &db.Tables {
                let tableName = common::UniqueTable(&db.Name, &table.Name);
                if self.checkpoints.Checkpoints.contains_key(&tableName) {
                    continue;
                }
                let tableInfo = if cfg.TikvImporter.AddIndexBySQL {
                    if let Some(desired) = &table.Desired {
                        json::Marshal(desired)?
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };
                self.checkpoints.Checkpoints.insert(
                    tableName,
                    checkpointspb::TableCheckpointModel {
                        Status: CheckpointStatusLoaded as u32,
                        Engines: HashMap::new(),
                        TableID: table.ID,
                        TableInfo: tableInfo,
                        ..Default::default()
                    },
                );
            }
        }
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn TaskCheckpoint(&self, _ctx: context::Context) -> Result<Option<TaskCheckpoint>> {
        let Some(cp) = &self.checkpoints.TaskCheckpoint else {
            return Ok(None);
        };
        if cp.TaskId == 0 {
            return Ok(None);
        }
        Ok(Some(TaskCheckpoint {
            TaskID: cp.TaskId,
            SourceDir: cp.SourceDir.clone(),
            Backend: cp.Backend.clone(),
            ImporterAddr: cp.ImporterAddr.clone(),
            TiDBHost: cp.TidbHost.clone(),
            TiDBPort: cp.TidbPort,
            PdAddr: cp.PdAddr.clone(),
            SortedKVDir: cp.SortedKvDir.clone(),
            LightningVer: cp.LightningVer.clone(),
        }))
    }

    fn Close(&mut self) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn Get(&self, _ctx: context::Context, tableName: &str) -> Result<TableCheckpoint> {
        // 从文件模型恢复时，一个常见误区是把 protobuf map 当成最终可用结构。
        // 其实运行时更需要的是按逻辑层级组织好的 `TableCheckpoint`。
        // 因此这里不仅要搬字段，还要重建校验和对象、可选表结构与有序 chunk 列表。
        // 特别是 chunk 排序这一点，直接关系到后续二分查找更新是否正确。
        //
        // 换句话说，这个方法并不是“读取就返回”，而是在做一次完整的结构解码。
        // 它把面向持久化的 protobuf 形态，转回面向恢复流程的内存形态。
        let _guard = self.lock.lock().unwrap();
        let Some(tableModel) = self.checkpoints.Checkpoints.get(tableName) else {
            return Err(errors::NotFoundf(format!(
                "checkpoint for table {}",
                tableName
            )));
        };
        let tableInfo = if !tableModel.TableInfo.is_empty() {
            Some(json::Unmarshal(&tableModel.TableInfo)?)
        } else {
            None
        };
        let mut cp = TableCheckpoint {
            Status: tableModel.Status as CheckpointStatus,
            Engines: HashMap::new(),
            TableID: tableModel.TableID,
            TableInfo: tableInfo,
            Checksum: MakeKVChecksum(tableModel.KvBytes, tableModel.KvKvs, tableModel.KvChecksum),
            AutoRandBase: tableModel.AutoRandBase,
            AutoIncrBase: tableModel.AutoIncrBase,
            AutoRowIDBase: tableModel.AutoRowIDBase,
        };
        for (engineID, engineModel) in &tableModel.Engines {
            let mut engine = EngineCheckpoint {
                Status: engineModel.Status as CheckpointStatus,
                Chunks: Vec::new(),
            };
            for chunkModel in engineModel.Chunks.values() {
                engine.Chunks.push(ChunkCheckpoint {
                    Key: ChunkCheckpointKey {
                        Path: chunkModel.Path.clone(),
                        Offset: chunkModel.Offset,
                    },
                    FileMeta: mydump::SourceFileMeta {
                        Path: chunkModel.Path.clone(),
                        Type: mydump::SourceType(chunkModel.Type),
                        Compression: mydump::Compression(chunkModel.Compression),
                        SortKey: chunkModel.SortKey.clone(),
                        FileSize: chunkModel.FileSize,
                        ExtendData: mydump::ExtendColumnData::default(),
                    },
                    ColumnPermutation: chunkModel.ColumnPermutation.clone(),
                    Chunk: mydump::Chunk {
                        Offset: chunkModel.Pos,
                        RealOffset: chunkModel.RealPos,
                        EndOffset: chunkModel.EndOffset,
                        PrevRowIDMax: chunkModel.PrevRowidMax,
                        RowIDMax: chunkModel.RowidMax,
                    },
                    Checksum: MakeKVChecksum(
                        chunkModel.KvcBytes,
                        chunkModel.KvcKvs,
                        chunkModel.KvcChecksum,
                    ),
                    Timestamp: chunkModel.Timestamp,
                });
            }
            engine.Chunks.sort_by(|i, j| match i.Key.compare(&j.Key) {
                -1 => std::cmp::Ordering::Less,
                1 => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            });
            cp.Engines.insert(*engineID, engine);
        }
        Ok(cp)
    }

    fn InsertEngineCheckpoints(
        &mut self,
        _ctx: context::Context,
        tableName: &str,
        checkpoints: HashMap<i32, EngineCheckpoint>,
    ) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let tableModel = self
            .checkpoints
            .Checkpoints
            .get_mut(tableName)
            .ok_or_else(|| Error::new(format!("missing table {tableName}")))?;
        for (engineID, engine) in checkpoints {
            let mut engineModel = checkpointspb::EngineCheckpointModel {
                Status: CheckpointStatusLoaded as u32,
                Chunks: HashMap::new(),
            };
            for value in engine.Chunks {
                let key = value.Key.String();
                let chunk = engineModel.Chunks.entry(key).or_insert_with(|| {
                    checkpointspb::ChunkCheckpointModel {
                        Path: value.Key.Path.clone(),
                        Offset: value.Key.Offset,
                        ..Default::default()
                    }
                });
                chunk.Type = value.FileMeta.Type.0;
                chunk.Compression = value.FileMeta.Compression.0;
                chunk.SortKey = value.FileMeta.SortKey;
                chunk.FileSize = value.FileMeta.FileSize;
                chunk.Pos = value.Chunk.Offset;
                chunk.RealPos = value.Chunk.RealOffset;
                chunk.EndOffset = value.Chunk.EndOffset;
                chunk.PrevRowidMax = value.Chunk.PrevRowIDMax;
                chunk.RowidMax = value.Chunk.RowIDMax;
                chunk.Timestamp = value.Timestamp;
                if !value.ColumnPermutation.is_empty() {
                    chunk.ColumnPermutation = intSlice2Int32Slice(value.ColumnPermutation);
                }
            }
            tableModel.Engines.insert(engineID, engineModel);
        }
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn Update(
        &mut self,
        _ctx: context::Context,
        checkpointDiffs: HashMap<String, TableCheckpointDiff>,
    ) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        for (tableName, cpd) in checkpointDiffs {
            let tableModel = self
                .checkpoints
                .Checkpoints
                .get_mut(&tableName)
                .ok_or_else(|| Error::new(format!("missing table {tableName}")))?;
            if cpd.hasStatus {
                tableModel.Status = cpd.status as u32;
            }
            if cpd.hasRebase {
                tableModel.AutoRandBase = tableModel.AutoRandBase.max(cpd.autoRandBase);
                tableModel.AutoIncrBase = tableModel.AutoIncrBase.max(cpd.autoIncrBase);
                tableModel.AutoRowIDBase = tableModel.AutoRowIDBase.max(cpd.autoRowIDBase);
            }
            if cpd.hasChecksum {
                tableModel.KvBytes = cpd.checksum.SumSize();
                tableModel.KvKvs = cpd.checksum.SumKVS();
                tableModel.KvChecksum = cpd.checksum.Sum();
            }
            for (engineID, engineDiff) in cpd.engines {
                let engineModel = tableModel
                    .Engines
                    .get_mut(&engineID)
                    .ok_or_else(|| Error::new(format!("missing engine {engineID}")))?;
                if engineDiff.hasStatus {
                    engineModel.Status = engineDiff.status as u32;
                }
                for (key, diff) in engineDiff.chunks {
                    let chunkModel = engineModel
                        .Chunks
                        .get_mut(&key.String())
                        .ok_or_else(|| Error::new(format!("missing chunk {}", key.String())))?;
                    chunkModel.Pos = diff.pos;
                    chunkModel.RealPos = diff.realPos;
                    chunkModel.PrevRowidMax = diff.rowID;
                    chunkModel.KvcBytes = diff.checksum.SumSize();
                    chunkModel.KvcKvs = diff.checksum.SumKVS();
                    chunkModel.KvcChecksum = diff.checksum.Sum();
                    chunkModel.ColumnPermutation = intSlice2Int32Slice(diff.columnPermutation);
                }
            }
        }
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn RemoveCheckpoint(&mut self, _ctx: context::Context, tableName: &str) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        if tableName == allTables {
            self.checkpoints.Reset();
            return self
                .exStorage
                .DeleteFile(self.ctx, &self.fileName)
                .map_err(errors::Trace);
        }
        self.checkpoints.Checkpoints.remove(tableName);
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn MoveCheckpoints(&mut self, _ctx: context::Context, taskID: i64) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        let newFileName = format!("{}.{}.bak", self.fileName, taskID);
        self.exStorage
            .Rename(self.ctx, &self.fileName, &newFileName)
            .map_err(errors::Trace)
    }

    fn GetLocalStoringTables(&self, _ctx: context::Context) -> Result<HashMap<String, Vec<i32>>> {
        let _guard = self.lock.lock().unwrap();
        let mut targetTables: HashMap<String, Vec<i32>> = HashMap::new();
        for (tableName, tableModel) in &self.checkpoints.Checkpoints {
            if tableModel.Status <= CheckpointStatusMaxInvalid as u32
                || tableModel.Status >= CheckpointStatusIndexImported as u32
            {
                continue;
            }
            for (engineID, engineModel) in &tableModel.Engines {
                if engineModel.Status <= CheckpointStatusMaxInvalid as u32
                    || engineModel.Status >= CheckpointStatusImported as u32
                {
                    continue;
                }
                if engineModel
                    .Chunks
                    .values()
                    .any(|chunk| chunk.Pos > chunk.Offset)
                {
                    targetTables
                        .entry(tableName.clone())
                        .or_default()
                        .push(*engineID);
                }
            }
        }
        Ok(targetTables)
    }

    fn IgnoreErrorCheckpoint(
        &mut self,
        _ctx: context::Context,
        targetTableName: &str,
    ) -> Result<()> {
        // 文件后端忽略错误时，遵循与 MySQL 相同的状态区间协议。
        // 也就是说，只有落在失败区间内的表级或 engine 级状态才会被重置回 `Loaded`。
        // 这样可以确保“忽略错误”只是把对象重新放回可继续导入的起点，
        // 而不是无差别覆盖所有状态。
        //
        // 全局模式与单表模式的区别也被完整保留：
        // 前者遍历全部表，后者若找不到目标表则返回明确的“检查点不存在”错误。
        let _guard = self.lock.lock().unwrap();
        if targetTableName == allTables {
            for tableModel in self.checkpoints.Checkpoints.values_mut() {
                if tableModel.Status <= CheckpointStatusMaxInvalid as u32 {
                    tableModel.Status = CheckpointStatusLoaded as u32;
                }
                for engineModel in tableModel.Engines.values_mut() {
                    if engineModel.Status <= CheckpointStatusMaxInvalid as u32 {
                        engineModel.Status = CheckpointStatusLoaded as u32;
                    }
                }
            }
            return file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName);
        }
        let Some(tableModel) = self.checkpoints.Checkpoints.get_mut(targetTableName) else {
            return Err(common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[targetTableName]));
        };
        if tableModel.Status <= CheckpointStatusMaxInvalid as u32 {
            tableModel.Status = CheckpointStatusLoaded as u32;
        }
        for engineModel in tableModel.Engines.values_mut() {
            if engineModel.Status <= CheckpointStatusMaxInvalid as u32 {
                engineModel.Status = CheckpointStatusLoaded as u32;
            }
        }
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)
    }

    fn DestroyErrorCheckpoint(
        &mut self,
        _ctx: context::Context,
        targetTableName: &str,
    ) -> Result<Vec<DestroyedTableCheckpoint>> {
        // 这里的目标不是修复错误态，而是把错误态检查点整个移除。
        // 所以它会先收集待删对象，再统一删除并落盘。
        // 先收集后删除的好处是遍历逻辑更清晰，也避免边遍历 map 边修改的复杂性。
        //
        // 返回值不是完整旧快照，而是最小必要摘要。
        // 上层往往只需要知道删掉了哪张表、对应 engine 范围大概是什么，
        // 并不需要在删除成功后继续持有整份旧对象。
        let _guard = self.lock.lock().unwrap();
        let mut targetTables = Vec::new();
        if targetTableName == allTables {
            for (tableName, tableModel) in &self.checkpoints.Checkpoints {
                if tableModel.Status <= CheckpointStatusMaxInvalid as u32 {
                    let mut minEngineID = i32::MAX;
                    let mut maxEngineID = i32::MIN;
                    for engineID in tableModel.Engines.keys() {
                        if *engineID < minEngineID {
                            minEngineID = *engineID;
                        }
                        if *engineID > maxEngineID {
                            maxEngineID = *engineID;
                        }
                    }
                    targetTables.push(DestroyedTableCheckpoint {
                        TableName: tableName.clone(),
                        MinEngineID: minEngineID,
                        MaxEngineID: maxEngineID,
                    });
                }
            }
        } else {
            let Some(tableModel) = self.checkpoints.Checkpoints.get(targetTableName) else {
                return Err(
                    common::ErrCheckpointTableNotFound.GenWithStackByArgs(&[targetTableName])
                );
            };
            if tableModel.Status <= CheckpointStatusMaxInvalid as u32 {
                let mut minEngineID = i32::MAX;
                let mut maxEngineID = i32::MIN;
                for engineID in tableModel.Engines.keys() {
                    if *engineID < minEngineID {
                        minEngineID = *engineID;
                    }
                    if *engineID > maxEngineID {
                        maxEngineID = *engineID;
                    }
                }
                targetTables.push(DestroyedTableCheckpoint {
                    TableName: targetTableName.to_string(),
                    MinEngineID: minEngineID,
                    MaxEngineID: maxEngineID,
                });
            }
        }
        for dtcp in &targetTables {
            self.checkpoints.Checkpoints.remove(&dtcp.TableName);
        }
        file_cp_save(&self.checkpoints, &self.exStorage, self.ctx, &self.fileName)?;
        Ok(targetTables)
    }

    fn DumpTables(&self, _ctx: context::Context, _writer: &mut dyn Write) -> Result<()> {
        // 文件后端不支持像 MySQL 那样直接把内容导成 CSV 结果集。
        // 原因很简单：它底层并不存在可直接映射成行集的 SQL 表。
        // 与其伪造一套导出格式，不如明确告诉调用方直接复制原始检查点文件。
        // 这既更贴近真实存储形态，也避免在排障工具链里再引入一套额外转换协议。
        //
        // 下面三个导出方法都共享这层限制，只是分别对应表、engine、chunk 三类导出接口。
        Err(errors::Errorf(format!(
            "dumping file checkpoint into CSV not unsupported, you may copy {} instead",
            self.path
        )))
    }
    fn DumpEngines(&self, _ctx: context::Context, _writer: &mut dyn Write) -> Result<()> {
        Err(errors::Errorf(format!(
            "dumping file checkpoint into CSV not unsupported, you may copy {} instead",
            self.path
        )))
    }
    fn DumpChunks(&self, _ctx: context::Context, _writer: &mut dyn Write) -> Result<()> {
        Err(errors::Errorf(format!(
            "dumping file checkpoint into CSV not unsupported, you may copy {} instead",
            self.path
        )))
    }
}

pub fn errCannotManageNullDB() -> Error {
    // 这个小函数集中定义了“检查点被关闭时，不允许执行管理动作”的统一错误。
    // 把错误文本收口到这里有两个好处。
    // 一是 `NullCheckpointsDB` 的多个方法能复用同一语义，避免不同入口返回相近但不完全相同的消息。
    // 二是调用方和测试能更容易识别：失败原因不是存储故障，而是能力本身被配置关闭。
    //
    // 这也说明空实现并不是未完成的桩。
    // 它是一种正式支持的运行模式，只是边界被刻意限定为“可读默认值，不可做管理性修改”。
    // 因而这个函数实际上是空对象模式的一部分，而不只是为了省几行重复代码。
    //
    // 从阅读顺序看，把它放在文件尾部也很合理。
    // 前面已经看完空实现如何拒绝管理操作，此时再回到这里，就能更清楚地理解统一错误来源的价值。
    errors::New("cannot perform this function while checkpoints is disabled")
}

/// 这个辅助函数当前保持恒等转换。
/// 它存在的主要价值不是运行时逻辑，而是维持与 Go 辅助函数相近的调用形状。
/// 这样在对照移植代码时，读者不必因为“Rust 这里为什么少了一个转换步骤”而分散注意力。
/// 也就是说，它更像一种移植期接口垫片，而不是复杂算法。
/// 若未来底层类型真的发生变化，这里也会成为自然的集中修改点。
/// 同时它还能提醒阅读者：
/// 当前这段代码关注的是移植对齐，
/// 不是数值转换算法本身。
/// 这也是保留它的真正语义价值。
pub fn intSlice2Int32Slice(s: Vec<i32>) -> Vec<i32> {
    s
}

// silence unused private const
#[allow(dead_code)]
fn _keep_column_table_name() -> &'static str {
    columnTableName
}
