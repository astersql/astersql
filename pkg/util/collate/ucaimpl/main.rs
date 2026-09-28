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

// Unicode CI 校对器实现源码的模板生成器。
//
// 读取 `unicode_ci.go.tpl`，按 `Data.name` / `Data.impl_name` 替换占位符，
// 写出 Go 侧 `unicode_*_ci_generated.go` 风格实现；`main` 按命令行目标文件名分支。

use std::io;
use std::path::Path;

/// 嵌入的 Go 模板正文，运行时做字符串替换而非完整模板引擎。
const UNICODE_CI_IMPL: &str = include_str!("unicode_ci.go.tpl");

/// 模板渲染参数：校对器类型名与具体 Impl 名。
pub struct Data {
    /// 生成代码中的校对器类型名（如 `unicodeCICollator`）。
    pub name: String,
    /// 生成代码中的实现标识（如 `unicode0400Impl`）。
    pub impl_name: String,
}

/// 渲染模板并写入目标路径（覆盖写，语义对齐 Go `os.Create` 截断）。
pub fn generate_file(filename: impl AsRef<Path>, data: &Data) -> io::Result<()> {
    let rendered = render_unicode_ci_template(UNICODE_CI_IMPL, data);
    std::fs::write(filename, rendered)
}

/// 将 `{{.Name}}` / `{{.ImplName}}` 替换为 [`Data`] 字段。
fn render_unicode_ci_template(template: &str, data: &Data) -> String {
    template
        .replace("{{.Name}}", &data.name)
        .replace("{{.ImplName}}", &data.impl_name)
}

/// 命令行入口：按最后一个参数选择 4.0.0 或 9.0.0 生成目标。
pub fn main() {
    let args: Vec<String> = std::env::args().collect();
    // 仅识别两个已知输出文件名；其它参数直接 panic（与迁移基线一致）。
    let result = match args.last().map(String::as_str) {
        Some("unicode_0400_ci_generated.go") => generate_file(
            "unicode_0400_ci_generated.go",
            &Data {
                name: "unicodeCICollator".to_owned(),
                impl_name: "unicode0400Impl".to_owned(),
            },
        ),
        Some("unicode_0900_ai_ci_generated.go") => generate_file(
            "unicode_0900_ai_ci_generated.go",
            &Data {
                name: "unicode0900AICICollator".to_owned(),
                impl_name: "unicode0900Impl".to_owned(),
            },
        ),
        _ => panic!("unreachable"),
    };
    result.expect("write generated unicode collator implementation");
}
