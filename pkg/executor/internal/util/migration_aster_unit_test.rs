// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// `executor/internal/util` 迁移期单元测试。
//
// 覆盖 `UpdateExecutorTableID` 对各扫描变体、递归/非递归路径、Join 与未知协议的行为，
// 以及随机串字母表、调用方函数名探测与 spill 临时文件泄漏检查。

use std::collections::HashSet;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use astersql_executor_internal_util::{
    CheckNoLeakFiles, ExecType, Executor, GenerateRandomString, GetFunctionName, LETTER_BYTES,
    UpdateExecutorTableID, UpdateExecutorTableIDContext,
};

/// 验证 TableScan / PartitionTableScan / IndexScan 的 ID 改写及 recording 上下文。
#[test]
fn update_executor_table_id_handles_scan_variants_and_context() {
    let mut visited = HashSet::new();
    let mut table_scan = Executor::table_scan(7);
    // TableScan：取 partition_ids[0]，并记入 visited。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::recording(&mut visited),
        Some(&mut table_scan),
        false,
        &[42, 43],
    )
    .unwrap();
    assert_eq!(table_scan, Executor::table_scan(42));
    assert_eq!(visited, HashSet::from([42]));

    let mut partition_scan = Executor::partition_table_scan(vec![1]);
    // PartitionTableScan：整表替换为传入的分区 ID 列表。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut partition_scan),
        false,
        &[42, 43],
    )
    .unwrap();
    assert_eq!(partition_scan, Executor::partition_table_scan(vec![42, 43]));

    let mut index_scan = Executor::index_scan(9);
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut index_scan),
        false,
        &[42],
    )
    .unwrap();
    assert_eq!(index_scan, Executor::index_scan(42));
}

/// 对齐 Go：recursive=true 下降一元子树；false 不改子节点；Join 只改非内表侧。
#[test]
fn update_executor_table_id_matches_recursive_go_paths() {
    let unary_types = [
        ExecType::Selection,
        ExecType::Aggregation,
        ExecType::StreamAgg,
        ExecType::TopN,
        ExecType::Limit,
        ExecType::ExchangeSender,
        ExecType::CteSink,
        ExecType::Projection,
        ExecType::Window,
        ExecType::Sort,
        ExecType::Expand,
        ExecType::Expand2,
    ];
    for tp in unary_types {
        let mut executor = Executor::unary(tp, Executor::table_scan(1));
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut executor),
            true,
            &[88],
        )
        .unwrap();
        assert_eq!(executor.child().unwrap(), &Executor::table_scan(88));
    }

    let mut non_recursive = Executor::unary(ExecType::Limit, Executor::table_scan(2));
    // recursive=false：当前节点无扫描载荷，子节点保持原 ID。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut non_recursive),
        false,
        &[99],
    )
    .unwrap();
    assert_eq!(non_recursive.child().unwrap(), &Executor::table_scan(2));

    for inner_idx in 0..2 {
        let mut join = Executor::join(
            [Executor::table_scan(3), Executor::table_scan(4)],
            inner_idx,
        );
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut join),
            true,
            &[66],
        )
        .unwrap();
        let children = join.join_children().unwrap();
        assert_eq!(
            children[inner_idx],
            &Executor::table_scan(3 + inner_idx as i64)
        );
        assert_eq!(children[1 - inner_idx], &Executor::table_scan(66));
    }
}

/// 覆盖 nil 执行器、无子节点终端算子与未知 tipb 协议错误。
#[test]
fn update_executor_table_id_covers_terminal_unknown_and_nil() {
    UpdateExecutorTableID(UpdateExecutorTableIDContext::default(), None, true, &[1]).unwrap();

    for tp in [ExecType::ExchangeReceiver, ExecType::CteSource] {
        let mut terminal = Executor::terminal(tp);
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut terminal),
            true,
            &[1],
        )
        .unwrap();
    }

    let mut unknown = Executor::unknown(1234);
    let err = UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut unknown),
        true,
        &[1],
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "unknown new tipb protocol 1234");
}

/// 对齐 Go 的切片边界：普通扫描要求首个分区 ID，分区扫描允许空列表。
#[test]
fn update_executor_table_id_matches_go_partition_id_boundaries() {
    for mut scan in [Executor::table_scan(1), Executor::index_scan(1)] {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = UpdateExecutorTableID(
                UpdateExecutorTableIDContext::default(),
                Some(&mut scan),
                false,
                &[],
            );
        }));
        assert!(panic.is_err());
    }

    let mut partition_scan = Executor::partition_table_scan(vec![1]);
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut partition_scan),
        false,
        &[],
    )
    .unwrap();
    assert_eq!(partition_scan, Executor::partition_table_scan(vec![]));
}

/// 随机串长度与 Go `LETTER_BYTES` 字母表一致。
#[test]
fn random_string_has_requested_length_and_go_alphabet() {
    assert_eq!(
        LETTER_BYTES,
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    );
    assert_eq!(GenerateRandomString(0), "");
    let value = GenerateRandomString(512);
    assert_eq!(value.len(), 512);
    assert!(
        value
            .bytes()
            .all(|byte| LETTER_BYTES.as_bytes().contains(&byte))
    );
}

/// 供 `GetFunctionName` 探测调用栈时识别的探针函数。
fn function_name_probe() -> String {
    GetFunctionName()
}

/// 验证 `GetFunctionName` 返回其直接调用方名称。
#[test]
fn function_name_reports_its_caller() {
    assert!(function_name_probe().contains("function_name_probe"));
}

/// 泄漏检查递归遍历目录，仅匹配给定前缀的文件名。
#[test]
fn leak_check_walks_recursively_and_matches_only_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("nested")).unwrap();
    fs::create_dir(temp.path().join("nested/spill-directory")).unwrap();
    fs::write(temp.path().join("nested/other-file"), b"ok").unwrap();
    CheckNoLeakFiles(temp.path(), "spill-").unwrap();

    fs::write(temp.path().join("nested/spill-0001"), b"leak").unwrap();
    let err = CheckNoLeakFiles(temp.path(), "spill-").unwrap_err();
    assert!(err.path().ends_with("spill-0001"));
}

/// Go 的 `WalkDir` 对所有非目录项检查前缀，符号链接也必须按泄漏处理。
#[cfg(unix)]
#[test]
fn leak_check_matches_go_for_prefixed_symbolic_links() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("target"), b"ok").unwrap();
    symlink(temp.path().join("target"), temp.path().join("spill-link")).unwrap();

    let err = CheckNoLeakFiles(temp.path(), "spill-").unwrap_err();
    assert!(err.path().ends_with("spill-link"));
}

#[test]
fn port_fba22fa599_float_layouts_and_nulls() {
    use crate::MarkTouchedRowsByColumn;
    use astersql_parser_types::{NewFieldType, mysql::*};
    use astersql_util_chunk::newFixedLenColumn;
    let rows = [
        (0., -0., false, false),
        (1., 9., false, false),
        (-0., 0., false, false),
        (2., 3., false, false),
        (4., 4., false, false),
        (5., 5., true, true),
        (6., 7., true, false),
        (7., 8., false, true),
    ];
    for tp in [TypeFloat, TypeDouble] {
        for not_null in [false, true] {
            let field = NewFieldType(tp);
            let size = if tp == TypeFloat { 4 } else { 8 };
            let mut old = newFixedLenColumn(size, 8);
            let mut new = newFixedLenColumn(size, 8);
            for (a, b, an, bn) in rows {
                for (col, v, is_null) in [(&mut old, a, an), (&mut new, b, bn)] {
                    if is_null && !not_null {
                        col.AppendNull();
                    } else if tp == TypeFloat {
                        col.AppendFloat32(v as f32);
                    } else {
                        col.AppendFloat64(v);
                    }
                }
            }
            for (stride, bit, want) in [
                (1, 3, vec![0x48, 0x40, 0x48, 0x40, 0x40, 0x48]),
                (
                    2,
                    11,
                    vec![
                        0x40, 0x48, 0x40, 0x40, 0x40, 0x48, 0x40, 0x40, 0x40, 0x40, 0x40, 0x48,
                    ],
                ),
            ] {
                let mut bitmap = vec![0x40; 6 * stride];
                MarkTouchedRowsByColumn(
                    &[7, 0, 3, 2, 5, 6],
                    &mut bitmap,
                    stride,
                    bit,
                    &old,
                    &new,
                    Some(&field),
                    not_null,
                    "aggregate change",
                )
                .unwrap();
                assert_eq!(bitmap, want);
            }
        }
    }
}

#[test]
fn port_fba22fa599_bit_varlen_and_fixed() {
    use crate::MarkTouchedRowsByColumn;
    use astersql_parser_types::{NewFieldType, mysql::*};
    use astersql_util_chunk::{newFixedLenColumn, newVarLenColumn};
    for (a, b) in [
        (vec![2], vec![3]),
        (
            vec![0x80, 0, 0, 0, 0, 0, 0, 2],
            vec![0x80, 0, 0, 0, 0, 0, 0, 3],
        ),
    ] {
        for not_null in [false, true] {
            let field = NewFieldType(TypeBit);
            let rows = if not_null {
                vec![
                    (Some(&a), Some(&b)),
                    (Some(&a), Some(&a)),
                    (Some(&b), Some(&b)),
                    (Some(&b), Some(&a)),
                ]
            } else {
                vec![
                    (None, None),
                    (Some(&a), Some(&b)),
                    (Some(&a), Some(&a)),
                    (None, Some(&a)),
                    (Some(&a), None),
                    (None, None),
                    (Some(&a), Some(&b)),
                ]
            };
            let mut old = newVarLenColumn(8);
            let mut new = newVarLenColumn(8);
            for (a, b) in &rows {
                for (col, v) in [(&mut old, a), (&mut new, b)] {
                    if let Some(v) = v {
                        col.AppendBytes(v);
                    } else {
                        col.AppendNull();
                    }
                }
            }
            let mut bitmap = vec![0; rows.len()];
            MarkTouchedRowsByColumn(
                &(0..rows.len()).collect::<Vec<_>>(),
                &mut bitmap,
                1,
                0,
                &old,
                &new,
                Some(&field),
                not_null,
                "aggregate change",
            )
            .unwrap();
            assert_eq!(
                bitmap,
                if not_null {
                    vec![1, 0, 0, 1]
                } else {
                    vec![0, 1, 0, 1, 1, 0, 1]
                }
            );
        }
    }
    let mut field = NewFieldType(TypeBit);
    field.AddFlag(UnsignedFlag | NotNullFlag);
    let mut old = newFixedLenColumn(8, 3);
    let mut new = newFixedLenColumn(8, 3);
    for (a, b) in [(0, 0), (1 << 63, 1 << 63), (1 << 63, (1 << 63) + 1)] {
        old.AppendUint64(a);
        new.AppendUint64(b);
    }
    let mut bitmap = vec![0; 3];
    MarkTouchedRowsByColumn(
        &[0, 1, 2],
        &mut bitmap,
        1,
        0,
        &old,
        &new,
        Some(&field),
        true,
        "aggregate change",
    )
    .unwrap();
    assert_eq!(bitmap, vec![0, 0, 1]);
}

#[test]
fn port_fba22fa599_binary_names_decimal_and_errors() {
    use crate::MarkTouchedRowsByColumn;
    use astersql_parser_types::{NewFieldType, mysql::*};
    use astersql_util_chunk::{newFixedLenColumn, newVarLenColumn};
    for tp in [TypeVarString, TypeEnum, TypeSet] {
        for not_null in [false, true] {
            let mut field = NewFieldType(tp);
            field.SetCollate("utf8mb4_general_ci".into());
            let mut old = newVarLenColumn(8);
            let mut new = newVarLenColumn(8);
            for (a, b, an, bn) in [
                ("x", "x", false, false),
                ("a", "A", false, false),
                ("", "", true, true),
                ("x", "x", true, false),
                ("x", "x", false, true),
            ] {
                for (col, name, null, value) in [(&mut old, a, an, 1), (&mut new, b, bn, 2)] {
                    if null && !not_null {
                        col.AppendNull();
                    } else if tp == TypeVarString {
                        col.AppendString(name);
                    } else {
                        col.appendNameValue(name, value);
                    }
                }
            }
            let mut bitmap = vec![0; 5];
            MarkTouchedRowsByColumn(
                &[0, 1, 2, 3, 4],
                &mut bitmap,
                1,
                0,
                &old,
                &new,
                Some(&field),
                not_null,
                "name",
            )
            .unwrap();
            assert_eq!(
                bitmap,
                if not_null {
                    vec![0, 1, 0, 0, 0]
                } else {
                    vec![0, 1, 0, 1, 1]
                }
            );
        }
    }
    let field = NewFieldType(TypeNewDecimal);
    let mut old = newFixedLenColumn(astersql_types_datum::MyDecimalStructSize, 8);
    let mut new = newFixedLenColumn(astersql_types_datum::MyDecimalStructSize, 8);
    for (a, b, an, bn) in [
        ("1.0", "1.00", false, false),
        ("0", "0.00", false, false),
        ("-1.0", "-1.000", false, false),
        ("9", "10", false, false),
        ("0", "1", true, false),
        ("1", "0", false, true),
        ("1", "2", true, true),
    ] {
        for (col, v, null) in [(&mut old, a, an), (&mut new, b, bn)] {
            if null {
                col.AppendNull();
            } else {
                let mut dec = astersql_types_datum::MyDecimal::default();
                dec.FromString(v.as_bytes()).unwrap();
                col.AppendMyDecimal(&dec);
            }
        }
    }
    let mut bitmap = vec![0; 7];
    MarkTouchedRowsByColumn(
        &[0, 1, 2, 3, 4, 5, 6],
        &mut bitmap,
        1,
        0,
        &old,
        &new,
        Some(&field),
        false,
        "decimal",
    )
    .unwrap();
    assert_eq!(bitmap, vec![0, 0, 0, 1, 1, 1, 0]);
    assert!(
        MarkTouchedRowsByColumn(&[], &mut [], 1, 0, &old, &new, None, false, "test")
            .unwrap_err()
            .contains("field type is nil")
    );
    MarkTouchedRowsByColumn(&[], &mut [], 1, 0, &old, &new, Some(&field), false, "test").unwrap();
}

#[test]
fn port_fba22fa599_decimal_padded_precision() {
    use astersql_parser_types::{NewFieldType, mysql::TypeNewDecimal};
    use astersql_types_datum::{MyDecimal, MyDecimalStructSize};
    use astersql_util_chunk::newFixedLenColumn;
    let mut compact = MyDecimal::default();
    compact.FromString(b"50").unwrap();
    let (encoded, err) = compact.ToBin(32, 0);
    err.unwrap();
    let mut padded = MyDecimal::default();
    padded.FromBin(&encoded, 32, 0).1.unwrap();
    assert_eq!(padded.Compare(&compact), 0);
    assert_ne!(padded.GetDigitsInt(), compact.GetDigitsInt());
    let mut different = MyDecimal::default();
    different.FromString(b"51").unwrap();
    let rows = [
        (Some(&padded), Some(&compact)),
        (Some(&compact), Some(&padded)),
        (Some(&compact), Some(&compact)),
        (Some(&padded), Some(&padded)),
        (Some(&padded), Some(&different)),
        (None, None),
        (None, Some(&compact)),
        (Some(&compact), None),
    ];
    let mut old = newFixedLenColumn(MyDecimalStructSize, 8);
    let mut new = newFixedLenColumn(MyDecimalStructSize, 8);
    for (a, b) in rows {
        for (col, value) in [(&mut old, a), (&mut new, b)] {
            if let Some(value) = value {
                col.AppendMyDecimal(value);
            } else {
                col.AppendNull();
            }
        }
    }
    let field = NewFieldType(TypeNewDecimal);
    let mut bitmap = vec![0; 8];
    crate::MarkTouchedRowsByColumn(
        &[0, 1, 2, 3, 4, 5, 6, 7],
        &mut bitmap,
        1,
        0,
        &old,
        &new,
        Some(&field),
        false,
        "aggregate change",
    )
    .unwrap();
    assert_eq!(bitmap, vec![0, 0, 0, 0, 1, 0, 1, 1]);
}
