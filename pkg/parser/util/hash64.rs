// Copyright 2026 AsterSQL.
// Copyright 2025 The ql Authors. All rights reserved.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 64 位增量哈希接口，对照 Go 的 `hash64.go` / cascades `base.Hasher`。
//
// 规划器（optimizer）对象通过逐字段写入保证稳定哈希，用于等价类判定与缓存键；
// 本模块只声明协议，不提供具体哈希算法实现。

// 本文件由 pkg/parser/util/hash64.go 迁移而来，保留 cascades/base.Hasher 的内部抽象。
// 该实现声明增量哈希接口，不创建哈希器、。
// IHasher 对应 Go 同名接口，为规划器对象提供稳定的逐字段 64 位哈希写入协议。
// Hash* 与 Reset 会改变累积状态，因此使用 &mut self；Sum64 只读取当前摘要。
/// IHasher 是规划器对象的逐字段 64 位哈希写入协议。
pub trait IHasher {
    /// HashBool 写入布尔值。
    // HashBool 写入布尔值。
    fn HashBool(&mut self, val: bool);
    /// HashInt 写入与平台指针宽度一致的 Go int（映射为 isize）。
    // HashInt 写入与平台指针宽度一致的 Go int 映射。
    fn HashInt(&mut self, val: isize);
    /// HashInt64 写入有符号 64 位整数。
    // HashInt64 写入有符号 64 位整数。
    fn HashInt64(&mut self, val: i64);
    /// HashUint64 写入无符号 64 位整数。
    // HashUint64 写入无符号 64 位整数。
    fn HashUint64(&mut self, val: u64);
    /// HashFloat64 写入 IEEE 754 双精度值；位级编码由实现决定。
    // HashFloat64 写入 IEEE 754 双精度值，具体位级规范由实现负责。
    fn HashFloat64(&mut self, val: f64);
    /// HashRune 写入 Go rune 的完整 int32 取值域。
    // HashRune 写入 Go rune 的完整 int32 取值域。
    fn HashRune(&mut self, val: i32);
    /// HashString 写入 UTF-8 字符串内容。
    // HashString 写入 UTF-8 字符串内容。
    fn HashString(&mut self, val: &str);
    /// HashByte 写入单个原始字节。
    // HashByte 写入单个原始字节。
    fn HashByte(&mut self, val: u8);
    /// HashBytes 写入字节切片；借用避免复制，顺序由实现保留。
    // HashBytes 写入字节切片；借用避免无意义复制，顺序由实现保留。
    fn HashBytes(&mut self, val: &[u8]);
    /// Reset 清空累积状态，使同一实例可复用。
    // Reset 清空累积状态，使同一实例可复用。
    fn Reset(&mut self);
    /// Sum64 返回当前 64 位摘要，不消费或重置哈希器。
    // Sum64 返回当前 64 位摘要，不消费或重置哈希器。
    fn Sum64(&self) -> u64;
}
