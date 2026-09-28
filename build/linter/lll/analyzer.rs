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

// 本文件由 build/linter/lll/analyzer.go 迁移而来：收集 Go 文件、扫描长行并通过 analysis.Pass 报告诊断。
// Go package: lll。
//
// Go imports:
// - bufio
// - fmt
// - go/token
// - os
// - strings
// - unicode/utf8
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

use std::fs::File;
use std::io::{BufRead, BufReader};

const lllName: &str = "lll";
pub(super) const MAX_SCAN_TOKEN_SIZE: usize = 64 * 1024;

enum ScannerLine {
    Eof,
    Line(Vec<u8>),
    TooLong,
}

// 该名字既用于 analyzer 注册，也会出现在外部调用方的诊断归类中。
const goCommentDirectivePrefix: &str = "//go:";

// settings 对应 Go 的同名结构体：保存长行阈值和 tab 展开宽度。
pub struct settings {
    // LineLength is the maximum line length.
    pub LineLength: i32,
    // TabWidth is the width of a tab character.
    pub TabWidth: i32,
}

// Analyzer is the analyzer struct of lll.
// 这里保留 Go 的 analysis.Analyzer 字段语义；Run 闭包仍调用 runLll 并把错误向上传递。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: lllName,
    doc: "Reports long lines",
    requires: &[],
    run: |pass: &mut analysis::Pass| -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
        runLll(
            pass,
            &settings {
                LineLength: 120,
                TabWidth: 1,
            },
        )?;

        Ok(None)
    },
};

// result 对应 Go 的诊断结果结构，保留文件名、行号和提示文本。
#[derive(Debug, PartialEq, Eq)]
pub struct result {
    pub Filename: String,
    pub Line: i32,
    pub Text: String,
}

// runLll 对应 Go 的入口函数：从 analysis.Pass 中取文件名，再逐个文件扫描长行。
pub fn runLll(pass: &mut analysis::Pass, settings: &settings) -> Result<(), analysis::Error> {
    let mut fileNames: Vec<String> = Vec::with_capacity(pass.Files.len());
    for f in &pass.Files {
        let pos = pass.Fset.PositionFor(f.Pos(), false);
        // Go 版本跳过 failpoint 生成文件；这里保留同样的后缀判断。
        if !pos.Filename.is_empty() && !pos.Filename.ends_with("failpoint_binding__.go") {
            fileNames.push(pos.Filename);
        }
    }

    let spaces = " ".repeat(usize::try_from(settings.TabWidth).expect("negative Repeat count"));

    for f in fileNames {
        let lintIssues = getLLLIssuesForFile(&f, settings.LineLength, &spaces)?;
        for i in lintIssues {
            let (fileContent, tf) = util::ReadFile(&mut pass.Fset, &i.Filename)
                .map_err(|err| format!("can't get file {} contents: {}", i.Filename, err))?;
            // ReadFile 当前返回 token.File 裸指针；只在读取稳定的 base 时做一次受限解引用。
            let file_base = unsafe { (*tf).Base() };
            let pos = token::Pos(file_base + findLineOffset(&fileContent, i.Line));
            pass.Reportf(pos, "too long");
        }
    }

    Ok(())
}

// getLLLIssuesForFile 对应 Go 的文件扫描函数：逐行替换 tab、忽略 go directive 和 import 块。
pub fn getLLLIssuesForFile(
    filename: &str,
    maxLineLen: i32,
    tabSpaces: &str,
) -> Result<Vec<result>, analysis::Error> {
    // Go 这里用 os.Open 并 defer Close；Rust 的 File/BufReader 在作用域结束时自动关闭。
    let f = File::open(filename).map_err(|err| format!("can't open file {}: {}", filename, err))?;
    let mut scanner = BufReader::new(f);
    scanLLLIssues(&mut scanner, filename, maxLineLen, tabSpaces)
}

// scanLLLIssues 保留 bufio.Scanner 的逐行和默认 64KiB token 上限语义，同时接受任意文件字节。
pub(super) fn scanLLLIssues<R: BufRead>(
    scanner: &mut R,
    filename: &str,
    maxLineLen: i32,
    tabSpaces: &str,
) -> Result<Vec<result>, analysis::Error> {
    let mut res: Vec<result> = Vec::new();

    let mut lineNumber = 0;
    let mut multiImportEnabled = false;

    loop {
        let mut raw_line = match nextScannerLine(scanner)
            .map_err(|err| format!("can't scan file {}: {}", filename, err))?
        {
            ScannerLine::Eof => break,
            ScannerLine::Line(line) => line,
            ScannerLine::TooLong => {
                // Scanner does not yield the offending line, so lineNumber remains the count of
                // successfully scanned lines.
                if maxLineLen >= MAX_SCAN_TOKEN_SIZE as i32 {
                    return Err(format!(
                        "can't scan file {}: bufio.Scanner: token too long",
                        filename
                    ));
                }
                res.push(result {
                    Filename: filename.to_string(),
                    Line: lineNumber,
                    Text: format!("line is more than {} characters", MAX_SCAN_TOKEN_SIZE),
                });
                break;
            }
        };

        lineNumber += 1;

        if raw_line.ends_with(b"\n") {
            raw_line.pop();
        }
        // bufio.ScanLines applies dropCR both before a newline and to the final token at EOF.
        if raw_line.ends_with(b"\r") {
            raw_line.pop();
        }
        let line = replaceTabs(&raw_line, tabSpaces.as_bytes());

        if line.starts_with(goCommentDirectivePrefix.as_bytes()) {
            continue;
        }

        if line.starts_with(b"import") {
            multiImportEnabled = line.ends_with(b"(");
            continue;
        }

        if multiImportEnabled {
            if line == b")" {
                multiImportEnabled = false;
            }

            continue;
        }

        let lineLen = goRuneCount(&line);
        if lineLen as i64 > i64::from(maxLineLen) {
            res.push(result {
                Filename: filename.to_string(),
                Line: lineNumber,
                Text: format!("line is {} characters", lineLen),
            });
        }
    }

    Ok(res)
}

// Read at most MAX_SCAN_TOKEN_SIZE bytes before deciding that Scanner would return ErrTooLong.
fn nextScannerLine<R: BufRead>(scanner: &mut R) -> std::io::Result<ScannerLine> {
    let mut line = Vec::new();
    loop {
        let available = scanner.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(ScannerLine::Eof)
            } else {
                Ok(ScannerLine::Line(line))
            };
        }

        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline >= MAX_SCAN_TOKEN_SIZE {
                return Ok(ScannerLine::TooLong);
            }
            line.extend_from_slice(&available[..=newline]);
            scanner.consume(newline + 1);
            return Ok(ScannerLine::Line(line));
        }

        if line.len() + available.len() >= MAX_SCAN_TOKEN_SIZE {
            return Ok(ScannerLine::TooLong);
        }
        let consumed = available.len();
        line.extend_from_slice(available);
        scanner.consume(consumed);
    }
}

fn replaceTabs(line: &[u8], tabSpaces: &[u8]) -> Vec<u8> {
    let mut replaced = Vec::with_capacity(line.len());
    for byte in line {
        if *byte == b'\t' {
            replaced.extend_from_slice(tabSpaces);
        } else {
            replaced.push(*byte);
        }
    }
    replaced
}

// utf8.RuneCountInString consumes one byte for each invalid UTF-8 encoding.
fn goRuneCount(mut line: &[u8]) -> usize {
    let mut count = 0;
    while !line.is_empty() {
        match std::str::from_utf8(line) {
            Ok(valid) => {
                count += valid.chars().count();
                break;
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to > 0 {
                    count += std::str::from_utf8(&line[..valid_up_to])
                        .expect("validated UTF-8 prefix")
                        .chars()
                        .count();
                    line = &line[valid_up_to..];
                }
                count += 1;
                line = &line[1..];
            }
        }
    }
    count
}

// runLll only asks for column one; scanning raw bytes preserves Go offsets for invalid UTF-8.
pub(super) fn findLineOffset(fileContent: &[u8], line: i32) -> i32 {
    if fileContent.is_empty() || line <= 0 {
        return -1;
    }
    if line == 1 {
        return 0;
    }

    let mut current_line = 1;
    for (offset, byte) in fileContent.iter().enumerate() {
        if *byte == b'\n' {
            current_line += 1;
            if current_line == line && offset + 1 < fileContent.len() {
                return (offset + 1) as i32;
            }
        }
    }
    -1
}

// init 对应 Go 的包初始化：按配置跳过 analyzer，再登记为跳过 analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}
