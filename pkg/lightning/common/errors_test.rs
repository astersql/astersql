// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `errors` 模块单元测试：NormalizeError / NormalizeOrWrapErr 与 CastValue 脱敏。

use crate::{
    BR_ErrPDUpdateFailed, BR_ErrStorageInvalidConfig, BR_ErrStorageInvalidPermission,
    BR_ErrStorageUnknown, ErrCastValue, ErrInvalidArgument, ErrInvalidConfig, ErrInvalidPermission,
    ErrInvalidStorageConfig, ErrStorageUnknown, ErrUnknown, ErrUpdatePD, Is, NormalizeError,
    NormalizeOrWrapErr, REDACT_LOG_ENABLED, RedactLogDisable, RedactLogEnable, RedactLogMarker,
};
use serial_test::serial;
use std::sync::atomic::Ordering;

/// 覆盖 None、未知错误包装、BR→Lightning 映射、已有 Lightning 错误保留，以及 CastValue 消息。
#[test]
#[serial]
fn test_normalize_error() {
    assert!(NormalizeError(None).is_none());
    let err = NormalizeError(Some(crate::CommonError::new("eof", "EOF"))).unwrap();
    assert!(Is(&err, &ErrUnknown));

    // (BR/源错误, annotate 前缀, 期望 Lightning ID, 期望展示消息)
    let test_cases = [
        (
            &*BR_ErrStorageUnknown,
            "ContentRange is empty",
            &*ErrStorageUnknown,
            "[Lightning:Storage:ErrStorageUnknown]ContentRange is empty",
        ),
        (
            &*BR_ErrStorageInvalidConfig,
            "host not found in endpoint",
            &*ErrInvalidStorageConfig,
            "[Lightning:Storage:ErrInvalidStorageConfig]host not found in endpoint",
        ),
        (
            &*BR_ErrStorageInvalidPermission,
            "check permission failed",
            &*ErrInvalidPermission,
            "[Lightning:Storage:ErrInvalidPermission]check permission failed",
        ),
        (
            &*BR_ErrPDUpdateFailed,
            "create pd client failed",
            &*ErrUpdatePD,
            "[Lightning:PD:ErrUpdatePD]create pd client failed",
        ),
        (
            &*ErrInvalidConfig,
            "tikv-importer.backend must not be empty!",
            &*ErrInvalidConfig,
            "[Lightning:Config:ErrInvalidConfig]tikv-importer.backend must not be empty!",
        ),
    ];

    for (rfc_err, err_msg, expect_err, expect_msg) in test_cases {
        let err = rfc_err.clone().annotate(err_msg);
        let normalized = NormalizeError(Some(err.clone())).unwrap();
        assert!(Is(&normalized, expect_err));
        assert_eq!(normalized.to_string(), expect_msg);
        assert_eq!(
            format!("{:?}", err.stack_trace()),
            format!("{:?}", normalized.stack_trace())
        );
    }

    // Go TestNormalizeError assumes default RedactLogDisable for ErrCastValue formatting.
    // 与 Go 一致：默认关闭脱敏后再格式化 CastValue。
    let original_mode = REDACT_LOG_ENABLED.load(Ordering::SeqCst);
    let _guard = scopeguard(original_mode);
    REDACT_LOG_ENABLED.store(RedactLogDisable, Ordering::SeqCst);
    let err = ErrCastValue.clone().gen_with_stack_by_args(&[
        "c1",
        "tinyint(4)",
        "\"BAD\"",
        "out of range",
    ]);
    let normalized = NormalizeError(Some(err)).unwrap();
    assert!(Is(&normalized, &ErrCastValue));
    assert_eq!(
        normalized.to_string(),
        "[Import:ErrCastValue]Value conversion failed for column 'c1'. Expected type: tinyint(4), received value: \"BAD\". Reason: out of range."
    );
}

/// 已知 RFC 错误直接返回；未知错误则用给定 rfc_error 包装。
#[test]
fn test_normalize_or_wrap_err() {
    assert!(NormalizeOrWrapErr(&ErrInvalidArgument, None, &[]).is_none());
    let err = NormalizeOrWrapErr(
        &ErrInvalidArgument,
        Some(
            ErrInvalidConfig
                .clone()
                .gen_with_stack("tikv-importer.backend must not be empty!"),
        ),
        &[],
    )
    .unwrap();
    assert!(Is(&err, &ErrInvalidConfig));
    let err = NormalizeOrWrapErr(
        &ErrInvalidArgument,
        Some(crate::CommonError::new("eof", "EOF")),
        &[],
    )
    .unwrap();
    assert!(Is(&err, &ErrInvalidArgument));

    let err = NormalizeOrWrapErr(
        &crate::ErrEncodeKV,
        Some(crate::CommonError::new("encode", "invalid datum")),
        &["data.csv", "42"],
    )
    .unwrap();
    assert_eq!(
        err.to_string(),
        "[Lightning:Restore:ErrEncodeKV]encode kv error in file data.csv at offset 42"
    );
    assert_eq!(err.stack_trace(), ["gen_with_stack_by_args"]);
}

#[test]
fn test_err_found_duplicate_keys_formats_bytes_like_go_percent_x() {
    let err = crate::ErrFoundDuplicateKeys(&[0x01, 0xab], &[0x00, 0xff]);
    assert_eq!(
        err.to_string(),
        "[Lightning:Restore:ErrFoundDuplicateKey]found duplicate key '01ab', value '00ff'"
    );
}

/// 验证 Disable / Enable / Marker 三种脱敏模式下 CastValue 用户值展示。
#[test]
#[serial]
fn test_err_cast_value_redact() {
    let original_mode = REDACT_LOG_ENABLED.load(Ordering::SeqCst);
    let _guard = scopeguard(original_mode);

    REDACT_LOG_ENABLED.store(RedactLogDisable, Ordering::SeqCst);
    let err = ErrCastValue.clone().gen_with_stack_by_args(&[
        "c1",
        "tinyint(4)",
        "\"BAD\"",
        "out of range",
    ]);
    assert_eq!(
        err.to_string(),
        "[Import:ErrCastValue]Value conversion failed for column 'c1'. Expected type: tinyint(4), received value: \"BAD\". Reason: out of range."
    );

    REDACT_LOG_ENABLED.store(RedactLogEnable, Ordering::SeqCst);
    let err = ErrCastValue.clone().gen_with_stack_by_args(&[
        "c1",
        "tinyint(4)",
        "\"BAD\"",
        "out of range",
    ]);
    assert_eq!(
        err.to_string(),
        "[Import:ErrCastValue]Value conversion failed for column 'c1'. Expected type: tinyint(4), received value: ?. Reason: out of range."
    );

    REDACT_LOG_ENABLED.store(RedactLogMarker, Ordering::SeqCst);
    let err = ErrCastValue.clone().gen_with_stack_by_args(&[
        "c1",
        "tinyint(4)",
        "\"BAD\"",
        "out of range",
    ]);
    assert_eq!(
        err.to_string(),
        "[Import:ErrCastValue]Value conversion failed for column 'c1'. Expected type: tinyint(4), received value: ‹\"BAD\"›. Reason: out of range."
    );
}

/// 测试结束时恢复全局脱敏开关，避免污染串行测试顺序。
fn scopeguard(original: u8) -> impl Drop {
    struct Guard(u8);
    impl Drop for Guard {
        fn drop(&mut self) {
            REDACT_LOG_ENABLED.store(self.0, Ordering::SeqCst);
        }
    }
    Guard(original)
}
