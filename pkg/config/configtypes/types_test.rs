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

// configtypes 类型的单元测试：验证 `ByteSize`（人类可读容量，如 "1MiB"）与
// `Duration`（Go 风格时长，如 "1h2m3s"）在 JSON 与 TOML 两种配置格式下的
// 序列化/反序列化往返行为，对应 Go 侧的 TestByteSize 与 TestDuration。
// 文件后半部分是一组字段级编解码辅助函数，用来模拟 Go 标准 json 库与
// BurntSushi/toml 库对结构体单个带标签字段的处理方式。

use super::*;
#[allow(dead_code, non_snake_case)]
use anyhow::{Context, Result, anyhow};

// byteSizeConfig 对应 Go 测试中的临时结构体，覆盖 json/toml 标签里的 size 字段。
/// 容量配置的测试载体：仅含一个 `Size` 字段，模拟 Go 中带
/// `json:"size"` / `toml:"size"` 标签的匿名配置结构体。
pub struct byteSizeConfig {
    /// 以字节为单位的容量值（`ByteSize` 是 u64 的别名语义）。
    pub Size: ByteSize,
}

// durationConfig 对应 Go 测试中的临时结构体，覆盖 json/toml 标签里的 duration 字段。
/// 时长配置的测试载体：仅含一个 `Duration` 字段，模拟 Go 中带
/// `json:"duration"` / `toml:"duration"` 标签的匿名配置结构体。
pub struct durationConfig {
    /// 包装 `time.Duration`（纳秒计数）的时长值。
    pub Duration: Duration,
}

// test_byte_size 对应 Go 的 TestByteSize，按 json 与 toml 两个子用例检查容量字符串往返。
/// 验证 `ByteSize` 的 JSON/TOML 往返：
/// "1MiB" 解码为 1048576 字节并可编码回原文本；TOML 侧同理使用 "512KiB"。
#[test]
pub fn test_byte_size() {
    // Go 子用例 "json"：从字符串 "1MiB" 反序列化为字节数，再序列化回相同 JSON 文本。
    let mut cfg = byteSizeConfig { Size: 0 };
    let json_text = r#"{"size":"1MiB"}"#;
    cfg.Size = ByteSize_UnmarshalJSON_from_field(json_text, "size").expect("Go require.NoError");
    assert_eq!(
        1024 * 1024,
        cfg.Size,
        "Go require.Equal(ByteSize(1024*1024), cfg.Size)"
    );

    let data = ByteSize_MarshalJSON_field("size", cfg.Size).expect("Go require.NoError");
    assert_eq!(r#"{"size":"1MiB"}"#, data);

    // Go 子用例 "toml"：BurntSushi/toml 解码 size 字段并用 NewEncoder 写回带换行文本。
    let mut cfg = byteSizeConfig { Size: 0 };
    cfg.Size = ByteSize_UnmarshalTOML_from_field(r#"size = "512KiB""#, "size")
        .expect("Go require.NoError");
    assert_eq!(
        512 * 1024,
        cfg.Size,
        "Go require.Equal(ByteSize(512*1024), cfg.Size)"
    );

    let buf = ByteSize_MarshalTOML_field("size", cfg.Size).expect("Go require.NoError");
    assert_eq!("size = \"512KiB\"\n", buf);
}

// test_duration 对应 Go 的 TestDuration，保留 Duration 包装 time.Duration 后的文本编解码断言。
/// 验证 `Duration` 的 JSON/TOML 往返：
/// 1 小时 2 分 3 秒编码为 "1h2m3s" 并可解码还原；TOML 侧使用 "2m3s"。
#[test]
pub fn test_duration() {
    // Go 子用例 "json"：time.Hour + 2*time.Minute + 3*time.Second 会输出紧凑字符串 "1h2m3s"。
    // 与 Go time 包一致，时长内部以纳秒为最小单位存储。
    const SECOND: i64 = 1_000_000_000;
    const MINUTE: i64 = 60 * SECOND;
    const HOUR: i64 = 60 * MINUTE;
    let duration = Duration {
        Duration: HOUR + 2 * MINUTE + 3 * SECOND,
    };
    let cfg = durationConfig { Duration: duration };
    let data = Duration_MarshalJSON_field("duration", &cfg.Duration).expect("Go require.NoError");
    assert_eq!(r#"{"duration":"1h2m3s"}"#, data);

    let mut decoded = durationConfig {
        Duration: Duration::default(),
    };
    decoded.Duration =
        Duration_UnmarshalJSON_from_field(&data, "duration").expect("Go require.NoError");
    assert_eq!(duration.Duration, decoded.Duration.Duration);

    // Go 子用例 "toml"：解析 TOML duration 字段，并验证编码器写回同一 duration 文本。
    let mut cfg = durationConfig {
        Duration: Duration::default(),
    };
    cfg.Duration = Duration_UnmarshalTOML_from_field(r#"duration = "2m3s""#, "duration")
        .expect("Go require.NoError");
    assert_eq!(2 * MINUTE + 3 * SECOND, cfg.Duration.Duration);

    let buf = Duration_MarshalTOML_field("duration", &cfg.Duration).expect("Go require.NoError");
    assert_eq!("duration = \"2m3s\"\n", buf);
}

#[test]
fn byte_size_matches_docker_units_precision_and_grammar() {
    assert_eq!(ByteSize_MarshalText(1234).unwrap(), b"1.205KiB");

    let mut size = 7;
    ByteSize_UnmarshalText(&mut size, b"1.5 kb").unwrap();
    assert_eq!(size, 1536);
    ByteSize_UnmarshalText(&mut size, b"1KB").unwrap();
    assert_eq!(size, 1024, "RAMInBytes treats KB as a binary unit");
    assert!(ByteSize_UnmarshalText(&mut size, b"1XB").is_err());
    assert_eq!(size, 1024, "failed parsing must preserve the receiver");
}

#[test]
fn duration_rejects_units_and_spacing_not_accepted_by_go() {
    for input in [b"1day".as_slice(), b"1second", b"1h 2m"] {
        let mut duration = Duration { Duration: 42 };
        assert!(Duration_UnmarshalText(&mut duration, input).is_err());
        assert_eq!(duration.Duration, 0, "text failure resets the receiver");
    }

    let mut duration = Duration::default();
    Duration_UnmarshalText(&mut duration, b"1.5h2m3.25s").unwrap();
    assert_eq!(duration.Duration, 5_523_250_000_000);
    Duration_UnmarshalText(&mut duration, b".5s1.s").unwrap();
    assert_eq!(duration.Duration, 1_500_000_000);
}

// 以下辅助函数对应 Go json/toml 库在测试中的字段级编解码动作。
/// 从 JSON 文本中取出指定字段，并调用 `ByteSize_UnmarshalJSON` 解码为字节数。
pub fn ByteSize_UnmarshalJSON_from_field(text: &str, field: &str) -> Result<ByteSize> {
    // 先解析整段 JSON，再定位到目标字段，把该字段的原始 JSON 片段交给解码函数。
    let value: serde_json::Value =
        serde_json::from_str(text).context("decode byte-size JSON config")?;
    let value = value
        .get(field)
        .ok_or_else(|| anyhow!("missing JSON field {field:?}"))?;
    let mut size = 0;
    ByteSize_UnmarshalJSON(&mut size, serde_json::to_string(value)?.as_bytes())?;
    Ok(size)
}

/// 把字节数编码为 JSON 值后包进 `{field: ...}` 对象，模拟结构体整体序列化的输出。
pub fn ByteSize_MarshalJSON_field(field: &str, size: ByteSize) -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(&ByteSize_MarshalJSON(size)?)?;
    let mut object = serde_json::Map::new();
    object.insert(field.to_owned(), value);
    serde_json::to_string(&object).context("encode byte-size JSON config")
}

/// 从 TOML 文本中取出指定字符串字段，并调用 `ByteSize_UnmarshalText` 解码为字节数。
pub fn ByteSize_UnmarshalTOML_from_field(text: &str, field: &str) -> Result<ByteSize> {
    let value: toml::Value = toml::from_str(text).context("decode byte-size TOML config")?;
    // TOML 中容量以字符串形式存储（如 "512KiB"），因此要求字段必须是字符串类型。
    let text = value
        .get(field)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow!("missing TOML string field {field:?}"))?;
    let mut size = 0;
    ByteSize_UnmarshalText(&mut size, text.as_bytes())?;
    Ok(size)
}

/// 把字节数编码为文本后写入单字段 TOML 表，模拟 toml.NewEncoder 的输出（带换行）。
pub fn ByteSize_MarshalTOML_field(field: &str, size: ByteSize) -> Result<String> {
    let text = String::from_utf8(ByteSize_MarshalText(size)?)?;
    let mut table = toml::Table::new();
    table.insert(field.to_owned(), toml::Value::String(text));
    toml::to_string(&table).context("encode byte-size TOML config")
}

/// 从 JSON 文本中取出指定字段，并调用 `Duration_UnmarshalJSON` 解码为时长。
pub fn Duration_UnmarshalJSON_from_field(text: &str, field: &str) -> Result<Duration> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("decode duration JSON config")?;
    let value = value
        .get(field)
        .ok_or_else(|| anyhow!("missing JSON field {field:?}"))?;
    let mut duration = Duration::default();
    Duration_UnmarshalJSON(&mut duration, serde_json::to_string(value)?.as_bytes())?;
    Ok(duration)
}

/// 把时长编码为 JSON 值后包进 `{field: ...}` 对象，模拟结构体整体序列化的输出。
pub fn Duration_MarshalJSON_field(field: &str, duration: &Duration) -> Result<String> {
    let value: serde_json::Value = serde_json::from_slice(&Duration_MarshalJSON(duration)?)?;
    let mut object = serde_json::Map::new();
    object.insert(field.to_owned(), value);
    serde_json::to_string(&object).context("encode duration JSON config")
}

/// 从 TOML 文本中取出指定字符串字段，并调用 `Duration_UnmarshalText` 解码为时长。
pub fn Duration_UnmarshalTOML_from_field(text: &str, field: &str) -> Result<Duration> {
    let value: toml::Value = toml::from_str(text).context("decode duration TOML config")?;
    // TOML 中时长以字符串形式存储（如 "2m3s"），因此要求字段必须是字符串类型。
    let text = value
        .get(field)
        .and_then(toml::Value::as_str)
        .ok_or_else(|| anyhow!("missing TOML string field {field:?}"))?;
    let mut duration = Duration::default();
    Duration_UnmarshalText(&mut duration, text.as_bytes())?;
    Ok(duration)
}

/// 把时长编码为文本后写入单字段 TOML 表，模拟 toml.NewEncoder 的输出（带换行）。
pub fn Duration_MarshalTOML_field(field: &str, duration: &Duration) -> Result<String> {
    let text = String::from_utf8(Duration_MarshalText(*duration)?)?;
    let mut table = toml::Table::new();
    table.insert(field.to_owned(), toml::Value::String(text));
    toml::to_string(&table).context("encode duration TOML config")
}
