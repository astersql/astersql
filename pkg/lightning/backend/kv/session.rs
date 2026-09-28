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
// Copyright 2026 AsterSQL.

// Lightning KV 编码用的轻量 Session 与内存缓冲。
//
// 模拟 TiDB Session / 事务接口的子集：在内存 MemBuf 中累积键值对（KV pairs），
// 提供表达式上下文与表变更上下文，供 SQL→KV 编码器写入记录与索引键。
// 不走真实 TiKV 事务或两阶段提交（2PC）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use encode::{Datum, SessionOptions};
use verification::KvPair;

use crate::{
    Pairs, litExprContext, litTableMutateContext, newLitExprContext, newLitTableMutateContext,
};

/// 可复用 BytesBuf 池的最大容量，超出时淘汰最旧缓冲。
pub const maxAvailableBufSize: usize = 20;
/// 1 MiB，用作缓冲分配的最小对齐单位。
const MIB: usize = 1024 * 1024;

/// 无效迭代器占位：始终 Valid=false，满足 transaction::Iter 签名。
#[derive(Clone, Debug, Default)]
pub struct invalidIterator;

impl invalidIterator {
    /// 始终返回 false。
    pub fn Valid(&self) -> bool {
        false
    }
    /// 空关闭。
    pub fn Close(&mut self) {}
}

/// 连续字节缓冲：按 idx 追加切片，cap 为可用总容量。
#[derive(Clone, Debug)]
pub struct BytesBuf {
    buf: Vec<u8>,
    pub idx: usize,
    pub cap: usize,
}

impl BytesBuf {
    /// 将 value 拷入缓冲并返回该段的副本（键/值所有权独立于缓冲生命周期）。
    fn add(&mut self, value: &[u8]) -> Vec<u8> {
        let start = self.idx;
        self.buf[start..start + value.len()].copy_from_slice(value);
        self.idx += value.len();
        self.buf[start..self.idx].to_vec()
    }
    /// 释放底层 Vec，重置 idx/cap。
    pub fn destroy(&mut self) {
        self.buf.clear();
        self.buf.shrink_to_fit();
        self.idx = 0;
        self.cap = 0;
    }
}

/// 按 size 预分配全零 BytesBuf。
pub fn newBytesBuf(size: usize) -> BytesBuf {
    BytesBuf {
        buf: vec![0; size],
        idx: 0,
        cap: size,
    }
}

/// 内存写缓冲：持有当前 BytesBuf、可复用池，以及已写入的 KV 对列表。
#[derive(Default)]
pub struct MemBuf {
    pub buf: Option<BytesBuf>,
    pub availableBufs: Arc<Mutex<Vec<BytesBuf>>>,
    pub kvPairs: Pairs,
    size: usize,
}

impl MemBuf {
    /// 将 buf 回收进池；池满时销毁最旧项。
    pub fn Recycle(&mut self, mut buf: BytesBuf) {
        buf.idx = 0;
        buf.cap = buf.buf.len();
        let mut available = self.availableBufs.lock().unwrap();
        if available.len() >= maxAvailableBufSize {
            let mut evicted = available.remove(0);
            evicted.destroy();
        }
        available.push(buf);
    }

    /// 分配至少能容纳 requested 字节的缓冲：优先从池中取，否则新建。
    /// 实际容量为 max(1MiB, next_power_of_two(requested)*2)。
    pub fn AllocateBuf(&mut self, requested: usize) {
        let size = MIB.max(requested.max(1).next_power_of_two().saturating_mul(2));
        let existing = {
            let mut available = self.availableBufs.lock().unwrap();
            available
                .iter()
                .position(|buf| buf.cap >= size)
                .map(|index| {
                    available.swap(0, index);
                    available.remove(0)
                })
        };
        self.buf = Some(existing.unwrap_or_else(|| newBytesBuf(size)));
    }

    /// 取出当前缓冲所有权，供调用方使用或 Recycle。
    pub fn TakeBuf(&mut self) -> Option<BytesBuf> {
        self.buf.take()
    }

    /// 写入一对 key/value：空间不足时回收旧缓冲并重新分配。
    pub fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        let size = key.len() + value.len();
        let needs_buffer = self
            .buf
            .as_ref()
            .is_none_or(|buf| buf.cap.saturating_sub(buf.idx) < size);
        if needs_buffer {
            if let Some(old) = self.buf.take() {
                self.Recycle(old);
            }
            self.AllocateBuf(size);
        }
        let buf = self.buf.as_mut().expect("buffer allocated above");
        self.kvPairs.Pairs.push(KvPair {
            key: buf.add(key),
            val: buf.add(value),
        });
        self.size += size;
        Ok(())
    }

    /// 带 flags 写入，当前实现与 Set 相同。
    pub fn SetWithFlags(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.Set(key, value)
    }
    /// 删除不支持（Lightning 编码路径只追加）。
    pub fn Delete(&self, _: &[u8]) -> Result<(), String> {
        Err("unsupported operation".into())
    }
    /// 释放 staging 标记的空实现。
    pub fn Release(&self, _: usize) {}
    /// 返回 staging 句柄；此处恒为 0。
    pub fn Staging(&self) -> usize {
        0
    }
    /// 清理 staging 的空实现。
    pub fn Cleanup(&self, _: usize) {}
    /// 查询 key flags；未实现。
    pub fn GetFlags(&self, _: &[u8]) -> Result<u32, String> {
        Err("key not exist".into())
    }
    /// 更新 flags 的空实现。
    pub fn UpdateFlags(&self, _: &[u8]) {}
    /// 更新断言 flags 的空实现。
    pub fn UpdateAssertionFlags(&self, _: &[u8]) {}
    /// 从本地已写 KV 对中按 key 查找（后写覆盖先写）。
    pub fn GetLocal(&self, key: &[u8]) -> Result<Vec<u8>, String> {
        self.kvPairs
            .Pairs
            .iter()
            .rev()
            .find(|pair| pair.key == key)
            .map(|pair| pair.val.clone())
            .ok_or_else(|| "key not exist".into())
    }
    /// 已写入字节总量。
    pub fn Size(&self) -> usize {
        self.size
    }
    /// 已写入键值对条数。
    pub fn Len(&self) -> usize {
        self.kvPairs.Pairs.len()
    }
}

/// 联合存储占位：仅暴露 MemBuf，对齐 Go 侧 kvUnionStore 接口。
#[derive(Default)]
pub struct kvUnionStore {
    pub MemBuf: MemBuf,
}

impl kvUnionStore {
    /// 取得可变内存缓冲。
    pub fn GetMemBuffer(&mut self) -> &mut MemBuf {
        &mut self.MemBuf
    }
    /// 索引名查询不支持。
    pub fn GetIndexName(&self, _: i64, _: i64) -> String {
        panic!("Unsupported Operation")
    }
    /// 缓存索引名的空实现。
    pub fn CacheIndexName(&self, _: i64, _: i64, _: &str) {}
    /// 缓存表信息的空实现。
    pub fn CacheTableInfo(&self, _: i64) {}
}

/// 轻量事务：键值只落在内存 MemBuf，无持久化与 2PC。
#[derive(Default)]
pub struct transaction {
    pub kvUnionStore: kvUnionStore,
}

impl transaction {
    /// 缓冲中的键值对数量。
    pub fn Len(&self) -> usize {
        self.kvUnionStore.MemBuf.Len()
    }
    /// 取得可变 MemBuf。
    pub fn GetMemBuffer(&mut self) -> &mut MemBuf {
        self.kvUnionStore.GetMemBuffer()
    }
    /// 丢弃事务的空实现。
    pub fn Discard(&self) {}
    /// 刷盘占位，恒返回 0。
    pub fn Flush(&self) -> Result<usize, String> {
        Ok(0)
    }
    /// 重置占位。
    pub fn Reset(&self) {}
    /// 读取不受支持；与 Go 的裁剪事务一致，始终返回 key 不存在。
    pub fn Get(&self, _: &[u8]) -> Result<Vec<u8>, String> {
        Err("key not exist".into())
    }
    /// 范围迭代不支持，返回 invalidIterator。
    pub fn Iter(&self, _: &[u8], _: &[u8]) -> Result<invalidIterator, String> {
        Ok(invalidIterator)
    }
    /// 写入一对键值。
    pub fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        self.GetMemBuffer().Set(key, value)
    }
    /// 表信息缓存查询，恒为 None。
    pub fn GetTableInfo(&self, _: i64) -> Option<()> {
        None
    }
    /// 缓存表信息空实现。
    pub fn CacheTableInfo(&self, _: i64) {}
    /// 是否流水线事务；Lightning 编码路径为 false。
    pub fn IsPipelined(&self) -> bool {
        false
    }
    /// 可能刷盘的空实现。
    pub fn MayFlush(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Lightning 编码会话：持有内存事务、表达式上下文与表变更上下文。
pub struct Session {
    txn: transaction,
    exprCtx: litExprContext,
    tblCtx: litTableMutateContext,
}

/// 根据 SessionOptions 构造 Session，仅保留已知系统变量。
pub fn NewSession(options: &SessionOptions) -> Result<Session, String> {
    const KNOWN: &[&str] = &[
        "max_allowed_packet",
        "div_precision_increment",
        "time_zone",
        "default_week_format",
        "block_encryption_mode",
        "group_concat_max_len",
        "tidb_backoff_weight",
        "tidb_row_format_version",
        "tidb_enable_row_level_checksum",
        "tidb_enable_mutation_checker",
        "tidb_txn_assertion_level",
    ];
    let mut sysVars = HashMap::new();
    // 过滤未知 sysvar，避免污染表达式上下文。
    for (key, value) in &options.SysVars {
        if KNOWN.contains(&key.as_str()) {
            sysVars.insert(key.clone(), value.clone());
        }
    }
    let exprCtx = newLitExprContext(options.SQLMode, &sysVars, options.Timestamp)?;
    let tblCtx = newLitTableMutateContext(&exprCtx, &sysVars)?;
    Ok(Session {
        txn: transaction::default(),
        exprCtx,
        tblCtx,
    })
}

impl Session {
    /// 表达式求值上下文（SQL mode、时区、用户变量等）。
    pub fn GetExprCtx(&self) -> &litExprContext {
        &self.exprCtx
    }
    /// 可变事务句柄。
    pub fn Txn(&mut self) -> &mut transaction {
        &mut self.txn
    }
    /// 表变更上下文（行编码开关等）。
    pub fn GetTableCtx(&self) -> &litTableMutateContext {
        &self.tblCtx
    }
    /// 取出并清空已累积的 KV 对，保留 Vec 容量以复用。
    pub fn TakeKvPairs(&mut self) -> Pairs {
        let memBuf = self.txn.GetMemBuffer();
        let capacity = memBuf.kvPairs.Pairs.capacity();
        let pairs = std::mem::replace(
            &mut memBuf.kvPairs,
            Pairs {
                Pairs: Vec::with_capacity(capacity),
                ..Default::default()
            },
        );
        memBuf.size = 0;
        pairs
    }
    /// 设置用户变量。
    pub fn SetUserVarVal(&mut self, name: &str, dt: Datum) {
        self.exprCtx.setUserVarVal(name, dt);
    }
    /// 清除用户变量。
    pub fn UnsetUserVar(&mut self, varName: &str) {
        self.exprCtx.unsetUserVar(varName);
    }
    /// 销毁当前缓冲与池中所有 BytesBuf，释放内存。
    pub fn Close(&mut self) {
        if let Some(mut buf) = self.txn.GetMemBuffer().buf.take() {
            buf.destroy();
        }
        let pool = Arc::clone(&self.txn.GetMemBuffer().availableBufs);
        for mut buf in pool.lock().unwrap().drain(..) {
            buf.destroy();
        }
    }
}
