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

// fastrand 高层随机 API：wyrand PRNG、Buf、Uint32N/Uint64N。
//
// 对应 Go `pkg/util/fastrand/random.go`。PRNG（伪随机数生成器）用固定算法
// 从种子推导序列；本文件保留 Go 的 wyrand / Lemire reduction 结构，供采样与测试数据生成。

// 本文件由 pkg/util/fastrand/random.go 迁移而来，保留 Go 实现结构。
// `bits.Mul64` 在 实现中用 u128 乘法拆高低 64 位表达同一算法。

use super::runtime::Uint32;

// wyrand is a fast PRNG. See https://github.com/wangyi-fudan/wyhash
// wyrand 对应 Go 的 uint64 新类型；这里用 tuple struct 保留内部状态。
/// 快速 PRNG 状态；内部 `u64` 为当前种子。
pub struct wyrand(pub u64);

// _wymix 对应 Go 的 bits.Mul64 后高低位异或。
// Rust 用 u128 乘法模拟 128 位乘积拆分。
/// 对 `a*b` 的 128 位乘积做高低 64 位异或，对应 Go `bits.Mul64` 混合。
pub fn _wymix(a: u64, b: u64) -> u64 {
    // 用 u128 乘法拆出高/低 64 位，再异或混合。
    let product = (a as u128) * (b as u128);
    let hi = (product >> 64) as u64;
    let lo = product as u64;
    hi ^ lo
}

impl wyrand {
    // Next 对应 Go 指针接收者方法：先按固定常量推进状态，再混合生成 uint64。
    /// 推进种子并返回下一个 `u64` 随机值。
    pub fn Next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0xa0761d6478bd642f);
        _wymix(self.0, self.0 ^ 0xe7037ed1a0b428db)
    }
}

// Buf generates a random string using ASCII characters but avoid separator character.
// See https://github.com/mysql/mysql-server/blob/5.7/mysys_ssl/crypt_genhash_impl.cc#L435
// Buf 保留 Go 的 ASCII 随机字节生成逻辑，并避免生成 0 和 '$' 分隔符。
/// 生成长度为 `size` 的 ASCII 随机字节，避开 `\0` 与 `$` 分隔符。
pub fn Buf(size: usize) -> Vec<u8> {
    let mut buf = vec![0; size];
    let mut r = wyrand(Uint32() as u64);
    for i in 0..size {
        // This is similar to Uint32() % n, but faster.
        // See https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
        // Go 使用乘法右移避免取模；这里保留相同的 Lemire reduction 写法。
        buf[i] = (((r.Next() as u32 as u64) * 127) >> 32) as u8;
        // 避开 NUL 与 MySQL 密码哈希里用的 '$' 分隔符。
        if buf[i] == 0 || buf[i] == b'$' {
            buf[i] = buf[i].wrapping_add(1);
        }
    }
    buf
}

// Uint32N returns, as an uint32, a pseudo-random number in [0,n).
// Uint32N 对应 Go 的快速缩放随机数。
/// 返回 `[0,n)` 内的伪随机 `u32`（Lemire 乘法缩放，避免取模）。
pub fn Uint32N(n: u32) -> u32 {
    // This is similar to Uint32() % n, but faster.
    // See https://lemire.me/blog/2016/06/27/a-fast-alternative-to-the-modulo-reduction/
    ((Uint32() as u64 * n as u64) >> 32) as u32
}

// Uint64N returns, as an uint64, a pseudo-random number in [0,n).
// Uint64N 保留 Go 从两个 Uint32 拼出 uint64 的形状，并对 2 的幂使用 mask。
/// 返回 `[0,n)` 内的伪随机 `u64`；`n` 为 2 的幂时用 mask，否则取模。
pub fn Uint64N(n: u64) -> u64 {
    // 用两次 Uint32 拼出 64 位随机值，再缩放到 [0,n)。
    let a = Uint32();
    let b = Uint32();
    let v = ((a as u64) << 32) + b as u64;
    let mask = n.wrapping_sub(1);
    if (n & mask) == 0 {
        // n is power of two, can mask
        return v & mask;
    }
    v % n
}
