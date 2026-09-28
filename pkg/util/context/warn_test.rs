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

#![allow(dead_code, non_snake_case)]
// SQL warning 单元测试，对应 Go `warn_test.go`。
//
// 覆盖 SQLWarn JSON 往返、IgnoreWarn 空操作、StaticWarnHandler 复制/截断
// 底层数组独立性，以及从已有 handler 克隆。

use util_context::errors;
use util_context::parser::terror;
use util_context::warn::*;

// TestSQLWarn 对应 Go 的 JSON 往返测试，覆盖普通错误、Trace 包装 terror.Error 和 io.EOF。
/// JSON 往返：普通错误、Trace 包装、terror.Error 与 EOF 文案。
#[test]
fn TestSQLWarn() {
    let warns = vec![
        SQLWarn {
            Level: WarnLevelError.to_string(),
            Err: Some(errors::New("any error")),
        },
        SQLWarn {
            Level: WarnLevelError.to_string(),
            Err: errors::Trace(Some(errors::New("any error"))),
        },
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: Some(
                terror::ErrResultUndetermined
                    .GenWithStackByArgs(&[errors::ErrorArg::String("unknown".to_owned())]),
            ),
        },
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: errors::Trace(Some(
                terror::ErrResultUndetermined
                    .GenWithStackByArgs(&[errors::ErrorArg::String("unknown".to_owned())]),
            )),
        },
        SQLWarn {
            Level: WarnLevelNote.to_string(),
            Err: Some(errors::New("EOF")),
        },
    ];

    let encoded: Vec<serde_json::Value> = warns
        .iter()
        .map(|warn| {
            serde_json::from_slice(&warn.MarshalJSON().expect("marshal SQLWarn"))
                .expect("decode marshaled SQLWarn")
        })
        .collect();
    let d = serde_json::to_vec(&encoded).expect("marshal SQLWarn array");
    let decoded: Vec<serde_json::Value> =
        serde_json::from_slice(&d).expect("unmarshal SQLWarn array");
    let newWarns: Vec<SQLWarn> = decoded
        .iter()
        .map(|value| {
            let mut warn = SQLWarn {
                Level: String::new(),
                Err: None,
            };
            warn.UnmarshalJSON(&serde_json::to_vec(value).expect("encode SQLWarn value"))
                .expect("unmarshal SQLWarn");
            warn
        })
        .collect();

    for (i, warn) in warns.iter().enumerate() {
        assert_eq!(warn.Level, newWarns[i].Level, "{}", i);
        assert_eq!(
            warn.Err.as_ref().unwrap().to_string(),
            newWarns[i].Err.as_ref().unwrap().to_string(),
            "{}",
            i
        );
    }
}

// Go 的 MarshalJSON 会对 errors.Cause(nil) 调用 Error，因此 nil error 会 panic。
#[test]
#[should_panic(expected = "SQLWarn.MarshalJSON requires Err")]
fn TestSQLWarnMarshalNilErrorPanics() {
    let warn = SQLWarn {
        Level: WarnLevelWarning.to_string(),
        Err: None,
    };

    let _ = warn.MarshalJSON();
}

// TestIgnoreWarn 对应 Go 的忽略型 handler 测试：所有追加、复制、截断都不记录 warning。
/// IgnoreWarn：追加、复制、截断均不记录且计数恒为 0。
#[test]
fn TestIgnoreWarn() {
    assert_eq!(0, IgnoreWarn.WarningCount());

    IgnoreWarn.AppendWarning(errors::New("warn0"));
    assert_eq!(0, IgnoreWarn.WarningCount());

    assert!(IgnoreWarn.CopyWarnings(Vec::new()).is_empty());
    assert!(IgnoreWarn.CopyWarnings(Vec::with_capacity(8)).is_empty());
    assert_eq!(0, IgnoreWarn.WarningCount());

    IgnoreWarn.AppendWarning(errors::New("warn1"));
    assert!(IgnoreWarn.TruncateWarnings(0).is_empty());
    assert_eq!(0, IgnoreWarn.WarningCount());
}

// TestStaticWarnHandler 对应 Go 的静态 warning handler 测试，重点是拷贝时不复用内部底层数组。
/// StaticWarnHandler：CopyWarnings 容量分支与 TruncateWarnings 边界。
#[test]
fn TestStaticWarnHandler() {
    let h = NewStaticWarnHandler(0);
    assert_eq!(0, h.WarningCount());
    h.AppendWarning(errors::NewNoStackError("warn0"));
    h.AppendWarning(errors::NewNoStackError("warn1"));
    h.AppendWarning(errors::NewNoStackError("warn2"));
    h.AppendWarning(errors::NewNoStackError("warn3"));
    assert_eq!(4, h.WarningCount());

    let expected = vec![
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: Some(errors::NewNoStackError("warn0")),
        },
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: Some(errors::NewNoStackError("warn1")),
        },
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: Some(errors::NewNoStackError("warn2")),
        },
        SQLWarn {
            Level: WarnLevelWarning.to_string(),
            Err: Some(errors::NewNoStackError("warn3")),
        },
    ];

    // Copy warnings with nil dst.
    // dst 为 nil/空：应新分配且与内部底层数组不同。
    let mut got = h.CopyWarnings(Vec::new());
    assertWarningsEqual(&expected, &got);
    assertWarningsEqual(&expected, &h.warnings.lock().expect("warnings lock"));
    assert_eq!(4, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));

    // Copy warnings to dst with enough capacity.
    // dst 容量足够：应复用传入缓冲指针。
    let mut dst = Vec::with_capacity(8);
    let dst_ptr = dst.as_ptr();
    got = h.CopyWarnings(dst);
    assertWarningsEqual(&expected, &got);
    assertWarningsEqual(&expected, &h.warnings.lock().expect("warnings lock"));
    assert_eq!(4, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));
    assert_eq!(dst_ptr, got.as_ptr());

    // Copy warnings to dst without enough capacity.
    // dst 容量不足：应重新分配。
    dst = Vec::with_capacity(1);
    let dst_ptr = dst.as_ptr();
    got = h.CopyWarnings(dst);
    assertWarningsEqual(&expected, &got);
    assertWarningsEqual(&expected, &h.warnings.lock().expect("warnings lock"));
    assert_eq!(4, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));
    assert_ne!(dst_ptr, got.as_ptr());

    // Copy warnings to dst with enough capacity but len(dst) < len(h.warnings).
    // dst 有容量但 len 小于 warnings：clear 后复用指针。
    dst = Vec::with_capacity(8);
    dst.push(SQLWarn {
        Level: String::new(),
        Err: None,
    });
    let dst_ptr = dst.as_ptr();
    got = h.CopyWarnings(dst);
    assertWarningsEqual(&expected, &got);
    assertWarningsEqual(&expected, &h.warnings.lock().expect("warnings lock"));
    assert_eq!(4, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));
    assert_eq!(dst_ptr, got.as_ptr());

    // Truncate warnings with start that is out of index
    // start 越界：返回空且不改动内部。
    let warning_count = h.warnings.lock().expect("warnings lock").len() as isize;
    got = h.TruncateWarnings(warning_count);
    assert!(got.is_empty());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));
    got = h.TruncateWarnings(warning_count + 1);
    assert!(got.is_empty());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));

    // Truncate warnings with start that is in index
    // start 在范围内：截出后缀并缩短内部。
    got = h.TruncateWarnings(2);
    assertWarningsEqual(&expected[2..], &got);
    assertWarningsEqual(&expected[..2], &h.warnings.lock().expect("warnings lock"));
    assert_eq!(2, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));

    // Truncate warnings with 0
    got = h.TruncateWarnings(0);
    assertWarningsEqual(&expected[..2], &got);
    assert!(h.warnings.lock().expect("warnings lock").is_empty());
    assert_eq!(0, h.WarningCount());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));

    // Copy when warnings are empty
    got = h.CopyWarnings(Vec::new());
    assert!(got.is_empty());
    assert!(h.warnings.lock().expect("warnings lock").is_empty());
    assert!(!warnSliceInnerArrayEqual(
        &h.warnings.lock().expect("warnings lock"),
        &got
    ));
}

// TestCopyWarnHandler 对应 Go 的 NewStaticWarnHandlerWithHandler 测试，确认复制后底层数组独立。
/// NewStaticWarnHandlerWithHandler：深拷贝后底层数组独立；None 得空 handler。
#[test]
fn TestCopyWarnHandler() {
    let h1 = NewStaticWarnHandler(0);
    h1.AppendWarning(errors::NewNoStackError("warn0"));
    h1.AppendWarning(errors::NewNoStackError("warn1"));
    h1.AppendWarning(errors::NewNoStackError("warn2"));

    let h2 = NewStaticWarnHandlerWithHandler(Some(&h1));
    assert_eq!(3, h2.WarningCount());
    assertWarningsEqual(
        &h2.warnings.lock().expect("warnings lock"),
        &[
            SQLWarn {
                Level: WarnLevelWarning.to_string(),
                Err: Some(errors::NewNoStackError("warn0")),
            },
            SQLWarn {
                Level: WarnLevelWarning.to_string(),
                Err: Some(errors::NewNoStackError("warn1")),
            },
            SQLWarn {
                Level: WarnLevelWarning.to_string(),
                Err: Some(errors::NewNoStackError("warn2")),
            },
        ],
    );
    assert!(!warnSliceInnerArrayEqual(
        &h1.warnings.lock().expect("warnings lock"),
        &h2.warnings.lock().expect("warnings lock"),
    ));

    let h2 = NewStaticWarnHandlerWithHandler(None);
    assert_eq!(0, h2.WarningCount());
}

// warnSliceInnerArrayEqual 对应 Go 的 unsafe.SliceData 比较；这里只比较 Vec 的 data 指针。
/// 比较两切片 data 指针是否相同（对应 Go unsafe.SliceData）。
fn warnSliceInnerArrayEqual(a: &[SQLWarn], b: &[SQLWarn]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    std::ptr::eq(a.as_ptr(), b.as_ptr())
}

fn assertWarningsEqual(expected: &[SQLWarn], actual: &[SQLWarn]) {
    // 按 Level 与错误文案逐条断言 warning 切片相等。
    assert_eq!(expected.len(), actual.len());
    for (i, (expected, actual)) in expected.iter().zip(actual).enumerate() {
        assert_eq!(expected.Level, actual.Level, "warning {i} level");
        assert_eq!(
            expected.Err.as_ref().map(ToString::to_string),
            actual.Err.as_ref().map(ToString::to_string),
            "warning {i} error",
        );
    }
}
