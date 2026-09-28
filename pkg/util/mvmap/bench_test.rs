// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// MVMap（多值哈希表）Put/Get 性能基准与冒烟测试。
//
// 对应 Go `pkg/util/mvmap` 的 `BenchmarkMVMapPut` / `BenchmarkMVMapGet`：
// 用 big-endian 序号作键值，测量插入与查找路径开销。

#![allow(dead_code)]
#![allow(non_snake_case)]

use super::NewMVMap;

/// BenchmarkMVMapPut 对应 Go 的同名 benchmark，每轮把循环序号按 big-endian 写入 8 字节缓冲区后插入 MVMap。
// BenchmarkMVMapPut 对应 Go 的同名 benchmark，每轮把循环序号按 big-endian 写入 8 字节缓冲区后插入 MVMap。
pub fn BenchmarkMVMapPut(iterations: usize) {
    let mut m = NewMVMap();
    let mut buffer = [0_u8; 8];
    for i in 0..iterations {
        // Go 使用 binary.BigEndian.PutUint64 原地改写同一块 buffer；这里保留复用缓冲区的分配语义。
        buffer.copy_from_slice(&(i as u64).to_be_bytes());
        m.Put(&buffer, &buffer);
    }
}

/// BenchmarkMVMapGet 对应 Go 的同名 benchmark，先填充 b.N 个键值对，再只测 Get 查找和校验成本。
// BenchmarkMVMapGet 对应 Go 的同名 benchmark，先填充 b.N 个键值对，再只测 Get 查找和校验成本。
pub fn BenchmarkMVMapGet(iterations: usize) {
    let mut m = NewMVMap();
    let mut buffer = [0_u8; 8];
    // 先写入全部键值，再进入纯 Get 计时区段。
    for i in 0..iterations {
        buffer.copy_from_slice(&(i as u64).to_be_bytes());
        m.Put(&buffer, &buffer);
    }

    // Go 预分配 make([][]byte, 0, 8)，每轮传入 val[:0] 复用外层 slice。
    let mut val: Vec<&[u8]> = Vec::with_capacity(8);
    for i in 0..iterations {
        buffer.copy_from_slice(&(i as u64).to_be_bytes());
        val.clear();
        val = m.Get(&buffer, val);
        // bytes.Equal 只比较返回的 value 字节；失败时 FailNow 立即结束 benchmark。
        if val.len() != 1 || val[0] != &buffer {
            panic!("MVMap Get did not return the inserted value");
        }
    }
}

/// 冒烟：小规模调用 `BenchmarkMVMapPut`，确认路径可跑通。
#[test]
fn benchmark_mvmap_put_smoke() {
    BenchmarkMVMapPut(1_024);
}

/// 冒烟：小规模调用 `BenchmarkMVMapGet`，确认查找与校验可跑通。
#[test]
fn benchmark_mvmap_get_smoke() {
    BenchmarkMVMapGet(1_024);
}
