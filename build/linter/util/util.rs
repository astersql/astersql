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

use std::any::Any;
use std::collections::HashSet;
use std::io::Read;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum skipType {
    skipNone = 0,
    skipLinter = 1,
    skipFile = 2,
}

// Directive is a comment of the form '//lint:<command> [arguments...]' and `//nolint:<command>`.
// It represents instructions to the static analysis tool.
#[derive(Clone)]
pub struct Directive {
    pub Command: skipType,
    pub Linters: Vec<String>,
    pub Directive: *mut ast::Comment,
    pub Node: ast::Node,
}

pub fn parseDirective(s: String) -> (skipType, Vec<String>) {
    if let Some(rest) = s.strip_prefix("//lint:") {
        // str::split keeps empty fields between spaces, matching strings.Split.
        let fields: Vec<String> = rest.split(' ').map(str::to_string).collect();
        match fields[0].as_str() {
            "ignore" => return (skipType::skipLinter, fields[1..].to_vec()),
            "file-ignore" => return (skipType::skipFile, fields[1..].to_vec()),
            _ => return (skipType::skipNone, Vec::new()),
        }
    }

    let linter = s.strip_prefix("//nolint:").unwrap_or(&s).to_string();
    (skipType::skipLinter, vec![linter])
}

// ParseDirectives extracts all directives from a list of Go files.
pub fn ParseDirectives(files: Vec<*mut ast::File>, fset: &token::FileSet) -> Vec<Directive> {
    let mut dirs = Vec::new();
    for file in files {
        let comments = unsafe { (*file).Comments.clone() };
        for (node, comment_groups) in ast::NewCommentMap(fset, file, comments) {
            for comment_group in comment_groups {
                for comment in comment_group.List {
                    let text = unsafe { (*comment).Text.clone() };
                    if !text.starts_with("//lint:") && !text.starts_with("//nolint:") {
                        continue;
                    }
                    let (command, linters) = parseDirective(text);
                    dirs.push(Directive {
                        Command: command,
                        Linters: linters,
                        Directive: comment,
                        Node: node.clone(),
                    });
                }
            }
        }
    }
    dirs
}

pub fn doDirectives(pass: &mut analysis::Pass) -> Result<Option<Box<dyn Any>>, analysis::Error> {
    Ok(Some(Box::new(ParseDirectives(
        pass.Files.clone(),
        &pass.Fset,
    ))))
}

fn directivesResultType() -> std::any::TypeId {
    reflect::TypeOf::<Vec<Directive>>()
}

// Directives is a fact that contains a list of directives.
pub static Directives: analysis::Analyzer = analysis::Analyzer {
    Name: "directives",
    Doc: "extracts linter directives",
    Requires: Vec::new(),
    Run: Some(analysis::Run::Function(doDirectives)),
    RunDespiteErrors: true,
    ResultType: Some(directivesResultType),
};

// SkipAnalyzer updates an analyzer from `staticcheck` and `golangci-linter` to make it work on nogo.
// They have "lint:ignore" or "nolint" to make the analyzer ignore the code.
pub fn SkipAnalyzer(analyzer: &mut analysis::Analyzer) {
    analyzer.Requires.push(&Directives);
    let analyzerName = analyzer.Name;
    let mut oldRun = analyzer.Run.take();
    analyzer.Run = Some(analysis::Run::Closure(Box::new(move |p| {
        let mut pass = p.clone();
        let dirs = pass
            .ResultOf
            .get(&(std::ptr::from_ref(&Directives)))
            .expect("Directives result is missing")
            .downcast_ref::<Vec<Directive>>()
            .expect("Directives result has the wrong type")
            .clone();

        let mut ignoreFiles = HashSet::new();
        for dir in &dirs {
            if dir.Command == skipType::skipFile {
                let ignorePos = report::DisplayPosition(&pass.Fset, dir.Node.Pos());
                ignoreFiles.insert(ignorePos.Filename);
            }
        }

        pass.Files = p
            .Files
            .iter()
            .copied()
            .filter(|file| {
                let pos = pass.Fset.PositionFor(unsafe { (**file).Pos() }, false);
                !ignoreFiles.contains(&pos.Filename)
            })
            .collect();

        let oldReport = Rc::clone(&p.Report);
        let reportFset = pass.Fset.clone();
        let reportDirs = dirs.clone();
        let reportIgnoreFiles = ignoreFiles.clone();
        pass.Report = Rc::new(move |diag: analysis::Diagnostic| {
            for dir in &reportDirs {
                match dir.Command {
                    skipType::skipLinter => {
                        let ignorePos = report::DisplayPosition(&reportFset, dir.Node.Pos());
                        let nodePos = report::DisplayPosition(&reportFset, diag.Pos);
                        if ignorePos.Filename != nodePos.Filename || ignorePos.Line != nodePos.Line
                        {
                            continue;
                        }
                        for check in dir.Linters[0].split(',') {
                            if check.trim() == analyzerName {
                                return;
                            }
                        }
                    }
                    skipType::skipFile => {
                        let nodePos = report::DisplayPosition(&reportFset, diag.Pos);
                        if reportIgnoreFiles.contains(&nodePos.Filename) {
                            return;
                        }
                    }
                    skipType::skipNone => continue,
                }
            }
            oldReport(diag);
        });

        oldRun
            .as_mut()
            .expect("analysis analyzer Run should exist")
            .call(&mut pass)
    })));
}

// SkipAnalyzerByConfig updates an analyzer to skip files according to `exclude_files`.
pub fn SkipAnalyzerByConfig(analyzer: &mut analysis::Analyzer) {
    let analyzerName = analyzer.Name;
    let mut oldRun = analyzer.Run.take();
    analyzer.Run = Some(analysis::Run::Closure(Box::new(move |p| {
        let mut pass = p.clone();
        pass.Files = p
            .Files
            .iter()
            .copied()
            .filter(|file| {
                let pos = pass.Fset.PositionFor(unsafe { (**file).Pos() }, false);
                shouldRun(analyzerName, &pos.Filename)
            })
            .collect();
        oldRun
            .as_mut()
            .expect("analysis analyzer Run should exist")
            .call(&mut pass)
    })));
}

// FormatCode is to format code for nogo.
pub fn FormatCode(code: &str) -> String {
    if code.contains('`') {
        return code.to_string(); // TODO: properly escape or remove
    }
    format!("`{code}`")
}

// MakeFakeLoaderPackageInfo creates a fake loader.PackageInfo for a given package.
pub fn MakeFakeLoaderPackageInfo(pass: &analysis::Pass) -> Box<loader::PackageInfo> {
    Box::new(loader::PackageInfo {
        Pkg: pass.Pkg.clone(),
        Importable: true,
        TransitivelyErrorFree: true,
        Files: pass.Files.clone(),
        Errors: Vec::new(),
        Info: pass.TypesInfo.clone(),
    })
}

// ReadFile reads a file and adds it to the FileSet so diagnostics can use its positions.
#[derive(Debug)]
pub struct ReadFileError {
    pub Op: &'static str,
    pub Path: String,
    pub Err: std::io::Error,
}

impl std::fmt::Display for ReadFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {}: {}", self.Op, self.Path, self.Err)
    }
}

impl std::error::Error for ReadFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.Err)
    }
}

pub fn ReadFile(
    fset: &mut token::FileSet,
    filename: &str,
) -> Result<(Vec<u8>, *mut token::File), ReadFileError> {
    //nolint: gosec
    let mut source = std::fs::File::open(filename).map_err(|error| ReadFileError {
        Op: "open",
        Path: filename.to_string(),
        Err: error,
    })?;
    let mut content = Vec::new();
    source
        .read_to_end(&mut content)
        .map_err(|error| ReadFileError {
            Op: "read",
            Path: filename.to_string(),
            Err: error,
        })?;
    let file = fset.AddFile(filename, -1, content.len());
    unsafe {
        (*file).SetLinesForContent(&content);
    }
    Ok((content, file))
}

// FindOffset returns the byte offset of a one-based line and rune column.
pub fn FindOffset(fileText: &[u8], line: i32, column: i32) -> i32 {
    let mut currentCol = 1;
    let mut currentLine = 1;
    let mut offset = 0;

    while offset < fileText.len() {
        if currentLine == line && currentCol == column {
            return offset as i32;
        }
        if fileText[offset] == b'\n' {
            currentLine += 1;
            currentCol = 1;
            offset += 1;
        } else {
            currentCol += 1;
            offset += goRuneLen(&fileText[offset..]);
        }
    }
    -1
}

fn goRuneLen(input: &[u8]) -> usize {
    let width = match input[0] {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 1,
    };
    if input.len() < width || std::str::from_utf8(&input[..width]).is_err() {
        1
    } else {
        width
    }
}

// GetPackageName returns the package name used in this file.
pub fn GetPackageName(imports: Vec<*mut ast::ImportSpec>, path: &str, defaultName: &str) -> String {
    let quoted = format!("\"{path}\"");
    for import in imports {
        unsafe {
            if (*import).Path.Value == quoted {
                if let Some(name) = &(*import).Name {
                    return name.Name.clone();
                }
                return defaultName.to_string();
            }
        }
    }
    String::new()
}
