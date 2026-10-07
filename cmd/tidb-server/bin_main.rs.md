# `cmd/tidb-server/bin_main.rs`

## 文件定位

[`bin_main.rs`](bin_main.rs) 是 `astersql-cmd-tidb-server` Cargo 包的可执行文件壳层。`cmd/tidb-server/Cargo.toml` 的 `[[bin]]` 将二进制名 `astersql-cmd-tidb-server` 明确映射到本文件，而同一个包的 `[lib]` 指向 [`lib.rs`](lib.rs)。这种拆分使操作系统启动的二进制与 Rust 测试、库调用共享库 crate 中的入口编排，而不是各自维护一份服务启动逻辑。

本文件不是 SQL 服务的实现主体。它只有一个进程级堆分配器静态项和一个私有 `main` 函数；CLI 解析、配置初始化、存储/Domain/Server 创建及退出清理位于 [`main.rs`](main.rs) 的库模块中。文件没有条件编译项、公开 API、类型、trait 或 `impl`。

## 核心职责

本文件承担两个只能由最终可执行 crate 统一决定的进程级职责：

1. 用 `#[global_allocator]` 把 `HEAP_PROFILER` 注册为整个进程的 Rust 全局分配器。它是包装系统分配器的 `rpprof::alloc::AllocProfiler`，因此后续 Rust 堆分配都从这个入口经过。
2. 在进入服务器共享入口前调用 `rpprof::alloc::start()`，然后把控制权交给 `astersql_cmd_tidb_server::main()`。依赖源码表明 `start()` 只是把全局 profiling 开关设为启用；采样工作由上述全局分配器在分配路径中执行。

它刻意不解析参数、不创建运行时、不启动线程，也不处理退出码。服务生命周期的真实边界是库入口及其下游，而这里保证堆采样在那些初始化动作发生前已经开启。

## 主要符号

- `static HEAP_PROFILER: rpprof::alloc::AllocProfiler`：私有、进程唯一的全局分配器。初始化式 `AllocProfiler::system()` 表示底层仍使用 `std::alloc::System`；`AllocProfiler` 只在外层增加采样记录。`#[global_allocator]` 使其作用域覆盖整个二进制，而不仅是本模块。
- `fn main()`：私有二进制入口，返回 `()`。它先调用 `rpprof::alloc::start()`，再调用库 crate 的 `astersql_cmd_tidb_server::main()`；源码中没有分支、参数或显式错误返回。
- `rpprof::alloc::start()`：来自 `Cargo.toml` 固定到 tag `v0.18.1` 的 Git 依赖。锁文件解析到提交 `3247a2e603a1d1d8ccf2addf33a439cdb6e33957`；该版本实现将 `IS_PROFILING` 原子开关以 `SeqCst` 顺序写为 `true`。
- `astersql_cmd_tidb_server::main()`：同包库 crate 的公开入口，而不是递归调用当前二进制的 `main`。`lib.rs` 中该函数先执行 `fips::enable_fips_only()`，再执行 `entry::main()`。

## 执行流程

1. 程序装载后，Rust 已把 `HEAP_PROFILER` 选为全局分配器；其底层是系统分配器。
2. 操作系统调用本文件的 `main()`。
3. `rpprof::alloc::start()` 打开进程级堆采样开关。该调用没有 `Result`，因此本层没有启动失败分支。
4. `astersql_cmd_tidb_server::main()` 进入 [`lib.rs`](lib.rs) 的共享入口。
5. 库入口调用 [`fips.rs`](fips.rs) 的 `enable_fips_only()`。普通构建不安装 FIPS provider；设置 `ASTERSQL_FIPS_ONLY` 的构建必须成功安装经验证的 provider，否则 panic，禁止静默降级。
6. 库入口再调用 `entry::main()`，即 [`main.rs`](main.rs) 中的服务器入口。该函数用 `stubs::args_from_env()` 读取进程参数并调用 `run_main()`。
7. `run_main()` 进入 `run_main_inner(argv, false)`，随后才发生 CLI/配置处理、存储和服务装配、信号等待与资源清理。`entry::main()` 用 `let _ = ...` 丢弃 `run_main()` 返回的整数退出码，因此本壳层本身不据此调用 `process::exit`。

调用链可概括为：`bin_main.rs::main → rpprof::alloc::start → lib.rs::main → fips::enable_fips_only → main.rs::main → run_main → run_main_inner`。

## 数据与状态

本文件不保存业务配置、连接、事务或会话状态。唯一长期状态是 `HEAP_PROFILER` 及 `rpprof` 依赖内部的进程级采样状态：

- 分配器的底层状态是 `System`，生命周期与进程一致。
- `start()` 打开全局原子开关；当前文件没有对应的 `stop()` 或 `reset()`，所以正常启动后采样持续到其他代码显式关闭它或进程结束。
- `rpprof` 的分配路径先完成真实分配，再在指针非空且 profiling 已启用时按采样率记录样本。未启用时依赖源码说明主要是一次原子检查；本文件不修改默认采样率。
- 参数、配置和服务器状态均从 `lib.rs` 下游进入；本文件不复制这些数据，因而不存在壳层与库入口之间的配置同步问题。

## 依赖与调用关系

上游只有 Cargo/进程启动机制：`cmd/tidb-server/Cargo.toml` 的 `[[bin]]` 选择本文件，外部脚本通过构建或运行该二进制触发它。`cmd/tidb-server/main_test.rs` 的 `mysql_compatibility_external_gate_targets_the_real_server_binary` 还核对 `scripts/test-mysql-compat.sh` 会构建 `astersql-cmd-tidb-server` 这个真实二进制，说明集成门禁不是只调用库函数。

直接下游有两项：

- `rpprof::alloc` 提供 `AllocProfiler::system()` 与 `start()`；`Cargo.toml` 和 `Cargo.lock` 固定其来源及版本。
- `astersql_cmd_tidb_server` 是本包的库 crate。其 `lib.rs::main` 接入 FIPS 初始化和 `main.rs` 的服务器主流程；这是本文件与 SQL 服务实现之间唯一的业务调用边。

RustCodeGraph 将本文件识别为 14 行、3 个符号，并显示目标文件没有被其他已索引源码文件引用，这与“Cargo 二进制入口由清单接线、无需源码调用者”的性质一致。图查询能读取目标文件和定位 `HEAP_PROFILER`、`main`，但用返回的 `main` 符号 ID继续执行 callers/callees 时错误解析到了 `pkg/parser/ast/base.rs::functionExpression`，因此本文没有把该异常输出当作调用边；上述调用关系均由目标源码、`Cargo.toml`、`lib.rs` 和 `main.rs` 直接核验。

## 错误处理与边界

本文件没有 `Result`、`?`、显式日志或恢复逻辑。`rpprof::alloc::start()` 在所锁定版本中返回 `()`，只是切换原子状态；因此这里没有可处理的 profiler 启动错误。

边界行为主要来自下游：`lib.rs::main()` 的 FIPS 初始化在显式请求 FIPS 而 provider 不可用时 panic；`entry::main()` 则忽略 `run_main()` 的整数返回值。帮助、版本、配置错误、信号退出码和服务清理均由 `main.rs` 决定，不应在本壳层重复实现。

分配器属于 Rust 程序的全局唯一资源：同一二进制不能再声明第二个 `#[global_allocator]`。替换它会影响整个进程的分配路径和性能，必须检查所有平台支持、采样开销以及堆报告消费者。本文没有证据表明 Go 二进制具备相同的 `rpprof` 分配采样，因此不能把该能力描述为 Go/Rust 共同行为。

## 并发与资源生命周期

`main()` 自己不创建线程、异步任务、锁或通道。调用顺序是严格同步的：先启用采样，再进入 FIPS 和服务器初始化。因此由下游启动的后台任务及服务线程产生的 Rust 堆分配，从创建之初就处于可采样状态。

并发安全由 `rpprof` 内部承担：启动开关是原子状态，采样器内部维护分片记录；当前文件既不持锁也不暴露共享引用。`HEAP_PROFILER` 静态项在进程结束时随程序一同终止，本文件没有显式析构或 flush。服务器下游的 Domain、storage、listener 和信号处理生命周期由 `main.rs::run_main_inner` 及 `cleanup` 管理，不能归因于本文件；`main_test.rs` 和 `parity_test.rs` 对这些下游释放顺序有独立断言。

## 与 Go 版本的对应关系

Go 的同路径入口是 [`main.go`](main.go) 中的 `func main()`。它直接执行 flag 解析、配置初始化、存储/Domain/Server 启动和退出清理；Rust 将这些可测试逻辑迁到 `main.rs`，再增加 `lib.rs` 作为共享库入口，最后由本文件提供极薄的 Cargo 二进制壳层。因此语义对应应分两层理解：

- `bin_main.rs::main` 对应“操作系统进入 tidb-server 进程”的最外层位置。
- Go `main()` 的大部分业务编排对应 Rust 的 `main.rs::main/run_main/run_main_inner`，而不是本文件的 3 行函数体。

Go 版本没有与 `HEAP_PROFILER` 和 `rpprof::alloc::start()` 一一对应的声明；这是 Rust 二进制额外的进程级观测接线。FIPS 方面，Rust 的共享库入口调用 `fips::enable_fips_only()`，对应 Go `fips.go` 在 `boringcrypto` 构建中的匿名导入副作用。测试映射也遵循这个拆分：Go `main_test.go::TestRunMain` 可在 coverage gate 下直接调用 Go `main()`；Rust `main_test.rs::test_run_main` 为避免启动阻塞服务，调用 `entry::run_main(["tidb-server", "--help"])` 验证入口路径。

## 扩展指南

- 新增或调整服务器 CLI、初始化阶段、退出码或清理顺序时，应修改 `main.rs` 的相应函数，并同步独立的 `main_test.rs`、`parity_test.rs` 以及必要的 Go `main_test.go` 对照；不要把业务分支堆入本壳层。
- 调整 FIPS 构建策略时，应修改 `fips.rs`/`lib.rs` 并更新 `parity_test.rs` 的 fail-closed 断言，而不是绕过共享库入口。
- 更换全局分配器、修改采样启停或采样率时，本文件才是正确接入点。需要新增独立测试文件中的回归验证，至少证明分配器选择、启动时序和 profiler active 状态；当前测试没有直接覆盖 `bin_main.rs::main` 的 `rpprof::alloc::start()`。
- 若需要让 `run_main()` 的返回码成为进程退出码，应先明确改变 `lib.rs::main`/`main.rs::main` 的契约和 Go 对齐语义；不能仅在本文件猜测性调用 `process::exit`，否则可能绕过既有清理路径。
- 任何扩展都应保持二进制壳层无测试内嵌。测试继续放在同目录独立文件中，符合当前 `lib.rs` 的 `#[cfg(test)] #[path = ...]` 组织方式。

兼容性风险集中在进程级行为：全局分配器影响所有 Rust 分配，入口顺序影响 FIPS 和观测初始化，退出方式影响 supervisor 可见状态。性能风险主要是堆采样在热分配路径上的额外原子检查与抽样记录；更改默认采样率前应做专门评估。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter cmd/tidb-server` 定位 `bin_main.rs`、`lib.rs`、`main.rs`、`fips.rs` 及独立测试；`node --file cmd/tidb-server/bin_main.rs --offset 1 --limit 220` 确认目标文件全文和 3 个符号；`query HEAP_PROFILER` 与大范围 `query main` 定位静态项和入口。callers/callees 的符号 ID 解析异常已在“依赖与调用关系”中限定，不作为事实来源。
- 清单与锁定依赖：`cmd/tidb-server/Cargo.toml` 的 `[lib]`、`[[bin]]`、`rpprof` 依赖；根 `Cargo.lock` 的 `rpprof 0.18.1` Git tag/提交记录。
- Rust 入口：`cmd/tidb-server/bin_main.rs::{HEAP_PROFILER, main}`、`cmd/tidb-server/lib.rs::main`、`cmd/tidb-server/fips.rs::{enable_fips_only, enable_fips_only_for_build}`、`cmd/tidb-server/main.rs::{main, run_main, run_main_inner}`。
- 依赖实现：本机 Cargo checkout 中锁定提交的 `rpprof/src/alloc.rs::{AllocProfiler, GlobalAlloc impl, start}`，用于核对系统分配器包装、原子启用和采样边界；这些结论没有从 API 名称猜测。
- Go 对照：`cmd/tidb-server/main.go::main`、`cmd/tidb-server/fips.go`、`cmd/tidb-server/main_test.go::{TestMain, TestRunMain}`。
- Rust 独立测试：`cmd/tidb-server/main_test.rs::{test_run_main, mysql_compatibility_external_gate_targets_the_real_server_binary}` 验证可测试入口和真实二进制构建门禁；`cmd/tidb-server/parity_test.rs::go_rust_public_contract_matches` 及其 FIPS、入口、错误和清理子场景验证共享库下游契约。未找到直接断言 `HEAP_PROFILER` 或 `rpprof::alloc::start()` 的测试，因此该缺口已在扩展建议中明确记录。
- 本任务是纯文档分析，按计划不运行 Cargo、Go 测试或代码构建；最终使用任务指定的 11 章节结构命令验证文档形状，并人工复核只新增本说明文件、未修改源码/Cargo/总计划。
