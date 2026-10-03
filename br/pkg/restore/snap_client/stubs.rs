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

//! Local stand-ins for PD/TiKV/gRPC/domain/kv/metautil boundaries
//! (darwin-safe; no kvproto/grpcio/kv/domain).
//! snap_client 测试与翻译期共用的桩/适配层，不是生产 PD/TiKV 实现。
//! 提供 Error/Result、metapb、codec、tablecodec、Mem* 客户端等最小能力。
//! 注释标明占位边界：返回默认值或内存状态，不代表真实集群语义已完整。
//! 调用方应把此处当作依赖注入点，而不是最终集成层。
//! 与 Go 类型名尽量同构，便于机械对照与 parity 测试。
//! Error/Result 是全包通用失败通道，code 字段对应 Go 错误分类。
//! MemPdClient/MemSplitClient/MemDomain 提供可注入的内存实现。
//! metapb/codec/tablecodec 子集足够支撑 snap_client 单测编码需求。
//! PlacementRule/LabelConstraint 结构与 PD HTTP JSON 字段对齐。
//! GetAllTiKVStoresWithRetry 在桩中可直接返回 stores 列表。
//! log 桩吞掉或打印消息，不引入 tracing 全局订阅。
//! berrors 常量用于 Annotate，保持上层 match 错误码可用。
//! Context 提供 Background/Done/Err 的最小取消模型。
//! CreatedTable/RegionInfo 等聚合类型减少测试样板代码。
//! 不要把此处 Default 返回值解读为生产默认配置已生效。
//! 新增桩方法前先确认 Go 边界是否真实需要，避免范围膨胀。
//! 字节/十六进制辅助函数必须与实现侧共用，防止测试与生产分叉。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// `Result`：类型别名，保持与 Go 命名空间可读对照。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// `Error`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct Error {
    pub msg: String,
    pub code: Option<&'static str>,
}

impl Error {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: None,
        }
    }

    /// `with_code`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn with_code(code: &'static str, msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            code: Some(code),
        }
    }

    /// `Trace`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// `Annotate`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
            code: err.code,
        }
    }

    /// `Annotatef`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self::Annotate(err, ctx)
    }

    /// `Errorf`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    /// `Wrap`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Wrap(err: Self, msg: impl Into<String>) -> Self {
        Self::Annotate(err, msg)
    }
}

impl fmt::Display for Error {
    /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

pub mod berrors {
    use super::Error;

    /// `ErrRestoreModeMismatch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrRestoreModeMismatch(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrRestoreModeMismatch", msg)
    }

    pub fn ErrRestoreRangeMismatch(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrRestoreRangeMismatch", msg)
    }

    pub fn ErrRestoreChecksumMismatch(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrRestoreChecksumMismatch", msg)
    }

    pub fn ErrStreamLogTaskExist(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Stream:ErrStreamLogTaskExist", msg)
    }

    /// `ErrPDInvalidResponse`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrPDInvalidResponse(msg: impl Into<String>) -> Error {
        Error::with_code("BR:PD:ErrPDInvalidResponse", msg)
    }

    /// `ErrUnknown`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrUnknown(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrUnknown", msg)
    }

    /// `ErrRestoreNotFreshCluster`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrRestoreNotFreshCluster(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Restore:ErrRestoreNotFreshCluster", msg)
    }

    /// `ErrUnsupportedOperation`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrUnsupportedOperation(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrUnsupportedOperation", msg)
    }

    /// `ErrInvalidArgument`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn ErrInvalidArgument(msg: impl Into<String>) -> Error {
        Error::with_code("BR:Common:ErrInvalidArgument", msg)
    }
}

#[derive(Clone, Default)]
/// `Context`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
    source: Option<Arc<dyn Fn() -> Option<Error> + Send + Sync>>,
    deadline: Option<std::time::Instant>,
}

impl Context {
    /// `Background`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Background() -> Self {
        Self::default()
    }

    /// `cancel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// `Err`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.source.as_ref().and_then(|source| source()))
            .or_else(|| {
                self.deadline
                    .filter(|d| std::time::Instant::now() >= *d)
                    .map(|_| Error::with_code("DeadlineExceeded", "context deadline exceeded"))
            })
    }

    pub fn WithCancellationSource(
        source: impl Fn() -> Option<Error> + Send + Sync + 'static,
    ) -> Self {
        Self {
            source: Some(Arc::new(source)),
            ..Self::default()
        }
    }

    pub fn WithTimeout(&self, duration: std::time::Duration) -> Self {
        let parent = self.clone();
        Self {
            source: Some(Arc::new(move || parent.Err())),
            deadline: Some(std::time::Instant::now() + duration),
            ..Self::default()
        }
    }

    /// `Done`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

pub mod log {
    /// `Info`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Info(_msg: &str) {}
    /// `Warn`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Warn(_msg: &str) {}
    /// `Error`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Error(_msg: &str) {}
    /// `Debug`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Debug(_msg: &str) {}
    /// `Panic`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Panic(msg: &str) -> ! {
        panic!("{msg}")
    }
}

pub mod summary {
    use std::sync::Mutex;

    /// `TotalKV`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    pub const TotalKV: &str = "TotalKV";
    /// `SkippedKVCountByCheckpoint`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    pub const SkippedKVCountByCheckpoint: &str = "SkippedKVCountByCheckpoint";
    /// `TotalBytes`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    pub const TotalBytes: &str = "TotalBytes";
    /// `SkippedBytesByCheckpoint`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    pub const SkippedBytesByCheckpoint: &str = "SkippedBytesByCheckpoint";

    static UNITS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
    static INTS: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());

    /// `CollectSuccessUnit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn CollectSuccessUnit(name: &str, _n: i32, v: u64) {
        UNITS.lock().unwrap().push((name.to_string(), v));
    }

    /// `CollectDuration`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn CollectDuration(_name: &str, _d: std::time::Duration) {}

    /// `CollectInt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn CollectInt(name: &str, v: i32) {
        INTS.lock().unwrap().push((name.to_string(), v));
    }

    /// `reset_for_test`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn reset_for_test() {
        UNITS.lock().unwrap().clear();
        INTS.lock().unwrap().clear();
    }

    /// `units_for_test`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn units_for_test() -> Vec<(String, u64)> {
        UNITS.lock().unwrap().clone()
    }

    /// Matches Go `summary.Succeed` used by PiTR collector close path.
    /// `Succeed`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn Succeed() -> bool {
        true
    }
}

/// `DefaultCFName`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const DefaultCFName: &str = "default";
/// `WriteCFName`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const WriteCFName: &str = "write";
/// `temporaryDBNamePrefix`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_";
/// `SystemDB`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const SystemDB: &str = "mysql";
/// `SysDB`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const SysDB: &str = "sys";
/// `WorkloadSchema`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const WorkloadSchema: &str = "workload_schema";

/// `TemporaryDBName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn TemporaryDBName(db: &str) -> String {
    format!("{temporaryDBNamePrefix}{db}")
}

/// `StripTempDBPrefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn StripTempDBPrefix(temp_db: &str) -> (String, bool) {
    if let Some(rest) = temp_db.strip_prefix(temporaryDBNamePrefix) {
        (rest.to_string(), true)
    } else {
        (temp_db.to_string(), false)
    }
}

/// `StripTempDBPrefixIfNeeded`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn StripTempDBPrefixIfNeeded(temp_db: &str) -> String {
    StripTempDBPrefix(temp_db).0
}

/// `IsSysDB`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn IsSysDB(db_lower_name: &str) -> bool {
    db_lower_name == SystemDB || db_lower_name == SysDB || db_lower_name == WorkloadSchema
}

/// `IsSysOrTempSysDB`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn IsSysOrTempSysDB(db: &str) -> bool {
    IsSysDB(&StripTempDBPrefixIfNeeded(db))
}

/// `EncloseName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn EncloseName(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// `EncloseDBAndTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn EncloseDBAndTable(database: &str, table: &str) -> String {
    format!("{}.{}", EncloseName(database), EncloseName(table))
}

pub mod codec {
    /// `ENC_GROUP_SIZE`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    const ENC_GROUP_SIZE: usize = 8;
    /// `ENC_MARKER`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    const ENC_MARKER: u8 = 0xff;
    /// `ENC_PAD`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    const ENC_PAD: u8 = 0x0;

    /// `EncodeBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn EncodeBytes(mut b: Vec<u8>, data: &[u8]) -> Vec<u8> {
        let d_len = data.len();
        for idx in (0..=d_len).step_by(ENC_GROUP_SIZE) {
            let remain = d_len.saturating_sub(idx);
            let pad_count = if remain >= ENC_GROUP_SIZE {
                b.extend_from_slice(&data[idx..idx + ENC_GROUP_SIZE]);
                0
            } else {
                let pad_count = ENC_GROUP_SIZE - remain;
                b.extend_from_slice(&data[idx..]);
                b.extend(vec![ENC_PAD; pad_count]);
                pad_count
            };
            let marker = ENC_MARKER - pad_count as u8;
            b.push(marker);
        }
        b
    }

    /// `DecodeBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn DecodeBytes(b: &[u8], _buf: Option<Vec<u8>>) -> Result<(Vec<u8>, Vec<u8>), String> {
        if b.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let mut data = Vec::new();
        let mut idx = 0;
        loop {
            if idx + ENC_GROUP_SIZE >= b.len() {
                return Err("insufficient bytes to decode value".into());
            }
            let group = &b[idx..idx + ENC_GROUP_SIZE];
            idx += ENC_GROUP_SIZE;
            let marker = b[idx];
            idx += 1;
            let pad = (ENC_MARKER - marker) as usize;
            if pad > ENC_GROUP_SIZE {
                return Err("invalid marker".into());
            }
            let real = ENC_GROUP_SIZE - pad;
            data.extend_from_slice(&group[..real]);
            if pad != 0 {
                return Ok((b[idx..].to_vec(), data));
            }
        }
    }
}

pub mod tablecodec {
    /// `TABLE_PREFIX`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    const TABLE_PREFIX: u8 = b't';
    /// `RECORD_PREFIX_SEP`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
    const RECORD_PREFIX_SEP: &[u8] = b"_r";

    /// `encode_int`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn encode_int(buf: &mut Vec<u8>, v: i64) {
        let u = (v as u64) ^ (1u64 << 63);
        buf.extend_from_slice(&u.to_be_bytes());
    }

    /// `EncodeTablePrefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn EncodeTablePrefix(table_id: i64) -> Vec<u8> {
        let mut key = Vec::with_capacity(9);
        key.push(TABLE_PREFIX);
        encode_int(&mut key, table_id);
        key
    }

    /// Matches Go `tablecodec.IsRecordKey` for row-count stats.
    /// `IsRecordKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn IsRecordKey(key: &[u8]) -> bool {
        if key.len() < 11 || key[0] != TABLE_PREFIX {
            return false;
        }
        &key[9..11] == RECORD_PREFIX_SEP
    }
}

pub mod backuppb {
    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `File`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct File {
        pub Name: String,
        #[serde(default)]
        pub StartKey: Vec<u8>,
        #[serde(default)]
        pub EndKey: Vec<u8>,
        #[serde(default)]
        pub TotalBytes: u64,
        #[serde(default)]
        pub TotalKvs: u64,
        #[serde(default)]
        pub Cf: String,
        #[serde(default, rename = "Size")]
        pub Size_: u64,
        #[serde(default)]
        pub CipherIv: Vec<u8>,
        #[serde(default)]
        pub Crc64Xor: u64,
    }

    impl File {
        /// `GetStartKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        /// `GetEndKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
        /// `GetName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetName(&self) -> &str {
            &self.Name
        }
        /// `GetCf`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetCf(&self) -> &str {
            &self.Cf
        }
        /// `GetSize_`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetSize_(&self) -> u64 {
            if self.Size_ != 0 {
                self.Size_
            } else {
                self.TotalBytes
            }
        }
        /// `GetCipherIv`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetCipherIv(&self) -> &[u8] {
            &self.CipherIv
        }
        /// `GetTotalKvs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetTotalKvs(&self) -> u64 {
            self.TotalKvs
        }
        /// `GetTotalBytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetTotalBytes(&self) -> u64 {
            self.TotalBytes
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `CipherInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct CipherInfo {
        pub CipherKey: Vec<u8>,
        pub CipherType: i32,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `StorageBackend`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct StorageBackend {
        pub Path: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RawRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Cf: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Policy {
        pub Info: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `BackupMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct BackupMeta {
        pub ClusterId: u64,
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub IsRawKv: bool,
        pub IsTxnKv: bool,
        pub ApiVersion: i32,
        pub Files: Vec<File>,
        pub RawRanges: Vec<RawRange>,
        pub Policies: Vec<Policy>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `IngestedSSTs`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct IngestedSSTs {
        #[serde(default)]
        pub Files: Vec<File>,
        #[serde(default)]
        pub RewrittenTables: Vec<RewrittenTableID>,
        #[serde(default)]
        pub Finished: bool,
        #[serde(default, rename = "AsIfTs")]
        pub AsOfTs: u64,
        #[serde(default)]
        pub RestoredTs: u64,
        #[serde(default, rename = "BackupUuid")]
        pub RestoreUuid: Vec<u8>,
        #[serde(default)]
        pub FilesPrefixHint: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    /// `RewrittenTableID`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct RewrittenTableID {
        pub AncestorUpstream: i64,
        pub Upstream: i64,
    }
}

pub mod import_sstpb {
    use std::collections::HashMap;

    use super::backuppb;
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `RewriteRule`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct RewriteRule {
        pub OldKeyPrefix: Vec<u8>,
        pub NewKeyPrefix: Vec<u8>,
        pub NewTimestamp: u64,
        pub IgnoreAfterTimestamp: u64,
        pub IgnoreBeforeTimestamp: u64,
    }

    impl RewriteRule {
        /// `GetOldKeyPrefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetOldKeyPrefix(&self) -> &[u8] {
            &self.OldKeyPrefix
        }
        /// `GetNewKeyPrefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetNewKeyPrefix(&self) -> &[u8] {
            &self.NewKeyPrefix
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Range`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Range {
        pub Start: Vec<u8>,
        pub End: Vec<u8>,
    }

    impl Range {
        /// `GetStart`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetStart(&self) -> &[u8] {
            &self.Start
        }
        /// `GetEnd`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetEnd(&self) -> &[u8] {
            &self.End
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `RegionEpoch`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct RegionEpoch {
        pub ConfVer: u64,
        pub Version: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `SSTMeta`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct SSTMeta {
        pub Uuid: Vec<u8>,
        pub CfName: String,
        pub Range: Option<Range>,
        pub Length: u64,
        pub RegionId: u64,
        pub RegionEpoch: Option<RegionEpoch>,
        pub CipherIv: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DownloadRequest {
        pub Sst: SSTMeta,
        pub StorageBackend: Option<backuppb::StorageBackend>,
        pub Name: String,
        pub RewriteRule: RewriteRule,
        pub CipherInfo: Option<backuppb::CipherInfo>,
        pub StorageCacheId: String,
        pub Ssts: HashMap<String, SSTMeta>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DownloadError {
        pub Message: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct DownloadResponse {
        pub Error: Option<DownloadError>,
        pub Range: Option<Range>,
        pub IsEmpty: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct KvContext {
        pub RegionId: u64,
        pub RegionEpoch: Option<RegionEpoch>,
        pub Peer: Option<super::metapb::Peer>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MultiIngestRequest {
        pub Context: Option<KvContext>,
        pub Ssts: Vec<SSTMeta>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct IngestResponse {
        pub Error: Option<DownloadError>,
    }

    impl SSTMeta {
        /// `GetRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetRange(&self) -> Range {
            self.Range.clone().unwrap_or_default()
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `SetDownloadSpeedLimitRequest`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct SetDownloadSpeedLimitRequest {
        pub TaskId: String,
        pub SpeedLimit: u64,
        pub TtlSeconds: u64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `AddPartitionRangeRequest`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct AddPartitionRangeRequest {
        pub Range: Range,
        pub TtlSeconds: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `RemovePartitionRangeRequest`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct RemovePartitionRangeRequest {
        pub Range: Range,
    }
}

pub mod metapb {
    use super::import_sstpb;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `StoreLabel`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct StoreLabel {
        pub Key: String,
        pub Value: String,
    }

    impl StoreLabel {
        /// `GetKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetKey(&self) -> &str {
            &self.Key
        }
        /// `GetValue`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetValue(&self) -> &str {
            &self.Value
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    /// `StoreState`：枚举分支表达协议/阶段差异，勿把占位变体当成已实现能力。
    pub enum StoreState {
        #[default]
        Up = 0,
        Offline = 1,
        Tombstone = 2,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Store`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub StatusAddress: String,
        pub State: StoreState,
        pub Labels: Vec<StoreLabel>,
    }

    impl Store {
        /// `GetId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
        /// `GetState`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetState(&self) -> StoreState {
            self.State
        }
        /// `GetLabels`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetLabels(&self) -> &[StoreLabel] {
            &self.Labels
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Peer`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Peer {
        pub Id: u64,
        pub StoreId: u64,
    }

    impl Peer {
        /// `GetStoreId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetStoreId(&self) -> u64 {
            self.StoreId
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Region`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub RegionEpoch: Option<import_sstpb::RegionEpoch>,
        pub Peers: Vec<Peer>,
    }

    impl Region {
        /// `GetId`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
        /// `GetStartKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        /// `GetEndKey`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
        /// `GetRegionEpoch`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetRegionEpoch(&self) -> Option<import_sstpb::RegionEpoch> {
            self.RegionEpoch.clone()
        }
        /// `GetPeers`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn GetPeers(&self) -> &[Peer] {
            &self.Peers
        }
    }
}

pub mod model {
    use std::fmt;

    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
    /// `CIStr`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }

    impl CIStr {
        /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        pub fn new(name: impl Into<String>) -> Self {
            let O = name.into();
            let L = O.to_lowercase();
            Self { O, L }
        }
    }

    impl fmt::Display for CIStr {
        /// `fmt`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
        /// 留意空集合、取消上下文与默认值是否保持一致。
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.O)
        }
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `ColumnInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct ColumnInfo {
        pub Name: CIStr,
        pub Collate: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `PartitionDefinition`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct PartitionDefinition {
        pub ID: i64,
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `PartitionInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `IndexInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Global: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `TableInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Columns: Vec<ColumnInfo>,
        pub Partition: Option<PartitionInfo>,
        pub Indices: Vec<IndexInfo>,
        pub IsCommonHandle: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `DBInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct DBInfo {
        pub ID: i64,
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `Job`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Job {
        pub SchemaName: String,
        pub Query: String,
        pub SchemaVersion: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct PolicyInfo {
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    /// `MetaUpdate`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct MetaUpdate {
        pub PhysicalID: i64,
        pub Count: i64,
        pub ModifyCount: i64,
    }
}

/// `GetPartitionByName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GetPartitionByName(table: &model::TableInfo, name: &model::CIStr) -> Result<i64> {
    if let Some(part) = &table.Partition {
        for def in &part.Definitions {
            if def.Name.L == name.L {
                return Ok(def.ID);
            }
        }
    }
    Err(Error::new(format!("partition {} not found", name.O)))
}

/// `StatsHandler`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait StatsHandler: Send + Sync {
    /// `SaveMetaToStorage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SaveMetaToStorage(
        &self,
        _source: &str,
        _update_cache: bool,
        updates: &[model::MetaUpdate],
    ) -> Result<()>;
}

#[derive(Default)]
/// `MemStatsHandler`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct MemStatsHandler {
    pub saved: Mutex<Vec<model::MetaUpdate>>,
}

impl StatsHandler for MemStatsHandler {
    /// `SaveMetaToStorage`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SaveMetaToStorage(
        &self,
        _source: &str,
        _update_cache: bool,
        updates: &[model::MetaUpdate],
    ) -> Result<()> {
        self.saved.lock().unwrap().extend_from_slice(updates);
        Ok(())
    }
}

/// `GlueProgress`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait GlueProgress: Send {
    /// `Inc`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn Inc(&mut self);
    /// `IncBy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn IncBy(&mut self, n: i64);
}

#[derive(Default)]
/// `CountingProgress`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct CountingProgress {
    pub n: i64,
}

impl GlueProgress for CountingProgress {
    /// `Inc`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn Inc(&mut self) {
        self.n += 1;
    }
    /// `IncBy`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn IncBy(&mut self, n: i64) {
        self.n += n;
    }
}

/// `Glue`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait Glue: Send + Sync {}

#[derive(Default)]
/// `MemGlue`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct MemGlue;

impl Glue for MemGlue {}

/// `DbSession`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait DbSession: Send {
    /// `Execute`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()>;
    /// `ExecDDL`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ExecDDL(&mut self, _ctx: &Context, job: &model::Job) -> Result<()>;
    /// `RegisterPreallocatedIDs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn RegisterPreallocatedIDs(&mut self, _ids: &PreallocIDs);
    /// `CreateTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        _table: &metautil::Table,
        _rebased: &HashMap<UniqueTableName, bool>,
        _support_policy: bool,
    ) -> Result<()>;

    fn CreateTables(
        &mut self,
        ctx: &Context,
        tables: &[metautil::Table],
        rebased: &HashMap<UniqueTableName, bool>,
        support_policy: bool,
    ) -> Result<()> {
        for table in tables {
            self.CreateTable(ctx, table, rebased, support_policy)?;
        }
        Ok(())
    }

    fn CreateDatabase(
        &mut self,
        ctx: &Context,
        database: &metautil::Database,
        _support_policy: bool,
    ) -> Result<bool> {
        self.Execute(ctx, &format!("CREATE DATABASE {}", database.Info.Name.O))?;
        Ok(false)
    }

    fn CreatePlacementPolicy(&mut self, ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.Execute(ctx, &format!("CREATE PLACEMENT POLICY {}", policy.Name.O))
    }

    fn Close(&mut self) {}

    fn GetGlobalID(&mut self) -> Result<i64> {
        Ok(0)
    }

    fn AdvanceGlobalIDs(&mut self, n: usize) -> Result<i64> {
        Ok(n as i64)
    }
}

#[derive(Default)]
/// `MemDb`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct MemDb {
    pub sqls: Vec<String>,
    pub prealloc: Option<PreallocIDs>,
    pub databases: HashSet<String>,
    pub closed: bool,
    pub global_id: i64,
}

impl DbSession for MemDb {
    /// `Execute`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.sqls.push(sql.to_string());
        Ok(())
    }

    /// `ExecDDL`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ExecDDL(&mut self, _ctx: &Context, job: &model::Job) -> Result<()> {
        self.sqls.push(job.Query.clone());
        Ok(())
    }

    /// `RegisterPreallocatedIDs`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn RegisterPreallocatedIDs(&mut self, ids: &PreallocIDs) {
        self.prealloc = Some(ids.clone());
    }

    /// `CreateTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        table: &metautil::Table,
        _rebased: &HashMap<UniqueTableName, bool>,
        _support_policy: bool,
    ) -> Result<()> {
        self.sqls.push(format!(
            "CREATE TABLE {}.{}",
            table.DB.Name.O, table.Info.Name.O
        ));
        Ok(())
    }

    fn CreateDatabase(
        &mut self,
        _ctx: &Context,
        database: &metautil::Database,
        _support_policy: bool,
    ) -> Result<bool> {
        let name = database.Info.Name.O.clone();
        let existed = !self.databases.insert(name.clone());
        self.sqls.push(format!("CREATE DATABASE {name}"));
        Ok(existed)
    }

    fn Close(&mut self) {
        self.closed = true;
    }

    fn GetGlobalID(&mut self) -> Result<i64> {
        Ok(self.global_id)
    }

    fn AdvanceGlobalIDs(&mut self, n: usize) -> Result<i64> {
        self.global_id += n as i64;
        Ok(self.global_id)
    }
}

/// `DomainLike`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait DomainLike: Send + Sync {
    /// `AssertUserDBsEmpty`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn AssertUserDBsEmpty(&self) -> Result<()>;
    /// `TableInfoByName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn TableInfoByName(&self, schema: &str, table: &str) -> Result<model::TableInfo>;

    fn UpdateMergeOptionRules(&self, _ctx: &Context, _rules: &[MergeOptionRule]) -> Result<()> {
        Ok(())
    }

    fn GetAPIVersion(&self) -> i32 {
        0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOptionRule {
    pub DbName: String,
    pub TableName: String,
    pub PartitionName: String,
    pub PhysicalID: i64,
}

#[derive(Default)]
/// `MemDomain`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct MemDomain {
    pub empty: bool,
    pub tables: Mutex<HashMap<(String, String), model::TableInfo>>,
}

impl DomainLike for MemDomain {
    /// `AssertUserDBsEmpty`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn AssertUserDBsEmpty(&self) -> Result<()> {
        if self.empty {
            Ok(())
        } else {
            Err(berrors::ErrRestoreNotFreshCluster("cluster not fresh"))
        }
    }

    /// `TableInfoByName`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn TableInfoByName(&self, schema: &str, table: &str) -> Result<model::TableInfo> {
        self.tables
            .lock()
            .unwrap()
            .get(&(schema.to_string(), table.to_string()))
            .cloned()
            .ok_or_else(|| Error::new(format!("table {schema}.{table} not found")))
    }
}

pub mod metautil {
    use super::backuppb;
    use super::model;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Debug, Default)]
    /// `Table`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Table {
        pub DB: model::DBInfo,
        pub Info: model::TableInfo,
        pub FilesOfPhysicals: HashMap<i64, Vec<backuppb::File>>,
        pub IsMergeOptionAllowed: bool,
        pub PartitionMergeOptionAllowed: HashMap<String, bool>,
    }

    #[derive(Clone, Debug, Default)]
    /// `Database`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
    pub struct Database {
        pub Info: model::DBInfo,
        pub Tables: Vec<Table>,
        pub(crate) reused_by_pitr: Arc<AtomicBool>,
    }

    impl Database {
        pub fn SetReusedByPITR(&self) {
            self.reused_by_pitr.store(true, Ordering::SeqCst);
        }

        pub fn IsReusedByPITR(&self) -> bool {
            self.reused_by_pitr.load(Ordering::SeqCst)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `TableIDRemap`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct TableIDRemap {
    pub Origin: i64,
    pub Rewritten: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// `RewriteRules`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct RewriteRules {
    pub Data: Vec<import_sstpb::RewriteRule>,
    pub NewTableID: i64,
    pub NewKeyspace: Vec<u8>,
    pub TableIDRemapHint: Vec<TableIDRemap>,
}

impl RewriteRules {
    /// `new_prefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn new_prefix(old: &[u8], new: &[u8]) -> Self {
        Self {
            Data: vec![import_sstpb::RewriteRule {
                OldKeyPrefix: old.to_vec(),
                NewKeyPrefix: new.to_vec(),
                NewTimestamp: 0,
                IgnoreAfterTimestamp: 0,
                IgnoreBeforeTimestamp: 0,
            }],
            NewTableID: 0,
            NewKeyspace: Vec::new(),
            TableIDRemapHint: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
/// `CreatedTable`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct CreatedTable {
    pub RewriteRule: Option<RewriteRules>,
    pub Table: model::TableInfo,
    pub OldTable: metautil::Table,
}

#[derive(Clone, Debug, Default)]
/// `BackupFileSet`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct BackupFileSet {
    pub TableID: i64,
    pub SSTFiles: Vec<backuppb::File>,
    pub RewriteRules: Option<RewriteRules>,
}

/// `BatchBackupFileSet`：类型别名，保持与 Go 命名空间可读对照。
pub type BatchBackupFileSet = Vec<BackupFileSet>;

#[derive(Clone, Debug, Default)]
/// `RegionInfo`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct RegionInfo {
    pub Region: metapb::Region,
    pub Leader: Option<metapb::Peer>,
}

#[derive(Clone, Debug, Default)]
/// `RangeStats`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct RangeStats {
    pub Size: u64,
    pub Count: u64,
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    pub Files: Vec<backuppb::File>,
}

#[derive(Clone, Debug, Default)]
/// `MergeRangesStat`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct MergeRangesStat {
    pub TotalFiles: i32,
    pub TotalWriteCFFile: i32,
    pub TotalDefaultCFFile: i32,
    pub TotalRegions: i32,
    pub RegionKeysAvg: i32,
    pub RegionBytesAvg: i32,
    pub MergedRegions: i32,
    pub MergedRegionKeysAvg: i32,
    pub MergedRegionBytesAvg: i32,
}

/// `GetPartitionIDMap`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GetPartitionIDMap(
    new_table: &model::TableInfo,
    old_table: &model::TableInfo,
) -> HashMap<i64, i64> {
    let mut table_id_map = HashMap::new();
    if let (Some(old_part), Some(new_part)) = (&old_table.Partition, &new_table.Partition) {
        let mut name_map: HashMap<String, i64> = HashMap::new();
        for old in &old_part.Definitions {
            name_map.insert(old.Name.L.clone(), old.ID);
        }
        for new in &new_part.Definitions {
            if let Some(old_id) = name_map.get(&new.Name.L) {
                table_id_map.insert(*old_id, new.ID);
            }
        }
    }
    table_id_map
}

/// `match_old_prefix`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn match_old_prefix<'a>(
    key: &[u8],
    rules: &'a RewriteRules,
) -> Option<&'a import_sstpb::RewriteRule> {
    rules.Data.iter().find(|r| key.starts_with(&r.OldKeyPrefix))
}

/// `rewrite_and_encode_raw_key`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn rewrite_and_encode_raw_key(
    key: &[u8],
    rule: Option<&import_sstpb::RewriteRule>,
) -> Option<Vec<u8>> {
    let rule = rule?;
    let rewritten = if key.starts_with(&rule.OldKeyPrefix) {
        [
            rule.NewKeyPrefix.clone(),
            key[rule.OldKeyPrefix.len()..].to_vec(),
        ]
        .concat()
    } else {
        key.to_vec()
    };
    Some(codec::EncodeBytes(Vec::new(), &rewritten))
}

/// `GetRewriteRawKeys`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn GetRewriteRawKeys(
    file: &backuppb::File,
    rules: Option<&RewriteRules>,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let rewrite = |key: &[u8]| -> Result<Vec<u8>> {
        if rules.is_none() {
            return Ok(codec::EncodeBytes(Vec::new(), key));
        }
        if key.is_empty() {
            return Ok(Vec::new());
        }
        let rule = match_old_prefix(key, rules.unwrap());
        rewrite_and_encode_raw_key(key, rule)
            .ok_or_else(|| Error::new("cannot find raw rewrite rule"))
    };
    Ok((rewrite(file.GetStartKey())?, rewrite(file.GetEndKey())?))
}

/// `ValidateFileRewriteRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn ValidateFileRewriteRule(
    _file: &backuppb::File,
    _rules: Option<&RewriteRules>,
) -> Result<()> {
    Ok(())
}

/// Simplified MergeAndRewriteFileRanges matching Go grouping/merge semantics.
/// `MergeAndRewriteFileRanges`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn MergeAndRewriteFileRanges(
    files: Vec<backuppb::File>,
    rewrite_rules: Option<&RewriteRules>,
    split_size_bytes: u64,
    split_key_count: u64,
) -> Result<(Vec<RangeStats>, MergeRangesStat)> {
    if files.is_empty() {
        return Ok((Vec::new(), MergeRangesStat::default()));
    }

    let mut files_map: HashMap<(Vec<u8>, Vec<u8>), Vec<backuppb::File>> = HashMap::new();
    let mut write_cf = 0i32;
    let mut default_cf = 0i32;
    let total_files = files.len() as i32;
    let mut total_bytes = 0u64;
    let mut total_kvs = 0u64;

    for file in files {
        let key = (file.StartKey.clone(), file.EndKey.clone());
        if file.Cf == WriteCFName || file.Name.contains(WriteCFName) {
            write_cf += 1;
        } else if file.Cf == DefaultCFName || file.Name.contains(DefaultCFName) {
            default_cf += 1;
        }
        total_bytes += file.TotalBytes;
        total_kvs += file.TotalKvs;
        files_map.entry(key).or_default().push(file);
    }

    let mut ranges: Vec<RangeStats> = Vec::new();
    for ((start, end), group) in files_map {
        let mut size = 0u64;
        let mut count = 0u64;
        for f in &group {
            size += f.TotalBytes;
            count += f.TotalKvs;
        }
        let (start_key, end_key) = if let Some(rules) = rewrite_rules {
            let sample = &group[0];
            GetRewriteRawKeys(sample, Some(rules))?
        } else {
            (
                codec::EncodeBytes(Vec::new(), &start),
                if end.is_empty() {
                    Vec::new()
                } else {
                    codec::EncodeBytes(Vec::new(), &end)
                },
            )
        };
        let _ = (start, end);
        ranges.push(RangeStats {
            Size: size,
            Count: count,
            StartKey: start_key,
            EndKey: end_key,
            Files: group,
        });
    }

    ranges.sort_by(|a, b| a.StartKey.cmp(&b.StartKey).then(a.EndKey.cmp(&b.EndKey)));

    // Merge adjacent ranges under thresholds (same spirit as rtree MergedRanges).
    let mut merged: Vec<RangeStats> = Vec::new();
    for rg in ranges {
        if let Some(last) = merged.last_mut() {
            let after_size = last.Size + rg.Size;
            let after_count = last.Count + rg.Count;
            if after_size <= split_size_bytes && after_count <= split_key_count {
                last.Size = after_size;
                last.Count = after_count;
                last.EndKey = rg.EndKey;
                last.Files.extend(rg.Files);
                continue;
            }
        }
        merged.push(rg);
    }

    let total_regions = write_cf.max(default_cf).max(1);
    let merged_regions = merged.len() as i32;
    Ok((
        merged,
        MergeRangesStat {
            TotalFiles: total_files,
            TotalWriteCFFile: write_cf,
            TotalDefaultCFFile: default_cf,
            TotalRegions: total_regions,
            RegionKeysAvg: (total_kvs / total_regions as u64) as i32,
            RegionBytesAvg: (total_bytes / total_regions as u64) as i32,
            MergedRegions: merged_regions,
            MergedRegionKeysAvg: if merged_regions == 0 {
                0
            } else {
                (total_kvs / merged_regions as u64) as i32
            },
            MergedRegionBytesAvg: if merged_regions == 0 {
                0
            } else {
                (total_bytes / merged_regions as u64) as i32
            },
        },
    ))
}

/// `BuildWorkerTokenChannel`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn BuildWorkerTokenChannel(size: usize) -> Arc<Mutex<VecDeque<()>>> {
    let mut q = VecDeque::with_capacity(size);
    for _ in 0..size {
        q.push_back(());
    }
    Arc::new(Mutex::new(q))
}

static UUID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// `new_uuid_bytes`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn new_uuid_bytes() -> Vec<u8> {
    let n = UUID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut out = vec![0u8; 16];
    out[8..16].copy_from_slice(&n.to_be_bytes());
    out
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
/// `UniqueTableName`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct UniqueTableName {
    pub DB: String,
    pub Table: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsConfig {
    pub CaPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableLocationInfo {
    pub ParentTableID: i64,
    pub TableName: String,
    pub DbID: i64,
    pub IsPartition: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterConfig {
    pub Schedulers: Vec<String>,
    pub ScheduleCfg: String,
    pub RuleID: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckpointMetadata {
    pub UpstreamClusterID: u64,
    pub RestoreStartTS: u64,
    pub RestoredTS: u64,
    pub LogRestoredTS: u64,
    pub Hash: Vec<u8>,
    pub PreallocIDs: Option<PreallocIDs>,
    pub RestoreUUID: Vec<u8>,
    pub SchedulersConfig: Option<ClusterConfig>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChecksumItem {
    pub TableID: i64,
    pub Crc64xor: u64,
    pub TotalKvs: u64,
    pub TotalBytes: u64,
}

pub trait CheckpointRunner: Send {
    fn WaitForFinish(&mut self, ctx: &Context, flush: bool);

    fn FlushChecksumItem(&mut self, _ctx: &Context, _item: &ChecksumItem) -> Result<()> {
        Ok(())
    }
}

pub trait SnapshotCheckpointManager: Send + Sync {
    fn LoadCheckpointMetadata(&self, ctx: &Context) -> Result<CheckpointMetadata>;
    fn SaveCheckpointMetadata(&self, ctx: &Context, metadata: &CheckpointMetadata) -> Result<()>;
    fn LoadCheckpointData(&self, ctx: &Context) -> Result<Vec<(i64, String)>>;
    fn LoadCheckpointChecksum(&self, ctx: &Context) -> Result<HashMap<i64, ChecksumItem>>;
    fn StartCheckpointRunner(&self, ctx: &Context) -> Result<Box<dyn CheckpointRunner>>;
}

pub trait ChecksumClient: Send + Sync {
    fn CalculateChecksum(
        &self,
        ctx: &Context,
        table: &CreatedTable,
        concurrency: u32,
    ) -> Result<ChecksumItem>;
}

pub trait PdController: Send + Sync {
    fn ResetTS(&self, ctx: &Context, ts: u64) -> Result<()>;
}

/// `PdClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait PdClient: Send + Sync {
    /// `GetClusterID`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetClusterID(&self, _ctx: &Context) -> u64 {
        1
    }

    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(Vec::new())
    }

    /// `GetTS`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        Ok((1, 1))
    }

    fn SupportsKeyspaceBR(&self, _ctx: &Context) -> Result<bool> {
        Ok(false)
    }
}

/// `PdHttpClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait PdHttpClient: Send + Sync {}

/// `StoreMeta`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait StoreMeta: Send + Sync {
    /// `GetAllStores`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>>;
}

/// `SplitClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait SplitClient: Send + Sync {
    /// `GetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        _group_id: &str,
        _rule_id: &str,
    ) -> Result<PlacementRule>;
    /// `SetPlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetPlacementRule(&self, _ctx: &Context, _rule: &PlacementRule) -> Result<()>;
    /// `DeletePlacementRule`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn DeletePlacementRule(&self, _ctx: &Context, _group_id: &str, _rule_id: &str) -> Result<()>;
    /// `ScanRegions`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn ScanRegions(
        &self,
        _ctx: &Context,
        _start: &[u8],
        _end: &[u8],
        _limit: i32,
    ) -> Result<Vec<RegionInfo>>;
    /// `PaginateScanRegion`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn PaginateScanRegion(
        &self,
        ctx: &Context,
        start: &[u8],
        end: &[u8],
    ) -> Result<Vec<RegionInfo>> {
        self.ScanRegions(ctx, start, end, -1)
    }
}

#[derive(Clone, Debug, Default)]
/// `PlacementRule`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PlacementRule {
    pub GroupID: String,
    pub ID: String,
    pub Index: i32,
    pub Override: bool,
    pub StartKeyHex: String,
    pub EndKeyHex: String,
    pub LabelConstraints: Vec<LabelConstraint>,
}

#[derive(Clone, Debug, Default)]
/// `LabelConstraint`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct LabelConstraint {
    pub Key: String,
    pub Op: String,
    pub Values: Vec<String>,
}

/// `ImporterClient`：抽象依赖边界，便于注入 Mem* 桩或真实客户端。
pub trait ImporterClient: Send + Sync {
    fn DownloadSST(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        Err(Error::new("DownloadSST is not implemented"))
    }
    fn BatchDownloadSST(
        &self,
        ctx: &Context,
        store_id: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        self.DownloadSST(ctx, store_id, req)
    }
    fn BatchDownloadLatestMVCC(
        &self,
        ctx: &Context,
        store_id: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        self.BatchDownloadSST(ctx, store_id, req)
    }
    fn MultiIngest(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse> {
        Err(Error::new("MultiIngest is not implemented"))
    }
    /// `SetDownloadSpeedLimit`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn SetDownloadSpeedLimit(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<()>;
    /// `CheckMultiIngestSupport`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CheckMultiIngestSupport(&self, _ctx: &Context, _stores: &[u64]) -> Result<()>;
    /// `CheckBatchDownloadSupport`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CheckBatchDownloadSupport(&self, _ctx: &Context, _stores: &[u64]) -> Result<bool>;
    /// `CheckBatchDownloadLatestMVCCSupport`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CheckBatchDownloadLatestMVCCSupport(&self, _ctx: &Context, _stores: &[u64]) -> Result<()>;
    fn IsBatchDownloadLatestMVCCSupported(&self, ctx: &Context, stores: &[u64]) -> Result<bool> {
        for &id in stores {
            match self.BatchDownloadLatestMVCC(ctx, id, &import_sstpb::DownloadRequest::default()) {
                Ok(_) => {}
                Err(err) if err.code == Some("Unimplemented") => return Ok(false),
                Err(err) => {
                    return Err(Error::Annotatef(
                        err,
                        format!("failed to check BatchDownloadLatestMVCC support. (store id {id})"),
                    ));
                }
            }
        }
        Ok(true)
    }
    /// `AddForcePartitionRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn AddForcePartitionRange(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<()>;
    /// `RemoveForcePartitionRange`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn RemoveForcePartitionRange(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<()>;
    /// `CloseGrpcClient`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    fn CloseGrpcClient(&self) -> Result<()>;
}

pub trait SstRestorer: Send {
    fn Close(&mut self) -> Result<()>;
    fn GoRestore(
        &mut self,
        _on_progress: &dyn Fn(i64),
        _groups: &[BatchBackupFileSet],
    ) -> Result<()>;
    fn WaitUntilFinish(&mut self) -> Result<()>;
}

pub trait ExternalStorage: Send + Sync {
    fn WriteFile(&self, _ctx: &Context, path: &str, data: &[u8]) -> Result<()>;
    fn ReadFile(&self, _ctx: &Context, path: &str) -> Result<Vec<u8>>;
    fn FileExists(&self, _ctx: &Context, path: &str) -> Result<bool>;
    fn WalkDir(&self, _ctx: &Context, prefix: &str) -> Result<Vec<String>>;
    fn URI(&self) -> String {
        "mem://".into()
    }
}

/// Copier boundary matching Go `storeapi.Copier`.
pub trait Copier: ExternalStorage {
    fn CopyFrom(
        &self,
        _ctx: &Context,
        from: &dyn ExternalStorage,
        from_path: &str,
        to_path: &str,
    ) -> Result<()>;
}

#[derive(Default)]
pub struct MemStorage {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemStorage {
    pub fn seed(&self, path: &str, data: &[u8]) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
    }
}

impl ExternalStorage for MemStorage {
    fn WriteFile(&self, _ctx: &Context, path: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }

    fn ReadFile(&self, _ctx: &Context, path: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {path}")))
    }

    fn FileExists(&self, _ctx: &Context, path: &str) -> Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(path))
    }

    fn WalkDir(&self, _ctx: &Context, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

impl Copier for MemStorage {
    fn CopyFrom(
        &self,
        ctx: &Context,
        from: &dyn ExternalStorage,
        from_path: &str,
        to_path: &str,
    ) -> Result<()> {
        let data = from.ReadFile(ctx, from_path)?;
        self.WriteFile(ctx, to_path, &data)
    }
}

pub fn bytes_to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub const DefaultRegionIndexStep: u32 = 128;

pub fn NormalizeRegionIndexStep(step: u32) -> u32 {
    if step == 0 {
        DefaultRegionIndexStep
    } else {
        step
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreallocIDs {
    pub Start: i64,
    pub ReusableBorder: i64,
    pub End: i64,
    pub Hash: Vec<u8>,
    pub AllocRule: HashMap<i64, i64>,
}

impl PreallocIDs {
    pub fn CreateCheckpoint(&self) -> Option<Self> {
        if self.Start >= self.End {
            return None;
        }
        Some(Self {
            Start: self.Start,
            ReusableBorder: self.ReusableBorder,
            End: self.End,
            Hash: self.Hash.clone(),
            AllocRule: HashMap::new(),
        })
    }

    pub fn RemainedIDRange(&self) -> Result<[i64; 2]> {
        if self.End < self.Start {
            return Err(Error::new("invalid prealloc id range"));
        }
        Ok([self.Start, self.End])
    }

    pub fn GetIDRange(&self) -> (i64, i64) {
        (self.Start, self.End)
    }

    pub fn AllocID(&self, upstream: i64) -> Result<i64> {
        let rewrite_id = self.AllocRule.get(&upstream).copied().unwrap_or(0);
        if rewrite_id < self.Start || rewrite_id >= self.End {
            return Err(Error::new(format!(
                "table ID {rewrite_id} is not in range [{}, {})",
                self.Start, self.End
            )));
        }
        Ok(rewrite_id)
    }
}

pub const InsaneTableIDThreshold: i64 = u32::MAX as i64;

pub fn CollectTableIDs(tables: &[metautil::Table]) -> Result<(i64, Vec<i64>)> {
    let mut max_id = 0_i64;
    let mut ids = Vec::with_capacity(tables.len());
    for table in tables {
        if table.Info.ID > max_id && table.Info.ID < InsaneTableIDThreshold {
            max_id = table.Info.ID;
        }
        ids.push(table.Info.ID);
        if let Some(partition) = &table.Info.Partition {
            for definition in &partition.Definitions {
                if definition.ID > max_id && definition.ID < InsaneTableIDThreshold {
                    max_id = definition.ID;
                }
                ids.push(definition.ID);
            }
        }
    }
    if max_id + ids.len() as i64 + 1 > InsaneTableIDThreshold {
        return Err(Error::new(format!("table ID {max_id} is too large")));
    }
    ids.sort_unstable();
    Ok((max_id, ids))
}

pub fn ComputeSortedIDsHash(ids: &[i64]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    for id in ids {
        hasher.update(id.to_be_bytes());
    }
    hasher.finalize().to_vec()
}

pub fn NewAndPreallocTableIDs(
    tables: &[metautil::Table],
    allocator: &mut dyn DbSession,
) -> Result<PreallocIDs> {
    if tables.is_empty() {
        return Ok(PreallocIDs {
            Start: i64::MAX,
            ..Default::default()
        });
    }
    let (max_id, ids) = CollectTableIDs(tables)?;
    let start = allocator.GetGlobalID()? + 1;
    let reusable_border = (max_id + 1).max(start);
    let mut alloc_rule = HashMap::with_capacity(ids.len());
    let mut rewrite_count = 0_i64;
    for id in &ids {
        if *id >= start && *id < InsaneTableIDThreshold {
            alloc_rule.insert(*id, *id);
        } else {
            alloc_rule.insert(*id, reusable_border + rewrite_count);
            rewrite_count += 1;
        }
    }
    let id_range = reusable_border - start + rewrite_count;
    allocator.AdvanceGlobalIDs(id_range as usize)?;
    Ok(PreallocIDs {
        Start: start,
        ReusableBorder: reusable_border,
        End: start + id_range,
        Hash: ComputeSortedIDsHash(&ids),
        AllocRule: alloc_rule,
    })
}

pub fn ReusePreallocatedTableIDs(
    legacy: &PreallocIDs,
    tables: &[metautil::Table],
) -> Result<PreallocIDs> {
    let (max_id, ids) = CollectTableIDs(tables)?;
    if legacy.ReusableBorder < max_id + 1 {
        return Err(Error::new(format!(
            "prealloc IDs reusable border {} does not match with the tables max ID {}",
            legacy.ReusableBorder,
            max_id + 1
        )));
    }
    if legacy.Hash != ComputeSortedIDsHash(&ids) {
        return Err(Error::new("prealloc IDs hash mismatch"));
    }
    let mut alloc_rule = HashMap::with_capacity(ids.len());
    let mut rewrite_count = 0_i64;
    for id in ids {
        if id < legacy.Start || id > InsaneTableIDThreshold {
            alloc_rule.insert(id, legacy.ReusableBorder + rewrite_count);
            rewrite_count += 1;
        } else if id < legacy.ReusableBorder {
            alloc_rule.insert(id, id);
        } else {
            return Err(Error::new(format!(
                "table ID {id} is out of range [{}, {})",
                legacy.Start, legacy.ReusableBorder
            )));
        }
    }
    Ok(PreallocIDs {
        Start: legacy.Start,
        ReusableBorder: legacy.ReusableBorder,
        End: legacy.End,
        Hash: legacy.Hash.clone(),
        AllocRule: alloc_rule,
    })
}

#[derive(Clone)]
pub struct MemPdClient {
    pub cluster_id: u64,
    pub stores: Vec<metapb::Store>,
}

impl PdClient for MemPdClient {
    fn GetClusterID(&self, _ctx: &Context) -> u64 {
        self.cluster_id
    }

    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.clone())
    }
}

impl StoreMeta for MemPdClient {
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.clone())
    }
}

#[derive(Default)]
pub struct MemSplitClient {
    pub rules: Mutex<HashMap<(String, String), PlacementRule>>,
    pub regions: Mutex<Vec<RegionInfo>>,
}

impl SplitClient for MemSplitClient {
    fn GetPlacementRule(
        &self,
        _ctx: &Context,
        group_id: &str,
        rule_id: &str,
    ) -> Result<PlacementRule> {
        Ok(self
            .rules
            .lock()
            .unwrap()
            .get(&(group_id.to_string(), rule_id.to_string()))
            .cloned()
            .unwrap_or(PlacementRule {
                GroupID: group_id.to_string(),
                ID: rule_id.to_string(),
                ..Default::default()
            }))
    }

    fn SetPlacementRule(&self, _ctx: &Context, rule: &PlacementRule) -> Result<()> {
        self.rules
            .lock()
            .unwrap()
            .insert((rule.GroupID.clone(), rule.ID.clone()), rule.clone());
        Ok(())
    }

    fn DeletePlacementRule(&self, _ctx: &Context, group_id: &str, rule_id: &str) -> Result<()> {
        self.rules
            .lock()
            .unwrap()
            .remove(&(group_id.to_string(), rule_id.to_string()));
        Ok(())
    }

    fn ScanRegions(
        &self,
        _ctx: &Context,
        start: &[u8],
        end: &[u8],
        _limit: i32,
    ) -> Result<Vec<RegionInfo>> {
        let regions = self.regions.lock().unwrap();
        Ok(regions
            .iter()
            .filter(|r| {
                (end.is_empty() || r.Region.StartKey.as_slice() < end)
                    && (r.Region.EndKey.is_empty() || r.Region.EndKey.as_slice() > start)
            })
            .cloned()
            .collect())
    }
}

#[derive(Default)]
pub struct MemImporterClient {
    pub speed_limits: Mutex<HashMap<u64, import_sstpb::SetDownloadSpeedLimitRequest>>,
    pub unimplemented: bool,
    pub batch_download_supported: bool,
    pub batch_download_checks: Mutex<Vec<Vec<u64>>>,
    pub downloads: Mutex<Vec<(u64, import_sstpb::DownloadRequest)>>,
    pub ingests: Mutex<Vec<(u64, import_sstpb::MultiIngestRequest)>>,
}

impl ImporterClient for MemImporterClient {
    fn DownloadSST(
        &self,
        _ctx: &Context,
        store_id: u64,
        req: &import_sstpb::DownloadRequest,
    ) -> Result<import_sstpb::DownloadResponse> {
        self.downloads.lock().unwrap().push((store_id, req.clone()));
        Ok(import_sstpb::DownloadResponse {
            Range: req.Sst.Range.clone(),
            ..Default::default()
        })
    }

    fn MultiIngest(
        &self,
        _ctx: &Context,
        store_id: u64,
        req: &import_sstpb::MultiIngestRequest,
    ) -> Result<import_sstpb::IngestResponse> {
        self.ingests.lock().unwrap().push((store_id, req.clone()));
        Ok(import_sstpb::IngestResponse::default())
    }
    fn SetDownloadSpeedLimit(
        &self,
        _ctx: &Context,
        store_id: u64,
        req: &import_sstpb::SetDownloadSpeedLimitRequest,
    ) -> Result<()> {
        self.speed_limits
            .lock()
            .unwrap()
            .insert(store_id, req.clone());
        Ok(())
    }

    fn CheckMultiIngestSupport(&self, _ctx: &Context, _stores: &[u64]) -> Result<()> {
        Ok(())
    }

    fn CheckBatchDownloadSupport(&self, _ctx: &Context, stores: &[u64]) -> Result<bool> {
        self.batch_download_checks
            .lock()
            .unwrap()
            .push(stores.to_vec());
        Ok(self.batch_download_supported)
    }

    fn CheckBatchDownloadLatestMVCCSupport(&self, _ctx: &Context, _stores: &[u64]) -> Result<()> {
        Ok(())
    }

    fn AddForcePartitionRange(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::AddPartitionRangeRequest,
    ) -> Result<()> {
        if self.unimplemented {
            return Err(Error::with_code("Unimplemented", "unimplemented"));
        }
        Ok(())
    }

    fn RemoveForcePartitionRange(
        &self,
        _ctx: &Context,
        _store_id: u64,
        _req: &import_sstpb::RemovePartitionRangeRequest,
    ) -> Result<()> {
        if self.unimplemented {
            return Err(Error::with_code("Unimplemented", "unimplemented"));
        }
        Ok(())
    }

    fn CloseGrpcClient(&self) -> Result<()> {
        Ok(())
    }
}

pub struct SimpleRestorer {
    pub closed: bool,
    pub restored: Vec<BatchBackupFileSet>,
}

impl SimpleRestorer {
    pub fn new() -> Self {
        Self {
            closed: false,
            restored: Vec::new(),
        }
    }
}

impl SstRestorer for SimpleRestorer {
    fn Close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }

    fn GoRestore(
        &mut self,
        on_progress: &dyn Fn(i64),
        groups: &[BatchBackupFileSet],
    ) -> Result<()> {
        for g in groups {
            self.restored.push(g.clone());
            on_progress(1);
        }
        Ok(())
    }

    fn WaitUntilFinish(&mut self) -> Result<()> {
        Ok(())
    }
}

pub fn GetAllTiKVStoresWithRetry(ctx: &Context, pd: &dyn StoreMeta) -> Result<Vec<metapb::Store>> {
    let stores = pd.GetAllStores(ctx)?;
    Ok(stores
        .into_iter()
        .filter(|s| {
            !s.Labels
                .iter()
                .any(|l| l.Key == "engine" && l.Value == "tiflash")
        })
        .collect())
}

/// Token channel used by store worker pools (bounded permit queue).
pub type TokenCh = Arc<Mutex<VecDeque<()>>>;

pub fn token_len(ch: &TokenCh) -> usize {
    ch.lock().unwrap().len()
}

pub fn acquire_token(ch: &TokenCh) {
    loop {
        let mut q = ch.lock().unwrap();
        if q.pop_front().is_some() {
            return;
        }
        drop(q);
        std::thread::yield_now();
    }
}

pub fn try_acquire_token(ch: &TokenCh) -> bool {
    ch.lock().unwrap().pop_front().is_some()
}

pub fn release_token(ch: &TokenCh) {
    ch.lock().unwrap().push_back(());
}

pub type HashSetMap = HashSet<String>;
