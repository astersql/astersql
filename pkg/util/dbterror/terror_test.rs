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

// `dbterror` 错误脱敏（redact）行为单测。
//
// 覆盖 `RedactLogEnable`（敏感参数替换为 `?`）与 `RedactLogMarker`（用标记符包裹）
// 两种模式，并分别走 `GenWithStackByArgs` / `FastGenByArgs` 生成路径。

use astersql_errno::errcode as errno;
use astersql_parser_terror as terror;
use astersql_parser_terror::errors::{
    ErrorArg, RedactLogEnable, RedactLogEnabled, RedactLogMarker, SharedError,
};

use crate::ErrClass;

/// 公开/非敏感参数样例，脱敏后仍应出现在消息中。
const NO_SENSITIVE_VALUE: &str = "no_sensitive";
/// 敏感参数样例；Enable 模式应变为 `?`，Marker 模式应变为带书名号标记。
const SENSITIVE_DATA: &str = "sensitive_data";
/// Marker 模式下敏感值应呈现的标记形式。
const SENSITIVE_MARKER: &str = "‹sensitive_data›";

/// RAII 守卫：测试结束时恢复全局 `RedactLogEnabled`，避免污染其它用例。
struct RedactModeGuard(&'static str);

impl Drop for RedactModeGuard {
    fn drop(&mut self) {
        RedactLogEnabled.Store(self.0);
    }
}

#[derive(Clone, Copy)]
/// 错误消息生成路径：带调用栈或快速生成。
enum Generation {
    Stack,
    Fast,
}

/// 单条脱敏用例：错误码、参数、生成方式、是否保留公开参数原文。
struct Case {
    code: u16,
    args: Vec<ErrorArg>,
    generation: Generation,
    keeps_public_value: bool,
}

/// 按用例选择 Stack/Fast 路径生成带参数的错误实例。
fn generate(class: &ErrClass, case: &Case) -> SharedError {
    let prototype = class.NewStd(case.code);
    match case.generation {
        Generation::Stack => prototype.GenWithStackByArgs(&case.args),
        Generation::Fast => prototype.FastGenByArgs(&case.args),
    }
}

/// 校验脱敏后的消息：公开值保留规则，以及 `?` / 标记符是否符合模式。
fn check_err_msg(mode: &str, message: &str, keeps_public_value: bool) {
    if keeps_public_value {
        assert!(message.contains(NO_SENSITIVE_VALUE), "{message}");
    }
    if mode == RedactLogEnable {
        assert!(message.contains('?'), "{message}");
        assert!(!message.contains(SENSITIVE_DATA), "{message}");
    } else {
        assert!(message.contains(SENSITIVE_MARKER), "{message}");
    }
}

#[test]
/// 在两种 redact 模式下遍历用例，比对完整期望消息字符串。
fn test_error_redact() {
    let original = RedactLogEnabled.Load();
    let _guard = RedactModeGuard(original);
    let class = ErrClass {
        inner: terror::ErrClass(0),
    };
    let public = || ErrorArg::from(NO_SENSITIVE_VALUE);
    let sensitive = || ErrorArg::from(SENSITIVE_DATA);
    let cases = vec![
        Case {
            code: errno::ErrDupEntry,
            args: vec![sensitive(), public()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrCutValueGroupConcat,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrDuplicatedValueInType,
            args: vec![public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrTruncatedWrongValue,
            args: vec![public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrInvalidCharacterString,
            args: vec![public(), sensitive()],
            generation: Generation::Fast,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrTruncatedWrongValueForField,
            args: vec![sensitive(), sensitive()],
            generation: Generation::Fast,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrIllegalValueForType,
            args: vec![public(), sensitive()],
            generation: Generation::Fast,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrPartitionWrongValues,
            args: vec![public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrNoParts,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrWrongValue,
            args: vec![public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrNoPartitionForGivenValue,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrDataOutOfRange,
            args: vec![public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
        Case {
            code: errno::ErrRowInWrongPartition,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrInvalidJSONText,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrTxnRetryable,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrIncorrectDatetimeValue,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrInvalidTimeFormat,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrRowNotFound,
            args: vec![sensitive()],
            generation: Generation::Stack,
            keeps_public_value: false,
        },
        Case {
            code: errno::ErrWriteConflict,
            args: vec![public(), public(), public(), sensitive()],
            generation: Generation::Stack,
            keeps_public_value: true,
        },
    ];
    let expected_on = [
        "[:1062]Duplicate entry '?' for key 'no_sensitive'",
        "[:1260]Some rows were cut by GROUPCONCAT(?)",
        "[:1291]Column 'no_sensitive' has duplicated value '?' in %!s(MISSING)",
        "[:1292]Truncated incorrect no_sensitive value: '?'",
        "[:1300]Invalid no_sensitive character string: '?'",
        "[:1366]Incorrect ? value: '?' for column '%!s(MISSING)' at row %!d(MISSING)",
        "[:1367]Illegal no_sensitive '?' value found during parsing",
        "[:1480]Only no_sensitive PARTITIONING can use VALUES ? in partition definition",
        "[:1504]Number of ? = 0 is not an allowed value",
        "[:1525]Incorrect no_sensitive value: '?'",
        "[:1526]Table has no partition for value ?",
        "[:1690]no_sensitive value is out of range in '?'",
        "[:1863]Found a row in wrong partition ?",
        "[:3140]Invalid JSON text: ?",
        "[:8022]Error: KV error safe to retry ? ",
        "[:8034]Incorrect datetime value: '?'",
        "[:8036]invalid time format: '?'",
        "[:8041]can not find the row: ?",
        "[:9007]Write conflict, txnStartTS=%!d(string=no_sensitive), conflictStartTS=%!d(string=no_sensitive), conflictCommitTS=%!d(string=no_sensitive), key=?%!s(MISSING)%!s(MISSING)%!s(MISSING), reason=%!s(MISSING)",
    ];
    let expected_marker = [
        "[:1062]Duplicate entry '‹sensitive_data›' for key 'no_sensitive'",
        "[:1260]Some rows were cut by GROUPCONCAT(‹sensitive_data›)",
        "[:1291]Column 'no_sensitive' has duplicated value '‹sensitive_data›' in %!s(MISSING)",
        "[:1292]Truncated incorrect no_sensitive value: '‹sensitive_data›'",
        "[:1300]Invalid no_sensitive character string: '‹sensitive_data›'",
        "[:1366]Incorrect ‹sensitive_data› value: '‹sensitive_data›' for column '%!s(MISSING)' at row %!d(MISSING)",
        "[:1367]Illegal no_sensitive '‹sensitive_data›' value found during parsing",
        "[:1480]Only no_sensitive PARTITIONING can use VALUES ‹sensitive_data› in partition definition",
        "[:1504]Number of ‹sensitive_data› = 0 is not an allowed value",
        "[:1525]Incorrect no_sensitive value: '‹sensitive_data›'",
        "[:1526]Table has no partition for value ‹sensitive_data›",
        "[:1690]no_sensitive value is out of range in '‹sensitive_data›'",
        "[:1863]Found a row in wrong partition ‹sensitive_data›",
        "[:3140]Invalid JSON text: ‹sensitive_data›",
        "[:8022]Error: KV error safe to retry ‹sensitive_data› ",
        "[:8034]Incorrect datetime value: '‹sensitive_data›'",
        "[:8036]invalid time format: '‹sensitive_data›'",
        "[:8041]can not find the row: ‹sensitive_data›",
        "[:9007]Write conflict, txnStartTS=%!d(string=no_sensitive), conflictStartTS=%!d(string=no_sensitive), conflictCommitTS=%!d(string=no_sensitive), key=‹sensitive_data›%!s(MISSING)%!s(MISSING)%!s(MISSING), reason=%!s(MISSING)",
    ];

    for mode in [RedactLogEnable, RedactLogMarker] {
        RedactLogEnabled.Store(mode);
        for (index, case) in cases.iter().enumerate() {
            let error = generate(&class, case);
            let message = error.to_string();
            check_err_msg(mode, &message, case.keeps_public_value);
            let expected = if mode == RedactLogEnable {
                expected_on[index]
            } else {
                expected_marker[index]
            };
            assert_eq!(message, expected, "error code {}", case.code);
            if case.code == errno::ErrTxnRetryable {
                // The Go test intentionally repeats this assertion.
                check_err_msg(mode, &message, case.keeps_public_value);
            }
        }
    }
}
