# `br/cmd/br/main.rs`

## 文件定位

[`main.rs`](main.rs) 是 `astersql-br-cmd-br` crate 的 BR 命令行共享入口实现。Cargo 将 [`lib.rs`](lib.rs) 作为库入口、将 [`bin_main.rs`](bin_main.rs) 声明为 `astersql-br-cmd-br` 二进制；实际启动链是 `bin_main.rs::main` → `astersql_br_cmd_br::main`（`lib.rs`）→ `entry::main`（本文件）。因此本文件位于进程壳层与各 BR 子命令实现之间，负责一次性装配和启动根命令，不实现备份、恢复或日志流业务。

[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting` 将该 crate 标记为从 Go 包 `br/cmd/br` 移植而来的 `binary`，且依赖说明明确采用 slim BR crates 与本地 trait/stub，避免在 arm64 Darwin 上引入完整的 KV、domain、kvproto、grpcio 栈。当前入口由 [`lib.rs`](lib.rs) 以 `#[path = "main.rs"] pub mod entry` 纳入库 crate；本文件没有条件编译项、模块级常量或公开 trait。

## 核心职责

`main()` 只承担进程级编排，职责可归纳为五项：

1. 从 `Context::Background()` 派生可取消上下文，并把取消回调交给 `CancelOnDrop` 管理。
2. 创建名为 `br` 的根 `Command`，设置简介、子命令遍历和静默 usage，并通过 `DefineCommonFlags` 统一注册版本、日志、状态服务及任务层公共 flags。
3. 用 `SetDefaultContext` 发布进程默认上下文，同时把全局 `TiDBEnableDDL` 原子开关置为 `false`，避免 BR 离线工具启用 TiDB DDL 行为。
4. 按固定顺序注册 `debug`、`backup`、`restore`、`stream`、`operator`、`abort` 六个一级命令，绑定输出并把 `argv[1..]` 交给命令解析器。
5. 执行命令树；失败时记录 `br failed`、绕过正常返回时的取消回调，并请求以状态码 1 退出。

边界同样重要：参数解析、持久 flag 继承及叶子回调调度由 [`stubs.rs`](stubs.rs) 的 `Command::Execute`/`execute_command` 承担；公共初始化由 [`cmd.rs`](cmd.rs) 承担；六类具体任务分别在同目录对应模块中构造。本文件不直接连接集群、不启动备份恢复任务，也不计算 `main_test.rs` 覆盖的内存上限。

## 主要符号

- `pub fn main()`：本文件唯一公开 API。签名不返回 `Result` 或退出码，成功路径自然返回；失败路径通过 `os::Exit(1)` 表达进程失败。上层 [`lib.rs`](lib.rs) 的公开 `main()` 和 [`bin_main.rs`](bin_main.rs) 的二进制 `main()` 都只是薄转发。
- `struct CancelOnDrop(Option<Box<dyn Fn() + Send>>)`：私有 RAII 守卫，持有至多一个可在线程间移动的取消闭包。`Option` 使回调能在析构时被 `take()`，保证每个守卫最多调用一次。
- `impl Drop for CancelOnDrop::drop(&mut self)`：普通返回或展开时取出并执行取消回调；若内部值已为空则无操作。
- 直接导入的命令工厂：`NewDebugCommand`、`NewBackupCommand`、`NewRestoreCommand`、`NewStreamCommand`、`newOperatorCommand`、`NewAbortCommand`，均返回本地 `Command` 值。RustCodeGraph 分别定位到 `debug.rs:66`、`backup.rs:181`、`restore.rs:221`、`stream.rs:75`、`operator.rs:70`、`abort.rs:31`。
- 公共接线函数：`DefineCommonFlags(&mut Command)` 位于 `cmd.rs:165`，`SetDefaultContext(Context)` 位于 `cmd.rs:380`；前者修改根命令 flags，后者写入进程级互斥保护的默认上下文。

## 执行流程

1. `Context::Background()` 创建无父级的后台上下文。
2. `utils::StartExitSingleListener(gCtx)` 返回派生上下文和取消闭包。当前 Rust 直接证据表明该函数是 `Context::WithCancel` 的轻量封装，不会像 Go 实现那样自行安装真实 OS 信号监听器。
3. `CancelOnDrop(Some(cancel))` 取得取消闭包所有权。只要后续正常离开 `main()`，析构就会把派生上下文标记为已取消。
4. 构造根命令：`Use = "br"`，`Short` 为 BR 工具说明，`TraverseChildren = true`，`SilenceUsage = true`，其余字段取 `Command::default()`。
5. `DefineCommonFlags` 写入版本信息，定义 `version`、日志、脱敏、status address 及任务层公共持久 flags；`SetDefaultContext(ctx)` 保存同一个可取消上下文，供后续初始化和 tracing 获取。
6. `config::GetGlobalConfig().Instance().TiDBEnableDDL_Store(false)` 以顺序一致的原子写把 DDL 开关关闭。
7. `AddCommand` 按源码顺序构造并加入六个一级子命令；`SetOut(os::Stdout())` 表达输出目标。当前 `os_stub::Stdout()` 返回单元值，`Command::SetOut` 也是占位操作，实际 `Command` 仍写入自身缓冲。
8. `os::Args()` 读取真实进程参数；存在 `argv[0]` 时传递 `args[1..]`，空参数向量则保持为空，随后 `SetArgs` 保存用户参数。
9. `Command::Execute` 递归匹配子命令、继承持久 flags，并运行 `PersistentPreRunE`/`RunE`；根命令没有可执行回调且没有用户参数时成功返回，未知命令则返回 `unknown command <name>`。
10. 执行失败时，`log::Error` 记录错误；随后 `std::mem::forget(_cancel_on_return)` 故意不析构守卫，再调用 `os::Exit(1)`。正常返回则自动调用 `CancelOnDrop::drop`。

## 数据与状态

- `gCtx`、`ctx`：分别是背景上下文与派生上下文。`Context` 当前由 `Arc<AtomicBool>` 保存本级取消状态、由祖先标志列表传播父取消，并用 `Arc<Mutex<HashMap<...>>>` 保存 values；它不实现 Go `context.Context` 的完整截止时间树。
- `_cancel_on_return`：函数栈上的资源守卫，拥有取消闭包。闭包由 `Context::WithCancel` 创建，执行时以 `SeqCst` 把派生上下文的取消标志设为 `true`。
- `rootCmd`：函数内可变命令树根节点。它拥有 flags、六个子命令、参数、回调及输出缓冲；离开 `main()` 后整体释放。
- 默认上下文：`SetDefaultContext` 把 `ctx` 移入 `cmd.rs` 的全局 `Mutex<Option<Context>>`。取消闭包与该 `Context` 共享同一个 `Arc<AtomicBool>`，所以守卫执行取消时会被下游持有者观察到。
- 全局配置：`TiDBEnableDDL_Store(false)` 最终写入 `stubs.rs` 中 `OnceLock<Mutex<Config>>` 内的 `AtomicBool`。这是进程级副作用，而非根命令局部字段。
- 退出状态：当前 `os_stub::Exit` 不终止进程，只把退出码写入 `Mutex<Option<i32>>`，测试可用 `take_exit()` 读取并清空。因此“错误路径不再执行后续代码”只与 Go 的真实 `os.Exit` 设计意图一致，并不是当前 Rust stub 的完整运行时语义。

## 依赖与调用关系

上游入口关系由源码模块接线直接验证：

```text
bin_main.rs::main
  -> astersql_br_cmd_br::main (lib.rs)
    -> entry::main (main.rs)
```

本文件下游分为四类：

- 生命周期：`Context::Background` → `utils::StartExitSingleListener` → `CancelOnDrop::drop`。
- 公共 CLI：`DefineCommonFlags`、`SetDefaultContext`、`Command::{AddCommand, SetOut, SetArgs, Execute}`。
- 全局配置与进程适配：`config::GetGlobalConfig`、`os_stub::{Args, Stdout, Exit}`、`log::Error`/`zap::Error`。
- 命令工厂：`debug`、`backup`、`restore`、`stream`、`operator`、`abort` 六个同级模块。

RustCodeGraph 将本文件识别为 4 个符号并显示被 `main_test.rs` 使用；对精确入口符号执行 `callers`/`callees` 时没有生成调用边。因此调用链结论以 [`bin_main.rs`](bin_main.rs)、[`lib.rs`](lib.rs) 和本文件的显式函数调用为依据，不能把图中空结果解释为入口无人调用或没有下游依赖。

Cargo 的直接生产依赖为 `astersql-br-pkg-task`、`astersql-br-pkg-task-operator`、`astersql-br-pkg-trace`、`astersql-br-pkg-streamhelper-config` 以及 `hex`、`serde`、`serde_json`、`sha2`；这些依赖主要经同 crate 的命令模块和 `stubs.rs` 间接服务于入口。本文件本身没有 feature gate。`astersql-util-memory` 仅是 dev-dependency，供独立入口测试清理全局内存仲裁器。

## 错误处理与边界

`main()` 只显式处理 `rootCmd.Execute()` 的 `Err`：错误被包装进结构化日志字段后映射为退出码 1；`SilenceUsage = true` 表明业务错误不应附带重复 usage。命令树当前在找不到子命令且根没有 `RunE` 时产生 `unknown command <name>`；下游 `PersistentPreRunE` 或 `RunE` 返回的错误通过 `?` 原样向入口传播。

本文件没有恢复策略或错误分类，也不会返回错误给库调用者。构造命令、锁全局状态或读取参数期间的 panic 不在此处捕获；Rust 展开会析构 `CancelOnDrop`，而进程 abort 则不会。`SetDefaultContext` 和配置代理内部使用的互斥锁若中毒会 `unwrap()` panic。

需要特别区分设计语义与当前替身能力：Go 的 `os.Exit(1)` 立即终止且跳过 `defer cancel()`；Rust 代码先 `forget` 守卫以模拟跳过取消，但当前 `os_stub::Exit` 仅记录退出码并返回，所以 `main()` 随后仍会自然结束。该限制意味着嵌入或测试路径可以观察到返回，不应据此声称 Rust 已具备真实进程退出等价性。

## 并发与资源生命周期

入口本身不创建线程、异步任务、通道或事务。并发相关状态来自共享替身：`Context` 的取消标志使用 `AtomicBool`，values 使用 `Mutex`，默认上下文和退出码也由互斥锁保护，全局 DDL 开关以原子操作更新。

正常生命周期为“创建派生上下文 → 发布共享 clone → 执行命令树 → 守卫析构 → 取消共享上下文”。`CancelOnDrop::drop` 通过 `Option::take` 保证闭包只被调用一次。错误路径则主动泄漏守卫以匹配 Go `os.Exit` 跳过 defer 的语义；这会同时泄漏守卫及其闭包捕获的 `Arc`，但真实进程本应立即退出。由于当前 `os_stub::Exit` 不终止，测试进程中这份泄漏会持续到进程结束，是已知移植边界。

[`main_test.rs`](main_test.rs) 的 `test_run_main` 在线程中调用 crate 公开入口并等待通道消息，验证当前无业务参数路径能够返回；测试前后清空 stub 退出码，防止 libtest 参数造成跨用例污染。`cleans_global_memory_arbitrator_before_leak_check` 另行验证测试清理 helper，但并非 `main.rs` 自身的资源管理逻辑。

## 与 Go 版本的对应关系

[`main.go`](main.go) 与 Rust `main()` 的顺序基本逐句对应：背景 context、`StartExitSingleListener`、延迟取消、根 Cobra 命令字段、公共 flags、默认 context、关闭 DDL、六个子命令、stdout、`os.Args[1:]`、执行失败日志和状态码 1 均被保留。六个子命令的顺序也一致，避免改变 CLI 顶层布局。

存在三项可验证差异：

1. Go 使用真实 `context.Context`、信号监听和 Cobra；Rust 当前使用 `stubs.rs` 中的精简 `Context` 与 `Command`，`StartExitSingleListener` 只派生取消标志，命令解析器也明确不是完整 Cobra。
2. Go 的 `defer cancel()` 在 `os.Exit` 路径不会运行；Rust 用 `CancelOnDrop` 模拟 defer，并在错误分支 `forget` 守卫。但 Rust 的 `os_stub::Exit` 只记录状态码，不实际退出，这是行为尚未完全对齐之处。
3. Go `rootCmd.SetOut(os.Stdout)` 连接真实标准输出；Rust `SetOut(())` 当前为空操作，命令输出保存在 `Command` 内部缓冲。

Go 的 [`main_test.go`](main_test.go) 用 goroutine/channel 验证 `main()` 可返回，并在 `TestMain` 中过滤 `--skip-goleak`、执行 goleak 检查和清理内存仲裁器。Rust [`main_test.rs`](main_test.rs) 以线程/同步通道保留“入口必须返回”的契约，并把参数过滤抽成纯函数；它验证内存仲裁器清理状态，但没有复刻 Go 的完整 goleak 扫描及忽略列表。内存上限表格测试对应 `cmd.rs::calculateMemoryLimit`，不是本文件行为。

## 扩展指南

- 新增一级命令时，在相应独立模块实现返回 `Command` 的工厂，再在 `lib.rs` 声明模块，并在本文件的 `AddCommand` 列表选择明确顺序接入；同步扩展独立测试文件（优先同目录 `*_test.rs`），不要把测试嵌入 `main.rs`。同时核对 Go `main.go` 是否存在对应命令，避免无意扩大迁移差异。
- 新增所有子命令共享的 flag 或初始化逻辑，应优先修改 `cmd.rs::DefineCommonFlags` 或其初始化链，而不是在 `main()` 为每个子命令重复配置。若只属于某个命令，应落在该命令工厂中。
- 改变默认 context 或退出行为时，需要同时审视 `CancelOnDrop`、`utils::StartExitSingleListener`、`os_stub::Exit` 与 `main_test.rs::test_run_main`。尤其要决定库调用场景是否允许真实终止进程，并避免在引入真实 `Exit` 后使测试进程被直接杀死。
- 接入真实 stdout、信号监听或 Cobra 等价解析器时，应替换对应适配层而保持本文件的编排顺序；兼容风险集中在参数解析、usage/输出去向、退出码及取消时机，性能风险主要是重复初始化或额外后台监听线程，而不是本入口的局部计算。
- 调整全局 DDL 开关时必须确认 BR 离线工具与下游 task 的契约；这是进程级共享状态，测试应在结束时复位，防止并行用例互相影响。

## 验证依据

- 目标源码：[`main.rs`](main.rs)；确认公开 `main`、私有 `CancelOnDrop`、`Drop` 实现、六个命令工厂调用、参数切片、错误日志与退出分支。
- crate 接线：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`bin_main.rs`](bin_main.rs)；确认 package/binary 元数据、模块重命名和两层转发入口。
- 直接实现：[`cmd.rs`](cmd.rs) 的 `DefineCommonFlags`、`SetDefaultContext`；[`stubs.rs`](stubs.rs) 的 `Context`、`Command::Execute`/`execute_command`、全局 config、`StartExitSingleListener` 与 `os_stub`。
- Go 对照：[`main.go`](main.go)；确认启动顺序、根命令字段、子命令顺序、DDL 开关、参数与错误退出意图。
- 独立测试：[`main_test.rs`](main_test.rs)、[`main_test.go`](main_test.go)；确认入口返回、参数过滤、测试资源清理以及 Rust 与 Go 测试覆盖范围的差异。
- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/cmd/br` 覆盖目标及直接证据文件；`node --file` 读取上述 Rust/Go 源码；`query` 定位入口、公共 helper 和六个命令工厂。对 `main.rs:23:function:main` 的 `callers`/`callees` 查询返回空数组，故入口调用链改由模块接线源码交叉验证。
- 任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令校验恰有十一个固定二级标题，并人工检查所有行为结论均可回溯到上述源码或查询结果。
