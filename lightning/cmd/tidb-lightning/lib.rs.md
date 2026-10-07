# `lightning/cmd/tidb-lightning/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-lightning-cmd-tidb-lightning` 的库 crate 根。其 crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 明确指定；同一清单还把 [`bin_main.rs`](bin_main.rs) 声明为二进制入口。二进制的 `main()` 调用 `astersql_lightning_cmd_tidb_lightning::main()`，本文件的同名公开函数再调用 [`main.rs`](main.rs) 中被命名为 `entry` 的模块，因此实际入口链为：

`bin_main.rs::main` → `lib.rs::main` → `entry::main`。

它属于 `lightning/cmd` 工具入口层，是 Go `lightning/cmd/tidb-lightning` 主包的 Rust crate 装配面，不是 Lightning 导入算法、HTTP 服务或配置解析的实现文件。Cargo 元数据也将其标记为 `kind = "binary"`，Go 包映射为 `lightning/cmd/tidb-lightning`。

## 核心职责

本文件只承担四项边界职责：

1. 用 `#[path = "stubs.rs"] pub mod stubs` 公开迁移期入口适配层；该模块承载退出钩子、配置、内存、GC、信号等 Go 风格接口。
2. 用 `#[path = "fips.rs"] pub mod fips` 公开 FIPS 初始化接线点；当前非 boringcrypto 对应实现是有意的空操作。
3. 用 `#[path = "main.rs"] pub mod entry` 把真实 CLI 启动控制流置于稳定的 `entry` 命名空间。
4. 提供公开的零参数 `main()`，让二进制包装层与其他库调用者共享同一启动入口。

文件顶部的 crate 级 `#![allow(...)]` 放宽未使用项、Go 风格命名以及 Clippy 告警。这是迁移边界的编译策略，不表示下游模块中的逻辑已经完整移植；尤其 [`fips.rs`](fips.rs) 明确说明当前尚未接入 Rust FIPS TLS provider，而 [`stubs.rs`](stubs.rs) 集中保存了入口所需的兼容实现。

## 主要符号

- `pub mod stubs`：公开模块，物理来源为 [`stubs.rs`](stubs.rs)。`main.rs` 通过 `crate::stubs::{config, debug, memory, os_signal}` 以及 `stubs::exit`、`stubs::logger_sync` 使用它，测试也用其可注入钩子隔离进程级副作用。
- `pub mod fips`：公开模块，物理来源为 [`fips.rs`](fips.rs)。其主要公开符号 `init_fips_only_tls_for_boringcrypto_build()` 是未来 FIPS provider 的固定接线点。
- `pub mod entry`：公开模块，物理来源为 [`main.rs`](main.rs)。真实入口、可测试控制流和应用抽象分别位于 `entry::main()`、`entry::run()`、`entry::run_with_factory()` 与 `entry::LightningApp`。
- `mod parity_test`：仅在 `cfg(test)` 下编译，物理来源为 [`parity_test.rs`](parity_test.rs)，验证 Rust 入口控制流与 Go `main.go`/`fips.go` 的可观察契约。
- `mod main_test`：仅在 `cfg(test)` 下编译，物理来源为 [`main_test.rs`](main_test.rs)，对齐 Go `TestRunMain` 的参数过滤、退出替换和等待语义。
- `pub fn main()`：本文件唯一函数和唯一直接执行语句；函数无参数、无返回值，只调用 `entry::main()`。

本文件没有常量、类型、trait、`impl`、宏定义或 feature 条件；条件编译仅用于两个独立测试模块。

## 执行流程

生产构建中的流程非常短：

1. Cargo 按 [`Cargo.toml`](Cargo.toml) 构建库 crate 和 [`bin_main.rs`](bin_main.rs) 所定义的同名二进制。
2. 操作系统进入 `bin_main.rs::main()`。
3. 二进制通过库 crate 名调用本文件的 `main()`。
4. 本文件无条件把控制权转交给 `entry::main()`，自身不拦截返回、不转换退出码，也不添加清理动作。
5. 此后的 FIPS 初始化、参数读取、配置装载、应用构造、信号线程、HTTP 服务、server/run-once 分支、日志同步及退出码判定均在 [`main.rs`](main.rs) 中完成，不属于本文件内部逻辑。

测试构建额外把 [`parity_test.rs`](parity_test.rs) 和 [`main_test.rs`](main_test.rs) 作为 crate 私有模块编入，因此它们可以通过 `crate::entry`、`crate::fips` 和 `crate::stubs` 检验同一装配边界，而不是复制生产模块。

## 数据与状态

本文件不定义或持有任何数据结构、全局变量、缓存、锁、通道或配置值；`main()` 也不接收或返回数据。它唯一传递的是控制权。

进程级可变状态存在于下游而非本文件：例如 `entry::main()` 读取 `std::env::args()`，`stubs` 保存可替换退出钩子和测试状态，`entry::run_with_factory()` 用 `Arc<Mutex<_>>` 共享 Lightning 应用。文档和扩展代码应保持这一归属区分，避免把下游状态误认为 crate 根状态。

`pub mod` 使 `stubs`、`fips` 和 `entry` 成为该库的公开 API 面；两个测试模块则保持私有且只在测试配置出现。这一可见性差异是本文件最重要的静态接口状态。

## 依赖与调用关系

直接上游是 [`bin_main.rs`](bin_main.rs)：其 `main()` 明确调用 `astersql_lightning_cmd_tidb_lightning::main()`。仓库文本检索没有发现其他 Rust 生产调用点；邻近测试绕过 crate 根 `main()`，直接调用 `crate::entry::run()` 或 `run_with_factory()`，以避免真实进程退出和不可控环境副作用。

本文件的唯一直接调用边是 `lib.rs::main → entry::main`。三个 `#[path]` 声明形成编译期包含关系，而不是运行时调用。进一步的依赖都属于被包含模块：

- [`main.rs`](main.rs) 依赖 `astersql-lightning-pkg-progress` 和 `astersql-lightning-pkg-server`；这两项也是 [`Cargo.toml`](Cargo.toml) 列出的全部直接包依赖。
- `main.rs` 从 `crate::stubs` 使用配置、调试、内存、信号、退出和日志同步适配面。
- `main.rs` 在进入主体控制流前调用 `crate::fips::init_fips_only_tls_for_boringcrypto_build()`。

RustCodeGraph 对 `lib.rs` 的文件节点识别出 43 行和两个节点（文件与 `main` 函数）。对精确函数节点执行 `callers`/`callees` 未产生静态边，因此上述两条运行时边以调用点源码为准：[`bin_main.rs`](bin_main.rs) 第 8 行和 [`lib.rs`](lib.rs) 第 42 行。

## 错误处理与边界

`lib.rs::main()` 没有 `Result` 返回值、错误分支、`panic` 处理或恢复策略。`entry::main()` 正常返回时它直接返回；下游调用 `process::exit`、记录退出码或发生 panic 时，本层也不介入。因此不能仅凭本函数的空返回类型推断进程总是成功。

实际错误与退出边界由 [`main.rs`](main.rs) 负责：配置早退、应用创建失败、HTTP 服务启动失败、导入错误、上下文取消、文件日志同步失败各自有不同语义。特别是 Go 对齐逻辑保留了 `GoServe` 失败后普通 `return` 的零退出状态，而运行错误或 server mode 取消会走退出码 1。那些规则由 [`parity_test.rs`](parity_test.rs) 验证，但不应移入 crate 根。

模块边界还有两项限制：`stubs` 是迁移兼容面，不能被描述为完整生产子系统；`fips` 当前为空接线点，不能声称 Rust 已支持 Go boringcrypto 构建。未来改变这两者时，应在各自文件中实现并保留本文件的稳定装配接口。

## 并发与资源生命周期

本文件本身不创建线程、不获取锁、不打开文件或网络资源，也没有显式析构/清理阶段。它的生命周期与库 crate 一致：模块在编译期装配，公开 `main()` 在每次调用时同步转发一次。

入口调用后的并发和资源生命周期位于 [`main.rs`](main.rs)：应用被放入 `Arc<Mutex<_>>`，可选信号线程等待 `SIGHUP`、`SIGINT`、`SIGTERM` 或 `SIGQUIT` 后调用 `Stop()`；主线程启动 HTTP 服务并运行 server 或一次性任务；文件日志在结束时尝试同步。[`parity_test.rs`](parity_test.rs) 的 `real_signal_wait_matches_go` 和 `contract_resource_cleanup_sync_and_cancel` 分别覆盖真实信号等待、注入信号触发 `Stop()`、取消语义及日志同步不覆盖主结果等边界。

[`main_test.rs`](main_test.rs) 通过子进程和工作线程隔离退出钩子与测试框架参数，说明直接测试 `entry` 比调用 crate 根 `main()` 更安全。新增 crate 根逻辑若引入资源，必须明确它是在转发前建立、由谁清理，以及测试子进程能否可靠退出；目前没有这类额外资源。

## 与 Go 版本的对应关系

Go 版本的 [`main.go`](main.go) 以 `package main` 直接实现完整入口；Rust 将同一职责拆成三层：二进制薄包装 [`bin_main.rs`](bin_main.rs)、本文件的库门面、以及对齐 Go 控制流的 [`main.rs`](main.rs)。所以 `lib.rs::main()` 对应的是 Go `main()` 的公开进程入口身份，而不是 Go 函数体逐句翻译；逐句行为映射位于 `entry::main()` 和 `entry::run_with_factory()`。

Go [`fips.go`](fips.go) 仅在 `boringcrypto` build tag 下通过空白导入触发 `crypto/tls/fipsonly` 副作用。Rust 始终装配 `fips` 模块并无条件调用一个稳定函数，但该函数当前为空，语义只等价于非 boringcrypto Go 构建。这个迁移差异已由 `fips.rs` 明示。

Go [`main_test.go`](main_test.go) 可替换包级 `exit`、改写 `os.Args` 并用 goroutine 等待 `main()` 返回；Rust [`main_test.rs`](main_test.rs) 无法安全地做完全相同的进程全局改写，因此用 `stubs` 退出钩子、参数过滤、子进程和线程复现可观察约束。Rust [`parity_test.rs`](parity_test.rs) 还覆盖正常 run-once、配置边界、GOGC/日志文件、错误路径、取消与资源清理，这是对入口迁移语义的补充保护。

## 扩展指南

- 新增 CLI 运行逻辑、退出码规则或模式分支时，优先修改 [`main.rs`](main.rs) 的 `entry::run_with_factory()`，并在 [`parity_test.rs`](parity_test.rs) 增加对应的正常、边界、错误或资源生命周期场景；不要让 `lib.rs::main()` 膨胀成第二份入口实现。
- 新增进程级适配能力时，判断其归属：Go/Rust 环境兼容层放入 [`stubs.rs`](stubs.rs)，FIPS TLS 初始化放入 [`fips.rs`](fips.rs)，业务服务能力应进入 `lightning/pkg` 的实际 crate。只有需要成为稳定库 API 时才从本文件公开模块或函数。
- 若要改变模块文件名或可见性，必须同步 `#[path]` 声明、`crate::entry`/`crate::stubs`/`crate::fips` 使用点、Cargo 的库/二进制入口以及两个独立测试文件。不要把 Rust 单元测试内嵌到 `lib.rs`；本仓库要求源文件与测试逻辑分离。
- 若接入真正的 FIPS provider，应使用明确 Cargo feature 或平台条件并更新 [`fips.rs`](fips.rs) 与 parity 测试；不得把当前空函数描述成已启用 FIPS。
- crate 根新增 fallible 初始化会改变当前“透明转发”不变量。若确有必要，应明确错误到退出码的映射，并验证不会绕过 `entry::main()` 中既有的日志、信号和清理顺序。
- 性能风险主要来自误在 crate 根重复初始化或重复启动后台任务；当前一层函数调用本身没有可观测性能成本。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 7032 个 Rust 文件；`files --filter lightning/cmd/tidb-lightning` 确认 `lib.rs`、`bin_main.rs`、`main.rs`、`fips.rs`、`stubs.rs` 及两个 Rust 测试均已索引；`node --file lightning/cmd/tidb-lightning/lib.rs --offset 1 --limit 240` 核对了完整 43 行源码与唯一函数；精确 `query main --kind function` 核对了三个入口函数的位置；精确 callers/callees 查询没有返回静态调用边，故调用关系再由源码调用点确认。
- crate 与入口：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`bin_main.rs`](bin_main.rs)、[`main.rs`](main.rs)。这些文件共同证明库/二进制双目标、三个公开模块和两级转发链。
- Go 对照：[`main.go`](main.go)、[`fips.go`](fips.go)、[`main_test.go`](main_test.go)。它们分别证明 Go 主流程、boringcrypto 条件副作用与 `TestRunMain` 的退出/参数/等待约束。
- Rust 独立测试：[`parity_test.rs`](parity_test.rs) 的 `go_rust_public_contract_matches`、`real_signal_wait_matches_go` 及四组 `contract_*` 场景；[`main_test.rs`](main_test.rs) 的 `test_run_main`。测试直接覆盖 `entry` 和兼容模块，本文件通过 `cfg(test)` 负责挂载它们。
- 本任务为纯文档分析，按计划不运行 Cargo；验收使用固定十一个二级标题的结构检查，并人工复核链接、符号归属、迁移限制和扩展入口。
