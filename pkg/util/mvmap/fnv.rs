// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Copyright 2011 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// FNV-1 64 位哈希，供 MVMap 将 key 映射到 hash 桶。
//
// 对应 Go `pkg/util/mvmap` 中的 `fnvHash64`（源自 Go 标准库 hash/fnv）：
// 无内部可变状态，仅对输入字节做乘法与异或混合。

/// OFFSET64 对应 Go 文件中的 FNV-1 64 位初始偏移量。
// OFFSET64 对应 Go 文件中的 FNV-1 64 位初始偏移量。
const OFFSET64: u64 = 14695981039346656037;

/// PRIME64 对应 Go 文件中的 FNV-1 64 位乘数。
// PRIME64 对应 Go 文件中的 FNV-1 64 位乘数。
const PRIME64: u64 = 1099511628211;

/// 对字节切片计算 FNV-1 64 位哈希，供 MVMap 的 hash 表索引使用。
// fnvHash64 is ported from go library, which is thread-safe.
// fnvHash64 按 Go 标准库迁移的字节切片哈希函数。
// 函数只读取传入切片并返回 u64，不保存共享状态，因此沿用 Go 注释里的线程安全语义。
pub(crate) fn fnv_hash64(data: &[u8]) -> u64 {
    let mut hash = OFFSET64;
    // Go 的 uint64 乘法溢出按 2^64 回绕；显式使用 wrapping_mul 保留该语义。
    for c in data {
        hash = hash.wrapping_mul(PRIME64);
        hash ^= u64::from(*c);
    }
    hash
}
