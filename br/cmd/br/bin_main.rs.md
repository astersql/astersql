# `br/cmd/br/bin_main.rs`

## 文件定位

[`bin_main.rs`](bin_main.rs) 是 Cargo 包 `astersql-br-cmd-br` 的二进制壳层入口。它由 [`Cargo.toml`](Cargo.toml) 的 `[[bin]]` 段以 `name = "astersql-br-cmd-br"`、`path = "bin_main.rs"` 显式注册；同一个包还以 [`lib.rs`](lib.rs) 作为库 crate 根。因此该文件位于“操作系统启动二进制”与“可由测试和其他 Rust 代码复用的 BR 库入口”之间，不是备份、恢复或命令行解析逻辑的实现位置。

文件当前只有 crate 级说明和一个私有函数 `fn main()`，没有模块声明、常量、类型、trait、`impl`、条件编译项或公开 API。进程启动后形成的入口链是 `bin_main.rs::main` → `astersql_br_cmd_br::main` → `lib.rs` 中的 `entry::main` → [`main.rs`](main.rs) 中的 `pub fn main()`。

## 核心职责

本文件只承担一次无参数、无返回值的跨 crate 转发：

- 为 Cargo 生成的 BR 可执行文件提供 Rust 约定的 `fn main()` 入口。
- 调用库 crate `astersql_br_cmd_br` 导出的 `main()`，让二进制执行和库内测试共用同一套入口实现。
- 把命令树装配、参数读取、默认上下文、信号取消、退出码等行为留在 [`lib.rs`](lib.rs) 和 [`main.rs`](main.rs)，避免二进制壳层复制业务逻辑。

它不解析参数、不创建运行时、不启动线程、不处理错误，也不自行选择子命令。上述行为若发生，均属于下游共享入口，而非本文件本身。

## 主要符号

- `fn main()`（[`bin_main.rs`](bin_main.rs) 第 8 行）：文件内唯一符号，也是仅由 Rust 进程启动约定调用的私有二进制入口。签名没有参数和显式返回值，函数体仅调用 `astersql_br_cmd_br::main()`。
- `astersql_br_cmd_br::main()`（定义于 [`lib.rs`](lib.rs) 的 `pub fn main()`）：本文件唯一直接依赖的函数。Cargo 会把包名 `astersql-br-cmd-br` 转换为 Rust crate 标识符 `astersql_br_cmd_br`。该函数继续调用 `entry::main()`；`entry` 是 `lib.rs` 通过 `#[path = "main.rs"] pub mod entry` 暴露的模块。

`bin_main.rs::main` 不属于库的公开 API；可复用边界是 `lib.rs` 导出的 `pub fn main()`。如果调用方已经链接库 crate，应调用后者，而不是试图引用二进制入口。

## 执行流程

1. Cargo 根据 [`Cargo.toml`](Cargo.toml) 的 `[[bin]]` 配置编译并启动 `bin_main.rs`。
2. Rust 运行时进入本文件的 `fn main()`。
3. `main()` 同步调用 `astersql_br_cmd_br::main()`，没有参数转换、分支或中间状态。
4. [`lib.rs`](lib.rs) 的公开 `main()` 同步转发到 `entry::main()`。
5. [`main.rs`](main.rs) 的入口创建背景上下文和退出信号监听器，设置取消守卫，构建 `br` 根命令，定义公共 flag，关闭进程内 DDL 行为，注册 `debug`、`backup`、`restore`、`stream`、`operator`、`abort` 子命令，将标准输出和 `argv[1..]` 注入命令对象，然后执行命令。
6. 下游入口正常返回时，控制流依次返回库入口和本文件；下游命令执行失败时，由 `main.rs` 记录错误并请求退出码 `1`。

本文件不会捕获下游返回或改变控制流，因此其行为等价于“以共享库入口作为整个进程主体”。

## 数据与状态

本文件没有自有数据结构、全局变量、静态状态、堆分配或参数缓存，也不直接读写环境变量、文件、网络或标准流。唯一可观察动作是调用共享入口。

进程级状态由下游 [`main.rs`](main.rs) 管理：默认上下文通过 `SetDefaultContext` 保存，根命令接收 `os::Args()` 去掉 `argv[0]` 后的参数，输出绑定到 `os::Stdout()`，全局配置中的 `TiDBEnableDDL` 被置为 `false`。这些状态是解释完整启动链所需的直接下游事实，但不由 `bin_main.rs` 持有。

## 依赖与调用关系

上游没有普通 Rust 函数调用者：`bin_main.rs::main` 是可执行文件的特殊入口，由运行时进入。RustCodeGraph 对该文件报告 “used by 0 files”，这与二进制入口不需要源码调用者的性质一致；不能据此判断该文件未接线，接线证据是 [`Cargo.toml`](Cargo.toml) 的 `[[bin]]` 声明。

唯一直接下游是同包库 crate 的 `astersql_br_cmd_br::main()`。随后调用关系如下：

```text
Cargo [[bin]]
  -> bin_main.rs::main
  -> astersql_br_cmd_br::main       (lib.rs)
  -> entry::main                    (main.rs)
  -> BR 根命令装配与 Command::Execute
  -> 选中的一级子命令
```

本文件不直接依赖 [`Cargo.toml`](Cargo.toml) 中列出的任务、跟踪、序列化或哈希 crate；这些依赖属于同一 package 的库实现。Cargo 文件没有为该二进制定义单独 feature，因此壳层没有条件化入口分支。

## 错误处理与边界

本文件没有 `Result` 返回值、`?`、匹配分支、日志或恢复策略，因而不会在转发边界解释、包装或吞掉错误。若共享入口 panic，panic 会穿过该帧并按进程 panic 策略处理；若共享入口正常返回，本文件也立即返回。

实际命令错误边界位于 [`main.rs`](main.rs)：`rootCmd.Execute()` 返回 `Err` 时记录 `br failed`，遗忘取消守卫以对齐 Go `os.Exit` 不执行 `defer` 的语义，并调用 `os::Exit(1)`。这意味着扩展本文件时不应在外层另加一套错误日志或退出码转换，否则可能导致重复日志或改变 Go/Rust 对齐的退出行为。

## 并发与资源生命周期

本文件本身不创建线程、任务、通道、锁、事务或资源守卫；对共享入口的调用是同步的，生命周期覆盖整个 BR 主流程。

直接下游 [`main.rs`](main.rs) 通过 `utils::StartExitSingleListener` 获得可取消上下文，并用 `CancelOnDrop` 模拟 Go 的 `defer cancel()`：正常返回时守卫执行取消回调；错误退出路径在 `os::Exit(1)` 前显式 `forget` 守卫，以保留 Go `os.Exit` 跳过 `defer` 的行为。独立测试 [`main_test.rs`](main_test.rs) 的 `test_run_main` 在线程中调用 `crate::main()` 并通过同步通道等待返回，验证共享入口在测试参数清洗后的可返回性；它测试的是本文件所转发到的同一库入口，而不是直接引用私有二进制 `main`。

## 与 Go 版本的对应关系

Go 版本 [`main.go`](main.go) 在 `package main` 中直接实现完整 `func main()`：建立信号上下文、配置根 Cobra 命令、注册六类子命令、设置输出和参数，并在执行失败时退出。Rust 为了让入口逻辑可被库测试复用，把这份逻辑放到 [`main.rs`](main.rs)，再由 [`lib.rs`](lib.rs) 导出；本文件是 Rust 构建布局所需的额外薄壳，在 Go 同路径中没有一一对应的第二个文件。

语义对应关系是：Go `main.go::main` 对应 Rust `main.rs::main`，而 `bin_main.rs::main` 与 `lib.rs::main` 共同完成从可执行文件到该实现的接线。入口契约由 Go 的 [`main_test.go`](main_test.go) 与 Rust 的独立 [`main_test.rs`](main_test.rs) 对照覆盖，包括过滤测试框架参数、入口可返回性，以及测试结束前清理全局内存仲裁器。当前 Rust 测试没有直接编译或调用 `bin_main.rs` 的私有函数；Cargo 接线由 manifest 和源码结构验证。

## 扩展指南

- 新增或调整 BR 子命令时，应修改 [`main.rs`](main.rs) 的命令树及对应的独立测试文件，而不是在 `bin_main.rs` 中装配命令。
- 需要让嵌入式调用者复用新启动行为时，应保持 `lib.rs::main` 为统一公开边界，并把可测试逻辑放在库模块中；本文件继续维持单一转发最安全。
- 若确实需要二进制专属的进程级初始化，应先判断它是否必须区别于库入口。任何参数改写、日志初始化、运行时建立或退出码处理都可能让二进制路径与 `crate::main()` 测试路径分叉，必须同步增加独立的 Rust 集成/入口测试，并核对 [`main.go`](main.go) 的对应语义。
- 不要把测试模块内嵌进本文件。入口行为测试应继续放在 [`main_test.rs`](main_test.rs) 或新增的独立测试文件中；若要验证真实二进制启动，可增加独立集成测试，通过 Cargo 暴露的二进制名启动进程。
- 保持 [`Cargo.toml`](Cargo.toml) 的二进制名、路径和库 crate 名一致接线；重命名 package、`[[bin]].path` 或公开库入口时，应同时更新这里的限定调用以及构建元数据。

兼容性风险主要来自命令行参数、输出、退出码和取消时机的变化；性能上该壳层只有一次普通函数调用，不应引入可感知开销。把初始化复制到壳层会提高二进制与库模式漂移的正确性风险。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`files --filter br/cmd/br` 列出了 `bin_main.rs`、`lib.rs`、`main.rs`、`main_test.rs` 等目标及直接证据文件。
- RustCodeGraph 文件节点：`node --file br/cmd/br/bin_main.rs` 显示全文件仅第 8 行的 `fn main()` 及第 9 行的 `astersql_br_cmd_br::main()` 调用，并报告该文件无源码使用者。
- RustCodeGraph 符号查询：`query main --kind function` 定位到 `br/cmd/br/bin_main.rs::main`、`lib.rs::main`、`br/cmd/br/main.rs::main` 和 Go `main.go::main`。对通用名执行 `callees` 时发生跨仓库同名歧义，且索引未生成本次跨 crate 转发边；因此调用边最终以目标函数体、[`lib.rs`](lib.rs) 的公开转发和 Cargo manifest 三方交叉确认，而未把缺失图边解释为“无调用”。
- Cargo 边界：[`Cargo.toml`](Cargo.toml) 同时声明 `lib.path = "lib.rs"` 与 `[[bin]] name = "astersql-br-cmd-br", path = "bin_main.rs"`，并将 porting kind 标记为 `binary`。
- 入口实现：[`lib.rs`](lib.rs) 第 43—45、83—88 行将 `main.rs` 命名为 `entry` 并由公开 `main()` 调用；[`main.rs`](main.rs) 第 23—74 行给出完整 BR 启动顺序、错误退出与取消守卫行为。
- Go 对照：[`main.go`](main.go) 第 14—46 行是 Rust 共享入口的直接语义来源。
- 测试证据：[`main_test.rs`](main_test.rs) 第 85—117 行通过线程与同步通道调用 `crate::main()` 并等待返回；[`main_test.go`](main_test.go) 第 70—92 行提供相应 Go 测试意图。两侧还验证测试参数过滤和全局内存仲裁器清理，但未直接测试私有的 `bin_main.rs::main`。
- 本任务是纯文档分析，按计划不运行 Cargo；验收使用固定章节结构检查，并人工复核文档只陈述上述源码、图查询、Cargo、Go 和独立测试能够支持的事实。
