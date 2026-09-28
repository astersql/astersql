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

//! Wire-compatible protobuf models matching Go `checkpointspb` /
//! gogo-generated `file_checkpoints.pb.go`. Hand-rolled encode/decode.
//! 该模块保存 Lightning checkpoint proto 的 Rust 侧镜像类型。
//! 目标不是提供更符合 Rust 习惯的抽象，而是尽量复刻
//! Go `file_checkpoints.pb.go` 的线协议、默认值和错误语义。
//! 因为上游依赖 gogo/protobuf 生成结果，所以这里保留
//! `Reset`、`Descriptor`、`XXX_*` 等历史兼容接口。
//! map 字段在编码时会先排序，避免 Rust `HashMap` 的迭代顺序
//! 影响产出的二进制字节序，从而和 Go 侧稳定输出保持一致。
//! 未知字段不会被保存，只负责按 wire type 正确跳过，行为与
//! Go 生成代码的 `skipFileCheckpoints` 保持一致。

use std::collections::HashMap;
use std::fmt;

/// 对齐 Go 生成文件里的版本断言常量。
/// 这里只暴露数值，真正的兼容性检查由调用方在编译期保证。
pub const PROTO_GOGO_PACKAGE_IS_VERSION_3: i32 = 3;

/// 汇总 Go gogo/protobuf 生成代码会抛出的解析错误。
/// 文案尽量与 Go 版本一致，便于跨语言对照日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 长度字段解析成负值，说明输入被截断或被污染。
    InvalidLength,
    /// varint 超过 64 位可表示范围时返回该错误。
    IntOverflow,
    /// 读取过程中提前到达字节流末尾。
    UnexpectedEof,
    /// 读到 group 结束标记，但当前并不在 group 上下文中。
    UnexpectedEndOfGroup,
    /// 字段号非法，例如 0 或负值。
    IllegalTag { field: i32, wire: u64 },
    /// 实际 wire type 与 proto 声明不一致。
    WrongWireType { field: &'static str, wire_type: i32 },
    /// 普通消息中遇到 end-group，和 Go 版一样视为协议错误。
    WiretypeEndGroup { msg: &'static str },
    /// wire type 不在 protobuf 规范允许的取值范围内。
    IllegalWireType(i32),
    /// 兜底错误，用于表达 Rust 侧额外的缓冲区问题。
    Other(String),
}

/// 维持与 Go 文本错误尽量一致的输出，方便复用上游排障经验。
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidLength => {
                write!(f, "proto: negative length found during unmarshaling")
            }
            Error::IntOverflow => write!(f, "proto: integer overflow"),
            Error::UnexpectedEof => write!(f, "unexpected EOF"),
            Error::UnexpectedEndOfGroup => write!(f, "proto: unexpected end of group"),
            Error::IllegalTag { field, wire } => {
                write!(f, "proto: illegal tag {field} (wire type {wire})")
            }
            Error::WrongWireType { field, wire_type } => {
                write!(f, "proto: wrong wireType = {wire_type} for field {field}")
            }
            Error::WiretypeEndGroup { msg } => {
                write!(f, "proto: {msg}: wiretype end group for non-group")
            }
            Error::IllegalWireType(t) => write!(f, "proto: illegal wireType {t}"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

/// 本模块统一使用的结果类型别名。
pub type Result<T> = std::result::Result<T, Error>;

/// 兼容 Go 版导出的固定错误文案，便于调用方做字符串级比对。
pub static ERR_INVALID_LENGTH_FILE_CHECKPOINTS: &str =
    "proto: negative length found during unmarshaling";
/// 兼容 Go 版导出的整数溢出错误文案。
pub static ERR_INT_OVERFLOW_FILE_CHECKPOINTS: &str = "proto: integer overflow";
/// 兼容 Go 版导出的 EOF 错误文案。
pub static ERR_UNEXPECTED_EOF: &str = "unexpected EOF";

/// 顶层检查点快照。
/// 同时携带按表聚合的进度，以及任务级公共元数据。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckpointsModel {
    /// 以表名为键保存表级检查点，编码时会按键排序保证稳定输出。
    pub Checkpoints: HashMap<String, TableCheckpointModel>,
    /// 可选的任务级元数据；缺失时与 Go 的 `nil` 指针语义一致。
    pub TaskCheckpoint: Option<TaskCheckpointModel>,
}

/// 任务级检查点元信息。
/// 这些字段帮助恢复导入上下文，而不是描述单表进度。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskCheckpointModel {
    /// 导入任务 ID，用于把检查点和某次 Lightning 运行绑定。
    pub TaskId: i64,
    /// 原始数据源目录，恢复时可据此重新定位输入文件。
    pub SourceDir: String,
    /// 当前任务使用的导入后端名称，例如 tidb 或 local。
    pub Backend: String,
    /// Importer 服务地址；仅在需要远端写入时生效。
    pub ImporterAddr: String,
    /// TiDB 主机名，用于重建目标集群连接信息。
    pub TidbHost: String,
    /// TiDB 端口号；按 proto varint 存储。
    pub TidbPort: i32,
    /// PD 地址串，恢复调度或 local backend 时会读取。
    pub PdAddr: String,
    /// 排序后 KV 临时目录，恢复 local backend 任务时要复用。
    pub SortedKvDir: String,
    /// 写入检查点时的 Lightning 版本，便于排查兼容性问题。
    pub LightningVer: String,
}

/// 表级检查点。
/// 汇总表整体状态、各 engine 进度以及校验统计信息。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableCheckpointModel {
    /// 表结构与输入元数据的摘要，帮助判断检查点是否仍可复用。
    pub Hash: Vec<u8>,
    /// 当前节点的检查点状态枚举值，保持和 Go 版本同一数值空间。
    pub Status: u32,
    /// 以 engine ID 为键保存 engine 级进度，键在编码时使用 zigzag32。
    pub Engines: HashMap<i32, EngineCheckpointModel>,
    /// 目标表 ID，便于把逻辑表名映射到实际下游对象。
    pub TableID: i64,
    /// 已生成 KV 的总字节数。
    pub KvBytes: u64,
    /// 已生成 KV 的总条数。
    pub KvKvs: u64,
    /// 表级 KV 校验和，采用 fixed64 保持与 Go 完全一致。
    pub KvChecksum: u64,
    /// 序列化后的表结构信息，恢复时无需重新推导。
    pub TableInfo: Vec<u8>,
    /// 已使用过的 auto-random 基值，不含分片位。
    pub AutoRandBase: i64,
    /// 已使用过的自增 ID 最大值。
    pub AutoIncrBase: i64,
    /// 已使用过的隐式 row ID 最大值。
    pub AutoRowIDBase: i64,
}

/// engine 级检查点。
/// 一个表可能拆成多个 engine 并行导入，因此需要单独跟踪。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineCheckpointModel {
    /// engine 当前状态，与表级状态共享同一枚举定义。
    pub Status: u32,
    /// 以 `$path:$offset` 为键保存 chunk 级进度，恢复时可直接定位输入切片。
    pub Chunks: HashMap<String, ChunkCheckpointModel>,
}

/// 最细粒度的 chunk 检查点。
/// 记录输入文件偏移、行号水位及 KV 统计，支持断点续跑。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChunkCheckpointModel {
    /// 源文件路径。
    pub Path: String,
    /// chunk 起始读取偏移。
    pub Offset: i64,
    /// 输入列到目标列的映射顺序；使用 packed repeated int32 编码。
    pub ColumnPermutation: Vec<i32>,
    /// chunk 逻辑结束偏移。
    pub EndOffset: i64,
    /// 当前已消费到的逻辑位置。
    pub Pos: i64,
    /// 本 chunk 开始前最后一个已分配 row ID。
    pub PrevRowidMax: i64,
    /// 当前 chunk 处理后的 row ID 上界。
    pub RowidMax: i64,
    /// chunk 已生成 KV 的字节数。
    pub KvcBytes: u64,
    /// chunk 已生成 KV 的条数。
    pub KvcKvs: u64,
    /// chunk 级校验和，便于增量校验。
    pub KvcChecksum: u64,
    /// 写入该 chunk 时使用的时间戳，按 sfixed64 保存。
    pub Timestamp: i64,
    /// chunk 类型枚举值，例如 CSV 或 Parquet 派生切片。
    pub Type: i32,
    /// 源文件压缩格式枚举值。
    pub Compression: i32,
    /// 排序键，帮助 local backend 恢复排序中间态。
    pub SortKey: String,
    /// 源文件总大小，恢复时可检测输入是否变更。
    pub FileSize: i64,
    /// 实际物理读取位置，和逻辑 `Pos` 分开保存。
    pub RealPos: i64,
}

/// 复刻 `proto.InternalMessageInfo.Merge` 的 proto3 合并规则。
/// 标量只在源值非零时覆盖，repeated 追加，map 按键覆盖，嵌套消息递归合并。
trait ProtoMerge {
    fn merge_from(&mut self, src: &Self);
}

impl ProtoMerge for CheckpointsModel {
    fn merge_from(&mut self, src: &Self) {
        self.Checkpoints.extend(src.Checkpoints.clone());
        if let Some(src_task) = &src.TaskCheckpoint {
            if let Some(task) = &mut self.TaskCheckpoint {
                task.merge_from(src_task);
            } else {
                self.TaskCheckpoint = Some(src_task.clone());
            }
        }
    }
}

impl ProtoMerge for TaskCheckpointModel {
    fn merge_from(&mut self, src: &Self) {
        if src.TaskId != 0 {
            self.TaskId = src.TaskId;
        }
        if !src.SourceDir.is_empty() {
            self.SourceDir.clone_from(&src.SourceDir);
        }
        if !src.Backend.is_empty() {
            self.Backend.clone_from(&src.Backend);
        }
        if !src.ImporterAddr.is_empty() {
            self.ImporterAddr.clone_from(&src.ImporterAddr);
        }
        if !src.TidbHost.is_empty() {
            self.TidbHost.clone_from(&src.TidbHost);
        }
        if src.TidbPort != 0 {
            self.TidbPort = src.TidbPort;
        }
        if !src.PdAddr.is_empty() {
            self.PdAddr.clone_from(&src.PdAddr);
        }
        if !src.SortedKvDir.is_empty() {
            self.SortedKvDir.clone_from(&src.SortedKvDir);
        }
        if !src.LightningVer.is_empty() {
            self.LightningVer.clone_from(&src.LightningVer);
        }
    }
}

impl ProtoMerge for TableCheckpointModel {
    fn merge_from(&mut self, src: &Self) {
        if !src.Hash.is_empty() {
            self.Hash.clone_from(&src.Hash);
        }
        if src.Status != 0 {
            self.Status = src.Status;
        }
        self.Engines.extend(src.Engines.clone());
        if src.TableID != 0 {
            self.TableID = src.TableID;
        }
        if src.KvBytes != 0 {
            self.KvBytes = src.KvBytes;
        }
        if src.KvKvs != 0 {
            self.KvKvs = src.KvKvs;
        }
        if src.KvChecksum != 0 {
            self.KvChecksum = src.KvChecksum;
        }
        if !src.TableInfo.is_empty() {
            self.TableInfo.clone_from(&src.TableInfo);
        }
        if src.AutoRandBase != 0 {
            self.AutoRandBase = src.AutoRandBase;
        }
        if src.AutoIncrBase != 0 {
            self.AutoIncrBase = src.AutoIncrBase;
        }
        if src.AutoRowIDBase != 0 {
            self.AutoRowIDBase = src.AutoRowIDBase;
        }
    }
}

impl ProtoMerge for EngineCheckpointModel {
    fn merge_from(&mut self, src: &Self) {
        if src.Status != 0 {
            self.Status = src.Status;
        }
        self.Chunks.extend(src.Chunks.clone());
    }
}

impl ProtoMerge for ChunkCheckpointModel {
    fn merge_from(&mut self, src: &Self) {
        if !src.Path.is_empty() {
            self.Path.clone_from(&src.Path);
        }
        if src.Offset != 0 {
            self.Offset = src.Offset;
        }
        self.ColumnPermutation
            .extend_from_slice(&src.ColumnPermutation);
        if src.EndOffset != 0 {
            self.EndOffset = src.EndOffset;
        }
        if src.Pos != 0 {
            self.Pos = src.Pos;
        }
        if src.PrevRowidMax != 0 {
            self.PrevRowidMax = src.PrevRowidMax;
        }
        if src.RowidMax != 0 {
            self.RowidMax = src.RowidMax;
        }
        if src.KvcBytes != 0 {
            self.KvcBytes = src.KvcBytes;
        }
        if src.KvcKvs != 0 {
            self.KvcKvs = src.KvcKvs;
        }
        if src.KvcChecksum != 0 {
            self.KvcChecksum = src.KvcChecksum;
        }
        if src.Timestamp != 0 {
            self.Timestamp = src.Timestamp;
        }
        if src.Type != 0 {
            self.Type = src.Type;
        }
        if src.Compression != 0 {
            self.Compression = src.Compression;
        }
        if !src.SortKey.is_empty() {
            self.SortKey.clone_from(&src.SortKey);
        }
        if src.FileSize != 0 {
            self.FileSize = src.FileSize;
        }
        if src.RealPos != 0 {
            self.RealPos = src.RealPos;
        }
    }
}

/// 直接保留 Go 生成文件导出的压缩 descriptor 字节串。
/// 兼容接口只需要把它原样返回，不在 Rust 侧做额外解析。
pub static FILE_DESCRIPTOR_E56085BB94E0B973: [u8; 939] = [
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0x8c, 0x55, 0xcf, 0x8e, 0xe3, 0xc4,
    0x13, 0x1e, 0x8f, 0xf3, 0xb7, 0x9c, 0xcc, 0x64, 0xfa, 0x37, 0xb3, 0xeb, 0xdf, 0x00, 0x21, 0x64,
    0xf7, 0x10, 0x69, 0x21, 0x11, 0xb3, 0x1c, 0xd0, 0x0a, 0x10, 0xcc, 0xcc, 0x4a, 0x0c, 0xa3, 0x15,
    0x23, 0xb3, 0x70, 0xe0, 0x62, 0x39, 0x76, 0x27, 0xb1, 0x3a, 0x71, 0x5b, 0xdd, 0x6d, 0xef, 0x66,
    0x9f, 0x82, 0xc7, 0xe0, 0x25, 0xb8, 0xef, 0x71, 0x0f, 0x1c, 0x38, 0xc2, 0x8c, 0xb8, 0xf2, 0x0c,
    0xa8, 0xab, 0x9d, 0xc4, 0x59, 0x05, 0xc4, 0xad, 0xeb, 0xab, 0xaf, 0x3e, 0x57, 0x57, 0x7f, 0xdd,
    0x86, 0x2f, 0xe6, 0xf1, 0x74, 0xa6, 0x92, 0x38, 0x99, 0x8e, 0x52, 0x36, 0x1d, 0x85, 0x33, 0x1a,
    0xb2, 0x94, 0xc7, 0x89, 0x92, 0xe5, 0x75, 0x3a, 0x1e, 0x4d, 0xe2, 0x39, 0xf5, 0x4b, 0xd0, 0x30,
    0x15, 0x5c, 0xf1, 0xd3, 0x4f, 0xa6, 0xb1, 0x9a, 0x65, 0xe3, 0x61, 0xc8, 0x17, 0xa3, 0x29, 0x9f,
    0xf2, 0x11, 0xc2, 0xe3, 0x6c, 0xf2, 0x65, 0xfe, 0xf1, 0xf0, 0xf1, 0xf0, 0x0c, 0x41, 0xc4, 0x70,
    0x65, 0xaa, 0xfa, 0x7f, 0x59, 0xd0, 0xb9, 0xd8, 0x68, 0x3d, 0xe3, 0x11, 0x9d, 0x93, 0x4b, 0x70,
    0x4a, 0xfa, 0xae, 0xd5, 0xb3, 0x07, 0xce, 0x59, 0x7f, 0xf8, 0x36, 0xaf, 0x0c, 0x3c, 0x4d, 0x94,
    0x58, 0x7a, 0xe5, 0x32, 0xf2, 0x39, 0x1c, 0xaa, 0x40, 0xb2, 0x52, 0xab, 0xee, 0x7e, 0xcf, 0x1a,
    0x38, 0x67, 0xc7, 0xc3, 0xe7, 0x81, 0x64, 0x9b, 0x62, 0x14, 0xf3, 0x0e, 0xd4, 0x16, 0x78, 0xfa,
    0xfd, 0x56, 0x63, 0xa8, 0x4f, 0x3a, 0x60, 0x33, 0xba, 0x74, 0xad, 0x9e, 0x35, 0x68, 0x7a, 0x7a,
    0x49, 0x1e, 0x41, 0x35, 0x0f, 0xe6, 0x19, 0x2d, 0xa4, 0x4f, 0x86, 0xcf, 0x83, 0xf1, 0x9c, 0xbe,
    0xad, 0x6d, 0x38, 0x4f, 0xf6, 0x3f, 0xb5, 0xfa, 0x3f, 0xef, 0xc3, 0xff, 0x76, 0x7c, 0x9e, 0xdc,
    0x87, 0x3a, 0x76, 0x1b, 0x47, 0x28, 0x6f, 0x7b, 0x35, 0x1d, 0x5e, 0x45, 0xe4, 0x3d, 0x00, 0xc9,
    0x33, 0x11, 0x52, 0x3f, 0x8a, 0x05, 0x7e, 0xa6, 0xe9, 0x35, 0x0d, 0x72, 0x19, 0x0b, 0xe2, 0x42,
    0x7d, 0x1c, 0x84, 0x8c, 0x26, 0x91, 0x6b, 0x63, 0x6e, 0x15, 0x92, 0x07, 0xd0, 0x8e, 0x17, 0x29,
    0x17, 0x8a, 0x0a, 0x3f, 0x88, 0x22, 0xe1, 0x56, 0x30, 0xdf, 0x5a, 0x81, 0x5f, 0x45, 0x91, 0x20,
    0xef, 0x40, 0x53, 0xc5, 0xd1, 0xd8, 0x9f, 0x71, 0xa9, 0xdc, 0x2a, 0x12, 0x1a, 0x1a, 0xf8, 0x9a,
    0x4b, 0xb5, 0x4e, 0x6a, 0xbe, 0x5b, 0xeb, 0x59, 0x83, 0xaa, 0x49, 0xde, 0x70, 0xa1, 0x74, 0xc3,
    0x69, 0x64, 0x84, 0xeb, 0x58, 0x57, 0x4b, 0x23, 0x94, 0xec, 0x43, 0x5b, 0xea, 0x0f, 0x44, 0x3e,
    0xcb, 0xb1, 0xe7, 0x06, 0xa6, 0x1d, 0x03, 0x5e, 0xe7, 0xba, 0xeb, 0x07, 0xd0, 0x5e, 0xdb, 0xcd,
    0xcf, 0xa9, 0x70, 0x9b, 0xa6, 0xb7, 0x35, 0xf8, 0x03, 0x15, 0xfd, 0x5f, 0x6d, 0x38, 0xde, 0x35,
    0x4e, 0x42, 0xa0, 0x32, 0x0b, 0xe4, 0x0c, 0x07, 0xd5, 0xf2, 0x70, 0x4d, 0xee, 0x41, 0x4d, 0xaa,
    0x40, 0x65, 0x12, 0xc7, 0xd0, 0xf6, 0x8a, 0x88, 0x7c, 0x06, 0x75, 0x9a, 0x4c, 0xe3, 0x84, 0x4a,
    0xb7, 0x51, 0xf8, 0x68, 0x97, 0xe6, 0xf0, 0xa9, 0x21, 0x19, 0x1f, 0xad, 0x4a, 0xf4, 0x74, 0x95,
    0x66, 0x5f, 0x5d, 0x62, 0x87, 0xb6, 0xb7, 0x0a, 0xc9, 0xff, 0xa1, 0xc1, 0x72, 0x7f, 0xbc, 0x54,
    0x54, 0xba, 0xd0, 0xb3, 0x06, 0x15, 0xaf, 0xce, 0xf2, 0x73, 0x1d, 0x92, 0x13, 0xa8, 0xb1, 0xdc,
    0x67, 0xb9, 0x74, 0x1d, 0x4c, 0x54, 0x59, 0x7e, 0x9d, 0x4b, 0xf2, 0x3e, 0x38, 0x2c, 0x37, 0x6e,
    0x94, 0xd9, 0xc2, 0x6d, 0xf5, 0xac, 0x41, 0xcd, 0x03, 0x96, 0x5f, 0x14, 0x88, 0x3e, 0x69, 0x54,
    0xf7, 0xe3, 0x64, 0xc2, 0xdd, 0x36, 0x6e, 0xae, 0x69, 0xbe, 0x97, 0x4c, 0x38, 0xe9, 0x43, 0x2b,
    0xc8, 0x14, 0xf7, 0x82, 0x24, 0x3a, 0x0f, 0x24, 0x75, 0x0f, 0xb0, 0xa1, 0x2d, 0x6c, 0xc5, 0xb9,
    0x4a, 0x42, 0x81, 0x9c, 0xc3, 0x0d, 0x67, 0x85, 0x91, 0x87, 0xd0, 0xc6, 0x1a, 0xfe, 0xe2, 0xea,
    0x12, 0x49, 0x1d, 0x24, 0x6d, 0x83, 0xa7, 0x1e, 0xb4, 0xca, 0x23, 0x29, 0x5b, 0xff, 0xc8, 0x58,
    0xff, 0xc3, 0x6d, 0xeb, 0xdf, 0x2b, 0x46, 0xf8, 0xcf, 0xde, 0xff, 0xa6, 0xd2, 0xa8, 0x74, 0xaa,
    0xfd, 0x5f, 0x2c, 0x38, 0xd9, 0x49, 0x2d, 0x9d, 0xa1, 0xb5, 0x75, 0x86, 0x4f, 0xa0, 0x16, 0xce,
    0xb2, 0x84, 0x49, 0x77, 0xbf, 0x38, 0xc2, 0x9d, 0xf5, 0xc3, 0x0b, 0x24, 0x99, 0x23, 0x2c, 0x2a,
    0x4e, 0x6f, 0xc0, 0x29, 0xc1, 0xff, 0xe5, 0x06, 0x23, 0xfd, 0x5f, 0x6e, 0xf0, 0x9f, 0x36, 0x1c,
    0xef, 0xe2, 0x68, 0x5b, 0xa6, 0x81, 0x9a, 0x15, 0xe2, 0xb8, 0xd6, 0x5b, 0xe2, 0x93, 0x89, 0xa4,
    0xe6, 0xed, 0xb1, 0xbd, 0x22, 0x22, 0x1f, 0x01, 0x09, 0xf9, 0x3c, 0x5b, 0x24, 0x7e, 0x4a, 0xc5,
    0x22, 0x53, 0x81, 0x8a, 0x79, 0xe2, 0xb6, 0x7a, 0xf6, 0xa0, 0xea, 0x1d, 0x99, 0xcc, 0xcd, 0x26,
    0xa1, 0xad, 0x41, 0x93, 0xc8, 0x2f, 0xa4, 0xaa, 0x28, 0xd5, 0xa4, 0x49, 0xf4, 0xad, 0x51, 0xeb,
    0x80, 0x9d, 0x72, 0x89, 0x57, 0xd4, 0xf6, 0xf4, 0x92, 0x3c, 0x84, 0x83, 0x54, 0xd0, 0xdc, 0x17,
    0xfc, 0x45, 0x1c, 0xf9, 0x8b, 0xe0, 0x25, 0x5e, 0x52, 0xdb, 0x6b, 0x69, 0xd4, 0xd3, 0xe0, 0xb3,
    0xe0, 0xa5, 0xbe, 0xe0, 0x1b, 0x42, 0x03, 0x09, 0x0d, 0x51, 0x4a, 0xb2, 0x3c, 0x2c, 0x2c, 0xde,
    0x44, 0x27, 0x37, 0x58, 0x1e, 0x1a, 0x8f, 0xdf, 0x87, 0xba, 0x4e, 0x6a, 0x93, 0x1b, 0xf7, 0xd7,
    0x58, 0x1e, 0x6a, 0x97, 0x7f, 0x00, 0x2d, 0x9d, 0x58, 0xdb, 0xdc, 0x41, 0x9b, 0x3b, 0x2c, 0x0f,
    0xd7, 0x3e, 0x7f, 0x57, 0x3f, 0x2b, 0x0b, 0x2a, 0x55, 0xb0, 0x48, 0xd1, 0xe6, 0x1d, 0x6f, 0x03,
    0xe8, 0x29, 0xaa, 0x65, 0x6a, 0xec, 0x5d, 0xf5, 0x70, 0x4d, 0x7a, 0xe0, 0x84, 0x7c, 0x91, 0x0a,
    0x2a, 0xa5, 0x1e, 0xd3, 0x21, 0xa6, 0xca, 0x90, 0xbe, 0x8e, 0xfa, 0x7d, 0xf1, 0xf5, 0xe1, 0x76,
    0xcc, 0x3b, 0xa8, 0xe3, 0x6b, 0xba, 0xd4, 0xfb, 0xc0, 0x5f, 0x96, 0x8c, 0x5f, 0x51, 0xf7, 0xc8,
    0x6c, 0x52, 0x03, 0xdf, 0xc5, 0xaf, 0xa8, 0xae, 0x13, 0x34, 0x98, 0xfb, 0x7a, 0x7c, 0xc4, 0xdc,
    0x70, 0x1d, 0xdf, 0x70, 0x79, 0xfe, 0xe8, 0xf5, 0x1f, 0xdd, 0xbd, 0xd7, 0xb7, 0x5d, 0xeb, 0xcd,
    0x6d, 0xd7, 0xfa, 0xfd, 0xb6, 0x6b, 0xfd, 0x74, 0xd7, 0xdd, 0x7b, 0x73, 0xd7, 0xdd, 0xfb, 0xed,
    0xae, 0xbb, 0xf7, 0x63, 0x7b, 0xeb, 0xa7, 0x38, 0xae, 0xe1, 0xef, 0xec, 0xf1, 0xdf, 0x01, 0x00,
    0x00, 0xff, 0xff, 0x77, 0x31, 0x96, 0xc6, 0x46, 0x07, 0x00, 0x00,
];

/// 为每个消息类型补齐 Go 时代遗留的 proto 兼容方法。
/// 这些方法名称保持首字母大写，是为了贴近上游调用约定。
macro_rules! impl_proto_compat {
    ($ty:ty, $idx:expr) => {
        impl $ty {
            /// 重置成 proto3 零值状态，和 Go `Reset()` 语义一致。
            pub fn Reset(&mut self) {
                *self = Default::default();
            }

            /// 返回调试文本；Rust 这里直接复用 `Debug` 输出。
            pub fn String(&self) -> String {
                format!("{:?}", self)
            }

            /// 空标记方法，仅用于兼容历史 proto 调用约定。
            pub fn ProtoMessage(&self) {}

            /// 返回压缩 descriptor 和消息索引，接口形状对齐 Go。
            pub fn Descriptor(&self) -> (&'static [u8], Vec<i32>) {
                (&FILE_DESCRIPTOR_E56085BB94E0B973, vec![$idx])
            }

            /// 兼容旧接口，内部直接转调新版 `Unmarshal`。
            pub fn XXX_Unmarshal(&mut self, data: &[u8]) -> Result<()> {
                self.Unmarshal(data)
            }

            /// 兼容旧接口；`deterministic` 形参被保留但当前不改变路径。
            pub fn XXX_Marshal(&self, mut data: Vec<u8>, deterministic: bool) -> Result<Vec<u8>> {
                let _ = deterministic;
                let size = self.Size();
                if data.capacity() < size {
                    data = vec![0u8; size];
                } else {
                    data.resize(size, 0);
                }
                let n = self.MarshalToSizedBuffer(&mut data)?;
                data.truncate(n);
                Ok(data)
            }

            /// 按 proto3 规则合并已设置字段、map、repeated 与嵌套消息。
            pub fn XXX_Merge(&mut self, src: &Self) {
                self.merge_from(src);
            }

            /// 兼容旧接口，返回当前消息的编码大小。
            pub fn XXX_Size(&self) -> usize {
                self.Size()
            }

            /// Rust 版本不缓存未知字段，因此这里保持空操作。
            pub fn XXX_DiscardUnknown(&mut self) {}
        }
    };
}

impl_proto_compat!(CheckpointsModel, 0);
impl_proto_compat!(TaskCheckpointModel, 1);
impl_proto_compat!(TableCheckpointModel, 2);
impl_proto_compat!(EngineCheckpointModel, 3);
impl_proto_compat!(ChunkCheckpointModel, 4);

/// Go `init` registers types with gogo; Rust has no global registry.
/// Go 版本会在 `init()` 中向 gogo 全局注册类型。
/// Rust 没有对应全局注册表，因此保留空实现满足接口形状。
pub fn init() {}

/// 从缓冲区尾部向前写入 varint。
/// 这种写法贴近 Go `MarshalToSizedBuffer` 的倒序填充策略。
pub fn encodeVarintFileCheckpoints(data: &mut [u8], offset: usize, mut v: u64) -> usize {
    let mut offset = offset - sovFileCheckpoints(v);
    let base = offset;
    let mut i = 0;
    while v >= 1 << 7 {
        data[base + i] = (v as u8 & 0x7f) | 0x80;
        v >>= 7;
        i += 1;
    }
    data[base + i] = v as u8;
    offset
}

/// 计算普通 varint 编码后的字节数。
/// `x | 1` 用来保证零值也至少占用一个字节。
pub fn sovFileCheckpoints(x: u64) -> usize {
    let bits = 64 - (x | 1).leading_zeros();
    ((bits + 6) / 7) as usize
}

/// 计算 zigzag 语义下的 varint 长度。
/// 目前主要服务于和 Go 生成器保持一致的辅助接口。
pub fn sozFileCheckpoints(x: u64) -> usize {
    sovFileCheckpoints((x << 1) ^ (((x as i64) >> 63) as u64))
}

/// 把有符号 32 位整数转换成 protobuf zigzag 编码。
fn encode_zigzag32(v: i32) -> u32 {
    ((v as u32) << 1) ^ ((v >> 31) as u32)
}

/// 把读取到的 zigzag32 结果恢复成带符号整数。
fn decode_zigzag32(n: i32) -> i32 {
    // 这里保留 Go 位运算的语义细节，避免 Rust 上的符号位处理与上游出现偏差。
    // Match Go: int32((uint32(n) >> 1) ^ uint32(((n&1)<<31)>>31))
    // The inner shifts are on signed int32 so >> is arithmetic.
    let sign_mask = ((n & 1) << 31) >> 31;
    (((n as u32) >> 1) ^ (sign_mask as u32)) as i32
}

/// 追加一个普通 varint 字段内容，不写入 field tag。
fn append_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// 写入字段号和 wire type 组成的 protobuf tag。
fn append_tag(buf: &mut Vec<u8>, field: u32, wire: u32) {
    append_varint(buf, ((field << 3) | wire) as u64);
}

/// 追加 bytes 字段，并在空值时复现 proto3 的省略策略。
fn append_bytes_field(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    append_tag(buf, field, 2);
    append_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

/// 追加字符串字段；字符串在线路层本质上仍是 bytes。
fn append_string_field(buf: &mut Vec<u8>, field: u32, s: &str) {
    append_bytes_field(buf, field, s.as_bytes());
}

/// 追加 varint 字段，并对零值执行 proto3 默认省略。
fn append_varint_field(buf: &mut Vec<u8>, field: u32, v: u64) {
    if v == 0 {
        return;
    }
    append_tag(buf, field, 0);
    append_varint(buf, v);
}

/// 追加 fixed64 字段，保持 8 字节小端布局。
fn append_fixed64_field(buf: &mut Vec<u8>, field: u32, v: u64) {
    if v == 0 {
        return;
    }
    append_tag(buf, field, 1);
    buf.extend_from_slice(&v.to_le_bytes());
}

/// 追加 sfixed64 字段；有符号值按同一字节布局直接写出。
fn append_sfixed64_field(buf: &mut Vec<u8>, field: u32, v: i64) {
    if v == 0 {
        return;
    }
    append_tag(buf, field, 1);
    buf.extend_from_slice(&(v as u64).to_le_bytes());
}

/// 追加长度定界的嵌套消息。
fn append_message_field(buf: &mut Vec<u8>, field: u32, msg: &[u8]) {
    append_tag(buf, field, 2);
    append_varint(buf, msg.len() as u64);
    buf.extend_from_slice(msg);
}

/// 追加 packed repeated int32 字段。
/// 这里故意按 Go 的符号扩展规则把 `i32` 当作 `u64` 写出。
fn append_packed_int32(buf: &mut Vec<u8>, field: u32, vals: &[i32]) {
    if vals.is_empty() {
        return;
    }
    let mut packed = Vec::with_capacity(vals.len() * 10);
    for &num1 in vals {
        // 负数必须按 Go 的有符号扩展方式写入，否则与上游字节流不兼容。
        // Go: num := uint64(num1) with int32 — sign-extends to 64-bit.
        append_varint(&mut packed, num1 as u64);
    }
    append_tag(buf, field, 2);
    append_varint(buf, packed.len() as u64);
    buf.extend_from_slice(&packed);
}

/// 计算 bytes 字段占用大小，空值返回 0 以匹配省略规则。
fn size_bytes_field(data: &[u8]) -> usize {
    if data.is_empty() {
        0
    } else {
        1 + sovFileCheckpoints(data.len() as u64) + data.len()
    }
}

/// 计算字符串字段大小。
fn size_string_field(s: &str) -> usize {
    size_bytes_field(s.as_bytes())
}

/// 计算单字节 tag 的 varint 字段大小。
fn size_varint_field(v: u64) -> usize {
    if v == 0 { 0 } else { 1 + sovFileCheckpoints(v) }
}

/// 计算 fixed64 字段大小；非零时固定为 1 个 tag 加 8 个字节。
fn size_fixed64_field(v: u64) -> usize {
    if v == 0 { 0 } else { 9 }
}

/// 计算 sfixed64 字段大小。
fn size_sfixed64_field(v: i64) -> usize {
    if v == 0 { 0 } else { 9 }
}

/// 计算嵌套消息字段大小。
fn size_message_field(l: usize) -> usize {
    1 + sovFileCheckpoints(l as u64) + l
}

/// 计算双字节 tag 的 varint 字段大小。
fn size_varint_field_tag2(v: u64) -> usize {
    if v == 0 { 0 } else { 2 + sovFileCheckpoints(v) }
}

/// 计算双字节 tag 的字符串字段大小。
fn size_string_field_tag2(s: &str) -> usize {
    if s.is_empty() {
        0
    } else {
        2 + sovFileCheckpoints(s.len() as u64) + s.len()
    }
}

/// 与 Go 生成器的倒序编码入口一致，把结果放在整个缓冲区的尾部。
fn copy_marshal_sized(encoded: &[u8], data: &mut [u8]) -> Result<usize> {
    if encoded.len() > data.len() {
        return Err(Error::Other("buffer too small".into()));
    }
    let n = encoded.len();
    let start = data.len() - n;
    data[start..].copy_from_slice(encoded);
    Ok(n)
}

/// 轻量级顺序读取器。
/// 只追踪当前位置，不做未知字段缓存，契合 Go 版实现风格。
struct Reader<'a> {
    data: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    /// 从整段输入字节流创建读取器。
    fn new(data: &'a [u8]) -> Self {
        Self { data, i: 0 }
    }

    /// 返回剩余未消费字节数，避免手写减法溢出。
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.i)
    }

    /// 读取单字节；EOF 统一映射为模块级错误类型。
    fn read_byte(&mut self) -> Result<u8> {
        if self.i >= self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        let b = self.data[self.i];
        self.i += 1;
        Ok(b)
    }

    /// 读取 protobuf varint，并对超过 64 位的输入报错。
    fn read_varint(&mut self) -> Result<u64> {
        let mut x = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.read_byte()?;
            if shift == 63 && b > 1 {
                return Err(Error::IntOverflow);
            }
            x |= ((b & 0x7f) as u64) << shift;
            if b < 0x80 {
                return Ok(x);
            }
        }
        Err(Error::IntOverflow)
    }

    /// 读取 fixed64 小端整数。
    fn read_fixed64(&mut self) -> Result<u64> {
        if self.remaining() < 8 {
            return Err(Error::UnexpectedEof);
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&self.data[self.i..self.i + 8]);
        self.i += 8;
        Ok(u64::from_le_bytes(buf))
    }

    /// 读取给定长度的切片，并推进游标。
    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::UnexpectedEof);
        }
        let s = &self.data[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
}

/// 跳过一个未知字段或未知 group。
/// 只返回跳过的字节数，不尝试保留原始内容。
pub fn skipFileCheckpoints(data: &[u8]) -> Result<usize> {
    let mut r = Reader::new(data);
    let mut depth = 0i32;
    loop {
        if r.i >= data.len() {
            return Err(Error::UnexpectedEof);
        }
        let wire = r.read_varint()?;
        let wire_type = (wire & 0x7) as i32;
        match wire_type {
            0 => {
                r.read_varint()?;
            }
            1 => {
                if r.remaining() < 8 {
                    return Err(Error::UnexpectedEof);
                }
                r.i += 8;
            }
            2 => {
                let length = r.read_varint()? as i64;
                if length < 0 {
                    return Err(Error::InvalidLength);
                }
                let length = length as usize;
                if length > r.remaining() {
                    return Err(Error::UnexpectedEof);
                }
                r.i += length;
            }
            3 => depth += 1,
            4 => {
                if depth == 0 {
                    return Err(Error::UnexpectedEndOfGroup);
                }
                depth -= 1;
            }
            5 => {
                if r.remaining() < 4 {
                    return Err(Error::UnexpectedEof);
                }
                r.i += 4;
            }
            _ => return Err(Error::IllegalWireType(wire_type)),
        }
        if depth == 0 {
            return Ok(r.i);
        }
    }
}

/// 读取长度定界字符串，并校验 wire type。
fn read_string(r: &mut Reader<'_>, wt: i32, field: &'static str) -> Result<String> {
    if wt != 2 {
        return Err(Error::WrongWireType {
            field,
            wire_type: wt,
        });
    }
    let n = r.read_varint()? as i64;
    if n < 0 {
        return Err(Error::InvalidLength);
    }
    Ok(String::from_utf8_lossy(r.read_bytes(n as usize)?).into_owned())
}

/// 读取 bytes 字段，返回独立 `Vec<u8>` 以便后续持有。
fn read_bytes_field(r: &mut Reader<'_>, wt: i32, field: &'static str) -> Result<Vec<u8>> {
    if wt != 2 {
        return Err(Error::WrongWireType {
            field,
            wire_type: wt,
        });
    }
    let n = r.read_varint()? as i64;
    if n < 0 {
        return Err(Error::InvalidLength);
    }
    Ok(r.read_bytes(n as usize)?.to_vec())
}

/// 读取下一个字段头，并在字段号或 wire type 非法时立即报错。
fn begin_field<'a>(
    r: &mut Reader<'a>,
    msg: &'static str,
) -> Result<Option<(usize, i32, i32, u64)>> {
    if r.i >= r.data.len() {
        return Ok(None);
    }
    let pre = r.i;
    let wire = r.read_varint()?;
    let field = (wire >> 3) as i32;
    let wt = (wire & 0x7) as i32;
    if wt == 4 {
        return Err(Error::WiretypeEndGroup { msg });
    }
    if field <= 0 {
        return Err(Error::IllegalTag { field, wire });
    }
    Ok(Some((pre, field, wt, wire)))
}

/// 回退到字段起点后复用 `skipFileCheckpoints` 跳过未知字段。
fn skip_unknown(r: &mut Reader<'_>, pre: usize) -> Result<()> {
    r.i = pre;
    let skippy = skipFileCheckpoints(&r.data[r.i..])?;
    if r.i + skippy > r.data.len() {
        return Err(Error::UnexpectedEof);
    }
    r.i += skippy;
    Ok(())
}

/// 读取长度定界字段的原始切片，供嵌套消息继续解析。
fn read_len_delim<'a>(r: &mut Reader<'a>, field: &'static str, wt: i32) -> Result<&'a [u8]> {
    if wt != 2 {
        return Err(Error::WrongWireType {
            field,
            wire_type: wt,
        });
    }
    let msglen = r.read_varint()? as i64;
    if msglen < 0 {
        return Err(Error::InvalidLength);
    }
    r.read_bytes(msglen as usize)
}

/// `CheckpointsModel` 的编码与解码实现。
/// 重点是复刻 Go map entry 的展开方式和字段顺序。
impl CheckpointsModel {
    /// 分配恰好足够的缓冲区并返回完整编码结果。
    /// 先走 `Size()` 再编码，行为与 Go `Marshal()` 一致。
    /// 这样可以避免多次扩容带来的额外拷贝。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.Size());
        self.encode_to(&mut buf)?;
        Ok(buf)
    }

    /// 向外部提供的缓冲区写入编码结果。
    /// 该方法保留 Go 接口形状，便于平移上游调用代码。
    /// 真正的长度检查仍由内部复制逻辑统一处理。
    pub fn MarshalTo(&self, data: &mut [u8]) -> Result<usize> {
        let size = self.Size();
        if data.len() < size {
            return Err(Error::Other("buffer too small".into()));
        }
        self.MarshalToSizedBuffer(&mut data[..size])
    }

    /// 兼容 Go `MarshalToSizedBuffer` 名称。
    /// Rust 实现内部先完成独立编码，再复制到目标缓冲区。
    /// 这样可以在保持接口兼容的同时简化倒序写入实现。
    pub fn MarshalToSizedBuffer(&self, data: &mut [u8]) -> Result<usize> {
        copy_marshal_sized(&self.Marshal()?, data)
    }

    /// 直接把当前消息追加到目标缓冲区。
    /// map 或嵌套消息字段会先局部编码，再整体附加。
    /// 出错点主要来自子消息编码失败。
    fn encode_to(&self, buf: &mut Vec<u8>) -> Result<()> {
        let mut keys: Vec<&String> = self.Checkpoints.keys().collect();
        // 显式排序以消除 `HashMap` 的随机迭代顺序，保证跨语言输出稳定。
        keys.sort();
        for k in keys {
            let v = &self.Checkpoints[k];
            // map 会在线路层展开成独立的 entry 子消息，再作为长度定界字段写出。
            let mut entry = Vec::new();
            append_string_field(&mut entry, 1, k);
            let vb = v.Marshal()?;
            append_message_field(&mut entry, 2, &vb);
            append_message_field(buf, 1, &entry);
        }
        if let Some(task) = &self.TaskCheckpoint {
            let tb = task.Marshal()?;
            append_message_field(buf, 2, &tb);
        }
        Ok(())
    }

    /// 按 Go 生成器的计数方式预估编码后的精确长度。
    /// 零值字段在这里直接被忽略，确保与真实编码结果一致。
    /// map entry 的长度也会被展开后逐项累计。
    pub fn Size(&self) -> usize {
        let mut n = 0;
        for (k, v) in &self.Checkpoints {
            let vl = v.Size();
            let l = 1
                + k.len()
                + sovFileCheckpoints(k.len() as u64)
                + 1
                + vl
                + sovFileCheckpoints(vl as u64);
            n += l + 1 + sovFileCheckpoints(l as u64);
        }
        if let Some(task) = &self.TaskCheckpoint {
            n += size_message_field(task.Size());
        }
        n
    }

    /// 解析 protobuf 字节流，并把结果合并进当前对象。
    /// 与 Go 一样，重复 map/message 字段会覆盖或增量更新现有值。
    /// 未识别字段会按 wire type 跳过，而不是作为未知字段保存。
    pub fn Unmarshal(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        while let Some((pre, field, wt, _)) = begin_field(&mut r, "CheckpointsModel")? {
            match field {
                1 => {
                    let slice = read_len_delim(&mut r, "Checkpoints", wt)?;
                    // 进入 map entry 子消息后，继续按 key/value 两个字段解析。
                    let mut er = Reader::new(slice);
                    let mut mapkey = String::new();
                    let mut mapvalue = TableCheckpointModel::default();
                    while er.i < slice.len() {
                        let entry_pre = er.i;
                        let w = er.read_varint()?;
                        let f = (w >> 3) as i32;
                        let ewt = (w & 0x7) as i32;
                        if f == 1 {
                            mapkey = read_string(&mut er, ewt, "key")?;
                        } else if f == 2 {
                            let vb = read_len_delim(&mut er, "value", ewt)?;
                            // Go generated code allocates a fresh map value for every
                            // occurrence, so a repeated value field keeps only the last message.
                            mapvalue = TableCheckpointModel::default();
                            mapvalue.Unmarshal(vb)?;
                        } else {
                            skip_unknown(&mut er, entry_pre)?;
                        }
                    }
                    self.Checkpoints.insert(mapkey, mapvalue);
                }
                2 => {
                    let slice = read_len_delim(&mut r, "TaskCheckpoint", wt)?;
                    // 复用已有对象可保留 Go 版“在原对象上反序列化”的语义。
                    let mut task = self.TaskCheckpoint.take().unwrap_or_default();
                    task.Unmarshal(slice)?;
                    self.TaskCheckpoint = Some(task);
                }
                _ => skip_unknown(&mut r, pre)?,
            }
        }
        Ok(())
    }
}

/// `TaskCheckpointModel` 的编解码实现。
/// 该类型只有标量与字符串字段，因此逻辑最接近标准 proto3 生成代码。
impl TaskCheckpointModel {
    /// 分配恰好足够的缓冲区并返回完整编码结果。
    /// 先走 `Size()` 再编码，行为与 Go `Marshal()` 一致。
    /// 这样可以避免多次扩容带来的额外拷贝。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.Size());
        self.encode_to(&mut buf);
        Ok(buf)
    }

    /// 向外部提供的缓冲区写入编码结果。
    /// 该方法保留 Go 接口形状，便于平移上游调用代码。
    /// 真正的长度检查仍由内部复制逻辑统一处理。
    pub fn MarshalTo(&self, data: &mut [u8]) -> Result<usize> {
        let size = self.Size();
        if data.len() < size {
            return Err(Error::Other("buffer too small".into()));
        }
        self.MarshalToSizedBuffer(&mut data[..size])
    }

    /// 兼容 Go `MarshalToSizedBuffer` 名称。
    /// Rust 实现内部先完成独立编码，再复制到目标缓冲区。
    /// 这样可以在保持接口兼容的同时简化倒序写入实现。
    pub fn MarshalToSizedBuffer(&self, data: &mut [u8]) -> Result<usize> {
        copy_marshal_sized(&self.Marshal()?, data)
    }

    /// 直接把当前消息追加到目标缓冲区。
    /// 该类型不会产生局部错误，因此无需返回 `Result`。
    /// 空值字段会按照 proto3 规则被省略。
    fn encode_to(&self, buf: &mut Vec<u8>) {
        append_varint_field(buf, 1, self.TaskId as u64);
        append_string_field(buf, 2, &self.SourceDir);
        append_string_field(buf, 3, &self.Backend);
        append_string_field(buf, 4, &self.ImporterAddr);
        append_string_field(buf, 5, &self.TidbHost);
        append_varint_field(buf, 6, self.TidbPort as u64);
        append_string_field(buf, 7, &self.PdAddr);
        append_string_field(buf, 8, &self.SortedKvDir);
        append_string_field(buf, 9, &self.LightningVer);
    }

    /// 按 Go 生成器的计数方式预估编码后的精确长度。
    /// 零值字段在这里直接被忽略，确保与真实编码结果一致。
    /// 该类型只有标量与字符串字段，因此不会出现 map entry 展开。
    pub fn Size(&self) -> usize {
        size_varint_field(self.TaskId as u64)
            + size_string_field(&self.SourceDir)
            + size_string_field(&self.Backend)
            + size_string_field(&self.ImporterAddr)
            + size_string_field(&self.TidbHost)
            + size_varint_field(self.TidbPort as u64)
            + size_string_field(&self.PdAddr)
            + size_string_field(&self.SortedKvDir)
            + size_string_field(&self.LightningVer)
    }

    /// 解析 protobuf 字节流，并把结果合并进当前对象。
    /// 与 Go 一样，重复字段会以最后一次出现的值为准。
    /// 未识别字段会按 wire type 跳过，而不是作为未知字段保存。
    pub fn Unmarshal(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        while let Some((pre, field, wt, _)) = begin_field(&mut r, "TaskCheckpointModel")? {
            match field {
                1 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "TaskId",
                            wire_type: wt,
                        });
                    }
                    self.TaskId = r.read_varint()? as i64;
                }
                2 => self.SourceDir = read_string(&mut r, wt, "SourceDir")?,
                3 => self.Backend = read_string(&mut r, wt, "Backend")?,
                4 => self.ImporterAddr = read_string(&mut r, wt, "ImporterAddr")?,
                5 => self.TidbHost = read_string(&mut r, wt, "TidbHost")?,
                6 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "TidbPort",
                            wire_type: wt,
                        });
                    }
                    self.TidbPort = r.read_varint()? as i32;
                }
                7 => self.PdAddr = read_string(&mut r, wt, "PdAddr")?,
                8 => self.SortedKvDir = read_string(&mut r, wt, "SortedKvDir")?,
                9 => self.LightningVer = read_string(&mut r, wt, "LightningVer")?,
                _ => skip_unknown(&mut r, pre)?,
            }
        }
        Ok(())
    }
}

/// `TableCheckpointModel` 的编解码实现。
/// 这里既要处理 bytes/fixed64，也要处理 `engines` 这类 map 字段。
impl TableCheckpointModel {
    /// 分配恰好足够的缓冲区并返回完整编码结果。
    /// 先走 `Size()` 再编码，行为与 Go `Marshal()` 一致。
    /// 这样可以避免多次扩容带来的额外拷贝。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.Size());
        self.encode_to(&mut buf)?;
        Ok(buf)
    }

    /// 向外部提供的缓冲区写入编码结果。
    /// 该方法保留 Go 接口形状，便于平移上游调用代码。
    /// 真正的长度检查仍由内部复制逻辑统一处理。
    pub fn MarshalTo(&self, data: &mut [u8]) -> Result<usize> {
        let size = self.Size();
        if data.len() < size {
            return Err(Error::Other("buffer too small".into()));
        }
        self.MarshalToSizedBuffer(&mut data[..size])
    }

    /// 兼容 Go `MarshalToSizedBuffer` 名称。
    /// Rust 实现内部先完成独立编码，再复制到目标缓冲区。
    /// 这样可以在保持接口兼容的同时简化倒序写入实现。
    pub fn MarshalToSizedBuffer(&self, data: &mut [u8]) -> Result<usize> {
        copy_marshal_sized(&self.Marshal()?, data)
    }

    /// 直接把当前消息追加到目标缓冲区。
    /// map 或嵌套消息字段会先局部编码，再整体附加。
    /// 出错点主要来自子消息编码失败。
    fn encode_to(&self, buf: &mut Vec<u8>) -> Result<()> {
        append_bytes_field(buf, 1, &self.Hash);
        append_varint_field(buf, 3, self.Status as u64);
        let mut keys: Vec<i32> = self.Engines.keys().copied().collect();
        // 显式排序以消除 `HashMap` 的随机迭代顺序，保证跨语言输出稳定。
        keys.sort();
        for k in keys {
            let v = &self.Engines[&k];
            // map 会在线路层展开成独立的 entry 子消息，再作为长度定界字段写出。
            let mut entry = Vec::new();
            // engine map 的键是 `int32`，Go 生成器会按 zigzag32 形式写入。
            append_tag(&mut entry, 1, 0);
            append_varint(&mut entry, encode_zigzag32(k) as u64);
            let vb = v.Marshal()?;
            append_message_field(&mut entry, 2, &vb);
            append_message_field(buf, 8, &entry);
        }
        append_varint_field(buf, 9, self.TableID as u64);
        append_varint_field(buf, 10, self.KvBytes);
        append_varint_field(buf, 11, self.KvKvs);
        append_fixed64_field(buf, 12, self.KvChecksum);
        append_bytes_field(buf, 13, &self.TableInfo);
        append_varint_field(buf, 14, self.AutoRandBase as u64);
        append_varint_field(buf, 15, self.AutoIncrBase as u64);
        if self.AutoRowIDBase != 0 {
            append_tag(buf, 16, 0);
            append_varint(buf, self.AutoRowIDBase as u64);
        }
        Ok(())
    }

    /// 按 Go 生成器的计数方式预估编码后的精确长度。
    /// 零值字段在这里直接被忽略，确保与真实编码结果一致。
    /// map entry 的长度也会被展开后逐项累计。
    pub fn Size(&self) -> usize {
        let mut n = size_bytes_field(&self.Hash) + size_varint_field(self.Status as u64);
        for (k, v) in &self.Engines {
            let vl = v.Size();
            let map_entry_size = 1
                + sovFileCheckpoints(encode_zigzag32(*k) as u64)
                + 1
                + vl
                + sovFileCheckpoints(vl as u64);
            n += map_entry_size + 1 + sovFileCheckpoints(map_entry_size as u64);
        }
        n += size_varint_field(self.TableID as u64)
            + size_varint_field(self.KvBytes)
            + size_varint_field(self.KvKvs)
            + size_fixed64_field(self.KvChecksum)
            + size_bytes_field(&self.TableInfo)
            + size_varint_field(self.AutoRandBase as u64)
            + size_varint_field(self.AutoIncrBase as u64)
            + size_varint_field_tag2(self.AutoRowIDBase as u64);
        n
    }

    /// 解析 protobuf 字节流，并把结果合并进当前对象。
    /// 与 Go 一样，重复 map/message 字段会覆盖或增量更新现有值。
    /// 未识别字段会按 wire type 跳过，而不是作为未知字段保存。
    pub fn Unmarshal(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        while let Some((pre, field, wt, _)) = begin_field(&mut r, "TableCheckpointModel")? {
            match field {
                1 => self.Hash = read_bytes_field(&mut r, wt, "Hash")?,
                3 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Status",
                            wire_type: wt,
                        });
                    }
                    self.Status = r.read_varint()? as u32;
                }
                8 => {
                    let slice = read_len_delim(&mut r, "Engines", wt)?;
                    // 进入 map entry 子消息后，继续按 key/value 两个字段解析。
                    let mut er = Reader::new(slice);
                    let mut mapkey: i32 = 0;
                    let mut mapvalue = EngineCheckpointModel::default();
                    while er.i < slice.len() {
                        let entry_pre = er.i;
                        let w = er.read_varint()?;
                        let f = (w >> 3) as i32;
                        let ewt = (w & 0x7) as i32;
                        if f == 1 {
                            if ewt != 0 {
                                return Err(Error::WrongWireType {
                                    field: "engines key",
                                    wire_type: ewt,
                                });
                            }
                            // 先按原始 varint 读出，再手动恢复 zigzag 编码。
                            let mapkeytemp = er.read_varint()? as i32;
                            mapkey = decode_zigzag32(mapkeytemp);
                        } else if f == 2 {
                            let vb = read_len_delim(&mut er, "engines value", ewt)?;
                            // Match gogo's `mapvalue = &EngineCheckpointModel{}` assignment.
                            mapvalue = EngineCheckpointModel::default();
                            mapvalue.Unmarshal(vb)?;
                        } else {
                            skip_unknown(&mut er, entry_pre)?;
                        }
                    }
                    self.Engines.insert(mapkey, mapvalue);
                }
                9 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "TableID",
                            wire_type: wt,
                        });
                    }
                    self.TableID = r.read_varint()? as i64;
                }
                10 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "KvBytes",
                            wire_type: wt,
                        });
                    }
                    self.KvBytes = r.read_varint()?;
                }
                11 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "KvKvs",
                            wire_type: wt,
                        });
                    }
                    self.KvKvs = r.read_varint()?;
                }
                12 => {
                    if wt != 1 {
                        return Err(Error::WrongWireType {
                            field: "KvChecksum",
                            wire_type: wt,
                        });
                    }
                    self.KvChecksum = r.read_fixed64()?;
                }
                13 => self.TableInfo = read_bytes_field(&mut r, wt, "TableInfo")?,
                14 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "AutoRandBase",
                            wire_type: wt,
                        });
                    }
                    self.AutoRandBase = r.read_varint()? as i64;
                }
                15 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "AutoIncrBase",
                            wire_type: wt,
                        });
                    }
                    self.AutoIncrBase = r.read_varint()? as i64;
                }
                16 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "AutoRowIDBase",
                            wire_type: wt,
                        });
                    }
                    self.AutoRowIDBase = r.read_varint()? as i64;
                }
                _ => skip_unknown(&mut r, pre)?,
            }
        }
        Ok(())
    }
}

/// `EngineCheckpointModel` 的编解码实现。
/// 主要复杂度来自 `chunks` map 的稳定编码与增量合并。
impl EngineCheckpointModel {
    /// 分配恰好足够的缓冲区并返回完整编码结果。
    /// 先走 `Size()` 再编码，行为与 Go `Marshal()` 一致。
    /// 这样可以避免多次扩容带来的额外拷贝。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.Size());
        self.encode_to(&mut buf)?;
        Ok(buf)
    }

    /// 向外部提供的缓冲区写入编码结果。
    /// 该方法保留 Go 接口形状，便于平移上游调用代码。
    /// 真正的长度检查仍由内部复制逻辑统一处理。
    pub fn MarshalTo(&self, data: &mut [u8]) -> Result<usize> {
        let size = self.Size();
        if data.len() < size {
            return Err(Error::Other("buffer too small".into()));
        }
        self.MarshalToSizedBuffer(&mut data[..size])
    }

    /// 兼容 Go `MarshalToSizedBuffer` 名称。
    /// Rust 实现内部先完成独立编码，再复制到目标缓冲区。
    /// 这样可以在保持接口兼容的同时简化倒序写入实现。
    pub fn MarshalToSizedBuffer(&self, data: &mut [u8]) -> Result<usize> {
        copy_marshal_sized(&self.Marshal()?, data)
    }

    /// 直接把当前消息追加到目标缓冲区。
    /// map 或嵌套消息字段会先局部编码，再整体附加。
    /// 出错点主要来自子消息编码失败。
    fn encode_to(&self, buf: &mut Vec<u8>) -> Result<()> {
        append_varint_field(buf, 1, self.Status as u64);
        let mut keys: Vec<&String> = self.Chunks.keys().collect();
        // 显式排序以消除 `HashMap` 的随机迭代顺序，保证跨语言输出稳定。
        keys.sort();
        for k in keys {
            let v = &self.Chunks[k];
            // map 会在线路层展开成独立的 entry 子消息，再作为长度定界字段写出。
            let mut entry = Vec::new();
            append_string_field(&mut entry, 1, k);
            let vb = v.Marshal()?;
            append_message_field(&mut entry, 2, &vb);
            append_message_field(buf, 2, &entry);
        }
        Ok(())
    }

    /// 按 Go 生成器的计数方式预估编码后的精确长度。
    /// 零值字段在这里直接被忽略，确保与真实编码结果一致。
    /// map entry 的长度也会被展开后逐项累计。
    pub fn Size(&self) -> usize {
        let mut n = size_varint_field(self.Status as u64);
        for (k, v) in &self.Chunks {
            let vl = v.Size();
            let l = 1
                + k.len()
                + sovFileCheckpoints(k.len() as u64)
                + 1
                + vl
                + sovFileCheckpoints(vl as u64);
            n += l + 1 + sovFileCheckpoints(l as u64);
        }
        n
    }

    /// 解析 protobuf 字节流，并把结果合并进当前对象。
    /// 与 Go 一样，重复 map/message 字段会覆盖或增量更新现有值。
    /// 未识别字段会按 wire type 跳过，而不是作为未知字段保存。
    pub fn Unmarshal(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        while let Some((pre, field, wt, _)) = begin_field(&mut r, "EngineCheckpointModel")? {
            match field {
                1 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Status",
                            wire_type: wt,
                        });
                    }
                    self.Status = r.read_varint()? as u32;
                }
                2 => {
                    let slice = read_len_delim(&mut r, "Chunks", wt)?;
                    // 进入 map entry 子消息后，继续按 key/value 两个字段解析。
                    let mut er = Reader::new(slice);
                    let mut mapkey = String::new();
                    let mut mapvalue = ChunkCheckpointModel::default();
                    while er.i < slice.len() {
                        let entry_pre = er.i;
                        let w = er.read_varint()?;
                        let f = (w >> 3) as i32;
                        let ewt = (w & 0x7) as i32;
                        if f == 1 {
                            mapkey = read_string(&mut er, ewt, "key")?;
                        } else if f == 2 {
                            let vb = read_len_delim(&mut er, "value", ewt)?;
                            // Repeated map values replace rather than merge in the Go output.
                            mapvalue = ChunkCheckpointModel::default();
                            mapvalue.Unmarshal(vb)?;
                        } else {
                            skip_unknown(&mut er, entry_pre)?;
                        }
                    }
                    self.Chunks.insert(mapkey, mapvalue);
                }
                _ => skip_unknown(&mut r, pre)?,
            }
        }
        Ok(())
    }
}

/// `ChunkCheckpointModel` 的编解码实现。
/// packed int32、fixed64 与高编号字段都在此集中体现。
impl ChunkCheckpointModel {
    /// 分配恰好足够的缓冲区并返回完整编码结果。
    /// 先走 `Size()` 再编码，行为与 Go `Marshal()` 一致。
    /// 这样可以避免多次扩容带来的额外拷贝。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(self.Size());
        self.encode_to(&mut buf);
        Ok(buf)
    }

    /// 向外部提供的缓冲区写入编码结果。
    /// 该方法保留 Go 接口形状，便于平移上游调用代码。
    /// 真正的长度检查仍由内部复制逻辑统一处理。
    pub fn MarshalTo(&self, data: &mut [u8]) -> Result<usize> {
        let size = self.Size();
        if data.len() < size {
            return Err(Error::Other("buffer too small".into()));
        }
        self.MarshalToSizedBuffer(&mut data[..size])
    }

    /// 兼容 Go `MarshalToSizedBuffer` 名称。
    /// Rust 实现内部先完成独立编码，再复制到目标缓冲区。
    /// 这样可以在保持接口兼容的同时简化倒序写入实现。
    pub fn MarshalToSizedBuffer(&self, data: &mut [u8]) -> Result<usize> {
        copy_marshal_sized(&self.Marshal()?, data)
    }

    /// 直接把当前消息追加到目标缓冲区。
    /// 该类型不会产生局部错误，因此无需返回 `Result`。
    /// 空值字段会按照 proto3 规则被省略。
    fn encode_to(&self, buf: &mut Vec<u8>) {
        append_string_field(buf, 1, &self.Path);
        append_varint_field(buf, 2, self.Offset as u64);
        append_varint_field(buf, 5, self.EndOffset as u64);
        append_varint_field(buf, 6, self.Pos as u64);
        append_varint_field(buf, 7, self.PrevRowidMax as u64);
        append_varint_field(buf, 8, self.RowidMax as u64);
        append_varint_field(buf, 9, self.KvcBytes);
        append_varint_field(buf, 10, self.KvcKvs);
        append_fixed64_field(buf, 11, self.KvcChecksum);
        append_packed_int32(buf, 12, &self.ColumnPermutation);
        append_sfixed64_field(buf, 13, self.Timestamp);
        append_varint_field(buf, 14, self.Type as u64);
        append_varint_field(buf, 15, self.Compression as u64);
        if !self.SortKey.is_empty() {
            append_tag(buf, 16, 2);
            append_varint(buf, self.SortKey.len() as u64);
            buf.extend_from_slice(self.SortKey.as_bytes());
        }
        if self.FileSize != 0 {
            append_tag(buf, 17, 0);
            append_varint(buf, self.FileSize as u64);
        }
        if self.RealPos != 0 {
            append_tag(buf, 18, 0);
            append_varint(buf, self.RealPos as u64);
        }
    }

    /// 按 Go 生成器的计数方式预估编码后的精确长度。
    /// 零值字段在这里直接被忽略，确保与真实编码结果一致。
    /// packed repeated 字段会先累计 payload，再加上长度前缀。
    pub fn Size(&self) -> usize {
        let mut n = size_string_field(&self.Path)
            + size_varint_field(self.Offset as u64)
            + size_varint_field(self.EndOffset as u64)
            + size_varint_field(self.Pos as u64)
            + size_varint_field(self.PrevRowidMax as u64)
            + size_varint_field(self.RowidMax as u64)
            + size_varint_field(self.KvcBytes)
            + size_varint_field(self.KvcKvs)
            + size_fixed64_field(self.KvcChecksum);
        if !self.ColumnPermutation.is_empty() {
            let mut l = 0;
            for &e in &self.ColumnPermutation {
                l += sovFileCheckpoints(e as u64);
            }
            n += 1 + sovFileCheckpoints(l as u64) + l;
        }
        n += size_sfixed64_field(self.Timestamp)
            + size_varint_field(self.Type as u64)
            + size_varint_field(self.Compression as u64)
            + size_string_field_tag2(&self.SortKey)
            + size_varint_field_tag2(self.FileSize as u64)
            + size_varint_field_tag2(self.RealPos as u64);
        n
    }

    /// 解析 protobuf 字节流，并把结果合并进当前对象。
    /// 与 Go 一样，重复字段会以最后一次出现的值为准。
    /// 未识别字段会按 wire type 跳过，而不是作为未知字段保存。
    pub fn Unmarshal(&mut self, data: &[u8]) -> Result<()> {
        let mut r = Reader::new(data);
        while let Some((pre, field, wt, _)) = begin_field(&mut r, "ChunkCheckpointModel")? {
            match field {
                1 => self.Path = read_string(&mut r, wt, "Path")?,
                2 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Offset",
                            wire_type: wt,
                        });
                    }
                    self.Offset = r.read_varint()? as i64;
                }
                5 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "EndOffset",
                            wire_type: wt,
                        });
                    }
                    self.EndOffset = r.read_varint()? as i64;
                }
                6 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Pos",
                            wire_type: wt,
                        });
                    }
                    self.Pos = r.read_varint()? as i64;
                }
                7 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "PrevRowidMax",
                            wire_type: wt,
                        });
                    }
                    self.PrevRowidMax = r.read_varint()? as i64;
                }
                8 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "RowidMax",
                            wire_type: wt,
                        });
                    }
                    self.RowidMax = r.read_varint()? as i64;
                }
                9 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "KvcBytes",
                            wire_type: wt,
                        });
                    }
                    self.KvcBytes = r.read_varint()?;
                }
                10 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "KvcKvs",
                            wire_type: wt,
                        });
                    }
                    self.KvcKvs = r.read_varint()?;
                }
                11 => {
                    if wt != 1 {
                        return Err(Error::WrongWireType {
                            field: "KvcChecksum",
                            wire_type: wt,
                        });
                    }
                    self.KvcChecksum = r.read_fixed64()?;
                }
                12 => {
                    if wt == 0 {
                        self.ColumnPermutation.push(r.read_varint()? as i32);
                    } else if wt == 2 {
                        let packed = read_len_delim(&mut r, "ColumnPermutation", wt)?;
                        // packed repeated 字段先取整段切片，再在局部读取器中逐项解包。
                        let mut pr = Reader::new(packed);
                        while pr.i < packed.len() {
                            self.ColumnPermutation.push(pr.read_varint()? as i32);
                        }
                    } else {
                        return Err(Error::WrongWireType {
                            field: "ColumnPermutation",
                            wire_type: wt,
                        });
                    }
                }
                13 => {
                    if wt != 1 {
                        return Err(Error::WrongWireType {
                            field: "Timestamp",
                            wire_type: wt,
                        });
                    }
                    // Go 把 timestamp 按 `fixed64` 写入，Rust 直接复用相同位模式。
                    self.Timestamp = r.read_fixed64()? as i64;
                }
                14 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Type",
                            wire_type: wt,
                        });
                    }
                    self.Type = r.read_varint()? as i32;
                }
                15 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "Compression",
                            wire_type: wt,
                        });
                    }
                    self.Compression = r.read_varint()? as i32;
                }
                16 => self.SortKey = read_string(&mut r, wt, "SortKey")?,
                17 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "FileSize",
                            wire_type: wt,
                        });
                    }
                    self.FileSize = r.read_varint()? as i64;
                }
                18 => {
                    if wt != 0 {
                        return Err(Error::WrongWireType {
                            field: "RealPos",
                            wire_type: wt,
                        });
                    }
                    self.RealPos = r.read_varint()? as i64;
                }
                _ => skip_unknown(&mut r, pre)?,
            }
        }
        Ok(())
    }
}
