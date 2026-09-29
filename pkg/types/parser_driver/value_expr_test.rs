// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// ValueExpr 的 Restore/Format 单元测试，对齐 Go `TestValueExpr*`。
//
// 表驱动覆盖 NULL、整数、浮点、字符串转义、二进制字面量、Decimal、
// Duration 与 Time 等 Datum 种类的 SQL 字面量输出。

use parser_ast::{InPlaceVisitor, Node, Walk};
use parser_driver::{ValueExpr, format};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use types_decimal::mydecimal::NewDecFromInt;

thread_local! {
    static GO_MERGE_10_COUNT_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static GO_MERGE_10_ALLOCATION_COUNT: Cell<usize> = const { Cell::new(0) };
}

struct GoMerge10CountingAllocator;

#[global_allocator]
static GO_MERGE_10_ALLOCATOR: GoMerge10CountingAllocator = GoMerge10CountingAllocator;

unsafe impl GlobalAlloc for GoMerge10CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = GO_MERGE_10_COUNT_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = GO_MERGE_10_ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = GO_MERGE_10_COUNT_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = GO_MERGE_10_ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = GO_MERGE_10_COUNT_ALLOCATIONS.try_with(|enabled| {
            if enabled.get() {
                let _ = GO_MERGE_10_ALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

struct GoMerge10Visitor {
    events: Vec<&'static str>,
    skip_children: bool,
    leave_result: bool,
}

impl InPlaceVisitor for GoMerge10Visitor {
    fn enter(&mut self, node: &mut dyn Node) -> bool {
        self.events.push("enter");
        if let Some(value) = node.as_any_mut().downcast_mut::<ValueExpr>() {
            value.SetProjectionOffset(7);
        } else if let Some(marker) = node
            .as_any_mut()
            .downcast_mut::<parser_driver::ParamMarkerExpr>()
        {
            marker.SetOrder(7);
        } else {
            panic!("unexpected node");
        }
        self.skip_children
    }

    fn leave(&mut self, _node: &mut dyn Node) -> bool {
        self.events.push("leave");
        self.leave_result
    }
}

#[test]
fn go_merge_10_value_and_param_marker_walk_in_place() {
    let mut value = ValueExpr::default();
    let mut marker = parser_driver::ParamMarkerExpr::default();
    for skip_children in [false, true] {
        for leave_result in [false, true] {
            let mut visitor = GoMerge10Visitor {
                events: Vec::new(),
                skip_children,
                leave_result,
            };
            assert_eq!(Walk(&mut value, &mut visitor), leave_result);
            assert_eq!(visitor.events, ["enter", "leave"]);
            assert_eq!(value.GetProjectionOffset(), 7);

            visitor.events.clear();
            assert_eq!(Walk(&mut marker, &mut visitor), leave_result);
            assert_eq!(visitor.events, ["enter", "leave"]);
            assert_eq!(marker.Order, 7);
        }
    }
}

#[test]
fn go_merge_10_value_and_param_marker_walk_without_allocations() {
    let mut value = ValueExpr::default();
    let mut marker = parser_driver::ParamMarkerExpr::default();
    let mut visitor = GoMerge10Visitor {
        events: Vec::with_capacity(2),
        skip_children: false,
        leave_result: true,
    };

    // Warm up thread-local state and the visitor's event storage before measuring.
    GO_MERGE_10_ALLOCATION_COUNT.with(|count| count.set(0));
    assert!(Walk(&mut value, &mut visitor));
    visitor.events.clear();
    assert!(Walk(&mut marker, &mut visitor));
    visitor.events.clear();

    GO_MERGE_10_COUNT_ALLOCATIONS.with(|enabled| enabled.set(true));
    for _ in 0..100 {
        assert!(Walk(&mut value, &mut visitor));
        visitor.events.clear();
        assert!(Walk(&mut marker, &mut visitor));
        visitor.events.clear();
    }
    GO_MERGE_10_COUNT_ALLOCATIONS.with(|enabled| enabled.set(false));
    assert_eq!(GO_MERGE_10_ALLOCATION_COUNT.with(Cell::get), 0);
}

/// 用默认 Restore 标志把 Datum 还原为 SQL 字符串。
fn restore(datum: types::Datum) -> String {
    let mut expression = ValueExpr::default();
    expression.Datum = datum;
    expression
        .RestoreToString(format::DefaultRestoreFlags)
        .expect("Go restore cases must not fail")
}

/// 调用 Format 写入缓冲区并转为 UTF-8 字符串。
fn formatted(datum: types::Datum) -> String {
    let mut expression = ValueExpr::default();
    expression.Datum = datum;
    let mut output = Vec::new();
    expression.Format(&mut output);
    String::from_utf8(output).expect("formatted SQL is UTF-8")
}

#[test]
#[allow(non_snake_case)]
/// 校验 RestoreToString 与 Go 用例表一致。
fn TestValueExprRestore() {
    testsetup::SetupForCommonTest();
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewDurationDatum(types::ZeroDuration), "'00:00:00'"),
        (
            types::NewTimeDatum(types::Time::default()),
            "'0000-00-00 00:00:00'",
        ),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
    ];

    assert_eq!(cases.len(), 13);
    for (datum, expected) in cases {
        assert_eq!(restore(datum), expected);
    }
}

#[test]
#[allow(non_snake_case)]
/// 校验 Format 紧凑输出与 Go 用例表一致。
fn TestValueExprFormat() {
    testsetup::SetupForCommonTest();
    let bytes = b"test `s't\"r.".to_vec();
    let cases = vec![
        (types::Datum::default(), "NULL"),
        (types::NewIntDatum(1), "1"),
        (types::NewIntDatum(-1), "-1"),
        (types::NewUintDatum(1), "1"),
        (types::NewFloat32Datum(1.1), "1.1e+00"),
        (types::NewFloat64Datum(1.1), "1.1e+00"),
        (
            types::NewStringDatum("test `s't\"r.".to_owned()),
            "'test `s''t\"r.'",
        ),
        (types::NewBytesDatum(bytes.clone()), "'test `s''t\"r.'"),
        (
            types::NewBinaryLiteralDatum(types::BinaryLiteral(bytes)),
            "b'11101000110010101110011011101000010000001100000011100110010011101110100001000100111001000101110'",
        ),
        (types::NewDecimalDatum(NewDecFromInt(321)), "321"),
        (types::NewStringDatum("\\".to_owned()), "'\\\\'"),
        (types::NewStringDatum("''".to_owned()), "''''''"),
        (
            types::NewStringDatum("\\''\t\n".to_owned()),
            "'\\\\''''\t\n'",
        ),
    ];

    assert_eq!(cases.len(), 13);
    for (datum, expected) in cases {
        assert_eq!(formatted(datum), expected);
    }
}
