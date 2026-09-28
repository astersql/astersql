// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// UCA 生成器迁移期单元测试：对齐 Go 的 CET 解析与表打包语义。
//
// 覆盖：十六进制/权重/条目解析、短长权重打包、Hangul 分解与隐式权重、
// 以及按模板写出格式化 Go / Rust 源码。

use super::generator::{
    OutputTarget, generateFile, generateRustFile, parseAllKeys, parseCETEntry, parseCETHex,
    parseCETWeights, selectOutputTarget, unicodeVersion,
};

/// 校验 `parseCETHex` / `parseCETWeights` / `parseCETEntry` 与 Go 行为一致。
#[test]
fn parses_hex_weights_and_entries_like_go() {
    assert_eq!(parseCETHex("10aF rest"), (true, 0x10af, " rest"));
    assert_eq!(parseCETHex("xyz"), (false, 0, "xyz"));

    let (ok, weights, left) = parseCETWeights("[.1234.0020.0002][*ABCD.0000] tail");
    assert!(ok);
    assert_eq!(weights, vec![0x1234, 0xabcd]);
    assert_eq!(left, " tail");

    let (ok, entry, left) = parseCETEntry("0041 ; [.1C47.0020.0008] trailing");
    assert!(ok);
    assert!(entry.is_some());
    assert_eq!(left, " trailing");
}

/// 校验短权重压入 MapTable4、长权重写 LongRune8 哨兵与 LongRuneMap。
#[test]
fn packs_short_and_long_weights_like_go() {
    let input = "0041 ; [.0001.0000.0000][.0000.0000.0000][.0002.0000.0000]\n\
                 0042 ; [.0001.0000.0000][.0002.0000.0000][.0003.0000.0000][.0004.0000.0000][.0005.0000.0000]\n";
    let table = parseAllKeys(input, 0x100, unicodeVersion::unicode0900);

    assert_eq!(table.MapTable4[0x41], 0x0002_0001);
    assert_eq!(table.MapTable4[0x42], super::LongRune8);
    assert_eq!(table.LongRuneMap[&0x42], [0x0004_0003_0002_0001, 0x0005]);
}

/// 校验 Hangul syllable 分解及 4.0.0 / 9.0.0 隐式权重计算。
#[test]
fn decomposes_hangul_and_calculates_implicit_weights_like_go() {
    assert_eq!(
        super::generator::decomposeHangulSyllable(0xAC01),
        vec![0x1100, 0x1161, 0x11A8]
    );

    let table = parseAllKeys("", 0x100, unicodeVersion::unicode0400);
    assert_eq!(table.getImplicitWeight0400(0x4E00), (0xCE00_FB40, 0));
    assert_eq!(table.getImplicitWeight0900(0xD800), (0xFFFD, 0));
}

/// 校验 `generateFile` 写出的 Go 源码含表名、长度、权重与 URL。
#[test]
fn generates_formatted_go_source_from_the_template() {
    let mut table = parseAllKeys(
        "0041 ; [.1234.0020.0002]\n",
        0x80,
        unicodeVersion::unicode0400,
    );
    table.Name = "TestTable".to_owned();
    table.URL = "https://example.invalid/allkeys.txt".to_owned();

    let output = std::env::temp_dir().join(format!("ucadata-generator-{}.go", std::process::id()));
    generateFile(output.to_str().unwrap(), &table);
    let source = std::fs::read_to_string(&output).unwrap();
    std::fs::remove_file(output).unwrap();

    assert!(source.contains("var TestTable = struct"));
    assert!(source.contains("[128]uint64"));
    assert!(source.contains("0x1234"));
    assert!(source.contains("https://example.invalid/allkeys.txt"));
}

/// 校验两种 Unicode 表保持各自既有 Rust API，且长权重按 rune 确定排序。
#[test]
fn generates_formatted_rust_source_for_both_unicode_tables() {
    let input = "0043 ; [.0001.0000.0000][.0002.0000.0000][.0003.0000.0000][.0004.0000.0000][.0005.0000.0000]\n\
                 0042 ; [.1234.0000.0000]\n\
                 0041 ; [.0006.0000.0000][.0007.0000.0000][.0008.0000.0000][.0009.0000.0000][.000A.0000.0000]\n";

    let cases = [
        (
            unicodeVersion::unicode0400,
            "DUCET0400Table",
            "MapTable4",
            "LongRuneMap",
            "map_table_weight",
        ),
        (
            unicodeVersion::unicode0900,
            "DUCET0900Table",
            "map_table4",
            "long_rune_map",
            "pub static DUCET0900Table",
        ),
    ];

    for (version, name, map_field, long_field, public_api) in cases {
        let mut table = parseAllKeys(input, 0x80, version);
        table.Name = name.to_owned();
        table.URL = "https://example.invalid/allkeys.txt".to_owned();

        let output = std::env::temp_dir().join(format!(
            "ucadata-generator-{name}-{}.rs",
            std::process::id()
        ));
        generateRustFile(output.to_str().unwrap(), &table);
        let source = std::fs::read_to_string(&output).unwrap();

        assert!(source.contains("pub struct UcaDataTable<const N: usize>"));
        assert!(source.contains(&format!("pub {map_field}: [u64; N]")));
        assert!(source.contains(&format!("pub {long_field}: &'static [(u32, [u64; 2])]")));
        assert!(source.contains(&format!("UcaDataTable<128>")));
        assert!(source.contains(&format!("pub static {name}: UcaDataTable<128>")));
        assert!(source.contains("0x1234"));
        assert!(source.contains("0x9000800070006"));
        assert!(source.contains(public_api));
        if version == unicodeVersion::unicode0400 {
            assert!(source.contains("pub fn long_rune_weight"));
        }
        assert!(source.find("(0x41,").unwrap() < source.find("(0x43,").unwrap());

        let status = std::process::Command::new("rustfmt")
            .args(["--edition", "2024", "--check"])
            .arg(&output)
            .status()
            .unwrap();
        std::fs::remove_file(output).unwrap();
        assert!(status.success());
    }
}

/// 校验 CLI 仅按目标文件名选择 Unicode 版本与 Go/Rust 渲染后端。
#[test]
fn selects_all_supported_output_targets_by_file_name() {
    let cases = [
        (
            "/tmp/generated/unicode_ci_data_generated.go",
            OutputTarget::Go0400,
        ),
        (
            "/tmp/generated/unicode_0900_ai_ci_data_generated.go",
            OutputTarget::Go0900,
        ),
        (
            "/tmp/generated/unicode_ci_data_generated.rs",
            OutputTarget::Rust0400,
        ),
        (
            "/tmp/generated/unicode_0900_ai_ci_data_generated.rs",
            OutputTarget::Rust0900,
        ),
    ];

    for (path, expected) in cases {
        assert_eq!(selectOutputTarget(path.as_ref()), Ok(expected));
    }
}

/// 未知目标必须返回可诊断错误，供 binary 映射为非零退出状态。
#[test]
fn rejects_unknown_output_target() {
    let error = selectOutputTarget("/tmp/generated/unknown.rs".as_ref()).unwrap_err();
    assert!(error.contains("unknown.rs"));
    assert!(error.contains("unsupported ucadata output target"));
}
