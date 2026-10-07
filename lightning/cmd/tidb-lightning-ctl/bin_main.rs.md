# `lightning/cmd/tidb-lightning-ctl/bin_main.rs`

## 文件定位

本文件是 Cargo 包 `astersql-lightning-cmd-tidb-lightning-ctl` 的可执行目标入口。`lightning/cmd/tidb-lightning-ctl/Cargo.toml` 的 `[[bin]]` 将同名二进制明确映射到 `bin_main.rs`，而 `[lib]` 将库目标映射到 `lib.rs`；因此编译后的进程先进入本文件的私有 `fn main()`，再跨目标调用同一包的库 crate。crate 名中的连字符在 Rust 路径里转换为下划线，所以源码使用 `astersql_lightning_cmd_tidb_lightning_ctl`。

它是一个只有八行源码的可执行壳，不定义 CLI 参数、业务状态或网络协议。真实装配链是 `bin_main.rs::main` → `lib.rs::main` → `main.rs` 中公开的 `entry::main`。这层拆分让测试可以直接链接库目标并调用可测试入口，而不必从测试进程启动二进制文件。

## 核心职责

本文件只承担两个职责：为 Cargo 二进制提供语言规定的进程入口 `fn main()`；把控制权无条件、同步地交给库 crate 的 `main()`。它不读取 `argv`、不解释返回值、不格式化错误，也不自行调用 `process::exit`。

参数收集从下游 `lightning/cmd/tidb-lightning-ctl/main.rs::main` 才开始；FIPS 钩子、退出码映射、配置加载、动作分派和客户端关闭也都在下游完成。扩展或修复这些行为时，不应把逻辑堆入本文件，否则会绕开库模式和现有独立测试的公共入口。

## 主要符号

- `fn main()`（`bin_main.rs:6`）：私有、无参数、无显式返回值的 Rust 进程入口。函数体只有一次对 `astersql_lightning_cmd_tidb_lightning_ctl::main()` 的调用，没有局部变量、条件分支、常量、类型、trait、`impl` 或条件编译项。
- `astersql_lightning_cmd_tidb_lightning_ctl::main()`（调用点 `bin_main.rs:7`，定义于 `lib.rs:39`）：库 crate 的公开入口。它继续调用 `entry::main()`；`entry` 是 `lib.rs:26-27` 通过 `#[path = "main.rs"] pub mod entry` 声明的模块。

本文件没有公开 API。对其他 Rust crate 可见的是库目标中的 `pub fn main()`，不是这里的二进制入口。

## 执行流程

1. 操作系统启动由 Cargo `[[bin]]` 产出的可执行文件，Rust 运行时调用 `bin_main.rs::main`。
2. `bin_main.rs::main` 立即同步调用库 crate 的 `main()`，没有前置初始化或分支。
3. `lib.rs::main` 再转发给 `entry::main()`。
4. `main.rs::main` 读取 `std::env::args()`，丢弃程序名后调用 `main_with_args`；后者依次执行 FIPS 初始化、`run_main`，并只在非零结果时调用可替换的退出钩子。
5. `run_main` 将帮助/成功、装载错误和运行期错误分别映射为 `0`、`2`、`1`；`run_loaded` 才构造配置、TLS 和 PD 客户端并分派 compact、fetch mode、switch mode 或 checkpoint 动作。

第 4、5 步描述的是本文件转交后的直接下游，用于说明完整应用位置；这些逻辑不属于 `bin_main.rs` 自身。

## 数据与状态

本文件不拥有任何数据结构或可变状态。它既不缓存参数，也不持有配置、连接、锁或全局变量；调用过程中唯一可观察的控制数据来自下游读取的进程参数和下游可能触发的退出行为。

进程级可变测试钩子及 PD 客户端状态位于 `stubs.rs`，CLI 运行状态位于 `main.rs` 的调用栈内。由于本入口不截获结果，库入口正常返回时它自然返回，库入口 panic 时 panic 继续展开，库入口触发退出钩子时相应进程边界语义也原样生效。

## 依赖与调用关系

上游是 Cargo/操作系统启动协议，而不是仓库内普通函数调用者。RustCodeGraph 将 `bin_main.rs::main` 识别为函数且 `callers` 返回空集合，符合二进制入口由运行时隐式调用的事实。`Cargo.toml:16-18` 是该隐式入口关系的权威配置证据；根 `Cargo.toml` 还把 `lightning/cmd/tidb-lightning-ctl` 列为 workspace 成员。

唯一直接下游是 `lib.rs::main`。RustCodeGraph 的 `callees` 没有解析出这条跨 crate 目标边，但 `bin_main.rs:7` 的完整限定调用与 `lib.rs:37-40` 的公开定义共同确认了它。再下一层是 `lib.rs::main` → `entry::main`，然后才进入 `main.rs` 的 CLI 主链。Cargo 包直接声明 `astersql-lightning-pkg-importer` 和 `astersql-lightning-pkg-server` 两个路径依赖，但本文件没有直接引用它们；其业务用途出现在下游命令实现和兼容桩中。

## 错误处理与边界

本文件没有 `Result`、`?`、错误转换、日志或恢复逻辑，也没有为库入口设置返回码。所有错误边界都由下游定义：`main.rs::run_main` 对帮助返回 `0`，对参数/配置装载错误返回 `2`，对运行期错误格式化到标准错误并返回 `1`；`main_with_args` 只对非零码调用 `call_exit`。

这里的重要边界是不应在壳层吞掉 panic、改写退出码或重复打印错误。任何此类改动都会改变进程可观察行为，并使直接调用库入口的测试与实际二进制产生差异。当前实现通过不做包装，确保二进制和库入口共享同一条错误路径。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或 I/O 资源，调用是单线程同步转发。因此它自身没有并发不变量和清理分支。

下游 `run_loaded` 创建 PD 客户端，调用 `dispatch` 后显式执行 `cli.Close()`；`main_test.rs::test_run_main` 和 `parity_test.rs::contract_resource_cleanup` 分别验证成功、运行期失败和动作提前返回时的关闭行为。测试中的子线程/子进程只用于隔离进程级退出钩子，并不意味着 `bin_main.rs` 启动了后台工作。若未来入口必须建立资源，应优先放在可测试的库层并确保所有退出路径清理，而不是在此文件增加无法由现有库测试直接观察的生命周期。

## 与 Go 版本的对应关系

Go 对照入口是 `lightning/cmd/tidb-lightning-ctl/main.go::main`。Go 的 `main()` 直接调用 `run()`，在错误时格式化到标准错误并执行可替换的 `exit(1)`；Rust 将同等业务入口拆成两层：本文件只满足可执行入口要求，`lib.rs::main` 再进入 `main.rs::main`。因此，本文件与 Go `main()` 在“进程启动后进入统一命令主链”这一职责上对应，但 Go 入口中的错误处理已下沉到 Rust 库实现，而非遗漏。

Rust 下游还显式保留了更细的装载错误退出码 `2` 和帮助退出码 `0`，相关契约由 `main_test.rs`、`parity_test.rs` 覆盖。`Cargo.toml` 的 `[package.metadata.porting]` 将 `go-package` 标为同一路径、`kind` 标为 `binary`，进一步说明这是 Go `package main` 的 Rust 二进制移植装配层。

## 扩展指南

- 若只增加或修改命令行参数、动作优先级、错误文案或退出码，应修改 `main.rs` 中的 `main_with_args`、`run_main`、`run_loaded` 或 `dispatch`，并同步更新独立的 `main_test.rs`/`parity_test.rs`；不要修改本文件。
- 若需要调整 crate 模块装配或提供新的库级入口，应先修改 `lib.rs`，保持 `bin_main.rs` 仍为一次转发，使二进制行为和库测试入口一致。
- 只有进程启动前且无法位于库层的最低级初始化才可能需要改动 `bin_main.rs`。这种改动必须补充独立测试或进程级集成验证，重点检查初始化顺序、panic/退出码传播和重复初始化风险。
- 测试逻辑应继续放在独立的 `main_test.rs` 或 `parity_test.rs`，不要在生产源文件内嵌 `#[cfg(test)]` 测试模块。
- 若改动二进制名或文件位置，必须同步 `Cargo.toml` 的 `[[bin]]`；若改动包名，还要同步调用路径中由连字符转换而来的 crate 标识符。此类改动存在脚本兼容性和打包入口风险。

性能方面，此壳层只有固定的一次函数调用，通常会被编译器内联；功能扩展不应在此引入重复配置解析、额外网络连接或后台任务。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust 文件；`files --filter lightning/cmd/tidb-lightning-ctl` 确认目标、库入口、实现和测试文件；`node --file .../bin_main.rs` 确认源码全貌；`node lightning/cmd/tidb-lightning-ctl/bin_main.rs::main` 确认唯一函数；`callers` 返回 `[]`，`callees` 返回 `[]`，后者的跨 crate 缺口已用源码与 Cargo 配置补证。
- 生产源码：`lightning/cmd/tidb-lightning-ctl/bin_main.rs:1-8`、`lib.rs:17-40`、`main.rs:25-125` 与 `main.rs:127-237`。
- crate 配置：`lightning/cmd/tidb-lightning-ctl/Cargo.toml` 的 `[lib]`、`[[bin]]`、移植元数据和依赖声明；根 `Cargo.toml` 的 workspace 成员条目。
- Go 对照：`lightning/cmd/tidb-lightning-ctl/main.go` 的 `main`、`run`、`formatFatalError`、`compactCluster` 和 `fetchMode`。
- 独立测试：`lightning/cmd/tidb-lightning-ctl/main_test.rs::test_run_main` 验证入口退出码、错误格式和客户端关闭；`parity_test.rs::go_rust_public_contract_matches` 及 `contract_resource_cleanup` 验证 Go/Rust 公共契约和资源回收；Go 基线位于 `main_test.go::TestRunMain`。没有发现直接调用私有 `bin_main.rs::main` 的单元测试，这与二进制壳通过 Cargo/进程启动验证的边界一致。
