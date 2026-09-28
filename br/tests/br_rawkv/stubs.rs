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

//! Local stand-ins for PD/TiKV rawkv / config / context / crc64 / logging
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 本文件为 `br/tests/br_rawkv` 提供本地桩：内存 RawKV、简易 Context、
//! TLS Security、线程安全 LCG、CRC64-ECMA、hex/日志助手。
//! 不连接真实 PD/TiKV/grpc；按 PD 地址注册表共享同一底层 store。
//! Scan 使用 BTreeMap 半开 range，与 TiKV raw scan 上界语义一致。
//! DeleteRange 先收集键列表再删除，避免迭代器失效。
//! Clone Client 共享底层 store，便于多句柄并发注入错误。
//! Security 字段原样保留，供创建参数回显断言。
//! hex 编码固定小写，与 Go encoding/hex.EncodeToString 默认一致。
//! 测试可注入 NewClient/Put/Scan/Delete 失败，验证客户端错误路径。

use std::cmp::Ordering as CmpOrdering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 本测试包统一 Result，错误仅承载消息字符串。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误：无错误码分类，Annotatef 仅前缀上下文。
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 透传错误，对齐 Go errors.Trace 的无包装路径。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 格式化为新错误（本桩忽略格式参数展开）。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }

    /// 将上下文拼到消息前，便于断言错误链路。
    pub fn Annotatef(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// 空 Context：RawKV 测试不需要取消/超时语义。
// --- context ---

#[derive(Clone, Default)]
pub struct Context;

impl Context {
    /// 背景上下文占位，与 Go context.Background 同名。
    pub fn Background() -> Self {
        Self
    }

    /// 未决定上下文占位，与 Go context.TODO 同名。
    pub fn TODO() -> Self {
        Self
    }
}

// TLS 路径字段；本桩不真正加载证书，只保存字符串供断言。
// --- config.Security ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Security {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

// 进程级 LCG：可 Seed，保证并发测试可复现随机键。
// --- Go math/rand stand-in (thread-safe LCG) ---

/// 懒初始化全局 RNG 状态；种子强制奇数避免退化周期。
fn rng_state() -> &'static Mutex<u64> {
    static RNG: OnceLock<Mutex<u64>> = OnceLock::new();
    RNG.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        // 强制奇数种子，降低 LCG 短周期风险。
        Mutex::new(seed | 1)
    })
}

/// Matches Go `rand.Intn(n)` for n > 0 (not bit-identical to math/rand).
/// 对齐 Go `rand.Intn` 语义（非位级相同），n 必须 >0。
pub fn Intn(n: i32) -> i32 {
    assert!(n > 0);
    let mut s = rng_state().lock().unwrap();
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 16) as i32).rem_euclid(n)
}

/// Test helper: reseed the process-local RNG.
/// 测试辅助：重设进程内 RNG，便于确定性用例。
pub fn Seed(seed: i64) {
    *rng_state().lock().unwrap() = seed as u64 | 1;
}

// ECMA 多项式表驱动实现，供 rawkv 客户端校验数据完整性。
// --- crc64 ECMA (Go hash/crc64) ---

/// Go `crc64.ECMA` 多项式常量。
const ECMA_POLY: u64 = 0xC96C5795D7870F42;

/// 惰性构建 256 项查表，避免每次 Write 重建。
fn ecma_table() -> &'static [u64; 256] {
    static TABLE: OnceLock<[u64; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u64; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut crc = i as u64;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ ECMA_POLY;
                } else {
                    crc >>= 1;
                }
            }
            *slot = crc;
        }
        table
    })
}

/// Go `crc64.New(crc64.MakeTable(crc64.ECMA))`.
#[derive(Clone)]
/// 流式 CRC64 摘要；初值 0，与 Go hash/crc64 默认一致。
pub struct Crc64Digest {
    crc: u64,
}

impl Crc64Digest {
    /// 构造空摘要。
    pub fn new_ecma() -> Self {
        Self { crc: 0 }
    }

    /// 吸收字节；对齐 Go `crc64.update` 的写入前后取反语义。
    pub fn Write(&mut self, p: &[u8]) {
        let tab = ecma_table();
        let mut crc = !self.crc;
        for &b in p {
            // 标准表格驱动更新：低 8 位索引。
            crc = tab[((crc as u8) ^ b) as usize] ^ (crc >> 8);
        }
        self.crc = !crc;
    }

    /// 返回当前 64 位校验和（不重置状态）。
    pub fn Sum64(&self) -> u64 {
        self.crc
    }
}

// 小写 hex 编解码；奇数长度/非法半字节返回 encoding/hex 风格错误。
// --- hex helpers ---

/// 字节 → 小写十六进制字符串。
pub fn encode_to_string(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &byte in b {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

/// 十六进制字符串 → 字节；与 Go `hex.DecodeString` 一样不隐式去除空白。
pub fn decode_string(s: &str) -> Result<Vec<u8>> {
    // 奇数长度非法，对齐 encoding/hex。
    if s.len() % 2 != 0 {
        return Err(Error::new(format!("encoding/hex: odd length hex string")));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

/// 单半字节解析；非法字符返回明确错误。
fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(Error::new(format!(
            "encoding/hex: invalid byte: {}",
            b as char
        ))),
    }
}

// 打到 stderr；panic 变体用于不可恢复测试失败。
// --- logging ---

/// 信息级日志：无字段时只打消息。
pub fn log_info(msg: &str, fields: &[(&str, String)]) {
    let mut parts = Vec::new();
    for (k, v) in fields {
        parts.push(format!("{k}={v}"));
    }
    if parts.is_empty() {
        eprintln!("[INFO] {msg}");
    } else {
        eprintln!("[INFO] {msg} {}", parts.join(" "));
    }
}

/// 错误级日志：字段以 k=v 空格拼接。
pub fn log_error(msg: &str, fields: &[(&str, String)]) {
    let mut parts = Vec::new();
    for (k, v) in fields {
        parts.push(format!("{k}={v}"));
    }
    if parts.is_empty() {
        eprintln!("[ERROR] {msg}");
    } else {
        eprintln!("[ERROR] {msg} {}", parts.join(" "));
    }
}

/// 致命日志后 panic，永不返回。
pub fn log_panic(msg: &str, fields: &[(&str, String)]) -> ! {
    let mut parts = Vec::new();
    for (k, v) in fields {
        parts.push(format!("{k}={v}"));
    }
    if parts.is_empty() {
        panic!("[PANIC] {msg}");
    } else {
        panic!("[PANIC] {msg} {}", parts.join(" "));
    }
}

// 内存 RawKV：按首个 PD 地址共享 store；可注入各 API 错误。
// --- in-memory rawkv Client (PD/TiKV boundary) ---

/// 客户端实例自增 ID，便于调试区分 clone。
static STORE_ID: AtomicU64 = AtomicU64::new(1);

type KvMap = BTreeMap<Vec<u8>, Vec<u8>>;

#[derive(Clone, Default)]
/// 共享存储与可注入故障槽；连接关闭状态由各 Client 句柄持有。
struct SharedStore {
    data: Arc<Mutex<KvMap>>,
    put_err: Arc<Mutex<Option<Error>>>,
    scan_err: Arc<Mutex<Option<Error>>>,
    delete_err: Arc<Mutex<Option<Error>>>,
    new_client_err: Arc<Mutex<Option<Error>>>,
}

/// 全局地址→store 注册表；同地址 NewClient 复用数据面。
fn registry() -> &'static Mutex<BTreeMap<String, SharedStore>> {
    static REG: OnceLock<Mutex<BTreeMap<String, SharedStore>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Test helper: install/clear dialer failure for a PD address.
/// 测试辅助：为某 PD 地址安装/清除拨号失败。
pub fn set_new_client_error(addr: &str, err: Option<Error>) {
    let mut reg = registry().lock().unwrap();
    let store = reg.entry(addr.to_string()).or_default();
    *store.new_client_err.lock().unwrap() = err;
}

/// Test helper: install Put failure.
/// 测试辅助：安装 Put 失败。
pub fn set_put_error(client: &Client, err: Option<Error>) {
    *client.store.put_err.lock().unwrap() = err;
}

/// Test helper: install Scan failure.
/// 测试辅助：安装 Scan 失败。
pub fn set_scan_error(client: &Client, err: Option<Error>) {
    *client.store.scan_err.lock().unwrap() = err;
}

/// Test helper: install DeleteRange failure.
/// 测试辅助：安装 DeleteRange 失败。
pub fn set_delete_error(client: &Client, err: Option<Error>) {
    *client.store.delete_err.lock().unwrap() = err;
}

/// Test helper: snapshot of stored KVs.
/// 测试辅助：快照当前全部 KV。
pub fn store_snapshot(client: &Client) -> Vec<(Vec<u8>, Vec<u8>)> {
    client
        .store
        .data
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Test helper: clear all KVs for a client.
/// 测试辅助：清空某客户端底层 KV。
pub fn clear_store(client: &Client) {
    client.store.data.lock().unwrap().clear();
}

/// Test helper: put a KV directly into the store.
/// 测试辅助：绕过 Put API 直接写入，构造预置数据。
pub fn put_raw(client: &Client, key: Vec<u8>, value: Vec<u8>) {
    client.store.data.lock().unwrap().insert(key, value);
}

/// RawKV 客户端句柄：持 PD 地址、Security 与共享 store。
pub struct Client {
    pub pd_addrs: Vec<String>,
    pub security: Security,
    store: SharedStore,
    closed: Arc<AtomicBool>,
    id: u64,
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            pd_addrs: self.pd_addrs.clone(),
            security: self.security.clone(),
            store: self.store.clone(),
            closed: self.closed.clone(),
            id: self.id,
        }
    }
}

/// Go `rawkv.NewClient(ctx, pdAddrs, security)`.
/// 对齐 Go `rawkv.NewClient`；按首地址取/建 store。
pub fn NewClient(pd_addrs: Vec<String>, security: Security) -> Result<Client> {
    // 空地址列表用空串作注册键。
    let key = pd_addrs.first().cloned().unwrap_or_default();
    let mut reg = registry().lock().unwrap();
    let store = reg.entry(key).or_default().clone();
    // 拨号失败注入：不创建 Client。
    if let Some(err) = store.new_client_err.lock().unwrap().clone() {
        return Err(err);
    }
    Ok(Client {
        pd_addrs,
        security,
        store,
        closed: Arc::new(AtomicBool::new(false)),
        id: STORE_ID.fetch_add(1, Ordering::SeqCst),
    })
}

impl Client {
    /// Go `client.Put(ctx, key, value)`.
    /// 写入 KV；closed 或注入错误时失败。
    pub fn Put(&self, _ctx: &Context, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        // 关闭后拒绝写入，模拟连接失效。
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::new("client closed"));
        }
        // 优先返回注入错误，覆盖真实写入。
        if let Some(err) = self.store.put_err.lock().unwrap().clone() {
            return Err(err);
        }
        self.store.data.lock().unwrap().insert(key, value);
        Ok(())
    }

    /// Go `client.Scan(ctx, startKey, endKey, limit)`.
    /// 半开区间扫描；limit<0 视为 0 条。
    pub fn Scan(
        &self,
        _ctx: &Context,
        start: &[u8],
        end: &[u8],
        limit: i32,
    ) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
        if let Some(err) = self.store.scan_err.lock().unwrap().clone() {
            return Err(err);
        }
        let data = self.store.data.lock().unwrap();
        let mut keys = Vec::new();
        let mut values = Vec::new();
        // 负 limit 收敛为 0，避免 usize 转换陷阱。
        let lim = if limit < 0 { 0 } else { limit as usize };
        for (k, v) in data.range(start.to_vec()..end.to_vec()) {
            if keys.len() >= lim {
                break;
            }
            keys.push(k.clone());
            values.push(v.clone());
        }
        Ok((keys, values))
    }

    /// Go `client.DeleteRange(ctx, start, end)`.
    /// 删除 [start,end) 内全部键。
    pub fn DeleteRange(&self, _ctx: &Context, start: &[u8], end: &[u8]) -> Result<()> {
        if let Some(err) = self.store.delete_err.lock().unwrap().clone() {
            return Err(err);
        }
        let mut data = self.store.data.lock().unwrap();
        // 先收集键再删，避免边遍历边修改。
        let keys: Vec<Vec<u8>> = data
            .range(start.to_vec()..end.to_vec())
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            data.remove(&k);
        }
        Ok(())
    }

    /// 标记当前连接句柄关闭；后续 Put 报 client closed。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// 字节字典序：-1/0/1，对齐 Go bytes.Compare。
pub fn bytes_compare(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        CmpOrdering::Less => -1,
        CmpOrdering::Equal => 0,
        CmpOrdering::Greater => 1,
    }
}
