// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 低层“零拷贝”字符串/字节互转与 map 桶内存估算常量（hack 工具集）。
//
// 由 `pkg/util/hack/hack.go` 迁移。Go 侧依赖 unsafe 别名；Rust 侧用 `&str`/`&[u8]`
// 表达同等意图，并保留 swiss map 默认桶占用常量供内存追踪。

// 本文件由 pkg/util/hack/hack.go 迁移而来，保留 Go 实现结构。
// 也不会执行真实业务动作；unsafe 片段只用于标注 Go 原实现依赖的内存别名语义。

use std::slice;
use std::str;

/// 可无拷贝当作字符串使用的视图类型（Go 中为 `string` 新类型）。
// MutableString can be used as string via string(MutableString) without performance loss.
// MutableString 在 Go 中是 string 的新类型；用 &str 表达“不拷贝地视为字符串”的意图。
pub type MutableString<'a> = &'a str;

/// 将字节切片解释为字符串视图（不拷贝）；调用方须保证合法 UTF-8 与生命周期安全。
// String converts slice to MutableString without copy.
// The MutableString can be converts to string without copy.
// Use it at your own risk.
// String 对应 Go 的 unsafe.String：把字节切片直接解释为字符串，不复制底层内存。
pub unsafe fn String(b: &[u8]) -> MutableString<'_> {
    if b.is_empty() {
        return "";
    }
    // 这里要求调用方保证 b 是合法 UTF-8 且生命周期内不被破坏；这正是 Go 注释里的风险点。
    unsafe { str::from_utf8_unchecked(b) }
}

/// 将字符串转为共享底层内存的字节切片视图（不拷贝）。
// Slice converts string to slice without copy.
// Use at your own risk.
// Slice 对应 Go 的 unsafe.Slice(unsafe.StringData(s), len(s))，返回与字符串共享底层内存的字节视图。
pub fn Slice(s: &str) -> &[u8] {
    s.as_bytes()
}

/// 包初始化入口：与 Go 一样调用当前版本的 map ABI 检查。
// init 对应 Go init，启动时检查 map ABI 是否匹配当前 Go 版本。
pub fn init() {
    crate::map_abi_go126::checkMapABI();
}

/// 由裸指针与长度构造字节切片；长度非零时指针须有效且不越界。
// GetBytesFromPtr return a bytes array from the given ptr and length
// GetBytesFromPtr 从裸指针和长度构造字节切片；非空时调用方必须保证指针有效且长度不越界。
pub unsafe fn GetBytesFromPtr<'a>(ptr: *const u8, length: usize) -> &'a [u8] {
    // Go's unsafe.Slice permits a nil pointer for a zero-length slice, while
    // Rust requires even an empty slice's pointer to be non-null.
    if length == 0 {
        return &[];
    }
    unsafe { slice::from_raw_parts(ptr, length) }
}

// Memory usage constants for swiss map
// 以下常量对应 Go swiss map 的默认桶内存估算值，供上层内存统计逻辑使用。
/// `map[string]any` 默认桶内存估算（字节）。
pub const DefBucketMemoryUsageForMapStringToAny: usize = 312;
/// `map[string]struct{}`（集合）默认桶内存估算。
pub const DefBucketMemoryUsageForSetString: usize = 248;
/// `map[float64]struct{}` 默认桶内存估算。
pub const DefBucketMemoryUsageForSetFloat64: usize = 184;
/// `map[int64]struct{}` 默认桶内存估算。
pub const DefBucketMemoryUsageForSetInt64: usize = 184;
/// `map[string]Decimal` 默认桶内存估算。
pub const DefBucketMemoryUsageForMapStringToDecimal: usize = 248;
/// `map[string]string` 默认桶内存估算。
pub const DefBucketMemoryUsageForMapStringToString: usize = 312;
