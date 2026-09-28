// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// MySQL 字符集与排序规则（collation）常量表及查表工具。
//
// 提供字符集名→默认 collation ID、collation 双向查找，以及
// `IsRangeGraph`（判定 Unicode 图形类字符可否用于标识符）。

// 本文件由 pkg/parser/mysql/charset.go 迁移而来，保留字符集、排序规则 ID 表及 Unicode 名称范围。
// 执行内存查表和字符分类。
use unicode_general_category::{GeneralCategory, get_general_category};

// CharsetNameToID 对常用字符集走与 Go 一致的快速路径，其余名称查完整默认排序规则表。
/// 字符集名到默认 collation ID；未知返回 0。
pub fn CharsetNameToID(charset: &str) -> u8 {
    match charset {
        "utf8mb4" => UTF8MB4DefaultCollationID,
        "binary" => BinaryDefaultCollationID,
        "utf8" => UTF8DefaultCollationID,
        "ascii" => ASCIIDefaultCollationID,
        "latin1" => Latin1DefaultCollationID,
        _ => CharsetIDs
            .iter()
            .find_map(|(name, id)| (*name == charset).then_some(*id))
            .unwrap_or(0),
    }
}

// CharsetIDs 保存字符集名称到其默认 collation ID 的完整映射。
/// 字符集名称到默认 collation ID 的完整表。
pub static CharsetIDs: &[(&str, u8)] = &[
    ("big5", 1),
    ("dec8", 3),
    ("cp850", 4),
    ("hp8", 6),
    ("koi8r", 7),
    ("latin1", Latin1DefaultCollationID),
    ("latin2", 9),
    ("swe7", 10),
    ("ascii", ASCIIDefaultCollationID),
    ("ujis", 12),
    ("sjis", 13),
    ("hebrew", 16),
    ("tis620", 18),
    ("euckr", 19),
    ("koi8u", 22),
    ("gb2312", 24),
    ("greek", 25),
    ("cp1250", 26),
    ("gbk", 28),
    ("latin5", 30),
    ("armscii8", 32),
    ("utf8", UTF8DefaultCollationID),
    ("ucs2", 35),
    ("cp866", 36),
    ("keybcs2", 37),
    ("macce", 38),
    ("macroman", 39),
    ("cp852", 40),
    ("latin7", 41),
    ("utf8mb4", UTF8MB4DefaultCollationID),
    ("cp1251", 51),
    ("utf16", 54),
    ("utf16le", 56),
    ("cp1256", 57),
    ("cp1257", 59),
    ("utf32", 60),
    ("binary", BinaryDefaultCollationID),
    ("geostd8", 92),
    ("cp932", 95),
    ("eucjpms", 97),
    ("gb18030", GB18030DefaultCollationID),
];
// Collations 按 MySQL 数字 ID 保存名称；保留不连续编号和 309 等扩展编号。
/// collation ID → 名称（保留不连续编号）。
pub static Collations: &[(u16, &str)] = &[
    (1, "big5_chinese_ci"),
    (2, "latin2_czech_cs"),
    (3, "dec8_swedish_ci"),
    (4, "cp850_general_ci"),
    (5, "latin1_german1_ci"),
    (6, "hp8_english_ci"),
    (7, "koi8r_general_ci"),
    (8, "latin1_swedish_ci"),
    (9, "latin2_general_ci"),
    (10, "swe7_swedish_ci"),
    (11, "ascii_general_ci"),
    (12, "ujis_japanese_ci"),
    (13, "sjis_japanese_ci"),
    (14, "cp1251_bulgarian_ci"),
    (15, "latin1_danish_ci"),
    (16, "hebrew_general_ci"),
    (18, "tis620_thai_ci"),
    (19, "euckr_korean_ci"),
    (20, "latin7_estonian_cs"),
    (21, "latin2_hungarian_ci"),
    (22, "koi8u_general_ci"),
    (23, "cp1251_ukrainian_ci"),
    (24, "gb2312_chinese_ci"),
    (25, "greek_general_ci"),
    (26, "cp1250_general_ci"),
    (27, "latin2_croatian_ci"),
    (28, "gbk_chinese_ci"),
    (29, "cp1257_lithuanian_ci"),
    (30, "latin5_turkish_ci"),
    (31, "latin1_german2_ci"),
    (32, "armscii8_general_ci"),
    (33, "utf8_general_ci"),
    (34, "cp1250_czech_cs"),
    (35, "ucs2_general_ci"),
    (36, "cp866_general_ci"),
    (37, "keybcs2_general_ci"),
    (38, "macce_general_ci"),
    (39, "macroman_general_ci"),
    (40, "cp852_general_ci"),
    (41, "latin7_general_ci"),
    (42, "latin7_general_cs"),
    (43, "macce_bin"),
    (44, "cp1250_croatian_ci"),
    (45, "utf8mb4_general_ci"),
    (46, "utf8mb4_bin"),
    (47, "latin1_bin"),
    (48, "latin1_general_ci"),
    (49, "latin1_general_cs"),
    (50, "cp1251_bin"),
    (51, "cp1251_general_ci"),
    (52, "cp1251_general_cs"),
    (53, "macroman_bin"),
    (54, "utf16_general_ci"),
    (55, "utf16_bin"),
    (56, "utf16le_general_ci"),
    (57, "cp1256_general_ci"),
    (58, "cp1257_bin"),
    (59, "cp1257_general_ci"),
    (60, "utf32_general_ci"),
    (61, "utf32_bin"),
    (62, "utf16le_bin"),
    (63, "binary"),
    (64, "armscii8_bin"),
    (65, "ascii_bin"),
    (66, "cp1250_bin"),
    (67, "cp1256_bin"),
    (68, "cp866_bin"),
    (69, "dec8_bin"),
    (70, "greek_bin"),
    (71, "hebrew_bin"),
    (72, "hp8_bin"),
    (73, "keybcs2_bin"),
    (74, "koi8r_bin"),
    (75, "koi8u_bin"),
    (77, "latin2_bin"),
    (78, "latin5_bin"),
    (79, "latin7_bin"),
    (80, "cp850_bin"),
    (81, "cp852_bin"),
    (82, "swe7_bin"),
    (83, "utf8_bin"),
    (84, "big5_bin"),
    (85, "euckr_bin"),
    (86, "gb2312_bin"),
    (87, "gbk_bin"),
    (88, "sjis_bin"),
    (89, "tis620_bin"),
    (90, "ucs2_bin"),
    (91, "ujis_bin"),
    (92, "geostd8_general_ci"),
    (93, "geostd8_bin"),
    (94, "latin1_spanish_ci"),
    (95, "cp932_japanese_ci"),
    (96, "cp932_bin"),
    (97, "eucjpms_japanese_ci"),
    (98, "eucjpms_bin"),
    (99, "cp1250_polish_ci"),
    (101, "utf16_unicode_ci"),
    (102, "utf16_icelandic_ci"),
    (103, "utf16_latvian_ci"),
    (104, "utf16_romanian_ci"),
    (105, "utf16_slovenian_ci"),
    (106, "utf16_polish_ci"),
    (107, "utf16_estonian_ci"),
    (108, "utf16_spanish_ci"),
    (109, "utf16_swedish_ci"),
    (110, "utf16_turkish_ci"),
    (111, "utf16_czech_ci"),
    (112, "utf16_danish_ci"),
    (113, "utf16_lithuanian_ci"),
    (114, "utf16_slovak_ci"),
    (115, "utf16_spanish2_ci"),
    (116, "utf16_roman_ci"),
    (117, "utf16_persian_ci"),
    (118, "utf16_esperanto_ci"),
    (119, "utf16_hungarian_ci"),
    (120, "utf16_sinhala_ci"),
    (121, "utf16_german2_ci"),
    (122, "utf16_croatian_ci"),
    (123, "utf16_unicode_520_ci"),
    (124, "utf16_vietnamese_ci"),
    (128, "ucs2_unicode_ci"),
    (129, "ucs2_icelandic_ci"),
    (130, "ucs2_latvian_ci"),
    (131, "ucs2_romanian_ci"),
    (132, "ucs2_slovenian_ci"),
    (133, "ucs2_polish_ci"),
    (134, "ucs2_estonian_ci"),
    (135, "ucs2_spanish_ci"),
    (136, "ucs2_swedish_ci"),
    (137, "ucs2_turkish_ci"),
    (138, "ucs2_czech_ci"),
    (139, "ucs2_danish_ci"),
    (140, "ucs2_lithuanian_ci"),
    (141, "ucs2_slovak_ci"),
    (142, "ucs2_spanish2_ci"),
    (143, "ucs2_roman_ci"),
    (144, "ucs2_persian_ci"),
    (145, "ucs2_esperanto_ci"),
    (146, "ucs2_hungarian_ci"),
    (147, "ucs2_sinhala_ci"),
    (148, "ucs2_german2_ci"),
    (149, "ucs2_croatian_ci"),
    (150, "ucs2_unicode_520_ci"),
    (151, "ucs2_vietnamese_ci"),
    (159, "ucs2_general_mysql500_ci"),
    (160, "utf32_unicode_ci"),
    (161, "utf32_icelandic_ci"),
    (162, "utf32_latvian_ci"),
    (163, "utf32_romanian_ci"),
    (164, "utf32_slovenian_ci"),
    (165, "utf32_polish_ci"),
    (166, "utf32_estonian_ci"),
    (167, "utf32_spanish_ci"),
    (168, "utf32_swedish_ci"),
    (169, "utf32_turkish_ci"),
    (170, "utf32_czech_ci"),
    (171, "utf32_danish_ci"),
    (172, "utf32_lithuanian_ci"),
    (173, "utf32_slovak_ci"),
    (174, "utf32_spanish2_ci"),
    (175, "utf32_roman_ci"),
    (176, "utf32_persian_ci"),
    (177, "utf32_esperanto_ci"),
    (178, "utf32_hungarian_ci"),
    (179, "utf32_sinhala_ci"),
    (180, "utf32_german2_ci"),
    (181, "utf32_croatian_ci"),
    (182, "utf32_unicode_520_ci"),
    (183, "utf32_vietnamese_ci"),
    (192, "utf8_unicode_ci"),
    (193, "utf8_icelandic_ci"),
    (194, "utf8_latvian_ci"),
    (195, "utf8_romanian_ci"),
    (196, "utf8_slovenian_ci"),
    (197, "utf8_polish_ci"),
    (198, "utf8_estonian_ci"),
    (199, "utf8_spanish_ci"),
    (200, "utf8_swedish_ci"),
    (201, "utf8_turkish_ci"),
    (202, "utf8_czech_ci"),
    (203, "utf8_danish_ci"),
    (204, "utf8_lithuanian_ci"),
    (205, "utf8_slovak_ci"),
    (206, "utf8_spanish2_ci"),
    (207, "utf8_roman_ci"),
    (208, "utf8_persian_ci"),
    (209, "utf8_esperanto_ci"),
    (210, "utf8_hungarian_ci"),
    (211, "utf8_sinhala_ci"),
    (212, "utf8_german2_ci"),
    (213, "utf8_croatian_ci"),
    (214, "utf8_unicode_520_ci"),
    (215, "utf8_vietnamese_ci"),
    (223, "utf8_general_mysql500_ci"),
    (224, "utf8mb4_unicode_ci"),
    (225, "utf8mb4_icelandic_ci"),
    (226, "utf8mb4_latvian_ci"),
    (227, "utf8mb4_romanian_ci"),
    (228, "utf8mb4_slovenian_ci"),
    (229, "utf8mb4_polish_ci"),
    (230, "utf8mb4_estonian_ci"),
    (231, "utf8mb4_spanish_ci"),
    (232, "utf8mb4_swedish_ci"),
    (233, "utf8mb4_turkish_ci"),
    (234, "utf8mb4_czech_ci"),
    (235, "utf8mb4_danish_ci"),
    (236, "utf8mb4_lithuanian_ci"),
    (237, "utf8mb4_slovak_ci"),
    (238, "utf8mb4_spanish2_ci"),
    (239, "utf8mb4_roman_ci"),
    (240, "utf8mb4_persian_ci"),
    (241, "utf8mb4_esperanto_ci"),
    (242, "utf8mb4_hungarian_ci"),
    (243, "utf8mb4_sinhala_ci"),
    (244, "utf8mb4_german2_ci"),
    (245, "utf8mb4_croatian_ci"),
    (246, "utf8mb4_unicode_520_ci"),
    (247, "utf8mb4_vietnamese_ci"),
    (248, "gb18030_chinese_ci"),
    (249, "gb18030_bin"),
    (255, "utf8mb4_0900_ai_ci"),
    (309, "utf8mb4_0900_bin"),
];

// CollationNames 是 Collations 的名称到 ID 反向表，按 源顺序完整保留。
/// collation 名称 → ID 反向表。
pub static CollationNames: &[(&str, u16)] = &[
    ("big5_chinese_ci", 1),
    ("latin2_czech_cs", 2),
    ("dec8_swedish_ci", 3),
    ("cp850_general_ci", 4),
    ("latin1_german1_ci", 5),
    ("hp8_english_ci", 6),
    ("koi8r_general_ci", 7),
    ("latin1_swedish_ci", 8),
    ("latin2_general_ci", 9),
    ("swe7_swedish_ci", 10),
    ("ascii_general_ci", 11),
    ("ujis_japanese_ci", 12),
    ("sjis_japanese_ci", 13),
    ("cp1251_bulgarian_ci", 14),
    ("latin1_danish_ci", 15),
    ("hebrew_general_ci", 16),
    ("tis620_thai_ci", 18),
    ("euckr_korean_ci", 19),
    ("latin7_estonian_cs", 20),
    ("latin2_hungarian_ci", 21),
    ("koi8u_general_ci", 22),
    ("cp1251_ukrainian_ci", 23),
    ("gb2312_chinese_ci", 24),
    ("greek_general_ci", 25),
    ("cp1250_general_ci", 26),
    ("latin2_croatian_ci", 27),
    ("gbk_chinese_ci", 28),
    ("cp1257_lithuanian_ci", 29),
    ("latin5_turkish_ci", 30),
    ("latin1_german2_ci", 31),
    ("armscii8_general_ci", 32),
    ("utf8_general_ci", 33),
    ("cp1250_czech_cs", 34),
    ("ucs2_general_ci", 35),
    ("cp866_general_ci", 36),
    ("keybcs2_general_ci", 37),
    ("macce_general_ci", 38),
    ("macroman_general_ci", 39),
    ("cp852_general_ci", 40),
    ("latin7_general_ci", 41),
    ("latin7_general_cs", 42),
    ("macce_bin", 43),
    ("cp1250_croatian_ci", 44),
    ("utf8mb4_general_ci", 45),
    ("utf8mb4_bin", 46),
    ("latin1_bin", 47),
    ("latin1_general_ci", 48),
    ("latin1_general_cs", 49),
    ("cp1251_bin", 50),
    ("cp1251_general_ci", 51),
    ("cp1251_general_cs", 52),
    ("macroman_bin", 53),
    ("utf16_general_ci", 54),
    ("utf16_bin", 55),
    ("utf16le_general_ci", 56),
    ("cp1256_general_ci", 57),
    ("cp1257_bin", 58),
    ("cp1257_general_ci", 59),
    ("utf32_general_ci", 60),
    ("utf32_bin", 61),
    ("utf16le_bin", 62),
    ("binary", 63),
    ("armscii8_bin", 64),
    ("ascii_bin", 65),
    ("cp1250_bin", 66),
    ("cp1256_bin", 67),
    ("cp866_bin", 68),
    ("dec8_bin", 69),
    ("greek_bin", 70),
    ("hebrew_bin", 71),
    ("hp8_bin", 72),
    ("keybcs2_bin", 73),
    ("koi8r_bin", 74),
    ("koi8u_bin", 75),
    ("latin2_bin", 77),
    ("latin5_bin", 78),
    ("latin7_bin", 79),
    ("cp850_bin", 80),
    ("cp852_bin", 81),
    ("swe7_bin", 82),
    ("utf8_bin", 83),
    ("big5_bin", 84),
    ("euckr_bin", 85),
    ("gb2312_bin", 86),
    ("gbk_bin", 87),
    ("sjis_bin", 88),
    ("tis620_bin", 89),
    ("ucs2_bin", 90),
    ("ujis_bin", 91),
    ("geostd8_general_ci", 92),
    ("geostd8_bin", 93),
    ("latin1_spanish_ci", 94),
    ("cp932_japanese_ci", 95),
    ("cp932_bin", 96),
    ("eucjpms_japanese_ci", 97),
    ("eucjpms_bin", 98),
    ("cp1250_polish_ci", 99),
    ("utf16_unicode_ci", 101),
    ("utf16_icelandic_ci", 102),
    ("utf16_latvian_ci", 103),
    ("utf16_romanian_ci", 104),
    ("utf16_slovenian_ci", 105),
    ("utf16_polish_ci", 106),
    ("utf16_estonian_ci", 107),
    ("utf16_spanish_ci", 108),
    ("utf16_swedish_ci", 109),
    ("utf16_turkish_ci", 110),
    ("utf16_czech_ci", 111),
    ("utf16_danish_ci", 112),
    ("utf16_lithuanian_ci", 113),
    ("utf16_slovak_ci", 114),
    ("utf16_spanish2_ci", 115),
    ("utf16_roman_ci", 116),
    ("utf16_persian_ci", 117),
    ("utf16_esperanto_ci", 118),
    ("utf16_hungarian_ci", 119),
    ("utf16_sinhala_ci", 120),
    ("utf16_german2_ci", 121),
    ("utf16_croatian_ci", 122),
    ("utf16_unicode_520_ci", 123),
    ("utf16_vietnamese_ci", 124),
    ("ucs2_unicode_ci", 128),
    ("ucs2_icelandic_ci", 129),
    ("ucs2_latvian_ci", 130),
    ("ucs2_romanian_ci", 131),
    ("ucs2_slovenian_ci", 132),
    ("ucs2_polish_ci", 133),
    ("ucs2_estonian_ci", 134),
    ("ucs2_spanish_ci", 135),
    ("ucs2_swedish_ci", 136),
    ("ucs2_turkish_ci", 137),
    ("ucs2_czech_ci", 138),
    ("ucs2_danish_ci", 139),
    ("ucs2_lithuanian_ci", 140),
    ("ucs2_slovak_ci", 141),
    ("ucs2_spanish2_ci", 142),
    ("ucs2_roman_ci", 143),
    ("ucs2_persian_ci", 144),
    ("ucs2_esperanto_ci", 145),
    ("ucs2_hungarian_ci", 146),
    ("ucs2_sinhala_ci", 147),
    ("ucs2_german2_ci", 148),
    ("ucs2_croatian_ci", 149),
    ("ucs2_unicode_520_ci", 150),
    ("ucs2_vietnamese_ci", 151),
    ("ucs2_general_mysql500_ci", 159),
    ("utf32_unicode_ci", 160),
    ("utf32_icelandic_ci", 161),
    ("utf32_latvian_ci", 162),
    ("utf32_romanian_ci", 163),
    ("utf32_slovenian_ci", 164),
    ("utf32_polish_ci", 165),
    ("utf32_estonian_ci", 166),
    ("utf32_spanish_ci", 167),
    ("utf32_swedish_ci", 168),
    ("utf32_turkish_ci", 169),
    ("utf32_czech_ci", 170),
    ("utf32_danish_ci", 171),
    ("utf32_lithuanian_ci", 172),
    ("utf32_slovak_ci", 173),
    ("utf32_spanish2_ci", 174),
    ("utf32_roman_ci", 175),
    ("utf32_persian_ci", 176),
    ("utf32_esperanto_ci", 177),
    ("utf32_hungarian_ci", 178),
    ("utf32_sinhala_ci", 179),
    ("utf32_german2_ci", 180),
    ("utf32_croatian_ci", 181),
    ("utf32_unicode_520_ci", 182),
    ("utf32_vietnamese_ci", 183),
    ("utf8_unicode_ci", 192),
    ("utf8_icelandic_ci", 193),
    ("utf8_latvian_ci", 194),
    ("utf8_romanian_ci", 195),
    ("utf8_slovenian_ci", 196),
    ("utf8_polish_ci", 197),
    ("utf8_estonian_ci", 198),
    ("utf8_spanish_ci", 199),
    ("utf8_swedish_ci", 200),
    ("utf8_turkish_ci", 201),
    ("utf8_czech_ci", 202),
    ("utf8_danish_ci", 203),
    ("utf8_lithuanian_ci", 204),
    ("utf8_slovak_ci", 205),
    ("utf8_spanish2_ci", 206),
    ("utf8_roman_ci", 207),
    ("utf8_persian_ci", 208),
    ("utf8_esperanto_ci", 209),
    ("utf8_hungarian_ci", 210),
    ("utf8_sinhala_ci", 211),
    ("utf8_german2_ci", 212),
    ("utf8_croatian_ci", 213),
    ("utf8_unicode_520_ci", 214),
    ("utf8_vietnamese_ci", 215),
    ("utf8_general_mysql500_ci", 223),
    ("utf8mb4_unicode_ci", 224),
    ("utf8mb4_icelandic_ci", 225),
    ("utf8mb4_latvian_ci", 226),
    ("utf8mb4_romanian_ci", 227),
    ("utf8mb4_slovenian_ci", 228),
    ("utf8mb4_polish_ci", 229),
    ("utf8mb4_estonian_ci", 230),
    ("utf8mb4_spanish_ci", 231),
    ("utf8mb4_swedish_ci", 232),
    ("utf8mb4_turkish_ci", 233),
    ("utf8mb4_czech_ci", 234),
    ("utf8mb4_danish_ci", 235),
    ("utf8mb4_lithuanian_ci", 236),
    ("utf8mb4_slovak_ci", 237),
    ("utf8mb4_spanish2_ci", 238),
    ("utf8mb4_roman_ci", 239),
    ("utf8mb4_persian_ci", 240),
    ("utf8mb4_esperanto_ci", 241),
    ("utf8mb4_hungarian_ci", 242),
    ("utf8mb4_sinhala_ci", 243),
    ("utf8mb4_german2_ci", 244),
    ("utf8mb4_croatian_ci", 245),
    ("utf8mb4_unicode_520_ci", 246),
    ("utf8mb4_vietnamese_ci", 247),
    ("gb18030_chinese_ci", 248),
    ("gb18030_bin", 249),
    ("utf8mb4_0900_ai_ci", 255),
    ("utf8mb4_0900_bin", 309),
];

// GetCollationNameByID returns the collation name registered for id.
/// 按 ID 查 collation 名称。
pub fn GetCollationNameByID(id: u16) -> Option<&'static str> {
    Collations
        .iter()
        .find_map(|(collation_id, name)| (*collation_id == id).then_some(*name))
}

// GetCollationIDByName returns the collation ID registered for name.
/// 按名称查 collation ID。
pub fn GetCollationIDByName(name: &str) -> Option<u16> {
    CollationNames
        .iter()
        .find_map(|(collation_name, id)| (*collation_name == name).then_some(*id))
}

// MySQL 字符集与默认 collation 常量对应 Go const 块。
/// utf8 字符集名常量。
pub const UTF8Charset: &str = "utf8";
/// utf8mb4 字符集名常量。
pub const UTF8MB4Charset: &str = "utf8mb4";
/// 默认字符集（utf8mb4）。
pub const DefaultCharset: &str = UTF8MB4Charset;
// DefaultCollationID 对应 utf8mb4_bin(46)。
/// 默认 collation ID（utf8mb4_bin）。
pub const DefaultCollationID: u16 = 46;
/// latin1 默认 collation ID。
pub const Latin1DefaultCollationID: u8 = 47;
/// ascii 默认 collation ID。
pub const ASCIIDefaultCollationID: u8 = 65;
/// utf8 默认 collation ID。
pub const UTF8DefaultCollationID: u8 = 83;
/// utf8mb4 默认 collation ID。
pub const UTF8MB4DefaultCollationID: u8 = 46;
/// binary 默认 collation ID。
pub const BinaryDefaultCollationID: u8 = 63;
/// gb18030 默认 collation ID。
pub const GB18030DefaultCollationID: u8 = 248;
/// utf8mb4 默认 collation 名称。
pub const UTF8MB4DefaultCollation: &str = "utf8mb4_bin";
/// 会话默认 collation 名称。
pub const DefaultCollationName: &str = UTF8MB4DefaultCollation;
/// utf8mb4_general_ci 名称。
pub const UTF8MB4GeneralCICollation: &str = "utf8mb4_general_ci";
// RFC 3629 规定 U+0000..U+10FFFF 的 UTF-8 编码最多占四个字节。
/// 单字符最大 UTF-8 字节数（RFC 3629）。
pub const MaxBytesOfCharacter: usize = 4;

// IsUTF8Charset 仅接受 utf8 与 utf8mb4，大小写语义与 Go 字符串比较一致。
/// 是否为 utf8 / utf8mb4。
pub fn IsUTF8Charset(charset: &str) -> bool {
    charset == UTF8Charset || charset == UTF8MB4Charset
}

// IsRangeGraph 对应 Go RangeGraph 中列出的 Unicode RangeTable：所有字母、
// 标记、数字、标点和符号类别均可用于 MySQL 标识符，分隔符与控制类不可用。
/// 是否属于可用于标识符的 Unicode 图形类别。
pub fn IsRangeGraph(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
            | GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
            | GeneralCategory::DecimalNumber
            | GeneralCategory::LetterNumber
            | GeneralCategory::OtherNumber
            | GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
            | GeneralCategory::MathSymbol
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::ModifierSymbol
            | GeneralCategory::OtherSymbol
    )
}
