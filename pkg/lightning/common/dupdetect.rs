// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 导入过程中的重复键（duplicate key）检测。
//
// 本地排序后的 KV 流里，相同业务键若出现多条，需要记录到重复库或直接报错。
// `DupDetector` 在迭代器上滑动，解码键后比较相邻项；遇重复则写入 `WriteBatch`，
// 批量达到 `maxDuplicateBatchSize` 时刷盘。

use crate::{CommonError, ErrFoundDuplicateKeys, KeyAdapter};
use std::sync::Arc;
/// 重复键写批的最大累计字节数（约 4MiB），超过则 flush。
pub const maxDuplicateBatchSize: usize = 4 << 20;
/// 只读 KV 迭代器：前进并暴露当前键值。
pub trait KVIter {
    fn Next(&mut self) -> bool;
    fn Key(&self) -> &[u8];
    fn Value(&self) -> &[u8];
}
/// 可提交的写批接口，用于把重复键落盘到本地重复库。
pub trait WriteBatch: Send {
    fn Set(&mut self, key: &[u8], value: &[u8]) -> Result<(), CommonError>;
    /// Commit the pending writes. `sync` requests durable persistence before return.
    fn Commit(&mut self, sync: bool) -> Result<(), CommonError>;
    fn Reset(&mut self);
    fn Close(&mut self) -> Result<(), CommonError>;
}
/// 内存实现的写批：区分待提交 `Pending` 与已提交 `Committed`。
#[derive(Default)]
pub struct MemoryWriteBatch {
    pub Pending: Vec<(Vec<u8>, Vec<u8>)>,
    pub Committed: Vec<(Vec<u8>, Vec<u8>)>,
    pub SyncCommits: usize,
    pub Closed: bool,
}
impl WriteBatch for MemoryWriteBatch {
    fn Set(&mut self, k: &[u8], v: &[u8]) -> Result<(), CommonError> {
        self.Pending.push((k.into(), v.into()));
        Ok(())
    }
    fn Commit(&mut self, sync: bool) -> Result<(), CommonError> {
        if sync {
            self.SyncCommits += 1;
        }
        self.Committed.append(&mut self.Pending);
        Ok(())
    }
    fn Reset(&mut self) {
        self.Pending.clear()
    }
    fn Close(&mut self) -> Result<(), CommonError> {
        self.Closed = true;
        Ok(())
    }
}
/// Receives the same diagnostic side effect as Go's duplicate-key debug log.
pub trait DupDetectLogger: Send + Sync {
    fn DuplicateDetected(&self, key: &[u8], value: &[u8], raw_key: &[u8]);
}

/// Logger implementation for callers that intentionally discard diagnostics.
#[derive(Clone, Copy, Default)]
pub struct NoopDupDetectLogger;

impl DupDetectLogger for NoopDupDetectLogger {
    fn DuplicateDetected(&self, _: &[u8], _: &[u8], _: &[u8]) {}
}
/// 重复检测选项：`ReportErrOnDup` 为真时遇重复立即返回错误而非静默记录。
#[derive(Clone, Copy, Default)]
pub struct DupDetectOpt {
    pub ReportErrOnDup: bool,
}
/// 重复键检测器状态：当前/下一业务键、原始编码键、写批与累计大小。
pub struct DupDetector {
    keyAdapter: Arc<dyn KeyAdapter>,
    dupDBWriteBatch: Box<dyn WriteBatch>,
    curBatchSize: usize,
    curKey: Vec<u8>,
    curRawKey: Vec<u8>,
    curVal: Vec<u8>,
    nextKey: Vec<u8>,
    logger: Arc<dyn DupDetectLogger>,
    option: DupDetectOpt,
}
/// 用给定键适配器、写批与选项构造检测器。
pub fn NewDupDetector(
    adapter: Arc<dyn KeyAdapter>,
    batch: Box<dyn WriteBatch>,
    logger: Arc<dyn DupDetectLogger>,
    option: DupDetectOpt,
) -> DupDetector {
    DupDetector {
        keyAdapter: adapter,
        dupDBWriteBatch: batch,
        curBatchSize: 0,
        curKey: vec![],
        curRawKey: vec![],
        curVal: vec![],
        nextKey: vec![],
        logger,
        option,
    }
}
impl DupDetector {
    /// 用迭代器当前位置初始化当前键值，并返回解码后的业务键与值。
    pub fn Init(&mut self, iter: &dyn KVIter) -> Result<(Vec<u8>, Vec<u8>), CommonError> {
        self.curKey = self.keyAdapter.Decode(vec![], iter.Key())?;
        self.curRawKey = iter.Key().into();
        self.curVal = iter.Value().into();
        Ok((self.curKey.clone(), self.curVal.clone()))
    }
    /// 推进迭代器：键变化则返回新键值；键相同则记重复或按选项报错。
    pub fn Next(
        &mut self,
        iter: &mut dyn KVIter,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>, CommonError> {
        let mut first = false;
        while iter.Next() {
            let encoded = iter.Key().to_vec();
            let value = iter.Value().to_vec();
            self.nextKey = self.keyAdapter.Decode(vec![], &encoded)?;
            // 业务键变化：切换当前项并作为“下一条唯一键”返回。
            if self.nextKey != self.curKey {
                std::mem::swap(&mut self.curKey, &mut self.nextKey);
                self.curRawKey = encoded;
                self.curVal = value;
                return Ok(Some((self.curKey.clone(), self.curVal.clone())));
            }
            if self.option.ReportErrOnDup {
                return Err(ErrFoundDuplicateKeys(&self.curKey, &self.curVal));
            }
            // 同一业务键首次撞车：先记旧项，再记当前项。
            if !first {
                let raw = self.curRawKey.clone();
                let key = self.curKey.clone();
                let val = self.curVal.clone();
                self.record(&raw, &key, &val)?;
                first = true
            }
            let key = self.nextKey.clone();
            self.record(&encoded, &key, &value)?
        }
        Ok(None)
    }
    /// 将一条重复 KV 写入写批，并按累计大小触发 flush。
    pub fn record(&mut self, raw: &[u8], key: &[u8], value: &[u8]) -> Result<(), CommonError> {
        self.logger.DuplicateDetected(key, value, raw);
        self.dupDBWriteBatch.Set(raw, value)?;
        self.curBatchSize += raw.len() + value.len();
        if self.curBatchSize >= maxDuplicateBatchSize {
            self.flush()?
        }
        Ok(())
    }
    /// 提交当前写批并重置累计大小。
    pub fn flush(&mut self) -> Result<(), CommonError> {
        self.dupDBWriteBatch.Commit(true)?;
        self.dupDBWriteBatch.Reset();
        self.curBatchSize = 0;
        Ok(())
    }
    /// 刷盘后关闭写批。
    pub fn Close(&mut self) -> Result<(), CommonError> {
        self.flush()?;
        self.dupDBWriteBatch.Close()
    }
}
