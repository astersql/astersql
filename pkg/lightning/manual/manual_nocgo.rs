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

// 无 cgo 构建下的手动分配回退实现。
//
// 当未启用 `cgo` feature 时编译本模块，用纯 Rust `Vec` 模拟 Go `make([]byte, n)`，
// 不依赖 C 侧 `malloc`/`free`。

#![cfg(not(feature = "cgo"))]

/// New 对应无 cgo 构建下的 Go `make([]byte, n)`：创建长度为 n 且内容清零的字节缓冲区。
pub fn New(n: isize) -> Vec<u8> {
    // Go 在负长度时由运行时失败；这里保留转换前检查，避免负数静默变成巨大的 usize。
    let len = usize::try_from(n).expect("makeslice: len out of range");
    vec![0; len]
}

/// Free 对应无 cgo 版本的空函数。
/// Go 依赖垃圾回收器回收切片；Rust 通过取得所有权并在函数返回时 Drop，表达相同的无需手工 C.free 语义。
pub fn Free(bytes: Vec<u8>) {
    drop(bytes);
}
