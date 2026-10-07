# `cmd/benchkv/bin_main.rs`

## 文件定位

`cmd/benchkv/bin_main.rs` 是 Cargo package `astersql-cmd-benchkv` 的可执行目标入口。`cmd/benchkv/Cargo.toml` 的 `[[bin]]` 将二进制名 `astersql-cmd-benchkv` 指向本文件，同时 `[lib]` 将同一 package 的库根指向 `cmd/benchkv/lib.rs`。因此本文件位于操作系统启动二进制和可复用库逻辑之间，是一个有意保持极薄的进程壳层，而不是压测实现所在位置。

本文件当前只有 10 行和一个私有函数，不声明模块、常量、类型、trait、`impl`、feature gate 或条件编译项。真正的 Rust 主流程由 `cmd/benchkv/lib.rs`、`cmd/benchkv/main.rs` 和 `cmd/benchkv/stubs.rs` 共同提供。

## 核心职责

本文件的唯一职责是提供 Rust 二进制所要求的 crate-root `fn main()`，并把控制权无参数地转交给库 crate 的 `astersql_cmd_benchkv::main()`。这种拆分使生产二进制与 `cmd/benchkv/parity_test.rs` 复用同一套库内入口和依赖适配层，避免在二进制壳层重复参数解析、TiKV 初始化、并发事务或指标输出逻辑。

它不负责解释命令行、创建运行时、连接 PD/TiKV、监听端口、启动工作线程、打印指标或转换错误。上述行为均是下游库入口的职责；阅读或修改本文件时必须保持这一边界，不能把“转发后发生的行为”误写成本文件自身实现。

## 主要符号

- `fn main()`（`cmd/benchkv/bin_main.rs:8`）：私有的进程入口，签名无参数、无显式返回值。函数体只有 `astersql_cmd_benchkv::main();` 一条语句。
- `astersql_cmd_benchkv`：由 Cargo package 名 `astersql-cmd-benchkv` 按 Rust 标识符规则将连字符转换为下划线得到的库 crate 名。它不是本文件声明的模块。
- `astersql_cmd_benchkv::main()`（定义于 `cmd/benchkv/lib.rs:31`）：公开库入口，继续转发到 `entry::main()`；`entry` 由 `lib.rs` 通过 `#[path = "main.rs"] pub mod entry;` 暴露。

本文件的 `main` 不属于公共 Rust API；它的可见入口是构建出的可执行文件。库根 `main` 才是壳层可链接的公共符号，也是测试能够复用的稳定接缝。

## 执行流程

1. Cargo 按 `cmd/benchkv/Cargo.toml` 的 `[[bin]]` 编译 `bin_main.rs`，并把同 package 的库 target 作为 `astersql_cmd_benchkv` 提供给二进制。
2. 操作系统启动 `astersql-cmd-benchkv` 后进入本文件的 `fn main()`。
3. 本函数同步调用 `astersql_cmd_benchkv::main()`，自身不做前置初始化，也不截获返回、错误或 panic。
4. `cmd/benchkv/lib.rs:31` 的库入口调用 `entry::main()`。
5. `cmd/benchkv/main.rs:35` 的 `entry::main()`读取进程参数、解析 flags、设置 Error 日志级别，并调用 `run_with_flags(flags, RuntimeDeps::default())`。后续初始化 TiKV/Prometheus/HTTP、并发写事务、抓取指标和打印结果均发生在该层及 `stubs.rs`，不在本文件内。
6. 下游调用正常返回时，本文件的 `main` 随即返回，进程按 Rust 默认规则成功退出；下游 panic 或终止进程时，本文件不做恢复。

## 数据与状态

本文件不创建、持有或变更任何业务数据和全局状态，也没有参数、局部绑定、返回值或缓存。所有进程环境和运行状态都由下游读取或构造：`entry::main()`获取命令行参数，`RuntimeDeps::default()`选择生产后端，`run_with_flags`管理 flags、存储、指标和 HTTP 句柄。

这也形成一个重要不变量：二进制壳层不得提前复制或变换参数、构造另一套默认依赖，或维护与库入口平行的状态，否则生产执行路径会与直接调用库入口的测试路径分叉。

## 依赖与调用关系

直接调用链为：

`cmd/benchkv/bin_main.rs::main` → `cmd/benchkv/lib.rs::main` → `cmd/benchkv/main.rs::entry::main` → `run_with_flags`。

本文件唯一直接依赖是同 package 的库 crate；它没有直接 `use` 外部 crate。传递到下游后，package 才使用 `cmd/benchkv/Cargo.toml` 声明的 `astersql-kv`、`astersql-store-driver`、`prometheus`、`reqwest` 和 `tiny_http`。根 `Cargo.toml` 将 `cmd/benchkv`列为 workspace member，`Cargo.toml` 的 `package.metadata.porting` 还把它标记为对应 Go package `cmd/benchkv` 的 binary 移植目标。

RustCodeGraph 将本文件识别为含 2 个节点的已索引文件，并定位到 `main` 定义；其调用图对这条跨 crate 调用报告“无 callees”，未解析 `astersql_cmd_benchkv::main()`。因此跨 crate 边由本文件调用表达式、`cmd/benchkv/Cargo.toml` 的双 target 声明以及 `cmd/benchkv/lib.rs:31-32` 的库入口共同核实，而不是从缺失的图边推断。

## 错误处理与边界

本文件没有 `Result`、`Option`、错误映射、日志或退出码处理。`astersql_cmd_benchkv::main()`的返回类型为 `()`，正常返回即让壳层正常结束；任何 panic 会自然越过本层。命令行解析失败、负值或零 worker、存储打开失败、事务失败、HTTP/响应体错误等策略均由 `main.rs` 和 `stubs.rs` 定义。

该边界是有意的：若在这里捕获 panic、吞掉错误、改变退出码或添加第二次初始化，会改变生产二进制的可观察语义，却绕过 `parity_test.rs` 主要针对库入口建立的断言。需要改变错误政策时，应首先修改可测试的库层并补充独立测试，壳层只在确有进程级需求时调整。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、网络监听器或响应体，调用期间只占用普通同步调用栈。它会一直等待库入口返回，因此下游完整生命周期自然被包含在进程入口生命周期内。

具体并发和资源行为位于 `cmd/benchkv/main.rs`：`init` 启动 metrics HTTP 后台线程，`batch_rw` 为 worker 创建线程并逐一 `join`，`run_with_flags`读取并关闭 metrics 响应体。`cmd/benchkv/parity_test.rs` 的 `contract_resource_cleanup` 验证响应体关闭、监听错误记录和等待 worker 完成；这些是下游事实，不意味着本壳层直接拥有相应资源。

## 与 Go 版本的对应关系

Go 版本 `cmd/benchkv/main.go` 使用 `package main`，其 `main()`直接执行 `flag.Parse()`、设置日志级别、调用 `Init()`和 `batchRW()`、读取 `/metrics` 并打印耗时。Rust 为了让同一流程可被独立测试，将对应逻辑移到 `cmd/benchkv/main.rs:35` 的公开 `entry::main()`，再经 `cmd/benchkv/lib.rs:31` 暴露；本文件只补齐 Cargo 二进制所需的顶层入口。

因此 Go `main()`与 Rust `bin_main.rs::main()`并非逐语句一一对应：前者同时承担业务编排，后者只对应“进入程序”这一最外层职责。Go 主流程的语义对应物是 Rust 的 `entry::main()`及其下游函数。`cmd/benchkv/parity_test.rs` 覆盖默认 flags、无冲突 key 分片、余数舍弃、参数解析、生产后端选择、指标服务、错误分支和资源清理，但没有直接调用本文件的私有 `main`；壳层正确性主要由 Cargo 接线和单语句转发结构保证。

## 扩展指南

- 新增或修改 benchkv 行为时，优先在 `cmd/benchkv/main.rs` 的 `entry::main`、`run_with_flags`、`init` 或 `batch_rw` 接入，并在独立的 `cmd/benchkv/parity_test.rs` 同步测试；不要把可测试业务逻辑塞进本文件。
- 需要新增外部依赖时应修改 `cmd/benchkv/Cargo.toml` 并由库层使用，而不是仅在壳层引入；同时核对 workspace 和可复现依赖规则。
- 只有真正属于进程最外层的策略，例如最终退出码、进程级 panic 展示或启动前平台设置，才可能适合放在这里。即使如此，也应把可判定逻辑提取到库层，以免生产路径无法由独立测试覆盖。
- 若重命名 package、库 target 或二进制 target，必须同步核对 `astersql_cmd_benchkv` 路径、`[[bin]].path`、根 workspace member 和构建/发布调用方。
- 保持本文件无测试内嵌；相关 Rust 测试继续放在独立 `cmd/benchkv/parity_test.rs`，符合源文件与测试文件分离要求。涉及 Go 对齐语义时还应逐项核对 `cmd/benchkv/main.go`，避免为通过 Rust 测试而简化 Go 现有行为。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter cmd/benchkv`确认 `bin_main.rs`、`lib.rs`、`main.rs`、`parity_test.rs`和 `stubs.rs`均已索引；`node --file cmd/benchkv/bin_main.rs`与 `node cmd/benchkv/bin_main.rs::main`确认本文件完整内容及唯一函数；`callees` 查询显示跨 crate 转发边未被图解析，故未把该缺失边当作否定证据。
- 源码：`cmd/benchkv/bin_main.rs:8-10`确认唯一入口与唯一调用；`cmd/benchkv/lib.rs:18-32`确认 `stubs`、`entry`、独立测试模块和库根转发；`cmd/benchkv/main.rs:29-155`确认 flags 默认值、入口编排、初始化、并发写入、指标抓取与错误分支。
- Cargo/构建：`cmd/benchkv/Cargo.toml`确认 package 名、`[lib]`、`[[bin]]`、移植元数据及直接依赖；根 `Cargo.toml`确认 workspace membership；`cmd/benchkv/BUILD.bazel`确认现有 Bazel target描述的是 Go `main.go`而非 Rust 二进制，不能作为 Rust 壳层调用边证据。
- Go 对照：`cmd/benchkv/main.go`确认 Go 的 `main`、`Init`和 `batchRW`的原始时序、错误处理、并发及资源清理语义。
- 测试：`cmd/benchkv/parity_test.rs`是同 crate 的独立 Rust 测试文件，直接测试库层公开入口及其依赖边界；同目录不存在 `doc.go`或同名 `bin_main`专属测试。其覆盖不能替代实际启动二进制的端到端验证，这一点保留为验证边界。
- 本任务是纯文档分析，按总计划不运行 Cargo；交付验证仅检查目标文档存在且恰有规定的十一个二级章节，并人工复核所有行为结论均归属到正确实现层。
