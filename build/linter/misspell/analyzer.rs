// Copyright 2026 AsterSQL.

// Copyright 2022 PingCAP, Inc.
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

// 本文件由 build/linter/misspell/analyzer.go 迁移而来：配置替换字典、遍历 pass.Files 并上报拼写诊断。
// Go package: misspell。
//
// Go imports:
// - fmt
// - go/token
// - github.com/golangci/misspell
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Name is the name of the analyzer.
pub const Name: &str = "misspell";

// Analyzer is the analyzer struct of misspell.
// Go 的 analysis.Analyzer 持有 run 函数指针；Rust 保留同一字段和入口函数对应关系。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: Name,
    doc: "Checks the spelling error in code",
    requires: &[],
    run,
};

// init 对应 Go 的包初始化：按照配置跳过 misspell，并登记到通用跳过集合。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
    util::SkipAnalyzer(&Analyzer);
}

// Misspell is the config of misspell.
// mapstructure:"ignore-words" 标签无法直接迁移，保留为字段注释说明配置键。
pub struct Misspell {
    pub Locale: String,
    // mapstructure:"ignore-words"
    pub IgnoreWords: Vec<String>,
}

// run 对应 Go 的 analyzer Run：初始化替换器、应用配置、编译规则并逐文件检查。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    let mut r = misspell::Replacer {
        Replacements: misspell::DictMain.to_vec(),
    };

    // Figure out regional variations
    let settings = Misspell {
        Locale: String::new(),
        IgnoreWords: Vec::new(),
    };
    // 迁移草稿保持“先构造默认配置，再按字段逐项覆写”的 Go 初始化顺序。

    if !settings.IgnoreWords.is_empty() {
        // Go 允许从配置中删除忽略词规则；这里保留外部库 RemoveRule 调用点。
        r.RemoveRule(&settings.IgnoreWords);
    }

    r.Compile();
    let mut files: Vec<String> = Vec::with_capacity(pass.Files.len());
    for file in &pass.Files {
        let pos = pass.Fset.PositionFor(file.Pos(), false);
        files.push(pos.Filename);
    }
    for f in files {
        runOnFile(&f, &r, pass)?;
    }

    Ok(None)
}

// runOnFile 对应 Go 的单文件检查：读取源码文本，用 misspell.Replace 生成差异并报告位置。
pub fn runOnFile(
    fileName: &str,
    r: &misspell::Replacer,
    pass: &mut analysis::Pass,
) -> Result<(), analysis::Error> {
    let (fileContent, tf) = util::ReadFile(&mut pass.Fset, fileName)
        .map_err(|err| format!("can't get file {} contents: {}", fileName, err))?;

    // use r.Replace, not r.ReplaceGo because r.ReplaceGo doesn't find
    // issues inside strings: it searches only inside comments. r.Replace
    // searches all words: it treats input as a plain text. A standalone misspell
    // tool uses r.Replace by default.
    // 这里保留 Go 的关键选择：按纯文本检查，因而字符串字面量中的拼写问题也会被报告。
    let (_updated, diffs) = r.Replace(&sanitizeForMisspell(&fileContent));
    for diff in diffs {
        // ReadFile 当前返回 token.File 裸指针；只在读取稳定的 base 时做一次受限解引用。
        let file_base = unsafe { (*tf).Base() };
        let pos = token::Pos(file_base + findOffset(&fileContent, diff.Line, diff.Column));
        pass.Reportf(
            pos,
            &format!(
                "[{}] `{}` is a misspelling of `{}`",
                Name, diff.Original, diff.Corrected
            ),
        );
    }
    Ok(())
}

// misspell 只识别 ASCII 单词。把每个非法 UTF-8 字节替换为单字节 NUL，既保持非词边界，
// 又保持 v0.8.0 Diff.Column 使用的原始 byte offset。
pub(super) fn sanitizeForMisspell(fileContent: &[u8]) -> String {
    let mut sanitized = fileContent.to_vec();
    let mut offset = 0;
    while offset < sanitized.len() {
        match std::str::from_utf8(&sanitized[offset..]) {
            Ok(_) => break,
            Err(err) => {
                let invalid = offset + err.valid_up_to();
                sanitized[invalid] = b'\0';
                offset = invalid + 1;
            }
        }
    }
    String::from_utf8(sanitized).expect("invalid bytes were replaced one-for-one")
}

// util.FindOffset iterates a Go string by runes; invalid encodings consume one byte each.
pub(super) fn findOffset(fileContent: &[u8], line: i32, column: i32) -> i32 {
    let mut current_line = 1;
    let mut current_column = 1;
    let mut offset = 0;

    while offset < fileContent.len() {
        if current_line == line && current_column == column {
            return offset as i32;
        }

        if fileContent[offset] == b'\n' {
            current_line += 1;
            current_column = 1;
        } else {
            current_column += 1;
        }
        offset += goRuneWidth(&fileContent[offset..]);
    }
    -1
}

fn goRuneWidth(input: &[u8]) -> usize {
    match std::str::from_utf8(input) {
        Ok(valid) => valid
            .chars()
            .next()
            .expect("caller excludes empty input")
            .len_utf8(),
        Err(err) if err.valid_up_to() > 0 => std::str::from_utf8(&input[..err.valid_up_to()])
            .expect("validated UTF-8 prefix")
            .chars()
            .next()
            .expect("non-empty validated prefix")
            .len_utf8(),
        Err(_) => 1,
    }
}
