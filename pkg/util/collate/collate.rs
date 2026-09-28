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

// Collation（排序规则）入口：注册/查找 Collator、开关新 collation，以及通用比较与 ID 编解码。
//
// Collation 决定字符串比较与排序 key 的语义（大小写/口音是否敏感、是否 PAD SPACE）。
// 新 collation 开启时按名称或 ID 分发到具体实现；关闭时统一回退为 binary 风格。

use std::any::Any;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::bin::{binCollator, binPaddingCollator, derivedBinCollator};
use crate::charset;
use crate::charset_switch::switchDefaultCollation;
use crate::gb18030_bin::gb18030BinCollator;
use crate::gb18030_chinese_ci::gb18030ChineseCICollator;
use crate::gbk_bin::gbkBinCollator;
use crate::gbk_chinese_ci::gbkChineseCICollator;

/// 未指定长度时的默认值占位（与 Go `DefaultLen` 对齐）。
pub const DefaultLen: i32 = 0;

/// Go `ErrUnsupportedCollation`：DDL 类、MySQL 1273、定制消息模板。
pub static ErrUnsupportedCollation: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    dbterror::ClassDDL.NewStdErr(
        dbterror::errno::ErrUnknownCollation,
        &dbterror::terror::parser::mysql::errname::Message(
            "Unsupported collation when new collation is enabled: '%-.64s'",
            &[],
        ),
    )
});

/// Go `ErrIllegalMixCollation`：表达式类、MySQL 1271。
pub static ErrIllegalMixCollation: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    synthesize_standard_expression_error(dbterror::errno::ErrCantAggregateNcollations)
});

/// Go `ErrIllegalMix2Collation`：表达式类、MySQL 1267。
pub static ErrIllegalMix2Collation: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    synthesize_standard_expression_error(dbterror::errno::ErrCantAggregate2collations)
});

/// Go `ErrIllegalMix3Collation`：表达式类、MySQL 1270。
pub static ErrIllegalMix3Collation: LazyLock<Box<dbterror::terror::Error>> = LazyLock::new(|| {
    synthesize_standard_expression_error(dbterror::errno::ErrCantAggregate3collations)
});

fn synthesize_standard_expression_error(code: u16) -> Box<dbterror::terror::Error> {
    dbterror::ClassExpression.NewStd(code)
}

// 与 Go 包级 var 初始化一致：在 main/libtest 前注册四个错误模板，避免
// `RegisterFinish` 后首次访问才尝试登记错误码。
#[cfg(any(target_family = "unix", target_os = "windows"))]
#[used]
#[cfg_attr(
    all(target_family = "unix", not(target_vendor = "apple")),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
static COLLATE_PACKAGE_INIT: extern "C" fn() = {
    extern "C" fn initialize() {
        LazyLock::force(&ErrUnsupportedCollation);
        LazyLock::force(&ErrIllegalMixCollation);
        LazyLock::force(&ErrIllegalMix2Collation);
        LazyLock::force(&ErrIllegalMix3Collation);
    }
    initialize
};

/// 字符串 Collator：比较、生成排序 key、通配符 pattern，以及克隆/类型查询。
pub trait Collator: Send + Sync {
    /// 按当前 collation 比较两字符串，返回 -1/0/1。
    fn Compare(&self, a: &str, b: &str) -> i32;
    /// Go string 的原始字节入口；允许保留无效 UTF-8。
    fn CompareBytes(&self, a: &[u8], b: &[u8]) -> i32 {
        let a = String::from_utf8_lossy(a);
        let b = String::from_utf8_lossy(b);
        self.Compare(&a, &b)
    }
    /// 生成可变排序 key（PAD SPACE 时会先裁尾部空格）。
    fn Key(&self, str_: &str) -> Vec<u8>;
    /// Go string 的原始字节 key 入口；允许保留无效 UTF-8。
    fn KeyBytes(&self, value: &[u8]) -> Vec<u8> {
        let value = String::from_utf8_lossy(value);
        self.Key(&value)
    }
    /// 生成不可变语义的排序 key（调用方不应修改返回缓冲）。
    fn ImmutableKey(&self, str_: &str) -> Vec<u8>;
    /// 不裁尾部空格地生成排序 key。
    fn KeyWithoutTrimRightSpace(&self, str_: &str) -> Vec<u8>;
    /// 构造 LIKE/通配符匹配用的 pattern 状态机。
    fn Pattern(&self) -> Box<dyn WildcardPattern>;
    /// 克隆一份独立的 Collator 实例。
    fn Clone(&self) -> Box<dyn Collator>;
    /// 估算字符串对应排序 key 的最大字节长度。
    fn MaxKeyLen(&self, str_: &str) -> i32;
    /// 向下转型辅助，供 `CanUseRawMemAsKey` 等做具体类型判断。
    fn as_any(&self) -> &dyn Any;
}

/// LIKE/通配符 pattern：编译 pattern 串后对目标串做匹配。
pub trait WildcardPattern: Send + Sync {
    /// 编译 pattern；`escape` 为转义字节。
    fn Compile(&mut self, patternStr: &str, escape: u8);
    /// 在已编译 pattern 上匹配目标串。
    fn DoMatch(&self, str_: &str) -> bool;
}

/// 全局新 collation 开关：1 开启，0 关闭。
///
/// Go 在包 `init` 中先设为 1，保证 bootstrap 之前的测试和临时会话使用真实
/// collation；生产 bootstrap 随后会用系统表参数覆盖它。
static newCollationEnabled: AtomicI32 = AtomicI32::new(1);

/// 测试用：同步默认 charset collation 并设置全局新 collation 开关。
pub fn SetNewCollationEnabledForTest(flag: bool) {
    switchDefaultCollation(flag);
    newCollationEnabled.store(i32::from(flag), Ordering::SeqCst);
}

/// 查询全局新 collation 是否开启。
pub fn NewCollationEnabled() -> bool {
    newCollationEnabled.load(Ordering::SeqCst) == 1
}

/// 判断两个 collation 名在兼容分组内是否可互换（同组或同名）。
pub fn CompatibleCollate(a: &str, b: &str) -> bool {
    // 分组：general_ci / bin / unicode_ci；0 表示不参与兼容折叠。
    let compatible_group = |name: &str| match name {
        "utf8mb4_general_ci" | "utf8_general_ci" => 1,
        "utf8mb4_bin" | "utf8_bin" | "latin1_bin" => 2,
        "utf8mb4_unicode_ci" | "utf8_unicode_ci" => 3,
        _ => 0,
    };
    a == b || compatible_group(a) != 0 && compatible_group(a) == compatible_group(b)
}

/// 新 collation 开启且 ID 非负时取负，用于协议侧区分新旧编码。
pub fn RewriteNewCollationIDIfNeeded(id: i32) -> i32 {
    if NewCollationEnabled() && id >= 0 {
        id.wrapping_neg()
    } else {
        id
    }
}

/// 新 collation 开启且 ID 非正时取负，把协议侧负数 ID 还原为正数。
pub fn RestoreCollationIDIfNeeded(id: i32) -> i32 {
    if NewCollationEnabled() && id <= 0 {
        id.wrapping_neg()
    } else {
        id
    }
}

/// 按名称构造 Collator；未知名称交给 `group2_collator`（可能依赖 full_collate）。
fn new_collator(name: &str) -> Option<Box<dyn Collator>> {
    Some(match name {
        "binary" => Box::new(binCollator::default()),
        "ascii_bin" | "latin1_bin" | "utf8_bin" | "utf8mb4_bin" => {
            Box::new(binPaddingCollator::default())
        }
        "utf8mb4_0900_bin" => Box::new(derivedBinCollator::default()),
        "gbk_bin" => Box::new(gbkBinCollator::default()),
        "gbk_chinese_ci" => Box::new(gbkChineseCICollator::default()),
        "gb18030_bin" => Box::new(gb18030BinCollator::default()),
        "gb18030_chinese_ci" => Box::new(gb18030ChineseCICollator::default()),
        _ => return group2_collator(name),
    })
}

/// full_collate 特性下的第二组：general_ci / unicode_ci / 0900_ai_ci / 拼音。
#[cfg(feature = "full_collate")]
fn group2_collator(name: &str) -> Option<Box<dyn Collator>> {
    use crate::{
        generalCICollator, unicode0900AICICollator, unicodeCICollator, zhPinyinTiDBASCSCollator,
    };
    Some(match name {
        "utf8mb4_general_ci" | "utf8_general_ci" => Box::new(generalCICollator::default()),
        "utf8mb4_unicode_ci" | "utf8_unicode_ci" => Box::new(unicodeCICollator::default()),
        "utf8mb4_0900_ai_ci" => Box::new(unicode0900AICICollator::default()),
        "utf8mb4_zh_pinyin_tidb_as_cs" => Box::new(zhPinyinTiDBASCSCollator::default()),
        _ => return None,
    })
}

/// 未启用 full_collate 时第二组一律不可用。
#[cfg(not(feature = "full_collate"))]
fn group2_collator(_: &str) -> Option<Box<dyn Collator>> {
    None
}

/// 按当前全局开关返回指定名称的 Collator。
pub fn GetCollator(collate: &str) -> Box<dyn Collator> {
    GetCollatorWithCollate(NewCollationEnabled(), collate)
}

/// 显式指定是否使用新 collation：关闭时统一 `derivedBinCollator`。
pub fn GetCollatorWithCollate(useNewCollate: bool, collate: &str) -> Box<dyn Collator> {
    if useNewCollate {
        // 未知名称回退为 PAD SPACE 的 bin collator。
        new_collator(collate).unwrap_or_else(|| Box::new(binPaddingCollator::default()))
    } else {
        Box::new(derivedBinCollator::default())
    }
}

/// 返回无 PAD SPACE 的 binary collator。
pub fn GetBinaryCollator() -> Box<dyn Collator> {
    Box::new(derivedBinCollator::default())
}

/// 构造 `n` 个独立的 binary collator。
pub fn GetBinaryCollatorSlice(n: usize) -> Vec<Box<dyn Collator>> {
    (0..n).map(|_| GetBinaryCollator()).collect()
}

/// 按 collation ID 查找；新 collation 关闭时直接返回 binary。
pub fn GetCollatorByID(id: i32) -> Box<dyn Collator> {
    if !NewCollationEnabled() {
        return GetBinaryCollator();
    }
    charset::GetCollationByID(id)
        .ok()
        .and_then(|collation| new_collator(&collation.Name))
        .unwrap_or_else(|| Box::new(binPaddingCollator::default()))
}

/// collation ID → 名称；未知时回退默认 collation 名。
pub fn CollationID2Name(id: i32) -> String {
    charset::GetCollationByID(id)
        .map(|collation| collation.Name)
        .unwrap_or_else(|_| crate::mysql::DefaultCollationName.to_owned())
}

/// collation 名称 → ID；未知时回退默认 collation ID。
pub fn CollationName2ID(name: &str) -> i32 {
    charset::GetCollationByName(name)
        .map(|collation| collation.ID)
        .unwrap_or(i32::from(crate::mysql::DefaultCollationID))
}

/// 按名称取 charset 元数据；新 collation 开启且无实现时返回 unsupported。
pub fn GetCollationByName(name: &str) -> Result<charset::Collation, dbterror::errors::SharedError> {
    let coll = charset::GetCollationByName(name)?;
    // Go checks `newCollatorIDMap[coll.ID]` after charset lookup has normalized
    // case and utf8mb3 aliases. Use the canonical name from that same lookup so
    // aliases inherit the support status of their resolved collation ID.
    if NewCollationEnabled() && new_collator(&coll.Name).is_none() {
        return Err(ErrUnsupportedCollation
            .GenWithStackByArgs(&[dbterror::errors::ErrorArg::String(name.to_owned())]));
    }
    Ok(coll)
}

/// 名称可用则原样返回，否则替换为默认 collation 名。
pub fn SubstituteMissingCollationToDefault(co: &str) -> String {
    if GetCollationByName(co).is_ok() {
        co.to_owned()
    } else {
        crate::mysql::DefaultCollationName.to_owned()
    }
}

/// 列出当前实现真正支持的 collation；新开关关闭时透传 charset 全表。
pub fn GetSupportedCollations() -> Vec<charset::Collation> {
    if !NewCollationEnabled() {
        return charset::GetSupportedCollations();
    }
    let mut result: Vec<_> = charset::GetSupportedCollations()
        .into_iter()
        .filter(|collation| new_collator(&collation.Name).is_some())
        .collect();
    result.sort_by(|a, b| a.Name.cmp(&b.Name));
    result
}

/// PAD SPACE：去掉字符串尾部 ASCII 空格（MySQL 比较语义）。
pub fn truncateTailingSpace(str_: &str) -> &str {
    str_.trim_end_matches(' ')
}

/// PAD SPACE 的原始字节版本；只裁 ASCII 空格，不要求 UTF-8 合法。
pub(crate) fn truncateTailingSpaceBytes(mut value: &[u8]) -> &[u8] {
    while value.last() == Some(&b' ') {
        value = &value[..value.len() - 1];
    }
    value
}

/// 按 Go `utf8.DecodeRuneInString` 解出一个 rune。
///
/// 非法编码返回 `REPLACEMENT_CHARACTER`、`invalid = true`，并只消费一个字节；
/// 合法编码出的 U+FFFD 则 `invalid = false`。
pub(crate) fn decodeRune(value: &[u8], index: &mut usize) -> (char, bool) {
    if *index >= value.len() {
        return ('\0', false);
    }

    let tail = &value[*index..];
    match std::str::from_utf8(tail) {
        Ok(valid) => {
            let rune = valid.chars().next().expect("non-empty UTF-8 tail");
            *index += rune.len_utf8();
            (rune, false)
        }
        Err(error) if error.valid_up_to() > 0 => {
            let valid =
                std::str::from_utf8(&tail[..error.valid_up_to()]).expect("validated UTF-8 prefix");
            let rune = valid.chars().next().expect("non-empty UTF-8 prefix");
            *index += rune.len_utf8();
            (rune, false)
        }
        Err(_) => {
            *index += 1;
            (char::REPLACEMENT_CHARACTER, true)
        }
    }
}

/// 将 isize 差值规范化为 -1/0/1。
pub fn sign(i: isize) -> i32 {
    i.signum() as i32
}

/// 根据 UTF-8 首字节估算该字符占用字节数（1–4）。
pub fn runeLen(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0x80..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// 是否为 utf8mb4 的默认候选 collation 之一。
pub fn IsDefaultCollationForUTF8MB4(collate: &str) -> bool {
    matches!(
        collate,
        "utf8mb4_bin" | "utf8mb4_general_ci" | "utf8mb4_0900_ai_ci"
    )
}

/// 是否为大小写不敏感（CI）collation。
pub fn IsCICollation(collate: &str) -> bool {
    matches!(
        collate,
        "utf8_general_ci"
            | "utf8mb4_general_ci"
            | "utf8_unicode_ci"
            | "utf8mb4_unicode_ci"
            | "gbk_chinese_ci"
            | "utf8mb4_0900_ai_ci"
            | "gb18030_chinese_ci"
    )
}

/// 将 CI collation 映射到同 charset 的 bin 变体；其余原样返回。
pub fn ConvertAndGetBinCollation(collate: &str) -> String {
    match collate {
        "utf8_general_ci" | "utf8_unicode_ci" => "utf8_bin",
        "utf8mb4_general_ci" | "utf8mb4_unicode_ci" | "utf8mb4_0900_ai_ci" => "utf8mb4_bin",
        "gbk_chinese_ci" => "gbk_bin",
        "gb18030_chinese_ci" => "gb18030_bin",
        _ => collate,
    }
    .to_owned()
}

/// 取与给定 collation 对应的 bin Collator。
pub fn ConvertAndGetBinCollator(collate: &str) -> Box<dyn Collator> {
    GetCollator(&ConvertAndGetBinCollation(collate))
}

/// 是否为 binary 风格 collation（含 utf8mb4_0900_bin）。
pub fn IsBinCollation(collate: &str) -> bool {
    matches!(
        collate,
        "ascii_bin" | "latin1_bin" | "utf8_bin" | "utf8mb4_bin" | "binary" | "utf8mb4_0900_bin"
    )
}

/// 是否为 PAD SPACE collation（比较前裁尾空格）；binary / 0900_* 除外。
pub fn IsPadSpaceCollation(collation: &str) -> bool {
    !matches!(
        collation,
        "binary" | "utf8mb4_0900_ai_ci" | "utf8mb4_0900_bin"
    )
}

/// 名称 → 协议侧 collation ID（可能经 Rewrite 取负）。
pub fn CollationToProto(c: &str) -> i32 {
    RewriteNewCollationIDIfNeeded(CollationName2ID(c))
}

/// 协议侧 ID → 名称（先 Restore 再查表）。
pub fn ProtoToCollation(c: i32) -> String {
    CollationID2Name(RestoreCollationIDIfNeeded(c))
}

/// 通用逐 rune 比较：先裁尾空格，再用 `keyFunc` 映射权重后字典序比较。
pub fn compareCommon(a: &str, b: &str, keyFunc: fn(char) -> u32) -> i32 {
    compareCommonBytes(a.as_bytes(), b.as_bytes(), keyFunc)
}

/// `compareCommon` 的 Go string 字节入口；任一侧遇到首个非法 UTF-8 字节即返回 0。
pub fn compareCommonBytes(a: &[u8], b: &[u8], keyFunc: fn(char) -> u32) -> i32 {
    let a = truncateTailingSpaceBytes(a);
    let b = truncateTailingSpaceBytes(b);
    let (mut ai, mut bi) = (0, 0);

    while ai < a.len() && bi < b.len() {
        let (left, left_invalid) = decodeRune(a, &mut ai);
        let (right, right_invalid) = decodeRune(b, &mut bi);
        if left_invalid || right_invalid {
            return 0;
        }
        match keyFunc(left).cmp(&keyFunc(right)) {
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Equal => {}
        }
    }
    sign((a.len() - ai) as isize - (b.len() - bi) as isize)
}

/// binary / derivedBin 的 key 可直接用原始字节内存，无需再编码。
pub fn CanUseRawMemAsKey(c: &dyn Collator) -> bool {
    c.as_any().is::<binCollator>() || c.as_any().is::<derivedBinCollator>()
}
