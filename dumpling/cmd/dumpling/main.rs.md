# `dumpling/cmd/dumpling/main.rs`

## 文件定位

本文件是 Rust Dumpling 命令行程序的控制流入口，源码见 [`main.rs`](main.rs)。Cargo 包 `astersql-dumpling-cmd-dumpling` 在 [`Cargo.toml`](Cargo.toml) 中同时声明了库入口 `lib.rs` 和二进制入口 `bin_main.rs`，并将迁移来源标为 Go 包 `dumpling/cmd/dumpling`、类型标为 `binary`。

真实启动链为 `bin_main.rs::main` → `lib.rs::main` → `entry::main`。`lib.rs` 通过 `#[path = "main.rs"] pub mod entry` 纳入本文件，因此本文件不是 Cargo 直接指定的 bin 文件，而是可由二进制壳层和 crate 内测试共同调用的可测试入口模块。该目录没有 `doc.go`；最近的包级说明是 `lib.rs` 的 crate 文档。

## 核心职责

`main.rs` 只编排一次 CLI 运行，不实现参数全集或实际导出算法：

1. 建立本地 `FlagSet`，安装 usage，并注册 `--version/-V` 与 Dumpling 参数。
2. 解析参数，处理帮助、版本、非法参数和剩余位置参数。
3. 将 flag 值写回 `astersql_dumpling_export::Config`。
4. 注册进程/Go runtime collector，并将配置中的 registry 安装为默认 gatherer。
5. 构造 `Dumper`，执行 `Dump`，无论导出成功与否都调用一次 `Close`。
6. 将各分支归一为退出码，由最外层 `main` 只在非零时调用 `std::process::exit`。

这种拆分保留了 Go [`main.go`](main.go) 的控制流顺序，同时用工厂和 `DumpSession` 把真实数据库导出从控制流测试中隔离出来。

## 主要符号

- `pub trait DumpSession`：主流程所需的最小会话接口，只有 `Dump(&mut self) -> ExportResult<()>`、`Close(&mut self) -> ExportResult<()>` 和 `L(&self) -> Logger`。它是测试注入边界，不是完整 Dumper API。
- `impl DumpSession for Dumper`：逐项转发到 `astersql_dumpling_export::Dumper` 的同名方法，使生产对象可进入泛型主流程。
- `pub fn main()`：收集除程序名外的 `std::env::args()`，调用 `run`；返回码非零时终止进程，成功时自然返回。
- `pub fn run(args: Vec<String>) -> i32`：生产便捷入口，把 `export::NewDumper` 作为工厂传给 `run_with_factory`。
- `pub fn run_with_factory<F, D>(...) -> i32`：文件的核心编排函数。`F: FnOnce(Config) -> ExportResult<D>` 表示每次运行只构造一个会话，`D: DumpSession` 允许测试替身。
- `pub(crate) fn long_version_output() -> String`：在 `cli::LongVersion()` 后再补一个换行，以复现 Go 内建 `println` 面对已含尾换行文本时的字节结果。
- `pub fn reset_cli_globals()`：调用 `clear_default_gatherer` 清理测试可见的全局 registry；生产主流程不会调用它。

本文件没有模块级常量、结构体、枚举或条件编译项。公开符号主要是为了 crate 入口复用和独立测试；`long_version_output` 仅在 crate 内可见。

## 执行流程

`run_with_factory` 的分支顺序具有兼容意义：

1. `FlagSet::new` 后通过 `set_usage` 固定帮助头，再先注册 `version`，随后调用 `config_flags::DefineFlags` 注册完整参数集合。
2. `flags.Parse(&args)` 失败时向标准错误打印解析错误并返回 `2`，对应 Go `pflag.CommandLine` 的 `ExitOnError` 语义；不会进入配置回填或导出。
3. 读取 `FlagHelp`。值为 `true` 时打印 usage 并成功返回；读取 help 本身失败时打印错误和 usage，仍返回 `0`，与 Go 的提前返回分支一致。
4. 在检查 `--version` 前总是通过 `long_version_output` 向标准错误写版本文本；`--version/-V` 为真时返回 `0`。
5. `ParseFromFlags(&mut conf, &flags)` 负责把已解析值覆盖到 `DefaultConfig` 并执行参数约束；失败返回 `1`。之后若 `NArg() > 0`，报告未消费的位置参数并返回 `1`。
6. 克隆 `conf.PromRegistry`，按进程 collector、Go collector 的顺序注册，再通过 `set_default_gatherer` 保存该 `Arc<dyn Registry>`。
7. 调用一次 `new_dumper(conf)`。构造失败时报告 `create dumper failed` 并返回 `1`，不执行 `Dump` 或 `Close`。
8. 构造成功后调用一次 `Dump`，立即调用一次 `Close` 并丢弃其结果。若先前 `Dump` 失败，则通过会话 logger 写错误、向标准输出报告并返回 `1`；否则记录成功日志并返回 `0`。

`main` 只把非零返回码转换成进程退出，因此 `run`/`run_with_factory` 的各条路径可在单元测试进程内直接断言，不会意外结束测试运行器。

## 数据与状态

主要局部状态包括 `FlagSet`、可变 `Config`、从配置克隆的 `Arc<dyn Registry>`、工厂返回的 `D: DumpSession` 和最终整数退出码。配置的数据转换边界清晰：`export::DefaultConfig()` 提供核心默认值，`DefineFlags` 提供 CLI 默认值，成功解析后 `ParseFromFlags` 再用 CLI 值覆盖配置；例如测试证明端口从 `DefaultConfig` 的 `3306` 变为 CLI 默认值 `4000`。

本文件唯一持久的进程级副作用是通过 `set_default_gatherer` 更新 `stubs.rs` 中的 `OnceLock<Mutex<Option<Arc<dyn Registry>>>>`。设置发生在 Dumper 构造之前，因此即使构造失败，gatherer 也已经安装。`reset_cli_globals` 仅是测试隔离辅助函数，不属于正常退出清理。

`DumpSession` 由可变借用串行调用，先 `Dump` 再 `Close`；工厂是 `FnOnce`，配置按值移交，防止一次运行重复消费同一工厂或同一配置。

## 依赖与调用关系

上游直接调用者：

- [`bin_main.rs`](bin_main.rs) 调用 crate 的 `main`。
- [`lib.rs`](lib.rs) 的 `main` 调用 `entry::main`，并把本文件声明为 `entry` 模块。
- [`parity_test.rs`](parity_test.rs) 直接调用 `run`、`run_with_factory`、`long_version_output`、`reset_cli_globals`，并为 `MockSession` 实现 `DumpSession`。

下游直接依赖：

- `astersql-dumpling-cli::LongVersion` 生成版本信息。
- `config_flags::{DefineFlags, ParseFromFlags}` 分别负责参数注册和 `Config` 回填/校验。
- `stubs::{FlagSet, FlagHelp}` 提供本地 pflag 兼容面；`register_runtime_collectors`、`set_default_gatherer`、`clear_default_gatherer` 提供指标副作用。
- `astersql-dumpling-export::{DefaultConfig, NewDumper, Config, Dumper}` 提供配置和真实导出会话。`NewDumper` 继续完成 TLS/格式校验、logger、外部存储、HTTP、数据库和服务端信息初始化；实际 `Dump`/`Close` 位于 [`dumpling/export/dump.rs`](../../export/dump.rs)。
- `astersql-dumpling-log::{Logger, Field}` 用于失败和成功日志。

[`Cargo.toml`](Cargo.toml) 只声明上述三个工作区路径依赖，没有 feature 开关。其注释说明该 crate 为 arm64 Darwin 避开 kv/domain/kvproto/grpcio，并以本地 pflag/prometheus stub 保持较瘦依赖边界。

## 错误处理与边界

退出码契约是：成功、帮助、版本以及 help 读取异常返回 `0`；参数语法错误返回 `2`；配置校验、剩余位置参数、Dumper 构造失败或 Dump 失败返回 `1`。只有 `main` 会实际退出进程，内部入口均返回整数。

输出通道并不完全统一：usage、参数解析错误和版本走标准错误；配置、位置参数、构造与导出错误走标准输出；导出错误还写 logger。修改输出时需与 Go 的 `fmt.Printf`、`fmt.Fprint(os.Stderr, ...)` 和内建 `println` 逐项对照，不能仅按一般 CLI 惯例重排。

重要边界与限制：

- help/version 在配置回填、collector 注册和 Dumper 创建前提前返回。
- 未消费的位置参数不允许静默通过。
- Dumper 构造失败不会调用 `Close`；只有已成功构造的会话进入资源清理阶段。
- `Close` 错误被有意忽略，最终状态只由 `Dump` 结果决定。这与当前 Go 代码一致，但意味着关闭失败不会改变退出码或产生本文件中的诊断。
- `flags.GetBool("version")` 的读取错误通过 `unwrap_or(false)` 被当作未请求版本；由于该 flag 已在解析前注册，正常路径不应触发读取错误。
- collector 实现是名称级 stub，不等同于 Prometheus 真正的 process/Go runtime 指标采集；文档不能据此宣称 Rust 版已经提供完整指标内容。

## 并发与资源生命周期

本文件不创建线程、异步任务或通道；CLI 编排本身完全串行。共享并发状态只来自 `stubs.rs` 的默认 gatherer 槽，该槽由 `OnceLock + Mutex` 保护，registry 用 `Arc` 在配置、本地变量和全局槽之间共享。

资源生命周期的关键不变量是：只有 `new_dumper` 成功后才拥有会话；`Dump` 返回后无条件调用一次 `Close`，即使 `Dump` 失败也如此；`Close` 完成后才检查并报告 `Dump` 错误。真实 `Dumper::Close` 会取消上下文、停止 HTTP、关闭 PD 客户端和数据库并注销指标，但其中数据库关闭错误会被本文件丢弃。真实导出器内部可能管理更多运行时资源，那些实现属于 `dumpling/export/dump.rs`，不能归因成本文件直接创建。

测试使用 `AtomicUsize` 与 `AtomicBool` 证明 `Dump` 恰被调用一次且失败后仍执行 `Close`；`reset_cli_globals` 则防止全局 gatherer 在并行或连续测试间泄漏状态。测试若新增共享全局断言，应继续显式重置，避免受执行顺序影响。

## 与 Go 版本的对应关系

直接对照文件是 [`main.go`](main.go)。Rust 与 Go 保持的主序列为：安装 usage → 注册版本参数 → 默认配置/定义参数 → 解析 → help → 打印版本 → version 提前返回 → 配置回填 → 拒绝位置参数 → 注册 collector/default gatherer → `NewDumper` → `Dump` → 无条件 `Close` → 失败/成功日志。

主要实现差异如下：

- Go 的失败分支直接 `os.Exit`；Rust 内部先返回退出码，最外层再退出，以支持进程内测试。
- Go 直接依赖全局 `pflag.CommandLine` 和 Prometheus collector；Rust 使用 `FlagSet` 与名称级 collector stub。
- Go 调用 `export.NewDumper(context.Background(), conf)`；Rust 的 `export::NewDumper(conf)` 在实现内部创建可取消背景上下文。
- Go 使用具体 `*export.Dumper`；Rust 增加 `DumpSession` 与 `FnOnce` 工厂，仅改变可注入性，不改变主流程的 Dump/Close 顺序。
- Go 的全局 gatherer 不在主流程清空；Rust 同样保留它，但额外公开 `reset_cli_globals` 给测试隔离使用。

当前 Go 目录没有 `main_test.go`；Rust 对照测试集中在 [`parity_test.rs`](parity_test.rs)，参数细节测试另在 [`config_flags_test.rs`](config_flags_test.rs)。因此对本文件行为的直接回归证据主要来自 Rust parity 测试和 Go 生产入口，而不是同名 Go 测试。

## 扩展指南

- 新增或修改命令行参数时，应在 `config_flags.rs::DefineFlags` 和 `ParseFromFlags` 成对接线，并在 `config_flags_test.rs` 增加独立测试；若影响提前返回、退出码或导出时序，还应扩展 `parity_test.rs`。
- 修改入口顺序、错误文本、输出通道或退出码时，应先对照 `main.go::main`，在 `run_with_factory` 的最小相关分支改动，并覆盖成功、语法错误、配置错误、构造错误和 Dump 错误。
- 增加会话级动作时，只有在主流程确实需要抽象该动作时才扩展 `DumpSession`；同步更新 `Dumper` 转发实现和 `parity_test.rs::MockSession`，测试逻辑继续放在独立测试文件，不能内嵌到 `main.rs`。
- 改变资源清理时，应明确构造失败是否拥有可清理资源、Dump 失败时的清理顺序、Close 错误是否影响退出码，并补对应计数/状态断言。
- 改变 registry 行为时，应同时检查 `stubs.rs` 的全局槽和 collector stub。若要提供真实 Prometheus 指标，需要处理 Cargo 依赖与平台边界，不能只修改本文件的注册调用。
- 兼容风险主要是脚本依赖的退出码/输出、Go/Rust 参数行为漂移和 Close 错误策略；性能风险主要来自真实 Dumper 下游，而本文件新增工作应避免在 help/version 提前返回前建立昂贵资源。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引可用，覆盖 11,467 个文件、7,032 个 Rust 文件；`files --filter dumpling/cmd/dumpling` 找到 `main.rs` 及入口、参数、stub 和测试文件。
- RustCodeGraph `node --file dumpling/cmd/dumpling/main.rs`：读取完整 177 行并确认 `DumpSession`、`main`、`run`、`run_with_factory`、`long_version_output`、`reset_cli_globals` 的定义。
- RustCodeGraph 精确 `query`：确认 `run_with_factory`、`DumpSession`、`long_version_output`、`reset_cli_globals` 位于本文件；确认 `NewDumper`/`DefaultConfig` 位于 `dumpling/export`，collector/gatherer 辅助函数位于 `stubs.rs`。
- RustCodeGraph 文件节点：核对 `config_flags.rs` 的参数职责、`stubs.rs` 的 `OnceLock<Mutex<Option<Arc<dyn Registry>>>>` 与 collector 注册顺序，以及 `dump.rs::NewDumper/Dump/Close` 的真实资源职责。
- 直接读取 [`Cargo.toml`](Cargo.toml)、[`bin_main.rs`](bin_main.rs)、[`lib.rs`](lib.rs)、[`main.go`](main.go)、[`parity_test.rs`](parity_test.rs) 和 [`config_flags_test.rs`](config_flags_test.rs)。`rg` 还确认本目录入口调用和测试引用，并确认 `dumpling` 下没有 `doc.go`。
- `parity_test.rs` 的契约测试覆盖 help/version、默认参数、pflag 解析错误码、位置参数、配置错误、Dumper 构造失败、Dump 失败、成功路径、gatherer 设置/清理，以及 Dump 失败后仍 Close；`config_flags_test.rs` 补充压缩、column filter 和 session 参数等参数层边界。

RustCodeGraph 的 `callers/callees` 查询本次未在等待窗口内返回，因此调用边以已索引源码节点和上述入口/测试文件的直接调用语句核验。任务是纯文档分析，按计划未运行 Cargo；最终只执行任务规定的 11 章节结构检查，并人工复核没有把 stub 或下游行为夸大为本文件能力。
