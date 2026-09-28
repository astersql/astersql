// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Dumpling CLI entry — mirrors `dumpling/cmd/dumpling/main.go`.
//!
//! 本文件复刻 Go `main.go` 的控制流顺序：先注册 usage 和版本开关，
//! 再解析参数、构造导出配置、安装指标采集器，最后创建 dumper 执行导出。
//! Rust 版把“创建 dumper”抽象成可注入工厂，便于契约测试覆盖成功与失败路径，
//! 但返回码、错误文案和 close 时机仍尽量与 Go 侧保持一致。

use astersql_dumpling_cli as cli;
use astersql_dumpling_export::{self as export, Config, Dumper, Result as ExportResult};
use astersql_dumpling_log::{Field, Logger};

use crate::config_flags::{DefineFlags, ParseFromFlags};
use crate::stubs::{
    FlagHelp, FlagSet, clear_default_gatherer, register_runtime_collectors, set_default_gatherer,
};

/// Dump + Close + logger surface used by the CLI (real `Dumper` or test mock).
/// 主流程只依赖这三个动作，测试即可注入轻量 mock 而不必连真实数据库。
pub trait DumpSession {
    fn Dump(&mut self) -> ExportResult<()>;
    fn Close(&mut self) -> ExportResult<()>;
    fn L(&self) -> Logger;
}

impl DumpSession for Dumper {
    fn Dump(&mut self) -> ExportResult<()> {
        Dumper::Dump(self)
    }
    fn Close(&mut self) -> ExportResult<()> {
        Dumper::Close(self)
    }
    fn L(&self) -> Logger {
        Dumper::L(self)
    }
}

/// Process entry matching Go `main`.
pub fn main() {
    // Go 在失败分支直接 `os.Exit(code)`；Rust 先返回整数再统一退出，便于测试复用。
    let code = run(std::env::args().skip(1).collect::<Vec<_>>());
    if code != 0 {
        std::process::exit(code);
    }
}

/// Runnable CLI body. Returns process exit code (0 = success / early help|version).
pub fn run(args: Vec<String>) -> i32 {
    // 默认路径仍使用真实导出器；只有测试会改为注入自定义工厂。
    run_with_factory(args, |conf| export::NewDumper(conf))
}

/// Same control flow as Go `main`, with injectable dumper factory for tests.
pub fn run_with_factory<F, D>(args: Vec<String>, new_dumper: F) -> i32
where
    F: FnOnce(Config) -> ExportResult<D>,
    D: DumpSession,
{
    let mut flags = FlagSet::new();
    // usage 文案沿用 Go 原文，避免命令行帮助快照或脚本说明发生漂移。
    flags.set_usage(|fs| {
        eprint!(
            "Dumpling is a CLI tool that helps you dump MySQL/TiDB data\n\nUsage:\n  dumpling [flags]\n\nFlags:\n"
        );
        fs.PrintDefaults();
    });

    // Go: printVersion := pflag.BoolP(...) — value read after Parse.
    // 版本开关需要在 Parse 之前注册，否则后续无法读取解析结果。
    flags.BoolP("version", 'V', false, "Print Dumpling version");

    let mut conf = export::DefaultConfig();
    // 先灌入默认 flag，再由 ParseFromFlags 把解析结果同步回 Config。
    DefineFlags(&mut flags);

    if let Err(err) = flags.Parse(&args) {
        // Go's package-level pflag.CommandLine uses ExitOnError: syntax errors are
        // written to stderr and terminate with status 2 before ParseFromFlags runs.
        eprintln!("{err}");
        return 2;
    }

    match flags.GetBool(FlagHelp) {
        Ok(print_help) if print_help => {
            // help 路径属于正常提前返回，不应该触发导出逻辑。
            flags.Usage();
            return 0;
        }
        Err(err) => {
            // Go 读取 help 标志异常时也会打印 usage，Rust 这里保持同一策略。
            println!("\nGet help flag error: {err}");
            flags.Usage();
            return 0;
        }
        _ => {}
    }

    // Go uses the built-in println, which writes to stderr and appends another newline.
    // 无论是否带 `--version`，Go 都会先打印版本信息，Rust 保持相同输出顺序。
    eprint!("{}", long_version_output());
    if flags.GetBool("version").unwrap_or(false) {
        return 0;
    }

    if let Err(err) = ParseFromFlags(&mut conf, &flags) {
        // 二次校验集中在 ParseFromFlags，例如线程数和模板规则等约束。
        println!("\nparse arguments failed: {err:+}");
        return 1;
    }
    if flags.NArg() > 0 {
        // Go 不接受剩余位置参数，这里显式拦截未消费的 argv。
        println!(
            "\nmeet some unparsed arguments, please check again: {:?}",
            flags.Args()
        );
        return 1;
    }

    let registry = conf.PromRegistry.clone();
    // 运行时 collector 注册顺序与 Go 对齐，便于外部指标抓取行为一致。
    register_runtime_collectors(registry.as_ref());
    // Go: if gatherer, ok := registry.(prometheus.Gatherer); ok { DefaultGatherer = gatherer }
    // 默认 gatherer 通过 stub 全局槽保存，供测试验证是否被正确安装或清理。
    set_default_gatherer(registry);

    let mut dumper = match new_dumper(conf) {
        Ok(d) => d,
        Err(err) => {
            // 创建失败时 Go 会直接退出；这里同样不进入 Dump/Close 阶段。
            println!("\ncreate dumper failed: {}", err.Error());
            return 1;
        }
    };

    let err = dumper.Dump();
    // Go always Close() and ignores Close error.
    // 即使导出失败也必须执行 Close，确保资源释放时机与 Go 一致。
    let _ = dumper.Close();
    if let Err(err) = err {
        // 错误同时写日志和标准输出，方便交互式 CLI 与日志采集都能拿到原因。
        dumper.L().Error(
            "dump failed error stack info",
            [Field::string("error", err.Error())],
        );
        println!("\ndump failed: {}", err.Error());
        return 1;
    }
    dumper
        .L()
        .Info("dump data successfully, dumpling will exit now", []);
    // 成功路径不再触碰全局状态，直接返回 0 交给外层决定是否 exit。
    0
}

pub(crate) fn long_version_output() -> String {
    format!("{}\n", cli::LongVersion())
}

/// Test helper: reset CLI-global prometheus gatherer stub.
pub fn reset_cli_globals() {
    // 测试间共享默认 gatherer 槽，必须显式清空才能避免用例互相污染。
    clear_default_gatherer();
}
