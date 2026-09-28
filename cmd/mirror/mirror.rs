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

//! 本模块把 Go module 依赖转换成 Bazel `go_repository` 定义。
//! 主流程沿用 Go 版本的四步顺序：建临时目录、列举模块、下载 zip、输出 `deps.bzl`。
//! 之所以维持这个顺序，是为了让输出、错误与清理语义都能直接对齐 Go。
//! `Bazel`、`Runner` 与 `Fs` 三个边界让生产实现和测试桩共享同一流程。
//! 因此 parity test 不需要真实 runfiles、真实 `go` 命令或真实磁盘目录。
//! 模块路径到 Bazel 仓库名的映射是本命令最关键的稳定契约之一。
//! 一旦命名规则漂移，仓库里已有的依赖声明和补丁文件都会立刻失效。
//! `replace` 语义同样敏感，因为本地 replace 与远端 replace 的处理方式不同。
//! 生成 `deps.bzl` 时的排序规则也不能随意更改，否则每次运行都会制造噪声 diff。
//! 补丁参数、特殊 build tag 与命名约定都来自 Go 版本的显式例外列表。
//! 这里不试图做更多推断，避免在迁移后引入新的隐式规则。
//! 清理临时目录使用 Drop 守卫，是 Rust 对 Go `defer` 的直接对应。
//! 守卫的价值不在“更优雅”，而在于覆盖成功和失败两类退出路径。
//! 对调用者来说，最重要的外观是 stdout 生成内容与 stderr/panic 包装。
//! 因此错误处理中会专门保留 `exec.ExitError` 风格的 stderr 透传。
//! 注释会重点解释这些外部契约与决策顺序，而不是重复描述语法。
//! 本次改动不调整任何命令参数、环境变量叠加顺序或输出模板。
//! 如果未来要扩展更多仓库例外，应优先补到显式分支与注释中。
//! 这样维护者才能一眼判断某个特判是历史兼容还是新的需求。
//! 理解这一点对于避免无意间破坏生成结果尤为重要。

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;

use crate::stubs::{self, Bazel, Error, Flags, Fs, OsFs, ProdBazel, ProdRunner, Result, Runner};

/// downloadedModule captures `go mod download -json` output.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
/// `DownloadedModule` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct DownloadedModule {
    #[serde(default, rename = "Path")]
    pub Path: String,
    #[serde(default, rename = "Sum")]
    pub Sum: String,
    #[serde(default, rename = "Version")]
    pub Version: String,
    #[serde(default, rename = "Zip")]
    pub Zip: String,
}

/// listedModule captures `go list -m -json` output.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
/// `ListedModule` 承担当前模块中的一组状态或配置。
/// 字段设计以 Go 对齐所需的最小信息为边界，不额外扩展职责。
/// 理解它的作用有助于看清后续流程为何只读取这些字段。
pub struct ListedModule {
    #[serde(default, rename = "Path")]
    pub Path: String,
    #[serde(default, rename = "Version")]
    pub Version: String,
    #[serde(default, rename = "Replace")]
    pub Replace: Option<Box<ListedModule>>,
}

// Package-level flags (Go `var` block); overwritten by flag parse in `main`.
thread_local! {
    static IS_MIRROR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static IS_UPLOAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Go `init` — register deprecated flags (no-op beyond documenting defaults).
/// `init_flags` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn init_flags() {
    IS_MIRROR.with(|c| c.set(false));
    IS_UPLOAD.with(|c| c.set(false));
}

// `set_flags` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn set_flags(flags: &Flags) {
    IS_MIRROR.with(|c| c.set(flags.is_mirror));
    IS_UPLOAD.with(|c| c.set(flags.is_upload));
}

/// Go `copyFile`.
/// `copy_file` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn copy_file(fs: &dyn Fs, src: &str, dst: &str) -> Result<()> {
    fs.copy_file(src, dst)
}

/// Go `createTmpDir` — prepare tmpdir and copy root/parser go.mod/go.sum.
/// `create_tmp_dir` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn create_tmp_dir(bazel: &dyn Bazel, fs: &dyn Fs) -> Result<String> {
    let tmpdir = bazel.NewTmpDir("gomirror")?;
    fs.mkdir_all(&join_path(&tmpdir, "pkg/parser"))?;
    let gomod = bazel.Runfile("go.mod")?;
    let gosum = bazel.Runfile("go.sum")?;
    // Go: strings.Replace(gomod, "go.mod", "pkg/parser/go.mod", 1)
    let parsergomod = replace_once(&gomod, "go.mod", "pkg/parser/go.mod");
    let parsergosum = replace_once(&gosum, "go.sum", "pkg/parser/go.sum");
    copy_file(fs, &gomod, &join_path(&tmpdir, "go.mod"))?;
    copy_file(fs, &parsergomod, &join_path(&tmpdir, "pkg/parser/go.mod"))?;
    copy_file(fs, &gosum, &join_path(&tmpdir, "go.sum"))?;
    copy_file(fs, &parsergosum, &join_path(&tmpdir, "pkg/parser/go.sum"))?;
    Ok(tmpdir)
}

/// Go `downloadZips`.
/// `download_zips` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn download_zips(
    bazel: &dyn Bazel,
    runner: &dyn Runner,
    tmpdir: &str,
    listed: &HashMap<String, ListedModule>,
) -> Result<HashMap<String, DownloadedModule>> {
    let gobin = bazel.Runfile("bin/go")?;
    let mut download_args: Vec<String> = Vec::with_capacity(listed.len() + 3);
    download_args.push("mod".to_string());
    download_args.push("download".to_string());
    download_args.push("-json".to_string());
    for mod_ in listed.values() {
        if let Some(replace) = &mod_.Replace {
            // 本地 replace 没有版本号，Go 版本会直接跳过，
            // 否则 `go mod download` 会把本地路径误当成远端模块坐标。
            if replace.Version.is_empty() {
                continue;
            }
            download_args.push(format!("{}@{}", replace.Path, replace.Version));
        } else {
            download_args.push(format!("{}@{}", mod_.Path, mod_.Version));
        }
    }
    let env = command_env_with_gosumdb();
    let json_bytes = runner.output(&gobin, &download_args, tmpdir, &env)?;
    parse_downloaded_modules(&json_bytes)
}

/// Go `listAllModules`.
/// `list_all_modules` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn list_all_modules(
    bazel: &dyn Bazel,
    runner: &dyn Runner,
    tmpdir: &str,
) -> Result<HashMap<String, ListedModule>> {
    let gobin = bazel.Runfile("bin/go")?;
    let args = vec![
        "list".to_string(),
        "-mod=readonly".to_string(),
        "-m".to_string(),
        "-json".to_string(),
        "all".to_string(),
    ];
    let env = command_env_with_gosumdb();
    let json_bytes = runner.output(&gobin, &args, tmpdir, &env)?;
    parse_listed_modules(&json_bytes)
}

// `command_env_with_gosumdb` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn command_env_with_gosumdb() -> Vec<String> {
    // Go starts from os.Environ() then appends GOSUMDB=sum.golang.org.
    // ProdRunner applies these as env overlays; ScriptedEnv records them.
    let mut env: Vec<String> = std::env::vars().map(|(k, v)| format!("{k}={v}")).collect();
    env.push("GOSUMDB=sum.golang.org".to_string());
    env
}

/// Parse concatenated JSON objects the way Go's line+`}` stream does.
/// `parse_listed_modules` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_listed_modules(json_bytes: &[u8]) -> Result<HashMap<String, ListedModule>> {
    let mut ret = HashMap::new();
    for chunk in split_json_objects(json_bytes) {
        let mod_: ListedModule =
            serde_json::from_slice(chunk.as_bytes()).map_err(|e| Error::new(e.to_string()))?;
        // 主仓库自身不应写回 `deps.bzl`，这里保持与 Go 相同的过滤点，
        // 避免生成对当前仓库的自引用仓库规则。
        if mod_.Path == "github.com/pingcap/tidb" {
            continue;
        }
        ret.insert(mod_.Path.clone(), mod_);
    }
    Ok(ret)
}

/// Parse `go mod download -json` stream into path → module map.
/// `parse_downloaded_modules` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn parse_downloaded_modules(json_bytes: &[u8]) -> Result<HashMap<String, DownloadedModule>> {
    let mut ret = HashMap::new();
    for chunk in split_json_objects(json_bytes) {
        let mod_: DownloadedModule =
            serde_json::from_slice(chunk.as_bytes()).map_err(|e| Error::new(e.to_string()))?;
        ret.insert(mod_.Path.clone(), mod_);
    }
    Ok(ret)
}

/// Match Go: accumulate lines (no newline), flush when a line has prefix `}`.
// `split_json_objects` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn split_json_objects(json_bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(json_bytes);
    let mut json_builder = String::new();
    let mut out = Vec::new();
    for line in text.split('\n') {
        json_builder.push_str(line);
        if line.starts_with('}') {
            out.push(std::mem::take(&mut json_builder));
        }
    }
    out
}

/// Go `mungeBazelRepoNameComponent`.
/// `munge_bazel_repo_name_component` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn munge_bazel_repo_name_component(component: &str) -> String {
    component.replace('-', "_").replace('.', "_").to_lowercase()
}

/// Go `modulePathToBazelRepoName`.
/// `module_path_to_bazel_repo_name` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn module_path_to_bazel_repo_name(mod_: &str) -> String {
    let mut components: Vec<String> = mod_.split('/').map(|s| s.to_string()).collect();
    let mut head: Vec<String> = components[0].split('.').map(|s| s.to_string()).collect();
    let mut i = 0usize;
    let mut j = head.len().saturating_sub(1);
    while i < j {
        let left = munge_bazel_repo_name_component(&head[j]);
        let right = munge_bazel_repo_name_component(&head[i]);
        head[i] = left;
        head[j] = right;
        i += 1;
        j -= 1;
    }
    for index in 1..components.len() {
        components[index] = munge_bazel_repo_name_component(&components[index]);
    }
    let mut parts = head;
    parts.extend(components.into_iter().skip(1));
    parts.join("_")
}

/// Go `dumpPatchArgsForRepo` — write Starlark patch args when patch file exists.
/// `dump_patch_args_for_repo` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn dump_patch_args_for_repo(
    bazel: &dyn Bazel,
    fs: &dyn Fs,
    out: &mut dyn Write,
    repo_name: &str,
) -> Result<()> {
    let runfiles = bazel.RunfilesPath()?;
    let candidate = join_path(&runfiles, &format!("build/patches/{repo_name}.patch"));
    match fs.stat(&candidate) {
        Ok(()) => {
            let _ = write!(
                out,
                "        patch_args = [\"-p1\"],\n        patches = [\n            \"//build/patches:{repo_name}.patch\",\n        ],\n"
            );
        }
        Err(err) if err.is_not_exist => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

/// Go `buildFileProtoModeForRepo`.
/// `build_file_proto_mode_for_repo` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn build_file_proto_mode_for_repo(repo_name: &str) -> &'static str {
    if repo_name == "io_etcd_go_etcd_api_v3" {
        return "disable";
    }
    "disable_global"
}

/// Go `dumpBuildNamingConventionArgsForRepo`.
/// `dump_build_naming_convention_args_for_repo` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn dump_build_naming_convention_args_for_repo(out: &mut dyn Write, repo_name: &str) {
    if repo_name == "com_github_grpc_ecosystem_grpc_gateway" {
        let _ = write!(
            out,
            "        build_naming_convention = \"go_default_library\",\n"
        );
    }
}

/// Go `dumpNewDepsBzl`.
/// `dump_new_deps_bzl` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn dump_new_deps_bzl(
    bazel: &dyn Bazel,
    fs: &dyn Fs,
    out: &mut dyn Write,
    listed: &HashMap<String, ListedModule>,
    downloaded: &HashMap<String, DownloadedModule>,
) -> Result<()> {
    let mut sorted: Vec<String> = Vec::new();
    let mut repo_name_to_mod_path: HashMap<String, String> = HashMap::new();
    for mod_ in listed.values() {
        let repo_name = module_path_to_bazel_repo_name(&mod_.Path);
        sorted.push(repo_name.clone());
        repo_name_to_mod_path.insert(repo_name, mod_.Path.clone());
    }
    // 这里必须稳定排序，否则同一依赖集合每次生成都可能因 map 遍历顺序不同而产生噪声 diff。
    sorted.sort();

    let _ = writeln!(
        out,
        r#"load("@bazel_gazelle//:deps.bzl", "go_repository")

def go_deps():
    # NOTE: We ensure that we pin to these specific dependencies by calling
    # this function FIRST, before calls to pull in dependencies for
    # third-party libraries (e.g. rules_go, gazelle, etc.)"#
    );

    for repo_name in sorted {
        // parser 来自仓库内 `pkg/parser` 的独立 `go.mod`，不是外部依赖，
        // 因此需要跳过对应仓库名，避免和仓库内源码定义重复。
        if repo_name == "com_github_pingcap_tidb_pkg_parser" {
            continue;
        }
        let path = &repo_name_to_mod_path[&repo_name];
        let mod_ = &listed[path];
        let replaced: &ListedModule = mod_.Replace.as_deref().unwrap_or(mod_);
        let _ = write!(out, "    go_repository(\n        name = \"{repo_name}\",\n");
        if repo_name.starts_with("com_github_tikv") {
            let _ = write!(out, "        build_tags = [\"nextgen\", \"intest\"],\n");
        }
        let _ = write!(
            out,
            "        build_file_proto_mode = \"{}\",\n",
            build_file_proto_mode_for_repo(&repo_name)
        );
        dump_build_naming_convention_args_for_repo(out, &repo_name);
        let _ = write!(out, "        importpath = \"{}\",\n", mod_.Path);
        dump_patch_args_for_repo(bazel, fs, out, &repo_name)?;
        let d = downloaded.get(&replaced.Path).ok_or_else(|| {
            Error::new(format!(
                "could not find downloaded module for {}@{}",
                replaced.Path, replaced.Version
            ))
        })?;
        if mod_.Replace.is_some() {
            let _ = write!(out, "        replace = \"{}\",\n", replaced.Path);
        }
        let _ = write!(
            out,
            "        sum = \"{}\",\n        version = \"{}\",\n",
            d.Sum, d.Version
        );
        let _ = writeln!(out, "    )");
    }
    Ok(())
}

/// Guard matching Go `defer os.RemoveAll(tmpdir)` (panic on failure).
struct TmpDirGuard<'a> {
    path: String,
    fs: &'a dyn Fs,
    active: bool,
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl<'a> TmpDirGuard<'a> {
    fn new(path: String, fs: &'a dyn Fs) -> Self {
        Self {
            path,
            fs,
            active: true,
        }
    }

    // `disarm` 承担该类型上的一个局部行为。
    // 注释重点说明调用目的、约束和 Go 对齐点，而不改变实现。
    fn disarm(&mut self) {
        self.active = false;
    }
}

// 下面这一段 `impl` 集中描述对应类型的行为面。
// 方法顺序大体跟随 Go 迁移路径，便于跨语言逐段对照。
impl Drop for TmpDirGuard<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Err(err) = self.fs.remove_all(&self.path) {
            panic!("{err}");
        }
    }
}

/// Go `mirror` with injectable boundaries (stdout writer for deps.bzl).
/// `mirror_with` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn mirror_with(
    bazel: &dyn Bazel,
    runner: &dyn Runner,
    fs: &dyn Fs,
    out: &mut dyn Write,
) -> Result<()> {
    let tmpdir = create_tmp_dir(bazel, fs)?;
    let _guard = TmpDirGuard::new(tmpdir.clone(), fs);
    let listed = list_all_modules(bazel, runner, &tmpdir)?;
    let downloaded = download_zips(bazel, runner, &tmpdir, &listed)?;
    dump_new_deps_bzl(bazel, fs, out, &listed, &downloaded)
}

/// Go `mirror` using production bazel/exec/fs and stdout.
/// `mirror` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn mirror() -> Result<()> {
    let bazel = ProdBazel;
    let runner = ProdRunner;
    let fs = OsFs;
    let mut out = std::io::stdout();
    mirror_with(&bazel, &runner, &fs, &mut out)
}

/// Go `main` with injectable argv / IO (used by binary and parity tests).
/// `run_main_with` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn run_main_with(
    args: &[String],
    bazel: &dyn Bazel,
    runner: &dyn Runner,
    fs: &dyn Fs,
    out: &mut dyn Write,
    err_out: &mut dyn Write,
) -> Result<()> {
    init_flags();
    let flags = stubs::parse_flags_checked(args)?;
    set_flags(&flags);
    if flags.is_mirror {
        let _ = writeln!(
            err_out,
            "--mirror is deprecated and ignored; modules are resolved through GOPROXY"
        );
    }
    if flags.is_upload {
        let _ = writeln!(
            err_out,
            "--upload is deprecated and ignored; modules are resolved through GOPROXY"
        );
    }
    if let Err(err) = mirror_with(bazel, runner, fs, out) {
        if err.is_exit {
            panic!(
                "subprocess exited with stderr:\n{}",
                String::from_utf8_lossy(&err.stderr)
            );
        }
        panic!("{err}");
    }
    Ok(())
}

/// Entry matching Go `main`.
/// `main` 是当前模块对外暴露的关键入口。
/// 它的输入、输出和失败语义都需要尽量贴近 Go 实现。
/// 因此维护时应优先保护外部契约，而不是随意调整内部顺序。
pub fn main() {
    let args = stubs::args_from_env();
    let bazel = ProdBazel;
    let runner = ProdRunner;
    let fs = OsFs;
    let mut out = std::io::stdout();
    let mut err_out = std::io::stderr();
    if let Err(err) = run_main_with(&args, &bazel, &runner, &fs, &mut out, &mut err_out) {
        let _ = writeln!(err_out, "{err}");
        std::process::exit(2);
    }
}

// `join_path` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn join_path(base: &str, rel: &str) -> String {
    Path::new(base).join(rel).to_string_lossy().into_owned()
}

// `replace_once` 是模块内部辅助步骤。
// 它负责收口重复细节，让更上层逻辑能围绕公共契约组织。
fn replace_once(haystack: &str, from: &str, to: &str) -> String {
    if let Some(idx) = haystack.find(from) {
        let mut out = String::with_capacity(haystack.len() - from.len() + to.len());
        out.push_str(&haystack[..idx]);
        out.push_str(to);
        out.push_str(&haystack[idx + from.len()..]);
        out
    } else {
        haystack.to_string()
    }
}
