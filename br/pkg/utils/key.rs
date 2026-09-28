// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Key helpers ported from `br/pkg/utils/key.go`.
//!
//! 提供 CLI/元数据用的键解析、区间比较与相交、时间格式化及 meta 前缀判定。
//! 空 end key 在备份区间中表示 +∞，比较时需显式选择是否按无穷处理。
//! 编解码细节对齐 Go：escaped/hex、CompareBytesExt 与 IntersectAll 双指针裁剪。
//! CLI 用户键输入经 ParseKey 统一入口，避免各处私自解码。
//! IntersectAll 假设两侧区间列表已按键序可推进（与 Go 相同前提）。
//! FormatDate 不依赖系统 TZ 数据库，仅用数值偏移生成墙钟。
//! meta 前缀函数用于过滤备份范围中的系统键。
//! hex_* 同时服务 JSON 元数据与 CLI hex 格式。
//! EncodeTxnMetaKey 组合三层编码，顺序不可颠倒。
//! IsMetaAutoIDKey 解码失败时返回 false 而非错误，便于过滤器热路径。
//! CompareBytesExt 的空键开关必须由调用方按 start/end 语义分别指定。
//! clamp 失败原因用于双指针推进，不只是日志。
//! 八进制转义最多三位，与 Go %03o 扫描宽度一致。
//! 本文件无全局可变状态，函数均可重入。
//! 与 Go key.go 符号名保持可检索对应。
//! DateFormat 常量仅文档用途，真正输出由 FormatDate 拼装。
//! 错误路径使用 SharedError，便于上层 Annotate。
//! 区间相交结果不保证输入顺序，调用方常需排序后再比。
//! 空 KeyRange 表示无交集，而非全键空间。
//! unix_secs_to_utc 从 1970 起算，覆盖闰年规则。
//! hex 非法字符与奇数长度均拒绝，避免静默截断。
//! 备份半开区间 [start, end) 中 end 空表示正无穷。
//! LeftNotOverlapped/RightNotOverlapped 驱动指针，Success 产出片段。
//! BuggyUnknown 仅告警，不 panic，以免中断大批次相交。
//! unescaped_key 流式读取，适配长键输入。
//! IoEof 显示为 EOF，贴近 Go 错误字符串。
//! 修改比较语义时必须同步 key_test 表驱动用例。
//! 不在此处理 TiKV 编码前缀（如 table prefix），由上层负责。
//! FormatDate 的 offset 可为负，符号位单独渲染。
//! nanos 去尾零后若为空串不会发生（nanos==0 已分支）。
//! IsDBOrDDLJobHistoryKey 宽前缀可能误匹配，调用方需再细分。
//! ParseKey raw 不做 UTF-8 校验，按字节原样返回。
//! hex_encode 输出长度恒为输入两倍。
//! 大端/小端与 EncodeUintDesc 约定一致，勿本地改写。
//! 本模块注释解释约束与 Go 对齐点，不复述字面语法。
//! 任务范围仅注释，保持可执行行为不变。

use std::cmp::Ordering;
use std::io::{self, Read};

use crate::stubs::{DecodeMetaKey, EncodeMetaKey, KeyRange, TableKey};
use astersql_br_pkg_errors::ErrInvalidArgument;
use astersql_br_pkg_logutil::{StringifyKeys, StringifyRange, log};
use astersql_errors::{Annotate, SharedError, Trace};
use astersql_meta::{
    is_auto_increment_id_key, is_auto_random_table_id_key, is_auto_table_id_key, is_sequence_key,
};
use astersql_util_codec::{DecodeBytes, EncodeBytes, EncodeUintDesc};

/// Go `time.Format` 布局字符串的文档常量；实际格式化见 `FormatDate`。
pub const DateFormat: &str = "%Y-%m-%d %H:%M:%S%.9f %z";

/// 按 format 解析用户输入的键：raw / escaped / hex。
pub fn ParseKey(format: &str, key: &str) -> Result<Vec<u8>, SharedError> {
    match format {
        "raw" => Ok(key.as_bytes().to_vec()),
        "escaped" => unescaped_key(key),
        "hex" => hex_decode_string(key),
        // 未知格式包装为 ErrInvalidArgument，文案保持 unknown format。
        _ => Err(Annotate(
            Some(SharedError::new((*ErrInvalidArgument).clone())),
            "unknown format",
        )
        .expect("annotate unknown format")),
    }
}

/// 解析 Go `strconv`/`fmt` 风格转义：`\a\b\f\n\r\t\v\\'"`、`\xHH`、八进制。
fn unescaped_key(text: &str) -> Result<Vec<u8>, SharedError> {
    let mut buf = Vec::new();
    let mut reader = text.as_bytes();
    loop {
        let mut c = [0u8; 1];
        match reader.read_exact(&mut c) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(SharedError::new(err)),
        }
        if c[0] != b'\\' {
            buf.push(c[0]);
            continue;
        }
        let mut n = [0u8; 1];
        // 孤立反斜杠视为 EOF，与 Go 扫描失败路径一致。
        if reader.read_exact(&mut n).is_err() {
            return Err(SharedError::new(IoEof));
        }
        // Go: strings.IndexByte(`abfnrtv\'"`, ...) + []byte("\a\b\f\n\r\t\v\\'\"")
        let escape_chars: &[u8] = b"abfnrtv\\'\"";
        let unescaped: &[u8] = b"\x07\x08\x0c\n\r\t\x0b\\'\"";
        if let Some(idx) = escape_chars.iter().position(|&b| b == n[0]) {
            buf.push(unescaped[idx]);
            continue;
        }
        match n[0] {
            b'x' => {
                // Go: fmt.Sscanf(string(r.Next(2)), "%02x", &c). The scan error is
                // deliberately ignored, so no digit leaves `c` as the backslash read above.
                let take = reader.len().min(2);
                let hex = &reader[..take];
                reader = &reader[take..];
                let value = scan_u8_prefix(hex, 16).unwrap_or(b'\\');
                buf.push(value);
            }
            _ => {
                // Go: n = append(n, r.Next(2)...); Sscanf("%03o")
                // 首字符已是八进制数字，再最多补 2 位组成最多 3 位八进制。
                let take = reader.len().min(2);
                let rest = &reader[..take];
                reader = &reader[take..];
                let mut octal = [0u8; 3];
                octal[0] = n[0];
                octal[1..1 + rest.len()].copy_from_slice(rest);
                let value = scan_u8_prefix(&octal[..1 + rest.len()], 8).ok_or_else(|| {
                    SharedError::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "invalid syntax",
                    ))
                })?;
                buf.push(value);
            }
        }
    }
    Ok(buf)
}

/// Match `fmt.Sscanf` integer scanning: consume the field, but parse its valid prefix.
fn scan_u8_prefix(bytes: &[u8], radix: u32) -> Option<u8> {
    let digit_count = bytes
        .iter()
        .take_while(|byte| char::from(**byte).is_digit(radix))
        .count();
    if digit_count == 0 {
        return None;
    }
    let text = std::str::from_utf8(&bytes[..digit_count]).ok()?;
    u8::from_str_radix(text, radix).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IoEof;

impl std::fmt::Display for IoEof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EOF")
    }
}

impl std::error::Error for IoEof {}

/// 比较 end key：空切片视为 +∞（备份半开区间上界）。
pub fn CompareEndKey(a: &[u8], b: &[u8]) -> i32 {
    if a.is_empty() {
        return if b.is_empty() { 0 } else { 1 };
    }
    if b.is_empty() {
        return -1;
    }
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 可配置空键是否按 +∞；关闭时退化为普通 `bytes.Compare`。
pub fn CompareBytesExt(a: &[u8], a_empty_as_inf: bool, b: &[u8], b_empty_as_inf: bool) -> i32 {
    // Mirror Go `CompareBytesExt`: only treat empty as +inf when the flag says so;
    // otherwise fall through to plain `bytes.Compare` (NOT `CompareEndKey`).
    if a.is_empty() && a_empty_as_inf && b.is_empty() && b_empty_as_inf {
        return 0;
    }
    if a.is_empty() && a_empty_as_inf {
        return 1;
    }
    if b.is_empty() && b_empty_as_inf {
        return -1;
    }
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 单次 clamp 结果：成功或左右无重叠；BuggyUnknown 为不应到达路径。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailedToClampReason {
    SuccessClamp,
    LeftNotOverlapped,
    RightNotOverlapped,
    BuggyUnknown,
}

/// 将 `rng` 裁进 `clamp_in`；无交集时返回空区间并带上失败原因。
fn clamp_in_one_range(rng: KeyRange, clamp_in: KeyRange) -> (KeyRange, FailedToClampReason) {
    let mut rng = rng;
    let mut possible_failure = FailedToClampReason::BuggyUnknown;
    // StartKey 只按普通比较抬升，不把空 start 当 +∞。
    if CompareBytesExt(
        rng.StartKey.as_ref(),
        false,
        clamp_in.StartKey.as_ref(),
        false,
    ) < 0
    {
        rng.StartKey = clamp_in.StartKey.clone();
        possible_failure = FailedToClampReason::LeftNotOverlapped;
    }
    // EndKey 空值按 +∞，对齐备份半开区间上界语义。
    if CompareBytesExt(rng.EndKey.as_ref(), true, clamp_in.EndKey.as_ref(), true) > 0 {
        rng.EndKey = clamp_in.EndKey.clone();
        possible_failure = FailedToClampReason::RightNotOverlapped;
    }
    // 裁剪后 start>=end（含空 end 规则）则无有效交集。
    if CompareBytesExt(rng.StartKey.as_ref(), false, rng.EndKey.as_ref(), true) >= 0 {
        return (KeyRange::default(), possible_failure);
    }
    (rng, FailedToClampReason::SuccessClamp)
}

/// 两路有序区间列表相交（双指针）；对称调用结果排序后应对等。
pub fn IntersectAll(mut s1: Vec<KeyRange>, s2: Vec<KeyRange>) -> Vec<KeyRange> {
    let mut current_clamping = 0usize;
    let mut current_clamp_target = 0usize;
    let mut rs = Vec::with_capacity(s1.len());
    while current_clamp_target < s2.len() && current_clamping < s1.len() {
        let cin = s2[current_clamp_target].clone();
        let crg = s1[current_clamping].clone();
        let crg_end = crg.EndKey.clone();
        let crg_log = crg.clone();
        let (rng, result) = clamp_in_one_range(crg, cin.clone());
        match result {
            FailedToClampReason::SuccessClamp => {
                rs.push(rng);
                // 当前 s1 段未越过 cin 上界则推进 s1，否则抬高 StartKey 继续裁。
                if CompareBytesExt(crg_end.as_ref(), true, cin.EndKey.as_ref(), true) <= 0 {
                    current_clamping += 1;
                } else {
                    s1[current_clamping].StartKey = cin.EndKey;
                }
            }
            // 完全偏左：丢弃当前 s1 段。
            FailedToClampReason::LeftNotOverlapped => current_clamping += 1,
            // 完全偏右：推进 clamp 目标。
            FailedToClampReason::RightNotOverlapped => current_clamp_target += 1,
            FailedToClampReason::BuggyUnknown => {
                // 理论不可达；打日志保留现场便于对齐 Go 排查。
                log::Warn(
                    "Unreachable path reached",
                    [
                        astersql_br_pkg_logutil::Field::string(
                            "over-ranges",
                            &StringifyKeys(s1.clone()).to_string(),
                        ),
                        astersql_br_pkg_logutil::Field::string(
                            "clamp-into",
                            &StringifyKeys(s2.clone()).to_string(),
                        ),
                        astersql_br_pkg_logutil::Field::string(
                            "current-clamping",
                            &StringifyRange::from(crg_log).to_string(),
                        ),
                        astersql_br_pkg_logutil::Field::string(
                            "current-target",
                            &StringifyRange::from(cin).to_string(),
                        ),
                    ],
                );
            }
        }
    }
    rs
}

/// Formats a timestamp like Go `time.Format("2006-01-02 15:04:05.999999999 -0700")`.
/// `unix_nanos` is the absolute instant; `offset_seconds` selects the displayed zone
/// (wall clock = UTC + offset), matching `ts.In(loc).Format(...)`.
/// 小数部分去尾零；整秒省略点号，与 Go `.999999999` 布局一致。
pub fn FormatDate(unix_nanos: i128, offset_seconds: i32) -> String {
    let adjusted = unix_nanos + (offset_seconds as i128) * 1_000_000_000;
    let secs = adjusted.div_euclid(1_000_000_000);
    let nanos = adjusted.rem_euclid(1_000_000_000) as u32;
    let (year, month, day, hour, minute, second) = unix_secs_to_utc(secs);
    let sign = if offset_seconds >= 0 { '+' } else { '-' };
    let offset = offset_seconds.unsigned_abs();
    let offset_hour = offset / 3600;
    let offset_min = (offset % 3600) / 60;
    // Go's `.999999999` trims trailing zeros; whole seconds omit the fractional part.
    if nanos == 0 {
        format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} {sign}{offset_hour:02}{offset_min:02}"
        )
    } else {
        let frac = format!("{nanos:09}");
        let frac = frac.trim_end_matches('0');
        format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{frac} {sign}{offset_hour:02}{offset_min:02}"
        )
    }
}

/// 将 Unix 秒转为公历墙钟字段；含 400/100/4 年周期与闰年二月。
fn unix_secs_to_utc(secs: i128) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let mut rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as u32;
    rem %= 3600;
    let minute = (rem / 60) as u32;
    let second = (rem % 60) as u32;

    // Proleptic Gregorian conversion, valid on both sides of the Unix epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day as u32, hour, minute, second)
}

/// TiDB meta 库键前缀 `mDB`。
pub fn IsMetaDBKey(key: &[u8]) -> bool {
    key.starts_with(b"mDB")
}

/// DDL job history 前缀 `mDDLJobH`。
pub fn IsMetaDDLJobHistoryKey(key: &[u8]) -> bool {
    key.starts_with(b"mDDLJobH")
}

/// 宽前缀 `mD`：同时覆盖 DB 与 DDL 相关 meta 键。
pub fn IsDBOrDDLJobHistoryKey(key: &[u8]) -> bool {
    key.starts_with(b"mD")
}

/// 编码带 ts 的事务 meta 键：EncodeMetaKey → EncodeBytes → EncodeUintDesc。
pub fn EncodeTxnMetaKey(key: &[u8], field: &[u8], ts: u64) -> Vec<u8> {
    let k = EncodeMetaKey(key, field);
    let txn_key = EncodeBytes(Vec::new(), k.as_ref());
    EncodeUintDesc(txn_key, ts)
}

/// 判断编码后的键是否为各类 AutoID / Sequence meta field。
pub fn IsMetaAutoIDKey(key: &[u8]) -> bool {
    // 末 8 字节为 ts；不足长度直接否。
    if key.len() < 8 {
        return false;
    }
    let (_, meta_key_bytes) = match DecodeBytes(&key[..key.len() - 8], None) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let (_, field) = match DecodeMetaKey(TableKey(meta_key_bytes)) {
        Ok(v) => v,
        Err(_) => return false,
    };
    is_auto_increment_id_key(&field)
        || is_auto_table_id_key(&field)
        || is_auto_random_table_id_key(&field)
        || is_sequence_key(&field)
}

/// 严格偶数长度十六进制解码；非法字符返回 InvalidInput。
pub fn hex_decode_string(text: &str) -> Result<Vec<u8>, SharedError> {
    if !text.len().is_multiple_of(2) {
        return Err(SharedError::new(std::io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid hex length",
        )));
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, SharedError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(SharedError::new(std::io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid hex digit",
        ))),
    }
}

/// 小写十六进制编码，供 JSON meta 中的 key/sha256 字段使用。
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(hex_char(byte >> 4));
        out.push(hex_char(byte & 0x0f));
    }
    out
}

fn hex_char(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'a' + nibble - 10) as char,
        _ => '?',
    }
}
