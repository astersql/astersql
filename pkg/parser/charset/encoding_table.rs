// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// HTML/WHATWG 编码标签到具体编解码器的查找表。
//
// 对应 Go `encoding_table.go`：按标签名查找 rust-encoding 编解码器，行为对齐 Go 的 x/text Encoding。
// 标签规范化会按 Go strings.ToLower 忽略 Unicode 大小写并裁剪 HTML 标准规定的首尾空白。

// The table returns concrete mature codecs from rust-encoding, matching Go's x/text Encoding value.

// LookupResult 保留 Go 匿名结构体中的编码对象和规范名称。
/// 编码查找结果：具体编解码器与规范名称。
#[derive(Clone, Copy)]
pub struct LookupResult {
    /// rust-encoding 编解码器引用。
    pub encoding: ::encoding::types::EncodingRef,
    /// 规范化后的编码名称。
    pub name: &'static str,
}

// lookup 对应 Go Lookup：按 Unicode 小写规则忽略大小写，并裁掉 HTML 标准规定的首尾空白字符。
/// 按 HTML 标签名查找编码；忽略大小写并裁剪首尾空白。
pub fn lookup(label: &str) -> Option<LookupResult> {
    let normalized = label
        .trim_matches(['\t', '\n', '\r', '\u{000c}', ' '])
        .to_lowercase();
    lookup_normalized(&normalized)
}

/// Lookup keeps the exported Go API spelling while `lookup` remains available to Rust callers.
pub fn Lookup(label: &str) -> Option<LookupResult> {
    lookup(label)
}

// entry 是静态编码条目的构造辅助，保持每个分支只声明编码种类和规范名。
/// 构造静态编码条目。
const fn entry(encoding: ::encoding::types::EncodingRef, name: &'static str) -> LookupResult {
    LookupResult { encoding, name }
}

// lookup_normalized 对应 Go 私有 lookup 和 encodings map。
// 分组只压缩重复值，所有 Go 标签仍逐项列出；未命中时对应 Go 的 nil、空字符串零值。
/// 在已规范化标签上匹配 encodings 表；未命中返回 None。
fn lookup_normalized(label: &str) -> Option<LookupResult> {
    let result = match label {
        "unicode-1-1-utf-8" | "utf-8" | "utf8" | "utf8mb4" => {
            entry(::encoding::all::UTF_8, "utf-8")
        }
        "binary" => entry(::encoding::all::UTF_8, "binary"),
        "866" | "cp866" | "csibm866" | "ibm866" => entry(::encoding::all::IBM866, "ibm866"),
        "csisolatin2" | "iso-8859-2" | "iso-ir-101" | "iso8859-2" | "iso88592" | "iso_8859-2"
        | "iso_8859-2:1987" | "l2" | "latin2" => entry(::encoding::all::ISO_8859_2, "iso-8859-2"),
        "csisolatin3" | "iso-8859-3" | "iso-ir-109" | "iso8859-3" | "iso88593" | "iso_8859-3"
        | "iso_8859-3:1988" | "l3" | "latin3" => entry(::encoding::all::ISO_8859_3, "iso-8859-3"),
        "csisolatin4" | "iso-8859-4" | "iso-ir-110" | "iso8859-4" | "iso88594" | "iso_8859-4"
        | "iso_8859-4:1988" | "l4" | "latin4" => entry(::encoding::all::ISO_8859_4, "iso-8859-4"),
        "csisolatincyrillic" | "cyrillic" | "iso-8859-5" | "iso-ir-144" | "iso8859-5"
        | "iso88595" | "iso_8859-5" | "iso_8859-5:1988" => {
            entry(::encoding::all::ISO_8859_5, "iso-8859-5")
        }
        "arabic" | "asmo-708" | "csiso88596e" | "csiso88596i" | "csisolatinarabic" | "ecma-114"
        | "iso-8859-6" | "iso-8859-6-e" | "iso-8859-6-i" | "iso-ir-127" | "iso8859-6"
        | "iso88596" | "iso_8859-6" | "iso_8859-6:1987" => {
            entry(::encoding::all::ISO_8859_6, "iso-8859-6")
        }
        "csisolatingreek" | "ecma-118" | "elot_928" | "greek" | "greek8" | "iso-8859-7"
        | "iso-ir-126" | "iso8859-7" | "iso88597" | "iso_8859-7" | "iso_8859-7:1987"
        | "sun_eu_greek" => entry(::encoding::all::ISO_8859_7, "iso-8859-7"),
        "csiso88598e" | "csisolatinhebrew" | "hebrew" | "iso-8859-8" | "iso-8859-8-e"
        | "iso-ir-138" | "iso8859-8" | "iso88598" | "iso_8859-8" | "iso_8859-8:1988" | "visual" => {
            entry(::encoding::all::ISO_8859_8, "iso-8859-8")
        }
        "csiso88598i" | "iso-8859-8-i" | "logical" => {
            entry(::encoding::all::ISO_8859_8, "iso-8859-8-i")
        }
        "csisolatin6" | "iso-8859-10" | "iso-ir-157" | "iso8859-10" | "iso885910" | "l6"
        | "latin6" => entry(::encoding::all::ISO_8859_10, "iso-8859-10"),
        "iso-8859-13" | "iso8859-13" | "iso885913" => {
            entry(::encoding::all::ISO_8859_13, "iso-8859-13")
        }
        "iso-8859-14" | "iso8859-14" | "iso885914" => {
            entry(::encoding::all::ISO_8859_14, "iso-8859-14")
        }
        "csisolatin9" | "iso-8859-15" | "iso8859-15" | "iso885915" | "iso_8859-15" | "l9" => {
            entry(::encoding::all::ISO_8859_15, "iso-8859-15")
        }
        "iso-8859-16" => entry(::encoding::all::ISO_8859_16, "iso-8859-16"),
        "cskoi8r" | "koi" | "koi8" | "koi8-r" | "koi8_r" => {
            entry(::encoding::all::KOI8_R, "koi8-r")
        }
        "koi8-u" => entry(::encoding::all::KOI8_U, "koi8-u"),
        "csmacintosh" | "mac" | "macintosh" | "x-mac-roman" => {
            entry(::encoding::all::MAC_ROMAN, "macintosh")
        }
        "dos-874" | "iso-8859-11" | "iso8859-11" | "iso885911" | "tis-620" | "windows-874" => {
            entry(::encoding::all::WINDOWS_874, "windows-874")
        }
        "cp1250" | "windows-1250" | "x-cp1250" => {
            entry(::encoding::all::WINDOWS_1250, "windows-1250")
        }
        "cp1251" | "windows-1251" | "x-cp1251" => {
            entry(::encoding::all::WINDOWS_1251, "windows-1251")
        }
        "ansi_x3.4-1968" | "ascii" | "cp1252" | "cp819" | "csisolatin1" | "ibm819"
        | "iso-8859-1" | "iso-ir-100" | "iso8859-1" | "iso88591" | "iso_8859-1"
        | "iso_8859-1:1987" | "l1" | "latin1" | "us-ascii" | "windows-1252" | "x-cp1252" => {
            entry(::encoding::all::WINDOWS_1252, "windows-1252")
        }
        "cp1253" | "windows-1253" | "x-cp1253" => {
            entry(::encoding::all::WINDOWS_1253, "windows-1253")
        }
        "cp1254" | "csisolatin5" | "iso-8859-9" | "iso-ir-148" | "iso8859-9" | "iso88599"
        | "iso_8859-9" | "iso_8859-9:1989" | "l5" | "latin5" | "windows-1254" | "x-cp1254" => {
            entry(::encoding::all::WINDOWS_1254, "windows-1254")
        }
        "cp1255" | "windows-1255" | "x-cp1255" => {
            entry(::encoding::all::WINDOWS_1255, "windows-1255")
        }
        "cp1256" | "windows-1256" | "x-cp1256" => {
            entry(::encoding::all::WINDOWS_1256, "windows-1256")
        }
        "cp1257" | "windows-1257" | "x-cp1257" => {
            entry(::encoding::all::WINDOWS_1257, "windows-1257")
        }
        "cp1258" | "windows-1258" | "x-cp1258" => {
            entry(::encoding::all::WINDOWS_1258, "windows-1258")
        }
        "x-mac-cyrillic" | "x-mac-ukrainian" => {
            entry(::encoding::all::MAC_CYRILLIC, "x-mac-cyrillic")
        }
        "chinese" | "csgb2312" | "csiso58gb231280" | "gb2312" | "gb_2312" | "gb_2312-80"
        | "gbk" | "iso-ir-58" | "x-gbk" => entry(::encoding::all::GBK, "gbk"),
        "gb18030" => entry(::encoding::all::GB18030, "gb18030"),
        "hz-gb-2312" => entry(::encoding::all::HZ, "hz-gb-2312"),
        "big5" | "big5-hkscs" | "cn-big5" | "csbig5" | "x-x-big5" => {
            entry(::encoding::all::BIG5_2003, "big5")
        }
        "cseucpkdfmtjapanese" | "euc-jp" | "x-euc-jp" => entry(::encoding::all::EUC_JP, "euc-jp"),
        "csiso2022jp" | "iso-2022-jp" => entry(::encoding::all::ISO_2022_JP, "iso-2022-jp"),
        "csshiftjis" | "ms_kanji" | "shift-jis" | "shift_jis" | "sjis" | "windows-31j"
        | "x-sjis" => entry(::encoding::all::WINDOWS_31J, "shift_jis"),
        "cseuckr" | "csksc56011987" | "euc-kr" | "iso-ir-149" | "korean" | "ks_c_5601-1987"
        | "ks_c_5601-1989" | "ksc5601" | "ksc_5601" | "windows-949" => {
            entry(::encoding::all::WINDOWS_949, "euc-kr")
        }
        "csiso2022kr" | "iso-2022-kr" | "iso-2022-cn" | "iso-2022-cn-ext" => {
            entry(::encoding::all::whatwg::REPLACEMENT, "replacement")
        }
        "utf-16be" => entry(::encoding::all::UTF_16BE, "utf-16be"),
        "utf-16" | "utf-16le" => entry(::encoding::all::UTF_16LE, "utf-16le"),
        "x-user-defined" => entry(::encoding::all::whatwg::X_USER_DEFINED, "x-user-defined"),
        _ => return None,
    };
    Some(result)
}
