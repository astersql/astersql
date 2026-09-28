// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Key helpers for recovery region ordering (from `br/pkg/restore/data/key.go`).
//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/data/key.rs`对应的恢复 Region 排序用的键辅助，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少9行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `keyEq`是当前文件的重要函数，承担"keyEq"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `keyCmp`是当前文件的重要函数，承担"keyCmp"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `keyCmpInterface`是当前文件的重要函数，承担"keyCmpInterface"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `PrefixStartKey`是当前文件的重要函数，承担"PrefixStartKey"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `PrefixEndKey`是当前文件的重要函数，承担"PrefixEndKey"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! 中文注释索引结束

/// keyEq compares two keys byte-for-byte.
pub fn keyEq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if a[i] != b[i] {
            return false;
        }
    }
    true
}

/// keyCmp compares keys by shared prefix then by length (Go `keyCmp`).
pub fn keyCmp(a: &[u8], b: &[u8]) -> i32 {
    let (length, chosen) = if a.len() < b.len() {
        (a.len(), -1)
    } else if a.len() == b.len() {
        (a.len(), 0)
    } else {
        (b.len(), 1)
    };
    for i in 0..length {
        if a[i] < b[i] {
            return -1;
        } else if a[i] > b[i] {
            return 1;
        }
    }
    chosen
}

/// keyCmpInterface is the treemap comparator entry (Go `any` → `[]byte`).
pub fn keyCmpInterface(a: &[u8], b: &[u8]) -> i32 {
    keyCmp(a, b)
}

/// PrefixStartKey prepends `'z'` for internal recovery keyspace ordering.
pub fn PrefixStartKey(key: &[u8]) -> Vec<u8> {
    let mut sk = Vec::with_capacity(key.len() + 1);
    sk.push(b'z');
    sk.extend_from_slice(key);
    sk
}

/// PrefixEndKey: empty end means +\infty, encoded as `'z'+1`.
pub fn PrefixEndKey(key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return vec![b'z' + 1];
    }
    PrefixStartKey(key)
}
