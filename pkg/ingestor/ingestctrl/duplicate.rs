// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 导入过程中的重复键（duplicate）检测与冲突处理。
//
// 提供本地/远端重复 KV 流、待处理键范围切分、冲突批量记录与按策略删除，
// 以及 `DupeDetector`/`DupeController` 将检测结果写入错误管理器。
// `DuplicateResolution` 决定遇冲突时报错、忽略或删除冲突行。

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::iterator::KeyAdapter;
use crate::{CancellationToken, DuplicateResolution, Error, KeyRange, KvPair, Result};

/// 远端重复扫描最大重试轮数。
const MAX_DUP_COLLECT_ATTEMPTS: usize = 5;
/// 向错误管理器批量上报冲突的默认批大小。
const DEFAULT_RECORD_CONFLICT_BATCH: usize = 1024;

/// 一条数据冲突信息：表名、原始 key/value 与可读行描述。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DataConflictInfo {
    pub table_name: String,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub row: String,
}

/// 索引冲突的待处理句柄：关联冲突信息与索引名、编码/原始 handle。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PendingIndexHandle {
    pub conflict: DataConflictInfo,
    pub index_name: String,
    pub handle: Vec<u8>,
    pub raw_handle: Vec<u8>,
}

/// 可排序、可截断的待处理索引句柄列表。
#[derive(Default)]
pub struct pendingIndexHandles {
    items: Vec<PendingIndexHandle>,
}

/// 按容量预分配的待处理索引句柄容器。
pub fn makePendingIndexHandlesWithCapacity(capacity: usize) -> pendingIndexHandles {
    pendingIndexHandles {
        items: Vec::with_capacity(capacity),
    }
}

impl pendingIndexHandles {
    /// 追加一条待处理句柄。
    pub fn append(&mut self, item: PendingIndexHandle) {
        self.items.push(item);
    }
    /// 清空全部句柄。
    pub fn truncate(&mut self) {
        self.items.clear();
    }
    /// 当前句柄数量。
    pub fn Len(&self) -> usize {
        self.items.len()
    }
    /// 按 `raw_handle` 排序，便于后续按行定位。
    pub fn sort(&mut self) {
        self.items
            .sort_by(|left, right| left.raw_handle.cmp(&right.raw_handle));
    }
}

/// 尚未完成重复扫描的键范围集合，按 end key 索引。
#[derive(Clone, Debug, Default)]
pub struct PendingKeyRanges {
    ranges_by_end: BTreeMap<Vec<u8>, Vec<KeyRange>>,
}

/// 用单个初始键范围构造待处理集合。
pub fn newPendingKeyRanges(key_range: KeyRange) -> PendingKeyRanges {
    let mut ranges = PendingKeyRanges::default();
    ranges
        .ranges_by_end
        .entry(key_range.end.clone())
        .or_default()
        .push(key_range);
    ranges
}

impl PendingKeyRanges {
    /// 展平为键范围列表。
    pub fn list(&self) -> Vec<KeyRange> {
        self.ranges_by_end.values().flatten().cloned().collect()
    }

    /// 是否已无待处理范围。
    pub fn empty(&self) -> bool {
        self.ranges_by_end.is_empty()
    }

    /// 将已完成的 `finished` 从重叠范围中切除，留下左右残余子区间。
    pub fn finish(&mut self, finished: &KeyRange) {
        let mut retained = Vec::new();
        for range in self.list() {
            if !range.overlaps(finished) {
                retained.push(range);
                continue;
            }
            // 左侧残余：[range.start, finished.start)
            if range.start < finished.start {
                retained.push(KeyRange {
                    start: range.start.clone(),
                    end: finished.start.clone(),
                });
            }
            // 右侧残余：[finished.end, range.end)；空 end 表示正无穷
            if !finished.end.is_empty() && (range.end.is_empty() || finished.end < range.end) {
                retained.push(KeyRange {
                    start: finished.end.clone(),
                    end: range.end,
                });
            }
        }
        self.ranges_by_end.clear();
        for range in retained {
            if range.end.is_empty() || range.start < range.end {
                self.ranges_by_end
                    .entry(range.end.clone())
                    .or_default()
                    .push(range);
            }
        }
    }
}

/// 重复 KV 流：顺序产出冲突键值对并可关闭。
pub trait DupKVStream {
    fn Next(&mut self) -> Result<Option<KvPair>>;
    fn Close(&mut self) -> Result<()>;
}

/// 基于内存已排序 KV 列表的本地重复流。
pub struct DupKVStreamImpl {
    pairs: Vec<KvPair>,
    cursor: usize,
    adapter: Arc<dyn KeyAdapter>,
    closed: bool,
}

/// 过滤到 `key_range` 内、排序后构造本地重复流；读出时经 KeyAdapter 解码。
pub fn NewLocalDupKVStream(
    pairs: Vec<KvPair>,
    adapter: Arc<dyn KeyAdapter>,
    key_range: &KeyRange,
) -> DupKVStreamImpl {
    let mut pairs: Vec<_> = pairs
        .into_iter()
        .filter(|pair| {
            pair.key >= key_range.start && (key_range.end.is_empty() || pair.key < key_range.end)
        })
        .collect();
    pairs.sort_by(|left, right| left.key.cmp(&right.key));
    DupKVStreamImpl {
        pairs,
        cursor: 0,
        adapter,
        closed: false,
    }
}

impl DupKVStream for DupKVStreamImpl {
    fn Next(&mut self) -> Result<Option<KvPair>> {
        if self.closed {
            return Err(Error::Closed);
        }
        let Some(pair) = self.pairs.get(self.cursor).cloned() else {
            return Ok(None);
        };
        self.cursor += 1;
        // 对外暴露解码后的用户 key
        Ok(Some(KvPair {
            key: self.adapter.Decode(&pair.key)?,
            value: pair.value,
        }))
    }

    fn Close(&mut self) -> Result<()> {
        self.closed = true;
        self.pairs.clear();
        Ok(())
    }
}

/// 远端重复数据源：按续传令牌分页扫描指定键范围。
pub trait RemoteDuplicateSource: Send + Sync {
    fn scan_duplicates(
        &self,
        token: &CancellationToken,
        key_range: &KeyRange,
        continuation: &[u8],
    ) -> Result<(Vec<KvPair>, Vec<u8>)>;
}

/// 远端重复流：缓冲分页结果，空 continuation 表示扫描结束。
pub struct RemoteDupKVStream {
    token: CancellationToken,
    source: Arc<dyn RemoteDuplicateSource>,
    key_range: KeyRange,
    continuation: Vec<u8>,
    buffer: Vec<KvPair>,
    cursor: usize,
    finished: bool,
}

/// 构造尚未拉取任何页的远端重复流。
pub fn NewRemoteDupKVStream(
    token: CancellationToken,
    source: Arc<dyn RemoteDuplicateSource>,
    key_range: KeyRange,
) -> RemoteDupKVStream {
    RemoteDupKVStream {
        token,
        source,
        key_range,
        continuation: Vec::new(),
        buffer: Vec::new(),
        cursor: 0,
        finished: false,
    }
}

impl DupKVStream for RemoteDupKVStream {
    fn Next(&mut self) -> Result<Option<KvPair>> {
        loop {
            self.token.check()?;
            if let Some(pair) = self.buffer.get(self.cursor).cloned() {
                self.cursor += 1;
                return Ok(Some(pair));
            }
            if self.finished {
                return Ok(None);
            }
            // 缓冲耗尽则拉取下一页；空 continuation 标记结束
            let (buffer, continuation) =
                self.source
                    .scan_duplicates(&self.token, &self.key_range, &self.continuation)?;
            self.buffer = buffer;
            self.cursor = 0;
            self.finished = continuation.is_empty();
            self.continuation = continuation;
            if self.buffer.is_empty() && self.finished {
                return Ok(None);
            }
        }
    }

    fn Close(&mut self) -> Result<()> {
        self.finished = true;
        self.buffer.clear();
        Ok(())
    }
}

/// 冲突错误管理器：持久化数据行或索引冲突。
pub trait ErrorManager: Send + Sync {
    fn RecordDataConflictError(&self, conflicts: &[DataConflictInfo]) -> Result<()>;
    fn RecordIndexConflictError(&self, conflicts: &[DataConflictInfo]) -> Result<()>;
}

/// 用于按值比对后删除冲突键的事务抽象。
pub trait Transaction: Send {
    fn BatchGet(&mut self, keys: &[Vec<u8>]) -> Result<HashMap<Vec<u8>, Vec<u8>>>;
    fn Delete(&mut self, key: &[u8]) -> Result<()>;
    fn Commit(self: Box<Self>) -> Result<()>;
}

/// 开启事务的工厂。
pub trait TransactionFactory: Send + Sync {
    fn Begin(&self, token: &CancellationToken) -> Result<Box<dyn Transaction>>;
}

/// 重复检测器：从流中收集冲突，按策略报错、记录或删除。
pub struct DupeDetector {
    table_name: String,
    error_manager: Arc<dyn ErrorManager>,
    transaction_factory: Arc<dyn TransactionFactory>,
    has_duplicate: AtomicBool,
    record_batch_size: usize,
}

/// 构造默认批大小的重复检测器。
pub fn NewDupeDetector(
    table_name: String,
    error_manager: Arc<dyn ErrorManager>,
    transaction_factory: Arc<dyn TransactionFactory>,
) -> DupeDetector {
    DupeDetector {
        table_name,
        error_manager,
        transaction_factory,
        has_duplicate: AtomicBool::new(false),
        record_batch_size: DEFAULT_RECORD_CONFLICT_BATCH,
    }
}

impl DupeDetector {
    /// 是否已观察到至少一条重复。
    pub fn HasDuplicate(&self) -> bool {
        self.has_duplicate.load(Ordering::Acquire)
    }

    /// 记录数据行冲突（非索引）。
    pub fn RecordDataConflictError(
        &self,
        token: &CancellationToken,
        stream: &mut dyn DupKVStream,
        algorithm: DuplicateResolution,
    ) -> Result<()> {
        self.collect_stream(token, stream, algorithm, false)
    }

    /// 记录索引冲突。
    pub fn RecordIndexConflictError(
        &self,
        token: &CancellationToken,
        stream: &mut dyn DupKVStream,
        algorithm: DuplicateResolution,
    ) -> Result<()> {
        self.collect_stream(token, stream, algorithm, true)
    }

    /// 从流中批量收集冲突：Error 立即返回；否则写入管理器，Remove 时再删除。
    fn collect_stream(
        &self,
        token: &CancellationToken,
        stream: &mut dyn DupKVStream,
        algorithm: DuplicateResolution,
        index: bool,
    ) -> Result<()> {
        let result = (|| {
            let mut batch = Vec::with_capacity(self.record_batch_size);
            while let Some(pair) = stream.Next()? {
                token.check()?;
                self.has_duplicate.store(true, Ordering::Release);
                if algorithm == DuplicateResolution::Error {
                    return Err(Error::Conflict {
                        key: pair.key,
                        value: pair.value,
                    });
                }
                batch.push(DataConflictInfo {
                    table_name: self.table_name.clone(),
                    row: format!("key={:?}, value={:?}", pair.key, pair.value),
                    key: pair.key,
                    value: pair.value,
                });
                if batch.len() >= self.record_batch_size {
                    self.write_conflicts(&batch, index)?;
                    if algorithm == DuplicateResolution::Remove {
                        self.delete_conflicts(token, &batch)?;
                    }
                    batch.clear();
                }
            }
            if !batch.is_empty() {
                self.write_conflicts(&batch, index)?;
                if algorithm == DuplicateResolution::Remove {
                    self.delete_conflicts(token, &batch)?;
                }
            }
            Ok(())
        })();
        // Go defers Close and preserves the operation's result on every exit path.
        let _ = stream.Close();
        result
    }

    /// 按数据/索引路径写入错误管理器。
    fn write_conflicts(&self, conflicts: &[DataConflictInfo], index: bool) -> Result<()> {
        if index {
            self.error_manager.RecordIndexConflictError(conflicts)
        } else {
            self.error_manager.RecordDataConflictError(conflicts)
        }
    }

    /// 事务内 BatchGet 后仅删除仍等于扫描时 value 的键，避免误删并发更新。
    fn delete_conflicts(
        &self,
        token: &CancellationToken,
        conflicts: &[DataConflictInfo],
    ) -> Result<()> {
        let mut transaction = self.transaction_factory.Begin(token)?;
        let keys: Vec<_> = conflicts
            .iter()
            .map(|conflict| conflict.key.clone())
            .collect();
        let existing = transaction.BatchGet(&keys)?;
        for conflict in conflicts {
            token.check()?;
            // Delete only the version still matching the conflict scan; a
            // concurrent update must not be removed.
            // 仅删除与冲突扫描时 value 仍一致的版本；并发更新不得被误删。
            if existing.get(&conflict.key) == Some(&conflict.value) {
                transaction.Delete(&conflict.key)?;
            }
        }
        transaction.Commit()
    }

    /// 远端重复任务：对 PendingKeyRanges 循环扫描，可重试直至完成或超限。
    pub fn processRemoteDupTask(
        &self,
        token: &CancellationToken,
        source: Arc<dyn RemoteDuplicateSource>,
        key_range: KeyRange,
        algorithm: DuplicateResolution,
    ) -> Result<()> {
        let mut pending = newPendingKeyRanges(key_range);
        let mut attempt = 0;
        while !pending.empty() {
            token.check()?;
            let ranges = pending.list();
            let mut progress = false;
            for range in ranges {
                let mut stream =
                    NewRemoteDupKVStream(token.clone(), Arc::clone(&source), range.clone());
                match self.RecordDataConflictError(token, &mut stream, algorithm) {
                    Ok(()) => {
                        pending.finish(&range);
                        progress = true;
                    }
                    // 可重试错误本轮跳过该 range，累计 attempt
                    Err(Error::Retryable(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            if progress {
                attempt = 0;
            } else {
                attempt += 1;
            }
            if attempt >= MAX_DUP_COLLECT_ATTEMPTS {
                return Err(Error::Retryable(
                    "duplicate scan exceeded retry limit".into(),
                ));
            }
        }
        Ok(())
    }
}

/// 发现的重复键值对载体（与错误解包结果对应）。
#[derive(Clone, Debug)]
pub struct FoundDuplicateKeys {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// 从 `Error::Conflict` 解出原始 key/value；其它错误类型返回 InvalidArgument。
pub fn RetrieveKeyAndValueFromErrFoundDuplicateKeys(error: &Error) -> Result<(Vec<u8>, Vec<u8>)> {
    match error {
        Error::Conflict { key, value } => Ok((key.clone(), value.clone())),
        _ => Err(Error::InvalidArgument(
            "error is not a duplicate-key error".into(),
        )),
    }
}

/// 本地重复控制器：绑定检测器、本地 KV 快照与 KeyAdapter。
pub struct DupeController {
    detector: DupeDetector,
    local_pairs: Arc<Mutex<Vec<KvPair>>>,
    key_adapter: Arc<dyn KeyAdapter>,
}

impl DupeController {
    /// 组装本地重复控制器。
    pub fn new(
        detector: DupeDetector,
        local_pairs: Arc<Mutex<Vec<KvPair>>>,
        key_adapter: Arc<dyn KeyAdapter>,
    ) -> Self {
        Self {
            detector,
            local_pairs,
            key_adapter,
        }
    }

    /// 在本地 KV 快照上对 `key_range` 做重复收集，返回是否发现重复。
    pub fn CollectLocalDuplicateRows(
        &self,
        token: &CancellationToken,
        key_range: KeyRange,
        algorithm: DuplicateResolution,
    ) -> Result<bool> {
        let pairs = self
            .local_pairs
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone();
        let mut stream = NewLocalDupKVStream(pairs, Arc::clone(&self.key_adapter), &key_range);
        self.detector
            .RecordDataConflictError(token, &mut stream, algorithm)?;
        Ok(self.detector.HasDuplicate())
    }
}
