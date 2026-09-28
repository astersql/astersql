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

//! 日志备份 backupmeta 文件名解析。
//! 对齐 Go `br/pkg/stream/backupmetas`：支持四段 hex 的旧格式，以及
//! `flushTs||storeID` 前缀 + 标签后缀的新格式；并从解析结果计算 shift TS。
//! 解析失败统一带文件名上下文，便于排查损坏或混入非 meta 对象。
//! shift TS 供 PiTR 窗口对齐 DefaultCF begin，避免漏扫历史写入。

use std::sync::LazyLock;

/// 旧格式固定四段：flushTs-minBegin-minTs-maxTs。
const LEGACY_BACKUP_META_PART_COUNT: usize = 4;
/// 每个标签段长度：1 字节 tag + 16 hex。
const TAGGED_META_TAG_VALUE_LEN: usize = 17;

/// 标签 `d`：DefaultCF 上的最小 begin TS。
pub const NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG: u8 = b'd';
/// 标签 `l`：文件覆盖的最小 TS。
pub const NAME_MIN_TS_TAG: u8 = b'l';
/// 标签 `u`：文件覆盖的最大 TS。
pub const NAME_MAX_TS_TAG: u8 = b'u';
/// 标签 `p`：可选 flags 位图。
pub const NAME_FLAGS_TAG: u8 = b'p';

/// flags 位：置位表示该 meta 不含 DDL 文件。
const FLAG_NO_DDL_FILES: u64 = 1 << 0;
// 以下 LazyLock 正则在首次 Parse 时编译，失败则进程启动期 panic（与 Go MustCompile 同意图）。

/// 旧文件名：四段 16 位 hex，以 `-` 连接。
static LEGACY_BACKUP_META_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^([0-9a-fA-F]{16})-([0-9a-fA-F]{16})-([0-9a-fA-F]{16})-([0-9a-fA-F]{16})$")
        .expect("legacy backupmeta regexp must compile")
});
/// 新文件名：32 hex 前缀 + 若干 `(tag + 16hex)` 后缀。
static TAGGED_BACKUP_META_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^[0-9a-fA-F]{32}-(?:[0-9A-Za-z][0-9a-fA-F]{16})+$")
        .expect("tagged backupmeta regexp must compile")
});

/// 从文件名解析出的 TS / store / flags 视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedName {
    /// flush 水位对应的 TS。
    pub FlushTS: u64,
    /// 产出该 meta 的 TiKV store id（legacy 恒为 0）。
    pub StoreID: u64,
    /// DefaultCF 观察到的最小 begin TS，供 shift 计算。
    pub MinBeginTsInDefaultCf: u64,
    /// 文件数据覆盖的最小/最大 TS。
    pub MinTS: u64,
    pub MaxTS: u64,
    /// 原始 flags 位图；仅当 HasFlags 时有意义。
    pub Flags: u64,
    /// 是否出现过 `p` 标签；缺省时 HasDDLFiles 视为 true。
    pub HasFlags: bool,
}

/// CalculateShiftTS 的结果状态，与 Go 枚举数值一致。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ShiftTSStatus {
    ShiftTSFound = 0,
    ShiftTSNotFound = 1,
    ShiftTSInvalidStats = 2,
}

/// 自动识别 tagged / legacy 格式；都不匹配则报格式错误。
pub fn ParseName(fileName: &str) -> Result<ParsedName, String> {
    // 先试 tagged，再试 legacy
    if TAGGED_BACKUP_META_PATTERN.is_match(fileName) {
        return parseTaggedBackupMetaFileName(fileName);
    }
    if LEGACY_BACKUP_META_PATTERN.is_match(fileName) {
        return parseLegacyBackupMetaFileName(fileName);
    }
    // 两种格式皆不匹配
    Err(format!("invalid backupmeta file name format: {fileName}"))
}

impl ParsedName {
    /// 在 [startTS, restoreTS] 窗口内取可用于 shift 的 MinBeginTsInDefaultCf。
    /// 与窗口无交集 → NotFound；统计非法（0 或 > MinTS）→ InvalidStats。
    pub fn CalculateShiftTS(&self, startTS: u64, restoreTS: u64) -> (u64, ShiftTSStatus) {
        if self.MinTS > restoreTS || self.MaxTS < startTS {
            return (0, ShiftTSStatus::ShiftTSNotFound);
        }
        if self.MinBeginTsInDefaultCf == 0 || self.MinBeginTsInDefaultCf > self.MinTS {
            return (0, ShiftTSStatus::ShiftTSInvalidStats);
        }
        (self.MinBeginTsInDefaultCf, ShiftTSStatus::ShiftTSFound)
    }

    /// 无 flags 标签时默认认为含 DDL；有 flags 则看 FLAG_NO_DDL_FILES。
    pub fn HasDDLFiles(&self) -> bool {
        if !self.HasFlags {
            return true;
        }
        self.Flags & FLAG_NO_DDL_FILES == 0
    }
}

/// 仅接受 tagged 格式（用于最新 meta 路径），拒绝 legacy。
pub fn TryParseTaggedBackupMetaFileName(fileName: &str) -> Result<ParsedName, String> {
    if !TAGGED_BACKUP_META_PATTERN.is_match(fileName) {
        return Err(format!(
            "invalid latest backupmeta file name format: {fileName}"
        ));
    }
    parseTaggedBackupMetaFileName(fileName)
}

/// 解析四段 legacy 名；StoreID/Flags 保持默认 0。
fn parseLegacyBackupMetaFileName(fileName: &str) -> Result<ParsedName, String> {
    // 正则已保证段数，此处再防御一次
    let parts: Vec<&str> = fileName.split('-').collect();
    if parts.len() != LEGACY_BACKUP_META_PART_COUNT {
        return Err(format!(
            "invalid backupmeta legacy file name format: {fileName}"
        ));
    }
    let flushTs = parseBackupMetaHexU64(fileName, "flushTs", parts[0])?;
    let minBeginTsInDefaultCf = parseBackupMetaHexU64(fileName, "minBeginTsInDefaultCf", parts[1])?;
    let minTs = parseBackupMetaHexU64(fileName, "minTs", parts[2])?;
    let maxTs = parseBackupMetaHexU64(fileName, "maxTs", parts[3])?;
    // StoreID/Flags/HasFlags 走 Default（legacy 无这些字段）
    Ok(ParsedName {
        FlushTS: flushTs,
        MinBeginTsInDefaultCf: minBeginTsInDefaultCf,
        MinTS: minTs,
        MaxTS: maxTs,
        ..Default::default()
    })
}

/// 解析 tagged 名：前 16 hex 为 flushTs，后 16 为 storeID；后缀按 tag 填充字段。
/// `d`/`l`/`u` 必填且不可重复；未知 tag 忽略以保持前向兼容。
fn parseTaggedBackupMetaFileName(fileName: &str) -> Result<ParsedName, String> {
    let (prefix, suffix) = fileName
        .split_once('-')
        .ok_or_else(|| format!("invalid backupmeta tagged file name format: {fileName}"))?;
    if prefix.len() != 32 {
        return Err(format!(
            "invalid backupmeta tagged prefix length in {fileName}"
        ));
    }
    // 前 16 hex = flushTs，后 16 hex = storeID
    let flushTs = parseBackupMetaHexU64(fileName, "flushTs", &prefix[..16])?;
    let storeID = parseBackupMetaHexU64(fileName, "storeID", &prefix[16..])?;
    let mut minBeginTsInDefaultCf = 0;
    let mut minTs = 0;
    let mut maxTs = 0;
    let mut flags = 0;
    let mut hasFlags = false;
    let mut seenTags = [false; 256];
    let bytes = suffix.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let remain = bytes.len() - pos;
        if remain < TAGGED_META_TAG_VALUE_LEN {
            return Err(format!("incomplete tag segment in {fileName}"));
        }
        let tag = bytes[pos];
        if !isASCIIAlphanumeric(tag) {
            return Err(format!(
                "invalid suffix tag {:?} in {fileName}",
                tag as char
            ));
        }
        // 跳过 tag 字节，取随后 16 hex
        let hexValue = &suffix[pos + 1..pos + TAGGED_META_TAG_VALUE_LEN];
        let value = parseBackupMetaHexU64(fileName, "tag value", hexValue)?;
        if seenTags[tag as usize] {
            return Err(format!(
                "duplicate suffix tag {:?} in {fileName}",
                tag as char
            ));
        }
        seenTags[tag as usize] = true;
        match tag {
            NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG => minBeginTsInDefaultCf = value,
            NAME_MIN_TS_TAG => minTs = value,
            NAME_MAX_TS_TAG => maxTs = value,
            NAME_FLAGS_TAG => {
                hasFlags = true;
                flags = value;
            }
            _ => {}
        }
        pos += TAGGED_META_TAG_VALUE_LEN;
    }
    // d/l/u 为契约必填；缺任一标签则无法安全计算覆盖窗口。
    for tag in [
        NAME_MIN_BEGIN_TS_IN_DEFAULT_CF_TAG,
        NAME_MIN_TS_TAG,
        NAME_MAX_TS_TAG,
    ] {
        if !seenTags[tag as usize] {
            return Err(format!("missing {:?} tag in {fileName}", tag as char));
        }
    }
    Ok(ParsedName {
        FlushTS: flushTs,
        StoreID: storeID,
        MinBeginTsInDefaultCf: minBeginTsInDefaultCf,
        MinTS: minTs,
        MaxTS: maxTs,
        Flags: flags,
        HasFlags: hasFlags,
    })
}

/// 解析恰好 16 位 hex 的 u64；长度或进制错误时带上文件名上下文。
fn parseBackupMetaHexU64(fileName: &str, partName: &str, hexPart: &str) -> Result<u64, String> {
    if hexPart.len() != 16 {
        return Err(format!("{partName} must be 16 hex digits in {fileName}"));
    }
    u64::from_str_radix(hexPart, 16)
        .map_err(|err| format!("failed to parse {partName} in {fileName}: {err}"))
}

/// 标签字节须为 ASCII 字母或数字。
fn isASCIIAlphanumeric(ch: u8) -> bool {
    (b'0' <= ch && ch <= b'9') || (b'a' <= ch && ch <= b'z') || (b'A' <= ch && ch <= b'Z')
}
