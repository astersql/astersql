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

// 行容器异步读取器：后台线程预取行，经有界 channel 交给调用方顺序消费。
//
// 对应 Go `row_container_reader.go`。`Row` 指向容器内 Chunk；工作线程通过
// `Arc` 持有数据源，保证发送中的行视图在 worker 结束后仍有效。支持取消与错误传播。

use crate::{ChunkError, Row};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// 行容器只读游标接口：前进、取当前行、结束哨兵、错误与关闭。
pub trait RowContainerReader {
    /// 推进到下一行并返回；channel 关闭时返回结束哨兵。
    fn Next(&mut self) -> Row;
    /// 返回当前行（不推进）。
    fn Current(&self) -> Row;
    /// 结束哨兵：空 `Row`（空指针视图）。
    fn End(&self) -> Row;
    /// 后台 worker 写入的错误（若有）。
    fn Error(&self) -> Option<ChunkError>;
    /// 发送取消并 join worker；幂等。
    fn Close(&mut self);
}

/// Read-only operations the background worker needs from a row container.
///
/// 后台 worker 所需的只读数据源：块数、块内行数、以及按块取出行列表。
pub trait RowContainerSource: Send + Sync {
    /// 容器中的 Chunk 数量。
    fn NumChunks(&self) -> usize;
    /// 指定下标 Chunk 的行数。
    fn NumRowsOfChunk(&self, index: usize) -> usize;
    /// 取出指定 Chunk 的全部行视图；失败则终止预取。
    fn RowsOfChunk(&self, index: usize) -> Result<Vec<Row>, ChunkError>;
}

/// channel 载荷：包装 `Row` 以声明 `Send`（底层指向 Arc 持有的 Chunk）。
#[derive(Clone)]
struct SendRow(Row);

// Row points into a RowContainer kept alive by the worker's Arc. Sending it is
// safe for the same reason Go can send Row values while spilling concurrently.
// Row 指向由 worker Arc 保活的容器；与 Go 在 spill 并发下发送 Row 同理，可安全跨线程。
unsafe impl Send for SendRow {}

/// 带后台预取线程的行读取器实现。
pub struct rowContainerReader {
    // Keeps every Row's backing chunks alive even after the worker exits.
    // 即使 worker 退出，仍通过 Arc 保活所有行所依赖的 Chunk。
    source: Arc<dyn RowContainerSource>,
    /// 当前游标行。
    currentRow: Row,
    /// 接收预取行的有界 channel。
    rowCh: Receiver<SendRow>,
    /// 向 worker 发送取消信号。
    cancel: Sender<()>,
    /// 后台预取线程句柄。
    worker: Option<JoinHandle<()>>,
    /// 共享错误槽。
    err: Arc<Mutex<Option<ChunkError>>>,
    /// 是否已 Close。
    closed: bool,
}

impl RowContainerReader for rowContainerReader {
    fn Next(&mut self) -> Row {
        // channel 关闭或发送端退出时视为结束。
        self.currentRow = match self.rowCh.recv() {
            Ok(SendRow(row)) => row,
            Err(_) => self.End(),
        };
        self.currentRow.clone()
    }

    fn Current(&self) -> Row {
        self.currentRow.clone()
    }
    fn End(&self) -> Row {
        Row::default()
    }

    fn Error(&self) -> Option<ChunkError> {
        self.err
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn Close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        // 先发取消，再 join，避免 worker 阻塞在满的 row channel 上。
        let _ = self.cancel.try_send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for rowContainerReader {
    fn drop(&mut self) {
        self.Close();
    }
}

/// 创建读取器并启动预取 worker；构造末尾会先 `Next` 一次定位到首行。
pub fn NewRowContainerReader<S>(source: Arc<S>) -> Box<rowContainerReader>
where
    S: RowContainerSource + 'static,
{
    let source: Arc<dyn RowContainerSource> = source;
    // channel 容量：空容器用 1024；否则取首块行数的两倍。
    let capacity = if source.NumChunks() == 0 {
        1024
    } else {
        source.NumRowsOfChunk(0).saturating_mul(2)
    };
    let (rowSender, rowReceiver) = bounded(capacity);
    let (cancelSender, cancelReceiver) = bounded(1);
    let error = Arc::new(Mutex::new(None));
    let workerError = Arc::clone(&error);
    let workerSource = Arc::clone(&source);

    let worker = thread::spawn(move || {
        // 按块扫描；错误写入共享槽后退出；发送与取消用 select 竞争。
        for chunkIndex in 0..workerSource.NumChunks() {
            let rows = match workerSource.RowsOfChunk(chunkIndex) {
                Ok(rows) => rows,
                Err(error) => {
                    *workerError
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
                    return;
                }
            };
            for row in rows {
                crossbeam_channel::select! {
                    send(rowSender, SendRow(row)) -> result => {
                        if result.is_err() { return; }
                    },
                    recv(cancelReceiver) -> _ => return,
                }
            }
        }
    });

    let mut reader = Box::new(rowContainerReader {
        source,
        currentRow: Row::default(),
        rowCh: rowReceiver,
        cancel: cancelSender,
        worker: Some(worker),
        err: error,
        closed: false,
    });
    reader.Next();
    reader
}
