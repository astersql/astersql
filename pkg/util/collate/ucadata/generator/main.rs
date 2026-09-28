// Copyright 2023 PingCAP, Inc.
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

// 从 Unicode allkeys 文本生成 collation 权重表的核心逻辑。
//
// 对应 Go `ucadata/generator/main.go`：解析 CET（Collation Element Table）条目、
// 压缩一级权重、补全 UCA 隐式权重、按模板写出 Go 源码并经 gofmt 格式化。
// 支持 Unicode 4.0.0（unicode_ci）与 9.0.0（unicode_0900_ai_ci）两套规则。

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use super::magic::{LongRune8, reverseHexTable};

// 对应 Go 的 //go:embed allkeys-4.0.0.txt。
/// Unicode 4.0.0 allkeys 嵌入文本。
static allkeys0400: &str = include_str!("allkeys-4.0.0.txt");

// 对应 Go 的 //go:embed allkeys-9.0.0.txt。
/// Unicode 9.0.0 allkeys 嵌入文本。
static allkeys0900: &str = include_str!("allkeys-9.0.0.txt");

// cetEntry 保存 allkeys 中一条 Collation Element Table 规则的有效部分。
// Go 版本只处理单字符规则；多字符 contraction 在 MySQL 这组 collation 中被忽略。
/// 单字符 CET 规则：码点 + 一级权重序列。
pub struct cetEntry {
    // a cetEntry can actually contain several characters, which means map many to one or many
    // but MySQL's collation doesn't handle these contractions, so we only care the rule with a
    // single char.
    char: u32,

    // weights is the first level of each collation element
    // as we only implement 'ai_ci' collation, so other levels are ignored directly
    weights: Vec<u16>,
}

// parseCETHex 迁移 Go 的十六进制扫描逻辑。
// 它从 input 开头连续读取合法十六进制字节，返回是否读到有效字符、累计值以及剩余切片。
/// 从字符串前缀扫描十六进制，返回 `(成功, 值, 剩余)`。
pub fn parseCETHex(input: &str) -> (bool, u32, &str) {
    let mut hasValidHex = false;
    let mut currentRune: u32 = 0;
    let mut end = 0;

    for (idx, c) in input.bytes().enumerate() {
        // reverseHexTable 来自同 package 的 magic.go；保留同名常量依赖，后续模块接线再处理。
        if reverseHexTable[c as usize] <= 0xf {
            hasValidHex = true;

            currentRune <<= 4;
            currentRune += reverseHexTable[c as usize] as u32;
            continue;
        }

        // invalid character
        // 遇到非十六进制字节后停止，Go 代码把 idx 作为剩余字符串起点。
        end = idx;
        break;
    }
    (hasValidHex, currentRune, &input[end..])
}

// parseCETWeights 解析一条规则里的一级权重序列。
// Go 代码忽略点号后更高层级权重，这里保留 outer 循环的退出语义。
/// 解析 `[.XXXX...]` / `[*XXXX...]` 形式的一级权重列表。
pub fn parseCETWeights(mut input: &str) -> (bool, Vec<u16>, &str) {
    let mut left: &str;
    let mut ok = true;

    let mut weights: Vec<u16> = Vec::with_capacity(1);
    'outer: loop {
        left = input;

        let weight: u32;

        if input.as_bytes()[0] != b'[' {
            break;
        }
        // ignore the dot or star
        // '[' 后只接受 '.' 或 '*'，其它字符表示权重格式不符合 allkeys 语法。
        if input.as_bytes()[1] != b'.' && input.as_bytes()[1] != b'*' {
            ok = false;
            break;
        }
        input = &input[2..];

        let (next_ok, next_weight, rest) = parseCETHex(input);
        ok = next_ok;
        weight = next_weight;
        input = rest;
        if !ok {
            ok = false;
            break;
        }
        if weight > u16::MAX as u32 {
            ok = false;
            break;
        }

        // then ignore all weight with higher level
        // 高级别权重不参与 ai_ci 排序；这里只验证并跳过，失败时跳出外层解析。
        loop {
            if input.as_bytes()[0] == b'.' {
                let (next_ok, _, rest) = parseCETHex(&input[1..]);
                ok = next_ok;
                input = rest;
                if !ok {
                    ok = false;
                    break 'outer;
                }
            } else if input.as_bytes()[0] == b']' {
                input = &input[1..];
                break;
            } else {
                ok = false;
                break 'outer;
            }
        }

        weights.push(weight as u16);
    }

    (ok, weights, left)
}

// parseCETEntry 从当前文本位置解析一条 CET 记录。
// 返回 nil/None 表示本位置不是有效规则，调用方会跳到下一行继续扫描。
/// 解析 `码点 ; 权重...` 形式的一条 CET 记录。
pub fn parseCETEntry(mut input: &str) -> (bool, Option<cetEntry>, &str) {
    let mut ok: bool;
    let char_value: u32;
    let weights: Vec<u16>;

    let left = input;

    let (next_ok, next_char, rest) = parseCETHex(input);
    ok = next_ok;
    char_value = next_char;
    input = rest;
    if !ok {
        return (false, None, left);
    }
    // then ignore the space and ';'
    // Go 使用带标签的 for/switch 跳过分隔符；用 loop 保持同样停止条件。
    'outer: loop {
        match input.as_bytes()[0] {
            b' ' => input = &input[1..],
            b';' => input = &input[1..],
            _ => break 'outer,
        }
    }
    // then parse the weights
    let (next_ok, next_weights, rest) = parseCETWeights(input);
    ok = next_ok;
    weights = next_weights;
    input = rest;
    if !ok {
        return (false, None, left);
    }
    (
        true,
        Some(cetEntry {
            char: char_value,
            weights,
        }),
        input,
    )
}

// unicodeVersion 对应 Go 的 int 枚举，区分 4.0.0 与 9.0.0 的隐式权重规则。
/// Unicode UCA 版本：决定隐式权重与特殊码点处理路径。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum unicodeVersion {
    unicode0400 = 400,
    unicode0900 = 900,
}

// cet 承载一次生成过程中的全部中间表。
// MapTable4 与 LongRuneMap 对应最终生成文件里的两个字段，explicitRune 用于判断是否需要补隐式权重。
/// 生成中的权重表：短表 MapTable4、长表 LongRuneMap 及元数据。
pub struct cet {
    pub Name: String,
    pub Length: u32,

    pub MapTable4: Vec<u64>,
    pub LongRuneMap: HashMap<u32, [u64; 2]>,

    pub URL: String,

    // explicitRune indicates whether this character is set in the `MapTable/LongRuneMap`.
    explicitRune: HashSet<u32>,
    version: unicodeVersion,
}

impl cet {
    // insertWeights 把一个 rune 的一级权重压缩进 MapTable4，必要时写入 LongRuneMap。
    // Go 版本最多支持 8 个非零权重；超过 8 个属于生成器不可达路径。
    /// 将一级权重写入短表或（超过 4 个时）LongRuneMap。
    pub fn insertWeights(&mut self, char_value: u32, mut weights: Vec<u16>) {
        let char_u32 = char_value;
        if char_u32 == 0xFDFA {
            // this is a special case, MySQL doesn't handle this character in unicode 4.0.0
            match self.version {
                unicodeVersion::unicode0400 => return,
                unicodeVersion::unicode0900 => weights.truncate(8),
            }
        }

        self.explicitRune.insert(char_value);

        let mut nonZeroWeights: Vec<u16> = Vec::with_capacity(weights.len());
        for w in weights {
            if w != 0 {
                nonZeroWeights.push(w);
            }
        }

        if nonZeroWeights.len() <= 4 {
            let mut idx = 0;
            for w in nonZeroWeights {
                if w != 0 {
                    self.MapTable4[char_u32 as usize] += (w as u64) << (idx * 16);
                    idx += 1;
                }
            }
        } else if nonZeroWeights.len() <= 8 {
            // 长权重无法装进单个 u64，Go 用 LongRune8 作为哨兵并把两段权重放到 LongRuneMap。
            self.MapTable4[char_u32 as usize] = LongRune8;
            let mut idx = 0;
            let mut weight0: u64 = 0;
            for w in &nonZeroWeights[..4] {
                if *w != 0 {
                    weight0 += (*w as u64) << (idx * 16);
                    idx += 1;
                }
            }
            idx = 0;
            let mut weight1: u64 = 0;
            for w in &nonZeroWeights[4..] {
                if *w != 0 {
                    weight1 += (*w as u64) << (idx * 16);
                    idx += 1;
                }
            }
            self.LongRuneMap.insert(char_value, [weight0, weight1]);
        } else {
            panic!("unreachable");
        }

        // a special value. The `MapTable4` is set to 0xFFFD automatically, but the implementation will get value from `LongRuneMap`
        // and gets 0.
        // 9.0.0 中 U+FFFD 的表项需要显式覆盖，避免运行时从长表读到空值。
        if self.version == unicodeVersion::unicode0900 && char_u32 == 0xFFFD {
            self.LongRuneMap.insert(char_value, [0xFFFD, 0]);
        }
    }

    // calcImplicitWeight 为 allkeys 中没有显式权重的 rune 填充 UCA 隐式权重。
    /// 遍历未显式出现的码点，按版本写入隐式权重。
    pub fn calcImplicitWeight(&mut self) {
        for i in 1..self.Length {
            if self.explicitRune.contains(&i) {
                continue;
            }

            let (first, second) = if self.version == unicodeVersion::unicode0400 {
                self.getImplicitWeight0400(i)
            } else {
                self.getImplicitWeight0900(i)
            };
            if second == 0 {
                self.MapTable4[i as usize] = first;
            } else {
                self.MapTable4[i as usize] = LongRune8;
                self.LongRuneMap.insert(i, [first, second]);
            }
        }
    }

    // getImplicitWeight0400 迁移 Unicode 4.0.0 的隐式权重计算。
    /// 按 UCA 4.0.0 规则计算隐式权重（汉字等未列出码点）。
    pub fn getImplicitWeight0400(&self, r: u32) -> (u64, u64) {
        // Han and other unsigned cases
        let code = r;
        let mut first = (code >> 15) as u64;
        if code >= 0x3400 && code <= 0x4DB5 {
            first += 0xFB80;
        } else if (code >= 0x4E00 && code <= 0x9FA5) || (code >= 0xFA0E && code <= 0xFA0F) {
            first += 0xFB40;
        } else {
            first += 0xFBC0;
        }

        (first + ((((code & 0x7FFF) | 0x8000) as u64) << 16), 0)
    }

    // getImplicitWeight0900 迁移 Unicode 9.0.0 的隐式权重计算。
    // 它额外处理无效 UTF-16 surrogate、U+FFFD、Hangul syllable 和 Tangut 范围。
    /// 按 UCA 9.0.0 规则计算隐式权重（含 Hangul / Tangut 等特殊区间）。
    pub fn getImplicitWeight0900(&self, r: u32) -> (u64, u64) {
        let code = r;
        // invalid characters, they are surrogate pair in utf-16, so removed in unicode
        if (code >= 0xD800 && code <= 0xDFFF) || code == 0xFFFD {
            return (0xFFFD, 0);
        }

        // handle hangul syllable
        if code >= 0xAC00 && code <= 0xD7AF {
            let jamo = decomposeHangulSyllable(r);
            // the length of jamo is 2 or 3, so it will only use a single uint64
            let mut first: u64 = 0;
            for (idx, j) in jamo.iter().enumerate() {
                // `ucadata.DUCET0900Table.MapTable[j]` should have only one weight
                // test has ensured the jamo has only one weight
                first += (self.MapTable4[*j as usize] & 0xFFFF) << (idx * 16);
            }
            return (first, 0);
        }

        // The implicit weight is always [.AAAA.0020.0002][.BBBB.0000.0000]
        // The calculation process of AAAA and BBBB is according to the UCA
        if code >= 0x17000 && code <= 0x18AFF {
            // Tangut characters
            return (0xFB00 + ((((code - 0x17000) | 0x8000) as u64) << 16), 0);
        }

        // Nushu and Khitan Small Script were added into unicode in 10.0 and 13.0, so they don't need to be handled
        // specially.

        // Han and other unsigned cases
        let mut first = (code >> 15) as u64;
        if (code >= 0x3400 && code <= 0x4DB5)
            || (code >= 0x20000 && code <= 0x2A6D6)
            || (code >= 0x2A700 && code <= 0x2B734)
            || (code >= 0x2B740 && code <= 0x2B81D)
            || (code >= 0x2B820 && code <= 0x2CEA1)
        {
            first += 0xFB80;
        } else if (code >= 0x4E00 && code <= 0x9FD5) || (code >= 0xFA0E && code <= 0xFA29) {
            first += 0xFB40;
        } else {
            first += 0xFBC0;
        }

        (first + ((((code & 0x7FFF) | 0x8000) as u64) << 16), 0)
    }
}

// parseAllKeys dumps the `MapTable0900` and `LongRuneTable0900` from allkeys0900
// this function actually has the potential to become a generic parser for both allkeys0900
// and the data in `unicode_ci`. TODO: migrate the `unicode_ci_data` to use this parser
// parseAllKeys 扫描嵌入的 allkeys 文本，构建初始 cet，并跳过无法解析的行。
/// 扫描 allkeys 文本构建初始 `cet`（尚未补隐式权重）。
pub fn parseAllKeys(mut input: &str, length: u32, version: unicodeVersion) -> cet {
    let mut cet_value = cet {
        Name: String::new(),
        Length: length,
        MapTable4: vec![0; length as usize],
        LongRuneMap: HashMap::new(),
        explicitRune: HashSet::new(),
        version,
        URL: String::new(),
    };

    loop {
        let entry: Option<cetEntry>;
        let (_, next_entry, rest) = parseCETEntry(input);
        entry = next_entry;
        input = rest;
        if input.is_empty() {
            break;
        }
        if let Some(entry) = entry {
            if entry.char < length {
                cet_value.insertWeights(entry.char, entry.weights);
            }
        }
        // just go to the next line
        // Go 代码逐字节跳到下一行，避免未解析内容影响下一条 CET 记录。
        loop {
            if input.as_bytes()[0] != b'\n' {
                input = &input[1..];
                continue;
            }

            input = &input[1..];
            break;
        }
    }

    cet_value
}

// decomposeHangulSyllable 将 Hangul syllable 分解为 leading/vowel/trailing jamo。
// 该逻辑服务于 9.0.0 隐式权重，返回长度为 2 或 3 的 rune 序列。
/// 将韩文音节分解为 jamo 序列（长度 2 或 3）。
pub fn decomposeHangulSyllable(r: u32) -> Vec<u32> {
    const syllableBase: u32 = 0xAC00;
    const leadingJamoBase: u32 = 0x1100;
    const vowelJamoBase: u32 = 0x1161;
    const trailingJamoBase: u32 = 0x11A7;
    const vowelJamoCnt: u32 = 21;
    const trailingJamoCnt: u32 = 28;

    let syllableIndex = r - syllableBase;
    let vtCombination = vowelJamoCnt * trailingJamoCnt;
    let leadingJamoIndex = syllableIndex / vtCombination;
    let vowelJamoIndex = (syllableIndex % vtCombination) / trailingJamoCnt;
    let trailingJamoIndex = syllableIndex % trailingJamoCnt;

    let mut result = vec![
        leadingJamoBase + leadingJamoIndex,
        vowelJamoBase + vowelJamoIndex,
    ];
    if trailingJamoIndex > 0 {
        result.push(trailingJamoBase + trailingJamoIndex);
    }

    result
}

// 对应 Go 的 //go:embed data.go.tpl。
/// Go 权重表源码模板（`data.go.tpl`）。
static unicodeDataTemplate: &str = include_str!("data.go.tpl");

/// Unicode 4.0.0 Rust 权重表源码模板。
static unicodeData0400RustTemplate: &str = include_str!("data_0400.rs.tpl");

/// Unicode 9.0.0 Rust 权重表源码模板。
static unicodeData0900RustTemplate: &str = include_str!("data_0900.rs.tpl");

// generateFile 迁移 Go 里的模板渲染、源码格式化和写文件流程。
/// 渲染模板、经 gofmt 格式化后写入目标文件。
pub fn generateFile(filename: &str, d: &cet) {
    // Go: template.New("unicode_template").Funcs(...).Parse(unicodeDataTemplate)
    let rendered_source = render_unicode_template(unicodeDataTemplate, d);
    // Go: format.Source(output.Bytes())；生成器需要写出 Go 源码，所以格式化器仍属于 Go 工具链语义。
    let formatted_source = go_format_source(rendered_source.as_bytes());
    write_formatted_source(filename, formatted_source);
}

/// 按 Unicode 版本渲染 Rust 表，经 Rust 2024 `rustfmt` 格式化后写入目标文件。
pub fn generateRustFile(filename: &str, d: &cet) {
    let template = match d.version {
        unicodeVersion::unicode0400 => unicodeData0400RustTemplate,
        unicodeVersion::unicode0900 => unicodeData0900RustTemplate,
    };
    let rendered_source = render_rust_unicode_template(template, d);
    let formatted_source = rust_format_source(rendered_source.as_bytes());
    write_formatted_source(filename, formatted_source);
}

/// 将 `cet` 数据填入模板，产出未格式化的 Go 源码字符串。
fn render_unicode_template(template: &str, d: &cet) -> String {
    let header_end = template
        .find("var {{.Name}}")
        .expect("unicode template declaration");
    let mut output = template[..header_end]
        .replace("{{.Name}}", &d.Name)
        .replace("{{.URL}}", &d.URL);
    output.push_str(&format!(
        "var {} = struct {{\n    MapTable4 [{}]uint64\n    LongRuneMap map[rune][2]uint64\n}} {{\n    MapTable4: [{}]uint64{{",
        d.Name, d.Length, d.Length
    ));
    for (idx, item) in d.MapTable4.iter().enumerate() {
        if idx % 0xF == 0 {
            output.push('\n');
        }
        output.push_str(&format!("0x{item:X}, "));
    }
    output.push_str("\n},\nLongRuneMap: map[rune][2]uint64{\n");
    let mut long_weights: Vec<_> = d.LongRuneMap.iter().collect();
    long_weights.sort_unstable_by_key(|(r, _)| **r);
    for (r, weights) in long_weights {
        output.push_str(&format!(
            "0x{r:X}: {{0x{:X}, 0x{:X}}},\n",
            weights[0], weights[1]
        ));
    }
    output.push_str("},\n}\n");
    output
}

/// 将 `cet` 数据填入版本专用模板，产出未格式化的 Rust 源码字符串。
fn render_rust_unicode_template(template: &str, d: &cet) -> String {
    let mut map_table = String::new();
    for item in &d.MapTable4 {
        map_table.push_str(&format!("        0x{item:X},\n"));
    }

    let mut long_weights: Vec<_> = d.LongRuneMap.iter().collect();
    long_weights.sort_unstable_by_key(|(r, _)| **r);
    let mut long_rune_map = String::new();
    for (r, weights) in long_weights {
        long_rune_map.push_str(&format!(
            "        (0x{r:X}, [0x{:X}, 0x{:X}]),\n",
            weights[0], weights[1]
        ));
    }

    template
        .replace("{{.Name}}", &d.Name)
        .replace("{{.URL}}", &d.URL)
        .replace("{{.Length}}", &d.Length.to_string())
        .replace("{{.MapTable4}}", &map_table)
        .replace("{{.LongRuneMap}}", &long_rune_map)
}

/// 调用外部 `gofmt` 格式化生成的 Go 源码字节。
fn go_format_source(bytes: &[u8]) -> Vec<u8> {
    format_source("gofmt", &[], bytes)
}

/// 调用外部 `rustfmt`，按 Rust 2024 格式化生成源码。
fn rust_format_source(bytes: &[u8]) -> Vec<u8> {
    format_source("rustfmt", &["--edition", "2024", "--emit", "stdout"], bytes)
}

/// 通过标准输入/输出运行源码格式化器并返回格式化结果。
fn format_source(command: &str, args: &[&str], bytes: &[u8]) -> Vec<u8> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("start {command}: {error}"));
    child
        .stdin
        .as_mut()
        .unwrap_or_else(|| panic!("{command} stdin"))
        .write_all(bytes)
        .unwrap_or_else(|error| panic!("write {command} input: {error}"));
    let output = child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for {command}: {error}"));
    assert!(
        output.status.success(),
        "{command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// 写入已经格式化的生成源码。
fn write_formatted_source(filename: &str, source: Vec<u8>) {
    std::fs::write(filename, source).expect("write generated unicode data");
}

/// CLI 支持的 Unicode 版本与源码后端组合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputTarget {
    Go0400,
    Go0900,
    Rust0400,
    Rust0900,
}

/// 仅按目标路径的文件名选择版本与后端，目录部分保持为调用方给出的值。
pub fn selectOutputTarget(filename: &Path) -> Result<OutputTarget, String> {
    let target_name = filename
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("unsupported ucadata output target: {}", filename.display()))?;

    match target_name {
        "unicode_ci_data_generated.go" => Ok(OutputTarget::Go0400),
        "unicode_0900_ai_ci_data_generated.go" => Ok(OutputTarget::Go0900),
        "unicode_ci_data_generated.rs" => Ok(OutputTarget::Rust0400),
        "unicode_0900_ai_ci_data_generated.rs" => Ok(OutputTarget::Rust0900),
        _ => Err(format!(
            "unsupported ucadata output target: {}",
            filename.display()
        )),
    }
}

/// 从内嵌 allkeys 构造指定版本的完整 DUCET 表。
fn buildTable(version: unicodeVersion) -> cet {
    match version {
        unicodeVersion::unicode0400 => {
            // in 4.0.0, only cares the character between 0 and 0xFFFF
            let mut table = parseAllKeys(allkeys0400, 0x10000, version);
            table.Name = "DUCET0400Table".to_string();
            table.URL = "https://www.unicode.org/Public/UCA/4.0.0/allkeys-4.0.0.txt".to_string();
            table.calcImplicitWeight();
            table
        }
        unicodeVersion::unicode0900 => {
            let mut table = parseAllKeys(allkeys0900, 0x2CEA1, version);
            table.Name = "DUCET0900Table".to_string();
            table.URL = "https://www.unicode.org/Public/UCA/9.0.0/allkeys.txt".to_string();
            table.calcImplicitWeight();
            table
        }
    }
}

/// 生成一个已知目标，并写入调用方给出的确切路径。
pub fn generateOutputTarget(filename: &Path) -> Result<(), String> {
    let target = selectOutputTarget(filename)?;
    let filename_str = filename.to_str().ok_or_else(|| {
        format!(
            "ucadata output path is not valid UTF-8: {}",
            filename.display()
        )
    })?;
    let version = match target {
        OutputTarget::Go0400 | OutputTarget::Rust0400 => unicodeVersion::unicode0400,
        OutputTarget::Go0900 | OutputTarget::Rust0900 => unicodeVersion::unicode0900,
    };
    let table = buildTable(version);

    match target {
        OutputTarget::Go0400 | OutputTarget::Go0900 => generateFile(filename_str, &table),
        OutputTarget::Rust0400 | OutputTarget::Rust0900 => generateRustFile(filename_str, &table),
    }
    Ok(())
}

/// 解析 binary 参数并执行单目标生成。
pub fn runGenerator(args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let mut args = args.into_iter();
    let program = args
        .next()
        .unwrap_or_else(|| OsString::from("ucadata-generator"));
    let output = args
        .next()
        .ok_or_else(|| format!("usage: {} <output-path>", Path::new(&program).display()))?;
    if args.next().is_some() {
        return Err(format!(
            "usage: {} <output-path>",
            Path::new(&program).display()
        ));
    }

    generateOutputTarget(Path::new(&output))
}
