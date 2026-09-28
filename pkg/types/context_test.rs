// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Context / Flags 行为单测：不可变更新、开关读写与告警存储。
//
// 验证 WithFlags 不改动原实例，各 Flag 的 With* / 读接口对称，
// 以及 WarnAppender 桩的 warning 收集与 AppendNote 未实现 panic。

use std::sync::{Arc, Mutex};

use types_group_1::{
    FlagAllowNegativeToUnsigned, FlagSkipASCIICheck, FlagSkipUTF8Check, FlagSkipUTF8MB4Check,
    Flags, NewContext, StrictFlags, TypeWarnAppender as WarnAppender, errors as context_errors,
};

/// WithFlags 返回新 Context，原 Flags 与 Location 保持不变。
#[test]
fn test_with_new_flags() {
    let context = NewContext(
        FlagSkipASCIICheck,
        chrono_tz::UTC,
        Arc::new(WarnStore::default()),
    );
    let changed = context.WithFlags(FlagSkipUTF8Check);

    assert_eq!(FlagSkipASCIICheck, context.Flags());
    assert_eq!(FlagSkipUTF8Check, changed.Flags());
    assert_eq!(chrono_tz::UTC, context.Location());
    assert_eq!(chrono_tz::UTC, changed.Location());
}

/// 表驱动校验常见 Flag 的开启/关闭与全位置位交互。
#[test]
fn test_simple_on_off_flags() {
    type ReadFlag = fn(Flags) -> bool;
    type WriteFlag = fn(Flags, bool) -> Flags;

    let cases: [(&str, Flags, ReadFlag, WriteFlag); 4] = [
        (
            "FlagAllowNegativeToUnsigned",
            FlagAllowNegativeToUnsigned,
            Flags::AllowNegativeToUnsigned,
            Flags::WithAllowNegativeToUnsigned,
        ),
        (
            "FlagSkipASCIICheck",
            FlagSkipASCIICheck,
            Flags::SkipASCIICheck,
            Flags::WithSkipSACIICheck,
        ),
        (
            "FlagSkipUTF8Check",
            FlagSkipUTF8Check,
            Flags::SkipUTF8Check,
            Flags::WithSkipUTF8Check,
        ),
        (
            "FlagSkipUTF8MB4Check",
            FlagSkipUTF8MB4Check,
            Flags::SkipUTF8MB4Check,
            Flags::WithSkipUTF8MB4Check,
        ),
    ];

    for (name, flag, read, write) in cases {
        assert!(!read(StrictFlags), "case: {name}");
        assert!(!read(Flags(0)), "case: {name}");
        assert!(read(flag), "case: {name}");

        // 从空标志开启单个位
        let enabled = write(Flags(0), true);
        assert_eq!(flag, enabled, "case: {name}");
        assert!(read(enabled), "case: {name}");
        let all_enabled = write(!Flags(0), true);
        assert_eq!(!Flags(0), all_enabled, "case: {name}");
        assert!(read(all_enabled), "case: {name}");

        // 关闭单个位不影响“从零关闭”仍为零，全 1 关闭后清除该位
        let disabled = write(Flags(0), false);
        assert_eq!(Flags(0), disabled, "case: {name}");
        assert!(!read(disabled), "case: {name}");
        let one_disabled = write(!Flags(0), false);
        assert_eq!(!flag, one_disabled, "case: {name}");
        assert!(!read(one_disabled), "case: {name}");
    }
}

/// 线程安全收集 warning 的测试用 WarnAppender。
#[derive(Default)]
struct WarnStore {
    warnings: Mutex<Vec<context_errors::SharedError>>,
}

impl WarnAppender for WarnStore {
    fn AppendWarning(&self, warning: context_errors::SharedError) {
        self.warnings.lock().unwrap().push(warning);
    }

    fn AppendNote(&self, _note: context_errors::SharedError) {
        panic!("not implemented");
    }
}

impl WarnStore {
    /// 清空已收集的 warning。
    fn reset(&self) {
        self.warnings.lock().unwrap().clear();
    }

    /// 返回当前 warning 快照。
    fn get_warnings(&self) -> Vec<context_errors::SharedError> {
        self.warnings.lock().unwrap().clone()
    }
}

/// 校验 AppendWarning 可收集，AppendNote 未实现会 panic。
#[test]
fn warn_store_matches_go_helper() {
    let store = WarnStore::default();
    store.AppendWarning(context_errors::New("warning"));
    assert_eq!(store.get_warnings().len(), 1);
    store.reset();
    assert!(store.get_warnings().is_empty());
    assert!(
        std::panic::catch_unwind(|| {
            store.AppendNote(context_errors::New("note"));
        })
        .is_err()
    );
}
