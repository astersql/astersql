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

// Cascades 基础哈希器（Hasher）：以 FNV-1a 对表达式/算子字段做增量摘要。
//
// 用于 Memo 中快速判定候选是否可能相等；真正相等性由 `Equals` 在哈希冲突后二次确认。
// FNV-1a（Fowler–Noll–Vo）是非加密哈希：速度快、实现简单，适合优化器内部键比较。

// 本文件由 pkg/planner/cascades/base/hash_equaler.go 迁移而来，保留 Go 的声明与方法顺序。
// 本实现以内存中的 FNV-1a 增量哈希管理临时字节缓存，不产生外部 IO。

/// FNV-1a 64 位初始偏移量，移植自 Go 标准库 fnv.go。
#[allow(non_upper_case_globals)]
const offset64: u64 = 14_695_981_039_346_656_037;

/// FNV-1a 64 位质数，移植自 Go 标准库 fnv.go。
#[allow(non_upper_case_globals)]
const prime64: u64 = 1_099_511_628_211;

/// Hasher 对应 Go 同名接口，为优化器表达式和算子提供基础类型哈希入口。
#[allow(non_snake_case)]
pub trait Hasher {
    fn HashBool(&mut self, val: bool);
    fn HashInt(&mut self, val: isize);
    fn HashInt64(&mut self, val: i64);
    fn HashUint64(&mut self, val: u64);
    fn HashFloat64(&mut self, val: f64);
    fn HashRune(&mut self, val: i32);
    fn HashString(&mut self, val: &str);
    fn HashByte(&mut self, val: u8);
    fn HashBytes(&mut self, val: &[u8]);
    fn Reset(&mut self);
    fn SetCache(&mut self, cache: Vec<u8>);
    fn Cache(&mut self) -> &mut [u8];
    fn Sum64(&self) -> u64;
}

/// 指针或接口字段为空时写入 NilFlag，非空时先写入 NotNilFlag 再写字段内容。
/// 非空标记不能省略，否则内容恰好为单字节 0 时会与 nil 的哈希序列冲突。
#[allow(non_upper_case_globals)]
pub const NilFlag: u8 = 0;
#[allow(non_upper_case_globals)]
pub const NotNilFlag: u8 = 1;

/// Hash64a 对应 Go 的命名 uint64 类型，用于保存逐步累积的哈希值。
pub type Hash64a = u64;

/// hasher 对应 Go 的同名私有结构。
/// cache 供 datum 等临时编码复用，Reset 只清空长度并保留已经分配的容量。
#[allow(non_camel_case_types)]
pub struct hasher {
    hash64a: Hash64a,
    cache: Vec<u8>,
}

/// NewHashEqualer 对应 Go 构造函数，以 FNV-1a offset 初始化摘要。
#[allow(non_snake_case)]
pub fn NewHashEqualer() -> Box<dyn Hasher> {
    Box::new(hasher {
        hash64a: offset64,
        cache: Vec::new(),
    })
}

impl hasher {
    /// 把一个已经转换成 u64 的基础值混入摘要。
    /// Go 的无符号乘法会模 2^64 回绕，Rust 必须使用 wrapping_mul 才能在调试构建中保持一致。
    fn mix(&mut self, value: u64) {
        self.hash64a ^= value;
        self.hash64a = self.hash64a.wrapping_mul(prime64);
    }
}

#[allow(non_snake_case)]
impl Hasher for hasher {
    /// Reset 对应 Go Reset：恢复初始 offset，并复用缓存容量。
    fn Reset(&mut self) {
        self.hash64a = offset64;
        self.cache.clear();
    }

    /// Cache 返回内部缓存的可变切片；调用方可像 Go []byte 一样复用内容，但所有权仍由哈希器持有。
    fn Cache(&mut self) -> &mut [u8] {
        &mut self.cache
    }

    /// SetCache 对应 Go 直接替换内部 []byte，Rust 通过 Vec 转移缓存所有权。
    fn SetCache(&mut self, cache: Vec<u8>) {
        self.cache = cache;
    }

    /// Sum64 返回当前累计摘要，不重置内部状态。
    fn Sum64(&self) -> u64 {
        self.hash64a
    }

    /// HashBool 把 false/true 分别映射为 0/1 后执行一次 FNV-1a 混合。
    fn HashBool(&mut self, val: bool) {
        self.mix(u64::from(val));
    }

    /// HashInt 保留 Go `int -> Hash64a` 的补码转换效果。
    fn HashInt(&mut self, val: isize) {
        self.mix(val as u64);
    }

    /// HashInt64 保留负数转无符号数时模 2^64 的 Go 转换语义。
    fn HashInt64(&mut self, val: i64) {
        self.mix(val as u64);
    }

    /// HashUint64 直接混入无符号整数。
    fn HashUint64(&mut self, val: u64) {
        self.mix(val);
    }

    /// HashFloat64 对应 math.Float64bits，按 IEEE-754 原始位模式哈希而非数值格式化结果。
    fn HashFloat64(&mut self, val: f64) {
        self.mix(val.to_bits());
    }

    /// HashRune 直接混入 Go rune 的 int32 位值；负值转换时同样保留补码语义。
    fn HashRune(&mut self, val: i32) {
        self.mix(val as u64);
    }

    /// HashString 先写入 Go `len(string)` 的 UTF-8 字节数，再依次写入每个 rune。
    /// 例如中文字符通常占 3 个 UTF-8 字节，因此不能用 chars().count() 代替 len。
    fn HashString(&mut self, val: &str) {
        self.HashInt(val.len() as isize);
        for rune in val.chars() {
            self.HashRune(rune as i32);
        }
    }

    /// HashByte 与 Go 一样先把 byte 扩展为 rune，再走相同混合路径。
    fn HashByte(&mut self, val: u8) {
        self.HashRune(i32::from(val));
    }

    /// HashBytes 先写切片长度，再按原顺序逐字节混入，防止不同分段产生相同序列解释。
    fn HashBytes(&mut self, val: &[u8]) {
        self.HashInt(val.len() as isize);
        for &byte in val {
            self.HashByte(byte);
        }
    }
}

// 与 Go 文件末尾的示例一致：组合对象应按字段顺序调用上述基础方法；指针字段还需先写 nil 标记。
// 相等性比较由 base.rs 的 Equals trait 承担，仅在摘要冲突后执行，不属于本哈希器的职责。
