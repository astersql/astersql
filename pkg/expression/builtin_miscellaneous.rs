// Copyright 2021 PingCAP, Inc.
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

// Rust implementation of the scalar behavior in `builtin_miscellaneous.go`.
//
// The surrounding expression dispatcher is migrated by separate file-group tasks.  This
// module therefore exposes the production behavior as typed functions while preserving the
// Go implementation's NULL propagation, validation, advisory-lock mapping, UUID formats,
// network conversions, and Vitess DES hash.
//
// 杂项内建函数标量实现：INET*/IPv4/IPv6、UUID 族、ANY_VALUE/NAME_CONST、
// Vitess DES 哈希与 TIDB_SHARD、会话级咨询锁（advisory lock）、SLEEP，
// 以及尚未支持的 DEFAULT/UUID_SHORT/tidb_row_checksum 错误路径。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::thread;
use std::time::{Duration, Instant};

use cipher::{Block, BlockEncrypt, KeyInit};
use des::Des;
use uuid::{ContextV1, Timestamp, Uuid};

/// TIDB_SHARD 取模桶数（0..255）。
const TIDB_SHARD_BUCKET_COUNT: u64 = 256;
/// GET_LOCK 锁名最大字符数（按 Unicode 标量计）。
const ADVISORY_LOCK_NAME_LIMIT: usize = 64;
/// SLEEP 轮询 kill 信号的间隔，与 Go 一致为 10ms。
const SLEEP_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// UUID v1 时间戳上下文（序列计数器）。
static UUID_V1_CONTEXT: ContextV1 = ContextV1::new(0);

/// Errors which correspond to the error branches in the Go builtin signatures.
/// 对应 Go 杂项内建各错误分支的枚举。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MiscError {
    WrongValue {
        value_type: &'static str,
        value: String,
        function: &'static str,
    },
    IncorrectArguments(&'static str),
    UserLockWrongName(String),
    UserLockDeadlock,
    Lock(String),
    FunctionNotExists(&'static str),
    NotSupported(&'static str),
}

impl fmt::Display for MiscError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongValue {
                value_type,
                value,
                function,
            } => {
                write!(
                    formatter,
                    "incorrect {value_type} value {value:?} for function {function}"
                )
            }
            Self::IncorrectArguments(function) => {
                write!(formatter, "incorrect arguments to {function}")
            }
            Self::UserLockWrongName(name) => write!(formatter, "invalid user lock name {name:?}"),
            Self::UserLockDeadlock => formatter.write_str("user lock deadlock"),
            Self::Lock(message) => formatter.write_str(message),
            Self::FunctionNotExists(function) => {
                write!(formatter, "function {function} does not exist")
            }
            Self::NotSupported(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for MiscError {}

/// 构造“错误字符串参数”类 MiscError。
fn wrong_value(value: impl Into<String>, function: &'static str) -> MiscError {
    MiscError::WrongValue {
        value_type: "string",
        value: value.into(),
        function,
    }
}

/// Implements `INET_ATON`.  Short forms retain MySQL's historical zero-fill semantics.
/// 将点分 IPv4（含历史短格式）转为无符号 32 位整数；非法输入报错。
pub fn inet_aton(value: Option<&str>) -> Result<Option<u64>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_empty() || value.ends_with('.') {
        return Err(wrong_value(value, "inet_aton"));
    }

    let mut byte_result = 0_u64;
    let mut result = 0_u64;
    let mut dot_count = 0_u8;
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => {
                byte_result = byte_result * 10 + u64::from(byte - b'0');
                if byte_result > 255 {
                    return Err(wrong_value(value, "inet_aton"));
                }
            }
            b'.' => {
                dot_count += 1;
                if dot_count > 3 {
                    return Err(wrong_value(value, "inet_aton"));
                }
                result = (result << 8) + byte_result;
                byte_result = 0;
            }
            _ => return Err(wrong_value(value, "inet_aton")),
        }
    }

    // MySQL 短格式：缺省高位用零填充（如 "127" → 0.0.0.127）。
    match dot_count {
        1 => result <<= 16,
        2 => result <<= 8,
        _ => {}
    }
    Ok(Some((result << 8) + byte_result))
}

/// Implements `INET_NTOA` and returns NULL for values outside the IPv4 range.
/// 将整数转为点分 IPv4；超出 u32 范围返回 NULL。
pub fn inet_ntoa(value: Option<i64>) -> Option<String> {
    let value = value?;
    let value = u32::try_from(value).ok()?;
    Some(Ipv4Addr::from(value).to_string())
}

/// Implements `INET6_ATON`, returning four bytes for IPv4 and sixteen for IPv6.
/// 文本地址 → 网络字节序二进制。
pub fn inet6_aton(value: Option<&str>) -> Result<Option<Vec<u8>>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let address: IpAddr = value
        .parse()
        .map_err(|_| wrong_value(value, "inet_aton6"))?;
    Ok(Some(match address {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    }))
}

/// Implements `INET6_NTOA`; only four- and sixteen-byte inputs are valid.
/// IPv4 映射地址输出为 `::ffff:a.b.c.d` 形式。
pub fn inet6_ntoa(value: Option<&[u8]>) -> Option<String> {
    let value = value?;
    match value.len() {
        4 => Some(Ipv4Addr::new(value[0], value[1], value[2], value[3]).to_string()),
        16 => {
            let octets: [u8; 16] = value.try_into().ok()?;
            if octets[..10] == [0; 10] && octets[10..12] == [0xff, 0xff] {
                let ipv4 = Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
                Some(format!("::ffff:{ipv4}"))
            } else {
                Some(Ipv6Addr::from(octets).to_string())
            }
        }
        _ => None,
    }
}

/// Checks the exact `A.B.C.D` decimal format accepted by `IS_IPV4`.
/// 仅接受严格四段十进制点分格式。
pub fn is_ipv4(value: Option<&str>) -> Option<bool> {
    value.map(is_ipv4_text)
}

/// 无分配的 IPv4 文本校验内核。
fn is_ipv4_text(value: &str) -> bool {
    let mut dots = 0_u8;
    let mut accumulator = 0_u16;
    let mut previous_was_dot = true;
    for byte in value.bytes() {
        match byte {
            b'0'..=b'9' => {
                accumulator = accumulator.saturating_mul(10) + u16::from(byte - b'0');
                previous_was_dot = false;
            }
            b'.' => {
                dots += 1;
                if dots > 3 || accumulator > 255 || previous_was_dot {
                    return false;
                }
                accumulator = 0;
                previous_was_dot = true;
            }
            _ => return false,
        }
    }
    dots == 3 && accumulator <= 255 && !previous_was_dot
}

/// Checks a textual IPv6 value, including IPv4-mapped IPv6 text.
/// 能解析为 Ipv6 即视为真（含 IPv4-mapped）。
pub fn is_ipv6(value: Option<&str>) -> Option<bool> {
    value.map(|value| matches!(value.parse::<IpAddr>(), Ok(IpAddr::V6(_))))
}

/// 16 字节前 12 字节全 0 则为 IPv4 兼容地址。
pub fn is_ipv4_compat(value: Option<&[u8]>) -> Option<bool> {
    value.map(|value| value.len() == 16 && value[..12] == [0; 12])
}

/// 16 字节形如 `::ffff:x.x.x.x` 的映射地址判定。
pub fn is_ipv4_mapped(value: Option<&[u8]>) -> Option<bool> {
    value.map(|value| value.len() == 16 && value[..10] == [0; 10] && value[10..12] == [0xff, 0xff])
}

/// `ANY_VALUE` evaluates and returns its only argument without modification.
/// 聚合场景下抑制 ONLY_FULL_GROUP_BY，语义上透传。
pub fn any_value<T>(value: Option<T>) -> Option<T> {
    value
}

/// `NAME_CONST` returns its second argument; the first argument only supplies a name.
/// 第一个参数仅作结果列名，不参与求值。
pub fn name_const<N, T>(_name: N, value: Option<T>) -> Option<T> {
    value
}

/// 兼容 google/uuid 接受的多种文本形式（含 Go 测试保留的花括号截断行为）。
fn parse_google_uuid(value: &str) -> Result<Uuid, uuid::Error> {
    // google/uuid accepts the URN, simple, canonical and braced forms.  Its braced parser also
    // ignores the last byte (google/uuid#60), which TiDB's Go test intentionally preserves.
    let normalized = if value.len() == 38 && value.starts_with('{') {
        &value[1..37]
    } else if value.len() == 45 && value.starts_with("urn:uuid:") {
        &value[9..]
    } else {
        value
    };
    Uuid::parse_str(normalized)
}

/// Implements the strict whitespace behavior of MySQL `IS_UUID` while preserving Go formats.
/// 首尾空白直接判假；其余交由 google/uuid 解析。
pub fn is_uuid(value: Option<&str>) -> Option<bool> {
    value.map(|value| value.trim() == value && parse_google_uuid(value).is_ok())
}

/// Generates the UUID v1 value returned by TiDB's `UUID()` builtin.
/// 节点 ID 固定为零，与 TiDB 实现一致。
pub fn uuid_v1() -> String {
    let timestamp = Timestamp::now(&UUID_V1_CONTEXT);
    Uuid::new_v1(timestamp, &[0, 0, 0, 0, 0, 0]).to_string()
}

/// 生成 UUID v4（随机）。
pub fn uuid_v4() -> String {
    Uuid::new_v4().to_string()
}

/// 生成 UUID v7（时间有序）。
pub fn uuid_v7() -> String {
    Uuid::now_v7().to_string()
}

/// 返回 UUID 版本号；非法文本报错，NULL 输入返回 NULL。
pub fn uuid_version(value: Option<&str>) -> Result<Option<u8>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let uuid = parse_google_uuid(value).map_err(|_| wrong_value(value, "uuid_version"))?;
    Ok(Some(uuid.get_version_num() as u8))
}

/// Exact six-decimal timestamp representation returned by `UUID_TIMESTAMP`.
/// `UUID_TIMESTAMP` 返回的秒.微秒（六位）表示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UuidTimestamp {
    pub unix_seconds: u64,
    pub microseconds: u32,
}

impl UuidTimestamp {
    /// 格式化为 `秒.微秒` 字符串。
    pub fn decimal_string(&self) -> String {
        format!("{}.{:06}", self.unix_seconds, self.microseconds)
    }
}

/// 仅版本 1/6/7 可提取时间戳；其余版本返回 NULL。
pub fn uuid_timestamp(value: Option<&str>) -> Result<Option<UuidTimestamp>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let uuid = parse_google_uuid(value).map_err(|_| wrong_value(value, "uuid_timestamp"))?;
    if !matches!(uuid.get_version_num(), 1 | 6 | 7) {
        return Ok(None);
    }
    let Some(timestamp) = uuid.get_timestamp() else {
        return Ok(None);
    };
    let (unix_seconds, nanoseconds) = timestamp.to_unix();
    Ok(Some(UuidTimestamp {
        unix_seconds,
        microseconds: nanoseconds / 1_000,
    }))
}

/// UUID 文本 → 16 字节；`swap_flag!=0` 时交换时间字段字节序（MySQL 兼容）。
pub fn uuid_to_bin(
    value: Option<&str>,
    swap_flag: Option<i64>,
) -> Result<Option<[u8; 16]>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.trim() != value {
        return Err(wrong_value(value, "uuid_to_bin"));
    }
    let uuid = parse_google_uuid(value).map_err(|_| wrong_value(value, "uuid_to_bin"))?;
    let mut bytes = *uuid.as_bytes();
    if swap_flag.unwrap_or(0) != 0 {
        bytes = swap_binary_uuid(bytes);
    }
    Ok(Some(bytes))
}

/// 16 字节 → UUID 文本；可选先做字节序交换再格式化。
pub fn bin_to_uuid(
    value: Option<&[u8]>,
    swap_flag: Option<i64>,
) -> Result<Option<String>, MiscError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let bytes: [u8; 16] = value
        .try_into()
        .map_err(|_| wrong_value(format_binary(value), "bin_to_uuid"))?;
    let text = Uuid::from_bytes(bytes).to_string();
    if swap_flag.unwrap_or(0) != 0 {
        Ok(Some(swap_string_uuid(&text)))
    } else {
        Ok(Some(text))
    }
}

/// 错误消息中的十六进制展示。
fn format_binary(value: &[u8]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 交换 UUID 二进制中的时间低/中/高字段顺序。
pub fn swap_binary_uuid(bytes: [u8; 16]) -> [u8; 16] {
    let mut output = bytes;
    output[0..2].copy_from_slice(&bytes[6..8]);
    output[2..4].copy_from_slice(&bytes[4..6]);
    output[4..8].copy_from_slice(&bytes[0..4]);
    output
}

/// 在规范文本形式上交换时间字段片段。
pub fn swap_string_uuid(value: &str) -> String {
    debug_assert_eq!(value.len(), 36);
    format!(
        "{}{}{}{}{}{}",
        &value[9..13],
        &value[14..18],
        &value[8..9],
        &value[4..8],
        &value[13..14],
        &value[0..4],
    ) + &value[18..]
}

/// Vitess' null-key DES hash over an unsigned 64-bit big-endian block.
/// Vitess 空密钥 DES 哈希，用于分片键映射。
pub fn vitess_hash_u64(shard_key: u64) -> u64 {
    let cipher = Des::new_from_slice(&[0; 8]).expect("DES accepts an eight-byte key");
    let mut block = Block::<Des>::clone_from_slice(&shard_key.to_be_bytes());
    cipher.encrypt_block(&mut block);
    let bytes: [u8; 8] = block.into();
    u64::from_be_bytes(bytes)
}

/// 有符号分片键按位解释为 u64 再哈希。
pub fn vitess_hash(shard_key: i64) -> u64 {
    vitess_hash_u64(shard_key as u64)
}

/// TIDB_SHARD：Vitess 哈希对 256 取模。
pub fn tidb_shard(shard_key: i64) -> u8 {
    (vitess_hash(shard_key) % TIDB_SHARD_BUCKET_COUNT) as u8
}

/// 咨询锁底层返回的错误分类。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdvisoryLockError {
    Timeout,
    Deadlock,
    Other(String),
}

/// Session-level advisory-lock operations required by the miscellaneous builtins.
/// 会话级咨询锁（GET_LOCK/RELEASE_LOCK 等）所需的上下文接口。
pub trait AdvisoryLockContext {
    fn get_advisory_lock(&mut self, name: &str, timeout_secs: i64)
    -> Result<(), AdvisoryLockError>;
    fn is_used_advisory_lock(&self, name: &str) -> u64;
    fn release_advisory_lock(&mut self, name: &str) -> bool;
    fn release_all_advisory_locks(&mut self) -> u64;
}

/// 超时被钳制到上限时附带的告警信息。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockWarning {
    pub supplied_timeout: i64,
    pub effective_timeout: i64,
}

/// GET_LOCK 结果：1 成功 / 0 超时，以及可选超时告警。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockOutcome {
    pub value: i64,
    pub warning: Option<LockWarning>,
}

/// 校验锁名非空且长度合法，并规范化为小写。
fn normalize_lock_name(name: Option<&str>) -> Result<String, MiscError> {
    let Some(name) = name else {
        return Err(MiscError::UserLockWrongName("NULL".into()));
    };
    if name.is_empty() || name.chars().count() > ADVISORY_LOCK_NAME_LIMIT {
        return Err(MiscError::UserLockWrongName(name.into()));
    }
    Ok(name.to_lowercase())
}

/// GET_LOCK：钳制超时、映射死锁/超时错误，成功返回 1。
pub fn get_lock<C: AdvisoryLockContext + ?Sized>(
    context: &mut C,
    name: Option<&str>,
    timeout_secs: Option<i64>,
    max_timeout_secs: i64,
) -> Result<LockOutcome, MiscError> {
    let name = normalize_lock_name(name)?;
    let supplied_timeout = timeout_secs.unwrap_or(0);
    let (effective_timeout, warning) =
        if supplied_timeout < 0 || supplied_timeout > max_timeout_secs {
            (
                max_timeout_secs,
                Some(LockWarning {
                    supplied_timeout,
                    effective_timeout: max_timeout_secs,
                }),
            )
        } else {
            (supplied_timeout, None)
        };

    let value = match context.get_advisory_lock(&name, effective_timeout) {
        Ok(()) => 1,
        Err(AdvisoryLockError::Timeout) => 0,
        Err(AdvisoryLockError::Deadlock) => return Err(MiscError::UserLockDeadlock),
        Err(AdvisoryLockError::Other(message)) => return Err(MiscError::Lock(message)),
    };
    Ok(LockOutcome { value, warning })
}

/// RELEASE_LOCK：释放成功返回 1，锁不存在返回 0。
pub fn release_lock<C: AdvisoryLockContext + ?Sized>(
    context: &mut C,
    name: Option<&str>,
) -> Result<i64, MiscError> {
    let name = normalize_lock_name(name)?;
    Ok(i64::from(context.release_advisory_lock(&name)))
}

/// IS_FREE_LOCK：未被占用返回 1。
pub fn is_free_lock<C: AdvisoryLockContext + ?Sized>(
    context: &C,
    name: Option<&str>,
) -> Result<i64, MiscError> {
    let name = normalize_lock_name(name)?;
    Ok(i64::from(context.is_used_advisory_lock(&name) == 0))
}

/// IS_USED_LOCK：返回占用连接 ID，空闲则为 NULL。
pub fn is_used_lock<C: AdvisoryLockContext + ?Sized>(
    context: &C,
    name: Option<&str>,
) -> Result<Option<u64>, MiscError> {
    let name = normalize_lock_name(name)?;
    let connection_id = context.is_used_advisory_lock(&name);
    Ok((connection_id != 0).then_some(connection_id))
}

/// 释放当前会话持有的全部咨询锁，返回释放数量。
pub fn release_all_locks<C: AdvisoryLockContext + ?Sized>(context: &mut C) -> u64 {
    context.release_all_advisory_locks()
}

/// Implements `SLEEP`, polling the caller's kill signal every ten milliseconds like Go.
/// 正常睡完返回 0；被 kill 打断返回 1；非法秒数报错。
pub fn sleep_builtin(
    seconds: Option<f64>,
    mut should_kill: impl FnMut() -> bool,
) -> Result<i64, MiscError> {
    let Some(seconds) = seconds else {
        return Err(MiscError::IncorrectArguments("sleep"));
    };
    if seconds < 0.0 || !seconds.is_finite() {
        return Err(MiscError::IncorrectArguments("sleep"));
    }
    if seconds == 0.0 {
        return Ok(0);
    }
    if seconds > i64::MAX as f64 / 1_000_000_000_f64 {
        return Err(MiscError::IncorrectArguments("sleep"));
    }

    let duration = Duration::from_nanos((seconds * 1_000_000_000_f64) as u64);
    let started = Instant::now();
    // 分段休眠以便响应查询 kill，避免长时间不可中断 sleep。
    loop {
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Ok(0);
        }
        thread::sleep(remaining.min(SLEEP_POLL_INTERVAL));
        if should_kill() {
            return Ok(1);
        }
    }
}

/// DEFAULT() 在表达式上下文中不可用。
pub fn default_function() -> Result<(), MiscError> {
    Err(MiscError::FunctionNotExists("DEFAULT"))
}

/// UUID_SHORT 在当前 TiDB 路径标记为不存在。
pub fn uuid_short() -> Result<(), MiscError> {
    Err(MiscError::FunctionNotExists("UUID_SHORT"))
}

/// tidb_row_checksum 仅允许出现在特定点查计划的选择列表。
pub fn tidb_row_checksum() -> Result<(), MiscError> {
    Err(MiscError::NotSupported(
        "FUNCTION tidb_row_checksum can only be used as a select field in a fast point plan",
    ))
}
