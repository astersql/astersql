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

// 对象 I/O 基础接口：取消上下文、流式 Reader / Writer。
//
// 对应 Go 侧 objectio 包中与外部文件读写相关的最小抽象；存储实现在远程
// I/O 前可检查 `Context` 是否已取消。

use std::io::{self, Read, Seek};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A small cancellation token corresponding to the part of Go's context used
/// by object writers. Storage implementations can check it before remote I/O.
/// 对象写入侧使用的轻量取消令牌（对应 Go context 的取消部分）；
/// 存储实现可在远程 I/O 前检查是否已取消。
#[derive(Clone, Default)]
pub struct Context {
    /// 是否已请求取消（跨克隆共享）。
    cancelled: Arc<AtomicBool>,
}

impl Context {
    /// 与上层任务共用取消标志，使存储 I/O 可直接观察任务关闭。
    pub fn from_cancellation_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    /// Wait for cancellation while an object-store request is in flight.
    pub async fn wait_cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// 标记为已取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// 若已取消则返回 Interrupted 错误，否则 Ok。
    pub fn check(&self) -> io::Result<()> {
        if self.is_cancelled() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ))
        } else {
            Ok(())
        }
    }
}

/// Streaming external-file reader, equivalent to Go's ReadSeekCloser plus
/// `GetFileSize`.
/// 流式外部文件读取器，等价于 Go 的 ReadSeekCloser 加上 `GetFileSize`。
pub trait Reader: Read + Seek {
    /// 关闭底层资源。
    fn close(&mut self) -> io::Result<()>;
    /// 返回对象字节大小。
    fn file_size(&self) -> io::Result<i64>;

    /// Go 风格别名，转发到 `close`。
    #[allow(non_snake_case)]
    fn Close(&mut self) -> io::Result<()> {
        self.close()
    }

    /// Go 风格别名，转发到 `file_size`。
    #[allow(non_snake_case)]
    fn GetFileSize(&self) -> io::Result<i64> {
        self.file_size()
    }
}

/// Streaming external-file writer. A write may synchronously upload a full
/// chunk; close uploads the final partial chunk and completes the object.
/// 流式外部文件写入器。`write` 可能同步上传满块；`close` 上传末块并完成对象。
pub trait Writer {
    /// 写入数据；可能触发整块上传。
    fn write(&mut self, ctx: &Context, data: &[u8]) -> io::Result<usize>;
    /// 刷出尾块并完成对象。
    fn close(&mut self, ctx: &Context) -> io::Result<()>;

    /// Go 风格别名，转发到 `write`。
    #[allow(non_snake_case)]
    fn Write(&mut self, ctx: &Context, data: &[u8]) -> io::Result<usize> {
        self.write(ctx, data)
    }

    /// Go 风格别名，转发到 `close`。
    #[allow(non_snake_case)]
    fn Close(&mut self, ctx: &Context) -> io::Result<()> {
        self.close(ctx)
    }
}

/// Bind the caller's cancellation context once for standard streaming encoders.
pub struct IOWriter<'a> {
    context: Context,
    writer: &'a mut dyn Writer,
}
#[allow(non_snake_case)]
pub fn NewIOWriter(context: Context, writer: &mut dyn Writer) -> IOWriter<'_> {
    IOWriter { context, writer }
}
impl io::Write for IOWriter<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.writer.write(&self.context, data)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
