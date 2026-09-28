// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TiDB SM3（国密杂凑算法）摘要实现，供 `tidb_sm3_password` 认证使用。
//
// 对照 `tidb_sm3.go`：保留 256 位状态字、512 位分组压缩、消息扩展与填充流程；
// 对外提供与 Go `hash.Hash` 类似的 `Write`/`Sum`/`Reset` 接口。

// 本文件对照 pkg/parser/auth/tidb_sm3.go，保留 SM3 状态布局与压缩流程。
// SM3 规范参考：http://www.sca.gov.cn/sca/xwdt/2010-12/17/content_1002389.shtml
// 此实现修改自：https://github.com/tjfoc/gmsm/tree/601ddb090dcf53d7951cc4dcc66276e2b817837c/sm3
// 其他参考：https://datatracker.ietf.org/doc/draft-sca-cfrg-sm3/

/*
Copyright Suzhou Tongji Fintech Research Institute 2017 All Rights Reserved.
Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at
                 http://www.apache.org/licenses/LICENSE-2.0
Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

/// sm3 对应 Go 的同名结构，保存中间摘要、累计位长度和不足一个分组的尾部消息。
#[allow(non_camel_case_types)]
pub struct sm3 {
    /// digest 是压缩函数当前的八个 32 位状态字，即规范中的 V。
    digest: [u32; 8],
    /// length 按位计数，而不是按字节计数，用于最终填充的 64 位长度字段。
    length: u64,
    /// 尚未凑满一个 64 字节分组的尾部消息缓冲。
    unhandleMsg: Vec<u8>,
    /// 压缩分组大小，固定为 64 字节。
    blockSize: usize,
    /// 最终摘要字节数，固定为 32。
    size: usize,
}

/// FF0：前 16 轮布尔函数，按位异或。
fn ff0(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}
/// FF1：后 48 轮布尔函数，多数表决。
fn ff1(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (x & z) | (y & z)
}
/// GG0：前 16 轮布尔函数，按位异或。
fn gg0(x: u32, y: u32, z: u32) -> u32 {
    x ^ y ^ z
}
/// GG1：后 48 轮布尔函数，条件选择。
fn gg1(x: u32, y: u32, z: u32) -> u32 {
    (x & y) | (!x & z)
}
/// P0：压缩步骤中对 TT2 的置换。
fn p0(x: u32) -> u32 {
    x ^ leftRotate(x, 9) ^ leftRotate(x, 17)
}
/// P1：消息扩展步骤中的置换。
fn p1(x: u32) -> u32 {
    x ^ leftRotate(x, 15) ^ leftRotate(x, 23)
}

/// leftRotate 对应 Go 的循环左移；rotate_left 同样会把位数约束到 0..32。
fn leftRotate(x: u32, i: u32) -> u32 {
    x.rotate_left(i % 32)
}

impl sm3 {
    /// pad 对未处理尾部追加 1 bit、零填充和大端位长度，使总长度成为 512 位的整数倍。
    fn pad(&self) -> Vec<u8> {
        let mut msg = self.unhandleMsg.clone();
        msg.push(0x80);

        // 最后八字节必须留给原消息位长度，因此先补零到每块的第 56 字节。
        while msg.len() % 64 != 56 {
            msg.push(0x00);
        }
        msg.extend_from_slice(&self.length.to_be_bytes());
        msg
    }

    /// update 对所有完整 64 字节分组执行消息扩展和压缩，返回新的八字摘要状态。
    fn update(&self, mut msg: &[u8]) -> [u32; 8] {
        let mut w = [0_u32; 68];
        let mut w1 = [0_u32; 64];
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.digest;

        while msg.len() >= 64 {
            // 前 16 个消息字按网络大端序读取，随后按 SM3 的 P1 公式扩展到 68 个字。
            for i in 0..16 {
                w[i] = u32::from_be_bytes(msg[4 * i..4 * (i + 1)].try_into().unwrap());
            }
            for i in 16..68 {
                w[i] = p1(w[i - 16] ^ w[i - 9] ^ leftRotate(w[i - 3], 15))
                    ^ leftRotate(w[i - 13], 7)
                    ^ w[i - 6];
            }
            for i in 0..64 {
                w1[i] = w[i] ^ w[i + 4];
            }

            let (mut a1, mut b1, mut c1, mut d1) = (a, b, c, d);
            let (mut e1, mut f1, mut g1, mut h1) = (e, f, g, h);

            // 0..16 轮使用 FF0/GG0 与常数 0x79cc4519。
            for i in 0..16 {
                // Go 的 uint32 加法自然回绕；显式 wrapping_add，避免调试模式改变算法。
                let ss1 = leftRotate(
                    leftRotate(a1, 12)
                        .wrapping_add(e1)
                        .wrapping_add(leftRotate(0x79cc4519, i as u32)),
                    7,
                );
                let ss2 = ss1 ^ leftRotate(a1, 12);
                let tt1 = ff0(a1, b1, c1)
                    .wrapping_add(d1)
                    .wrapping_add(ss2)
                    .wrapping_add(w1[i]);
                let tt2 = gg0(e1, f1, g1)
                    .wrapping_add(h1)
                    .wrapping_add(ss1)
                    .wrapping_add(w[i]);
                d1 = c1;
                c1 = leftRotate(b1, 9);
                b1 = a1;
                a1 = tt1;
                h1 = g1;
                g1 = leftRotate(f1, 19);
                f1 = e1;
                e1 = p0(tt2);
            }

            // 16..64 轮切换到 FF1/GG1 与常数 0x7a879d8a，其余寄存器轮转保持一致。
            for i in 16..64 {
                let ss1 = leftRotate(
                    leftRotate(a1, 12)
                        .wrapping_add(e1)
                        .wrapping_add(leftRotate(0x7a879d8a, i as u32)),
                    7,
                );
                let ss2 = ss1 ^ leftRotate(a1, 12);
                let tt1 = ff1(a1, b1, c1)
                    .wrapping_add(d1)
                    .wrapping_add(ss2)
                    .wrapping_add(w1[i]);
                let tt2 = gg1(e1, f1, g1)
                    .wrapping_add(h1)
                    .wrapping_add(ss1)
                    .wrapping_add(w[i]);
                d1 = c1;
                c1 = leftRotate(b1, 9);
                b1 = a1;
                a1 = tt1;
                h1 = g1;
                g1 = leftRotate(f1, 19);
                f1 = e1;
                e1 = p0(tt2);
            }

            // 每个分组结束后，把工作寄存器异或回链接状态。
            a ^= a1;
            b ^= b1;
            c ^= c1;
            d ^= d1;
            e ^= e1;
            f ^= f1;
            g ^= g1;
            h ^= h1;
            msg = &msg[64..];
        }
        [a, b, c, d, e, f, g, h]
    }

    /// BlockSize 对应 hash.Hash.BlockSize，返回底层压缩分组大小 64 字节。
    pub fn BlockSize(&self) -> usize {
        self.blockSize
    }

    /// Size 对应 hash.Hash.Size，返回最终摘要长度 32 字节。
    pub fn Size(&self) -> usize {
        self.size
    }

    /// Reset 恢复 SM3 规范初始向量，并清空累计长度和未处理尾部。
    pub fn Reset(&mut self) {
        self.digest = [
            0x7380166f, 0x4914b2b9, 0x172442d7, 0xda8a0600, 0xa96f30bc, 0x163138aa, 0xe38dee4d,
            0xb0fb0e4e,
        ];
        self.length = 0;
        self.unhandleMsg.clear();
        self.blockSize = 64;
        self.size = 32;
    }

    /// Write 对应 Go 的 io.Writer 方法：接收任意长度数据，处理完整分组并缓存尾段。
    pub fn Write(&mut self, input: &[u8]) -> Result<usize, std::convert::Infallible> {
        let to_write = input.len();
        self.length = self
            .length
            .wrapping_add((input.len() as u64).wrapping_mul(8));

        // 先拼接上次不足一块的尾部，确保跨 Write 调用的分组边界与 Go 一致。
        let mut msg = self.unhandleMsg.clone();
        msg.extend_from_slice(input);
        let nblocks = msg.len() / self.BlockSize();
        self.digest = self.update(&msg);
        self.unhandleMsg = msg[nblocks * self.BlockSize()..].to_vec();
        Ok(to_write)
    }

    /// Sum 对应 Go 的同名方法，把摘要追加到输入字节后返回。
    pub fn Sum(&mut self, input: &[u8]) -> Vec<u8> {
        // 原 Go 实现会先 Write(in)，虽与 hash.Hash 的常规 Sum 约定不同，此处忠实保留其状态变化。
        let _ = self.Write(input);
        let msg = self.pad();
        let digest = self.update(&msg);

        // Go 实现虽然注释沿用 hash.Hash 的“追加到输入”，但最终返回的是
        // `in[len(in):len(in)+Size()]`，即只返回固定长度摘要。
        let mut output = Vec::with_capacity(self.Size());
        for word in digest {
            output.extend_from_slice(&word.to_be_bytes());
        }
        output
    }
}

/// NewSM3 对应 Go 构造函数；具体类型暂代替尚未可用的 `hash.Hash` trait object。
pub fn NewSM3() -> sm3 {
    let mut h = sm3 {
        digest: [0; 8],
        length: 0,
        unhandleMsg: Vec::new(),
        blockSize: 0,
        size: 0,
    };
    h.Reset();
    h
}

/// Sm3Hash 一次性计算输入数据的 SM3 摘要。
pub fn Sm3Hash(data: &[u8]) -> Vec<u8> {
    let mut h = NewSM3();
    let _ = h.Write(data);
    h.Sum(&[])
}
