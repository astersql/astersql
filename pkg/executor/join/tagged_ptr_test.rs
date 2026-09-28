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

/*
// tagged pointer helper 的位运算测试，不会解引用真实地址，也不会改变生产侧指针封装行为。

#[test]
pub fn test_tagged_bits() {
    let mut p: usize = 0;
    for i in 0..=64 {
        let tagged_bits = get_tagged_bits_from_uintptr(p);
        // Go 使用 min(int8(64-i), maxTaggedBits)；这里保留从全 0 到全 1 逐步减少 leading zeros 的检查。
        require::Equal(&mut testing::T::new(), (64_i8 - i as i8).min(MAX_TAGGED_BITS), tagged_bits as i8);
        p = (p << 1) + 1;
    }
}

#[test]
pub fn test_tag_helper_init() {
    let mut mask = !MAX_TAGGED_MASK;
    for tagged_bits in (0..=MAX_TAGGED_BITS).rev() {
        let mut tag_helper = TagPtrHelper { tagged_mask: 0 };
        tag_helper.init(tagged_bits as u8);
        require::Equal(&mut testing::T::new(), mask, tag_helper.tagged_mask);
        // Go 每轮左移一次，验证可用 tag 位减少时高位掩码同步收缩。
        mask <<= 1;
    }
}

#[test]
pub fn test_tag_helper() {
    let mut raw_data = vec![0_u8; 10 * 1024 * 1024];
    let start_ptr = raw_data.as_mut_ptr() as *mut std::ffi::c_void;
    let end_ptr = unsafe { raw_data.as_mut_ptr().add(raw_data.len() - 1) } as *mut std::ffi::c_void;
    let start_uintptr = start_ptr as usize;
    let end_uintptr = end_ptr as usize;
    let tagged_bits = get_tagged_bits_from_uintptr(start_uintptr | end_uintptr);
    let mut tag_helper = TagPtrHelper { tagged_mask: 0 };
    tag_helper.init(tagged_bits);

    let mut tagged_value = 0x1234_u64 << (64 - MAX_TAGGED_BITS);
    loop {
        if tagged_value & tag_helper.tagged_mask == tagged_value {
            break;
        }
        tagged_value <<= 1;
    }
    require::True(&mut testing::T::new(), tagged_value != 0, "tagged value should not be zero");

    // Go 对起止两个真实 unsafe.Pointer 做 round trip；保留裸指针整数化与清 tag 的检查形状。
    for test_ptr in [start_ptr, end_ptr] {
        let tagged_ptr = tag_helper.to_tagged_ptr(tagged_value, test_ptr);
        require::Equal(&mut testing::T::new(), tagged_value, tag_helper.get_tagged_value(tagged_ptr as u64));
        require::Equal(&mut testing::T::new(), test_ptr, tag_helper.to_unsafe_pointer(tagged_ptr));
    }
}
*/

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
