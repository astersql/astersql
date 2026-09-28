// Copyright 2026 AsterSQL.

// `configInspection` 可读容量字符串解析的内部单元测试。
//
// 验证 `convertReadableSizeToByteSize` 对 KiB/MiB 等二进制单位的换算，
// 以及非法输入返回错误。

use crate::inspection_result::{configInspection, inspectionName};

#[test]
/// 合法单位换算到字节，非法字符串应失败。
fn readable_size_conversion_keeps_binary_units_and_rejects_invalid_input() {
    let inspection = configInspection {
        inspectionName: inspectionName("config".to_owned()),
    };

    let test_cases = [
        ("100", 100),
        ("1KiB", 1_024),
        ("1MiB", 1_048_576),
        ("1GiB", 1_073_741_824),
        ("1TiB", 1_099_511_627_776),
        ("1PiB", 1_125_899_906_842_624),
        ("100B", 100),
    ];

    for (input, expected) in test_cases {
        assert_eq!(
            inspection.convertReadableSizeToByteSize(input).unwrap(),
            expected,
            "input: {input}"
        );
    }

    assert!(inspection.convertReadableSizeToByteSize("abc").is_err());
}
