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

//! Local stand-ins for Storage / cipher / session / domain boundaries (darwin-safe).
//!
//! checkpoint 包的本地桩：用内存实现替代真实对象存储、TiDB Session/Domain、
//! PD timer 与 failpoint，便于在无 TiKV/PD 环境下做单元与对等测试。
//!
//! 边界说明（勿把桩当成生产能力）：
//! - `MemStorage` 仅内存 HashMap，无持久化、无权限、无重试语义；
//! - `Session`/`Domain`/`Glue`/`InfoSchema` 是最小 trait，真实 SQL 语义未实现；
//! - `Encrypt`/`Decrypt` 实现 AES-CTR 子集，供检查点文件加解密测试；
//! - `failpoint` 模块只识别一条刷盘失败注入点，其它名返回错误。
//! 对应 Go 侧分散在 glue/storage/crypto 的依赖，在此收拢为可注入接口。

use std::collections::HashMap;

// 下列依赖仅用于 AES-CTR 与序列化；不含真实 TiDB/PD 客户端。
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aes::cipher::{KeyIvInit, StreamCipher};
use aes::{Aes128, Aes192, Aes256};
use ctr::Ctr128BE;
use rand::RngCore;
use serde::{Deserialize, Serialize};

/// 本包统一 Result；错误类型为简易 `Error`，非 pingcap/errors 全功能包装。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误：仅字符串消息 + Annotate 前缀，满足测试断言与传播。
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 构造错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 对齐 Go `errors.Errorf` 命名的别名。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    /// 在消息前附加上下文：`ctx: old`。
    pub fn Annotate(self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), self.msg),
        }
    }

    /// 同 Annotate，保留 Go `Annotatef` 调用点。
    pub fn Annotatef(self, ctx: impl Into<String>) -> Self {
        self.Annotate(ctx)
    }

    /// 桩实现：不增加栈，原样返回（Go Trace 的占位）。
    pub fn Trace(err: Self) -> Self {
        err
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Error::new(value.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Error::new(value.to_string())
    }
}

/// 近似 Go `context.Context` 的取消令牌：仅支持 cancel/Err/Done，无 deadline/value。
/// Cancellation token approximating Go context.Context for cancel checks.
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    /// 未取消的根上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 写入取消原因；之后 `Done` 为 true。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// 若已取消返回原因副本。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }

    /// 是否已取消。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

/// AES-CTR IV 长度，与 Go crypter 一致。
pub const CrypterIvLen: usize = 16;
/// TiDB TS 物理部分左移位数。
pub const PhysicalShiftBits: i64 = 18;

/// 合成混合时间戳：`(physical << 18) | logical`。
pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
    ((physical as u64) << (PhysicalShiftBits as u64)) | (logical as u64)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
/// 加密算法枚举；`UNKNOWN` 在加解密时拒绝。
pub enum EncryptionMethod {
    #[default]
    UNKNOWN = 0,
    PLAINTEXT = 1,
    AES128_CTR = 2,
    AES192_CTR = 3,
    AES256_CTR = 4,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 加密参数：算法 + 密钥字节；密钥长度须匹配 AES128/192/256。
pub struct CipherInfo {
    pub CipherType: EncryptionMethod,
    pub CipherKey: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 备份文件元数据片段（名与起止 key），JSON 字段名对齐 Go。
pub struct File {
    #[serde(default, rename = "name", skip_serializing_if = "String::is_empty")]
    pub Name: String,
    #[serde(default, rename = "start_key", skip_serializing_if = "Vec::is_empty")]
    pub StartKey: Vec<u8>,
    #[serde(default, rename = "end_key", skip_serializing_if = "Vec::is_empty")]
    pub EndKey: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 键范围；字段名大写以匹配既有序列化。
pub struct KeyRange {
    #[serde(default, rename = "StartKey")]
    pub StartKey: Vec<u8>,
    #[serde(default, rename = "EndKey")]
    pub EndKey: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 带文件列表的范围；`KeyRange` flatten 进同级 JSON。
pub struct Range {
    #[serde(flatten)]
    pub KeyRange: KeyRange,
    #[serde(default, rename = "Files", skip_serializing_if = "Vec::is_empty")]
    pub Files: Vec<File>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
/// 集群调度器配置快照，写入检查点 meta 以便恢复后还原。
pub struct ClusterConfig {
    #[serde(default)]
    pub Schedulers: Vec<String>,
    #[serde(default)]
    pub ScheduleCfg: HashMap<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 大小写不敏感标识：`O` 原始、`L` 小写，对齐 `ast.CIStr`。
pub struct CIStr {
    #[serde(default, rename = "O")]
    pub O: String,
    #[serde(default, rename = "L")]
    pub L: String,
}

impl CIStr {
    /// 由字符串构造，自动填 `L`。
    pub fn new(s: impl Into<String>) -> Self {
        let o = s.into();
        let l = o.to_lowercase();
        Self { O: o, L: l }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// TiFlash 副本计数桩，供日志恢复 meta 序列化。
pub struct TiFlashReplicaInfo {
    #[serde(default)]
    pub Count: u64,
}

#[derive(Clone, Debug, Default)]
/// `WalkDir` 选项；目前仅 `SubDir` 前缀过滤。
pub struct WalkOption {
    pub SubDir: String,
}

/// 对象存储最小接口。生产应注入真实实现；此处仅有 `MemStorage`。
pub trait Storage: Send + Sync {
    /// 遍历目录回调 `(path, size)`；错误应中止遍历。
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    /// 读取完整文件；不存在则错误。
    fn ReadFile(&self, _ctx: &Context, path: &str) -> Result<Vec<u8>>;
    /// 覆盖写入。
    fn WriteFile(&self, _ctx: &Context, path: &str, data: &[u8]) -> Result<()>;
    /// 删除路径（MemStorage 对缺失静默）。
    fn DeleteFile(&self, _ctx: &Context, path: &str) -> Result<()>;
    /// 是否存在。
    fn FileExists(&self, _ctx: &Context, path: &str) -> Result<bool>;
    /// 逻辑 URI，如 `mem://`。
    fn URI(&self) -> String;
}

#[derive(Default)]
/// 进程内文件映射；无持久化。
pub struct MemStorage {
    /// 路径 → 内容
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemStorage {
    /// 空存储。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回已排序路径列表，便于测试断言。
    pub fn paths(&self) -> Vec<String> {
        let mut v: Vec<_> = self.files.lock().unwrap().keys().cloned().collect();
        v.sort();
        v
    }
}

// WalkDir 按 SubDir 过滤；读写删直接操作 HashMap。
impl Storage for MemStorage {
    fn WalkDir(
        &self,
        _ctx: &Context,
        opt: &WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        let prefix = opt.SubDir.trim_matches('/');
        let entries: Vec<(String, i64)> = {
            let files = self.files.lock().unwrap();
            let mut paths: Vec<_> = files.keys().cloned().collect();
            paths.sort();
            paths
                .into_iter()
                .filter(|path| {
                    // 前缀为空则全部；否则匹配路径段
                    if prefix.is_empty() {
                        true
                    } else {
                        path == prefix
                            || path.starts_with(&format!("{prefix}/"))
                            || path.contains(&format!("/{prefix}/"))
                    }
                })
                .map(|path| {
                    let size = files.get(&path).map(|b| b.len() as i64).unwrap_or(0);
                    (path, size)
                })
                .collect()
        };
        for (path, size) in entries {
            f(&path, size)?;
        }
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

    fn WriteFile(&self, _ctx: &Context, path: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_string(), data.to_vec());
        Ok(())
    }

    fn DeleteFile(&self, _ctx: &Context, path: &str) -> Result<()> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn FileExists(&self, _ctx: &Context, path: &str) -> Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(path))
    }

    fn URI(&self) -> String {
        "mem://".into()
    }
}

/// 加密：空/无 cipher 透传；PLAINTEXT 不变换；AES-CTR 返回 `(密文, iv)`。
pub fn Encrypt(content: &[u8], cipher: Option<&CipherInfo>) -> Result<(Vec<u8>, Vec<u8>)> {
    // 空或无 cipher：透传明文与空 IV
    if content.is_empty() || cipher.is_none() {
        return Ok((content.to_vec(), Vec::new()));
    }
    let cipher = cipher.unwrap();
    match cipher.CipherType {
        // PLAINTEXT：不加密
        EncryptionMethod::PLAINTEXT => Ok((content.to_vec(), Vec::new())),
        EncryptionMethod::AES128_CTR
        | EncryptionMethod::AES192_CTR
        | EncryptionMethod::AES256_CTR => {
            // 随机 IV 后做 CTR
            let mut iv = vec![0u8; CrypterIvLen];
            rand::thread_rng().fill_bytes(&mut iv);
            let encrypted = aes_crypt_ctr(content, &cipher.CipherKey, &iv)?;
            Ok((encrypted, iv))
        }
        EncryptionMethod::UNKNOWN => {
            Err(Error::new("cipher type invalid").Annotate("invalid argument"))
        }
    }
}

/// 解密：CTR 对称；UNKNOWN 类型报错。
pub fn Decrypt(content: &[u8], cipher: Option<&CipherInfo>, iv: &[u8]) -> Result<Vec<u8>> {
    if content.is_empty() || cipher.is_none() {
        return Ok(content.to_vec());
    }
    let cipher = cipher.unwrap();
    match cipher.CipherType {
        EncryptionMethod::PLAINTEXT => Ok(content.to_vec()),
        EncryptionMethod::AES128_CTR
        | EncryptionMethod::AES192_CTR
        | EncryptionMethod::AES256_CTR => aes_crypt_ctr(content, &cipher.CipherKey, iv),
        EncryptionMethod::UNKNOWN => Err(Error::new(format!(
            "cipher type invalid {:?}",
            cipher.CipherType
        ))
        .Annotate("invalid argument")),
    }
}

/// 按密钥长度选择 AES128/192/256-CTR；IV 必须 16 字节。
fn aes_crypt_ctr(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    if iv.len() != CrypterIvLen {
        return Err(Error::new("invalid iv length"));
    }
    let mut out = data.to_vec();
    match key.len() {
        16 => {
            type Aes128Ctr = Ctr128BE<Aes128>;
            let mut cipher =
                Aes128Ctr::new_from_slices(key, iv).map_err(|e| Error::new(e.to_string()))?;
            cipher.apply_keystream(&mut out);
        }
        24 => {
            type Aes192Ctr = Ctr128BE<Aes192>;
            let mut cipher =
                Aes192Ctr::new_from_slices(key, iv).map_err(|e| Error::new(e.to_string()))?;
            cipher.apply_keystream(&mut out);
        }
        32 => {
            type Aes256Ctr = Ctr128BE<Aes256>;
            let mut cipher =
                Aes256Ctr::new_from_slices(key, iv).map_err(|e| Error::new(e.to_string()))?;
            cipher.apply_keystream(&mut out);
        }
        _ => return Err(Error::new("invalid aes key length")),
    }
    Ok(out)
}

// 进程级计时起点，供 NowDureTime 使用
static SUMMARY_START: Mutex<Option<Instant>> = Mutex::new(None);

/// 自首次调用起的经过时间，模拟 Go summary 计时起点。
pub fn NowDureTime() -> Duration {
    let mut guard = SUMMARY_START.lock().unwrap();
    // 首次调用固定起点
    if guard.is_none() {
        *guard = Some(Instant::now());
    }
    guard.unwrap().elapsed()
}

/// 有限次重试：上下文取消立即返回；耗尽返回最后一次错误。
pub fn WithRetry<F>(ctx: &Context, mut f: F, max_attempts: usize) -> Result<()>
where
    F: FnMut() -> Result<()>,
{
    let mut last = Error::new("retry exhausted");
    // 至少尝试一次；每次先查取消
    for _ in 0..max_attempts.max(1) {
        if let Some(err) = ctx.Err() {
            return Err(err);
        }
        match f() {
            Ok(()) => return Ok(()),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// PD TSO 桩接口；仅返回一对 physical/logical。
pub trait GlobalTimer: Send + Sync {
    fn GetTS(&self, ctx: &Context) -> Result<(i64, i64)>;
}

/// 固定返回预设 TS 的 timer。
pub struct MockTimer {
    /// 物理 TS
    pub p: i64,
    /// 逻辑 TS
    pub l: i64,
}

impl MockTimer {
    /// 指定固定 physical/logical。
    pub fn new(p: i64, l: i64) -> Self {
        Self { p, l }
    }
}

impl GlobalTimer for MockTimer {
    /// 忽略 ctx，恒返回构造时的 (p, l)。
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)> {
        Ok((self.p, self.l))
    }
}

/// 表检查点所需的最小 SQL Session；真实 SQL 语义未实现。
/// Minimal SQL session for table checkpoint storage.
pub trait Session: Send + Sync {
    /// 关闭会话资源（桩可仅置位）。
    fn Close(&mut self);
    /// 执行内部 SQL；占位符与真实 TiDB 方言未完全对齐。
    fn ExecuteInternal(&mut self, ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()>;
    /// 返回只读查询执行器。
    fn GetRestrictedSQLExecutor(&self) -> Arc<dyn RestrictedSQLExecutor>;
}

/// 受限 SQL 查询接口，对应 Go RestrictedSQLExecutor。
pub trait RestrictedSQLExecutor: Send + Sync {
    fn ExecRestrictedSQL(&self, ctx: &Context, sql: &str, args: &[SqlValue])
    -> Result<Vec<SqlRow>>;
}

/// BR glue：由 Domain Store 创建 Session。
pub trait Glue: Send + Sync {
    /// 由 Store 创建新 Session。
    fn CreateSession(&self, store: &dyn Storage) -> Result<Box<dyn Session>>;
}

/// Domain 桩：暴露 Store 与 InfoSchema。
pub trait Domain: Send + Sync {
    /// 返回底层 Storage 句柄。
    fn Store(&self) -> Arc<dyn Storage>;
    /// 返回 InfoSchema 视图。
    fn InfoSchema(&self) -> Arc<dyn InfoSchema>;
}

/// InfoSchema 子集：表存在性与库内表列表。
pub trait InfoSchema: Send + Sync {
    /// 库表是否存在。
    fn TableExists(&self, db: &CIStr, table: &CIStr) -> bool;
    /// 列出库内表；空列表表示可 DROP DATABASE。
    fn SchemaTableInfos(&self, ctx: &Context, db: &CIStr) -> Result<Vec<TableInfoName>>;
}

#[derive(Clone, Debug, Default)]
/// SchemaTableInfos 返回的表名条目。
pub struct TableInfoName {
    pub Name: CIStr,
}

#[derive(Clone, Debug)]
/// ExecuteInternal/查询参数与列值的简易枚举。
pub enum SqlValue {
    Bytes(Vec<u8>),
    U64(u64),
    I64(i64),
    Str(String),
}

#[derive(Clone, Debug, Default)]
/// 一行结果；`GetBytes`/`GetUint64` 做宽松类型转换。
pub struct SqlRow {
    cols: Vec<SqlValue>,
}

impl SqlRow {
    /// 由列值构造行。
    pub fn new(cols: Vec<SqlValue>) -> Self {
        Self { cols }
    }

    /// Bytes/Str 返回字节；其它类型返回空切片。
    pub fn GetBytes(&self, i: usize) -> &[u8] {
        match &self.cols[i] {
            SqlValue::Bytes(b) => b,
            SqlValue::Str(s) => s.as_bytes(),
            _ => &[],
        }
    }

    /// U64/I64 转换；其它为 0。
    pub fn GetUint64(&self, i: usize) -> u64 {
        match &self.cols[i] {
            SqlValue::U64(v) => *v,
            SqlValue::I64(v) => *v as u64,
            _ => 0,
        }
    }
}

/// Duration ↔ 纳秒 i64 的 serde 适配，对齐 Go 侧 ns 序列化。
pub mod duration_ns {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(d: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        (d.as_nanos() as i64).serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ns = i64::deserialize(deserializer)?;
        Ok(Duration::from_nanos(ns as u64))
    }
}

/// Go failpoint 极简桩：仅支持 `failed-after-checkpoint-flushes`。
/// Go `pingcap/failpoint` stand-ins used by checkpoint flush retry tests.
pub mod failpoint {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{Error, Result};

    // 与 Go failpoint 路径字符串保持一致
    const FAILED_AFTER_CHECKPOINT_FLUSHES: &str =
        "github.com/pingcap/tidb/br/pkg/checkpoint/failed-after-checkpoint-flushes";

    static FAILED_AFTER_FLUSHES: AtomicBool = AtomicBool::new(false);

    /// 打开命名 failpoint；表达式忽略。
    pub fn Enable(name: &str, _expr: &str) -> Result<()> {
        match name {
            FAILED_AFTER_CHECKPOINT_FLUSHES => {
                FAILED_AFTER_FLUSHES.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(Error::new(format!("unknown failpoint: {name}"))),
        }
    }

    /// 关闭命名 failpoint。
    pub fn Disable(name: &str) -> Result<()> {
        match name {
            FAILED_AFTER_CHECKPOINT_FLUSHES => {
                FAILED_AFTER_FLUSHES.store(false, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(Error::new(format!("unknown failpoint: {name}"))),
        }
    }

    /// 查询刷盘后失败注入是否开启。
    pub fn failed_after_checkpoint_flushes() -> bool {
        FAILED_AFTER_FLUSHES.load(Ordering::SeqCst)
    }
}
