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

// Cascades 描述文本用的带缓冲字符串 writer。
//
// StrBufferWriter 隐藏写入长度与错误返回（对齐 Go 无 error 接口）；
// StrBuffer 用 BufWriter 批量缓冲底层 `io::Write`，供 Memo/Task Desc 等调试输出。

// 它为级联优化器的描述文本提供带缓冲的字符串 writer，并统一隐藏写入长度与错误返回值。

use std::io::{BufWriter, Write};

/// StrBufferWriter 对应 Go 的同名接口，让调用方只关心字符串写入和最终刷新。
pub trait StrBufferWriter {
    /// 追加一段字符串到缓冲（不保证立即落底层）。
    fn WriteString(&mut self, s: &str);
    /// 把缓冲内容提交给底层 writer。
    fn Flush(&mut self);
}

/// StrBuffer 对应 Go 对 `bufio.Writer` 的轻量包装。
/// 泛型 W 保留 io.Writer 可替换性，BufWriter 负责批量缓冲底层 IO。
pub struct StrBuffer<W: Write> {
    bio: BufWriter<W>,
}

/// NewStrBuffer 对应 Go 构造函数，为传入的 writer 创建新的缓冲层并返回接口对象。
/// writer 的所有权交给 StrBuffer；刷新仍需由调用方显式调用 Flush。
pub fn NewStrBuffer<'a, W>(w: W) -> Box<dyn StrBufferWriter + 'a>
where
    W: Write + 'a,
{
    Box::new(StrBuffer {
        bio: BufWriter::new(w),
    })
}

impl<W: Write> StrBufferWriter for StrBuffer<W> {
    /// WriteString 对应 Go 的实现，忽略成功时的已写字节数。
    fn WriteString(&mut self, s: &str) {
        let result = self.bio.write_all(s.as_bytes());
        // Go 用 intest.Assert 认定测试中的缓冲写入不应失败；这里保留同样的立即断言语义。
        assert!(
            result.is_ok(),
            "buffer-io WriteString should be no error in test"
        );
    }

    /// Flush 对应 Go 的实现，把内存缓冲区内容提交给底层 writer。
    fn Flush(&mut self) {
        let result = self.bio.flush();
        // 刷新错误不向上传播，与 Go 接口无 error 返回保持一致；测试路径通过断言暴露错误。
        assert!(result.is_ok(), "buffer-io Flush should be no error in test");
    }
}
