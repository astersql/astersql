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

// `ROW_NUMBER` 窗口函数实现。
//
// 对应 Go 的 `row_number.go`：在当前窗口分区内为每一行分配从 1 开始的连续序号。
// 窗口函数（window function）按 PARTITION BY / ORDER BY 定义的帧逐行求值；
// `ROW_NUMBER` 与排名类函数不同，不因值相等而跳号或并列。

use std::mem::size_of;

/// `RowNumber` 部分结果的固定体积，对应 Go `DefPartialResult4RowNumberSize`。
pub const DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE: i64 = size_of::<RowNumber>() as i64;

/// `ROW_NUMBER` 的部分结果：仅保存当前分区内已输出的行号计数器。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowNumber {
    cur_idx: i64,
}

impl RowNumber {
    /// 重置行号计数器（切换分区或重新开始窗口时调用）。
    pub fn reset(&mut self) {
        self.cur_idx = 0;
    }

    /// 窗口 `Update` 钩子占位：`ROW_NUMBER` 不在 Update 阶段写结果，固定返回 0。
    pub const fn update(&self) -> i64 {
        0
    }

    /// 产出下一行的行号：计数器自增后返回（首行得到 1）。
    pub fn next_value(&mut self) -> i64 {
        // Go 的 int64++ 按二进制补码回绕；显式 wrapping 保持 debug/release 一致。
        self.cur_idx = self.cur_idx.wrapping_add(1);
        self.cur_idx
    }

    /// 滑动窗口帧移动时的钩子；`ROW_NUMBER` 无需额外状态调整。
    pub const fn slide(&self) {}

    #[cfg(test)]
    pub(crate) const fn from_index_for_test(cur_idx: i64) -> Self {
        Self { cur_idx }
    }
}
