# `dumpling/export/lib.rs`

## 文件定位

本文件是 Rust library crate `astersql-dumpling-export` 的根文件；[`Cargo.toml`](Cargo.toml) 以 `[lib] path = "lib.rs"` 明确入口，并用 `package.metadata.porting.go-package = "dumpling/export"` 标记其 Go 对照包。它对应的不是某一个 Go 源文件，而是 [`dumpling/export`](./) 整个 Go 包的 Rust 装配边界。

该 crate 采用迁移期的“单包视图”：`prepare.rs` 到 `dump.rs` 的 20 个生产文件通过 `include!` 展开到 crate 根作用域，而不是成为彼此隔离的 Rust 子模块。因此这些文件可直接共享本文件导入的标准库类型、`cli`/`tcontext`/`log` 别名，以及其他被包含文件在 crate 根定义的符号。例外是 `stubs.rs` 和 `schema_projection.rs`：前者是私有子模块但经 `pub use stubs::*` 重导出全部公开替身，后者是私有子模块，不会像 `include!` 文件那样自动把符号放到 crate 根。

本文件自身不实现导出算法；真实命令行上游是 [`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs)，它从该 crate 取得 `Config`、`Dumper`、`Result`、`DefaultConfig` 和 `NewDumper`，随后驱动导出会话。

## 核心职责

1. 定义 `astersql-dumpling-export` 的编译单元边界，并把 Go `export` 包迁移后的实现聚合为一个 Rust library API 面。
2. 提供迁移期共享命名空间：集中导入集合、格式化、原子量、通道、锁、线程与时间类型，以及三个 Dumpling 基础 crate 的别名，供 `include!` 展开的文件直接使用。
3. 用 `stubs` 子模块承载尚未由完整上游依赖替换的 SQL、存储、HTTP、指标等轻量接口，并通过通配重导出让实现文件和外部调用者使用这些兼容类型。
4. 按职责顺序装配配置准备、任务与 IR、SQL/连接、一致性与元数据、writer 和顶层 dump 流程；这个顺序也决定宏展开后的名称可见环境，调整时需要视为编译结构变更。
5. 在 `cfg(test)` 下集中挂载 22 个独立 Rust 测试模块，遵守生产源码与测试分文件的仓库约束；测试不进入普通 library 构建。
6. 以 crate 级 `allow` 暂时容纳 Go 风格命名、未接线迁移项和 Clippy 告警。这是迁移兼容策略，不表示相关代码已经完整接线或可以忽略行为验证。

## 主要符号

- `#![allow(...)]`：crate 级 lint 配置，允许 `dead_code`、Go 风格大小写、未使用导入/变量/赋值/属性以及 `clippy::all`。它影响所有 `include!` 内容和子模块。
- `use std::collections::{HashMap, HashSet}`、`std::fmt`：为配置、元数据、IR、writer 等被包含实现提供共享容器与格式化名称。
- `use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering}`、`mpsc::{Receiver, Sender}`、`Arc`、`Mutex`、`OnceLock`、`thread`：为取消、计数、任务传递、共享状态与后台工作提供 crate 级名称；本文件只导入，不创建实例。
- `use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH}`：为重试、状态、指标及时间戳逻辑提供统一名称。
- `use astersql_dumpling_cli as cli`、`astersql_dumpling_context as tcontext`、`astersql_dumpling_log::{self as log, Field, Logger}`：分别接入命令行公共类型、可取消上下文和日志设施。
- `mod stubs; pub use stubs::*;`：编译 `stubs.rs` 并把其中公开符号提升为 crate 公共 API。这是本文件唯一显式公开声明。
- 20 个 `include!`：依次包含 `prepare`、`task`、`ir`、`metrics`、`sql_type`、`writer_util`、`config`、`column_filter`、`block_allow_list`、`retry`、`util`、`http_handler`、`conn`、`sql`、`consistency`、`metadata`、`ir_impl`、`status`、`writer`、`dump`。各文件自己的 `pub` 项由此直接成为 crate 根 API，例如 [`dump.rs`](dump.rs) 的 `Dumper`、`NewDumper` 和 `Dumper::Dump`。
- `mod schema_projection;`：编译独立的私有投影子模块；其实现不会被通配重导出。
- 22 个 `#[cfg(test)] #[path = "..."] mod ...;`：挂载 parity、主流程、公共 fixture 及各主题测试。条件编译边界确保普通构建不包含测试辅助逻辑。

本文件没有函数、结构体、枚举、trait、常量或 `impl`；这些业务符号均来自被包含文件或子模块。

## 执行流程

编译期流程如下：

1. 编译器先应用 crate 级 lint 配置并解析共享 `use`。
2. `stubs.rs` 被编译为子模块，公开项再通过 `pub use` 进入 crate 根；后续 `include!` 文件可以用 `crate::*` 或未限定名称访问这些替身。
3. 20 个实现文件按源码列出的顺序文本式展开。前半段建立准备、任务、IR、配置与辅助类型，中段建立 HTTP、连接、SQL、一致性和元数据设施，后半段建立 IR 实现、状态、writer 与顶层 `Dumper` 流程。
4. 普通构建最后编译私有 `schema_projection` 子模块；测试构建还按 `#[cfg(test)]` 挂载各独立测试模块。

运行期没有“执行 `lib.rs`”这一步。典型主链由 [`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs) 发起：`run` 先调用 crate 根的 `DefaultConfig`，解析配置后调用 `NewDumper`；成功时执行 `Dumper::Dump`，无论导出成功与否随后都调用 `Dumper::Close`。这些符号分别由 `config.rs` 和 `dump.rs` 经 `include!` 暴露。`Dumper::Dump` 再协调表准备、一致性控制、元数据、任务、writer 和状态逻辑；本文件只决定这些实现处于同一 crate 命名空间，不决定业务分支。

## 数据与状态

本文件没有自己的运行期对象或可变全局状态。所有 `use` 都只是名称导入；原子量、`Arc<Mutex<_>>`、`mpsc` 通道、线程和时间值的真实所有权在被包含的实现中建立和释放。

最重要的“状态”是编译期命名空间形态：`include!` 内容与根文件共享作用域，因而同名顶层项会冲突，私有项也可被其他被包含文件直接访问；`schema_projection` 与测试文件则保有模块边界。`pub use stubs::*` 还使替身公开项成为外部可观察 API，替换或改名可能影响 [`dumpling/cmd/dumpling`](../cmd/dumpling/) 以及其他依赖 crate。

测试状态仅存在于测试构建。`util_for_test.rs` 是同级测试 fixture 模块，`main_test.rs` 对照 Go `TestMain` 的包级准备语义，其余测试模块各自覆盖对应生产主题；生产构建不会初始化这些测试设施。

## 依赖与调用关系

上游方面，[`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs) 明确导入 `astersql_dumpling_export::{self as export, Config, Dumper, Result}`，并调用 `export::DefaultConfig()` 与 `export::NewDumper(conf)`。RustCodeGraph 对 `NewDumper` 找到 Go/Rust 两个定义，Rust 定义位于 `dump.rs:36`；精确源码搜索确认 CLI 的 `run` 是生产调用点。Cargo workspace 与 CLI manifest 负责把这个 library 链入 dumpling 二进制。

crate 内部的主要关系由包含顺序表达：`config.rs` 提供配置，`prepare.rs`/`metadata.rs` 准备导出对象，`conn.rs`/`sql.rs` 提供数据库交互，`consistency.rs` 管理一致性边界，`task.rs`/`ir.rs`/`ir_impl.rs` 描述工作单元，`writer.rs`/`writer_util.rs` 落盘，`status.rs`/`metrics.rs` 暴露进度，`dump.rs` 汇总为 `Dumper` 生命周期。具体调用边属于相应实现文件，不能仅凭 `include!` 顺序推断。

下游依赖由 [`Cargo.toml`](Cargo.toml) 核定：直接依赖 Dumpling 的 `cli`、`context`、`log`，本地 dumpformat（CSV/SQL/Parquet）、objstore、table-filter 与 parser 系列 crate，并依赖带已发布 tag 的 `astersql/arrow-rs` Parquet。`Cargo.toml` 注释说明当前 Rust crate为 arm64 Darwin 保持精简，SQL/MySQL/storage/HTTP/metrics 的若干能力由本地 stubs 承担；因此不能用 Go [`BUILD.bazel`](BUILD.bazel) 的完整依赖集合推断 Rust 已拥有同等真实后端。

RustCodeGraph 的文件节点报告 `lib.rs` 被 `tools/tazel/parity_test.rs` 使用，但其跨文件符号图对 `include!` 和常见方法名存在歧义；本说明对 CLI 主链采用精确源码调用点补证，没有把模糊的 `Dump`/`Close` 查询结果当作唯一依据。

## 错误处理与边界

本文件没有返回 `Result` 的逻辑，也不捕获、转换或记录运行期错误。错误类型和传播策略来自 `stubs.rs` 及被包含文件；例如 `NewDumper`、`Dumper::Dump`、`Dumper::Close` 的错误由 `dump.rs` 定义并由 CLI 处理。

它的主要失败边界在编译期：缺失任一 `include!`/`#[path]` 文件、被包含文件顶层名称冲突、共享导入被删除、私有子模块可见性变化，都会造成该 crate 编译失败或公共 API 改变。因为 crate 级 lint 广泛放宽，未使用或未接线代码不会以告警阻止构建；评估功能完成度必须依赖实际调用点和独立测试，而不能依赖“能够被包含”。

`schema_projection` 的私有模块边界与 20 个 `include!` 文件不同，扩展者不能假定其中项目自动出现在 crate 根。测试模块只在 `cfg(test)` 下存在，生产代码不得依赖其中符号。`stubs` 虽声明为私有模块，其公开内容却经通配重导出，修改时必须检查名称碰撞与外部兼容性。

## 并发与资源生命周期

本文件仅为并发原语提供共享导入，不启动线程、不创建通道、不获取锁，也不打开数据库、文件、网络服务或事务。具体生命周期由 `dump.rs`、`task.rs`、`writer.rs`、`status.rs`、`http_handler.rs`、`consistency.rs` 等实现管理。

对完整应用而言，资源总入口是 `Dumper`：CLI 在 `NewDumper` 成功后调用 `Dump`，随后即使 `Dump` 返回错误也执行 `Close`。独立 Rust 测试 [`dump_test.rs`](dump_test.rs)、[`parity_test.rs`](parity_test.rs)、[`http_handler_test.rs`](http_handler_test.rs) 和 [`status_test.rs`](status_test.rs) 分别覆盖取消/清理、Go 契约、HTTP 句柄和状态并发等边界。修改根文件的模块装配或共享并发类型时，应在这些独立测试文件中验证，不应把测试嵌入 `lib.rs`。

由于 `include!` 共享作用域，根级 `Ordering`、`Sender`、`Receiver` 等名称是多个实现文件的隐式编译依赖。把它们移动到单个文件或改为模块化导入虽然不直接改变运行时，却可能破坏其他被包含文件；重构时应先逐文件显式化依赖，再改变装配方式。

## 与 Go 版本的对应关系

Go 没有对应的 `lib.go` 聚合文件；[`BUILD.bazel`](BUILD.bazel) 的 `go_library(name = "export")` 直接列出 20 个 `.go` 生产文件，Go 编译器按 package 规则天然把它们放入同一包作用域。Rust `lib.rs` 的 20 个 `include!` 正是在迁移期模拟这一包级可见性，文件集合与 Bazel `srcs` 一一对应，包括后加入的 `schema_projection.go`；Rust 对后者选择了独立私有模块，而非文本包含，这是结构上的差异。

Go 生产入口 [`dumpling/cmd/dumpling/main.go`](../cmd/dumpling/main.go) 调用 `export.DefaultConfig()`、`export.NewDumper(context.Background(), conf)`、`dumper.Dump()` 和 `dumper.Close()`；Rust CLI 保留同一控制流，但 `NewDumper` 的取消上下文由 Rust 实现内部创建，签名不接收 Go 的 `context.Context`。

Go 测试由 `BUILD.bazel` 的 `go_test` 聚合 18 个 `*_test.go`/fixture 文件，并通过 package `export` 自动共享符号；Rust 必须在 `lib.rs` 中显式声明 22 个测试模块。额外的 [`parity_test.rs`](parity_test.rs) 锁定 Go/Rust 公共契约；Rust 的 [`conn_test.rs`](conn_test.rs)、[`ir_test.rs`](ir_test.rs)、[`sql_type_test.rs`](sql_type_test.rs)、[`writer_util_test.rs`](writer_util_test.rs) 等也形成更细的独立测试面。Go [`main_test.go`](main_test.go) 的 `TestMain` 初始化列类型、logger、Prometheus registry 并检查 goroutine 泄漏；Rust 对照测试不能据此声称具有 Go runtime/goleak 的完全相同机制，只能验证已移植的契约。

## 扩展指南

- 新增同包式实现文件时，需要同时判断 Go/Bazel 文件集合、Rust `Cargo.toml` 依赖与本根文件装配。若必须访问大量现有私有根符号，可暂按依赖顺序加入 `include!`；更长期的模块化实现应优先显式导入和清晰的 `pub(crate)` 边界。
- 新增或调整公共 API 时，应修改实际所属实现文件，而不是在 `lib.rs` 写转发桩；随后检查 `dumpling/cmd/dumpling` 等上游。若符号来自 `stubs.rs`，还要判断它应继续作为兼容替身、被真实依赖替换，还是仅保持 crate 内可见。
- 调整 `include!` 顺序或改为 `mod` 前，必须清点共享 `use`、跨文件私有符号、宏和同名项。模块化会改变路径与可见性，是兼容性变更，不是纯格式重排。
- 新增生产文件的测试应放在独立 `*_test.rs` 中，并在此处用 `#[cfg(test)] #[path = "..."] mod ...;` 挂载；不要把测试逻辑写进生产 `.rs`。按职责选择现有测试文件，顶层公共契约优先放入 `parity_test.rs`，Dumper 生命周期放入 `dump_test.rs`。
- 引入外部能力时应更新 [`Cargo.toml`](Cargo.toml) 并遵守仓库对 tagged Git 依赖的要求；不能仅把 Go `BUILD.bazel` 依赖照搬为 Rust 已支持能力。当前 stubs 边界尤其需要真实调用与测试证据。
- 收紧 crate 级 lint 时宜分项进行，并先清理受影响实现；一次删除整个 `allow` 会把所有被包含文件纳入同一告警面，容易掩盖实际行为改动。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件，其中 Rust 7,032 个；`node --file dumpling/export/lib.rs --offset 1 --limit 400` 返回完整 163 行并确认其被索引；`query NewDumper --kind function` 定位 Go `dump.go:82` 与 Rust `dump.rs:36` 两个定义。对 `Dump`/`Close` 的普通名称查询存在跨仓库歧义，因此调用关系另用精确源码搜索核验。
- Rust 根与 crate 配置：[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`BUILD.bazel`](BUILD.bazel)。人工核对了 20 个生产 `include!`、`stubs` 重导出、私有 `schema_projection` 和 22 个条件测试模块。
- Rust 生产调用证据：[`dumpling/cmd/dumpling/main.rs`](../cmd/dumpling/main.rs) 的 `run`/`run_with_factory`，以及 [`dump.rs`](dump.rs) 的 `Dumper`、`NewDumper`、`Dumper::Dump`、`Dumper::Close`。
- Rust 独立测试：[`parity_test.rs`](parity_test.rs)、[`main_test.rs`](main_test.rs)、[`dump_test.rs`](dump_test.rs)、[`conn_test.rs`](conn_test.rs)、[`http_handler_test.rs`](http_handler_test.rs)、[`status_test.rs`](status_test.rs)，以及本文件列出的其余主题测试模块。它们证明测试与生产文件分离，并覆盖 crate 公共契约和关键资源边界。
- Go 对照：[`dumpling/cmd/dumpling/main.go`](../cmd/dumpling/main.go)、[`dump.go`](dump.go)、[`main_test.go`](main_test.go) 和 [`BUILD.bazel`](BUILD.bazel) 的完整 package 源/测试集合。
- 本任务只新增说明文档，按任务要求未运行 Cargo。交付前使用指定结构命令验证文件存在且恰有 11 个固定二级章节，并人工检查链接、符号、数量、调用链和“聚合根而非业务实现”的边界表述。
