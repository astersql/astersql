// Copyright 2009 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.
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

// Copyright 2026 AsterSQL.

// 字节集合与扫描工具，对应 Go `bytes` 包中的 `byteSet` / `IndexAnyByte`。
//
// 用 8 个 `u32`（共 256 bit）表示 ASCII/字节是否命中，供 mydump 解析器
// 在数据流中快速查找分隔符等特殊字节。

/// ByteSet 对应 Go `byteSet [8]uint32`：256 个 bit 分别表示一个 byte 是否存在。
#[derive(Clone, Copy, Default)]
pub struct ByteSet([u32; 8]);

/// make_byte_set 对应 Go makeByteSet，将每个字节拆成桶下标和桶内 bit。
pub fn make_byte_set(chars: &[u8]) -> ByteSet {
    let mut set = ByteSet::default();
    for &byte in chars {
        // 高 3 bit 选择 8 个 u32 桶，低 5 bit 选择桶内位置。
        set.0[(byte >> 5) as usize] |= 1_u32 << (byte & 31);
    }
    set
}

/// Go 风格命名别名，转发到 `make_byte_set`。
pub fn makeByteSet(chars: &[u8]) -> ByteSet {
    make_byte_set(chars)
}

impl ByteSet {
    /// contains 对应 Go 指针接收者方法；查询不修改集合，因此 Rust 使用共享借用。
    pub fn contains(&self, byte: u8) -> bool {
        (self.0[(byte >> 5) as usize] & (1_u32 << (byte & 31))) != 0
    }
}

/// index_any_byte 对应 Go IndexAnyByte，返回首个命中字节的位置；未命中保留 -1 哨兵。
pub fn index_any_byte(bytes: &[u8], set: &ByteSet) -> isize {
    for (index, &byte) in bytes.iter().enumerate() {
        if set.contains(byte) {
            return index as isize;
        }
    }
    -1
}

/// Go 风格命名别名，转发到 `index_any_byte`。
pub fn IndexAnyByte(bytes: &[u8], set: &ByteSet) -> isize {
    index_any_byte(bytes, set)
}
