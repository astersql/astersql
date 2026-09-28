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

// dump 包迁移对照单测（Aster 补充）。
//
// 相对 Go 边界做更细回归：长度编码整数全边界、字符串追加、小端 Uint、
// BinaryTime 符号/小数分支，以及 BinaryDateTime 各长度形态。

use dump::{
    BinaryDateTime, BinaryTime, LengthEncodedInt, LengthEncodedString, Uint16, Uint32, Uint64,
};
use time::Duration;

/// 用年月日时分秒微秒构造 types::Time，便于断言二进制时间包。
fn tidb_time(
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    microsecond: i32,
    time_type: u8,
) -> types::Time {
    types::NewTime(
        types::FromDate(year, month, day, hour, minute, second, microsecond),
        time_type,
        6,
    )
}

/// 长度编码整数在 250 / 0xffff / 0xffffff / u64::MAX 等边界与 Go 一致。
#[test]
fn length_encoded_int_matches_all_go_protocol_boundaries() {
    let cases = [
        (0, vec![0x00]),
        (250, vec![0xfa]),
        (251, vec![0xfc, 0xfb, 0x00]),
        (0xffff, vec![0xfc, 0xff, 0xff]),
        (0x1_0000, vec![0xfd, 0x00, 0x00, 0x01]),
        (0xff_ffff, vec![0xfd, 0xff, 0xff, 0xff]),
        (
            0x1_000000,
            vec![0xfe, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00],
        ),
        (
            u64::MAX,
            vec![0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ),
    ];

    for (value, expected) in cases {
        assert_eq!(LengthEncodedInt(Vec::new(), value), expected);
    }
}

/// LengthEncodedString 在已有缓冲区末尾追加长度前缀与内容。
#[test]
fn length_encoded_string_appends_to_the_existing_go_buffer() {
    assert_eq!(
        LengthEncodedString(vec![0xaa, 0xbb], b"TiDB"),
        vec![0xaa, 0xbb, 4, b'T', b'i', b'D', b'B']
    );
}

/// Uint16/32/64 均以小端追加到既有缓冲区。
#[test]
fn uint_helpers_append_little_endian_bytes() {
    assert_eq!(Uint16(vec![9], 0x0201), vec![9, 1, 2]);
    assert_eq!(Uint32(vec![9], 0x0403_0201), vec![9, 1, 2, 3, 4]);
    assert_eq!(
        Uint64(vec![9], 0x0807_0605_0403_0201),
        vec![9, 1, 2, 3, 4, 5, 6, 7, 8]
    );
}

/// BinaryTime：零值、负号、跨天进位、无微秒截断、带微秒完整形态。
#[test]
fn binary_time_matches_go_zero_sign_and_fraction_branches() {
    assert_eq!(BinaryTime(Duration::ZERO), vec![0]);
    assert_eq!(
        BinaryTime(Duration::nanoseconds(-1)),
        vec![12, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        BinaryTime(Duration::seconds(86_400) + Duration::nanoseconds(1)),
        vec![12, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        BinaryTime(Duration::hours(25) + Duration::minutes(2) + Duration::seconds(3)),
        vec![8, 0, 1, 0, 0, 0, 1, 2, 3]
    );
    assert_eq!(
        BinaryTime(Duration::nanoseconds(1) + Duration::microseconds(86_400_000)),
        vec![12, 0, 0, 0, 0, 0, 0, 1, 26, 128, 26, 6, 0]
    );
    // Go 的 `-time.Duration(math.MinInt64)` 按 int64 回绕后仍为负数。
    assert_eq!(
        BinaryTime(Duration::nanoseconds(i64::MIN)),
        vec![12, 1, 1, 0, 0, 0, 233, 209, 240, 9, 245, 242, 255]
    );
}

/// BinaryDateTime：各类型零值、DATE(4)、DATETIME(7)、TIMESTAMP 带微秒(11)。
#[test]
fn binary_datetime_matches_go_zero_date_time_and_microsecond_shapes() {
    for time_type in [
        types::mysql::TypeTimestamp,
        types::mysql::TypeDatetime,
        types::mysql::TypeDate,
    ] {
        assert_eq!(
            BinaryDateTime(vec![0xaa], tidb_time(0, 0, 0, 0, 0, 0, 0, time_type)),
            vec![0xaa, 0]
        );
    }

    assert_eq!(
        BinaryDateTime(
            Vec::new(),
            tidb_time(1992, 6, 1, 0, 0, 0, 0, types::mysql::TypeDate)
        ),
        vec![4, 200, 7, 6, 1]
    );
    assert_eq!(
        BinaryDateTime(
            Vec::new(),
            tidb_time(1993, 7, 13, 1, 1, 1, 0, types::mysql::TypeDatetime)
        ),
        vec![7, 201, 7, 7, 13, 1, 1, 1]
    );
    assert_eq!(
        BinaryDateTime(
            Vec::new(),
            tidb_time(1991, 5, 1, 1, 1, 1, 100_001, types::mysql::TypeTimestamp,)
        ),
        vec![11, 199, 7, 5, 1, 1, 1, 1, 161, 134, 1, 0]
    );
}
