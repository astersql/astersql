// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// NTILE 窗口函数实现。
//
// 对应 SQL `NTILE(n)`：把分区内行均分为 n 个桶（bucket），返回每行所属桶号（从 1 起）。
// 不能整除时，余数优先分给靠前的桶，使前 `remainder` 个桶各多一行。
// `n == 0` 时视为非法参数，求值返回 `None`（对应 SQL NULL）。

/// NTILE 部分结果：累计分区行数，并按桶序号依次吐出结果。
///
/// 字段含义：`n` 为目标桶数；`quotient`/`remainder` 为 `num_rows / n` 与取模；
/// `cur_group_idx` 为当前桶号；`cur_idx` 为桶内已输出行数。
#[derive(Clone, Debug, PartialEq)]
pub struct Ntile {
    n: u64,
    cur_idx: u64,
    cur_group_idx: u64,
    remainder: u64,
    quotient: u64,
    num_rows: u64,
}

impl Ntile {
    /// 以可选桶数构造；`None` 按 0 处理，后续 `next_value` 将返回空。
    pub fn new(n: Option<u64>) -> Self {
        Self {
            n: n.unwrap_or(0),
            cur_idx: 0,
            cur_group_idx: 1,
            remainder: 0,
            quotient: 0,
            num_rows: 0,
        }
    }

    /// 清空累计行数与桶游标，准备处理下一个分区。
    ///
    /// 与 Go `ResetPartialResult` 一致，桶宽和余数保留到下一次 `update` 重算。
    pub fn reset(&mut self) {
        self.cur_idx = 0;
        self.cur_group_idx = 1;
        self.num_rows = 0;
    }

    /// 累加分区行数，并刷新每桶基准大小与余数。
    pub fn update(&mut self, row_count: u64) {
        self.num_rows += row_count;
        // 有合法 n 时重算：前 remainder 个桶大小为 quotient+1，其余为 quotient。
        if self.n != 0 {
            self.quotient = self.num_rows / self.n;
            self.remainder = self.num_rows % self.n;
        }
    }

    /// 输出当前行所属桶号，并推进桶内游标；桶满后切换到下一桶。
    pub fn next_value(&mut self) -> Option<u64> {
        // n=0 无法分桶，与 SQL 非法 NTILE 参数语义对齐。
        if self.n == 0 {
            return None;
        }
        let result = self.cur_group_idx;
        self.cur_idx += 1;
        // 前 remainder 个桶多分配一行，保证桶大小差至多 1。
        let mut current_group_size = self.quotient;
        if self.cur_group_idx <= self.remainder {
            current_group_size += 1;
        }
        if self.cur_idx == current_group_size {
            self.cur_idx = 0;
            self.cur_group_idx += 1;
        }
        Some(result)
    }
}
