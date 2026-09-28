// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 字符集（charset）与排序规则（collation）元数据注册表。
//
// 对照 `charset.go`：维护 MySQL 认识的完整字符集目录、当前真正支持编码处理的子集、
// 按 ID/名称查询排序规则，以及自定义字符集/排序规则的增删接口。
// “排序规则”决定字符串比较与排序时是否区分大小写、如何处理尾部空格等。

// errors、log、mysql、terror、zap 及跨文件模块连接留待后续统一接线。
// github.com/pingcap/tidb/pkg/parser/mysql、github.com/pingcap/tidb/pkg/parser/terror、go.uber.org/zap。
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, RwLock};

/// ErrUnknownCollation 对应 Go 的“未知排序规则”DDL 标准错误。
pub static ErrUnknownCollation: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    terror::ClassDDL.NewStd(terror::ErrCode(mysql::ErrUnknownCollation as isize))
});
/// ErrCollationCharsetMismatch 对应 Go 的“排序规则与字符集不匹配”标准错误。
pub static ErrCollationCharsetMismatch: LazyLock<Box<terror::Error>> = LazyLock::new(|| {
    terror::ClassDDL.NewStd(terror::ErrCode(mysql::ErrCollationCharsetMismatch as isize))
});

/// PadSpace 表示比较时忽略尾部空格。
pub const PadSpace: &str = "PAD SPACE";
/// PadNone 表示比较时保留尾部空格差异。
pub const PadNone: &str = "NO PAD";

/// Charset 对应 Go 的字符集元数据；Collations 保存该字符集可用的排序规则。
#[derive(Clone, Debug)]
pub struct Charset {
    /// 字符集名称，如 `utf8mb4`。
    pub Name: String,
    /// 该字符集的默认排序规则名。
    pub DefaultCollation: String,
    /// 隶属于此字符集的排序规则表。
    pub Collations: HashMap<String, Collation>,
    /// 人类可读描述。
    pub Desc: String,
    /// 单字符最大字节数。
    pub Maxlen: i32,
}

/// Collation 对应 Go 的排序规则元数据，字段顺序与源结构一致。
#[derive(Clone, Debug)]
pub struct Collation {
    /// MySQL 排序规则数字 ID。
    pub ID: i32,
    /// 所属字符集名称。
    pub CharsetName: String,
    /// 排序规则名称，如 `utf8mb4_bin`。
    pub Name: String,
    /// 是否为所属字符集的默认排序规则。
    pub IsDefault: bool,
    /// 排序键长度相关属性，与 Go 字段对齐。
    pub Sortlen: i32,
    /// 尾部空格处理：`PAD SPACE` 或 `NO PAD`。
    pub PadAttribute: String,
}

/// 构造一条字符集目录行。
fn charset_row(name: &str, maxlen: i32, default_collation: &str, desc: &str) -> Charset {
    Charset {
        Name: name.to_owned(), DefaultCollation: default_collation.to_owned(),
        Collations: HashMap::new(), Desc: desc.to_owned(), Maxlen: maxlen,
    }
}

/// 构造一条排序规则目录行。
fn collation_row(id: i32, charset: &str, name: &str, is_default: bool, sortlen: i32, pad: &str) -> Collation {
    Collation {
        ID: id, CharsetName: charset.to_owned(), Name: name.to_owned(),
        IsDefault: is_default, Sortlen: sortlen, PadAttribute: pad.to_owned(),
    }
}

// 以下映射对应 Go 的包级可变注册表。RwLock 保存 Go map 的共享可变形状，
// 不表示源实现新增了异步、IO 或额外并发协议。
/// 按排序规则 ID 索引。
static collationsIDMap: LazyLock<RwLock<HashMap<i32, Collation>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
/// 按排序规则名称索引。
static collationsNameMap: LazyLock<RwLock<HashMap<String, Collation>>> = LazyLock::new(|| RwLock::new(HashMap::new()));
/// 当前对外“受支持”的排序规则列表。
static supportedCollations: LazyLock<RwLock<Vec<Collation>>> = LazyLock::new(|| RwLock::new(Vec::new()));
/// 保证静态排序规则表只初始化一次。
static INITIALIZE_COLLATIONS: std::sync::Once = std::sync::Once::new();

/// CharacterSetInfos 包含当前真正支持编码处理的七种字符集。
pub static CharacterSetInfos: LazyLock<RwLock<HashMap<String, Charset>>> = LazyLock::new(|| {
    let rows = [
        charset_row(CharsetUTF8, 3, CollationUTF8, "UTF-8 Unicode"),
        charset_row(CharsetUTF8MB4, 4, CollationUTF8MB4, "UTF-8 Unicode"),
        charset_row(CharsetASCII, 1, CollationASCII, "US ASCII"),
        charset_row(CharsetLatin1, 1, CollationLatin1, "Latin1"),
        charset_row(CharsetBin, 1, CollationBin, "binary"),
        charset_row(CharsetGBK, 2, CollationGBKBin, "Chinese Internal Code Specification"),
        charset_row(CharsetGB18030, 4, CollationGB18030Bin, "China National Standard GB18030"),
    ];
    RwLock::new(rows.into_iter().map(|c| (c.Name.clone(), c)).collect())
});

/// 受支持排序规则名称白名单；登记时据此决定是否进入公开列表。
static supportedCollationNames: LazyLock<HashSet<&'static str>> = LazyLock::new(|| HashSet::from([
    CollationUTF8, CollationUTF8MB4, CollationASCII, CollationLatin1,
    CollationBin, CollationGBKBin, CollationGB18030Bin,
]));

/// 惰性初始化：把静态 `collations` 表写入各查询索引。
fn ensure_initialized() {
    INITIALIZE_COLLATIONS.call_once(|| {
        for collation in collations.iter().cloned() {
            add_collation(collation);
        }
    });
}

/// TiFlashSupportedCharsets 对应 Go 的 TiFlash 字符集白名单。
pub static TiFlashSupportedCharsets: LazyLock<HashSet<&'static str>> = LazyLock::new(|| HashSet::from([
    CharsetUTF8, CharsetUTF8MB4, CharsetASCII, CharsetLatin1, CharsetBin,
]));

/// GetSupportedCharsets 返回当前支持的字符集，并按名称稳定排序。
pub fn GetSupportedCharsets() -> Vec<Charset> {
    ensure_initialized();
    let mut result: Vec<_> = CharacterSetInfos.read().unwrap().values().cloned().collect();
    result.sort_by(|left, right| left.Name.cmp(&right.Name));
    result
}

/// GetSupportedCollations 返回初始化后登记为受支持的排序规则。
pub fn GetSupportedCollations() -> Vec<Collation> {
    ensure_initialized();
    supportedCollations.read().unwrap().clone()
}

/// ValidCharsetAndCollation 校验字符集和排序规则组合；空字符集与 utf8mb3 都按 utf8 处理。
pub fn ValidCharsetAndCollation(mut charset: &str, collation: &str) -> bool {
    if charset.is_empty() || charset == CharsetUTF8MB3 { charset = CharsetUTF8; }
    let Ok(info) = GetCharsetInfo(charset) else { return false; };
    if collation.is_empty() { return true; }
    let name = utf8Alias(&collation.to_lowercase());
    info.Collations.contains_key(&name)
}

/// GetDefaultCollationLegacy 保留旧解析器仅接受五类字符集及 utf8mb3 别名的限制。
pub fn GetDefaultCollationLegacy(charset: &str) -> Result<String, errors::SharedError> {
    match charset.to_lowercase().as_str() {
        CharsetUTF8MB3 => GetDefaultCollation(CharsetUTF8),
        CharsetUTF8 | CharsetUTF8MB4 | CharsetASCII | CharsetLatin1 | CharsetBin => GetDefaultCollation(charset),
        _ => Err(errors::Errorf("Unknown charset %s", &[errors::ErrorArg::String(charset.to_owned())])),
    }
}

/// GetDefaultCollation 返回字符集登记的默认排序规则。
pub fn GetDefaultCollation(charset: &str) -> Result<String, errors::SharedError> {
    Ok(GetCharsetInfo(charset)?.DefaultCollation)
}

/// GetDefaultCharsetAndCollate 返回 mysql 包定义的服务端默认值。
pub fn GetDefaultCharsetAndCollate() -> (String, String) {
    (mysql::DefaultCharset.to_owned(), mysql::DefaultCollationName.to_owned())
}

/// GetCharsetInfo 优先查询受支持表；已知但不支持与完全未知分别返回不同错误。
pub fn GetCharsetInfo(charset: &str) -> Result<Charset, errors::SharedError> {
    ensure_initialized();
    let name = if charset.to_lowercase() == CharsetUTF8MB3 { CharsetUTF8.to_owned() } else { charset.to_lowercase() };
    if let Some(info) = CharacterSetInfos.read().unwrap().get(&name) { return Ok(info.clone()); }
    if let Some(info) = charsets.read().unwrap().get(&name) {
        let _ = info;
        return Err(errors::Errorf("Unsupported charset %s", &[errors::ErrorArg::String(charset.to_owned())]));
    }
    Err(errors::Errorf("Unknown charset %s", &[errors::ErrorArg::String(charset.to_owned())]))
}

// GetCharsetInfoForIntroducer mirrors the Go scanner's use of GetCharsetInfo:
// unsupported-but-known charsets still form an underscoreCS token so the
// parser can report the character-introducer error with the charset name.
/// 词法 introducer（如 `_utf8mb4`）用：已知但不支持的字符集仍返回元数据，便于报错带名称。
pub fn GetCharsetInfoForIntroducer(charset: &str) -> Option<Charset> {
    ensure_initialized();
    let name = if charset.eq_ignore_ascii_case(CharsetUTF8MB3) {
        CharsetUTF8.to_owned()
    } else {
        charset.to_lowercase()
    };
    CharacterSetInfos
        .read()
        .unwrap()
        .get(&name)
        .cloned()
        .or_else(|| charsets.read().unwrap().get(&name).cloned())
}

/// GetCharsetInfoByID 按排序规则 ID 查询；失败时记录警告并连同错误返回默认值。
pub fn GetCharsetInfoByID(id: i32) -> (String, String, Option<errors::SharedError>) {
    ensure_initialized();
    if id == i32::from(mysql::DefaultCollationID) {
        return (mysql::DefaultCharset.to_owned(), mysql::DefaultCollationName.to_owned(), None);
    }
    if let Some(collation) = collationsIDMap.read().unwrap().get(&id) {
        return (collation.CharsetName.clone(), collation.Name.clone(), None);
    }
    log::warn!("unable to get collation name from collation ID {id}, returning defaults");
    (mysql::DefaultCharset.to_owned(), mysql::DefaultCollationName.to_owned(),
        Some(errors::Errorf("Unknown collation id %d", &[errors::ErrorArg::Signed(i128::from(id))])))
}

/// utf8Alias 把 MySQL 旧 utf8mb3 排序规则名归一化为 utf8 别名。
fn utf8Alias(name: &str) -> String {
    match name {
        "utf8mb3_bin" => "utf8_bin".to_owned(),
        "utf8mb3_unicode_ci" => "utf8_unicode_ci".to_owned(),
        "utf8mb3_general_ci" => "utf8_general_ci".to_owned(),
        _ => name.to_owned(),
    }
}

/// GetCollationByName 进行不区分大小写及 utf8mb3 别名查询。
pub fn GetCollationByName(name: &str) -> Result<Collation, errors::SharedError> {
    ensure_initialized();
    let canonical = utf8Alias(&name.to_lowercase());
    collationsNameMap.read().unwrap().get(&canonical).cloned()
        .ok_or_else(|| ErrUnknownCollation.GenWithStackByArgs(&[errors::ErrorArg::String(name.to_owned())]))
}

/// GetCollationByID 按数字 ID 查询排序规则。
pub fn GetCollationByID(id: i32) -> Result<Collation, errors::SharedError> {
    ensure_initialized();
    collationsIDMap.read().unwrap().get(&id).cloned()
        .ok_or_else(|| errors::Errorf("Unknown collation id %d", &[errors::ErrorArg::Signed(i128::from(id))]))
}

/// 二进制排序规则名。
pub const CollationBin: &str = "binary";
/// utf8（utf8mb3）二进制排序规则。
pub const CollationUTF8: &str = "utf8_bin";
/// utf8mb4 二进制排序规则。
pub const CollationUTF8MB4: &str = "utf8mb4_bin";
/// ascii 二进制排序规则。
pub const CollationASCII: &str = "ascii_bin";
/// latin1 二进制排序规则。
pub const CollationLatin1: &str = "latin1_bin";
/// gbk 二进制排序规则。
pub const CollationGBKBin: &str = "gbk_bin";
/// utf8mb4_0900_bin（MySQL 8.0 风格）。
pub const CollationUTF8MB40900Bin: &str = "utf8mb4_0900_bin";
/// gbk 中文 CI 排序规则。
pub const CollationGBKChineseCI: &str = "gbk_chinese_ci";
/// gb18030 二进制排序规则。
pub const CollationGB18030Bin: &str = "gb18030_bin";
/// gb18030 中文 CI 排序规则。
pub const CollationGB18030ChineseCI: &str = "gb18030_chinese_ci";

/// US-ASCII 字符集名。
pub const CharsetASCII: &str = "ascii";
/// 二进制伪字符集名。
pub const CharsetBin: &str = "binary";
/// latin1 字符集名。
pub const CharsetLatin1: &str = "latin1";
/// utf8（兼容名，实际常映射自 utf8mb3）字符集名。
pub const CharsetUTF8: &str = "utf8";
/// utf8mb3 字符集名（旧 utf8 别名）。
pub const CharsetUTF8MB3: &str = "utf8mb3";
/// utf8mb4 字符集名。
pub const CharsetUTF8MB4: &str = "utf8mb4";
/// GB18030 字符集名。
pub const CharsetGB18030: &str = "gb18030";
/// ARMSCII-8 字符集名（目录项，默认不支持编码）。
pub const CharsetARMSCII8: &str = "armscii8";
/// Big5 字符集名。
pub const CharsetBig5: &str = "big5";
/// Windows CP1250 字符集名。
pub const CharsetCP1250: &str = "cp1250";
/// Windows CP1251 字符集名。
pub const CharsetCP1251: &str = "cp1251";
/// Windows CP1256 字符集名。
pub const CharsetCP1256: &str = "cp1256";
/// Windows CP1257 字符集名。
pub const CharsetCP1257: &str = "cp1257";
/// DOS CP850 字符集名。
pub const CharsetCP850: &str = "cp850";
/// DOS CP852 字符集名。
pub const CharsetCP852: &str = "cp852";
/// DOS CP866 字符集名。
pub const CharsetCP866: &str = "cp866";
/// CP932 字符集名。
pub const CharsetCP932: &str = "cp932";
/// DEC8 字符集名。
pub const CharsetDEC8: &str = "dec8";
/// eucjpms 字符集名。
pub const CharsetEUCJPMS: &str = "eucjpms";
/// EUC-KR 字符集名。
pub const CharsetEUCKR: &str = "euckr";
/// GB2312 字符集名。
pub const CharsetGB2312: &str = "gb2312";
/// GBK 字符集名。
pub const CharsetGBK: &str = "gbk";
/// GEOSTD8 字符集名。
pub const CharsetGEOSTD8: &str = "geostd8";
/// Greek 字符集名。
pub const CharsetGreek: &str = "greek";
/// Hebrew 字符集名。
pub const CharsetHebrew: &str = "hebrew";
/// HP8 字符集名。
pub const CharsetHP8: &str = "hp8";
/// KEYBCS2 字符集名。
pub const CharsetKEYBCS2: &str = "keybcs2";
/// KOI8-R 字符集名。
pub const CharsetKOI8R: &str = "koi8r";
/// KOI8-U 字符集名。
pub const CharsetKOI8U: &str = "koi8u";
/// latin2 字符集名。
pub const CharsetLatin2: &str = "latin2";
/// latin5 字符集名。
pub const CharsetLatin5: &str = "latin5";
/// latin7 字符集名。
pub const CharsetLatin7: &str = "latin7";
/// Mac CE 字符集名。
pub const CharsetMacCE: &str = "macce";
/// Mac Roman 字符集名。
pub const CharsetMacRoman: &str = "macroman";
/// Shift-JIS 字符集名。
pub const CharsetSJIS: &str = "sjis";
/// swe7 字符集名。
pub const CharsetSWE7: &str = "swe7";
/// TIS620 字符集名。
pub const CharsetTIS620: &str = "tis620";
/// UCS-2 字符集名。
pub const CharsetUCS2: &str = "ucs2";
/// UJIS 字符集名。
pub const CharsetUJIS: &str = "ujis";
/// UTF-16 字符集名。
pub const CharsetUTF16: &str = "utf16";
/// UTF-16LE 字符集名。
pub const CharsetUTF16LE: &str = "utf16le";
/// UTF-32 字符集名。
pub const CharsetUTF32: &str = "utf32";

/// charsets 保存 MySQL 认识的完整字符集目录，其中多数仅用于区分“不支持”和“未知”。
static charsets: LazyLock<RwLock<HashMap<String, Charset>>> = LazyLock::new(|| {
    let rows = [
        charset_row(CharsetARMSCII8, 1, "armscii8_general_ci", "ARMSCII-8 Armenian"),
        charset_row(CharsetASCII, 1, "ascii_general_ci", "US ASCII"),
        charset_row(CharsetBig5, 2, "big5_chinese_ci", "Big5 Traditional Chinese"),
        charset_row(CharsetBin, 1, "binary", "Binary pseudo charset"),
        charset_row(CharsetCP1250, 1, "cp1250_general_ci", "Windows Central European"),
        charset_row(CharsetCP1251, 1, "cp1251_general_ci", "Windows Cyrillic"),
        charset_row(CharsetCP1256, 1, "cp1256_general_ci", "Windows Arabic"),
        charset_row(CharsetCP1257, 1, "cp1257_general_ci", "Windows Baltic"),
        charset_row(CharsetCP850, 1, "cp850_general_ci", "DOS West European"),
        charset_row(CharsetCP852, 1, "cp852_general_ci", "DOS Central European"),
        charset_row(CharsetCP866, 1, "cp866_general_ci", "DOS Russian"),
        charset_row(CharsetCP932, 2, "cp932_japanese_ci", "SJIS for Windows Japanese"),
        charset_row(CharsetDEC8, 1, "dec8_swedish_ci", "DEC West European"),
        charset_row(CharsetEUCJPMS, 3, "eucjpms_japanese_ci", "UJIS for Windows Japanese"),
        charset_row(CharsetEUCKR, 2, "euckr_korean_ci", "EUC-KR Korean"),
        charset_row(CharsetGB18030, 4, "gb18030_chinese_ci", "China National Standard GB18030"),
        charset_row(CharsetGB2312, 2, "gb2312_chinese_ci", "GB2312 Simplified Chinese"),
        charset_row(CharsetGBK, 2, "gbk_chinese_ci", "GBK Simplified Chinese"),
        charset_row(CharsetGEOSTD8, 1, "geostd8_general_ci", "GEOSTD8 Georgian"),
        charset_row(CharsetGreek, 1, "greek_general_ci", "ISO 8859-7 Greek"),
        charset_row(CharsetHebrew, 1, "hebrew_general_ci", "ISO 8859-8 Hebrew"),
        charset_row(CharsetHP8, 1, "hp8_english_ci", "HP West European"),
        charset_row(CharsetKEYBCS2, 1, "keybcs2_general_ci", "DOS Kamenicky Czech-Slovak"),
        charset_row(CharsetKOI8R, 1, "koi8u_general_ci", "KOI8-U Ukrainian"),
        charset_row(CharsetKOI8U, 1, "koi8r_general_ci", "KOI8-R Relcom Russian"),
        charset_row(CharsetLatin1, 1, "latin1_swedish_ci", "cp1252 West European"),
        charset_row(CharsetLatin2, 1, "latin2_general_ci", "ISO 8859-2 Central European"),
        charset_row(CharsetLatin5, 1, "latin5_turkish_ci", "ISO 8859-9 Turkish"),
        charset_row(CharsetLatin7, 1, "latin7_general_ci", "ISO 8859-13 Baltic"),
        charset_row(CharsetMacCE, 1, "macce_general_ci", "Mac Central European"),
        charset_row(CharsetMacRoman, 1, "macroman_general_ci", "Mac West European"),
        charset_row(CharsetSJIS, 2, "sjis_japanese_ci", "Shift-JIS Japanese"),
        charset_row(CharsetSWE7, 1, "swe7_swedish_ci", "7bit Swedish"),
        charset_row(CharsetTIS620, 1, "tis620_thai_ci", "TIS620 Thai"),
        charset_row(CharsetUCS2, 2, "ucs2_general_ci", "UCS-2 Unicode"),
        charset_row(CharsetUJIS, 3, "ujis_japanese_ci", "EUC-JP Japanese"),
        charset_row(CharsetUTF16, 4, "utf16_general_ci", "UTF-16 Unicode"),
        charset_row(CharsetUTF16LE, 4, "utf16le_general_ci", "UTF-16LE Unicode"),
        charset_row(CharsetUTF32, 4, "utf32_general_ci", "UTF-32 Unicode"),
        charset_row(CharsetUTF8, 3, "utf8_general_ci", "UTF-8 Unicode"),
        charset_row(CharsetUTF8MB4, 4, "utf8mb4_0900_ai_ci", "UTF-8 Unicode"),
    ];
    RwLock::new(rows.into_iter().map(|c| (c.Name.clone(), c)).collect())
});

/// collations 完整保留 Go 源文件内的 MySQL 排序规则目录和声明顺序。
static collations: LazyLock<Vec<Collation>> = LazyLock::new(|| {
    vec![
        collation_row(1, "big5", "big5_chinese_ci", true, 1, PadSpace),
        collation_row(2, "latin2", "latin2_czech_cs", false, 1, PadSpace),
        collation_row(3, "dec8", "dec8_swedish_ci", true, 1, PadSpace),
        collation_row(4, "cp850", "cp850_general_ci", true, 1, PadSpace),
        collation_row(5, "latin1", "latin1_german1_ci", false, 1, PadSpace),
        collation_row(6, "hp8", "hp8_english_ci", true, 1, PadSpace),
        collation_row(7, "koi8r", "koi8r_general_ci", true, 1, PadSpace),
        collation_row(8, "latin1", "latin1_swedish_ci", false, 1, PadSpace),
        collation_row(9, "latin2", "latin2_general_ci", true, 1, PadSpace),
        collation_row(10, "swe7", "swe7_swedish_ci", true, 1, PadSpace),
        collation_row(11, "ascii", "ascii_general_ci", false, 1, PadSpace),
        collation_row(12, "ujis", "ujis_japanese_ci", true, 1, PadSpace),
        collation_row(13, "sjis", "sjis_japanese_ci", true, 1, PadSpace),
        collation_row(14, "cp1251", "cp1251_bulgarian_ci", false, 1, PadSpace),
        collation_row(15, "latin1", "latin1_danish_ci", false, 1, PadSpace),
        collation_row(16, "hebrew", "hebrew_general_ci", true, 1, PadSpace),
        collation_row(18, "tis620", "tis620_thai_ci", true, 1, PadSpace),
        collation_row(19, "euckr", "euckr_korean_ci", true, 1, PadSpace),
        collation_row(20, "latin7", "latin7_estonian_cs", false, 1, PadSpace),
        collation_row(21, "latin2", "latin2_hungarian_ci", false, 1, PadSpace),
        collation_row(22, "koi8u", "koi8u_general_ci", true, 1, PadSpace),
        collation_row(23, "cp1251", "cp1251_ukrainian_ci", false, 1, PadSpace),
        collation_row(24, "gb2312", "gb2312_chinese_ci", true, 1, PadSpace),
        collation_row(25, "greek", "greek_general_ci", true, 1, PadSpace),
        collation_row(26, "cp1250", "cp1250_general_ci", true, 1, PadSpace),
        collation_row(27, "latin2", "latin2_croatian_ci", false, 1, PadSpace),
        collation_row(28, "gbk", "gbk_chinese_ci", false, 1, PadSpace),
        collation_row(29, "cp1257", "cp1257_lithuanian_ci", false, 1, PadSpace),
        collation_row(30, "latin5", "latin5_turkish_ci", true, 1, PadSpace),
        collation_row(31, "latin1", "latin1_german2_ci", false, 1, PadSpace),
        collation_row(32, "armscii8", "armscii8_general_ci", true, 1, PadSpace),
        collation_row(33, "utf8", "utf8_general_ci", false, 1, PadSpace),
        collation_row(34, "cp1250", "cp1250_czech_cs", false, 1, PadSpace),
        collation_row(35, "ucs2", "ucs2_general_ci", true, 1, PadSpace),
        collation_row(36, "cp866", "cp866_general_ci", true, 1, PadSpace),
        collation_row(37, "keybcs2", "keybcs2_general_ci", true, 1, PadSpace),
        collation_row(38, "macce", "macce_general_ci", true, 1, PadSpace),
        collation_row(39, "macroman", "macroman_general_ci", true, 1, PadSpace),
        collation_row(40, "cp852", "cp852_general_ci", true, 1, PadSpace),
        collation_row(41, "latin7", "latin7_general_ci", true, 1, PadSpace),
        collation_row(42, "latin7", "latin7_general_cs", false, 1, PadSpace),
        collation_row(43, "macce", "macce_bin", false, 1, PadSpace),
        collation_row(44, "cp1250", "cp1250_croatian_ci", false, 1, PadSpace),
        collation_row(45, "utf8mb4", "utf8mb4_general_ci", false, 1, PadSpace),
        collation_row(46, "utf8mb4", "utf8mb4_bin", true, 1, PadSpace),
        collation_row(47, "latin1", "latin1_bin", true, 1, PadSpace),
        collation_row(48, "latin1", "latin1_general_ci", false, 1, PadSpace),
        collation_row(49, "latin1", "latin1_general_cs", false, 1, PadSpace),
        collation_row(50, "cp1251", "cp1251_bin", false, 1, PadSpace),
        collation_row(51, "cp1251", "cp1251_general_ci", true, 1, PadSpace),
        collation_row(52, "cp1251", "cp1251_general_cs", false, 1, PadSpace),
        collation_row(53, "macroman", "macroman_bin", false, 1, PadSpace),
        collation_row(54, "utf16", "utf16_general_ci", true, 1, PadSpace),
        collation_row(55, "utf16", "utf16_bin", false, 1, PadSpace),
        collation_row(56, "utf16le", "utf16le_general_ci", true, 1, PadSpace),
        collation_row(57, "cp1256", "cp1256_general_ci", true, 1, PadSpace),
        collation_row(58, "cp1257", "cp1257_bin", false, 1, PadSpace),
        collation_row(59, "cp1257", "cp1257_general_ci", true, 1, PadSpace),
        collation_row(60, "utf32", "utf32_general_ci", true, 1, PadSpace),
        collation_row(61, "utf32", "utf32_bin", false, 1, PadSpace),
        collation_row(62, "utf16le", "utf16le_bin", false, 1, PadSpace),
        collation_row(63, "binary", "binary", true, 1, PadNone),
        collation_row(64, "armscii8", "armscii8_bin", false, 1, PadSpace),
        collation_row(65, "ascii", "ascii_bin", true, 1, PadSpace),
        collation_row(66, "cp1250", "cp1250_bin", false, 1, PadSpace),
        collation_row(67, "cp1256", "cp1256_bin", false, 1, PadSpace),
        collation_row(68, "cp866", "cp866_bin", false, 1, PadSpace),
        collation_row(69, "dec8", "dec8_bin", false, 1, PadSpace),
        collation_row(70, "greek", "greek_bin", false, 1, PadSpace),
        collation_row(71, "hebrew", "hebrew_bin", false, 1, PadSpace),
        collation_row(72, "hp8", "hp8_bin", false, 1, PadSpace),
        collation_row(73, "keybcs2", "keybcs2_bin", false, 1, PadSpace),
        collation_row(74, "koi8r", "koi8r_bin", false, 1, PadSpace),
        collation_row(75, "koi8u", "koi8u_bin", false, 1, PadSpace),
        collation_row(76, "utf8", "utf8_tolower_ci", false, 1, PadNone),
        collation_row(77, "latin2", "latin2_bin", false, 1, PadSpace),
        collation_row(78, "latin5", "latin5_bin", false, 1, PadSpace),
        collation_row(79, "latin7", "latin7_bin", false, 1, PadSpace),
        collation_row(80, "cp850", "cp850_bin", false, 1, PadSpace),
        collation_row(81, "cp852", "cp852_bin", false, 1, PadSpace),
        collation_row(82, "swe7", "swe7_bin", false, 1, PadSpace),
        collation_row(83, "utf8", "utf8_bin", true, 1, PadSpace),
        collation_row(84, "big5", "big5_bin", false, 1, PadSpace),
        collation_row(85, "euckr", "euckr_bin", false, 1, PadSpace),
        collation_row(86, "gb2312", "gb2312_bin", false, 1, PadSpace),
        collation_row(87, "gbk", "gbk_bin", true, 1, PadSpace),
        collation_row(88, "sjis", "sjis_bin", false, 1, PadSpace),
        collation_row(89, "tis620", "tis620_bin", false, 1, PadSpace),
        collation_row(90, "ucs2", "ucs2_bin", false, 1, PadSpace),
        collation_row(91, "ujis", "ujis_bin", false, 1, PadSpace),
        collation_row(92, "geostd8", "geostd8_general_ci", true, 1, PadSpace),
        collation_row(93, "geostd8", "geostd8_bin", false, 1, PadSpace),
        collation_row(94, "latin1", "latin1_spanish_ci", false, 1, PadSpace),
        collation_row(95, "cp932", "cp932_japanese_ci", true, 1, PadSpace),
        collation_row(96, "cp932", "cp932_bin", false, 1, PadSpace),
        collation_row(97, "eucjpms", "eucjpms_japanese_ci", true, 1, PadSpace),
        collation_row(98, "eucjpms", "eucjpms_bin", false, 1, PadSpace),
        collation_row(99, "cp1250", "cp1250_polish_ci", false, 1, PadSpace),
        collation_row(101, "utf16", "utf16_unicode_ci", false, 1, PadSpace),
        collation_row(102, "utf16", "utf16_icelandic_ci", false, 1, PadSpace),
        collation_row(103, "utf16", "utf16_latvian_ci", false, 1, PadSpace),
        collation_row(104, "utf16", "utf16_romanian_ci", false, 1, PadSpace),
        collation_row(105, "utf16", "utf16_slovenian_ci", false, 1, PadSpace),
        collation_row(106, "utf16", "utf16_polish_ci", false, 1, PadSpace),
        collation_row(107, "utf16", "utf16_estonian_ci", false, 1, PadSpace),
        collation_row(108, "utf16", "utf16_spanish_ci", false, 1, PadSpace),
        collation_row(109, "utf16", "utf16_swedish_ci", false, 1, PadSpace),
        collation_row(110, "utf16", "utf16_turkish_ci", false, 1, PadSpace),
        collation_row(111, "utf16", "utf16_czech_ci", false, 1, PadSpace),
        collation_row(112, "utf16", "utf16_danish_ci", false, 1, PadSpace),
        collation_row(113, "utf16", "utf16_lithuanian_ci", false, 1, PadSpace),
        collation_row(114, "utf16", "utf16_slovak_ci", false, 1, PadSpace),
        collation_row(115, "utf16", "utf16_spanish2_ci", false, 1, PadSpace),
        collation_row(116, "utf16", "utf16_roman_ci", false, 1, PadSpace),
        collation_row(117, "utf16", "utf16_persian_ci", false, 1, PadSpace),
        collation_row(118, "utf16", "utf16_esperanto_ci", false, 1, PadSpace),
        collation_row(119, "utf16", "utf16_hungarian_ci", false, 1, PadSpace),
        collation_row(120, "utf16", "utf16_sinhala_ci", false, 1, PadSpace),
        collation_row(121, "utf16", "utf16_german2_ci", false, 1, PadSpace),
        collation_row(122, "utf16", "utf16_croatian_ci", false, 1, PadSpace),
        collation_row(123, "utf16", "utf16_unicode_520_ci", false, 1, PadSpace),
        collation_row(124, "utf16", "utf16_vietnamese_ci", false, 1, PadSpace),
        collation_row(128, "ucs2", "ucs2_unicode_ci", false, 1, PadSpace),
        collation_row(129, "ucs2", "ucs2_icelandic_ci", false, 1, PadSpace),
        collation_row(130, "ucs2", "ucs2_latvian_ci", false, 1, PadSpace),
        collation_row(131, "ucs2", "ucs2_romanian_ci", false, 1, PadSpace),
        collation_row(132, "ucs2", "ucs2_slovenian_ci", false, 1, PadSpace),
        collation_row(133, "ucs2", "ucs2_polish_ci", false, 1, PadSpace),
        collation_row(134, "ucs2", "ucs2_estonian_ci", false, 1, PadSpace),
        collation_row(135, "ucs2", "ucs2_spanish_ci", false, 1, PadSpace),
        collation_row(136, "ucs2", "ucs2_swedish_ci", false, 1, PadSpace),
        collation_row(137, "ucs2", "ucs2_turkish_ci", false, 1, PadSpace),
        collation_row(138, "ucs2", "ucs2_czech_ci", false, 1, PadSpace),
        collation_row(139, "ucs2", "ucs2_danish_ci", false, 1, PadSpace),
        collation_row(140, "ucs2", "ucs2_lithuanian_ci", false, 1, PadSpace),
        collation_row(141, "ucs2", "ucs2_slovak_ci", false, 1, PadSpace),
        collation_row(142, "ucs2", "ucs2_spanish2_ci", false, 1, PadSpace),
        collation_row(143, "ucs2", "ucs2_roman_ci", false, 1, PadSpace),
        collation_row(144, "ucs2", "ucs2_persian_ci", false, 1, PadSpace),
        collation_row(145, "ucs2", "ucs2_esperanto_ci", false, 1, PadSpace),
        collation_row(146, "ucs2", "ucs2_hungarian_ci", false, 1, PadSpace),
        collation_row(147, "ucs2", "ucs2_sinhala_ci", false, 1, PadSpace),
        collation_row(148, "ucs2", "ucs2_german2_ci", false, 1, PadSpace),
        collation_row(149, "ucs2", "ucs2_croatian_ci", false, 1, PadSpace),
        collation_row(150, "ucs2", "ucs2_unicode_520_ci", false, 1, PadSpace),
        collation_row(151, "ucs2", "ucs2_vietnamese_ci", false, 1, PadSpace),
        collation_row(159, "ucs2", "ucs2_general_mysql500_ci", false, 1, PadSpace),
        collation_row(160, "utf32", "utf32_unicode_ci", false, 1, PadSpace),
        collation_row(161, "utf32", "utf32_icelandic_ci", false, 1, PadSpace),
        collation_row(162, "utf32", "utf32_latvian_ci", false, 1, PadSpace),
        collation_row(163, "utf32", "utf32_romanian_ci", false, 1, PadSpace),
        collation_row(164, "utf32", "utf32_slovenian_ci", false, 1, PadSpace),
        collation_row(165, "utf32", "utf32_polish_ci", false, 1, PadSpace),
        collation_row(166, "utf32", "utf32_estonian_ci", false, 1, PadSpace),
        collation_row(167, "utf32", "utf32_spanish_ci", false, 1, PadSpace),
        collation_row(168, "utf32", "utf32_swedish_ci", false, 1, PadSpace),
        collation_row(169, "utf32", "utf32_turkish_ci", false, 1, PadSpace),
        collation_row(170, "utf32", "utf32_czech_ci", false, 1, PadSpace),
        collation_row(171, "utf32", "utf32_danish_ci", false, 1, PadSpace),
        collation_row(172, "utf32", "utf32_lithuanian_ci", false, 1, PadSpace),
        collation_row(173, "utf32", "utf32_slovak_ci", false, 1, PadSpace),
        collation_row(174, "utf32", "utf32_spanish2_ci", false, 1, PadSpace),
        collation_row(175, "utf32", "utf32_roman_ci", false, 1, PadSpace),
        collation_row(176, "utf32", "utf32_persian_ci", false, 1, PadSpace),
        collation_row(177, "utf32", "utf32_esperanto_ci", false, 1, PadSpace),
        collation_row(178, "utf32", "utf32_hungarian_ci", false, 1, PadSpace),
        collation_row(179, "utf32", "utf32_sinhala_ci", false, 1, PadSpace),
        collation_row(180, "utf32", "utf32_german2_ci", false, 1, PadSpace),
        collation_row(181, "utf32", "utf32_croatian_ci", false, 1, PadSpace),
        collation_row(182, "utf32", "utf32_unicode_520_ci", false, 1, PadSpace),
        collation_row(183, "utf32", "utf32_vietnamese_ci", false, 1, PadSpace),
        collation_row(192, "utf8", "utf8_unicode_ci", false, 8, PadSpace),
        collation_row(193, "utf8", "utf8_icelandic_ci", false, 1, PadNone),
        collation_row(194, "utf8", "utf8_latvian_ci", false, 1, PadNone),
        collation_row(195, "utf8", "utf8_romanian_ci", false, 1, PadNone),
        collation_row(196, "utf8", "utf8_slovenian_ci", false, 1, PadNone),
        collation_row(197, "utf8", "utf8_polish_ci", false, 1, PadNone),
        collation_row(198, "utf8", "utf8_estonian_ci", false, 1, PadNone),
        collation_row(199, "utf8", "utf8_spanish_ci", false, 1, PadNone),
        collation_row(200, "utf8", "utf8_swedish_ci", false, 1, PadNone),
        collation_row(201, "utf8", "utf8_turkish_ci", false, 1, PadNone),
        collation_row(202, "utf8", "utf8_czech_ci", false, 1, PadNone),
        collation_row(203, "utf8", "utf8_danish_ci", false, 1, PadNone),
        collation_row(204, "utf8", "utf8_lithuanian_ci", false, 1, PadNone),
        collation_row(205, "utf8", "utf8_slovak_ci", false, 1, PadNone),
        collation_row(206, "utf8", "utf8_spanish2_ci", false, 1, PadNone),
        collation_row(207, "utf8", "utf8_roman_ci", false, 1, PadNone),
        collation_row(208, "utf8", "utf8_persian_ci", false, 1, PadNone),
        collation_row(209, "utf8", "utf8_esperanto_ci", false, 1, PadNone),
        collation_row(210, "utf8", "utf8_hungarian_ci", false, 1, PadNone),
        collation_row(211, "utf8", "utf8_sinhala_ci", false, 1, PadNone),
        collation_row(212, "utf8", "utf8_german2_ci", false, 1, PadNone),
        collation_row(213, "utf8", "utf8_croatian_ci", false, 1, PadNone),
        collation_row(214, "utf8", "utf8_unicode_520_ci", false, 1, PadNone),
        collation_row(215, "utf8", "utf8_vietnamese_ci", false, 1, PadNone),
        collation_row(223, "utf8", "utf8_general_mysql500_ci", false, 1, PadNone),
        collation_row(224, "utf8mb4", "utf8mb4_unicode_ci", false, 8, PadSpace),
        collation_row(225, "utf8mb4", "utf8mb4_icelandic_ci", false, 1, PadSpace),
        collation_row(226, "utf8mb4", "utf8mb4_latvian_ci", false, 1, PadSpace),
        collation_row(227, "utf8mb4", "utf8mb4_romanian_ci", false, 1, PadSpace),
        collation_row(228, "utf8mb4", "utf8mb4_slovenian_ci", false, 1, PadSpace),
        collation_row(229, "utf8mb4", "utf8mb4_polish_ci", false, 1, PadSpace),
        collation_row(230, "utf8mb4", "utf8mb4_estonian_ci", false, 1, PadSpace),
        collation_row(231, "utf8mb4", "utf8mb4_spanish_ci", false, 1, PadSpace),
        collation_row(232, "utf8mb4", "utf8mb4_swedish_ci", false, 1, PadSpace),
        collation_row(233, "utf8mb4", "utf8mb4_turkish_ci", false, 1, PadSpace),
        collation_row(234, "utf8mb4", "utf8mb4_czech_ci", false, 1, PadSpace),
        collation_row(235, "utf8mb4", "utf8mb4_danish_ci", false, 1, PadSpace),
        collation_row(236, "utf8mb4", "utf8mb4_lithuanian_ci", false, 1, PadSpace),
        collation_row(237, "utf8mb4", "utf8mb4_slovak_ci", false, 1, PadSpace),
        collation_row(238, "utf8mb4", "utf8mb4_spanish2_ci", false, 1, PadSpace),
        collation_row(239, "utf8mb4", "utf8mb4_roman_ci", false, 1, PadSpace),
        collation_row(240, "utf8mb4", "utf8mb4_persian_ci", false, 1, PadSpace),
        collation_row(241, "utf8mb4", "utf8mb4_esperanto_ci", false, 1, PadSpace),
        collation_row(242, "utf8mb4", "utf8mb4_hungarian_ci", false, 1, PadSpace),
        collation_row(243, "utf8mb4", "utf8mb4_sinhala_ci", false, 1, PadSpace),
        collation_row(244, "utf8mb4", "utf8mb4_german2_ci", false, 1, PadSpace),
        collation_row(245, "utf8mb4", "utf8mb4_croatian_ci", false, 1, PadSpace),
        collation_row(246, "utf8mb4", "utf8mb4_unicode_520_ci", false, 1, PadSpace),
        collation_row(247, "utf8mb4", "utf8mb4_vietnamese_ci", false, 1, PadSpace),
        collation_row(248, "gb18030", "gb18030_chinese_ci", false, 1, PadSpace),
        collation_row(249, "gb18030", "gb18030_bin", true, 1, PadSpace),
        collation_row(250, "gb18030", "gb18030_unicode_520_ci", false, 1, PadSpace),
        collation_row(255, "utf8mb4", "utf8mb4_0900_ai_ci", false, 0, PadNone),
        collation_row(256, "utf8mb4", "utf8mb4_de_pb_0900_ai_ci", false, 1, PadNone),
        collation_row(257, "utf8mb4", "utf8mb4_is_0900_ai_ci", false, 1, PadNone),
        collation_row(258, "utf8mb4", "utf8mb4_lv_0900_ai_ci", false, 1, PadNone),
        collation_row(259, "utf8mb4", "utf8mb4_ro_0900_ai_ci", false, 1, PadNone),
        collation_row(260, "utf8mb4", "utf8mb4_sl_0900_ai_ci", false, 1, PadNone),
        collation_row(261, "utf8mb4", "utf8mb4_pl_0900_ai_ci", false, 1, PadNone),
        collation_row(262, "utf8mb4", "utf8mb4_et_0900_ai_ci", false, 1, PadNone),
        collation_row(263, "utf8mb4", "utf8mb4_es_0900_ai_ci", false, 1, PadNone),
        collation_row(264, "utf8mb4", "utf8mb4_sv_0900_ai_ci", false, 1, PadNone),
        collation_row(265, "utf8mb4", "utf8mb4_tr_0900_ai_ci", false, 1, PadNone),
        collation_row(266, "utf8mb4", "utf8mb4_cs_0900_ai_ci", false, 1, PadNone),
        collation_row(267, "utf8mb4", "utf8mb4_da_0900_ai_ci", false, 1, PadNone),
        collation_row(268, "utf8mb4", "utf8mb4_lt_0900_ai_ci", false, 1, PadNone),
        collation_row(269, "utf8mb4", "utf8mb4_sk_0900_ai_ci", false, 1, PadNone),
        collation_row(270, "utf8mb4", "utf8mb4_es_trad_0900_ai_ci", false, 1, PadNone),
        collation_row(271, "utf8mb4", "utf8mb4_la_0900_ai_ci", false, 1, PadNone),
        collation_row(273, "utf8mb4", "utf8mb4_eo_0900_ai_ci", false, 1, PadNone),
        collation_row(274, "utf8mb4", "utf8mb4_hu_0900_ai_ci", false, 1, PadNone),
        collation_row(275, "utf8mb4", "utf8mb4_hr_0900_ai_ci", false, 1, PadNone),
        collation_row(277, "utf8mb4", "utf8mb4_vi_0900_ai_ci", false, 1, PadNone),
        collation_row(278, "utf8mb4", "utf8mb4_0900_as_cs", false, 1, PadNone),
        collation_row(279, "utf8mb4", "utf8mb4_de_pb_0900_as_cs", false, 1, PadNone),
        collation_row(280, "utf8mb4", "utf8mb4_is_0900_as_cs", false, 1, PadNone),
        collation_row(281, "utf8mb4", "utf8mb4_lv_0900_as_cs", false, 1, PadNone),
        collation_row(282, "utf8mb4", "utf8mb4_ro_0900_as_cs", false, 1, PadNone),
        collation_row(283, "utf8mb4", "utf8mb4_sl_0900_as_cs", false, 1, PadNone),
        collation_row(284, "utf8mb4", "utf8mb4_pl_0900_as_cs", false, 1, PadNone),
        collation_row(285, "utf8mb4", "utf8mb4_et_0900_as_cs", false, 1, PadNone),
        collation_row(286, "utf8mb4", "utf8mb4_es_0900_as_cs", false, 1, PadNone),
        collation_row(287, "utf8mb4", "utf8mb4_sv_0900_as_cs", false, 1, PadNone),
        collation_row(288, "utf8mb4", "utf8mb4_tr_0900_as_cs", false, 1, PadNone),
        collation_row(289, "utf8mb4", "utf8mb4_cs_0900_as_cs", false, 1, PadNone),
        collation_row(290, "utf8mb4", "utf8mb4_da_0900_as_cs", false, 1, PadNone),
        collation_row(291, "utf8mb4", "utf8mb4_lt_0900_as_cs", false, 1, PadNone),
        collation_row(292, "utf8mb4", "utf8mb4_sk_0900_as_cs", false, 1, PadNone),
        collation_row(293, "utf8mb4", "utf8mb4_es_trad_0900_as_cs", false, 1, PadNone),
        collation_row(294, "utf8mb4", "utf8mb4_la_0900_as_cs", false, 1, PadNone),
        collation_row(296, "utf8mb4", "utf8mb4_eo_0900_as_cs", false, 1, PadNone),
        collation_row(297, "utf8mb4", "utf8mb4_hu_0900_as_cs", false, 1, PadNone),
        collation_row(298, "utf8mb4", "utf8mb4_hr_0900_as_cs", false, 1, PadNone),
        collation_row(300, "utf8mb4", "utf8mb4_vi_0900_as_cs", false, 1, PadNone),
        collation_row(303, "utf8mb4", "utf8mb4_ja_0900_as_cs", false, 1, PadNone),
        collation_row(304, "utf8mb4", "utf8mb4_ja_0900_as_cs_ks", false, 1, PadNone),
        collation_row(305, "utf8mb4", "utf8mb4_0900_as_ci", false, 1, PadNone),
        collation_row(306, "utf8mb4", "utf8mb4_ru_0900_ai_ci", false, 1, PadNone),
        collation_row(307, "utf8mb4", "utf8mb4_ru_0900_as_cs", false, 1, PadNone),
        collation_row(308, "utf8mb4", "utf8mb4_zh_0900_as_cs", false, 1, PadNone),
        collation_row(309, "utf8mb4", "utf8mb4_0900_bin", false, 1, PadNone),
        collation_row(2048, "utf8mb4", "utf8mb4_zh_pinyin_tidb_as_cs", false, 1, PadNone),
    ]
});

#[cfg(test)]
pub(crate) fn all_collations_for_test() -> &'static [Collation] {
    &collations
}

/// AddCharset 注册一个自定义的受支持字符集。
pub fn AddCharset(charset: Charset) {
    ensure_initialized();
    CharacterSetInfos.write().unwrap().insert(charset.Name.clone(), charset);
}

/// RemoveCharset 删除自定义字符集，并保留 Go 按排序规则 Name 比较输入字符串的行为。
pub fn RemoveCharset(name: &str) {
    ensure_initialized();
    CharacterSetInfos.write().unwrap().remove(name);
    supportedCollations.write().unwrap().retain(|collation| collation.Name != name);
}

/// AddCollation 同时登记 ID、名称及所属字符集；受支持白名单中的项还进入公开列表。
pub fn AddCollation(collation: Collation) {
    ensure_initialized();
    add_collation(collation);
}

/// 内部登记：写入 ID/名称索引，并挂到所属字符集；白名单项进入受支持列表。
fn add_collation(collation: Collation) {
    collationsIDMap.write().unwrap().insert(collation.ID, collation.clone());
    collationsNameMap.write().unwrap().insert(collation.Name.clone(), collation.clone());
    if supportedCollationNames.contains(collation.Name.as_str()) {
        supportedCollations.write().unwrap().push(collation.clone());
    }

    // Go 的指针 map 会原地更新对象；这里分别写入两张注册表以保留可见结果。
    if let Some(charset) = CharacterSetInfos.write().unwrap().get_mut(&collation.CharsetName) {
        charset.Collations.insert(collation.Name.clone(), collation.clone());
    }
    if let Some(charset) = charsets.write().unwrap().get_mut(&collation.CharsetName) {
        charset.Collations.insert(collation.Name.clone(), collation);
    }
}

/// AddSupportedCollation 追加一个自定义受支持排序规则。
pub fn AddSupportedCollation(collation: Collation) {
    ensure_initialized();
    supportedCollations.write().unwrap().push(collation);
}

/// init 必须保持在文件末尾；它按静态表顺序构建所有查询索引。
pub fn init() {
    ensure_initialized();
}
