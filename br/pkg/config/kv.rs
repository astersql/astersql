// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! TiKV config parsing helpers ported from `br/pkg/config/kv.go`.
//!
//! 从 TiKV 配置 JSON 响应中抽取 BR 关心的若干字段：导入线程数、
//! region 分裂尺寸/键数、日志备份开关。对照 Go `br/pkg/config/kv.go`；
//! 人类可读容量单位按 docker/go-units 的二进制（1024）倍率解析。

// ConfigTerm mirrors Go's generic ConfigTerm[T uint | uint64].
/// 带“是否被显式修改”标记的配置项；用于区分默认值与用户/集群覆盖。
#[derive(Clone, Debug, Default)]
pub struct ConfigTerm<T> {
    /// 实际取值（默认或覆盖后）。
    pub Value: T,
    /// 为 true 表示该字段已被外部显式设定，不应再被默认推断覆盖。
    pub Modified: bool,
}

/// BR 侧聚合的 TiKV 相关可调参数；字段含义与 Go `KVConfig` 对齐。
#[derive(Clone, Debug, Default)]
pub struct KVConfig {
    /// 导入路径并发（对应 TiKV `import.num-threads`）。
    pub ImportGoroutines: ConfigTerm<usize>,
    /// region 合并/分裂尺寸阈值（字节）。
    pub MergeRegionSize: ConfigTerm<u64>,
    /// region 合并/分裂键数阈值。
    pub MergeRegionKeyCount: ConfigTerm<u64>,
}

/// Parses `import.num-threads` from a TiKV config JSON payload.
/// 缺省或空对象时返回 0；非法 JSON 原样传播 `serde_json::Error`。
pub fn ParseImportThreadsFromConfig(resp: &[u8]) -> Result<usize, serde_json::Error> {
    #[derive(serde::Deserialize, Default)]
    struct Importer {
        #[serde(rename = "num-threads", default)]
        threads: usize,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "import", default)]
        import: Option<Importer>,
    }
    let config: Config = serde_json::from_slice(resp)?;
    Ok(config.import.unwrap_or_default().threads)
}

/// Parses `coprocessor.region-split-size` (human-readable RAM size) and
/// `coprocessor.region-split-keys` from a TiKV config JSON payload.
/// 尺寸字符串经 `units::RAMInBytes` 转为字节数；后缀非法或 JSON 失败均报错。
pub fn ParseMergeRegionSizeFromConfig(
    resp: &[u8],
) -> Result<(u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    #[derive(serde::Deserialize, Default)]
    struct Coprocessor {
        #[serde(rename = "region-split-size", default)]
        region_split_size: String,
        #[serde(rename = "region-split-keys", default)]
        region_split_keys: u64,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "coprocessor", default)]
        cop: Coprocessor,
    }
    let config: Config = serde_json::from_slice(resp)?;
    // Go 调用 units.RAMInBytes；成功后强转为 uint64。
    let ram = units::RAMInBytes(&config.cop.region_split_size)?;
    Ok((ram as u64, config.cop.region_split_keys))
}

/// Parses `log-backup.enable` from a TiKV config JSON payload.
/// 缺省为 false，与 Go 零值一致。
pub fn ParseLogBackupEnableFromConfig(resp: &[u8]) -> Result<bool, serde_json::Error> {
    #[derive(serde::Deserialize, Default)]
    struct LogBackup {
        #[serde(rename = "enable", default)]
        enable: bool,
    }
    #[derive(serde::Deserialize, Default)]
    struct Config {
        #[serde(rename = "log-backup", default)]
        log_backup: Option<LogBackup>,
    }
    let config: Config = serde_json::from_slice(resp)?;
    Ok(config.log_backup.unwrap_or_default().enable)
}

/// Port of docker/go-units `RAMInBytes`: parses sizes such as "96MiB",
/// "1.5GB" or "32" using binary (1024) multiples for all suffix styles.
pub mod units {
    /// 解析人类可读容量为字节；纯数字视为字节字面量。
    pub fn RAMInBytes(size: &str) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        parse_size(size)
    }

    /// Faithful port of go-units v0.5.0 `parseSize` with its whitespace,
    /// exponent, suffix and negative-value behavior.
    fn parse_size(size: &str) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
        let Some(separator) = size.rfind(|character: char| {
            character.is_ascii_digit() || character == '.' || character == ' '
        }) else {
            return Err(format!("invalid size: '{size}'").into());
        };

        let (number, suffix) = if size.as_bytes()[separator] == b' ' {
            (&size[..separator], &size[separator + 1..])
        } else {
            (&size[..separator + 1], &size[separator + 1..])
        };
        let mut value: f64 = number.parse()?;
        if !value.is_finite() || value < 0.0 {
            return Err(format!("invalid size: '{size}'").into());
        }
        if suffix.is_empty() {
            return Ok(value as i64);
        }

        let suffix = suffix.to_ascii_lowercase();
        if suffix.len() > 3 {
            return Err(format!("invalid suffix: '{suffix}'").into());
        }
        if suffix == "b" {
            return Ok(value as i64);
        }

        let multiplier = match suffix.as_bytes().first() {
            Some(b'k') => 1024_i64,
            Some(b'm') => 1024_i64.pow(2),
            Some(b'g') => 1024_i64.pow(3),
            Some(b't') => 1024_i64.pow(4),
            Some(b'p') => 1024_i64.pow(5),
            _ => return Err(format!("invalid suffix: '{suffix}'").into()),
        };
        let suffix_is_valid = match suffix.len() {
            1 => true,
            2 => suffix.as_bytes()[1] == b'b',
            3 => &suffix[1..] == "ib",
            _ => false,
        };
        if !suffix_is_valid {
            return Err(format!("invalid suffix: '{suffix}'").into());
        }
        value *= multiplier as f64;
        Ok(value as i64)
    }
}
