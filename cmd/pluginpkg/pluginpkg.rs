// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

//! Plugin packager matching Go `cmd/pluginpkg/pluginpkg.go`.
//!
//! Reads `manifest.toml`, generates a temporary `*.gen.go` via Go's
//! `text/template` semantics, invokes `go build -buildmode=plugin`, then
//! prints the packaged path and indent-encoded manifest JSON.
//!
//!
//! - 这个模块负责把插件目录里的 `manifest.toml` 转成 Go 插件所需的构建输入。
//! - 它并不直接编译 Rust 代码，而是生成一个临时 `*.gen.go` 文件交给 `go build`。
//! - 生成逻辑需要尽量保持与 Go 原版一致，否则插件元数据和扩展点导出会偏离上游。
//! - Rust 版本把文件系统、命令执行和时间源都做成可注入接口，便于测试覆盖错误路径。
//! - `run_with` 是主流程入口，`main` 只负责接线生产环境实现。
//! - 模板执行器只支持当前模板实际使用到的 `if`、`range` 和字段访问，避免伪造完整模板引擎。
//! - 错误分支继续沿用 Go 版本的“记录日志后立即退出”语义，因此成功前不会做过度清理。
//! - 输出 JSON 时会显式重写缩进前缀，目的是复现 Go `SetIndent(" ", "\t")` 的文本形式。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::stubs::{
    self, Clock, Flags, Fs, OsFs, ProdRunner, Result, Runner, SystemClock, fatal_exit, log_printf,
};

/// Go `codeTemplate` — keep byte-for-byte with `pluginpkg.go` (leading newline).
///
/// - 这里保存要写入临时 `*.gen.go` 的固定模板。
/// - 前导换行也要保留，因为 Go 原常量就是这样定义的。
/// - 模板字段名直接取自 `manifest.toml` 解码后的键。
/// - 任何细节差异都可能导致导出的 `plugin.Manifest` 字段布局变化。
/// - 因此这里只做注释补充，不重排模板文本，避免引入行为偏差。
pub const CODE_TEMPLATE: &str = r#"
package main

import (
	"github.com/pingcap/tidb/pkg/plugin"
)

func PluginManifest() *plugin.Manifest {
	return plugin.ExportManifest(&plugin.{{.kind}}Manifest{
		Manifest: plugin.Manifest{
			Kind:           plugin.{{.kind}},
			Name:           "{{.name}}",
			Description:    "{{.description}}",
			Version:        {{.version}},
			RequireVersion: map[string]uint16{},
			License:        "{{.license}}",
			BuildTime:      "{{.buildTime}}",
			{{if .validate }}
				Validate:   {{.validate}},
			{{end}}
			{{if .onInit }}
				OnInit:     {{.onInit}},
			{{end}}
			{{if .onShutdown }}
				OnShutdown: {{.onShutdown}},
			{{end}}
			{{if .onFlush }}
				OnFlush:    {{.onFlush}},
			{{end}}
		},
		{{range .export}}
		{{.extPoint}}: {{.impl}},
		{{end}}
	})
}
"#;

// Package-level flags (Go `var` block); overwritten by flag parse in `main`.
//
// - Go 版本使用包级变量承接 `flag` 解析结果。
// - Rust 没有同样的全局可变写法，这里用 `thread_local!` 模拟同等生命周期。
// - 之所以不用进程级静态可变，是为了避免额外的 `unsafe` 和跨测试污染。
// - `main` 解析完参数后会覆盖这些默认值，保持与 Go `init + flag.Parse` 的顺序一致。
thread_local! {
    static PKG_DIR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static OUT_DIR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static PGO_FILE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static NEXT_GEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Go `init` — register CLI flags (defaults match Go empty / false).
///
/// - 这里重置线程局部 flag 状态，等价于 Go 包初始化后的默认值。
/// - 每次入口执行前都先调用，避免测试之间残留上一次的参数。
/// - 默认值必须保持空字符串和 `false`，这样 `usage` 判定才与 Go 一致。
pub fn init_flags() {
    PKG_DIR.with(|c| *c.borrow_mut() = String::new());
    OUT_DIR.with(|c| *c.borrow_mut() = String::new());
    PGO_FILE.with(|c| *c.borrow_mut() = String::new());
    NEXT_GEN.with(|c| c.set(false));
}

//
// - `run_with` 使用显式 `Flags` 参数驱动逻辑，但部分辅助函数仍按 Go 习惯从全局读取。
// - 这里先把解析结果写回线程局部，保证后续读取和帮助输出看到的是同一份状态。
fn set_flags(flags: &Flags) {
    PKG_DIR.with(|c| *c.borrow_mut() = flags.pkg_dir.clone());
    OUT_DIR.with(|c| *c.borrow_mut() = flags.out_dir.clone());
    PGO_FILE.with(|c| *c.borrow_mut() = flags.pgo_file.clone());
    NEXT_GEN.with(|c| c.set(flags.next_gen));
}

//
// - 从线程局部还原出一份独立 `Flags`，避免借用跨越整个主流程。
// - 返回拷贝后，后续路径归一化可以直接覆盖结构体字段，而不会影响读取实现。
fn get_flags() -> Flags {
    Flags {
        pkg_dir: PKG_DIR.with(|c| c.borrow().clone()),
        out_dir: OUT_DIR.with(|c| c.borrow().clone()),
        pgo_file: PGO_FILE.with(|c| c.borrow().clone()),
        next_gen: NEXT_GEN.with(|c| c.get()),
    }
}

/// Go `usage` — print help and exit 1.
///
/// - 帮助文本既要显示自定义用法，也要显示每个 flag 的默认值。
/// - 这里不直接返回 `Result`，而是像 Go `flag.Usage` 一样最终终止进程。
/// - 这样可以保持错误路径输出顺序和退出码约定不变。
pub fn usage(log: &mut dyn Write, argv0: &str) -> ! {
    let base = Path::new(argv0)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| argv0.to_string());
    log_printf(
        log,
        format_args!(
            "Usage: {base} --pkg-dir [plugin source pkg folder] --out-dir [plugin packaged folder path]"
        ),
    );
    //
    // - Go 版本这里调用 `flag.PrintDefaults()`。
    // - Rust 版本没有复刻完整 flag 包，因此直接按注册顺序打印默认项。
    // - 文本保持与 Go 选项名一致，避免用户文档和自动化脚本出现偏差。
    // Go also calls flag.PrintDefaults(); we emit the registered defaults.
    let _ = writeln!(
        log,
        "  -next-gen\n    \twhether to build plugin with next-gen features"
    );
    let _ = writeln!(log, "  -out-dir string\n    \tplugin packaged folder path");
    let _ = writeln!(
        log,
        "  -pgo-file string\n    \tgo profile-guided optimization(pgo) file path"
    );
    let _ = writeln!(log, "  -pkg-dir string\n    \tplugin package folder path");
    fatal_exit("usage");
}

/// Decode `manifest.toml` into a JSON-compatible object (Go `map[string]any`).
///
/// - Go 原版把 TOML 解到 `map[string]any`，后续模板和 JSON 编码都基于动态键访问。
/// - Rust 这里先交给 `toml` crate 解析，再统一转成 `serde_json::Map`。
/// - 这样能复用同一份动态数据给模板执行器和最终清单输出。
pub fn decode_manifest_toml(text: &str) -> Result<serde_json::Map<String, JsonValue>> {
    let value: toml::Value =
        toml::from_str(text).map_err(|e| stubs::Error::new(format!("toml decode: {e}")))?;
    let json = toml_to_json(value);
    match json {
        JsonValue::Object(map) => Ok(map),
        _ => Err(stubs::Error::new("manifest root must be a table")),
    }
}

//
// - 递归把 TOML 值映射成 JSON 值，是为了模拟 Go `map[string]any` 的宽松结构。
// - 这里保留数组、表和标量层次，不提前约束 manifest 模式。
// - 浮点数如果无法表达成 JSON number，会退回 0，与“尽量生成可打印结果”的思路一致。
// - 该函数只做表示层转换，不负责校验必填字段。
fn toml_to_json(v: toml::Value) -> JsonValue {
    match v {
        toml::Value::String(s) => JsonValue::String(s),
        toml::Value::Integer(i) => JsonValue::Number(i.into()),
        toml::Value::Float(f) => JsonValue::Number(
            serde_json::Number::from_f64(f).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        toml::Value::Boolean(b) => JsonValue::Bool(b),
        toml::Value::Datetime(d) => JsonValue::String(d.to_string()),
        toml::Value::Array(arr) => JsonValue::Array(arr.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => {
            let mut map = serde_json::Map::new();
            for (k, v) in table {
                map.insert(k, toml_to_json(v));
            }
            JsonValue::Object(map)
        }
    }
}

/// Truthiness matching Go `text/template` `if` on interface{} values.
///
/// - Go 模板里的 `if` 会对不同动态类型做真值判断。
/// - 这里按当前 manifest 会出现的 JSON 值复刻该语义。
/// - 空字符串、零值、空数组和空对象都视为假，避免把未配置字段错误输出到模板里。
pub fn template_truthy(v: Option<&JsonValue>) -> bool {
    match v {
        None => false,
        Some(JsonValue::Null) => false,
        Some(JsonValue::Bool(b)) => *b,
        Some(JsonValue::String(s)) => !s.is_empty(),
        Some(JsonValue::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(JsonValue::Array(a)) => !a.is_empty(),
        Some(JsonValue::Object(o)) => !o.is_empty(),
    }
}

/// Render Go `text/template` for the fixed `CODE_TEMPLATE` against a manifest map.
///
/// - 外部只暴露固定模板的执行入口，调用方不需要知道简化模板引擎的内部细节。
/// - 这样可以把与 Go 对齐的范围限定在 `CODE_TEMPLATE`，减少误用其他模板语法的风险。
/// - 出错时返回统一错误，让主流程继续沿用 Go 的日志文案。
pub fn execute_code_template(manifest: &serde_json::Map<String, JsonValue>) -> Result<String> {
    let mut out = String::new();
    execute_go_template(CODE_TEMPLATE, manifest, None, &mut out)?;
    Ok(out)
}

//
// - 这是一个只覆盖当前模板子集的解释器，而不是完整的 Go `text/template` 实现。
// - 支持的动作只有三类：`if`、`range` 和字段访问。
// - `range_item` 表示当前 `range` 循环项；进入循环后 `.` 完全绑定到该项。
// - 循环项缺少字段时返回 `<no value>`，不能回退到根 manifest。
// - 逐字节扫描可以避免引入更大的模板依赖，也更容易精确复制当前模板行为。
// - 一旦遇到未支持语法，立即报错而不是默默忽略，防止生成错误插件源码。
fn execute_go_template(
    tmpl: &str,
    data: &serde_json::Map<String, JsonValue>,
    range_item: Option<&JsonValue>,
    out: &mut String,
) -> Result<()> {
    let bytes = tmpl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let end = find_action_end(bytes, i + 2)?;
            let action = std::str::from_utf8(&bytes[i + 2..end])
                .map_err(|e| stubs::Error::new(format!("template utf8: {e}")))?
                .trim();
            //
            // - 先把动作内容切出来，再把游标移动到 `}}` 后面。
            // - 之后各类分支只关心模板语义，不再重复处理定界符。
            i = end + 2; // skip `}}`

            if let Some(rest) = action.strip_prefix("if ") {
                let (body, new_i) = take_until_end(bytes, i, "if")?;
                i = new_i;
                let key = rest.trim().trim_start_matches('.');
                let val = lookup_field(data, range_item, key)?;
                if template_truthy(val) {
                    //
                    // - 只有真值字段才递归渲染分支体。
                    // - 这样才能复现模板中可选回调函数不存在时整段字段被省略的行为。
                    execute_go_template(body, data, range_item, out)?;
                }
                continue;
            }
            if let Some(rest) = action.strip_prefix("range ") {
                let (body, new_i) = take_until_end(bytes, i, "range")?;
                i = new_i;
                let key = rest.trim().trim_start_matches('.');
                let val = lookup_field(data, range_item, key)?;
                match val {
                    None | Some(JsonValue::Null) => {}
                    Some(JsonValue::Array(arr)) => {
                        for item in arr {
                            execute_go_template(body, data, Some(item), out)?;
                        }
                    }
                    Some(JsonValue::Object(map)) => {
                        // Go text/template iterates maps in sorted key order when the
                        // key type has a defined order. serde_json maps provide the
                        // same deterministic value sequence here.
                        for item in map.values() {
                            execute_go_template(body, data, Some(item), out)?;
                        }
                    }
                    Some(JsonValue::Number(number)) => {
                        // Go 1.25 text/template accepts integer range values. TOML
                        // integers are signed, but retain the unsigned branch for
                        // callers constructing a serde_json map directly.
                        if let Some(count) = number.as_i64() {
                            for index in 0..count.max(0) {
                                let item = JsonValue::Number(index.into());
                                execute_go_template(body, data, Some(&item), out)?;
                            }
                        } else if let Some(count) = number.as_u64() {
                            for index in 0..count {
                                let item = JsonValue::Number(index.into());
                                execute_go_template(body, data, Some(&item), out)?;
                            }
                        } else {
                            return Err(stubs::Error::new(format!(
                                "range can't iterate over {}",
                                format_field(val)
                            )));
                        }
                    }
                    Some(_) => {
                        return Err(stubs::Error::new(format!(
                            "range can't iterate over {}",
                            format_field(val)
                        )));
                    }
                }
                continue;
            }
            if action == "end" {
                return Err(stubs::Error::new("unexpected {{end}}"));
            }
            //
            // - 当前模板只使用最简单的字段输出，没有函数管道或格式化调用。
            // - 因此拿到字段值后直接按 Go `fmt` 风格转成字符串即可。
            // Field pipeline: {{.name}} / {{.extPoint}}
            if let Some(field) = action.strip_prefix('.') {
                let val = lookup_field(data, range_item, field)?;
                out.push_str(&format_field(val));
                continue;
            }
            return Err(stubs::Error::new(format!(
                "unsupported template action: {action}"
            )));
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    Ok(())
}

//
// - 在线性扫描中定位动作结束的 `}}`。
// - 模板语法本身不支持在动作里嵌套 `{{`，所以简单扫描即可满足这里的模板。
fn find_action_end(bytes: &[u8], start: usize) -> Result<usize> {
    let mut j = start;
    while j + 1 < bytes.len() {
        if bytes[j] == b'}' && bytes[j + 1] == b'}' {
            return Ok(j);
        }
        j += 1;
    }
    Err(stubs::Error::new("unclosed template action"))
}

//
// - 抽取 `if` 或 `range` 的主体，同时正确处理同类和异类控制块嵌套。
// - `depth` 模拟 Go 模板解析器的栈深度，直到碰到匹配的 `{{end}}` 才返回。
// - 返回值里的 `usize` 是调用方继续扫描的位置，已经越过闭合 `end`。
// - 这样外层解释器不需要回看正文内容，保持单向流式处理。
fn take_until_end<'a>(bytes: &'a [u8], mut i: usize, kind: &str) -> Result<(&'a str, usize)> {
    let start = i;
    let mut depth = 1;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let end = find_action_end(bytes, i + 2)?;
            let action = std::str::from_utf8(&bytes[i + 2..end])
                .map_err(|e| stubs::Error::new(format!("template utf8: {e}")))?
                .trim();
            if action.starts_with("if ") || action.starts_with("range ") {
                depth += 1;
            } else if action == "end" {
                depth -= 1;
                if depth == 0 {
                    let body = std::str::from_utf8(&bytes[start..i])
                        .map_err(|e| stubs::Error::new(format!("template utf8: {e}")))?;
                    return Ok((body, end + 2));
                }
            }
            i = end + 2;
            continue;
        }
        i += 1;
    }
    Err(stubs::Error::new(format!("unclosed {{{{{kind}}}}} block")))
}

//
// - 字段解析在 `range` 外读取根 manifest，在 `range` 内只读取当前项。
// - 这复现 Go 模板对 `.` 的重新绑定，避免循环项缺字段时误读根级同名键。
fn lookup_field<'a>(
    data: &'a serde_json::Map<String, JsonValue>,
    range_item: Option<&'a JsonValue>,
    key: &str,
) -> Result<Option<&'a JsonValue>> {
    if let Some(item) = range_item {
        return match item {
            JsonValue::Object(map) => Ok(map.get(key)),
            JsonValue::Null => Err(stubs::Error::new(format!(
                "nil pointer evaluating interface {{}}.{key}"
            ))),
            other => Err(stubs::Error::new(format!(
                "can't evaluate field {key} in type {}",
                go_dynamic_type(Some(other))
            ))),
        };
    }
    Ok(data.get(key))
}

fn go_dynamic_type(value: Option<&JsonValue>) -> &'static str {
    match value {
        None | Some(JsonValue::Null) => "nil",
        Some(JsonValue::Bool(_)) => "bool",
        Some(JsonValue::String(_)) => "string",
        Some(JsonValue::Number(number)) if number.is_i64() => "int64",
        Some(JsonValue::Number(number)) if number.is_u64() => "uint64",
        Some(JsonValue::Number(_)) => "float64",
        Some(JsonValue::Array(_)) => "[]interface {}",
        Some(JsonValue::Object(_)) => "map[string]interface {}",
    }
}

fn assert_manifest_string(manifest: &serde_json::Map<String, JsonValue>, key: &str) -> String {
    match manifest.get(key) {
        Some(JsonValue::String(value)) => value.clone(),
        value => panic!(
            "interface conversion: interface {{}} is {}, not string",
            go_dynamic_type(value)
        ),
    }
}

fn format_go_value(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "<nil>".to_string(),
        JsonValue::String(value) => value.clone(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Number(value) if value.is_f64() => value
            .as_f64()
            .map(|number| number.to_string())
            .unwrap_or_else(|| value.to_string()),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(format_go_value)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        JsonValue::Object(values) => format!(
            "map[{}]",
            values
                .iter()
                .map(|(key, value)| format!("{key}:{}", format_go_value(value)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

fn render_code_template(
    manifest: &serde_json::Map<String, JsonValue>,
    out: &mut String,
) -> Result<()> {
    execute_go_template(CODE_TEMPLATE, manifest, None, out)
}

fn write_generated_source(fs: &dyn Fs, gen_file_name: &str, source: &str, log: &mut dyn Write) {
    if let Err(err) = fs.write(gen_file_name, source.as_bytes(), 0o700) {
        log_printf(
            log,
            format_args!("generate code failure during generating code, {err:?}\n"),
        );
        fatal_exit("template execute");
    }
}

fn persist_partial_source(fs: &dyn Fs, gen_file_name: &str, source: &str) {
    if !source.is_empty() {
        let _ = fs.write(gen_file_name, source.as_bytes(), 0o700);
    }
}

// Go text/template uses fmt-style presentation for dynamic field values rather
// than JSON notation. Missing map keys keep the distinct `<no value>` marker.
fn format_field(v: Option<&JsonValue>) -> String {
    match v {
        None => "<no value>".to_string(),
        Some(value) => format_go_value(value),
    }
}

/// Build `go build` argv matching Go `flags` construction.
///
/// - 这里集中拼装 `go build` 参数，确保测试可以只校验参数顺序和内容。
/// - 输出文件名规则必须与 Go 相同：`<plugin>-<version>.so`。
/// - `codes` 标签始终开启，`nextgen` 只在显式请求时追加。
/// - `pgo` 参数可选，缺省时完全省略，避免把空值传给 Go 工具链。
pub fn build_go_flags(
    pkg_dir: &str,
    out_dir: &str,
    plugin_name: &str,
    version: &str,
    pgo_file: &str,
    next_gen: bool,
    fs: &dyn Fs,
) -> (String, Vec<String>) {
    let output_file = fs.join(out_dir, &format!("{plugin_name}-{version}.so"));
    let mut flags: Vec<String> = Vec::with_capacity(6);
    flags.push("build".to_string());
    if !pgo_file.is_empty() {
        flags.push(format!("-pgo={pgo_file}"));
    }
    let mut build_tags = vec!["codes".to_string()];
    if next_gen {
        build_tags.push("nextgen".to_string());
    }
    flags.push(format!("-tags={}", build_tags.join(",")));
    flags.push("-buildmode=plugin".to_string());
    flags.push("-o".to_string());
    flags.push(output_file.clone());
    flags.push(pkg_dir.to_string());
    (output_file, flags)
}

/// Core packaging pipeline (injectable FS / runner / clock / writers).
///
/// Matches Go `main` control flow. Error paths call `fatal_exit` like Go
/// `os.Exit(1)` (deferred cleanup does **not** run — gen file left behind).
///
/// - 这是文件的核心入口，把原本耦合在 `main` 中的行为拆成可注入依赖。
/// - `Fs` 负责路径和文件操作，`Runner` 负责调用 `go build`，`Clock` 提供构建时间。
/// - 所有错误都尽量沿用 Go 版本的日志文字，避免行为对比时出现噪声。
/// - 成功路径会删除临时生成文件，失败路径则刻意保留，以匹配 Go `os.Exit` 跳过 `defer`。
/// - `stdout` 和 `log` 分离是为了复刻 Go 中标准输出与标准错误分流的效果。
pub fn run_with(
    flags: Flags,
    fs: &dyn Fs,
    runner: &dyn Runner,
    clock: &dyn Clock,
    log: &mut dyn Write,
    stdout: &mut dyn Write,
    argv0: &str,
) {
    set_flags(&flags);
    let mut flags = get_flags();

    //
    // - 两个目录参数都是必填；缺任意一个都直接走帮助分支。
    // - 这里保持与 Go `flag.Usage()` 相同的控制流，而不是返回普通错误。
    if flags.pkg_dir.is_empty() || flags.out_dir.is_empty() {
        usage(log, argv0);
    }

    //
    // - 先把输入路径归一化为绝对路径，后续日志和输出文件路径都基于归一化结果。
    // - 一旦绝对化失败，继续执行没有意义，因此直接打印原因并退到 `usage`。
    flags.pkg_dir = match fs.abs(&flags.pkg_dir) {
        Ok(p) => p,
        Err(err) => {
            log_printf(
                log,
                format_args!("unable to resolve absolute representation of package path , {err:?}"),
            );
            usage(log, argv0);
        }
    };
    flags.out_dir = match fs.abs(&flags.out_dir) {
        Ok(p) => p,
        Err(err) => {
            log_printf(
                log,
                format_args!("unable to resolve absolute representation of output path , {err:?}"),
            );
            usage(log, argv0);
        }
    };
    if !flags.pgo_file.is_empty() {
        //
        // - `pgo-file` 是可选参数，所以只有用户提供时才做绝对路径解析。
        // - 这样既保留 Go 行为，也避免把空字符串误判成当前目录。
        flags.pgo_file = match fs.abs(&flags.pgo_file) {
            Ok(p) => p,
            Err(err) => {
                log_printf(
                    log,
                    format_args!(
                        "unable to resolve absolute representation of pgo-file path , {err:?}"
                    ),
                );
                usage(log, argv0);
            }
        };
    }

    let manifest_path = fs.join(&flags.pkg_dir, "manifest.toml");
    //
    // - manifest 是插件元数据的唯一入口，读失败就无法继续生成源码。
    // - 错误信息带上 `pkg_dir`，便于在批量打包场景定位具体插件目录。
    let manifest_text = match fs.read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(err) => {
            log_printf(
                log,
                format_args!("read pkg {}'s manifest failure, {err:?}\n", flags.pkg_dir),
            );
            fatal_exit("manifest read");
        }
    };
    let mut manifest = match decode_manifest_toml(&manifest_text) {
        Ok(m) => m,
        Err(err) => {
            log_printf(
                log,
                format_args!("read pkg {}'s manifest failure, {err:?}\n", flags.pkg_dir),
            );
            fatal_exit("manifest decode");
        }
    };
    manifest.insert(
        "buildTime".to_string(),
        JsonValue::String(clock.now_string()),
    );
    //
    // - 构建时间在 Go 原版中由 `time.Now().String()` 写入 manifest。
    // - Rust 版本通过 `Clock` 注入，既可测试，也能保持最终模板字段存在。

    //
    // - 插件名必须同时来自 manifest 和目录名，二者不一致时直接拒绝构建。
    // - 这是上游工具约束：输出的插件 identity 依赖目录和 manifest 双重一致。
    let plugin_name = assert_manifest_string(&manifest, "name");
    if plugin_name != fs.base(&flags.pkg_dir) {
        log_printf(
            log,
            format_args!("plugin package must be same with plugin name in manifest file\n"),
        );
        fatal_exit("name mismatch");
    }

    let version = assert_manifest_string(&manifest, "version");

    let gen_file_name = fs.join(
        &flags.pkg_dir,
        &format!("{}.gen.go", fs.base(&flags.pkg_dir)),
    );
    //
    // - 临时文件名与 Go 一样取 `<目录名>.gen.go`，放在插件包目录下。
    // - 权限 `0700` 也保持一致，避免生成文件权限在安全检查或对比脚本里出现偏差。
    // Go: os.OpenFile(..., O_RDWR|O_CREATE|O_TRUNC, 0700)
    if let Err(err) = fs.write(&gen_file_name, &[], 0o700) {
        log_printf(
            log,
            format_args!("generate code failure during prepare output file, {err:?}\n"),
        );
        fatal_exit("gen open");
    }

    // NOTE: Go registers `defer os.Remove(genFileName)` here. Because all
    // subsequent error paths call `os.Exit`, deferred cleanup only runs on
    // the successful return path. We mirror that: remove only after success.
    //
    // - 这段语义很关键：失败时保留临时文件不是疏忽，而是与 Go `defer` + `os.Exit` 对齐。
    // - 这样排查模板生成失败或编译失败时，用户还能直接检查生成的 `.gen.go` 内容。

    // Go executes the template directly into the already-open file. Render into
    // an accumulating buffer so a template error can still leave the same partial
    // diagnostic source behind before the os.Exit-equivalent path is taken.
    let mut gen_src = String::new();
    if let Err(err) = render_code_template(&manifest, &mut gen_src) {
        persist_partial_source(fs, &gen_file_name, &gen_src);
        log_printf(
            log,
            format_args!("generate code failure during generating code, {err:?}\n"),
        );
        fatal_exit("template execute");
    }
    write_generated_source(fs, &gen_file_name, &gen_src, log);

    let (output_file, go_flags) = build_go_flags(
        &flags.pkg_dir,
        &flags.out_dir,
        &plugin_name,
        &version,
        &flags.pgo_file,
        flags.next_gen,
        fs,
    );

    //
    // - Go 命令固定追加 `GO111MODULE=on`，确保插件构建走模块模式。
    // - 其余环境变量沿用调用方进程环境，避免破坏现有 Go 工具链配置。
    // Go: append(os.Environ(), "GO111MODULE=on") — runner inherits env; we pass extra.
    let env_extra = vec!["GO111MODULE=on".to_string()];
    if let Err(err) = runner.run("go", &go_flags, &flags.pkg_dir, &env_extra) {
        log_printf(
            log,
            format_args!("compile plugin source code failure, {err:?}\n"),
        );
        // os.Exit(1) — gen file NOT removed (Go defer skipped).
        fatal_exit("go build");
    }

    let _ = write!(
        stdout,
        "Package \"{}\" as plugin \"{}\" success.\nManifest:\n",
        flags.pkg_dir, output_file
    );
    if let Err(err) = encode_manifest_json(stdout, &manifest) {
        log_printf(
            log,
            format_args!("print manifest detail failure, err: {err}"),
        );
    }

    // Go's deferred removal runs only as main returns, after the success banner,
    // manifest encoding, and any JSON error log have completed.
    if let Err(err1) = fs.remove(&gen_file_name) {
        log_printf(
            log,
            format_args!(
                "remove tmp file {gen_file_name} failure, please clean up manually at {err1:?}"
            ),
        );
    }
}

/// JSON encode matching Go `json.NewEncoder.SetIndent(" ", "\t")`.
///
/// - 目标不是生成“好看”的 JSON，而是尽量贴近 Go `Encoder` 的缩进文本。
/// - Rust 标准库没有完全同构的格式器，所以这里分两步：先 pretty-print，再重写每行前缀。
/// - 同时使用稳定键序，避免测试受 map 迭代顺序影响。
pub fn encode_manifest_json(
    out: &mut dyn Write,
    manifest: &serde_json::Map<String, JsonValue>,
) -> std::io::Result<()> {
    //
    // - Go map 输出顺序本来不稳定，但 Rust 测试更适合断言确定文本。
    // - 因此这里只在 Rust 端做稳定排序，不影响插件构建逻辑本身。
    // Stable key order for deterministic tests; Go map order is random.
    let ordered: BTreeMap<_, _> = manifest
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let value = JsonValue::Object(ordered.into_iter().collect());
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    //
    // - `PrettyFormatter` 只能控制缩进字符，不能直接表达 Go 的“单空格前缀 + tab 缩进”。
    // - 所以先得到纯 tab 缩进的 JSON，再手工重写每行。
    // Go prefix is a single space on each indented line; PrettyFormatter only
    // controls indent width. We emit tab-indent JSON then prefix lines with
    // a space to match `SetIndent(" ", "\t")`.
    {
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
        serde::Serialize::serialize(&value, &mut ser)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    }
    //
    // - Go `Encode` 总会补一个换行，serde_json 默认不会。
    // - 这里显式补齐，避免最终 stdout 文本少最后一行换行。
    // serde_json pretty does not add trailing newline; Go Encode does.
    if !buf.ends_with(b"\n") {
        buf.push(b'\n');
    }
    //
    // - 重写时保留首行 `{` 原样，其余行在原缩进前再加一个空格。
    // - 这与 Go `SetIndent(" ", "\t")` 的常见输出形式一致。
    // Prefix every line (including `{`) with a single space? Go's encoder:
    //   prefix is written before each indent level's content after newline.
    // Practical form:
    // {\n \t"k": ...\n }
    // Rewrite lines to add the Go prefix space after each newline when indented.
    let s = String::from_utf8_lossy(&buf)
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let mut rewritten = String::new();
    for (idx, line) in s.lines().enumerate() {
        if idx > 0 {
            rewritten.push('\n');
        }
        if line.is_empty() {
            continue;
        }
        //
        // - 根行不加前缀，后续所有非空行统一补一个空格。
        // - 关闭分支上的 `}` 也要补空格，否则与 Go 输出的视觉层级不同。
        // Non-root lines in Go get prefix (" ") before their indent tabs.
        if idx == 0 {
            rewritten.push_str(line);
        } else if line.starts_with('\t') || line.starts_with('}') {
            rewritten.push(' ');
            rewritten.push_str(line);
        } else {
            rewritten.push(' ');
            rewritten.push_str(line);
        }
    }
    rewritten.push('\n');
    out.write_all(rewritten.as_bytes())
}

/// Production entrypoint.
///
/// - 生产入口只负责解析环境中的参数并装配真实依赖。
/// - 真正的业务逻辑仍放在 `run_with`，这样测试可以跳过进程环境直接调用核心流程。
pub fn main() {
    init_flags();
    let args = stubs::args_from_env();
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "pluginpkg".into());
    let flags = match stubs::try_parse_flags(&args) {
        Ok(flags) => flags,
        Err(message) => {
            let mut log = std::io::stderr();
            if !message.is_empty() {
                log_printf(&mut log, format_args!("{message}"));
            }
            usage(&mut log, &argv0);
        }
    };
    let fs = OsFs;
    let runner = ProdRunner;
    let clock = SystemClock;
    let mut log = std::io::stderr();
    let mut stdout = std::io::stdout();
    run_with(flags, &fs, &runner, &clock, &mut log, &mut stdout, &argv0);
}
