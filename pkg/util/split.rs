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

// Region/键空间均匀切分辅助：在 lower/upper 字节键之间生成中间分割点。
//
// 对应 Go `pkg/util/split`。先求最长公共前缀，再把后缀解释为大端 uint64，
// 按步长生成 `num-1` 个中间 key，供表/Region 分裂等场景使用。

#![allow(dead_code)]
#![allow(non_snake_case)]

/// 在 lower/upper 之间生成 `num-1` 个中间 key，追加到 `valuesList` 后返回。
// GetValuesList is used to get `num` values between lower and upper value.
// To Simplify the explain, suppose lower and upper value type is int64, and lower=0, upper=100, num=10,
// then calculate the step=(upper-lower)/num=10, then the function should return 0+10, 10+10, 20+10... all together 9 (num-1) values.
// Then the function will return [10,20,30,40,50,60,70,80,90].
// The difference is the value type of upper, lower is []byte, So I use getUint64FromBytes to convert []byte to uint64.
// GetValuesList 对应 Go 的导出函数：根据 lower/upper 的公共前缀和后缀数值区间生成 num-1 个中间 key。
// valuesList 保持 Go 入参/返回同一个 slice 的形状，用可变 Vec 所有权表示 append 后返回。
pub fn GetValuesList(
    lower: Vec<u8>,
    upper: Vec<u8>,
    num: usize,
    mut valuesList: Vec<Vec<u8>>,
) -> Vec<Vec<u8>> {
    let commonPrefixIdx = longestCommonPrefixLen(&lower, &upper);
    let step = getStepValue(&lower[commonPrefixIdx..], &upper[commonPrefixIdx..], num);
    let mut startV = getUint64FromBytes(&lower[commonPrefixIdx..], 0);

    // To get `num` regions, only need to split `num-1` idx keys.
    // Go 里 buf := make([]byte, 8) 会在循环中复用同一块 8 字节缓冲；
    // Rust 这里用固定数组保留“先写大端 uint64，再追加字节”的顺序。
    let mut buf = [0_u8; 8];
    // Go 1.22 的 `for range num - 1` 表示循环 num-1 次；num 的有效性由调用方保证。
    for _ in 0..num.saturating_sub(1) {
        // value 初始容量为 commonPrefixIdx+8，随后复制公共前缀，再追加新的 8 字节分割点。
        let mut value = Vec::with_capacity(commonPrefixIdx + 8);
        value.extend_from_slice(&lower[..commonPrefixIdx]);
        // Go 的 uint64 加法在溢出时按模 2^64 环绕；wrapping_add 明确保留该数值语义。
        startV = startV.wrapping_add(step);
        buf.copy_from_slice(&startV.to_be_bytes());
        value.extend_from_slice(&buf);
        valuesList.push(value);
    }
    valuesList
}

/// 返回两字节切片的最长公共前缀长度；遇首个不同字节即停。
// longestCommonPrefixLen gets the longest common prefix byte length.
// longestCommonPrefixLen 逐字节比较两个 Go []byte，返回最长公共前缀长度。
// 它只读取内存中的切片，不分配新对象，也没有外部依赖。
pub(crate) fn longestCommonPrefixLen(s1: &[u8], s2: &[u8]) -> usize {
    let l = std::cmp::min(s1.len(), s2.len());
    let mut i = 0;
    while i < l {
        if s1[i] != s2[i] {
            // 遇到第一个不同字节时立即退出，保持 Go for 循环中的 break 语义。
            break;
        }
        i += 1;
    }
    i
}

/// 将 lower/upper 后缀转为 uint64 后计算步长 `(upper-lower)/num`（可环绕）。
// getStepValue gets the step of between the lower and upper value. step = (upper-lower)/num.
// Convert byte slice to uint64 first.
// getStepValue 先把 lower/upper 后缀补齐并解释成 uint64，再计算每个 split 区间的步长。
pub(crate) fn getStepValue(lower: &[u8], upper: &[u8], num: usize) -> u64 {
    let lowerUint = getUint64FromBytes(lower, 0);
    let upperUint = getUint64FromBytes(upper, 0xff);
    // Go 的 uint64 减法可能环绕；除以 uint64(num) 也保留原来的“num 为 0 时会失败”的约束。
    upperUint.wrapping_sub(lowerUint) / num as u64
}

/// 字节切片按大端解释为 uint64；不足 8 字节用 `pad` 补齐，超过则只读前 8 字节。
// getUint64FromBytes gets a uint64 from the `bs` byte slice.
// If len(bs) < 8, then padding with `pad`.
// getUint64FromBytes 对应 Go helper：把字节切片按大端解释为 uint64。
// 当原切片不足 8 字节时，按 Go 代码追加 pad 到 8 字节；超过 8 字节时 binary.BigEndian.Uint64 只读前 8 字节。
fn getUint64FromBytes(bs: &[u8], pad: u8) -> u64 {
    let mut buf: Vec<u8> = bs.to_vec();
    if buf.len() < 8 {
        // Go 代码重新 make 一个容量为 8 的切片，然后先 append 原字节再补 pad。
        // 这里直接 push pad，保留最终传给 BigEndian.Uint64 的 8 字节内容。
        while buf.len() < 8 {
            buf.push(pad);
        }
    }

    let mut firstEight = [0_u8; 8];
    firstEight.copy_from_slice(&buf[..8]);
    u64::from_be_bytes(firstEight)
}
