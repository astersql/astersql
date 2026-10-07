# `cmd/benchkv/lib.rs`

## 文件定位

[`cmd/benchkv/lib.rs`](lib.rs) 是 Cargo 包 `astersql-cmd-benchkv` 的库 crate 根。`cmd/benchkv/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它声明为库入口，同时通过 `[[bin]] path = "bin_main.rs"` 构建同名可执行程序。因此该文件位于进程壳层与实际 benchkv 编排逻辑之间：[`bin_main.rs`](bin_main.rs) 调用这里公开的 `main()`，这里再把执行委托给以 [`main.rs`](main.rs) 挂载的 `entry` 模块。

它不是压测算法的实现文件。参数解析、依赖初始化、并发事务写入、Prometheus 指标抓取和输出均在 `entry` 或 `stubs` 中实现；本文件只定义 crate 的模块边界、测试接线和稳定入口。

## 核心职责

本文件有四项职责，均可直接由 `lib.rs` 的声明验证：

1. 以 `#[path = "stubs.rs"] pub mod stubs;` 公开运行时边界适配层，使实际入口和 crate 外调用者可使用存储、HTTP、指标、参数及错误处理适配。
2. 以 `#[path = "main.rs"] pub mod entry;` 将实际 benchkv 主流程公开为 `entry` 模块，避免二进制壳和测试复制实现。
3. 仅在 `cfg(test)` 下以 `#[path = "parity_test.rs"] mod parity_test;` 编入独立测试模块；测试逻辑没有内嵌在生产源文件中。
4. 提供 crate 根的 `pub fn main()`，稳定地转发到 `entry::main()`。

文件级 `#![allow(...)]` 允许迁移代码中的 Go 风格命名、暂未使用符号和广泛 Clippy 告警。该设置作用于整个库 crate，新增代码时不应把它误解为无需保持常规 Rust 可读性；它主要是迁移兼容边界。

## 主要符号

- `pub mod stubs`：公开模块，源码来自 `cmd/benchkv/stubs.rs`。它承载 `Flags`、`RuntimeDeps`、`Storage`、`Metrics`、`HttpServer`、`TiKVDriver` 以及 Go 风格错误辅助函数等运行时边界。
- `pub mod entry`：公开模块，源码来自 `cmd/benchkv/main.rs`。主要入口包括 `default_flags() -> Flags`、`main()`、`run_with_flags(Flags, RuntimeDeps)`、`init(&Flags, &RuntimeDeps)` 和 `batch_rw(&Flags, &Storage, &Metrics, &[u8])`。
- `mod parity_test`：私有且仅测试构建存在的模块，源码来自 `cmd/benchkv/parity_test.rs`。它直接引用 `crate::entry` 和 `crate::stubs` 验证 Go/Rust 行为契约。
- `pub fn main()`：本文件唯一函数，无参数、无返回值，只调用 `entry::main()`；它是 `bin_main.rs` 使用的库级进程入口。

本文件没有常量、结构体、枚举、trait、`impl`、可变静态状态或 feature 条件；唯一条件编译项是 `#[cfg(test)]` 的测试模块。

## 执行流程

生产调用链如下：

1. Cargo 构建的二进制进入 `cmd/benchkv/bin_main.rs::main`。
2. 二进制壳调用 `astersql_cmd_benchkv::main()`，即本文件的 `pub fn main()`。
3. 本文件调用 `entry::main()`。
4. `entry::main()` 从进程环境读取并解析参数，将日志级别设为 Error，然后用 `RuntimeDeps::default()` 调用 `run_with_flags`。
5. `run_with_flags` 初始化 TiKV 存储、指标和 HTTP 服务，创建指定大小的 value，并调用 `batch_rw` 启动无冲突事务写入；完成后读取本机 `/metrics`，输出指标、耗时和目标数据量，最后关闭响应体。

测试构建时，编译器还会从本文件挂载 `parity_test.rs`。测试绕过进程参数，直接调用 `entry::{default_flags, init, batch_rw, run_with_flags}` 并注入 `RuntimeDeps::for_test()`，但没有改变生产入口的转发逻辑。

## 数据与状态

`lib.rs` 自身不拥有业务数据和可变状态。它只建立模块命名空间并进行一次同步函数调用。所有状态都位于下游：

- `entry::main` 创建 `Flags` 和默认 `RuntimeDeps`。
- `entry::init` 得到或打开 `Storage`，克隆 `Metrics` 与 `HttpServer`，注册指标及 handler。
- `entry::batch_rw` 克隆存储、指标和值到 worker 线程，并以 `key_{base * worker + offset}` 形成互不重叠的 key 区间。
- `stubs` 在生产默认路径包装 canonical TiKV driver、Prometheus 和 HTTP 实现，在测试路径提供可观察的替身状态。

因此修改本文件的模块可见性或路径会改变状态类型的可达性，但本文件没有需要单独维护的状态不变量。实际不变量由 `main.rs` 和 `parity_test.rs` 约束，例如 worker 整除余数不补写、每次尝试增加事务计数、提交失败增加回滚计数并调用回滚。

## 依赖与调用关系

上游直接调用者是 `cmd/benchkv/bin_main.rs::main`，其函数体调用 `astersql_cmd_benchkv::main()`。下游直接被调用者是 `cmd/benchkv/main.rs` 中挂载为 `entry::main` 的函数。模块关系则是 `lib.rs → stubs.rs`、`lib.rs → main.rs`，测试配置下另有 `lib.rs → parity_test.rs`。

`cmd/benchkv/Cargo.toml` 声明的直接依赖为 `astersql-kv`、`astersql-store-driver`、`prometheus`、启用 blocking feature 的 `reqwest` 和 `tiny_http`。`lib.rs` 没有直接 `use` 这些 crate；它们经由公开的 `entry`/`stubs` 模块参与 TiKV 存储、指标采集和 HTTP 服务。Cargo 元数据同时把 Go 对照包标记为 `cmd/benchkv`、种类标记为 `binary`。

RustCodeGraph 已索引 `lib.rs` 的 `main` 以及相邻文件，但对 `bin_main.rs → 库 crate main → entry::main` 这组跨 crate/模块转发没有返回静态 caller/callee 边；上述直接边由三个函数体中的调用表达式核验，不以缺失的图边推断不存在调用。

## 错误处理与边界

本文件不捕获、转换或返回错误；`main()` 的下游 panic、进程级致命错误和正常返回均原样穿过这层薄转发。模块挂载失败属于编译期错误，而不是运行时分支。

实际边界由 `entry` 与独立测试固定：参数解析错误或未知 flag 失败；worker 数为零时整数除法 panic，负 worker 数和负 value size 也按 Go 对照语义 panic；存储 `Begin` 失败是致命错误；`Set` 失败仅记录后继续提交；`Commit` 失败增加回滚指标并尝试回滚；HTTP 监听、读响应或关闭响应体的非致命错误按各自路径记录。`parity_test.rs` 分别覆盖这些分支，因此更改入口接线后应确保这些测试仍可从 crate 根访问相同实现。

## 并发与资源生命周期

`lib.rs::main` 自身不创建线程、锁、通道、事务或资源句柄，调用期间也不保存所有权。并发和资源生命周期完全由下游同步调用覆盖：

- `entry::init` 启动后台线程运行指标 HTTP 服务；生产默认会监听 `:9191`。
- `entry::batch_rw` 为每个 worker 生成线程，克隆线程所需的存储、指标和值，最后逐一 `join`；任一 worker panic 会在主线程恢复展开，成功返回意味着全部 worker 已结束。
- 每次循环创建事务，尝试 `Set` 和 `Commit`，提交失败时调用 `Rollback`。
- `run_with_flags` 在读取指标响应后关闭响应体；关闭失败只记录，不升级为致命错误。

`parity_test.rs::contract_resource_cleanup` 验证响应先读取后关闭、关闭/监听失败只记录以及所有 worker 完成后才返回。由于本文件是同步转发层，不能在这里提前返回、吞掉 panic 或另起不受管理的入口线程，否则会破坏这些生命周期保证。

## 与 Go 版本的对应关系

Go 对照文件 `cmd/benchkv/main.go` 使用单一 `package main` 文件同时保存全局 flag/指标/存储、`Init`、`batchRW` 和 `main`。Rust 将这些职责拆开：本文件提供 crate 根与稳定入口，`main.rs` 对应 Go 的控制流，`stubs.rs` 抽象生产依赖和测试替身，`bin_main.rs` 提供真正的 Rust 二进制壳。

对应关系为：Rust `lib.rs::main → entry::main` 等价于把 Go `main` 暴露为可被二进制与测试复用的库入口；Rust `entry::init` 对应 Go `Init`；Rust `entry::batch_rw` 对应 Go `batchRW`。这种拆分改变了代码组织和依赖注入方式，没有意图改变 Go 的可观察行为。`parity_test.rs` 明确验证默认参数、TiKV URL、指标桶、整除分片、错误分支、后台监听、响应关闭和等待 worker 等对照契约。

## 扩展指南

- 新增命令级业务步骤时，优先修改 `cmd/benchkv/main.rs` 的 `main`/`run_with_flags`/`init`/`batch_rw` 中对应阶段；不要把压测逻辑堆入本文件的转发函数。
- 新增外部系统依赖或可替换后端时，在 `cmd/benchkv/stubs.rs` 扩展 `RuntimeDeps` 及生产/测试实现，并在 `Cargo.toml` 声明真正需要的依赖；保持 `RuntimeDeps::default()` 走生产后端。
- 只有需要改变 crate 公共表面时才修改 `pub mod stubs`、`pub mod entry` 或 `pub fn main`。收紧可见性会影响二进制壳或外部调用者，重命名模块路径也会影响独立 parity tests。
- 行为变化应同步更新独立的 `cmd/benchkv/parity_test.rs`，不要把 Rust 单元测试写入 `lib.rs`。至少覆盖正常路径、Go 特有边界、错误等级、并发完成和资源清理；若改变 Go 对齐语义，还需以 `cmd/benchkv/main.go` 的真实差异为依据。
- 并发度、指标或事务路径的改动存在性能与兼容风险：避免无意补写整除余数、引入 key 冲突、改变指标名称/标签/桶位、使入口在 worker 未结束前返回，或改变 `Begin`/`Set`/`Commit` 错误等级。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter cmd/benchkv` 找到 `lib.rs`、`bin_main.rs`、`main.rs`、`stubs.rs` 和 `parity_test.rs` 等文件。
- RustCodeGraph `node --file cmd/benchkv/lib.rs`：确认全部 33 行、三个模块声明、唯一函数 `main()` 及其 `entry::main()` 调用。
- RustCodeGraph `node`：读取 `cmd/benchkv/bin_main.rs`、`cmd/benchkv/main.rs` 和完整 `cmd/benchkv/parity_test.rs`，核对生产调用顺序及正常、边界、故障、资源清理测试。
- RustCodeGraph `query main --kind function`：确认 `cmd/benchkv/bin_main.rs::main`、`cmd/benchkv/lib.rs::main`、`cmd/benchkv/main.rs::main` 的独立符号；精确 `callers`/`callees` 查询没有为跨 crate/模块转发返回边，故调用关系另由源码调用表达式复核。
- `cmd/benchkv/Cargo.toml`：核对库/二进制入口、包边界、porting 元数据和直接依赖。
- `cmd/benchkv/main.go`：核对 Go 的 `main`、`Init`、`batchRW`、默认 flag、指标、错误处理和资源关闭语义。
- `rg` 引用检查：确认 `bin_main.rs` 调用库级 `main`，`lib.rs` 挂载并调用 `entry`，`parity_test.rs` 直接测试 `entry`/`stubs`；同目录不存在 `doc.go`，相关 Rust 测试位于独立文件 `cmd/benchkv/parity_test.rs`。

本任务为纯文档分析，未运行 Cargo 或代码测试；最终结构以任务规定的 11 个二级标题校验命令验证。
