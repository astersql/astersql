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

// 可导入数据接口：TiKV write + ingest 路径共用的数据源、向前迭代器与取消上下文。
//
// 多个 regionJob 可共享同一 `IngestData`，因此引用计数与 `Finish` 需实现方提供
// 并发安全的内部可变性。`ForwardIter` 只能向前扫描键值批次。
//
// regionJob：按 Region 切分后的导入任务；TS：事务时间戳，此处同时作 start/commit TS。

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use membuf;

/// EngineError preserves Go's open `error` interface while requiring errors that
/// can safely cross the worker threads used by write-and-ingest jobs.
/// 引擎错误类型：对应 Go 开放的 `error`，且需能安全跨写导入工作线程传递。
pub type EngineError = Box<dyn Error + Send + Sync + 'static>;

/// Context is the cancellation subset of Go's context.Context needed by this API.
/// Clones share cancellation state, matching a context passed to multiple jobs.
/// 取消上下文：Go `context.Context` 的取消子集；克隆共享同一取消标志，可传给多个任务。
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// 共享的取消标志；置 true 后各任务应停止继续读取/写入。
    cancelled: Arc<AtomicBool>,
}

impl Context {
    /// 构造未取消的后台上下文（对应 Go `context.Background` 的取消语义子集）。
    pub fn background() -> Self {
        Self::default()
    }

    /// 发出取消信号，所有共享本 Context 的任务可见。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// 查询是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// IngestData 对应 TiKV write + ingest RPC 共同依赖的数据接口。
/// 多个 regionJob 可以共享同一实现，因此引用计数和 Finish 都需要实现方提供并发安全的内部可变性。
#[allow(non_snake_case)]
pub trait IngestData: Send + Sync {
    /// GetFirstAndLastKey 返回半开区间 `[lowerBound, upperBound)` 内的首尾键。
    /// 空切片对应 Go 的 empty/nil，表示该方向无边界；调用方必须保证 lowerBound < upperBound。
    /// 区间内没有数据时返回 `Ok((None, None))`，对应 Go 的 `nil, nil, nil`。
    fn GetFirstAndLastKey(
        &self,
        lowerBound: &[u8],
        upperBound: &[u8],
    ) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), EngineError>;

    /// NewIter 创建只能按批次向前读取 KV 的迭代器。
    /// 实现可从 bufPool 分配内存来延长一批键值的存活期；Close 或 ReleaseBuf 必须归还这些内存。
    /// Context 取消和底层 IO 错误由返回的迭代器观察，不在本接口这里执行。
    fn NewIter(
        &self,
        ctx: &Context,
        lowerBound: &[u8],
        upperBound: &[u8],
        bufPool: &mut membuf::Pool,
    ) -> Box<dyn ForwardIter>;

    /// GetTS 返回该数据同时用作 start TS 与 commit TS 的时间戳。
    fn GetTS(&self) -> u64;

    /// IncRef 必须在每个 regionJob 引用数据时调用；多个任务可以共享同一 IngestData。
    fn IncRef(&self);

    /// DecRef 与 IncRef 成对调用；实现应在引用归零时安全释放文件、reader 或缓存资源。
    fn DecRef(&self);

    /// Finish 在数据成功导入后累计完成的字节数和 KV 数。
    /// 一份 IngestData 可能被分段导入，因此该方法允许被并发或重复调用。
    fn Finish(&self, totalBytes: i64, totalCount: i64);
}

/// ForwardIter 对应只能向前移动的 Go 迭代器。
/// 它显式暴露 Close、Error 与 ReleaseBuf，以保留 Go 中迭代状态和缓冲区的独立收尾时机。
#[allow(non_snake_case)]
pub trait ForwardIter: Send {
    /// First 把迭代器移动到第一条键值，返回是否存在有效当前位置。
    fn First(&mut self) -> bool;

    /// Valid 判断当前位置是否尚未越过末尾。
    fn Valid(&self) -> bool;

    /// Next 只向前移动一项，返回移动后的状态。
    fn Next(&mut self) -> bool;

    /// Key 返回当前位置的键。
    /// Go 返回的切片可跨后续 Next/Key 调用继续访问，但 Close 或 ReleaseBuf 后立即失效；
    /// Rust 后续接线需用缓冲区 guard 或等价所有权模型表达这一生命周期，不能返回临时切片。
    fn Key(&self) -> &[u8];

    /// Value 与 Key 共享相同的批次缓冲区存活规则，Close/ReleaseBuf 后不得再次访问旧值。
    fn Value(&self) -> &[u8];

    /// Close 关闭迭代器并释放其持有的 reader 和缓冲区；底层 IO 关闭失败通过 Result 返回。
    fn Close(&mut self) -> Result<(), EngineError>;

    /// Error 返回迭代过程中记录的当前错误；None 对应 Go 的 nil error。
    fn Error(&self) -> Option<&EngineError>;

    /// ReleaseBuf 只释放保存此前 Key/Value 的内存，不隐式关闭迭代器。
    /// 调用后所有先前返回的键和值均不可再访问，后续读取可重新从 bufPool 申请空间。
    fn ReleaseBuf(&mut self);
}
