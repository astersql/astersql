// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// unistore TiKV 侧通用工具：键范围边界、锁哈希与安全拷贝。
//
// Latch（闩锁）按键指纹串行化写路径；指纹需在加锁与释放路径上确定性一致。
// Region 的 end_key 为空表示右开无界区间，扫描时不能误判为越界。

use crate::mvcc::Mutation;

/// 判断 `current` 是否已达到或越过 Region 的 `end_key`。
///
/// 空 `end_key` 表示无上界，始终返回 false；否则按字节序比较（半开区间）。
pub fn exceed_end_key(current: &[u8], end_key: &[u8]) -> bool {
    !end_key.is_empty() && current >= end_key
}
/// 对哈希值排序并去重，供 Latch 按稳定顺序获取，避免死锁。
pub fn sort_and_dedup_hash_values(mut values: Vec<u64>) -> Vec<u64> {
    if values.len() > 1 {
        values.sort_unstable();
        values.dedup();
    }
    values
}
/// 将 Mutation 列表中的 key 转为排序去重后的指纹哈希。
pub fn mutations_to_hash_values(mutations: &[Mutation]) -> Vec<u64> {
    sort_and_dedup_hash_values(
        mutations
            .iter()
            .map(|mutation| fingerprint64(&mutation.key))
            .collect(),
    )
}
/// 将原始 key 列表转为排序去重后的指纹哈希。
pub fn keys_to_hash_values(keys: &[Vec<u8>]) -> Vec<u64> {
    sort_and_dedup_hash_values(keys.iter().map(|key| fingerprint64(key)).collect())
}
/// 将版本化存储键中已提取的 user key 转为排序去重后的指纹哈希。
///
/// Rust 端不暴露 Go `badger/y.Key`，因此调用方直接传其 `UserKey` 视图。
pub fn user_keys_to_hash_values<T: AsRef<[u8]>>(keys: &[T]) -> Vec<u64> {
    sort_and_dedup_hash_values(keys.iter().map(|key| fingerprint64(key.as_ref())).collect())
}
/// 拷贝字节切片到独立 `Vec`，避免与调用方共享底层缓冲区。
pub fn safe_copy(value: &[u8]) -> Vec<u8> {
    value.to_vec()
}

const FARM_K0: u64 = 0xc3a5_c85c_97cb_3127;
const FARM_K1: u64 = 0xb492_b66f_be98_f273;
const FARM_K2: u64 = 0x9ae1_6a3b_2f90_404f;

fn fetch64(value: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        value[offset..offset + 8]
            .try_into()
            .expect("eight-byte chunk"),
    )
}

fn fetch32(value: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        value[offset..offset + 4]
            .try_into()
            .expect("four-byte chunk"),
    )
}

fn shift_mix(value: u64) -> u64 {
    value ^ (value >> 47)
}

fn hash_len_16_mul(u: u64, v: u64, mul: u64) -> u64 {
    let mut a = (u ^ v).wrapping_mul(mul);
    a ^= a >> 47;
    let mut b = (v ^ a).wrapping_mul(mul);
    b ^= b >> 47;
    b.wrapping_mul(mul)
}

fn hash_len_0_to_16(value: &[u8]) -> u64 {
    let len = value.len();
    if len >= 8 {
        let mul = FARM_K2.wrapping_add((len as u64).wrapping_mul(2));
        let a = fetch64(value, 0).wrapping_add(FARM_K2);
        let b = fetch64(value, len - 8);
        let c = b.rotate_right(37).wrapping_mul(mul).wrapping_add(a);
        let d = a.rotate_right(25).wrapping_add(b).wrapping_mul(mul);
        return hash_len_16_mul(c, d, mul);
    }
    if len >= 4 {
        let mul = FARM_K2.wrapping_add((len as u64).wrapping_mul(2));
        let a = u64::from(fetch32(value, 0));
        return hash_len_16_mul(
            (len as u64).wrapping_add(a << 3),
            u64::from(fetch32(value, len - 4)),
            mul,
        );
    }
    if len > 0 {
        let y = u32::from(value[0]) + (u32::from(value[len >> 1]) << 8);
        let z = len as u32 + (u32::from(value[len - 1]) << 2);
        return shift_mix(u64::from(y).wrapping_mul(FARM_K2) ^ u64::from(z).wrapping_mul(FARM_K0))
            .wrapping_mul(FARM_K2);
    }
    FARM_K2
}

fn hash_len_17_to_32(value: &[u8]) -> u64 {
    let len = value.len();
    let mul = FARM_K2.wrapping_add((len as u64).wrapping_mul(2));
    let a = fetch64(value, 0).wrapping_mul(FARM_K1);
    let b = fetch64(value, 8);
    let c = fetch64(value, len - 8).wrapping_mul(mul);
    let d = fetch64(value, len - 16).wrapping_mul(FARM_K2);
    hash_len_16_mul(
        a.wrapping_add(b)
            .rotate_right(43)
            .wrapping_add(c.rotate_right(30))
            .wrapping_add(d),
        a.wrapping_add(b.wrapping_add(FARM_K2).rotate_right(18))
            .wrapping_add(c),
        mul,
    )
}

fn weak_hash_len_32_with_seeds(value: &[u8], mut a: u64, mut b: u64) -> (u64, u64) {
    let w = fetch64(value, 0);
    let x = fetch64(value, 8);
    let y = fetch64(value, 16);
    let z = fetch64(value, 24);
    a = a.wrapping_add(w);
    b = b.wrapping_add(a).wrapping_add(z).rotate_right(21);
    let c = a;
    a = a.wrapping_add(x).wrapping_add(y);
    b = b.wrapping_add(a.rotate_right(44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

fn hash_len_33_to_64(value: &[u8]) -> u64 {
    let len = value.len();
    let mul = FARM_K2.wrapping_add((len as u64).wrapping_mul(2));
    let a = fetch64(value, 0).wrapping_mul(FARM_K2);
    let b = fetch64(value, 8);
    let c = fetch64(value, len - 8).wrapping_mul(mul);
    let d = fetch64(value, len - 16).wrapping_mul(FARM_K2);
    let y = a
        .wrapping_add(b)
        .rotate_right(43)
        .wrapping_add(c.rotate_right(30))
        .wrapping_add(d);
    let z = hash_len_16_mul(
        y,
        a.wrapping_add(b.wrapping_add(FARM_K2).rotate_right(18))
            .wrapping_add(c),
        mul,
    );
    let e = fetch64(value, 16).wrapping_mul(mul);
    let f = fetch64(value, 24);
    let g = y.wrapping_add(fetch64(value, len - 32)).wrapping_mul(mul);
    let h = z.wrapping_add(fetch64(value, len - 24)).wrapping_mul(mul);
    hash_len_16_mul(
        e.wrapping_add(f)
            .rotate_right(43)
            .wrapping_add(g.rotate_right(30))
            .wrapping_add(h),
        e.wrapping_add(f.wrapping_add(a).rotate_right(18))
            .wrapping_add(g),
        mul,
    )
}

/// Go `farm.Fingerprint64`（FarmHash NA）的逐位等价实现。
pub fn fingerprint64(value: &[u8]) -> u64 {
    let len = value.len();
    if len <= 16 {
        return hash_len_0_to_16(value);
    }
    if len <= 32 {
        return hash_len_17_to_32(value);
    }
    if len <= 64 {
        return hash_len_33_to_64(value);
    }

    let mut v = (0_u64, 0_u64);
    let mut w = (0_u64, 0_u64);
    let mut x = 81_u64.wrapping_mul(FARM_K2).wrapping_add(fetch64(value, 0));
    let mut y = 81_u64.wrapping_mul(FARM_K1).wrapping_add(113);
    let mut z = shift_mix(y.wrapping_mul(FARM_K2).wrapping_add(113)).wrapping_mul(FARM_K2);
    let end = ((len - 1) / 64) * 64;
    let last64 = end + ((len - 1) & 63) - 63;
    let mut offset = 0;
    while offset < end {
        let chunk = &value[offset..offset + 64];
        x = x
            .wrapping_add(y)
            .wrapping_add(v.0)
            .wrapping_add(fetch64(chunk, 8))
            .rotate_right(37)
            .wrapping_mul(FARM_K1);
        y = y
            .wrapping_add(v.1)
            .wrapping_add(fetch64(chunk, 48))
            .rotate_right(42)
            .wrapping_mul(FARM_K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(fetch64(chunk, 40));
        z = z.wrapping_add(w.0).rotate_right(33).wrapping_mul(FARM_K1);
        v = weak_hash_len_32_with_seeds(chunk, v.1.wrapping_mul(FARM_K1), x.wrapping_add(w.0));
        w = weak_hash_len_32_with_seeds(
            &chunk[32..],
            z.wrapping_add(w.1),
            y.wrapping_add(fetch64(chunk, 16)),
        );
        std::mem::swap(&mut x, &mut z);
        offset += 64;
    }
    let chunk = &value[last64..];
    let mul = FARM_K1.wrapping_add((z & 0xff) << 1);
    w.0 = w.0.wrapping_add(((len - 1) & 63) as u64);
    v.0 = v.0.wrapping_add(w.0);
    w.0 = w.0.wrapping_add(v.0);
    x = x
        .wrapping_add(y)
        .wrapping_add(v.0)
        .wrapping_add(fetch64(chunk, 8))
        .rotate_right(37)
        .wrapping_mul(mul);
    y = y
        .wrapping_add(v.1)
        .wrapping_add(fetch64(chunk, 48))
        .rotate_right(42)
        .wrapping_mul(mul);
    x ^= w.1.wrapping_mul(9);
    y = y
        .wrapping_add(v.0.wrapping_mul(9))
        .wrapping_add(fetch64(chunk, 40));
    z = z.wrapping_add(w.0).rotate_right(33).wrapping_mul(mul);
    v = weak_hash_len_32_with_seeds(chunk, v.1.wrapping_mul(mul), x.wrapping_add(w.0));
    w = weak_hash_len_32_with_seeds(
        &chunk[32..],
        z.wrapping_add(w.1),
        y.wrapping_add(fetch64(chunk, 16)),
    );
    std::mem::swap(&mut x, &mut z);
    hash_len_16_mul(
        hash_len_16_mul(v.0, w.0, mul)
            .wrapping_add(shift_mix(y).wrapping_mul(FARM_K0))
            .wrapping_add(z),
        hash_len_16_mul(v.1, w.1, mul).wrapping_add(x),
        mul,
    )
}
