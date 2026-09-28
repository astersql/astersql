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

// 可复用的压缩输出缓冲：写入明文，内部经压缩 Writer 落到共享字节区。
//
// 供对象存储分块上传等场景收集已压缩字节，支持 reset/flush/close。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use super::{CompressType, Flusher, Writer, new_writer};

#[derive(Clone)]
/// 线程安全的底层字节容器，可被压缩 Writer 与 `Buffer` 同时持有。
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    /// 预分配容量，减少压缩过程中的反复扩容。
    fn with_capacity(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(Vec::with_capacity(capacity))))
    }

    /// 获取互斥锁；若曾被毒化则吞掉毒化并继续使用内层数据。
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 可复用缓冲：保存压缩 Writer 写出的压缩字节。
/// A reusable buffer containing the compressed bytes emitted by its writer.
pub struct Buffer {
    buffer: SharedBuffer,
    writer: Option<Box<dyn Writer>>,
    capacity: usize,
}

impl Buffer {
    /// 当前已缓冲的压缩字节数。
    pub fn len(&self) -> usize {
        self.buffer.lock().len()
    }

    /// 缓冲是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 构造时声明的容量（未必等于底层 Vec 容量）。
    pub fn cap(&self) -> usize {
        self.capacity
    }

    /// 清空缓冲内容，保留 Writer 以便继续写入。
    pub fn reset(&mut self) {
        self.buffer.lock().clear();
    }

    /// 刷新压缩器内部缓冲到共享字节区；无压缩 Writer 时与 Go 一样 panic。
    pub fn flush(&mut self) -> io::Result<()> {
        let writer = self.writer.as_mut().expect("compressed writer is nil");
        Flusher::flush(writer.as_mut())
    }

    /// 关闭压缩 Writer，写出尾部帧；无压缩 Writer 时与 Go 一样 panic。
    pub fn close(&mut self) -> io::Result<()> {
        self.writer
            .as_mut()
            .expect("compressed writer is nil")
            .close()
    }

    /// 恒为 true，表示本缓冲始终走压缩路径。
    pub fn compressed(&self) -> bool {
        true
    }

    /// 克隆当前压缩字节快照。
    pub fn bytes(&self) -> Vec<u8> {
        self.buffer.lock().clone()
    }
}

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer
            .as_mut()
            .expect("compressed writer is nil")
            .write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Buffer::flush(self)
    }
}

/// 按块大小与压缩类型创建缓冲；无压缩时 `writer` 为 `None`。
pub fn new_buffer(chunk_size: usize, compress_type: CompressType) -> Buffer {
    let buffer = SharedBuffer::with_capacity(chunk_size);
    let writer = new_writer(compress_type, Box::new(buffer.clone()));
    Buffer {
        buffer,
        writer,
        capacity: chunk_size,
    }
}
