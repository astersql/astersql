// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// builtin 线程安全代码生成器，对应 Go `expression/generator` 中的 threadsafe 工具。
//
// 扫描 `builtin_*.go`（排除测试），识别 `builtin*Sig` 结构体：
// - 仅嵌入 `baseBuiltinFunc` / `baseBuiltinCastFunc`，或落在 specialSafeFuncs 白名单 → 可跨会话共享；
// - 其余视为不安全，生成恒返回 false 的方法。
// 生成结果为 Go 源码字节，由调用方决定是否写盘。

// 它负责扫描 expression 的 Go 源码、区分可跨会话共享与不可共享的 builtin，并生成对应方法文件。

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tree_sitter::{Node, Parser};

/// 对应 Go 的 specialSafeFuncs：这些签名即使结构体不只嵌入基类，也由维护者显式判定为线程安全。
/// 新增条目时仍应像 Go 注释要求的那样补齐测试用例。
const SPECIAL_SAFE_FUNCS: &[&str] = &[
    "builtinInIntSig",
    "builtinInStringSig",
    "builtinInRealSig",
    "builtinInDecimalSig",
    "builtinInTimeSig",
    "builtinInDurationSig",
    "builtinRealIsTrueSig",
    "builtinDecimalIsTrueSig",
    "builtinIntIsTrueSig",
    "builtinRealIsFalseSig",
    "builtinDecimalIsFalseSig",
    "builtinIntIsFalseSig",
];

/// Go AST 中与本生成器有关的最小类型声明视图。
/// 真正接入时应由 Go 解析器填充字段，而不是在 Rust 侧猜测 Go 语法。
pub struct GoTypeSpec {
    pub name: String,
    pub is_struct: bool,
    pub field_type_names: Vec<String>,
}

/// 读取 AST 节点 UTF-8 文本。
fn node_text<'a>(node: Node<'a>, source: &'a [u8]) -> Result<&'a str, String> {
    node.utf8_text(source).map_err(|err| err.to_string())
}

/// 递归遍历 Go AST，收集普通及别名类型声明（结构体字段类型名列表）。
fn visit_type_specs(
    node: Node<'_>,
    source: &[u8],
    specs: &mut Vec<GoTypeSpec>,
) -> Result<(), String> {
    if matches!(node.kind(), "type_spec" | "type_alias") {
        let name = node
            .child_by_field_name("name")
            .ok_or_else(|| "Go type declaration is missing its name".to_owned())?;
        let declared_type = node
            .child_by_field_name("type")
            .ok_or_else(|| "Go type declaration is missing its type".to_owned())?;
        let mut field_type_names = Vec::new();
        if declared_type.kind() == "struct_type" {
            if let Some(fields) = declared_type
                .named_children(&mut declared_type.walk())
                .find(|child| child.kind() == "field_declaration_list")
            {
                let mut cursor = fields.walk();
                for field in fields
                    .named_children(&mut cursor)
                    .filter(|child| child.kind() == "field_declaration")
                {
                    // Go 仅检查 ast.Field.Type 是否为 *ast.Ident；字段是否命名、
                    // 是否带 tag 不影响 Type，因此必须只读取 tree-sitter 的 type 字段。
                    let type_name = field
                        .child_by_field_name("type")
                        .filter(|field_type| {
                            matches!(field_type.kind(), "type_identifier" | "identifier")
                        })
                        .map(|field_type| node_text(field_type, source))
                        .transpose()?
                        .unwrap_or_default()
                        .to_owned();
                    field_type_names.push(type_name);
                }
            }
        }
        specs.push(GoTypeSpec {
            name: node_text(name, source)?.to_owned(),
            is_struct: declared_type.kind() == "struct_type",
            field_type_names,
        });
        return Ok(());
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit_type_specs(child, source, specs)?;
    }
    Ok(())
}

/// 解析一个 Go 文件并返回其中的非别名类型声明。
fn parse_go_type_specs(file: &Path) -> Result<Vec<GoTypeSpec>, String> {
    let source = fs::read(file).map_err(|err| err.to_string())?;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|err| err.to_string())?;
    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| "tree-sitter failed to parse Go source".to_owned())?;
    if tree.root_node().has_error() {
        return Err(format!("failed to parse Go source: {}", file.display()));
    }
    let mut specs = Vec::new();
    visit_type_specs(tree.root_node(), &source, &mut specs)?;
    Ok(specs)
}

/// 对应 collectThreadSafeBuiltinFuncs：收集 builtin*Sig 结构体，并按字段形状拆分安全与不安全集合。
pub fn collect_thread_safe_builtin_funcs(
    file: &Path,
) -> Result<(Vec<String>, Vec<String>), String> {
    let specs = parse_go_type_specs(file)?;
    let special_safe: HashSet<&str> = SPECIAL_SAFE_FUNCS.iter().copied().collect();
    let mut all_func_names = Vec::with_capacity(32);
    let mut safe_func_names = Vec::new();

    for spec in specs {
        // Go 只考虑名称为 builtin*Sig 的结构体类型，别名或其它声明继续遍历。
        if !spec.name.starts_with("builtin") || !spec.name.ends_with("Sig") || !spec.is_struct {
            continue;
        }
        all_func_names.push(spec.name.clone());

        if special_safe.contains(spec.name.as_str()) {
            safe_func_names.push(spec.name);
            continue;
        }

        // 普通签名只有一个匿名基类字段时才安全；额外状态可能携带会话数据，不能跨会话共享。
        if spec.field_type_names.len() == 1
            && matches!(
                spec.field_type_names[0].as_str(),
                "baseBuiltinFunc" | "baseBuiltinCastFunc"
            )
        {
            safe_func_names.push(spec.name);
        }
    }

    let safe_set: HashSet<&str> = safe_func_names.iter().map(String::as_str).collect();
    let unsafe_func_names = all_func_names
        .into_iter()
        .filter(|name| !safe_set.contains(name.as_str()))
        .collect();
    Ok((safe_func_names, unsafe_func_names))
}

/// 对应 genBuiltinThreadSafeCode：稳定扫描 builtin_*.go（排除测试），聚合后分别生成安全与不安全代码。
pub fn gen_builtin_thread_safe_code(expr_code_dir: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut files = Vec::<PathBuf>::new();
    for entry in std::fs::read_dir(expr_code_dir).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        // os.DirEntry.IsDir only excludes directories. In particular, a symlink
        // whose name matches builtin_*.go is parsed through the link by Go.
        if entry.file_type().map_err(|err| err.to_string())?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("builtin_") && name.ends_with(".go") && !name.contains("_test") {
            files.push(entry.path());
        }
    }
    files.sort();

    let mut safe_funcs = Vec::with_capacity(32);
    let mut unsafe_funcs = Vec::with_capacity(32);
    for file in files {
        let (mut safe, mut unsafe_names) = collect_thread_safe_builtin_funcs(&file)?;
        safe_funcs.append(&mut safe);
        unsafe_funcs.append(&mut unsafe_names);
    }
    safe_funcs.sort();

    let safe = generate_code(&safe_funcs, SAFE_HEADER, SAFE_FUNC_TEMPLATE)?;
    let unsafe_code = generate_code(&unsafe_funcs, UNSAFE_HEADER, UNSAFE_FUNC_TEMPLATE)?;
    Ok((safe, unsafe_code))
}

/// 对应 generateCode：按名称顺序展开模板，再交给 gofmt（等价于 Go format.Source）。
pub fn generate_code(
    func_names: &[String],
    header: &str,
    template: &str,
) -> Result<Vec<u8>, String> {
    let mut buffer = String::from(header);
    for func_name in func_names {
        buffer.push_str(&template.replacen("%s", func_name, 1));
    }
    format_go_source(buffer.as_bytes())
}

fn format_go_source(source: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("gofmt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| err.to_string())?;
    child
        .stdin
        .take()
        .ok_or_else(|| "gofmt stdin was not piped".to_owned())?
        .write_all(source)
        .map_err(|err| err.to_string())?;
    let output = child.wait_with_output().map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// 对应 Go main 的生成入口。
// / 为避免意外覆盖仓库文件，这里只返回两个待写文件及其内容，由显式接线的调用方负责 IO。
pub fn generated_outputs(expr_code_dir: &Path) -> Result<[(PathBuf, Vec<u8>); 2], String> {
    let (safe, unsafe_code) = gen_builtin_thread_safe_code(expr_code_dir)?;
    Ok([
        (expr_code_dir.join("builtin_threadsafe_generated.go"), safe),
        (
            expr_code_dir.join("builtin_threadunsafe_generated.go"),
            unsafe_code,
        ),
    ])
}

/// 保留 Go main 的顺序写盘副作用：先写安全实现，再写不安全实现。
pub fn write_generated_outputs(expr_code_dir: &Path) -> Result<(), String> {
    for (path, source) in generated_outputs(expr_code_dir)? {
        fs::write(path, source).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// 对应 Go main 的当前目录生成入口。
fn main() {
    if let Err(err) = write_generated_outputs(Path::new(".")) {
        panic!("failed to generate builtin thread-safety code: {err}");
    }
}

/// 安全签名的方法模板，对应 Go safeFuncTemp。
pub const SAFE_FUNC_TEMPLATE: &str = r#"// SafeToShareAcrossSession implements BuiltinFunc.SafeToShareAcrossSession.
func (s *%s) SafeToShareAcrossSession() bool {
	return safeToShareAcrossSession(&s.safeToShareAcrossSessionFlag, s.args)
}
"#;

/// 不安全签名的方法模板，对应 Go unsafeFuncTemp。
pub const UNSAFE_FUNC_TEMPLATE: &str = r#"// SafeToShareAcrossSession implements BuiltinFunc.SafeToShareAcrossSession.
func (s *%s) SafeToShareAcrossSession() bool {
	return false
}
"#;

/// 安全输出文件头，对应 Go safeHeader；原子缓存的 0/1/2 状态分别表示未知、安全、不安全。
pub const SAFE_HEADER: &str = r#"// Copyright 2024 PingCAP, Inc.
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

// Code generated by go generate in expression/generator; DO NOT EDIT.

package expression

import "sync/atomic"

func safeToShareAcrossSession(flag *uint32, args []Expression) bool {
	flagV := atomic.LoadUint32(flag)
	if flagV != 0 {
		return flagV == 1
	}

	allArgsSafe := true
	for _, arg := range args {
		if !arg.SafeToShareAcrossSession() {
			allArgsSafe = false
			break
		}
	}
	if allArgsSafe {
		atomic.StoreUint32(flag, 1)
	} else {
		atomic.StoreUint32(flag, 2)
	}
	return allArgsSafe
}

"#;

/// 不安全输出文件头，对应 Go unsafeHeader；这类方法固定返回 false，不需要原子缓存。
pub const UNSAFE_HEADER: &str = r#"// Copyright 2024 PingCAP, Inc.
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

// Code generated by go generate in expression/generator; DO NOT EDIT.

package expression

"#;
