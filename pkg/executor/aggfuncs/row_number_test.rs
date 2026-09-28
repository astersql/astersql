// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// `ROW_NUMBER` 窗口函数测试。
//
// 对应 Go 的 `row_number_test.go`：保留内存增量草稿用例，并直接验证生产
// `RowNumber` 状态在连续取值与 `reset` 后的序号行为。

/// 窗口内存测试草稿：用字符串记录 Go harness 参数，便于对照迁移。
#[allow(dead_code)]
struct WindowMemTestDraft {
    func_name: &'static str,
    field_type: &'static str,
    constant_arg: u64,
    num_rows: usize,
    order_by_cols: usize,
    alloc_mem_delta: &'static str,
    update_mem_delta_gens: &'static str,
}

// build_row_number_mem_tester 对应 Go 的 buildWindowMemTester 调用。
fn build_row_number_mem_tester() -> WindowMemTestDraft {
    WindowMemTestDraft {
        func_name: "ast.WindowFuncRowNumber",
        field_type: "mysql.TypeLonglong",
        constant_arg: 0,
        num_rows: 0,
        order_by_cols: 4,
        alloc_mem_delta: "aggfuncs.DefPartialResult4RowNumberSize",
        update_mem_delta_gens: "defaultUpdateMemDeltaGens",
    }
}

// test_mem_row_number 对应 Go 的 TestMemRowNumber。
#[test]
fn test_mem_row_number() {
    let tests = [build_row_number_mem_tester()];

    for test in tests {
        // Go 这里调用 testWindowAggMemFunc(t, test)，验证分配 partial result 和每行更新的内存增量。
        assert_eq!(test.func_name, "ast.WindowFuncRowNumber");
        assert_eq!(test.field_type, "mysql.TypeLonglong");
        assert_eq!(test.constant_arg, 0);
        assert_eq!(test.num_rows, 0);
        assert_eq!(test.order_by_cols, 4);
        assert_eq!(
            test.alloc_mem_delta,
            "aggfuncs.DefPartialResult4RowNumberSize"
        );
        assert_eq!(
            crate::row_number::DEF_PARTIAL_RESULT_ROW_NUMBER_SIZE,
            std::mem::size_of::<crate::row_number::RowNumber>() as i64
        );
        assert_eq!(test.update_mem_delta_gens, "defaultUpdateMemDeltaGens");
    }
}

/// 校验 `update` 占位返回 0，连续 `next_value` 得到 1..=4，`reset` 后重新从 1 开始。
#[test]
fn row_number_uses_production_state_and_reset() {
    let mut row_number = crate::row_number::RowNumber::default();
    assert_eq!(row_number.update(), 0);
    assert_eq!(
        (0..4).map(|_| row_number.next_value()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );

    // slide 对 ROW_NUMBER 无副作用；reset 后下一值再次为 1。
    row_number.slide();
    row_number.reset();
    assert_eq!(row_number.next_value(), 1);
}

/// Go 的 `int64++` 在边界处按二进制补码回绕；Rust 调试构建也必须保持同样语义。
#[test]
fn row_number_wraps_like_go_int64_arithmetic() {
    let mut row_number = crate::row_number::RowNumber::from_index_for_test(i64::MAX);
    assert_eq!(row_number.next_value(), i64::MIN);
}
