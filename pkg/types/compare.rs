// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 向量化与标量整数/字符串比较，对齐 Go `types` 包比较语义。
//
// 有符号与无符号混比时，负数或超出 i64 范围的无符号值
// 按 MySQL 规则直接判定大小，避免错误强制转换。

/// 将 Rust Ordering 映射为 Go 风格比较结果：-1 / 0 / 1。
fn ordering(value: std::cmp::Ordering) -> i32 {
    match value {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// 逐元素比较两个无符号整数向量，结果写入 `res`。
pub fn VecCompareUU(x: &[u64], y: &[u64], res: &mut [i64]) {
    for index in 0..x.len() {
        res[index] = i64::from(ordering(x[index].cmp(&y[index])));
    }
}

/// 逐元素比较两个有符号整数向量，结果写入 `res`。
pub fn VecCompareII(x: &[i64], y: &[i64], res: &mut [i64]) {
    for index in 0..x.len() {
        res[index] = i64::from(ordering(x[index].cmp(&y[index])));
    }
}

/// 无符号对有符号向量比较：右侧为负或左侧超过 i64::MAX 时左侧更大。
pub fn VecCompareUI(x: &[u64], y: &[i64], res: &mut [i64]) {
    for index in 0..x.len() {
        // 无法安全同型比较时，按 MySQL 混合符号规则直接给出结果
        res[index] = if y[index] < 0 || x[index] > i64::MAX as u64 {
            1
        } else {
            i64::from(ordering((x[index] as i64).cmp(&y[index])))
        };
    }
}

/// 有符号对无符号向量比较：左侧为负或右侧超过 i64::MAX 时左侧更小。
pub fn VecCompareIU(x: &[i64], y: &[u64], res: &mut [i64]) {
    for index in 0..x.len() {
        res[index] = if x[index] < 0 || y[index] > i64::MAX as u64 {
            -1
        } else {
            i64::from(ordering(x[index].cmp(&(y[index] as i64))))
        };
    }
}

/// 按指定 collation（校对规则）比较两个字符串。
pub fn CompareString(x: &str, y: &str, collation: &str) -> i32 {
    collate::GetCollator(collation).Compare(x, y)
}

/// 标量整数比较，`isUnsigned*` 标明对应参数是否按无符号解释。
pub fn CompareInt(arg0: i64, isUnsigned0: bool, arg1: i64, isUnsigned1: bool) -> i32 {
    match (isUnsigned0, isUnsigned1) {
        (true, true) => ordering((arg0 as u64).cmp(&(arg1 as u64))),
        (true, false) => {
            // 有符号为负或无符号超出 i64 正范围时，无符号一侧更大
            if arg1 < 0 || arg0 as u64 > i64::MAX as u64 {
                1
            } else {
                ordering(arg0.cmp(&arg1))
            }
        }
        (false, true) => {
            if arg0 < 0 || arg1 as u64 > i64::MAX as u64 {
                -1
            } else {
                ordering(arg0.cmp(&arg1))
            }
        }
        (false, false) => ordering(arg0.cmp(&arg1)),
    }
}
