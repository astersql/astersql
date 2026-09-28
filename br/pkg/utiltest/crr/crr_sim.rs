// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

//! CRR 复制工人与上游存储包装的测试替身。
//! 对应 Go `br/pkg/utiltest/crr` 中的 worker/upstream 模拟逻辑。
//! 工人从事件通道拉取「新版本」路径，缓冲后复制到下游；默认最新优先。
//! 上游 Write/Create/Rename 成功后发射事件，通道满时自旋短睡直至取消。
//! 本模拟不实现真实跨区域 CRR 协议，仅验证事件驱动复制与最新优先策略。
//! Pull/Replicate 拆步调用，便于测试插入断言与故障注入点。
//! Random 复制路径依赖注入的 intN，保证单测可复现。
//! 工人与上游解耦：仅通过事件通道与 Storage 接口交互。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::stubs::{
    Context, Error, Reader, ReaderOption, Result, Storage, WalkOption, Writer, WriterOption,
};

/// NewVersionCreatedEvent describes one upstream object version creation.
/// 仅携带对象路径；内容由工人再向上游 ReadFile。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewVersionCreatedEvent {
    pub Path: String,
}

/// CRRWorker simulates a CRR background worker.
///
/// It only consumes NewVersionCreatedEvent from a channel. PullMessages moves
/// available events into a local buffer. ReplicateBuffered replicates files from
/// the buffer to downstream, and intentionally writes newest first.
///
/// 通道断开后 `messages` 置 None，后续 Pull 直接返回 0。
pub struct CRRWorker {
    upstream: Arc<dyn Storage>,
    // 下游目标：复制成功写入此处。
    downstream: Arc<dyn Storage>,
    messages: Option<Receiver<NewVersionCreatedEvent>>,
    // 尚未复制的事件；ReplicateBuffered 从尾部弹出（最新优先）。
    buffer: Vec<NewVersionCreatedEvent>,
}

/// 构造工人；接管 messages 接收端，初始缓冲为空。
pub fn NewCRRWorker(
    upstream: Arc<dyn Storage>,
    // 下游目标：复制成功写入此处。
    downstream: Arc<dyn Storage>,
    messages: impl Into<Option<Receiver<NewVersionCreatedEvent>>>,
) -> CRRWorker {
    CRRWorker {
        upstream,
        downstream,
        messages: messages.into(),
        buffer: Vec::new(),
    }
}

impl CRRWorker {
    /// PullMessages drains up to limit events from channel into local buffer.
    ///
    /// If limit <= 0, it drains all currently available events.
    /// 空 Path 事件丢弃不计拉取数；非阻塞 try_recv，遇 Empty 即停。
    pub fn PullMessages(&mut self, limit: i32) -> i32 {
        let Some(ref messages) = self.messages else {
            return 0;
        };

        let mut pulled = 0i32;
        while limit <= 0 || pulled < limit {
            match messages.try_recv() {
                Ok(event) => {
                    if event.Path.is_empty() {
                        // 无效应答跳过，不增加 pulled。
                        continue;
                    }
                    self.buffer.push(event);
                    pulled += 1;
                }
                Err(TryRecvError::Empty) => return pulled,
                Err(TryRecvError::Disconnected) => {
                    // 发送端关闭：释放 Receiver，避免后续误用。
                    self.messages = None;
                    return pulled;
                }
            }
        }
        pulled
    }

    /// BufferedMessages returns a snapshot of not-yet-replicated events.
    /// 返回缓冲克隆，供断言观察排队顺序。
    pub fn BufferedMessages(&self) -> Vec<NewVersionCreatedEvent> {
        self.buffer.clone()
    }

    /// ReplicateBuffered replicates up to limit buffered events.
    ///
    /// If limit <= 0, it replicates all buffered events.
    /// 故意从缓冲尾部取事件，模拟「最新版本优先」复制策略。
    pub fn ReplicateBuffered(&mut self, ctx: &Context, limit: i32) -> Result<i32> {
        let mut limit = limit;
        if limit <= 0 || limit as usize > self.buffer.len() {
            limit = self.buffer.len() as i32;
        }

        let mut replicated = 0i32;
        while replicated < limit {
            let idx = self.buffer.len() - 1;
            let event = self.buffer[idx].clone();

            self.replicateOne(ctx, &event.Path)?;
            // truncate 去掉已复制尾部，保留更旧事件。
            self.buffer.truncate(idx);
            replicated += 1;
        }
        Ok(replicated)
    }

    /// ReplicateBufferedRandom replicates up to limit buffered events in random order.
    /// 先用注入的 intN 做 Fisher-Yates 打乱，再委托 ReplicateBuffered。
    pub fn ReplicateBufferedRandom(
        &mut self,
        ctx: &Context,
        limit: i32,
        intN: Option<&mut dyn FnMut(usize) -> usize>,
    ) -> Result<i32> {
        let Some(intN) = intN else {
            return Err(Error::new("random replicate buffered: nil intN"));
        };
        if self.buffer.len() >= 2 {
            for i in (1..self.buffer.len()).rev() {
                let j = intN(i + 1);
                self.buffer.swap(i, j);
            }
        }
        self.ReplicateBuffered(ctx, limit)
    }

    // 单文件复制：上游不存在则视为成功跳过；读 NotExist 同样跳过。
    fn replicateOne(&self, ctx: &Context, name: &str) -> Result<()> {
        let exists = self
            .upstream
            .FileExists(ctx, name)
            .map_err(|e| Error::new(format!("check upstream file {name}: {e}")))?;
        if !exists {
            return Ok(());
        }

        let payload = match self.upstream.ReadFile(ctx, name) {
            Ok(p) => p,
            // 检查与读取之间的竞态删除：吞掉 NotExist。
            Err(e) if e.is_not_exist => return Ok(()),
            Err(e) => return Err(Error::new(format!("read upstream file {name}: {e}"))),
        };

        self.downstream
            .WriteFile(ctx, name, &payload)
            .map_err(|e| Error::new(format!("write downstream file {name}: {e}")))?;
        Ok(())
    }
}

/// CRRUpstreamStorage writes to upstream and emits NewVersionCreatedEvent.
/// 装饰底层 Storage：写路径成功后向工人通道发事件。
pub struct CRRUpstreamStorage {
    storage: Arc<dyn Storage>,
    // 有界发送端；与工人 Receiver 成对。
    events: Option<SyncSender<NewVersionCreatedEvent>>,
}

/// 包装存储与事件发送端；events 以 Option 持有以便关闭语义扩展。
pub fn NewCRRUpstreamStorage(
    storage: Arc<dyn Storage>,
    events: impl Into<Option<SyncSender<NewVersionCreatedEvent>>>,
) -> CRRUpstreamStorage {
    CRRUpstreamStorage {
        storage,
        events: events.into(),
    }
}

// 发送新版本事件；通道满则短睡重试，上下文取消或断开则失败。
fn emitNewVersionEvent(
    ctx: &Context,
    events: &Option<SyncSender<NewVersionCreatedEvent>>,
    name: &str,
) -> Result<()> {
    let Some(events) = events else {
        return Err(Error::new(format!(
            "emit new-version event for {name}: event channel is nil"
        )));
    };
    loop {
        if ctx.is_done() {
            return Err(Error::new(format!(
                "emit new-version event for {name}: {}",
                ctx.err_message()
                    .unwrap_or_else(|| "context canceled".into())
            )));
        }
        match events.try_send(NewVersionCreatedEvent {
            Path: name.to_string(),
        }) {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                // 有界通道背压：避免阻塞测试线程过久。
                thread::sleep(Duration::from_millis(1));
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                return Err(Error::new(format!(
                    "emit new-version event for {name}: channel closed"
                )));
            }
        }
    }
}

impl Storage for CRRUpstreamStorage {
    /// 先写底层再发事件；底层失败则不发事件。
    fn WriteFile(&self, ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
        self.storage.WriteFile(ctx, name, data)?;
        emitNewVersionEvent(ctx, &self.events, name)
    }

    /// 透传读取，不产生新版本事件。
    fn ReadFile(&self, ctx: &Context, name: &str) -> Result<Vec<u8>> {
        self.storage.ReadFile(ctx, name)
    }

    /// 透传存在性检查。
    fn FileExists(&self, ctx: &Context, name: &str) -> Result<bool> {
        self.storage.FileExists(ctx, name)
    }

    /// 透传删除；删除不发 NewVersion 事件。
    fn DeleteFile(&self, ctx: &Context, name: &str) -> Result<()> {
        self.storage.DeleteFile(ctx, name)
    }

    /// 批量删除透传。
    fn DeleteFiles(&self, ctx: &Context, names: &[String]) -> Result<()> {
        self.storage.DeleteFiles(ctx, names)
    }

    /// 目录遍历透传到底层存储。
    fn WalkDir(
        &self,
        ctx: &Context,
        opt: &WalkOption,
        fn_: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()> {
        self.storage.WalkDir(ctx, opt, fn_)
    }

    /// 返回底层 URI，便于日志定位。
    fn URI(&self) -> String {
        self.storage.URI()
    }

    /// 打开读流，不发射事件。
    fn Open(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&ReaderOption>,
    ) -> Result<Box<dyn Reader>> {
        self.storage.Open(ctx, name, option)
    }

    /// Create 返回包装 Writer，在 Close 成功后再发事件（对齐流式上传完成点）。
    fn Create(
        &self,
        ctx: &Context,
        name: &str,
        option: Option<&WriterOption>,
    ) -> Result<Box<dyn Writer>> {
        let inner = self.storage.Create(ctx, name, option)?;
        Ok(Box::new(CrrEventWriter {
            inner,
            name: name.to_string(),
            events: self.events.clone(),
        }))
    }

    /// Rename 成功后对「新名」发事件，模拟新版本出现。
    fn Rename(&self, ctx: &Context, old_file_name: &str, new_file_name: &str) -> Result<()> {
        self.storage.Rename(ctx, old_file_name, new_file_name)?;
        emitNewVersionEvent(ctx, &self.events, new_file_name)
    }

    /// 预签名透传。
    fn PresignFile(&self, ctx: &Context, file_name: &str, expire: Duration) -> Result<String> {
        self.storage.PresignFile(ctx, file_name, expire)
    }

    /// 关闭底层存储资源。
    fn Close(&self) {
        self.storage.Close();
    }
}

/// 流式写入包装：内容写完 Close 时才通知复制工人。
struct CrrEventWriter {
    inner: Box<dyn Writer>,
    // 对象名，Close 时用于发事件。
    name: String,
    // 有界发送端；与工人 Receiver 成对。
    events: Option<SyncSender<NewVersionCreatedEvent>>,
}

impl Writer for CrrEventWriter {
    /// 透传写缓冲；事件延后到 Close。
    fn Write(&mut self, ctx: &Context, p: &[u8]) -> Result<usize> {
        self.inner.Write(ctx, p)
    }

    fn Close(&mut self, ctx: &Context) -> Result<()> {
        self.inner.Close(ctx)?;
        // 与 WriteFile 路径一致：对象完整可见后再发 NewVersion。
        emitNewVersionEvent(ctx, &self.events, &self.name)
    }
}

/// Helper to create a bounded event channel matching Go buffer size 1024.
/// 容量 1024 对齐 Go 侧缓冲，避免测试中过早 Full。
pub fn new_event_channel() -> (
    SyncSender<NewVersionCreatedEvent>,
    Receiver<NewVersionCreatedEvent>,
) {
    std::sync::mpsc::sync_channel(1024)
}
