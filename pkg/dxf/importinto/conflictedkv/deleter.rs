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

// 冲突行删除器：根据冲突编码行收集待删键，并在后台 worker 中批量删除。
//
// 在 resolve-conflicts 步骤中，Handler 解码冲突 KV 后，Deleter 通过快照 BatchGet
// 确认键仍存在，缓冲到阈值/数量上限后发给 DeleteWorker；删除事务可重试，
// 并支持取消（Cancel）。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use astersql_dxf_framework_taskexecutor_execute::Collector as ProgressCollector;
use astersql_kv::{Handle, Key};
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::KvPair;
use astersql_meta_model::TableInfo;
use astersql_types::datum::Datum;

use crate::{
    ConflictContext, ConflictKVPair, ConflictRowCodec, ConflictStore, DataKVGroup,
    EncodedRowHandler, Handler, LazyRefreshedSnapshot, NewBaseHandler, NewDataKVHandler,
    NewIndexKVHandler, NewLazyRefreshedSnapshot, TrafficRecorder,
};

/// 存储操作重试的最小退避间隔。
const storeOpMinBackoff: Duration = Duration::from_millis(100);
/// 存储操作重试的最大退避间隔。
const storeOpMaxBackoff: Duration = Duration::from_secs(1);
/// 存储操作最大重试次数。
const storeOpMaxRetryCnt: usize = 10;

/// 缓冲待删键的字节数上限（默认 2MiB）。
pub static BufferedKeySizeLimit: AtomicUsize = AtomicUsize::new(2 * 1024 * 1024);
/// 缓冲待删键的条数上限（默认 9600）。
pub static BufferedKeyCountLimit: AtomicUsize = AtomicUsize::new(9600);

/// 冲突删除器：驱动 Handler，收集仍存在的键并交给后台 DeleteWorker 删除。
pub struct Deleter {
    handler: Option<Box<dyn Handler>>,
    store: Arc<dyn ConflictStore>,
    snapshot: LazyRefreshedSnapshot,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
    buffered_keys: Vec<Key>,
    buffered_size: usize,
    keys_sender: Option<mpsc::SyncSender<Vec<Key>>>,
}

/// 按 `kv_group` 构造 Deleter：data 组用 DataKVHandler，索引组用 IndexKVHandler（无 handle 过滤）。
pub fn NewDeleter(
    target_table: Arc<TableInfo>,
    store: Arc<dyn ConflictStore>,
    kv_group: impl Into<String>,
    codec: Box<dyn ConflictRowCodec>,
    progress_collector: Option<Arc<dyn ProgressCollector>>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
) -> Deleter {
    let kv_group = kv_group.into();
    let base = NewBaseHandler(target_table, kv_group.clone(), codec, progress_collector);
    let handler: Box<dyn Handler> = if kv_group == DataKVGroup {
        Box::new(NewDataKVHandler(base))
    } else {
        Box::new(NewIndexKVHandler(
            base,
            NewLazyRefreshedSnapshot(store.clone(), traffic_recorder.clone()),
            None,
        ))
    };
    Deleter {
        handler: Some(handler),
        store: store.clone(),
        snapshot: NewLazyRefreshedSnapshot(store, traffic_recorder.clone()),
        traffic_recorder,
        buffered_keys: Vec::new(),
        buffered_size: 0,
        keys_sender: None,
    }
}

impl Deleter {
    /// 启动同步删除 worker，跑完 Handler 后冲刷缓冲并等待 worker 结束。
    pub fn Run(
        &mut self,
        context: &ConflictContext,
        pairs: &mpsc::Receiver<ConflictKVPair>,
    ) -> Result<(), String> {
        // 容量为 0 的同步通道：发送方在 worker 忙时阻塞，形成背压。
        let (sender, receiver) = mpsc::sync_channel::<Vec<Key>>(0);
        self.keys_sender = Some(sender);
        let worker = DeleteWorker {
            store: self.store.clone(),
            traffic_recorder: self.traffic_recorder.clone(),
        };
        let worker_context = context.clone();
        std::thread::scope(|scope| {
            let deletion = scope.spawn(move || worker.deleteLoop(&worker_context, receiver));
            let mut handler = self
                .handler
                .take()
                .ok_or_else(|| "deleter handler is already running".to_owned())?;
            let run_result = handler
                .PreRun()
                .and_then(|_| handler.Run(context, pairs, self));
            let close_result = handler.Close(context, self);
            let flush_result = self.sendKeysToDelete(context);
            self.keys_sender.take();
            self.handler = Some(handler);
            let delete_result = deletion
                .join()
                .map_err(|_| "delete worker panicked".to_owned())?;
            run_result
                .and(close_result)
                .and(flush_result)
                .and(delete_result)
        })
    }

    /// 带重试地收集仍存在的键；缓冲达大小或条数上限时发给删除 worker。
    fn gatherAndDeleteKeysWithRetry(
        &mut self,
        context: &ConflictContext,
        pairs: &[KvPair],
    ) -> Result<(), String> {
        let store = self.store.clone();
        retry(context, store.as_ref(), || {
            self.gatherKeysToDelete(context, pairs)
        })?;
        if self.buffered_size >= BufferedKeySizeLimit.load(Ordering::Acquire)
            || self.buffered_keys.len() >= BufferedKeyCountLimit.load(Ordering::Acquire)
        {
            self.sendKeysToDelete(context)?;
        }
        Ok(())
    }

    /// 对编码出的键做快照 BatchGet，仅把仍存在的键加入缓冲。
    fn gatherKeysToDelete(
        &mut self,
        context: &ConflictContext,
        pairs: &[KvPair],
    ) -> Result<(), String> {
        let keys = pairs
            .iter()
            .map(|pair| Key(pair.key.clone()))
            .collect::<Vec<_>>();
        let result = self.snapshot.BatchGet(context, &keys)?;
        for key in result.keys() {
            self.buffered_size += key.len();
            self.buffered_keys.push(Key(key.clone()));
        }
        Ok(())
    }

    /// 把缓冲键批量发给删除 worker；空缓冲为无操作，取消时返回错误。
    fn sendKeysToDelete(&mut self, context: &ConflictContext) -> Result<(), String> {
        if self.buffered_keys.is_empty() {
            return Ok(());
        }
        if context.IsCancelled() {
            return Err("conflict deletion cancelled".into());
        }
        let keys = std::mem::take(&mut self.buffered_keys);
        self.buffered_size = 0;
        self.keys_sender
            .as_ref()
            .ok_or_else(|| "delete worker is not running".to_owned())?
            .send(keys)
            .map_err(|_| "delete worker stopped before receiving keys".to_owned())
    }
}

impl EncodedRowHandler for Deleter {
    /// 对冲突行重编码得到的 KV 对，收集并排队删除。
    fn HandleEncodedRow(
        &mut self,
        context: &ConflictContext,
        _handle: &dyn Handle,
        _row: &[Datum],
        pairs: &Pairs,
    ) -> Result<(), String> {
        self.gatherAndDeleteKeysWithRetry(context, &pairs.Pairs)
    }
}

/// 后台删除 worker：接收键批次，在事务中批量 Delete 并 Commit。
struct DeleteWorker {
    store: Arc<dyn ConflictStore>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
}

impl DeleteWorker {
    /// 循环接收键批次直至通道关闭。
    fn deleteLoop(
        &self,
        context: &ConflictContext,
        receiver: mpsc::Receiver<Vec<Key>>,
    ) -> Result<(), String> {
        while let Ok(keys) = receiver.recv() {
            retry(context, self.store.as_ref(), || {
                self.deleteBufferedKeys(context, &keys)
            })?;
        }
        Ok(())
    }

    /// 开启事务删除一批键；任一 Delete 失败则 Rollback。
    fn deleteBufferedKeys(&self, context: &ConflictContext, keys: &[Key]) -> Result<(), String> {
        if keys.is_empty() {
            return Ok(());
        }
        if let Some(recorder) = &self.traffic_recorder {
            recorder.IncClusterWriteBytes(keys.iter().map(|key| key.0.len() as u64).sum());
        }
        let mut transaction = self.store.Begin()?;
        for key in keys {
            if let Err(error) = transaction.Delete(key) {
                let _ = transaction.Rollback();
                return Err(error);
            }
        }
        transaction.Commit(context)
    }
}

/// 对可重试存储错误做指数退避重试，直至成功、不可重试或达到次数上限；支持取消。
fn retry(
    context: &ConflictContext,
    store: &dyn ConflictStore,
    mut operation: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let mut delay = storeOpMinBackoff;
    let mut last_error = None;
    for attempt in 0..storeOpMaxRetryCnt {
        if context.IsCancelled() {
            return Err("conflict store operation cancelled".into());
        }
        match operation() {
            Ok(()) => return Ok(()),
            Err(error) if store.IsRetryableError(&error) && attempt + 1 < storeOpMaxRetryCnt => {
                last_error = Some(error);
                std::thread::sleep(delay);
                delay = delay.saturating_mul(2).min(storeOpMaxBackoff);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| "conflict store operation failed".into()))
}
