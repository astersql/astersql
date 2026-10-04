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

// 截断标志优先级与向量存储/算术的 Aster 单元测试。
//
// 覆盖 HandleTruncate 的 ignore/warning 语义，以及 VectorFloat32
// 解析、距离、逐元素运算与反序列化，对齐 Go 行为。

use astersql_types_vector::{
    CheckVectorDimValid, Context, CreateVectorFloat32, FLAG_IGNORE_TRUNCATE_ERR,
    FLAG_TRUNCATE_AS_WARNING, Flags, InitVectorFloat32, MustCreateVectorFloat32,
    ParseVectorFloat32, PeekBytesAsVectorFloat32, VectorFloat32, ZeroCopyDeserializeVectorFloat32,
    ZeroVectorFloat32, errno, errors,
};

/// 浮点近似相等断言（容差 1e-6）。
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "actual={actual}, expected={expected}"
    );
}

/// 按 MySQL errno 构造规范化 SQL 错误。
fn sql_error(code: u16) -> errors::SharedError {
    errors::SharedError::new(errors::Normalize(
        format!("sql error {code}"),
        &[errors::MySQLErrorCode(code as i32)],
    ))
}

/// 校验截断错误分类与 IgnoreTruncateErr 优先于 TruncateAsWarning。
#[test]
fn handle_truncate_matches_go_error_classification_and_flag_priority() {
    let truncate_codes = [
        errno::ErrTruncatedWrongValue,
        errno::ErrDataTooLong,
        errno::ErrTruncatedWrongValueForField,
        errno::ErrWarnDataOutOfRange,
        errno::ErrDataOutOfRange,
        errno::ErrBadNumber,
        errno::ErrWrongValueForType,
        errno::ErrDatetimeFunctionOverflow,
        errno::WarnDataTruncated,
        errno::ErrIncorrectDatetimeValue,
    ];

    assert!(Context::new(Flags(0)).HandleTruncate(None).is_ok());

    for code in truncate_codes {
        let mut strict = Context::new(Flags(0));
        let returned = strict.HandleTruncate(Some(sql_error(code))).unwrap_err();
        assert_eq!(
            returned
                .downcast_ref::<errors::Error>()
                .expect("root SQL error")
                .Code(),
            code as i32
        );
        assert!(strict.warnings().is_empty());

        let mut ignored = Context::new(Flags(FLAG_IGNORE_TRUNCATE_ERR));
        assert!(ignored.HandleTruncate(Some(sql_error(code))).is_ok());
        assert!(ignored.warnings().is_empty());

        let mut warned = Context::new(Flags(FLAG_TRUNCATE_AS_WARNING));
        assert!(warned.HandleTruncate(Some(sql_error(code))).is_ok());
        assert_eq!(warned.warnings().len(), 1);
        assert_eq!(
            warned.warnings()[0]
                .downcast_ref::<errors::Error>()
                .expect("warning keeps SQL error")
                .Code(),
            code as i32
        );

        // 两标志同时开启时 ignore 胜出，不写入 warning
        let mut both = Context::new(Flags(FLAG_IGNORE_TRUNCATE_ERR | FLAG_TRUNCATE_AS_WARNING));
        assert!(both.HandleTruncate(Some(sql_error(code))).is_ok());
        assert!(both.warnings().is_empty(), "ignore wins over warning");
    }

    // 包装错误应剥到根因再分类
    let root = sql_error(errno::ErrDataTooLong);
    let wrapped = errors::WithMessage(Some(root), "outer").expect("wrapped error");
    let mut warned = Context::new(Flags(FLAG_TRUNCATE_AS_WARNING));
    assert!(warned.HandleTruncate(Some(wrapped)).is_ok());
    assert_eq!(
        warned.warnings()[0]
            .downcast_ref::<errors::Error>()
            .expect("warning contains root cause")
            .Code(),
        errno::ErrDataTooLong as i32
    );

    // 非截断类错误即使开启 ignore 也原样返回
    let mut ignore_non_sql = Context::new(Flags(FLAG_IGNORE_TRUNCATE_ERR));
    let non_sql = ignore_non_sql
        .HandleTruncate(Some(errors::New("ordinary error")))
        .unwrap_err();
    assert_eq!(non_sql.to_string(), "ordinary error");

    let mut ignore_other_sql = Context::new(Flags(FLAG_IGNORE_TRUNCATE_ERR));
    let other_sql = ignore_other_sql
        .HandleTruncate(Some(sql_error(errno::ErrUnknown)))
        .unwrap_err();
    assert_eq!(
        other_sql
            .downcast_ref::<errors::Error>()
            .expect("other SQL error")
            .Code(),
        errno::ErrUnknown as i32
    );
}

/// 向量线格式、解析、截断展示与维度校验对齐 Go。
#[test]
fn vector_storage_parse_and_format_match_go() {
    let mut vector = InitVectorFloat32(2);
    vector.ElementsMut().copy_from_slice(&[1.1, 2.2]);
    assert_eq!(
        vector.SerializeTo(Vec::new()),
        vec![
            0x02, 0x00, 0x00, 0x00, 0xcd, 0xcc, 0x8c, 0x3f, 0xcd, 0xcc, 0x0c, 0x40,
        ]
    );
    assert_eq!(vector.SerializedSize(), 12);
    assert_eq!(vector.String(), "[1.1,2.2]");

    let zero = ZeroVectorFloat32();
    assert!(zero.IsZeroValue());
    assert_eq!(zero.Compare(&ZeroVectorFloat32()), 0);
    assert_eq!(zero.ZeroCopySerialize(), &[0, 0, 0, 0]);
    assert_eq!(zero.SerializedSize(), 4);
    assert_eq!(zero.SerializeTo(vec![1, 2, 3]), vec![1, 2, 3, 0, 0, 0, 0]);

    let (decoded_zero, remaining) = ZeroCopyDeserializeVectorFloat32(&[0, 0, 0, 0]).unwrap();
    assert!(remaining.is_empty());
    assert_eq!(decoded_zero.Len(), 0);
    assert_eq!(decoded_zero.String(), "[]");
    assert!(decoded_zero.IsZeroValue());
    assert_eq!(decoded_zero.Compare(&zero), 0);
    assert_eq!(zero.Compare(&decoded_zero), 0);

    for invalid in [
        "abc",
        "null",
        "\"json_str\"",
        "123",
        "[123",
        "123]",
        "[123,]",
    ] {
        let error = ParseVectorFloat32(invalid).unwrap_err();
        assert!(error.to_string().contains("Invalid vector text"));
    }
    for invalid in ["[1,2,3]extra", "[1,2,3] trailing"] {
        let error = ParseVectorFloat32(invalid).unwrap_err();
        assert!(error.to_string().contains("Invalid vector text"));
    }

    let empty = ParseVectorFloat32("[]").unwrap();
    assert_eq!(empty.Len(), 0);
    assert_eq!(empty.String(), "[]");
    assert!(empty.IsZeroValue());
    assert_eq!(empty.Compare(&zero), 0);
    assert_eq!(zero.Compare(&empty), 0);

    let parsed = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    assert_eq!(parsed.Len(), 3);
    assert_eq!(parsed.String(), "[1.1,2.2,3.3]");
    assert!(!parsed.IsZeroValue());
    assert_eq!(parsed.Clone().Elements(), parsed.Elements());
    assert_eq!(parsed.Compare(&zero), 1);
    assert_eq!(zero.Compare(&parsed), -1);

    assert_eq!(
        ParseVectorFloat32("[-1e39, 1e39]").unwrap_err().to_string(),
        "value -1e+39 out of range for float32"
    );
    assert_eq!(
        ParseVectorFloat32("[1e-30, 1e20]").unwrap().String(),
        "[0.000000000000000000000000000001,100000000000000000000]"
    );

    let truncated = MustCreateVectorFloat32(&[1.1, 2.2, 3.3, 4.4, 5.5, 123.4]);
    assert_eq!(
        truncated.TruncatedString(),
        "[1.1,2.2,3.3,4.4,5.5,(1 more)...]"
    );
    let scientific = MustCreateVectorFloat32(&[123.4, 0.001234]);
    assert_eq!(scientific.TruncatedString(), "[1.2e+02,0.0012]");

    assert!(CreateVectorFloat32(&[f32::NAN]).is_err());
    assert!(CreateVectorFloat32(&[f32::INFINITY]).is_err());
    assert!(CheckVectorDimValid(-1).is_err());
    assert!(CheckVectorDimValid(16_383).is_ok());
    assert!(CheckVectorDimValid(16_384).is_err());
    assert!(parsed.CheckDimsFitColumn(-1).is_ok());
    assert!(parsed.CheckDimsFitColumn(3).is_ok());
    assert_eq!(
        parsed.CheckDimsFitColumn(2).unwrap_err().to_string(),
        "vector has 3 dimensions, does not fit VECTOR(2)"
    );
}

/// 距离/算术/比较与带尾字节的反序列化对齐 Go。
#[test]
fn vector_deserialize_and_arithmetic_match_go() {
    let a = MustCreateVectorFloat32(&[1.0, 2.0, 3.0]);
    let b = MustCreateVectorFloat32(&[4.0, 6.0, 3.0]);

    assert_close(a.L2SquaredDistance(&b).unwrap(), 25.0);
    assert_close(a.L2Distance(&b).unwrap(), 5.0);
    assert_close(a.InnerProduct(&b).unwrap(), 25.0);
    assert_close(a.NegativeInnerProduct(&b).unwrap(), -25.0);
    assert_close(a.L1Distance(&b).unwrap(), 7.0);
    assert_close(a.L2Norm(), 14.0_f64.sqrt());
    assert_close(
        a.CosineDistance(&b).unwrap(),
        1.0 - 25.0 / (14.0_f64 * 61.0).sqrt(),
    );

    assert_eq!(a.Add(&b).unwrap().Elements(), &[5.0, 8.0, 6.0]);
    assert_eq!(a.Sub(&b).unwrap().Elements(), &[-3.0, -4.0, 0.0]);
    assert_eq!(a.Mul(&b).unwrap().Elements(), &[4.0, 12.0, 9.0]);

    assert_eq!(a.Compare(&b), -1);
    assert_eq!(b.Compare(&a), 1);
    assert_eq!(a.Compare(&a), 0);
    assert_eq!(MustCreateVectorFloat32(&[1.0, 2.0]).Compare(&a), -1);
    let go_compare_left = ParseVectorFloat32("[1.1, 2.2, 3.3]").unwrap();
    let go_compare_right = ParseVectorFloat32("[-1.1, 4.2]").unwrap();
    assert_eq!(go_compare_left.Compare(&go_compare_right), 1);
    assert_eq!(go_compare_right.Compare(&go_compare_left), -1);
    let go_compare_right = ParseVectorFloat32("[1.1, 4.2]").unwrap();
    assert_eq!(go_compare_left.Compare(&go_compare_right), -1);
    assert_eq!(go_compare_right.Compare(&go_compare_left), 1);
    assert!(
        ZeroVectorFloat32()
            .CosineDistance(&ZeroVectorFloat32())
            .unwrap()
            .is_nan()
    );

    let mismatch = MustCreateVectorFloat32(&[1.0]);
    assert_eq!(
        a.Add(&mismatch).unwrap_err().to_string(),
        "vectors have different dimensions: 3 and 1"
    );
    assert_eq!(
        MustCreateVectorFloat32(&[f32::MAX])
            .Add(&MustCreateVectorFloat32(&[f32::MAX]))
            .unwrap_err()
            .to_string(),
        "value out of range: overflow"
    );

    let mut bytes = a.SerializeTo(Vec::new());
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    assert_eq!(PeekBytesAsVectorFloat32(&bytes).unwrap(), 16);
    let (decoded, remaining) = ZeroCopyDeserializeVectorFloat32(&bytes).unwrap();
    assert_eq!(decoded.Elements(), a.Elements());
    assert_eq!(remaining, &[1, 2, 3, 4]);
    let invalid_bytes = [0xf1, 0xfc];
    assert_eq!(
        PeekBytesAsVectorFloat32(&invalid_bytes)
            .unwrap_err()
            .to_string(),
        "bad VectorFloat32 value header (len=2)"
    );
    let error = ZeroCopyDeserializeVectorFloat32(&invalid_bytes).unwrap_err();
    assert_eq!(error.to_string(), "bad VectorFloat32 value header (len=2)");

    // 反序列化可保留 NaN 位型，算术时再报错
    let nan_bytes = [1, 0, 0, 0, 0, 0, 0xc0, 0x7f];
    let (nan_vector, _) = ZeroCopyDeserializeVectorFloat32(&nan_bytes).unwrap();
    assert_eq!(
        nan_vector
            .Add(&MustCreateVectorFloat32(&[1.0]))
            .unwrap_err()
            .to_string(),
        "value out of range: NaN"
    );
}

/// Clone 后修改不回写原向量。
#[test]
fn vector_clone_is_independent() {
    let original = MustCreateVectorFloat32(&[1.0, 2.0]);
    let mut cloned: VectorFloat32 = original.Clone();
    cloned.ElementsMut()[0] = 9.0;
    assert_eq!(original.Elements(), &[1.0, 2.0]);
    assert_eq!(cloned.Elements(), &[9.0, 2.0]);
}

/// Length arithmetic must widen before multiplication and addition, including the Go overflow input.
#[test]
fn vector_deserialize_rejects_overflowing_lengths() {
    for elements in [0x3fff_ffff_u32, 0x4000_0000, u32::MAX] {
        let bytes = elements.to_le_bytes();
        let original = bytes;
        let expected_size = u64::from(elements) * 4 + 4;
        let expected = format!("bad VectorFloat32 value (len=4, expected={expected_size})");
        assert_eq!(
            PeekBytesAsVectorFloat32(&bytes).unwrap_err().to_string(),
            expected
        );
        // Result::Err exposes neither an invalid vector nor a consumed suffix.
        assert_eq!(
            ZeroCopyDeserializeVectorFloat32(&bytes)
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(bytes, original);
    }

    let zero_with_suffix = [0, 0, 0, 0, 0xaa];
    assert_eq!(PeekBytesAsVectorFloat32(&zero_with_suffix).unwrap(), 4);
    let (zero, remaining) = ZeroCopyDeserializeVectorFloat32(&zero_with_suffix).unwrap();
    assert!(zero.IsZeroValue());
    assert_eq!(remaining, &[0xaa]);

    let single = [1, 0, 0, 0, 0, 0, 0x80, 0x3f];
    assert_eq!(
        PeekBytesAsVectorFloat32(&single[..7])
            .unwrap_err()
            .to_string(),
        "bad VectorFloat32 value (len=7, expected=8)"
    );
    assert_eq!(PeekBytesAsVectorFloat32(&single).unwrap(), 8);
    let (decoded, remaining) = ZeroCopyDeserializeVectorFloat32(&single).unwrap();
    assert_eq!(decoded.Elements(), &[1.0]);
    assert!(remaining.is_empty());
}
