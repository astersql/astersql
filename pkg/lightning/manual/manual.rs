// Copyright 2026 AsterSQL.
// Copyright 2020 The LevelDB-Go and Pebble Authors. All rights reserved. Use
// of this source code is governed by a BSD-style license that can be found in
// the LICENSE file.
//
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

// 手动字节缓冲分配：对应 Go `manual.New`/`Free` 与最大切片长度常量。
//
// 源自 LevelDB-Go/Pebble 风格的手动管理；Rust 用 `Vec<u8>` 表达切片，
// `Free` 通过取得所有权并 `drop` 释放，对齐 Go 侧显式归还语义。

/// MaxArrayLen 对应 Go 在当前架构上可安全构造的最大切片长度。
pub const MaxArrayLen: usize = (1usize << 31) - 1;

/// New 对应 Go 的手动分配函数：申请清零内存，并返回长度和容量均为 n 的字节缓冲区。
pub fn New(n: isize) -> Vec<u8> {
    // 零长度与 Go `make([]byte, 0)` 一致，返回无后备存储的空切片。
    if n == 0 {
        return Vec::new();
    }

    // 负数或超出 MaxArrayLen 时 panic，对应 Go runtime 的 makeslice 范围检查。
    let len = usize::try_from(n).expect("makeslice: len out of range");
    assert!(len <= MaxArrayLen, "makeslice: len out of range");
    vec![0; len]
}

/// Free 对应 Go 的释放函数。取得所有权后在返回时释放底层缓冲区。
pub fn Free(bytes: Vec<u8>) {
    drop(bytes);
}
