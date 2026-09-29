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

// Tagged Pointer 辅助结构的单元测试。
//
// 覆盖前导零推算 tag 位数、高位掩码初始化，以及真实堆分配地址的 round-trip
//（嵌入 tag 后再剥离，指针与指向内容保持不变）。不验证生产侧哈希表探测逻辑。

use crate::tagged_ptr::{
    MAX_TAGGED_BITS, MAX_TAGGED_MASK, TAGGED_POINTER_LEN, TagPtrHelper,
    get_tagged_bits_from_uintptr,
};

/// 核对指针字长常量，以及全 0 / 最高位为 1 时前导零推算的 tag 位数。
#[test]
fn tagged_bits_follow_machine_pointer_leading_zeros() {
    assert_eq!(TAGGED_POINTER_LEN as usize, std::mem::size_of::<usize>());
    if usize::BITS == 64 {
        let mut pointer = 0_usize;
        for occupied_bits in 0..=64 {
            assert_eq!(
                get_tagged_bits_from_uintptr(pointer),
                (64_u8 - occupied_bits).min(MAX_TAGGED_BITS as u8)
            );
            pointer = pointer.wrapping_shl(1).wrapping_add(1);
        }
    } else {
        assert_eq!(get_tagged_bits_from_uintptr(0), 0);
    }
}

/// 初始化后掩码应等于 `!MAX_TAGGED_MASK`，且 get_tagged_value 只保留高位。
#[test]
fn tag_helper_masks_only_available_high_bits() {
    let mut helper = TagPtrHelper { tagged_mask: 0 };
    helper.init(MAX_TAGGED_BITS as u8);
    assert_eq!(helper.tagged_mask, !MAX_TAGGED_MASK);
    let hash = 0xabcdefff_12345678_u64;
    assert_eq!(helper.get_tagged_value(hash), hash & helper.tagged_mask);
}

/// 与 Go `TestTagHelperInit` 一致，覆盖从最大 tag 位数到 0 的全部掩码。
#[test]
fn tag_helper_init_matches_all_go_masks() {
    let mut expected_mask = !MAX_TAGGED_MASK;
    for tagged_bits in (0..=MAX_TAGGED_BITS as u8).rev() {
        let mut helper = TagPtrHelper { tagged_mask: 0 };
        helper.init(tagged_bits);
        assert_eq!(helper.tagged_mask, expected_mask);
        expected_mask <<= 1;
    }
}

/// 对真实堆分配地址做 tagged 往返：剥离 tag 后指针与载荷不变。
#[test]
fn tagged_pointer_round_trip_preserves_real_allocation() {
    let mut allocation = Box::new(0x1234_5678_u64);
    let pointer = (&mut *allocation as *mut u64).cast::<std::ffi::c_void>();
    let mut helper = TagPtrHelper { tagged_mask: 0 };
    // 按该分配地址的可用前导零位数初始化掩码。
    helper.init(get_tagged_bits_from_uintptr(pointer as usize));
    let tag = helper.get_tagged_value(0xfeed_beef_dead_cafe);
    let tagged = helper.to_tagged_ptr(tag, pointer);
    assert_eq!(helper.get_tagged_value(tagged as u64), tag);
    let restored = helper.to_unsafe_pointer(tagged);
    assert_eq!(restored, pointer);
    assert_eq!(unsafe { *restored.cast::<u64>() }, 0x1234_5678);
}
