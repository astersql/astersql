# `cmd/benchraw/lib.rs`

源码：[`cmd/benchraw/lib.rs`](./lib.rs)

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-cmd-benchraw` 的库根，而不是 RawKV 压测算法的实现文件。`cmd/benchraw/Cargo.toml` 的 `[lib] path = "lib.rs"` 将它注册为共享库；同一清单又把 `bin_main.rs` 注册为 `astersql-cmd-benchraw` 二进制。运行时入口因而形成 `bin_main.rs::main` → 本文件 `main` → `entry::main` 的三层薄转发链。

本文件用 `#[path = "main.rs"] pub mod entry` 把 Go 对齐的命令实现纳入库，用 `#[path = "stubs.rs"] pub mod stubs` 暴露外部边界适配层，并只在测试编译时用 `#[cfg(test)]`、`#[path = "parity_test.rs"]` 装入独立测试模块。它存在的目的，是让二进制和 crate 内测试复用同一套实现，同时保持测试逻辑不嵌入生产源文件。

## 核心职责

本文件只有三项职责：声明共享模块、为测试挂接独立测试模块、提供稳定的公开进程入口 `pub fn main()`。它不解析参数、不创建 TiKV 客户端、不启动线程，也不生成压测键；这些行为分别位于 `entry`（`main.rs`）与 `stubs`（`stubs.rs`）。

crate 根的公开面包括 `stubs`、`entry` 和 `main`。其中前两个模块是 `pub`，因此外部 Rust 调用者理论上可以直接访问其公开项；`parity_test` 是私有且仅在 `cfg(test)` 下存在，不进入生产构建。文件级 `#![allow(...)]` 放宽未使用项、Go 风格命名和 Clippy 等检查，以容纳迁移代码与对齐接口；它作用于整个 crate，扩展时不应把它误认为局部函数属性。

## 主要符号

- `pub mod stubs`（`lib.rs:18-19`）：通过路径属性载入 `stubs.rs`。该模块定义 `Flags`、`ClientFactory`、`RawKvClient`、真实 TiKV RawKV 适配器、测试桩、日志与 HTTP/pprof 边界。
- `pub mod entry`（`lib.rs:21-22`）：通过路径属性载入 `main.rs`。关键公开函数是 `default_flags`、`main`、`run_with_flags`、`batch_raw_put` 和 `format_elapse_line`。
- `mod parity_test`（`lib.rs:24-26`）：仅在测试配置下编译的私有模块，对应独立文件 `parity_test.rs`，覆盖 Go/Rust 公共契约。
- `pub fn main()`（`lib.rs:31-33`）：本文件唯一函数和公开入口，无参数、无返回值；函数体只调用 `entry::main()`。

本文件没有模块级常量、类型、trait、`impl` 或额外条件编译项。RustCodeGraph 对 `cmd/benchraw/lib.rs` 识别出文件节点和 `main` 函数节点，源码节点也确认函数体仅含上述转发。

## 执行流程

1. Cargo 按 `Cargo.toml` 的 `[[bin]]` 启动 `bin_main.rs::main`。
2. `bin_main.rs::main` 调用库 crate 的 `astersql_cmd_benchraw::main()`，即本文件公开入口。
3. 本文件 `main` 不加工输入，立即把控制权交给 `entry::main()`。
4. `entry::main` 从进程环境取得参数，经 `stubs::parse_flags` 解析；帮助请求返回 `None` 时直接结束，否则用 `default_client_factory()` 和 `start_pprof = true` 调用 `run_with_flags`。
5. `run_with_flags` 设置 Warn 日志级别，后台启动 `:9191` pprof 服务，分配 value，调用 `batch_raw_put`，等待压测完成后输出耗时摘要。
6. `batch_raw_put` 拆分 PD 地址并透传 TLS 配置，创建共享 RawKV 客户端，按 worker 切分连续键区间，启动线程执行 Put，再逐一 `join`。

步骤 4-6 是本文件转发到的下游流程，不是 `lib.rs` 内部实现。RustCodeGraph 明确给出 `entry::main → run_with_flags → batch_raw_put`，以及 `run_with_flags → listen_and_serve`、`batch_raw_put → split_pd_addrs/fatal_put_failed`；图未为路径模块转发解析出 `lib.rs::main → entry::main` 边，但该直接调用由 `lib.rs:31-33` 本身确认。

## 数据与状态

`lib.rs::main` 不创建、持有或变更任何数据；它也没有参数和返回值，因此所有命令状态都由下游模块管理。`entry::main` 产生 `Vec<String>` 参数和 `Flags`，后者包含数据量、worker 数、PD 地址、value 大小及 TLS 三元组。`run_with_flags` 再构造 value、计时器和客户端工厂调用。

`stubs` 中存在日志级别、测试故障注入、最近建连参数、全局 Put 日志以及 HTTP 监听记录等进程级状态；这些状态是边界实现和测试观测点，不属于 `lib.rs` 自身。生产 RawKV 客户端由 `tikv-client` 提供，适配器持有一个 Tokio runtime；测试则使用共享 `PutLog` 和 `StubRawKvClient`。保持这种归属区分很重要：改变模块可见性会影响 crate API，而改变压测数据结构应在 `main.rs` 或 `stubs.rs` 完成。

## 依赖与调用关系

上游直接调用者是 `cmd/benchraw/bin_main.rs::main`，它通过 Cargo 生成的 crate 名 `astersql_cmd_benchraw` 调用本文件 `main`。测试编译则从本文件装入 `parity_test.rs`；测试通过 `crate::entry` 与 `crate::stubs` 访问下游公开契约。仓库搜索未发现其他生产 Rust 文件调用 `benchraw::main`。

本文件直接下游只有 `entry::main`，但其模块声明决定整个 crate 的依赖边界。`Cargo.toml` 列出 `log`、带 `prost-codec` 的 `pprof`、固定 tag `v0.4.2-aster.10` 的 `astersql/client-rust` `tikv-client`、`tiny_http` 和带多线程运行时功能的 `tokio`。这些依赖实际由 `main.rs`/`stubs.rs` 使用；`lib.rs` 自身只依赖 Rust 模块系统。

调用链的关键边是：`bin_main.rs::main → lib.rs::main → entry::main → run_with_flags → batch_raw_put → RawKvClient::Put`。并行的服务边是 `run_with_flags → thread::spawn → stubs::listen_and_serve(:9191)`。Cargo 将库与二进制放在同一 package 中，避免二进制复制命令逻辑。

## 错误处理与边界

本文件不捕获、转换或返回错误。`entry::main()` 正常返回时，本文件同步返回；下游 panic（包括参数错误、负 value 大小、建连失败、零 worker、Put 失败或 worker panic）会原样越过这个薄入口。这里不增加 `Result` 或兜底捕获，是当前入口契约的一部分。

真实错误策略位于下游：帮助参数使 `parse_flags` 返回 `None` 并正常停止；负 value 大小在建客户端前失败；建连错误和零 worker 通过 `fatal` 快速失败；Put 错误保留 `put failed` 语义；worker panic 在 `join` 后重新抛出。pprof 后台监听失败例外：它经 `errors_trace`/`terror_log` 记录，但不阻塞主压测流程。

边界上，`#[cfg(test)]` 保证 `parity_test.rs` 不进入生产库。`#![allow(clippy::all)]` 会降低 crate 级静态检查敏感度，新增逻辑时仍应主动遵循仓库 Rust 规范，不能把该属性当作忽略正确性或可维护性问题的许可。

## 并发与资源生命周期

`lib.rs` 本身不创建线程、锁、通道、事务或网络资源，并以普通同步调用把 `entry::main` 的整个生命周期包在自身调用栈中。由于没有提前返回或资源包装层，进程入口的生命周期完全服从下游实现。

下游 `run_with_flags` 启动一个不 `join` 的 pprof 后台线程；监听错误只写日志。`batch_raw_put` 为每个 worker 启动一个线程，每个线程克隆共享客户端 `Arc` 和独立 value 缓冲区，主线程逐一 `join` 后才输出摘要并返回。这与 Go 的后台 pprof goroutine、RawKV worker goroutine 和 `WaitGroup.Wait` 对齐。`parity_test.rs::contract_resource_cleanup_and_pprof` 证明 worker 在返回前全部完成，并验证 `:9191` 监听副作用与监听错误日志。

生产客户端适配器持有 Tokio runtime，并在同步 worker 中用 `block_on` 调用异步 `tikv_client::RawClient::put`；其释放发生在最后一个 `Arc` 及客户端对象离开作用域时。本文只记录该直接下游事实，`lib.rs` 没有额外的关闭或取消机制。

## 与 Go 版本的对应关系

Go 的 `cmd/benchraw/main.go` 是单一 `package main` 文件，直接包含 flags、`batchRawPut` 和 `main`。Rust 为可测试与可复用性将同一行为拆为三层：本文件负责 crate 门面，`main.rs` 负责命令装配和并发写入，`stubs.rs` 负责 RawKV、日志、参数与 HTTP 边界；`bin_main.rs` 再提供 Cargo 二进制入口。因此，`lib.rs::main` 对应 Go `main` 的入口身份，但其函数体只转发，Go `main` 的实际步骤对应 `entry::main`/`run_with_flags`。

`parity_test.rs` 锁定的移植语义包括：默认 `N=1_000_000`、`C=100`、PD `localhost:2379`、value 大小 5；整除分片且余数被舍弃；key 形如 `key_<n>`；PD 逗号拆分和 TLS 字段原样透传；零大小 value 仍执行 Put；零 worker、非法 flag、建连或 Put 失败会快速失败；worker 完成后才返回；pprof 在 `:9191` 后台启动。Rust 的 fatal 以 panic 表达测试可见性，生产 RawKV 通过 Rust `tikv-client` 与 Tokio 适配，而 Go 使用 `client-go/rawkv` 和 `context.Background()`，这些是实现机制差异，不应被描述为行为删减。

## 扩展指南

- 若新增或调整命令行为，优先修改 `entry` 的 `main`、`run_with_flags` 或 `batch_raw_put`；不要把业务逻辑堆入本文件薄入口。
- 若新增外部系统能力或可注入边界，在 `stubs.rs` 扩展最小 trait/适配器，并同步真实实现与测试桩；若增加第三方 crate，再更新 `Cargo.toml`。
- 只有需要形成稳定 crate 级 API 时才改变 `pub mod entry`、`pub mod stubs` 或新增根级再导出。收窄现有可见性可能破坏外部调用者，扩宽可见性则会扩大兼容承诺。
- 若改变启动顺序或错误传播，应同步 `parity_test.rs` 的正常、边界、错误与资源生命周期契约；测试必须继续保存在独立测试文件，而非嵌入 `lib.rs`。
- 若新增另一个入口包装器，应继续调用本文件 `main` 或明确共享 `entry` 流程，避免参数解析、pprof 启动与 RawKV 行为出现两份实现。
- 主要风险是 Go/Rust 语义漂移、并发完成时机变化、后台服务阻塞主流程、TLS/PD 参数被意外改写，以及对 crate 级 `allow`/公开模块面的无意扩大。吞吐优化还需关注每 worker 克隆 value 和同步 `block_on` 的成本，但这属于 `main.rs`/`stubs.rs` 的修改范围。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter cmd/benchraw` 返回 `lib.rs`、`bin_main.rs`、`main.rs`、`stubs.rs`、`parity_test.rs` 与 Go 对照文件。
- RustCodeGraph `node --file cmd/benchraw/lib.rs`：确认 33 行完整内容、两个索引符号以及 `main` 的单语句转发。
- RustCodeGraph `node`：核对 `cmd/benchraw/bin_main.rs::main`、`cmd/benchraw/lib.rs::main`、`cmd/benchraw/main.rs::main`、`run_with_flags`、`batch_raw_put`；图给出 `entry::main → run_with_flags/default_client_factory`、`run_with_flags → batch_raw_put/listen_and_serve`、`batch_raw_put → fatal_put_failed/split_pd_addrs`，并列出三个契约分组对 `batch_raw_put` 的调用。
- Cargo 边界：`cmd/benchraw/Cargo.toml` 的 `[lib]`、`[[bin]]`、`package.metadata.porting` 和依赖列表。
- 入口与实现：`cmd/benchraw/bin_main.rs`、`cmd/benchraw/main.rs`、`cmd/benchraw/stubs.rs`。
- Go 对照：`cmd/benchraw/main.go` 的 flag 定义、`batchRawPut`、`main`、WaitGroup 与 pprof goroutine。
- 独立测试：`cmd/benchraw/parity_test.rs`，包括公开契约、负 value 大小、flag 停止规则、真实 PD/HTTP 边界、正常写入、余数舍弃、错误传播和资源清理。仓库 `rg` 搜索未发现其他直接 benchraw 测试；该文件是本 crate 的对应测试面。
- 本任务为纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构验证，并人工核对本文能回答文件存在原因、完整调用路径与安全扩展位置。
