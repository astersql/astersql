// Copyright 2019 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Vectorized implementations of TiDB's miscellaneous builtins.
//
// The package-level expression traits are migrated in separate file groups.  This module keeps
// the complete row/null/error behavior in independently usable column functions so that those
// traits can delegate to it without duplicating the Go algorithms.

// 杂项内置函数的向量化实现（按列/按批求值，对应 Go `builtin_miscellaneous_vec.go`）。
// 覆盖网络地址转换、UUID 生成与校验、ANY_VALUE/NAME_CONST 透传、Vitess Hash、SLEEP 等；
// 保持与 Go 一致的行序、SQL NULL 传播及错误语义，供上层表达式签名直接委托调用。

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use des::{
    Des,
    cipher::{Block, BlockEncrypt, KeyInit},
};
use uuid::Uuid;

/// Length of a UUID in the canonical string format.
/// 标准连字符 UUID 字符串长度。
pub const UUID_STR_LEN: usize = 36;

/// All signatures whose Go `vectorized` method returns true in this file.
/// 本文件中 Go `vectorized` 返回 true 的全部签名名。
pub const VECTORIZED_SIGNATURES: [&str; 32] = [
    "InetNtoa",
    "IsIPv4",
    "JSONAnyValue",
    "RealAnyValue",
    "StringAnyValue",
    "IsIPv6",
    "IsUUID",
    "NameConstString",
    "DecimalAnyValue",
    "UUID",
    "UUIDv4",
    "UUIDv7",
    "UUIDVersion",
    "UUIDTimestamp",
    "NameConstDuration",
    "DurationAnyValue",
    "IntAnyValue",
    "IsIPv4Compat",
    "NameConstInt",
    "NameConstTime",
    "Sleep",
    "IsIPv4Mapped",
    "NameConstDecimal",
    "NameConstJSON",
    "Inet6Aton",
    "TimeAnyValue",
    "InetAton",
    "Inet6Ntoa",
    "NameConstReal",
    "VitessHash",
    "UUIDToBin",
    "BinToUUID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
/// 向量化杂项函数求值错误：参数非法、UUID/类型值错误、列长度不一致。
pub enum EvalError {
    IncorrectArguments(&'static str),
    WrongValueForType {
        function: &'static str,
        value: String,
    },
    MismatchedColumnLength {
        values: usize,
        flags: usize,
    },
}

impl fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncorrectArguments(function) => {
                write!(formatter, "incorrect arguments to {function}")
            }
            Self::WrongValueForType { function, value } => {
                write!(formatter, "wrong value {value:?} for {function}")
            }
            Self::MismatchedColumnLength { values, flags } => write!(
                formatter,
                "value and flag columns have different lengths: {values} and {flags}"
            ),
        }
    }
}

impl std::error::Error for EvalError {}

/// Go's ANY_VALUE vectorized implementations delegate directly to their first argument.
/// ANY_VALUE 向量化：直接透传第一参数列。
pub fn vec_any_value<T: Clone>(values: &[Option<T>]) -> Vec<Option<T>> {
    values.to_vec()
}

/// Go's NAME_CONST vectorized implementations delegate directly to their second argument.
/// NAME_CONST 向量化：直接透传第二参数列（值列）。
pub fn vec_name_const<T: Clone>(values: &[Option<T>]) -> Vec<Option<T>> {
    values.to_vec()
}

/// Hybrid integer fields preserve the argument's binary/string representation. Non-hybrid
/// fields use the base builtin's normal integer-to-string fallback.
/// Hybrid 整型字段保留参数的二进制/字符串表示；非 hybrid 走普通整数转字符串回退。
pub fn vec_int_any_value_string(
    hybrid: bool,
    argument_strings: &[Option<String>],
    default_strings: &[Option<String>],
) -> Vec<Option<String>> {
    if hybrid {
        argument_strings.to_vec()
    } else {
        default_strings.to_vec()
    }
}

/// INET_NTOA：将整数形式的 IPv4 地址转为点分十进制字符串；越界或 NULL 输出 NULL。
pub fn vec_inet_ntoa(values: &[Option<i64>]) -> Vec<Option<String>> {
    values
        .iter()
        .map(|value| match value {
            Some(value) if (0..=u32::MAX as i64).contains(value) => {
                Some(Ipv4Addr::from(*value as u32).to_string())
            }
            _ => None,
        })
        .collect()
}

/// 严格按点分十进制解析 IPv4（与 Go 一致，不接受 IPv6 映射写法）。
fn is_ipv4(value: &str) -> bool {
    let (mut dots, mut accumulator, mut previous_dot) = (0_u8, 0_u16, true);
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => {
                accumulator = accumulator
                    .saturating_mul(10)
                    .saturating_add(u16::from(byte - b'0'));
                previous_dot = false;
            }
            b'.' => {
                dots += 1;
                if dots > 3 || accumulator > 255 || previous_dot {
                    return false;
                }
                accumulator = 0;
                previous_dot = true;
            }
            _ => return false,
        }
    }
    dots == 3 && accumulator <= 255 && !previous_dot
}

/// IS_IPV4：列上逐行判定是否为合法 IPv4 文本；NULL 行保持 NULL。
pub fn vec_is_ipv4(values: &[Option<&str>]) -> Vec<Option<i64>> {
    values
        .iter()
        .map(|value| value.map(|value| i64::from(is_ipv4(value))))
        .collect()
}

/// IS_IPV6：可解析为 IPv6 且不是纯 IPv4 文本时为真。
pub fn vec_is_ipv6(values: &[Option<&str>]) -> Vec<Option<i64>> {
    values
        .iter()
        .map(|value| {
            value.map(|value| i64::from(value.parse::<Ipv6Addr>().is_ok() && !is_ipv4(value)))
        })
        .collect()
}

/// Match google/uuid's accepted Parse forms, including its historical 38-byte wrapper behavior.
/// 对齐 google/uuid 可解析形式，含历史 38 字节包装行为。
fn parse_google_uuid(value: &str) -> Result<Uuid, uuid::Error> {
    if !value.is_ascii() {
        return Uuid::parse_str(value);
    }
    let candidate = if value.len() == 45 && value[..9].eq_ignore_ascii_case("urn:uuid:") {
        &value[9..]
    } else if value.len() == 38 {
        // google/uuid intentionally ignores the first and last wrapper bytes. TiDB's Go tests
        // preserve that behavior even when the closing byte is not `}`.
        // google/uuid 故意忽略首尾包装字节；TiDB Go 测试在结尾不是 `}` 时仍保留该行为。
        &value[1..37]
    } else {
        value
    };
    Uuid::parse_str(candidate)
}

/// IS_UUID：接受 google/uuid 解析形式；首尾空白会使结果为假。
pub fn vec_is_uuid(values: &[Option<&str>]) -> Vec<Option<i64>> {
    values
        .iter()
        .map(|value| {
            value.map(|value| i64::from(value.trim() == value && parse_google_uuid(value).is_ok()))
        })
        .collect()
}

/// UUID v1 固定节点 ID，保证向量化生成路径可复现节点字段。
const V1_NODE_ID: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

/// 生成 `rows` 个 UUID v1 字符串（带连字符的标准形式）。
pub fn vec_uuid_v1(rows: usize) -> Result<Vec<String>, EvalError> {
    Ok((0..rows)
        .map(|_| Uuid::now_v1(&V1_NODE_ID).hyphenated().to_string())
        .collect())
}

/// 生成 `rows` 个随机 UUID v4 字符串。
pub fn vec_uuid_v4(rows: usize) -> Result<Vec<String>, EvalError> {
    Ok((0..rows)
        .map(|_| Uuid::new_v4().hyphenated().to_string())
        .collect())
}

/// 生成 `rows` 个基于时间的 UUID v7 字符串。
pub fn vec_uuid_v7(rows: usize) -> Result<Vec<String>, EvalError> {
    Ok((0..rows)
        .map(|_| Uuid::now_v7().hyphenated().to_string())
        .collect())
}

/// 构造 WrongValueForType 错误，对齐 Go 的类型取值报错文案。
fn wrong_value(function: &'static str, value: impl Into<String>) -> EvalError {
    EvalError::WrongValueForType {
        function,
        value: value.into(),
    }
}

/// UUID_VERSION：解析 UUID 并返回版本号；非法串报错。
pub fn vec_uuid_version(values: &[Option<&str>]) -> Result<Vec<Option<i64>>, EvalError> {
    values
        .iter()
        .map(|value| {
            value
                .map(|value| {
                    parse_google_uuid(value)
                        .map(|uuid| uuid.get_version_num() as i64)
                        .map_err(|_| wrong_value("uuid_version", value))
                })
                .transpose()
        })
        .collect()
}

/// DECIMAL(20, 6) seconds represented exactly as an integer number of microseconds.
/// 以整数微秒精确表示 DECIMAL(20,6) 秒。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimalSeconds(i64);

/// 微秒与 DECIMAL(20,6) 秒表示之间的转换。
impl DecimalSeconds {
    pub const fn from_micros(micros: i64) -> Self {
        Self(micros)
    }

    pub const fn micros(self) -> i64 {
        self.0
    }
}

impl fmt::Display for DecimalSeconds {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = i128::from(self.0).abs();
        write!(
            formatter,
            "{sign}{}.{:06}",
            magnitude / 1_000_000,
            magnitude % 1_000_000
        )
    }
}

/// UUID Gregorian 纪元到 Unix 纪元的 100ns 刻度差。
const UUID_EPOCH_TO_UNIX_100NS: i128 = 122_192_928_000_000_000;

/// 从 UUID v1/v6/v7 提取时间戳微秒；其他版本返回 None。
fn uuid_timestamp_micros(uuid: Uuid) -> Option<i64> {
    let bytes = uuid.as_bytes();
    let ticks_or_millis = match uuid.get_version_num() {
        1 => {
            let low = u32::from_be_bytes(bytes[0..4].try_into().expect("four bytes")) as u64;
            let mid = u16::from_be_bytes(bytes[4..6].try_into().expect("two bytes")) as u64;
            let high =
                (u16::from_be_bytes(bytes[6..8].try_into().expect("two bytes")) & 0x0fff) as u64;
            let ticks = (high << 48) | (mid << 32) | low;
            i128::from(ticks) - UUID_EPOCH_TO_UNIX_100NS
        }
        6 => {
            let high = u32::from_be_bytes(bytes[0..4].try_into().expect("four bytes")) as u64;
            let mid = u16::from_be_bytes(bytes[4..6].try_into().expect("two bytes")) as u64;
            let low =
                (u16::from_be_bytes(bytes[6..8].try_into().expect("two bytes")) & 0x0fff) as u64;
            let ticks = (high << 28) | (mid << 12) | low;
            i128::from(ticks) - UUID_EPOCH_TO_UNIX_100NS
        }
        7 => {
            let millis = bytes[..6]
                .iter()
                .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
            return i64::try_from(millis.checked_mul(1_000)?).ok();
        }
        _ => return None,
    };
    i64::try_from(ticks_or_millis / 10).ok()
}

/// UUID_TIMESTAMP：将可解析时间戳的 UUID 转为 DECIMAL 秒；无时间戳版本为 NULL。
pub fn vec_uuid_timestamp(
    values: &[Option<&str>],
) -> Result<Vec<Option<DecimalSeconds>>, EvalError> {
    values
        .iter()
        .map(|value| {
            value
                .map(|value| {
                    parse_google_uuid(value)
                        .map(|uuid| uuid_timestamp_micros(uuid).map(DecimalSeconds::from_micros))
                        .map_err(|_| wrong_value("uuid_timestamp", value))
                })
                .transpose()
                .map(Option::flatten)
        })
        .collect()
}

/// IS_IPV4_COMPAT：16 字节地址是否以前导 12 个零表示 IPv4 兼容。
pub fn vec_is_ipv4_compat(values: &[Option<&[u8]>]) -> Vec<Option<i64>> {
    const PREFIX: [u8; 12] = [0; 12];
    values
        .iter()
        .map(|value| value.map(|value| i64::from(value.len() == 16 && value.starts_with(&PREFIX))))
        .collect()
}

/// IS_IPV4_MAPPED：16 字节地址是否为 `::ffff:x.x.x.x` 映射前缀。
pub fn vec_is_ipv4_mapped(values: &[Option<&[u8]>]) -> Vec<Option<i64>> {
    const PREFIX: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff];
    values
        .iter()
        .map(|value| value.map(|value| i64::from(value.len() == 16 && value.starts_with(&PREFIX))))
        .collect()
}

/// INET6_ATON：文本 IP 转为二进制；IPv4 为 4 字节，IPv6 为 16 字节。
pub fn vec_inet6_aton(values: &[Option<&str>]) -> Vec<Option<Vec<u8>>> {
    values
        .iter()
        .map(|value| {
            value.and_then(|value| {
                if value.is_empty() {
                    return None;
                }
                match value.parse::<IpAddr>().ok()? {
                    IpAddr::V4(ipv4) => Some(ipv4.octets().to_vec()),
                    IpAddr::V6(ipv6) => Some(ipv6.octets().to_vec()),
                }
            })
        })
        .collect()
}

/// INET_ATON：点分 IPv4（含省略段写法）转为无符号整数；非法输入为 NULL。
pub fn vec_inet_aton(values: &[Option<&str>]) -> Vec<Option<u64>> {
    values
        .iter()
        .map(|value| {
            value.and_then(|ip| {
                if ip.is_empty() || ip.ends_with('.') {
                    return None;
                }
                let (mut byte_result, mut result, mut dot_count) = (0_u64, 0_u64, 0_u8);
                for byte in ip.bytes() {
                    match byte {
                        b'0'..=b'9' => {
                            byte_result = byte_result * 10 + u64::from(byte - b'0');
                            if byte_result > 255 {
                                return None;
                            }
                        }
                        b'.' => {
                            dot_count += 1;
                            if dot_count > 3 {
                                return None;
                            }
                            result = (result << 8) + byte_result;
                            byte_result = 0;
                        }
                        _ => return None,
                    }
                }
                if dot_count == 1 {
                    result <<= 16;
                } else if dot_count == 2 {
                    result <<= 8;
                }
                Some((result << 8) + byte_result)
            })
        })
        .collect()
}

/// INET6_NTOA：二进制地址转文本；映射地址输出 `::ffff:` 形式。
pub fn vec_inet6_ntoa(values: &[Option<&[u8]>]) -> Vec<Option<String>> {
    const MAPPED_PREFIX: [u8; 12] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff];
    values
        .iter()
        .map(|value| {
            value.and_then(|value| match value.len() {
                4 => Some(Ipv4Addr::new(value[0], value[1], value[2], value[3]).to_string()),
                16 if value.starts_with(&MAPPED_PREFIX) => Some(format!(
                    "::ffff:{}",
                    Ipv4Addr::new(value[12], value[13], value[14], value[15])
                )),
                16 => Some(Ipv6Addr::from(<[u8; 16]>::try_from(value).ok()?).to_string()),
                _ => None,
            })
        })
        .collect()
}

/// Vitess Hash 使用的全零 DES 密钥（与 Go 一致）。
static NULL_KEY_DES: LazyLock<Des> =
    LazyLock::new(|| Des::new_from_slice(&[0_u8; 8]).expect("an eight-byte DES key is valid"));

/// 对 u64 做 DES 加密块变换，得到 Vitess 风格哈希。
fn vitess_hash(value: u64) -> u64 {
    let mut block = Block::<Des>::default();
    block.copy_from_slice(&value.to_be_bytes());
    NULL_KEY_DES.encrypt_block(&mut block);
    u64::from_be_bytes(block.into())
}

/// VITESS_HASH：列上逐行哈希；NULL 传播。
pub fn vec_vitess_hash(values: &[Option<u64>]) -> Vec<Option<u64>> {
    values.iter().map(|value| value.map(vitess_hash)).collect()
}

/// 校验可选 swap flag 列与值列行数一致。
fn validate_flags_len<T>(values: &[T], flags: Option<&[Option<i64>]>) -> Result<(), EvalError> {
    if let Some(flags) = flags
        && values.len() != flags.len()
    {
        return Err(EvalError::MismatchedColumnLength {
            values: values.len(),
            flags: flags.len(),
        });
    }
    Ok(())
}

/// UUID 二进制时间戳字段重排（MySQL swap flag=1）。
fn swap_binary_uuid(bytes: &[u8; 16]) -> Vec<u8> {
    let mut swapped = [0_u8; 16];
    swapped[0..2].copy_from_slice(&bytes[6..8]);
    swapped[2..4].copy_from_slice(&bytes[4..6]);
    swapped[4..8].copy_from_slice(&bytes[0..4]);
    swapped[8..].copy_from_slice(&bytes[8..]);
    swapped.to_vec()
}

/// 标准 UUID 字符串上对应的时间字段交换。
fn swap_string_uuid(value: &str) -> String {
    let bytes = value.as_bytes();
    debug_assert_eq!(bytes.len(), UUID_STR_LEN);
    let mut swapped = [0_u8; UUID_STR_LEN];
    swapped[0..4].copy_from_slice(&bytes[9..13]);
    swapped[4..8].copy_from_slice(&bytes[14..18]);
    swapped[8..9].copy_from_slice(&bytes[8..9]);
    swapped[9..13].copy_from_slice(&bytes[4..8]);
    swapped[13..14].copy_from_slice(&bytes[13..14]);
    swapped[14..18].copy_from_slice(&bytes[0..4]);
    swapped[18..].copy_from_slice(&bytes[18..]);
    String::from_utf8(swapped.to_vec()).expect("canonical UUIDs are ASCII")
}

/// UUID_TO_BIN：UUID 文本转 16 字节；可选 swap 重排时间字段。
pub fn vec_uuid_to_bin(
    values: &[Option<&str>],
    flags: Option<&[Option<i64>]>,
) -> Result<Vec<Option<Vec<u8>>>, EvalError> {
    validate_flags_len(values, flags)?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .map(|value| {
                    if value.trim() != value {
                        return Err(wrong_value("uuid_to_bin", value));
                    }
                    let uuid =
                        parse_google_uuid(value).map_err(|_| wrong_value("uuid_to_bin", value))?;
                    let should_swap = flags
                        .and_then(|flags| flags[index])
                        .is_some_and(|flag| flag != 0);
                    Ok(if should_swap {
                        swap_binary_uuid(uuid.as_bytes())
                    } else {
                        uuid.as_bytes().to_vec()
                    })
                })
                .transpose()
        })
        .collect()
}

/// BIN_TO_UUID：16 字节转连字符 UUID 字符串；可选 swap。
pub fn vec_bin_to_uuid(
    values: &[Option<&[u8]>],
    flags: Option<&[Option<i64>]>,
) -> Result<Vec<Option<String>>, EvalError> {
    validate_flags_len(values, flags)?;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .map(|value| {
                    let uuid = Uuid::from_slice(value)
                        .map_err(|_| wrong_value("bin_to_uuid", String::from_utf8_lossy(value)))?;
                    let canonical = uuid.hyphenated().to_string();
                    let should_swap = flags
                        .and_then(|flags| flags[index])
                        .is_some_and(|flag| flag != 0);
                    Ok(if should_swap {
                        swap_string_uuid(&canonical)
                    } else {
                        canonical
                    })
                })
                .transpose()
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// SLEEP 非法参数处理：严格报错或累计 warning。
pub enum InvalidArgumentMode {
    Error,
    Warning,
}

/// Minimal session state used by SLEEP. It preserves SQLKiller polling and the Go reset rule.
/// SLEEP 所需最小会话状态：保留 SQLKiller 轮询与 Go 的复位规则。
#[derive(Debug, Default)]
pub struct SleepSession {
    killed: AtomicBool,
    has_table_ids: bool,
    in_insert_statement: bool,
    in_update_statement: bool,
    in_delete_statement: bool,
}

/// 构造/查询 kill 状态，并在纯 SELECT 场景复位 SQLKiller。
impl SleepSession {
    pub const fn with_statement_side_effects(
        has_table_ids: bool,
        in_insert_statement: bool,
        in_update_statement: bool,
        in_delete_statement: bool,
    ) -> Self {
        Self {
            killed: AtomicBool::new(false),
            has_table_ids,
            in_insert_statement,
            in_update_statement,
            in_delete_statement,
        }
    }

    pub fn send_kill_signal(&self) {
        self.killed.store(true, Ordering::Release);
    }

    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::Acquire)
    }

    fn reset_if_plain_select(&self) {
        if !self.has_table_ids
            && !self.in_insert_statement
            && !self.in_update_statement
            && !self.in_delete_statement
        {
            self.killed.store(false, Ordering::Release);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// SLEEP 向量化结果：每行返回值（0/1）及 warning 计数。
pub struct SleepOutcome {
    pub values: Vec<i64>,
    pub warnings: usize,
}

/// 阻塞休眠；被 kill 时按 Go 规则可能复位 killer 并返回 true（表示中断）。
fn do_sleep(seconds: f64, session: &SleepSession) -> bool {
    if seconds <= 0.0 || seconds.is_nan() {
        return false;
    }

    let nanoseconds = seconds * 1_000_000_000.0;
    // Go converts an out-of-range float64 to time.Duration (int64) as MinInt64 on the
    // supported architectures. time.NewTimer therefore fires immediately instead of
    // sleeping for the maximum representable duration.
    if nanoseconds >= i64::MAX as f64 {
        return false;
    }
    let duration = Duration::from_nanos(nanoseconds as u64);
    let started = Instant::now();
    loop {
        let elapsed = started.elapsed();
        if elapsed >= duration {
            return false;
        }
        thread::sleep(Duration::from_millis(10).min(duration - elapsed));
        if session.is_killed() {
            session.reset_if_plain_select();
            return true;
        }
    }
}

/// SLEEP：按行休眠；非法参数依模式报错或告警；kill 后后续行填 1。
pub fn vec_sleep(
    values: &[Option<f64>],
    session: &SleepSession,
    invalid_argument_mode: InvalidArgumentMode,
) -> Result<SleepOutcome, EvalError> {
    let mut result = SleepOutcome {
        values: vec![0; values.len()],
        warnings: 0,
    };
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else {
            match invalid_argument_mode {
                InvalidArgumentMode::Error => {
                    return Err(EvalError::IncorrectArguments("sleep"));
                }
                InvalidArgumentMode::Warning => result.warnings += 1,
            }
            continue;
        };
        if *value < 0.0 {
            match invalid_argument_mode {
                InvalidArgumentMode::Error => {
                    return Err(EvalError::IncorrectArguments("sleep"));
                }
                InvalidArgumentMode::Warning => result.warnings += 1,
            }
            continue;
        }
        if *value > f64::MAX / 1_000_000_000.0 {
            return Err(EvalError::IncorrectArguments("sleep"));
        }
        if do_sleep(*value, session) {
            result.values[index..].fill(1);
            return Ok(result);
        }
    }
    Ok(result)
}
