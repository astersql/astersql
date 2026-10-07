# `br/pkg/utils/dyn_pprof_other.rs`

## 文件定位

该文件是 `astersql-br-pkg-utils` library crate 的非 Unix 平台适配单元，crate 边界由 [`br/pkg/utils/Cargo.toml`](Cargo.toml) 定义，模块由 [`br/pkg/utils/lib.rs`](lib.rs) 以 `#[path = "dyn_pprof_other.rs"] pub mod dyn_pprof_other;` 挂载。它只在不满足 Linux、macOS、FreeBSD 且不具有 Rust `unix` cfg 的目标上提供函数实体，与 [`dyn_pprof_unix.rs`](dyn_pprof_unix.rs) 形成平台互斥的实现对。

该文件当前是兼容桩，而不是 pprof 服务器的实现。RustCodeGraph 对 `br/pkg/utils/dyn_pprof_other.rs::StartDynamicPProfListener` 未找到调用边；`lib.rs` 也没有在 crate 根重导出该函数，因此当前只能通过平台上存在的 `astersql_br_pkg_utils::dyn_pprof_other::StartDynamicPProfListener` 路径访问。

## 核心职责

- 在无 POSIX 信号支持的目标上保留“启动动态 pprof 监听”这一 API 形状，调用时安全地不做任何事情。
- 避免非 Unix 平台引入 `signal_hook::iterator::Signals`、`SIGUSR1`、后台线程和 status/pprof 监听器的运行时行为。
- 对齐 Go 文件 [`dyn_pprof_other.go`](dyn_pprof_other.go) 的平台降级语义：接收 TLS 配置，但不启动监听、不返回错误。

## 主要符号

- `use astersql_util::security::TLS`：引入与 BR status 监听器共用的 TLS 配置类型。依赖由 `Cargo.toml` 中的 `astersql-util = { path = "../../../pkg/util" }` 提供。
- `#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd", unix)))]`：函数级条件编译门。只要目标具有 `unix` cfg，即使未单独列出 OS 名称，该实现也不会出现。
- `pub fn StartDynamicPProfListener(_tls: Option<&TLS>)`：文件中唯一的公开函数。它借用一个可选 `TLS`，函数体为空，返回类型为 `()`。参数名前缀 `_` 表明有意不读取。命名保留 Go 导出函数风格，crate 根的 `#![allow(non_snake_case)]` 允许该形式。

文件中没有常量、struct、enum、trait、`impl`、内部辅助函数或静态状态。

## 执行流程

1. Cargo 为 `astersql-br-pkg-utils` 编译 `lib.rs`，`lib.rs` 声明 `dyn_pprof_other` 模块。
2. 编译器评估 `StartDynamicPProfListener` 上的 cfg。在 Unix 目标上，函数被完全排除；在非 Unix 目标上，函数被编入。
3. 如果非 Unix 调用方通过模块路径调用函数，传入 `None` 或 `Some(&TLS)` 均立即返回 `()`。
4. 整个过程不注册信号、不创建 socket、不启动线程，也不启动 `StartStatusListener`。

需要区分 Go 的完整主链与 Rust 当前接线：Go [`br/cmd/br/cmd.go`](../../cmd/br/cmd.go) 在没有显式 `status-addr` 时调用 `utils.StartDynamicPProfListener(tls)`；Rust [`br/cmd/br/cmd.rs`](../../cmd/br/cmd.rs) 虽有对应调用，但 `utils` 来自其本地 [`stubs.rs`](../../cmd/br/stubs.rs)，实际命中的是另一个空桩，不是本文件的函数。

## 数据与状态

该实现不保存数据、不修改全局状态，也不观察 `TLS` 内容。`Option<&TLS>` 只是一个借用的输入形状：

- `None` 和 `Some` 的运行结果相同。
- 函数不获取 `TLS` 所有权，不延长其生命周期，不克隆、分配或缓存任何配置。
- 没有与调用次数相关的状态；重复调用始终是等价的空操作。

## 依赖与调用关系

- 上游模块声明：`br/pkg/utils/lib.rs` 无条件声明 `dyn_pprof_other` 模块；函数本身再用 cfg 决定是否存在。
- 静态调用者：RustCodeGraph `callers br/pkg/utils/dyn_pprof_other.rs::StartDynamicPProfListener` 无输出，全库 `rg` 也未找到指向 `dyn_pprof_other::StartDynamicPProfListener` 的 Rust 调用。
- 下游调用：函数体为空，RustCodeGraph `callees` 无输出；唯一类型依赖是 `astersql_util::security::TLS`。
- 平台对应：`dyn_pprof_unix.rs::StartDynamicPProfListener` 在 Unix 上注册 `SIGUSR1`，并在后台线程中调用 `crate::pprof::StartStatusListener("0.0.0.0:0", tls.as_deref())`。本文件故意不依赖这些组件。
- crate 依赖：`signal-hook`、`pprof` 和日志 crate 虽在 `br/pkg/utils/Cargo.toml` 中存在，但本文件的空实现不直接使用它们。

## 错误处理与边界

该 API 不返回 `Result`，也没有可触发的错误分支。它的边界就是“目标平台不提供 Unix 信号驱动的动态 pprof”：

- 传入空 TLS 或实际 TLS 都不会验证证书、打开端口或产生错误。
- 调用方无法从返回值区分“不支持”与“已启动”；这是与 Go 空实现一致的兼容契约，不应擅自改成错误。
- cfg 条件是可用性边界。在 Unix 上直接引用本模块中的该函数会因符号不存在而无法编译；调用方必须使用相同的平台 cfg，或先在 crate 入口建立统一的条件重导出。

## 并发与资源生命周期

本实现不创建线程、任务、通道、锁、socket、信号注册或 pprof 资源，因而没有启动、取消、join 或释放顺序。调用在当前线程中同步结束，不会将 `&TLS` 逃逸到函数之外。

这与 Unix 版形成明确对比：Unix 版把可选 `Arc<TLS>` 移入 `thread::spawn` 闭包，并让 `Signals::forever()` 持续等待；非 Unix 版不需要 `Arc`，因为它根本不保留 TLS。

## 与 Go 版本的对应关系

Rust 文件直接对应 `br/pkg/utils/dyn_pprof_other.go`：

- Go build tag 是 `!linux && !darwin && !freebsd && !unix`；Rust cfg 是对同一组条件的 `not(any(...))`，平台选择语义一致。
- Go 签名接收 `*tidbutils.TLS`，Rust 用 `Option<&TLS>` 表达可空且不取所有权的对应参数。
- 两者的函数体均无操作，都不返回错误。
- Go 包按 build tag 为相同的包级名称二选一提供 `StartDynamicPProfListener`；Rust 当前则暴露 `dyn_pprof_other` 和 `dyn_pprof_unix` 两个模块，且没有条件重导出统一函数。因此“实现语义已对齐”不等于“Rust 主调用链已接入”。

测试方面，同目录未发现 `dyn_pprof_other_test.rs`。[`dyn_pprof_unix_test.rs`](dyn_pprof_unix_test.rs) 只验证 Unix 辅助函数在收到 `SIGUSR1` 时调用启动回调，不覆盖本非 Unix 空实现。Go [`br/cmd/br/main_test.go`](../../cmd/br/main_test.go) 中对 Unix goroutine 名称的 goleak 忽略同样不是本文件的回归测试。

## 扩展指南

- 如果只是新增一个非 Unix 目标，首先核对 Rust 的 `target_family`/`target_os` cfg 与 Go build tag，不要在两个平台实现中造成同时编译或同时缺失。
- 如果要把该功能接入 Rust BR 命令主链，应在 `br/pkg/utils/lib.rs` 中用平台 cfg 建立唯一的 `StartDynamicPProfListener` 重导出，再让 `br/cmd/br` 依赖真实 utils crate；不应继续增强 `br/cmd/br/stubs.rs` 的占位函数来代替真实接线。这属于后续迁移工作，不是本文档任务已实现的现状。
- 如果非 Unix 版未来需要真正启动服务，必须先定义该平台上的可观测启动机制、端口暴露和 TLS 生命周期，并重新评估不返回 `Result` 的 Go 兼容契约。
- 测试仍应放在独立文件，不要内嵌到生产 `.rs` 中。对空实现的最小回归应使用可编译非 Unix 目标验证 `None` 和 `Some(&TLS)` 都能调用且无副作用；统一导出接线则应增加编译期 API 回归。
- 保留源文件现有 PingCAP Apache License 头和 `// Copyright 2026 AsterSQL.`，不要为了文档化而调整生产代码。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件，其中 7032 个 Rust 文件；目标文件可通过 `node --file br/pkg/utils/dyn_pprof_other.rs` 读取。
- 符号查询：`query StartDynamicPProfListener --kind function` 同时命中 Go 非 Unix/Unix 实现、Rust 非 Unix/Unix 实现和 BR 命令桩；精确目标的 `callers` 与 `callees` 查询均无输出。
- 已读生产路径：`br/pkg/utils/dyn_pprof_other.rs`、`br/pkg/utils/dyn_pprof_unix.rs`、`br/pkg/utils/lib.rs`、`br/pkg/utils/Cargo.toml`、`br/cmd/br/cmd.rs`、`br/cmd/br/stubs.rs`。
- 已读 Go 对照：`br/pkg/utils/dyn_pprof_other.go`、`br/pkg/utils/dyn_pprof_unix.go`、`br/cmd/br/cmd.go`。
- 已读测试证据：`br/pkg/utils/dyn_pprof_unix_test.rs`；通过全库引用搜索确认没有同名非 Unix Rust 独立测试。`br/cmd/br/main_test.go` 只有 Unix 监听 goroutine 的 goleak 忽略证据。
- 人工复核结论：文档区分了空实现的真实行为、Unix 对应实现、Go 主链与 Rust 当前桩调用链；未将未接线功能表述为已支持。
