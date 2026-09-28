// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Cascades util 包 StrBuffer 的迁移回归测试。
//
// 核对 Flush 前内容停留在缓冲、写入顺序保持，以及 NewStrBuffer 可接受
// 借用型 writer（对齐 Go `io.Writer` 用法）。

#![allow(dead_code, non_snake_case)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::NewStrBuffer;

/// 可共享的内存 writer：多处句柄写入同一 Vec，便于断言缓冲行为。
#[derive(Clone, Default)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 连续 WriteString 在 Flush 前不落底层，Flush 后按写入顺序可见。
#[test]
fn write_string_stays_buffered_until_flush_and_preserves_order() {
    let output = SharedWriter::default();
    let observed = Arc::clone(&output.0);
    let mut writer = NewStrBuffer(output);

    writer.WriteString("memo");
    writer.WriteString(" -> group");
    // Flush 前底层应仍为空（内容在 BufWriter 中）。
    assert!(observed.lock().unwrap().is_empty());

    writer.Flush();
    assert_eq!(&*observed.lock().unwrap(), b"memo -> group");
}

/// NewStrBuffer 接受 `&mut Vec<u8>` 这类借用 writer，语义对齐 Go io.Writer。
#[test]
fn new_str_buffer_accepts_a_borrowed_writer_like_go_io_writer() {
    let mut output = Vec::new();
    {
        let mut writer = NewStrBuffer(&mut output);
        writer.WriteString("borrowed");
        writer.Flush();
    }

    assert_eq!(output, b"borrowed");
}
