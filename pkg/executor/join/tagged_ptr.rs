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

// Tagged Pointer（带标签指针）工具：把哈希值的高位 tag 与行地址拼进同一个整数。
//
// 哈希连接（Hash Join）探测阶段用链表串联同桶候选行；为减少一次间接访问，
// Go/TiDB 把部分 hash tag 嵌入指针高位。本模块只做位运算封装，不解引用真实地址。

// 哈希连接如何把指针和 hash tag 合并到一个整数形态中；不会解引用真实地址或执行业务动作。

/// 可嵌入指针的最大 tag 位数（与 Go `maxTaggedBits` 一致）。
pub const MAX_TAGGED_BITS: i8 = 24;
/// 低 40 位掩码：64 - 24，用于与高位 tag 掩码互补。
pub const MAX_TAGGED_MASK: u64 = 0xffffffffff;

// taggedPointerLen 对应 Go 的 unsafe.Sizeof(taggedPtr(0))。
// 这里用 usize 的尺寸表达机器字长；真实布局仍依赖后续 unsafe 接线。
/// taggedPtr 的机器字长（字节数），对应 Go `unsafe.Sizeof(taggedPtr(0))`。
pub const TAGGED_POINTER_LEN: i64 = std::mem::size_of::<TaggedPtr>() as i64;

// taggedPtr 对应 Go 的 uintptr 别名。
// Go 通过 uintptr 避开 GC 对 unsafe.Pointer 的校验；只保留位运算形状。
/// 带标签指针的整数形态；对应 Go 的 `uintptr`/`taggedPtr`。
pub type TaggedPtr = usize;

// tagPtrHelper 保存用于抽取 tag 高位的掩码。
/// 维护高位 tag 掩码，负责指针与 tag 的嵌入/剥离。
pub struct TagPtrHelper {
    // taggedMask 对应 Go 字段：从 taggedPtr 中取出高位 tag。
    /// 从 taggedPtr 中取出高位 tag 的掩码。
    pub tagged_mask: u64,
}

impl TagPtrHelper {
    // init 对应 Go 的 (*tagPtrHelper).init：根据可用 tag 位数构造高位掩码。
    /// 按可用 tag 位数构造高位掩码：低 `tagged_bits` 位全 1，再左移到字的高位。
    pub fn init(&mut self, tagged_bits: u8) {
        if tagged_bits == 0 {
            self.tagged_mask = 0;
            return;
        }
        let hash_value_tagged_mask = (1_u64 << tagged_bits) - 1;
        let hash_value_tagged_offset = 64 - tagged_bits;
        self.tagged_mask = hash_value_tagged_mask << hash_value_tagged_offset;
    }

    // getTaggedValue 对应 Go 的同名方法：只保留 hashValue 中可嵌入指针的高位 tag。
    /// 从完整哈希值中只保留可嵌入指针的高位 tag 位。
    pub fn get_tagged_value(&self, hash_value: u64) -> u64 {
        hash_value & self.tagged_mask
    }

    // toTaggedPtr 对应 Go 的同名方法：先保存原始指针整数，再把 tag 写入高位。
    /// 把原始裸指针与 tag 按位或，得到 tagged pointer。
    pub fn to_tagged_ptr(&self, tagged_value: u64, ptr: *mut std::ffi::c_void) -> TaggedPtr {
        // Go 这里通过 unsafe.Pointer 写入 uintptr；直接做地址整数转换，避免真实解引用。
        let ret = ptr as usize;
        (ret as u64 | tagged_value) as TaggedPtr
    }

    // toUnsafePointer 对应 Go 的同名方法：清掉 tag 高位，还原成原始 unsafe.Pointer。
    /// 清掉嵌入的 tag 高位，还原为原始裸指针。
    pub fn to_unsafe_pointer(&self, t_ptr: TaggedPtr) -> *mut std::ffi::c_void {
        // 清除嵌入的 tag 后再转回裸指针；真实安全性依赖调用方保证地址仍有效。
        let untagged = (t_ptr as u64) & !self.tagged_mask;
        untagged as usize as *mut std::ffi::c_void
    }
}

// getTaggedBitsFromUintptr 对应 Go 的同名函数：根据指针高位连续 0 的数量估算可存 tag 位数。
/// 按指针前导零位数估算可安全嵌入的 tag 位数；非 64 位平台返回 0。
pub fn get_tagged_bits_from_uintptr(ptr: usize) -> u8 {
    if std::mem::size_of::<usize>() != 8 {
        return 0;
    }
    // Go 使用 bits.LeadingZeros64，并把结果限制在 maxTaggedBits 内。
    ((ptr as u64).leading_zeros() as i8).min(MAX_TAGGED_BITS) as u8
}
