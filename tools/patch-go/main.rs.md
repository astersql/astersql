# `tools/patch-go/main.rs`

源文件：[`tools/patch-go/main.rs`](main.rs)

## 文件定位

该文件是 Cargo 包 `astersql-tools-patch-go` 的二进制入口。`tools/patch-go/Cargo.toml` 通过 `[[bin]]` 将二进制名 `astersql-tools-patch-go` 映射到本文件，同时以 `lib.rs` 作为同包库入口。根 `Cargo.toml` 又将 `tools/patch-go` 列为 workspace 成员。因此，本文件位于进程启动边界，不实现补丁探测本身，只把 Cargo/操作系统启动的进程转交给库侧统一入口 `astersql_tools_patch_go::entry()`。

它是一个有意保持极薄的二进制门面：真实探针逻辑依次位于 `lib.rs::entry`、`check.rs::main`、`check.rs::run_main` 和 `check.rs::grunningnanos`。这也意味着不能仅凭本文件判断 Go 运行时补丁是否存在，必须继续检查上述库调用链。

## 核心职责

本文件只有一项职责：在 Rust 二进制启动时调用一次 `astersql_tools_patch_go::entry()`。入口不解析参数、不产生输出、不判断返回值，也不在二进制层复制探针逻辑。

这一单层转发让二进制执行与库/测试可复用路径共享实现：`lib.rs::entry()` 再调用 `check::main()`，后者最终触发运行时纳秒探针。边界清晰的直接收益是，探针行为发生变化时应修改库侧实现，而不是让二进制入口与测试路径分别演化。

## 主要符号

- `fn main()`：本文件唯一的模块级符号，也是私有的 Rust 进程入口。它无参数、无返回值，函数体仅调用 `astersql_tools_patch_go::entry()`。
- `astersql_tools_patch_go`：由 Cargo 包名 `astersql-tools-patch-go` 在 Rust 路径中按连字符转下划线规则形成的库 crate 名称；本文件通过绝对 crate 路径调用其公开函数。
- 本文件没有常量、类型、trait、`impl`、条件编译项或公开 API。对外可复用入口是相邻 `lib.rs` 中的 `pub fn entry()`，不是这里的 `main()`。

## 执行流程

1. Cargo 构建出的 `astersql-tools-patch-go` 二进制由操作系统启动，Rust 运行时进入 `main.rs::main()`。
2. `main()` 无条件调用 `astersql_tools_patch_go::entry()`，没有命令行参数分支或提前返回路径。
3. `lib.rs::entry()` 调用 `check::main()`。
4. `check.rs::main()` 调用 `run_main()`；`run_main()` 在 `unsafe` 块中调用 `grunningnanos()` 并丢弃其 `i64` 返回值。
5. 当前 Rust 实现的 `grunningnanos()` 转发到 `stubs::runtime_grunningnanos()`。正常时调用计数增加并返回桩中的纳秒值；桩被设为不可用时会触发断言失败。

上述链路保持了 Go `check.go::main()` 的最小可观察语义：只触发一次 `grunningnanos()`，不消费返回值。本文件自身不负责 `patch-go.sh` 的版本识别或补丁应用；该脚本是调用 Go 编译探针并决定是否应用补丁的外部流程。

## 数据与状态

`main.rs` 不声明或持有任何数据：没有全局变量、局部业务状态、配置对象、参数集合或返回值。调用产生的状态位于 `stubs.rs` 的三个进程级原子变量中：`NANOS: AtomicI64` 保存模拟返回值，`CALLS: AtomicU64` 记录进入次数，`AVAILABLE: AtomicBool` 表示模拟符号是否可用。

二进制入口既不读取也不修改这些变量；它只是使下游调用发生。`parity_test.rs` 通过 `reset_for_test`、`set_nanos`、`set_available` 和 `call_count` 操作或观察这些状态，证明入口下游的结果形状、调用次数与失败边界。生产式直接执行时，当前 Rust crate 使用的是本地桩，而非真实 Go runtime 符号，这一点是理解其迁移状态的重要限制。

## 依赖与调用关系

上游方面，`main.rs::main()` 是由 Rust 二进制启动协议隐式调用的根入口，仓库内没有普通 Rust 函数直接调用它。`Cargo.toml` 的 `[[bin]]` 声明是它成为入口的直接配置依据；根 workspace 的成员声明负责将该包纳入工作区。

下游调用链为：

`main.rs::main → lib.rs::entry → check.rs::main → check.rs::run_main → check.rs::grunningnanos → stubs.rs::runtime_grunningnanos`。

`tools/patch-go/Cargo.toml` 的 `[dependencies]` 为空，因此本文件没有第三方 crate 依赖。其唯一显式依赖是同包库 crate 暴露的 `entry()`。RustCodeGraph `status` 显示索引可用，`files --filter tools/patch-go` 确认 `main.rs`、`lib.rs`、`check.rs`、`stubs.rs`、`parity_test.rs` 与 Go 对照文件均在索引集合中；精确 `callers/callees` 查询未为该薄入口返回调用边，所以这里的调用链以实际函数体和相邻入口逐层核验为准，而不虚构图边。

## 错误处理与边界

`main()` 没有 `Result` 返回值、错误转换、日志或恢复分支。若 `entry()` 下游 panic，本文件不会捕获，进程会按 Rust 默认 panic 行为失败；这符合探针不应把缺失符号静默降级为成功的意图。

当前下游桩在 `AVAILABLE == false` 时由 `runtime_grunningnanos()` 的 `assert!` 触发 panic。Go 版本则依靠 `//go:linkname` 引用 `runtime.grunningnanos`：未打补丁的工具链无法满足目标符号，`patch-go.sh` 以 `go build check.go` 的成功或失败识别状态。两者的失败阶段和机制并不相同，但都遵守“探针不可用时不得静默成功”的边界。

本入口没有检查返回的纳秒值，零、负数和极值均不会在这里改变控制流。`parity_test.rs::contract_boundary` 明确覆盖 `0`、`i64::MIN`、`i64::MAX` 和 `-1`，说明返回值只需保持 Go `int64` 的形状，而不是由入口解释其业务含义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件、网络连接或事务，也没有显式清理阶段。调用是当前线程上的同步调用；`entry()` 返回后，`main()` 立即返回并结束进程。

并发相关状态只存在于下游测试桩，且使用 `SeqCst` 原子操作。`main.rs` 不提供隔离或重置这些全局状态的生命周期钩子。测试通过 `stubs::reset_for_test()` 清理调用次数、纳秒值和可用性；若未来并行运行会共享桩状态的测试，应继续在独立测试文件中设计隔离，不能把测试清理逻辑塞入生产入口。

## 与 Go 版本的对应关系

直接 Go 对照是 `tools/patch-go/check.go`。Go 文件属于 `package main`，其 `main()` 直接调用通过 `//go:linkname` 绑定到 `runtime.grunningnanos` 的 `grunningnanos()`，并丢弃返回值。Rust 为了同时支持二进制和库测试，将同一条语义拆成两层：本文件的 `main()` 调 `lib.rs::entry()`，再由 `check.rs::main()` 完成与 Go `main()` 对应的调用。

两版一致之处是：入口无参数业务、仅触发一次探针调用、不使用 `int64`/`i64` 结果，也不在入口恢复失败。差异是：Go 的真实性来自链接实际运行时私有符号，而当前 Rust 的 `check.rs::grunningnanos()` 调用可控的 `stubs.rs::runtime_grunningnanos()`。因此 Rust 路径验证了调用契约和失败传播，不能被描述成已验证真实 Go runtime 补丁或替代 `patch-go.sh` 的 Go 编译检查。

## 扩展指南

- 若只是改变探针算法、返回值处理或符号接线，应修改 `check.rs`/`stubs.rs` 及独立的 `parity_test.rs`，保持 `main.rs::main()` 为单层转发，避免二进制与库调用路径分叉。
- 若要增加真正的命令行参数、退出码或用户输出，入口职责才可能需要扩展；应先同步评估 `lib.rs::entry()` 的签名和 Go `check.go`/`patch-go.sh` 的调用契约，避免让脚本误判“已打补丁”。
- 新行为的测试应继续放在独立文件（当前为 `parity_test.rs`），不要在 `main.rs` 内嵌 `#[cfg(test)]` 测试模块。涉及二进制参数或退出状态时，可新增独立集成测试来启动 `astersql-tools-patch-go`；现有测试只直接覆盖库侧 `run_main()` 与 `grunningnanos()`，未直接启动本二进制。
- 兼容性风险主要是破坏 Go 探针“一次调用、忽略结果、失败外露”的语义；性能风险目前可忽略，因为入口仅增加一次普通函数转发。若新增 I/O、重试或后台任务，则必须重新说明错误传播和资源退出规则。

## 验证依据

- `tools/patch-go/main.rs`：确认唯一符号 `fn main()` 及其唯一调用 `astersql_tools_patch_go::entry()`。
- `tools/patch-go/Cargo.toml`：确认包名、`lib.rs` 库入口、`main.rs` 二进制入口、`kind = "binary"` 和空依赖表；根 `Cargo.toml` 确认 workspace 成员关系。
- `tools/patch-go/lib.rs`、`check.rs`、`stubs.rs`：逐层确认 `entry → check::main → run_main → grunningnanos → runtime_grunningnanos`，以及 panic、原子状态和清理语义。
- `tools/patch-go/check.go`、`patch-go.sh`：确认 Go `main()` 直接触发 linkname 目标，以及脚本用 Go 编译成功与否判定补丁状态。
- `tools/patch-go/parity_test.rs`：确认一次调用、`i64` 边界值、符号不可用失败和测试状态清理；同时确认没有直接启动 `main.rs` 二进制的测试。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件；`files --filter tools/patch-go` 列出目标及五个直接相关 Go/Rust 文件。对入口执行的精确源码/调用边查询没有产生可用边，因此未将其作为超出源码可见关系的证据。
- 结构验收以任务规定命令检查文档存在且恰含十一个固定二级章节；本任务为纯文档分析，按计划不运行 Cargo 或代码测试。
