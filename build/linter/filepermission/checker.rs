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

// 本文件由 build/linter/filepermission/checker.go 机械迁移而来，保留 Go 实现结构；当前不保证可编译。
// 这个草稿描述 filepermission analyzer 如何遍历 Go 源文件并报告可执行权限；
// 当前不会真正接入 Go analysis 框架，也不会修改文件权限或执行业务动作。
// Go package: filepermission。
//
// Go imports:
// - os
// - github.com/pingcap/tidb/build/linter/util
// - golang.org/x/tools/go/analysis

// Name 对应 Go 的 analyzer 名称常量。
pub const Name: &str = "filepermission";

// Analyzer 对应 Go 的 analysis.Analyzer 字面量，保留 Name、Doc 和 Run 字段。
// Run 字段指向本文件的 run 函数，表示 analyzer 框架会把 analysis.Pass 交给它。
pub static Analyzer: analysis::Analyzer = analysis::Analyzer {
    name: Name,
    doc: "Go files should not have execution permission",
    requires: &[],
    run,
};

// formatGoFileMode renders Unix st_mode bits with the same symbolic layout as Go's os.FileMode.String.
// The analyzer only receives source files, but keeping every Unix file type and special bit makes the
// diagnostic stable even when a test or unusual filesystem exposes a non-regular entry.
#[cfg(unix)]
pub fn formatGoFileMode(mode: u32) -> String {
    const FILE_TYPE_MASK: u32 = 0o170000;
    let file_type = mode & FILE_TYPE_MASK;
    let is_char_device = file_type == 0o020000;
    let mut prefix = String::new();

    match file_type {
        0 | 0o100000 => {}
        0o040000 => prefix.push('d'),
        0o120000 => prefix.push('L'),
        0o060000 => prefix.push('D'),
        0o010000 => prefix.push('p'),
        0o140000 => prefix.push('S'),
        0o020000 => prefix.push('D'),
        _ => prefix.push('?'),
    }
    if mode & 0o4000 != 0 {
        prefix.push('u');
    }
    if mode & 0o2000 != 0 {
        prefix.push('g');
    }
    if is_char_device {
        prefix.push('c');
    }
    if mode & 0o1000 != 0 {
        prefix.push('t');
    }
    if prefix.is_empty() {
        prefix.push('-');
    }

    const PERMISSIONS: [(u32, char); 9] = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, character) in PERMISSIONS {
        prefix.push(if mode & bit != 0 { character } else { '-' });
    }
    prefix
}

// run 对应 Go 的 analyzer 回调：逐个检查 pass.Files 中源文件的文件模式。
// Go 返回 (any, error)；Rust 使用 analyzer 统一结果类型并通过 ? 原样传播 metadata 错误。
pub fn run(pass: &mut analysis::Pass) -> Result<Option<Box<dyn std::any::Any>>, analysis::Error> {
    for file_index in 0..pass.Files.len() {
        // 先复制位置，避免在 Reportf 可变借用 pass 时仍持有 Files 的不可变借用。
        let file_position = pass.Files[file_index].Pos();
        let fn_name = pass.Fset.PositionFor(file_position, false).Filename;
        if !fn_name.is_empty() {
            // Go 这里调用 os.Stat 读取真实文件元数据；草稿保留 IO 入口和错误向上传播语义。
            let stat = std::fs::metadata(&fn_name)?;

            // Go 使用 stat.Mode()&0111 判断任意执行位；Unix 权限位在 Rust 中以 mode() 保留。
            // 只要用户、组或其他任一执行位被置上，就把该 Go 源文件视为需要修正的权限配置。
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                let mode = stat.permissions().mode();
                if mode & 0o111 != 0 {
                    let rendered_mode = formatGoFileMode(mode);
                    pass.Reportf(
                        file_position,
                        &format!(
                            "[{}] source code file should not have execute permission {}",
                            Name, rendered_mode
                        ),
                    );
                }
            }
            // 非 Unix 平台没有同构的 mode 掩码接口，因此这里与 Go 一样默认不额外生成权限诊断。
        }
    }

    Ok(None)
}

// init 对应 Go 的 init 函数：按仓库 linter 配置决定是否跳过该 analyzer。
pub fn init() {
    util::SkipAnalyzerByConfig(&Analyzer);
}
