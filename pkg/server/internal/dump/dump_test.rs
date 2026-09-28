// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// dump 模块与 Go 对照的协议编码单测。
//
// 覆盖 BinaryDateTime / BinaryTime、长度编码整数边界，以及 Uint64 小端往返。

use dump::{BinaryDateTime, BinaryTime, LengthEncodedInt, Uint64};
use time::Duration;

// test_dump_binary_time 对应 Go 的 TestDumpBinaryTime，覆盖零时间、日期时间、日期和 duration 编码。
/// 对应 Go TestDumpBinaryTime：零值、带微秒 DATETIME、DATE、负/正 duration。
#[test]
pub fn test_dump_binary_time() {
    let type_ctx = types::BasicTimeContext::default();
    let utc_type_ctx = types::BasicTimeContext {
        location: chrono_tz::UTC,
        ..type_ctx
    };

    let mut parsed_time = types::ParseTimestamp(&type_ctx, "0000-00-00 00:00:00.000000").unwrap();
    let mut d = BinaryDateTime(vec![], parsed_time);
    assert_eq!(vec![0_u8], d);

    parsed_time = types::ParseTimestamp(&utc_type_ctx, "1991-05-01 01:01:01.100001").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    // 199 & 7 composed to uint16 1991 (litter-endian)
    // 160 & 134 & 1 & 0 composed to uint32 1000001 (litter-endian)
    assert_eq!(vec![11, 199, 7, 5, 1, 1, 1, 1, 161, 134, 1, 0], d);

    parsed_time = types::ParseDatetime(&type_ctx, "0000-00-00 00:00:00.000000").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    assert_eq!(vec![0_u8], d);

    parsed_time = types::ParseDatetime(&type_ctx, "1993-07-13 01:01:01.000000").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    // 201 & 7 composed to uint16 1993 (litter-endian)
    assert_eq!(vec![7, 201, 7, 7, 13, 1, 1, 1], d);

    parsed_time = types::ParseDate(&type_ctx, "0000-00-00").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    assert_eq!(vec![0_u8], d);

    parsed_time = types::ParseDate(&type_ctx, "1992-06-01").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    // 200 & 7 composed to uint16 1992 (litter-endian)
    assert_eq!(vec![4, 200, 7, 6, 1], d);

    parsed_time = types::ParseDate(&type_ctx, "0000-00-00").unwrap();
    d = BinaryDateTime(vec![], parsed_time);
    assert_eq!(vec![0_u8], d);

    let (my_duration, _) =
        types::ParseDuration(&type_ctx, "0000-00-00 00:00:00.000000", 6).unwrap();
    d = BinaryTime(Duration::nanoseconds(my_duration.Duration));
    assert_eq!(vec![0_u8], d);

    d = BinaryTime(Duration::ZERO);
    assert_eq!(vec![0_u8], d);

    d = BinaryTime(Duration::nanoseconds(-1));
    assert_eq!(vec![12, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], d);

    d = BinaryTime(Duration::nanoseconds(1) + Duration::microseconds(86_400_000));
    assert_eq!(vec![12, 0, 0, 0, 0, 0, 0, 1, 26, 128, 26, 6, 0], d);
}

// test_dump_length_encoded_int 对应 Go 的 TestDumpLengthEncodedInt，覆盖 MySQL 长度编码四个边界形态。
/// 对应 Go TestDumpLengthEncodedInt：1/3/4/9 字节四种长度编码形态。
#[test]
pub fn test_dump_length_encoded_int() {
    struct TestCase {
        num: u64,
        buffer: Vec<u8>,
    }
    let test_cases = vec![
        TestCase {
            num: 0,
            buffer: vec![0x00],
        },
        TestCase {
            num: 513,
            buffer: vec![b'\xfc', b'\x01', b'\x02'],
        },
        TestCase {
            num: 197121,
            buffer: vec![b'\xfd', b'\x01', b'\x02', b'\x03'],
        },
        TestCase {
            num: 578437695752307201,
            buffer: vec![
                b'\xfe', b'\x01', b'\x02', b'\x03', b'\x04', b'\x05', b'\x06', b'\x07', b'\x08',
            ],
        },
    ];
    for tc in test_cases {
        let b = LengthEncodedInt(vec![], tc.num);
        assert_eq!(tc.buffer, b);
    }
}

// test_dump_uint 对应 Go 的 TestDumpUint，使用闭包按小端顺序还原 uint64。
/// 对应 Go TestDumpUint：Uint64 写出后按小端手动还原，校验 0/1/MAX。
#[test]
pub fn test_dump_uint() {
    let test_cases = vec![0_u64, 1, u64::MAX];
    let parse_uint64 = |b: &[u8]| -> u64 {
        (b[0] as u64)
            | ((b[1] as u64) << 8)
            | ((b[2] as u64) << 16)
            | ((b[3] as u64) << 24)
            | ((b[4] as u64) << 32)
            | ((b[5] as u64) << 40)
            | ((b[6] as u64) << 48)
            | ((b[7] as u64) << 56)
    };
    for tc in test_cases {
        let b = Uint64(vec![], tc);
        assert_eq!(b.len(), 8);
        assert_eq!(tc, parse_uint64(&b));
    }
}
