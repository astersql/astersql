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

//! Local stand-ins for PD/TiKV txnkv / config / context / crc64 / logging
//! (darwin arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 本文件为 `br/tests/br_txn` 提供本地桩：内存事务 KV、Context、全局 Config、
//! concurrency 参数在 DeleteRange 中忽略：单测不模拟并行删除。
//! Clone Client 共享 store，多句柄看到同一提交数据。
//! Begin 不分配真实 startTS，仅提供缓冲语义。
//! Commit 后 buffer 清空且 committed=true，供调试观察。
//! Iter 不感知其他未提交事务（单测单线程模型）。
//! 全局 Config 与 Security 供 TLS 字段回显，不建真实连接。
//! LCG/CRC64/日志与 hex。不连接真实 PD/TiKV。
//! 事务模型：Begin 得 Transaction，Set 写缓冲，Commit 合并进共享 store；
//! Iter 合并已提交与缓冲（缓冲优先）。可注入 Begin/Commit/Set/Delete 错误。

use std::cmp::Ordering as CmpOrdering;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// 本测试包统一 Result。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误；Trace/Errorf 仅为 API 对齐。
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 透传错误。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 新建错误消息。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

// 可取消 Context；WithTimeout 返回 FnOnce 取消闭包。
// --- context (cancellation + timeout) ---

#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
    deadline: Arc<Mutex<Option<Instant>>>,
}

impl Context {
    /// 背景上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// TODO 上下文占位。
    pub fn TODO() -> Self {
        Self::default()
    }

    /// 派生超时上下文；取消闭包置位 cancelled。
    pub fn WithTimeout(parent: &Self, timeout: Duration) -> (Self, Box<dyn FnOnce() + Send>) {
        let ctx = Self {
            cancelled: Arc::new(AtomicBool::new(parent.cancelled.load(Ordering::SeqCst))),
            deadline: Arc::new(Mutex::new(Some(Instant::now() + timeout))),
        };
        let flag = ctx.cancelled.clone();
        let cancel = Box::new(move || {
            flag.store(true, Ordering::SeqCst);
        });
        (ctx, cancel)
    }

    /// 取消或超时到达。
    pub fn Done(&self) -> bool {
        if self.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        if let Some(dl) = *self.deadline.lock().unwrap() {
            if Instant::now() >= dl {
                self.cancelled.store(true, Ordering::SeqCst);
                return true;
            }
        }
        false
    }

    /// Done 时返回 canceled 错误。
    pub fn Err(&self) -> Option<Error> {
        if self.Done() {
            Some(Error::new("context canceled"))
        } else {
            None
        }
    }
}

// 进程级 Config，供测试切换 TLS 字段。
// --- config.Security / global config ---

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 集群 TLS 路径三件套。
pub struct Security {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 全局配置容器。
pub struct Config {
    pub Security: Security,
}

/// 懒初始化全局 Config 互斥量。
fn global_config() -> &'static Mutex<Config> {
    static CFG: OnceLock<Mutex<Config>> = OnceLock::new();
    CFG.get_or_init(|| Mutex::new(Config::default()))
}

/// 克隆当前全局配置。
pub fn GetGlobalConfig() -> Config {
    global_config().lock().unwrap().clone()
}

/// 覆盖全局配置。
pub fn StoreGlobalConfig(conf: Config) {
    *global_config().lock().unwrap() = conf;
}

// 线程安全 LCG，可 Seed。
// --- Go math/rand stand-in (thread-safe LCG) ---

/// 全局 RNG 状态。
fn rng_state() -> &'static Mutex<u64> {
    static RNG: OnceLock<Mutex<u64>> = OnceLock::new();
    RNG.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Mutex::new(seed | 1)
    })
}

/// Matches Go `rand.Intn(n)` for n > 0 (not bit-identical to math/rand).
/// 对齐 Go rand.Intn（非位级相同）。
pub fn Intn(n: i32) -> i32 {
    assert!(n > 0);
    let mut s = rng_state().lock().unwrap();
    // Numerical Recipes LCG 参数。
    // Numerical Recipes LCG
    *s = s.wrapping_mul(1664525).wrapping_add(1013904223);
    ((*s >> 16) as i32).rem_euclid(n)
}

/// Test helper: reseed the process-local RNG.
/// 重设 RNG 种子。
pub fn Seed(seed: i64) {
    *rng_state().lock().unwrap() = seed as u64 | 1;
}

// ECMA 表驱动 CRC64。
// --- crc64 ECMA (Go hash/crc64) ---

/// ECMA 多项式。
const ECMA_POLY: u64 = 0xC96C5795D7870F42;

/// 惰性 CRC 表。
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
/// 流式 CRC64 摘要。
pub struct Crc64Digest {
    crc: u64,
}

impl Crc64Digest {
    /// 空摘要。
    pub fn new_ecma() -> Self {
        Self { crc: 0 }
    }

    /// 吸收字节。
    pub fn Write(&mut self, p: &[u8]) {
        let tab = ecma_table();
        // Go hash/crc64.Update complements the running state before and after
        // processing each chunk. Keeping the complemented value only locally
        // also preserves streaming Write semantics.
        let mut crc = !self.crc;
        for &b in p {
            crc = tab[((crc as u8) ^ b) as usize] ^ (crc >> 8);
        }
        self.crc = !crc;
    }

    /// 当前校验和。
    pub fn Sum64(&self) -> u64 {
        self.crc
    }
}

// stderr 日志桩。
// --- logging (no-op / stderr) ---

/// 信息日志。
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

/// 致命日志后 panic。
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

/// 小写 hex 编码。
pub fn encode_to_string(b: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &byte in b {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

// 内存事务客户端：按 PD 首地址共享 store。
// --- in-memory txnkv Client (PD/TiKV boundary) ---

/// 客户端自增 ID。
static STORE_ID: AtomicU64 = AtomicU64::new(1);

type KvMap = BTreeMap<Vec<u8>, Vec<u8>>;

#[derive(Clone, Default)]
/// 共享数据与各 API 注入错误槽。
struct SharedStore {
    data: Arc<Mutex<KvMap>>,
    /// When set, Begin returns this error.
    begin_err: Arc<Mutex<Option<Error>>>,
    /// When set, Commit returns this error.
    commit_err: Arc<Mutex<Option<Error>>>,
    /// When set, Set returns this error.
    set_err: Arc<Mutex<Option<Error>>>,
    /// When set, Iter returns this error.
    iter_err: Arc<Mutex<Option<Error>>>,
    /// When set, iterator Next returns this error.
    next_err: Arc<Mutex<Option<Error>>>,
    /// When set, DeleteRange returns this error.
    delete_err: Arc<Mutex<Option<Error>>>,
    /// When set, NewClient returns this error.
    new_client_err: Arc<Mutex<Option<Error>>>,
    closed: Arc<AtomicBool>,
}

/// 地址→store 注册表。
fn registry() -> &'static Mutex<BTreeMap<String, SharedStore>> {
    static REG: OnceLock<Mutex<BTreeMap<String, SharedStore>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Test helper: install/clear dialer failure for a PD address.
/// 注入 NewClient 失败。
pub fn set_new_client_error(addr: &str, err: Option<Error>) {
    let mut reg = registry().lock().unwrap();
    let store = reg.entry(addr.to_string()).or_default();
    *store.new_client_err.lock().unwrap() = err;
}

/// Test helper: install Begin failure on an existing client store.
/// 注入 Begin 失败。
pub fn set_begin_error(client: &Client, err: Option<Error>) {
    *client.store.begin_err.lock().unwrap() = err;
}

/// Test helper: install Commit failure.
/// 注入 Commit 失败。
pub fn set_commit_error(client: &Client, err: Option<Error>) {
    *client.store.commit_err.lock().unwrap() = err;
}

/// Test helper: install Set failure.
/// 注入 Set 失败。
pub fn set_set_error(client: &Client, err: Option<Error>) {
    *client.store.set_err.lock().unwrap() = err;
}

/// Test helper: install Iter failure.
pub fn set_iter_error(client: &Client, err: Option<Error>) {
    *client.store.iter_err.lock().unwrap() = err;
}

/// Test helper: install iterator Next failure.
pub fn set_next_error(client: &Client, err: Option<Error>) {
    *client.store.next_err.lock().unwrap() = err;
}

/// Test helper: install DeleteRange failure.
/// 注入 DeleteRange 失败。
pub fn set_delete_error(client: &Client, err: Option<Error>) {
    *client.store.delete_err.lock().unwrap() = err;
}

/// Test helper: snapshot of stored KVs.
/// 快照已提交 KV。
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
/// 清空已提交 KV。
pub fn clear_store(client: &Client) {
    client.store.data.lock().unwrap().clear();
}

/// Test helper: put a KV directly into the store.
/// 直接写入已提交层，绕过事务。
pub fn put_raw(client: &Client, key: Vec<u8>, value: Vec<u8>) {
    client.store.data.lock().unwrap().insert(key, value);
}

/// txnkv 客户端句柄。
pub struct Client {
    pub pd_addrs: Vec<String>,
    store: SharedStore,
    id: u64,
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            pd_addrs: self.pd_addrs.clone(),
            store: self.store.clone(),
            id: self.id,
        }
    }
}

/// Go `txnkv.NewClient(pdAddrs)`.
/// 对齐 Go txnkv.NewClient。
pub fn NewClient(pd_addrs: Vec<String>) -> Result<Client> {
    let key = pd_addrs.first().cloned().unwrap_or_default();
    let mut reg = registry().lock().unwrap();
    let store = reg.entry(key).or_default().clone();
    if let Some(err) = store.new_client_err.lock().unwrap().clone() {
        return Err(err);
    }
    Ok(Client {
        pd_addrs,
        store,
        id: STORE_ID.fetch_add(1, Ordering::SeqCst),
    })
}

impl Client {
    /// 开启事务；缓冲初始为空。
    pub fn Begin(&self) -> Result<Transaction> {
        if let Some(err) = self.store.begin_err.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(Transaction {
            store: self.store.clone(),
            buffer: BTreeMap::new(),
            committed: false,
        })
    }

    /// Go `client.DeleteRange(ctx, start, end, concurrency)`.
    /// 删除已提交层 [start,end)；返回删除条数。
    pub fn DeleteRange(
        &self,
        _ctx: &Context,
        start: &[u8],
        end: &[u8],
        _concurrency: i32,
    ) -> Result<(i32, ())> {
        if let Some(err) = self.store.delete_err.lock().unwrap().clone() {
            return Err(err);
        }
        let mut data = self.store.data.lock().unwrap();
        let keys: Vec<Vec<u8>> = data
            .range(start.to_vec()..end.to_vec())
            .map(|(k, _)| k.clone())
            .collect();
        // 返回删除数量，对齐 Go 多返回值第一分量。
        let n = keys.len() as i32;
        for k in keys {
            data.remove(&k);
        }
        Ok((n, ()))
    }
}

/// 未提交事务：缓冲 + 提交标志。
pub struct Transaction {
    store: SharedStore,
    buffer: BTreeMap<Vec<u8>, Vec<u8>>,
    committed: bool,
}

impl Transaction {
    /// 写入事务缓冲（不立即可见于其他事务）。
    pub fn Set(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        if let Some(err) = self.store.set_err.lock().unwrap().clone() {
            return Err(err);
        }
        self.buffer.insert(key, value);
        Ok(())
    }

    /// 将缓冲合并进共享 store。
    pub fn Commit(&mut self, _ctx: &Context) -> Result<()> {
        if let Some(err) = self.store.commit_err.lock().unwrap().clone() {
            return Err(err);
        }
        let mut data = self.store.data.lock().unwrap();
        // take 缓冲后写入，避免部分提交残留。
        for (k, v) in std::mem::take(&mut self.buffer) {
            data.insert(k, v);
        }
        self.committed = true;
        Ok(())
    }

    /// Go `txn.Iter(start, end)`.
    /// 合并视图上的半开区间迭代器。
    pub fn Iter(&self, start: &[u8], end: &[u8]) -> Result<KvIterator> {
        if let Some(err) = self.store.iter_err.lock().unwrap().clone() {
            return Err(err);
        }
        let data = self.store.data.lock().unwrap();
        // 合并已提交与缓冲：同键缓冲覆盖。
        // Merge committed store with uncommitted buffer (buffer wins).
        let mut merged: KvMap = data.clone();
        for (k, v) in &self.buffer {
            merged.insert(k.clone(), v.clone());
        }
        let entries: Vec<(Vec<u8>, Vec<u8>)> = merged
            .range(start.to_vec()..end.to_vec())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // 空结果直接无效，避免 Valid 误真。
        let idx = if entries.is_empty() { -1 } else { 0 };
        let next_err = self.store.next_err.lock().unwrap().clone();
        Ok(KvIterator {
            entries,
            idx,
            next_err,
        })
    }
}

/// 快照迭代器；idx=-1 表示无效/耗尽。
pub struct KvIterator {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    /// 当前位置；-1 表示无效或已耗尽。
    /// Current position; -1 = invalid/exhausted.
    idx: isize,
    next_err: Option<Error>,
}

impl KvIterator {
    /// 当前位置是否有效。
    pub fn Valid(&self) -> bool {
        self.idx >= 0 && (self.idx as usize) < self.entries.len()
    }

    /// 前进；越界后 Valid=false。
    pub fn Next(&mut self) -> Result<()> {
        if let Some(err) = self.next_err.clone() {
            return Err(err);
        }
        if self.idx < 0 {
            return Ok(());
        }
        self.idx += 1;
        if self.idx as usize >= self.entries.len() {
            self.idx = -1;
        }
        Ok(())
    }

    /// 当前键；无效时空切片。
    pub fn Key(&self) -> &[u8] {
        if self.idx < 0 || (self.idx as usize) >= self.entries.len() {
            return &[];
        }
        &self.entries[self.idx as usize].0
    }

    /// 当前值；无效时空切片。
    pub fn Value(&self) -> &[u8] {
        if self.idx < 0 || (self.idx as usize) >= self.entries.len() {
            return &[];
        }
        &self.entries[self.idx as usize].1
    }
}

/// 字节比较：-1/0/1。
pub fn bytes_compare(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        CmpOrdering::Less => -1,
        CmpOrdering::Equal => 0,
        CmpOrdering::Greater => 1,
    }
}
