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

#![allow(non_snake_case)]

// 导入测试用的可追踪内存对象存储。
//
// `TrackOpenMemStorage` 在 `MemStorage` 外包一层原子计数，记录当前尚未关闭的
// reader（`Opened`）与累计打开尝试（`TotalOpened`），便于单测断言资源生命周期。
// 对象存储：外部键值文件抽象；此处为内存实现，无真实网络 IO。

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use objstore::storage::Storage as _;

/// TrackOpenMemStorage 对应 Go 的同名结构体：在 MemStorage 外增加两个并发安全的打开计数器。
/// Go 的匿名指针嵌入在 Rust 中显式保存为 Arc，使返回的 reader 能继续共享存储包装器的生命周期。
pub struct TrackOpenMemStorage {
    /// 底层内存对象存储。
    pub MemStorage: Arc<objstore::memstore::MemStorage>,
    /// 当前尚未成功 Close 的 reader 数量。
    pub Opened: AtomicI32,
    /// 进程内累计发起的 Open 次数（含失败）。
    pub TotalOpened: AtomicI32,
}

impl TrackOpenMemStorage {
    /// Open 对应 Go 的文件打开方法：先记录一次尝试，再委托底层内存存储创建 reader。
    pub fn Open(
        self: &Arc<Self>,
        ctx: &storeapi::Context,
        path: &str,
        opt: Option<&storeapi::ReaderOption>,
    ) -> io::Result<Box<dyn objectio::Reader>> {
        // 两个计数器分别表示当前尚未关闭的 reader 和进程内累计发起的打开次数。
        self.Opened.fetch_add(1, Ordering::SeqCst);
        self.TotalOpened.fetch_add(1, Ordering::SeqCst);

        // Go 的 MemStorage.Open 原样返回 context.Canceled；用 Interrupted 保留
        // Rust 侧可判别的取消分类，同时仍按失败 Open 回滚当前打开数。
        if ctx.is_cancelled() {
            self.Opened.fetch_sub(1, Ordering::SeqCst);
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }

        let storage_ctx = objstore::storage::Context::background();
        let storage_opt = opt.map(|option| objstore::storage::ReaderOption {
            start_offset: option.StartOffset,
            end_offset: option.EndOffset,
        });

        let reader = match self
            .MemStorage
            .Open(&storage_ctx, path, storage_opt.as_ref())
        {
            Ok(reader) => reader,
            Err(err) => {
                // 底层 Open 失败时没有 reader 可供 Close，因此只回滚当前打开数；累计尝试数保持递增。
                self.Opened.fetch_sub(1, Ordering::SeqCst);
                return Err(io::Error::other(err.to_string()));
            }
        };

        // Go 返回 TrackOpenFileReader 指针；Box trait object 保留相同的动态 Reader 返回形状。
        Ok(Box::new(TrackOpenFileReader {
            Reader: Box::new(ObjectReaderAdapter { inner: reader }),
            store: Arc::clone(self),
        }))
    }
}

/// ObjectReaderAdapter 连接已迁移 objstore MemStorage 与 objectio 的公共 Reader 接口。
struct ObjectReaderAdapter {
    inner: Box<dyn objstore::storage::ObjectReader>,
}

impl Read for ObjectReaderAdapter {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for ObjectReaderAdapter {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl objectio::Reader for ObjectReaderAdapter {
    fn close(&mut self) -> io::Result<()> {
        self.inner
            .close()
            .map_err(|err| io::Error::other(err.to_string()))
    }

    fn file_size(&self) -> io::Result<i64> {
        self.inner
            .get_file_size()
            .map_err(|err| io::Error::other(err.to_string()))
    }
}

/// TrackOpenFileReader 对应 Go 的 reader 包装器，保存底层 Reader 以及所属的计数存储。
pub struct TrackOpenFileReader {
    /// 被包装的底层对象 reader。
    pub Reader: Box<dyn objectio::Reader>,
    store: Arc<TrackOpenMemStorage>,
}

impl Read for TrackOpenFileReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.Reader.read(buf)
    }
}

impl Seek for TrackOpenFileReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.Reader.seek(pos)
    }
}

impl objectio::Reader for TrackOpenFileReader {
    /// Close 对应 Go 的资源收尾顺序：必须先成功关闭底层 reader，随后才能减少当前打开数。
    fn close(&mut self) -> io::Result<()> {
        if let Err(err) = self.Reader.close() {
            // 关闭失败时保持 Opened 不变，表示该 reader 尚未被确认释放，并原样向调用方传播错误。
            return Err(err);
        }

        self.store.Opened.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    fn file_size(&self) -> io::Result<i64> {
        self.Reader.file_size()
    }
}
