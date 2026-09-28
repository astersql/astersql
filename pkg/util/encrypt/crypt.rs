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

// MySQL 风格 SQL 编解码（`ENCODE`/`DECODE` 函数所用置换表算法）。
//
// 对应 Go `pkg/util/encrypt/crypt.go`。由密码派生伪随机种子，生成 256 字节
// 编/解码置换表，再结合逐字节移位异或完成可逆变换。非现代密码学强度，
// 仅为兼容 MySQL 语义。

/// MySQL `sql_crypt` 使用的双种子线性同余伪随机状态。
#[derive(Clone, Copy, Default)]
struct randStruct {
    seed1: u32,
    seed2: u32,
    maxValue: u32,
    maxValueDbl: f64,
}

impl randStruct {
    /// 由密码字节初始化种子（跳过空格与制表符，算法对齐 MySQL）。
    fn randomInit(&mut self, password: &[u8]) {
        let (mut nr, mut add, mut nr2) = (1345345333_u32, 7_u32, 0x12345671_u32);
        for &byte in password {
            if byte == b' ' || byte == b'\t' {
                continue;
            }
            let value = byte as u32;
            nr ^= (nr & 63)
                .wrapping_add(add)
                .wrapping_mul(value)
                .wrapping_add(nr << 8);
            nr2 = nr2.wrapping_add((nr2 << 8) ^ nr);
            add = add.wrapping_add(value);
        }
        self.maxValue = 0x3fff_ffff;
        self.maxValueDbl = self.maxValue as f64;
        self.seed1 = (nr & 0x7fff_ffff) % self.maxValue;
        self.seed2 = (nr2 & 0x7fff_ffff) % self.maxValue;
    }

    /// 产生 [0, 1) 伪随机浮点，并推进双种子状态。
    fn myRand(&mut self) -> f64 {
        self.seed1 = self.seed1.wrapping_mul(3).wrapping_add(self.seed2) % self.maxValue;
        self.seed2 = self.seed1.wrapping_add(self.seed2).wrapping_add(33) % self.maxValue;
        self.seed1 as f64 / self.maxValueDbl
    }
}

/// 持有编/解码置换表与移位状态的 SQL 密码上下文。
struct sqlCrypt {
    rand: randStruct,
    decodeBuff: [u8; 256],
    encodeBuff: [u8; 256],
    shift: u32,
}

impl sqlCrypt {
    /// 由密码构造置换表：先洗牌 decodeBuff，再求逆得到 encodeBuff。
    fn new(password: &[u8]) -> Self {
        let mut value = Self {
            rand: randStruct::default(),
            decodeBuff: [0; 256],
            encodeBuff: [0; 256],
            shift: 0,
        };
        value.rand.randomInit(password);
        for i in 0..256 {
            value.decodeBuff[i] = i as u8;
        }
        for i in 0..256 {
            let index = (value.rand.myRand() * 255.0) as usize;
            value.decodeBuff.swap(index, i);
        }
        for i in 0..256 {
            value.encodeBuff[value.decodeBuff[i] as usize] = i as u8;
        }
        value
    }

    /// 原地编码：置换表映射后与移位异或，并回写移位状态。
    fn encode(&mut self, data: &mut [u8]) {
        for byte in data {
            self.shift ^= (self.rand.myRand() * 255.0) as u32;
            let original = *byte;
            *byte = self.encodeBuff[original as usize] ^ self.shift as u8;
            self.shift ^= original as u32;
        }
    }

    /// 原地解码：先与移位异或再经 decodeBuff 还原，并回写移位状态。
    fn decode(&mut self, data: &mut [u8]) {
        for byte in data {
            self.shift ^= (self.rand.myRand() * 255.0) as u32;
            *byte = self.decodeBuff[(*byte ^ self.shift as u8) as usize];
            self.shift ^= *byte as u32;
        }
    }
}

/// SQL `DECODE`：用密码对密文做 MySQL 风格解码，返回明文。
pub fn SQLDecode(str_: &[u8], password: &[u8]) -> Result<Vec<u8>, std::convert::Infallible> {
    let mut data = str_.to_vec();
    sqlCrypt::new(password).decode(&mut data);
    Ok(data)
}

/// SQL `ENCODE`：用密码对明文做 MySQL 风格编码，返回密文。
pub fn SQLEncode(data: &[u8], password: &[u8]) -> Result<Vec<u8>, std::convert::Infallible> {
    let mut data = data.to_vec();
    sqlCrypt::new(password).encode(&mut data);
    Ok(data)
}
