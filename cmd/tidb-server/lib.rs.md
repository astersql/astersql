# `cmd/tidb-server/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-cmd-tidb-server` 的库 crate 根。`Cargo.toml` 用 `[lib] path = "lib.rs"` 声明该入口，并用 `[[bin]] path = "bin_main.rs"` 将同包的可执行文件与库分开。这个文件不实现 SQL 服务器主流程；它把 `stubs.rs`、`fips.rs` 和 `main.rs` 组装为库模块，再提供给 `bin_main.rs` 和 crate 内测试一个统一入口。

该 Cargo 包的 porting 元数据将 Go 对照包定位为 `cmd/tidb-server`、类型定位为 `binary`。因此 `lib.rs` 是 Rust 为可测试性增加的 crate 边界，不是 Go 版中独立存在的业务文件。

## 核心职责

1. 通过 `#[path]` 把相邻文件暴露为公开模块：`stubs`、`fips` 和名为 `entry` 的 `main.rs`。
2. 通过 `extern crate self as astersql_cmd_tidb_server` 为当前 crate 建立稳定的自别名，使 crate 内代码或测试语境可用与包名一致的路径引用 crate 根。
3. 在库级 `main()` 中锁定启动顺序：先执行 `fips::enable_fips_only()`，再执行 `entry::main()`。
4. 仅在 `cfg(test)` 下把 `parity_test.rs` 和 `main_test.rs` 纳入 crate，让独立测试文件能访问 `crate::entry`、`crate::fips` 和 `crate::stubs`，而不把测试逻辑放入生产源文件。

crate 根上的 `#![allow(...)]` 将死代码、Go 风格命名、未使用项及 Clippy 警告的宽免传递给整个 crate。这是迁移期兼容措施，不代表所有被组装能力都已由生产路径完整使用。

## 主要符号

- `extern crate self as astersql_cmd_tidb_server`：当前 crate 的自别名。它不创建第二个 crate 实例，也不持有状态。
- `pub mod stubs`：映射到 `stubs.rs`。`main.rs` 通过 `crate::stubs` 使用大量迁移期边界适配与可观测测试替身；不应由 `lib.rs` 的公开性推断它们全部是 canonical 生产实现。
- `pub mod fips`：映射到 `fips.rs`，实现 FIPS-only 构建的早期加密 provider 安装策略。
- `pub mod entry`：将 `main.rs` 以 `entry` 模块名纳入，其 `entry::main()` / `entry::run_main()` / `entry::run_main_inner()` 才是实际启动编排的递进入口。
- `mod parity_test` 与 `mod main_test`：仅测试构建可见的私有模块，分别承载 Go/Rust 公开契约回归和 Go `main_test.go` 语义的 Rust 对照测试。
- `pub fn main()`：返回类型为 `()` 的库级进程入口；本文件唯一的函数。

## 执行流程

1. Cargo 用 `bin_main.rs` 构建二进制目标 `astersql-cmd-tidb-server`。
2. `bin_main.rs::main()` 先调用 `rpprof::alloc::start()` 启动分配器剖析，然后调用 `astersql_cmd_tidb_server::main()`。全局 allocator 的安装也在二进制壳层，不在本文件。
3. `lib.rs::main()` 调用 `fips::enable_fips_only()`。普通构建下该步骤返回成功；当编译环境含 `ASTERSQL_FIPS_ONLY` 时，它必须安装经验证的 FIPS provider，失败则 panic，不会继续启动。
4. FIPS 预检通过后，`lib.rs::main()` 调用 `entry::main()`。
5. `entry::main()` 从 `stubs::args_from_env()` 取得参数并转入 `run_main()`；`run_main_inner()` 再负责 CLI/配置、存储、Domain、Server、信号与清理等完整流程。因此本文件的流程边界到“完成安全预检并交出控制权”为止。

## 数据与状态

`lib.rs` 没有自己的常量、结构体、trait、可变静态量、锁或通道。自别名和模块声明都是编译期组织信息；`main()` 也不接受参数或返回退出码。

进程状态实际位于下游：`entry` 管理解析后的 flag 快照、全局配置和服务生命周期，`stubs` 提供部分边界状态与测试事件记录，`fips` 只根据编译期 `option_env!` 选择安装策略。对这些状态的修改不应放进 `lib.rs`。

## 依赖与调用关系

上游直接调用者是 `bin_main.rs::main()`，调用边为 `bin_main.rs::main -> astersql_cmd_tidb_server::main`。下游直接边是 `lib.rs::main -> fips::enable_fips_only` 和 `lib.rs::main -> entry::main`。两条边的先后顺序是安全契约：启动主流程前必须先完成 FIPS 策略检查。

Cargo 边界显示该包直接依赖 `astersql-config`、`astersql-domain`、`astersql-server`、`astersql-session`、`astersql-store`、`astersql-store-driver`、`astersql-metaservice`、`astersql-util-signal`、带固定 tag 的 `tikv-client` 及 `rpprof`。其中 `rpprof` 在二进制壳层直接使用；其他依赖主要由 `entry`、`fips` 和 `stubs` 触达，不是 `lib.rs::main()` 本身的业务依赖。`nextgen` feature 仅向 `astersql-session/nextgen` 和 `astersql-store/nextgen` 透传，`lib.rs` 没有自己的条件分支。

RustCodeGraph 对目标文件的 file node 显示它含 2 个符号，并给出模块源文本；但索引将多个 crate 根的函数都简写为 `lib.rs::main`，对该同名符号执行精确 `callers` 查询未在 60 秒内返回。因此上述直接调用边以 RustCodeGraph 的 `node --file` 源码结果与 `bin_main.rs` 的明示调用交叉核对，不使用模糊的全局 `main` 结果。

## 错误处理与边界

`lib.rs::main()` 没有 `Result` 返回值，也不捕获 panic。`fips::enable_fips_only()` 将 provider 安装错误转为带 `FIPS-only initialization failed` 上下文的 panic；这是 fail-closed 边界，保证明确请求 FIPS 时不会回退到非 FIPS 加密。本文件不屏蔽该失败。

`entry::main()` 内部以 `let _ = run_main(...)` 调用可测试入口；退出码计算、部分显式 `process::exit` 路径及启动/清理错误的细节属于 `main.rs`，而非本门面。向 `lib.rs::main()` 增加错误吞噬或不同的退出策略会导致库调用和二进制启动语义分叉。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、存储连接或网络监听器。它只规定两个同步调用的先后关系，并将之后的资源所有权交给 `entry::main()`。

完整资源生命周期在 `main.rs::run_main_inner()` 及其调用的 `cleanup` 路径中；`main_test.rs` 使用可观测假后端检查 PD、safe point 和 store 等资源的关闭顺序，`parity_test.rs::contract_resource_cleanup` 检查 server、resource manager、CPU profiler 和 executor 等收尾事件。这些测试验证了门面交出控制权后的下游契约，但不表示 `lib.rs` 本身管理这些资源。

## 与 Go 版本的对应关系

Go 版在 `cmd/tidb-server/main.go::main()` 中直接执行参数解析、配置初始化、组件启动与退出清理，没有与 Rust `lib.rs` 一对一的库门面。Rust 将 Go `main` 主体移到 `main.rs::entry::main/run_main/run_main_inner`，再用本文件让二进制和测试复用同一套模块与入口。

Go `fips.go` 仅在 `boringcrypto` build tag 下通过匿名导入 `crypto/tls/fipsonly` 触发初始化副作用。Rust 对照是 `lib.rs::main()` 在主启动前显式调用 `fips::enable_fips_only()`，并以 `ASTERSQL_FIPS_ONLY` 编译环境标记决定是否要求 FIPS provider。两者的实现机制不同，但共同契约是在普通主流程前完成加密模式约束。

Go `main_test.go::TestRunMain` 在特定 coverage gate 下直接调用 Go `main()`。Rust 对照测试不直接启动不可控进程，而是由 `lib.rs` 纳入 `main_test.rs` 和 `parity_test.rs`，主要调用 `entry::run_main()` 和更细的可测试符号验证契约。

## 扩展指南

- 新增完整启动阶段时，优先修改 `main.rs::run_main_inner()` 并在 `main_test.rs` 或 `parity_test.rs` 增加独立回归；只有必须在所有启动逻辑之前执行的进程级安全预检，才应接到 `lib.rs::main()`。
- 新增顶层模块时，应根据真实 API 需求决定 `pub mod` 或私有 `mod`；不要因为测试方便而无条件扩大生产公开面。
- 修改 FIPS 顺序或失败策略时，必须同步 `fips.rs` 与 `parity_test.rs` 中 `enable_fips_only_for_build` 的普通/显式请求分支，并核对 Go `fips.go` 的 build-tag 语义。安全风险是静默降级，启动兼容风险是 provider 重复安装。
- 修改入口签名或错误/退出码传递时，要同时检查 `bin_main.rs`、`main.rs::main/run_main`、Go `main.go` 和 coverage/entry 相关测试，避免可执行文件与库模式分叉。
- 保持 Rust 生产逻辑与测试逻辑分文件；本 crate 已用 `#[cfg(test)] #[path = ...]` 提供接线，新回归应继续放在对应的 `*_test.rs` / `parity_test.rs` 中。
- 调整 crate 级 `allow` 时要分阶段评估整个 `entry`/`stubs` 的 Go 命名兼容面；一次性移除会产生大量非行为性噪声，而无限扩大豁免则会隐藏新代码问题。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件，其中 7032 个 Rust 文件；目标 `cmd/tidb-server/lib.rs` 在索引中且完整显示 1–45 行。
- RustCodeGraph 查询：`status`、`files --filter cmd/tidb-server`、`node --file cmd/tidb-server/lib.rs`、`node --file cmd/tidb-server/bin_main.rs`、`node --file cmd/tidb-server/main.rs --offset 560`、`node --file cmd/tidb-server/fips.rs`、`node --file cmd/tidb-server/parity_test.rs` 和 `node --file cmd/tidb-server/main_test.rs`。对歧义名 `lib.rs::main` 的 `callers` 查询超过 60 秒未返回，已改用带文件路径的 node 结果与明示源码调用核对。
- 生产源码：`cmd/tidb-server/lib.rs`、`bin_main.rs`、`main.rs`、`fips.rs` 和 `stubs.rs`。关键边界是 `bin_main.rs::main -> lib.rs::main -> fips::enable_fips_only -> entry::main -> entry::run_main`。
- Cargo 边界：`cmd/tidb-server/Cargo.toml` 中的 `[lib]`、`[[bin]]`、`[features]` 与 `[dependencies]`。
- Go 对照：`cmd/tidb-server/main.go::main`、`cmd/tidb-server/fips.go` 的 `boringcrypto` build tag 及匿名 `fipsonly` 导入。
- 独立测试：`cmd/tidb-server/parity_test.rs::go_rust_public_contract_matches`、`contract_resource_cleanup`、FIPS 分支断言；`cmd/tidb-server/main_test.rs` 的入口、配置、驱动接线和资源顺序测试；Go `cmd/tidb-server/main_test.go::TestRunMain` 及相关入口契约测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 与固定二级标题计数命令验证文档结构。
