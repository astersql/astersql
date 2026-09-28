// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// configtypes 迁移单元测试模块。
//
// 本文件验证从 Go(TiDB) 迁移到 Rust 的配置类型（`ByteSize` 字节大小与
// `Duration` 时间间隔）在 JSON / TOML(Text) 序列化与反序列化上的行为
// 与 Go 原实现保持一致，包括：
// - 人类可读单位（如 `1MiB`、`512KiB`、`1h2m3s`）的解析与格式化往返一致性；
// - Go 特有的小数、负数、零值时长表示形式的保留；
// - 解码失败时的接收者变化与 Go 实现保持一致。

// 引入父模块（configtypes）中被测的类型与函数，
// 如 ByteSize_UnmarshalJSON、Duration_MarshalText 等按 Go 命名风格迁移的自由函数。
use super::*;

/// 验证 ByteSize 的 JSON 与 TOML(Text) 编解码行为与 Go 版本一致。
///
/// ByteSize 用带二进制单位后缀（KiB/MiB 等，1KiB = 1024 字节）的字符串
/// 表示字节数，常用于配置文件中限制内存、缓存等大小。
#[test]
fn byte_size_matches_go_json_and_toml_behavior() {
    let mut size = 0;
    // JSON 形式带双引号："1MiB" -> 1048576 字节，且序列化能还原原字符串。
    ByteSize_UnmarshalJSON(&mut size, br#""1MiB""#).unwrap();
    assert_eq!(size, 1024 * 1024);
    assert_eq!(ByteSize_MarshalJSON(size).unwrap(), br#""1MiB""#);

    // Text 形式（用于 TOML）不带引号："512KiB" 同样满足往返一致。
    ByteSize_UnmarshalText(&mut size, b"512KiB").unwrap();
    assert_eq!(size, 512 * 1024);
    assert_eq!(ByteSize_MarshalText(size).unwrap(), b"512KiB");
}

/// 验证 Duration 的 JSON 与 TOML(Text) 编解码行为与 Go 版本一致。
///
/// Duration 对应 Go 的 time.Duration，内部以纳秒(i64)计数，
/// 文本形式使用 Go 风格的复合单位串（如 "1h2m3s"）。
#[test]
fn duration_matches_go_json_and_toml_behavior() {
    let mut duration = Duration::default();
    // "1h2m3s" = 3723 秒 = 3_723_000_000_000 纳秒，序列化后应还原原字符串。
    Duration_UnmarshalJSON(&mut duration, br#""1h2m3s""#).unwrap();
    assert_eq!(duration.Duration, 3_723_000_000_000);
    assert_eq!(Duration_MarshalJSON(&duration).unwrap(), br#""1h2m3s""#);

    // Text 形式（用于 TOML）："2m3s" = 123 秒，同样满足往返一致。
    Duration_UnmarshalText(&mut duration, b"2m3s").unwrap();
    assert_eq!(duration.Duration, 123_000_000_000);
    assert_eq!(Duration_MarshalText(duration).unwrap(), b"2m3s");
}

/// 验证 Duration 保留 Go 特有的小数、负数与零值文本形式。
///
/// Go 的 time.Duration 格式化规则有若干特殊情况：
/// 负数带前导负号、小数秒保留小数点、微秒使用 "µs"、
/// 零值输出 "0s"、整小时输出 "1h0m0s"（补全分秒）。
#[test]
fn duration_preserves_go_fractional_negative_and_zero_forms() {
    let mut duration = Duration::default();
    // 负的小数秒：-1.5s = -1_500_000_000 纳秒，格式化后仍为 "-1.5s"。
    Duration_UnmarshalText(&mut duration, b"-1.5s").unwrap();
    assert_eq!(duration.Duration, -1_500_000_000);
    assert_eq!(Duration_MarshalText(duration).unwrap(), b"-1.5s");

    // 微秒：输入可用 ASCII 的 "us"，但 Go 输出统一用 Unicode 的 "µs"。
    Duration_UnmarshalText(&mut duration, b"250us").unwrap();
    assert_eq!(duration.Duration, 250_000);
    assert_eq!(Duration_MarshalText(duration).unwrap(), "250µs".as_bytes());

    // 零值：输入裸 "0"（无单位）也能解析，输出规范化为 "0s"。
    Duration_UnmarshalText(&mut duration, b"0").unwrap();
    assert_eq!(duration.Duration, 0);
    assert_eq!(Duration_MarshalText(duration).unwrap(), b"0s");

    // 整小时：1 小时格式化为 "1h0m0s"，分和秒即使为 0 也会补全输出。
    duration.Duration = 3_600_000_000_000;
    assert_eq!(Duration_MarshalText(duration).unwrap(), b"1h0m0s");
}

/// 验证解码失败时的接收者变化与 Go 实现一致。
///
/// ByteSize 与 Duration JSON 路径都在解析成功后才赋值，因此失败时保留原值；
/// Go `Duration.UnmarshalText` 则直接将 `time.ParseDuration` 的返回值赋给接收者，
/// 因此解析失败时会将时长重置为零。
#[test]
fn decoding_errors_match_go_receiver_mutation() {
    let mut size = 42;
    // JSON 要求带引号，裸 "1MiB" 非法；非法输入后 size 仍应保持 42。
    assert!(ByteSize_UnmarshalJSON(&mut size, b"1MiB").is_err());
    assert_eq!(size, 42);
    assert!(ByteSize_UnmarshalText(&mut size, b"not-a-size").is_err());
    assert_eq!(size, 42);

    // JSON 方法使用临时值解析，失败时不改动内部纳秒计数。
    let mut duration = Duration { Duration: 42 };
    assert!(Duration_UnmarshalJSON(&mut duration, br#""not-a-duration""#).is_err());
    assert_eq!(duration.Duration, 42);

    // Go 的 Text 方法在检查错误前就赋值；ParseDuration 失败返回零值。
    assert!(Duration_UnmarshalText(&mut duration, b"not-a-duration").is_err());
    assert_eq!(duration.Duration, 0);
}
