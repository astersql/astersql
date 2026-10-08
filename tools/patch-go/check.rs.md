# [`tools/patch-go/check.rs`](check.rs)

## 文件定位

`check.rs` 是 `astersql-tools-patch-go` crate 中的补丁版 Go 运行时探针核心。crate 由根工作区 `Cargo.toml` 纳入成员，包边界和二进制目标定义在 `tools/patch-go/Cargo.toml`：库入口为 `lib.rs`，二进制入口为 `main.rs`。实际调用链为 `main.rs::main` → `lib.rs::entry` → `check::main` → `run_main` → `grunningnanos` → `stubs::runtime_grunningnanos`。

它对应同目录 Go 程序 `tools/patch-go/check.go`。Go 程序通过 `//go:linkname` 引用 `runtime.grunningnanos`，而 `tools/patch-go/patch-go.sh` 以 `go build check.go` 是否成功判断当前 Go 工具链是否已经应用补丁。Rust 文件不参与真正 Go 工具链的链接检查；它在 Rust crate 内用可控 stub 保存同一条“入口必须触发目标符号”的行为契约。

## 核心职责

本文件只承担三层薄转发：`grunningnanos` 暴露与 Go `int64` 对齐的 `i64` 探针接口；`run_main` 调用探针并丢弃返回值；`main` 提供与 Go `main()` 同形的模块级入口。文件没有实现 goroutine 计时算法、没有解释返回值，也没有把失败转换成成功。

真正的 Go 运行时算法存在于 `go1.20.1.patch`、`go1.20.2.patch` 和 `go1.20.5.patch`：补丁为 goroutine 维护 `lastsched` 与 `runningnanos`，并由运行时 `grunningnanos()` 计算累计运行时间加当前运行区间。Rust 侧 `check.rs` 仅验证调用路径，不是该算法的移植实现。

## 主要符号

- `pub unsafe fn grunningnanos() -> i64`（`check.rs`）：公开的底层探针边界，直接返回 `stubs::runtime_grunningnanos()`。`unsafe` 表达其语义上对应 Go 的非公开运行时链接符号；当前 Rust 实现自身没有裸指针、FFI 声明或其他局部不安全操作。
- `pub fn run_main()`（`check.rs`）：安全包装层，在一个显式 `unsafe` 块中调用 `grunningnanos`，用 `let _ = ...` 丢弃结果。该函数是 `parity_test.rs` 直接验证的主流程入口。
- `pub fn main()`（`check.rs`）：调用 `run_main`，供 `lib.rs::entry` 转发。它是普通公开函数，不是 Rust 二进制由语言约定直接识别的顶层入口；真正的二进制 `fn main()` 位于 `tools/patch-go/main.rs`。

本文件没有模块级常量、类型、trait、`impl` 或条件编译项。

## 执行流程

1. 启动 `astersql-tools-patch-go` 二进制时，`tools/patch-go/main.rs::main` 调用库函数 `astersql_tools_patch_go::entry`。
2. `tools/patch-go/lib.rs::entry` 调用本模块的 `check::main()`。
3. `check::main` 无条件调用 `run_main`，没有参数解析或分支。
4. `run_main` 在 `unsafe` 块中调用一次 `grunningnanos`，随后丢弃返回的 `i64`；因此成功路径没有输出，也不根据数值判断成败。
5. `grunningnanos` 调用 `stubs::runtime_grunningnanos`。stub 先递增调用计数；若符号被配置为不可用则 panic，否则读取并返回原子保存的纳秒值。

Go 生产脚本的链路不同：`patch-go.sh` 直接尝试编译 `check.go`；已打补丁工具链能解析 `runtime.grunningnanos`，未打补丁时编译/链接失败。不能把 Rust stub 成功运行等同于当前系统 Go 工具链已经打补丁。

## 数据与状态

`check.rs` 本身不保存状态。唯一经过该文件的数据是 `grunningnanos` 返回的 `i64`，与 Go 声明 `func grunningnanos() int64` 保持位宽和有符号语义一致；`run_main` 有意忽略该值。

可观察状态位于 `tools/patch-go/stubs.rs` 的三个进程内原子量：`NANOS: AtomicI64` 保存模拟返回值，`CALLS: AtomicU64` 保存进入次数，`AVAILABLE: AtomicBool` 表示目标符号是否可用。它们全部使用 `Ordering::SeqCst`。`parity_test.rs` 覆盖 `0`、`-1`、`i64::MIN`、`i64::MAX`，证明转发层不截断、不改号；这些极值是接口形状测试，不代表真实 Go 运行时间会自然取得所有这些值。

## 依赖与调用关系

上游调用者有两类：生产二进制通过 `main.rs::main` 和 `lib.rs::entry` 最终调用 `check::main`；独立测试 `tools/patch-go/parity_test.rs` 直接调用公开的 `run_main` 与 `grunningnanos`。`lib.rs` 通过 `#[path = "check.rs"] pub mod check` 装配本文件，并只在 `cfg(test)` 下装配 `parity_test.rs`。

下游只有 crate 内部模块 `crate::stubs`，具体边为 `grunningnanos` → `stubs::runtime_grunningnanos`。`tools/patch-go/Cargo.toml` 没有声明外部依赖，因而此调用链只依赖 Rust 标准库（原子状态实际由 `stubs.rs` 使用）。RustCodeGraph 能识别 `check.rs` 中的 `grunningnanos`、`run_main`、`main` 和导入节点，也能识别 `stubs.rs::runtime_grunningnanos`；本次索引的 callers/callees 查询没有产出边，因此上述边由模块入口和源码引用搜索交叉核对。

## 错误处理与边界

本文件没有 `Result`、错误枚举或恢复分支。正常路径无论纳秒值为何都视为调用成功并丢弃结果。当前 stub 在 `AVAILABLE == false` 时以消息 `runtime.grunningnanos: symbol not linked (go unpatched)` 触发 panic；`check.rs` 不捕获 panic，因此失败沿调用栈传播并终止未捕获的二进制调用。

这一 Rust 失败模式只近似 Go 的外部可观察契约：Go `check.go` 依赖链接名，未打补丁时通常在 `go build` 的编译/链接阶段失败；Rust crate 使用普通本地函数，因此不会真实验证 Go 符号表。`grunningnanos` 的安全前提也只写在接口文档中，当前实现没有要求调用者提供额外内存或生命周期保证。

## 并发与资源生命周期

`check.rs` 不创建线程、任务、锁、通道、文件、网络连接或堆资源；一次 `run_main` 是同步的一次函数调用。共享状态全部在 `stubs.rs` 中以顺序一致原子操作管理，所以单次读写不存在数据竞争，但多个线程或并行测试仍共享同一组全局值，复合序列（例如“reset 后恰好调用一次”）并没有事务隔离。

`parity_test.rs` 当前把四个场景串行放在单个 `go_rust_public_contract_matches` 测试函数内，并在场景开始调用 `reset_for_test`，避免同一测试内部的状态泄漏。若以后拆成多个可并行测试，应增加测试级串行化或把 stub 状态改为可注入实例；仅依赖原子变量不能保证跨场景断言稳定。

## 与 Go 版本的对应关系

Go `check.go` 的 `grunningnanos` 只有声明，没有函数体，并借助空白导入 `unsafe` 启用 `//go:linkname grunningnanos runtime.grunningnanos`；Go `main()` 仅调用一次该函数。Rust 对应关系是：Go 声明映射为 `unsafe fn grunningnanos() -> i64`，Go `main` 的单次调用映射为 `run_main`，模块 `main` 再保持入口名称对应。

两者关键差异是链接真实性。Go 文件会直接证明所用 Go 工具链是否提供运行时符号，且 `patch-go.sh` 依赖这个构建结果；Rust 文件总是链接 crate 内 `stubs.rs`，只能通过 `set_available(false)` 模拟不可用。Go 侧也没有 Rust 的 `run_main` 分层、调用计数或可注入返回值；这些是为了独立测试而增加的局部接线，并未改变“调用一次、忽略结果”的原测试意图。

`build.sh` 当前下载 Go 1.20.5 源码并应用补丁；目录同时保留 1.20.1、1.20.2、1.20.5 三份补丁。所有补丁都加入同名运行时函数，但版本升级时仍必须同步确认 patch 内容、`build.sh` 版本变量和 `check.go` 的 linkname，而不能只修改 Rust stub。

## 扩展指南

- 若改变探针调用次数、返回值处理或失败传播，优先修改 `run_main` 或 `grunningnanos`，并同步更新独立测试 `tools/patch-go/parity_test.rs`；不要把测试模块嵌入 `check.rs`。
- 若需要真实验证新 Go 运行时符号，应更新 `check.go`、相应 `go<version>.patch` 与 `patch-go.sh`/`build.sh`，不能以扩展 `stubs.rs` 代替真实链接检查。
- 若仅增加 Rust 可测试性，保持 `main.rs → entry → check::main` 的薄转发和无输出成功语义，避免 Rust 路径比 Go 探针增加参数解析、阈值判断或静默降级。
- 若并行化测试或引入多探针，先解决 `stubs.rs` 全局原子状态的场景隔离；性能上当前路径只有固定次数的顺序一致原子操作，但它不是高频生产计时实现。
- 兼容性风险主要来自 `i64`/Go `int64` 形状、linkname 名称和失败阶段的差异；任何修改都应同时核对 Go 声明、运行时 patch 与 Rust parity 测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `tools/patch-go` 的 6 个 Go/Rust 文件；`files --filter tools/patch-go` 列出 `check.go`、`check.rs`、`lib.rs`、`main.rs`、`parity_test.rs`、`stubs.rs`；`query` 定位 `check.rs::grunningnanos`（第 34 行）、`check.rs::run_main`（第 41 行）和 `stubs.rs::runtime_grunningnanos`（第 49 行）。精确 callers/callees 查询未返回调用边，故没有据此虚构图关系。
- 核心源码：`tools/patch-go/check.rs`（三个公开函数及唯一直接下游）、`tools/patch-go/lib.rs`（模块装配与 `entry`）、`tools/patch-go/main.rs`（二进制入口）、`tools/patch-go/stubs.rs`（原子状态、panic 与调用计数）。
- Go 与构建证据：`tools/patch-go/check.go`（linkname 和单次调用）、`tools/patch-go/patch-go.sh`（以 `go build check.go` 判断补丁状态）、`tools/patch-go/build.sh`（Go 1.20.5 构建流程）、`tools/patch-go/go1.20.1.patch`、`go1.20.2.patch`、`go1.20.5.patch`（真实运行时状态与计算公式）。
- 测试证据：`tools/patch-go/parity_test.rs` 独立覆盖正常调用次数、返回值透传、`i64` 边界、不可用时失败和 reset 后状态清理；同目录不存在内嵌于 `check.rs` 的测试。
- crate 证据：`tools/patch-go/Cargo.toml` 声明库/二进制入口、Go 包元数据且无外部依赖；根 `Cargo.toml` 将 `tools/patch-go` 纳入 workspace。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务文件指定命令验证本文档存在且恰有 11 个固定二级章节，并人工复核唯一生产物和调用链描述。
