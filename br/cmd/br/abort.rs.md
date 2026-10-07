# `br/cmd/br/abort.rs`

源文件：[`abort.rs`](./abort.rs)

## 文件定位

本文件属于 `astersql-br-cmd-br` crate 的 BR 命令行装配层。crate 由 [`Cargo.toml`](./Cargo.toml) 定义，库入口 [`lib.rs`](./lib.rs) 通过 `pub mod abort` 暴露本模块，二进制入口 [`main.rs`](./main.rs) 再把 `NewAbortCommand()` 注册为 `br` 的一级子命令。它对应 Go 文件 [`abort.go`](./abort.go)，负责建立 `br abort restore {full,db,table,point}` 命令树、初始化公共环境、解析恢复配置并将请求交给任务层；恢复任务注册表与 checkpoint 的实际处理不在本文件内。

## 核心职责

- `NewAbortCommand` 创建 `abort` 根节点，注册持久化前置回调，并通过 `DefineRestoreFlags` 提供所有叶子节点可继承的通用恢复标志。
- `newAbortRestoreCommand` 建立中间层 `restore`，其下固定挂载 `full`、`db`、`table`、`point` 四种恢复类型。当前没有 backup abort；源码注释和 Go 对照文件都只把它作为未来扩展点。
- 四个 `newAbortRestore*Command` 为各恢复类型绑定命令名常量、无位置参数约束和类型专用 flags，最终统一进入 `runAbortRestoreCommand`。
- `runAbortRestoreCommand` 负责把命令 flags 转成 `RestoreConfig`、为 point/PITR 补充解析流恢复 flags、建立 tracing 包裹并调用 `astersql_br_pkg_task::RunRestoreAbort`。因此该函数是 CLI 与任务层的边界，而不是中止状态机本身。

## 主要符号

- `pub fn NewAbortCommand() -> Command`：本文件唯一公开 API。返回 `Use = "abort"`、`SilenceUsage = true` 的命令，并设置 `PersistentPreRunE`。前置回调依次调用 `Init`、`build::LogInfo(build::BR)`、`logutil::LogEnvVariables`、`log_arguments_for` 和 `session::DisableStats4Test`；任一步 `Init` 失败都会阻止叶子命令执行。
- `fn newAbortRestoreCommand() -> Command`：私有命令分组节点，按 full、db、table、point 的顺序保存四个子命令。
- `fn newAbortRestoreFullCommand() -> Command`：绑定 `FullRestoreCmd`，设置默认过滤器 `filterOutSysAndMemKeepAuthAndBind()`、非 stream 过滤语义以及 snapshot restore flags。
- `fn newAbortRestoreDBCommand() -> Command`：绑定 `DBRestoreCmd`，通过 `DefineDatabaseFlags` 接收数据库选择条件。
- `fn newAbortRestoreTableCommand() -> Command`：绑定 `TableRestoreCmd`，通过 `DefineTableFlags` 接收表选择条件。
- `fn newAbortRestorePointCommand() -> Command`：绑定 `PointRestoreCmd`，使用 stream 过滤语义，并通过 `DefineStreamRestoreFlags` 注册 PITR 的时间戳、全量备份位置和批处理等 flags。
- `fn runAbortRestoreCommand(command: &mut Command, cmdName: &str) -> Result<()>`：四个叶子的公共执行函数。`cmdName` 是任务类型判别值，也是任务层摘要标签。

本文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项；命令回调由 `Arc` 持有，以满足 `Command` 桩中回调的共享所有权要求。

## 执行流程

1. [`main.rs`](./main.rs) 构造 `br` 根命令并调用 `NewAbortCommand()`；`NewAbortCommand` 创建 `abort`、挂载 `restore` 子树并定义可继承的恢复 flags。
2. 命令执行器进入 abort 树时运行其 `PersistentPreRunE`：初始化日志/状态环境、记录参数，并关闭 session statistics 以降低 abort 路径内存占用。
3. 用户选择 `full`、`db`、`table` 或 `point`。叶子节点拒绝位置参数，并把对应的 `*RestoreCmd` 常量传入 `runAbortRestoreCommand`。
4. 公共执行函数以 `HasLogFile()` 初始化 `RestoreConfig.Config.LogProgress`，再用 `effective_task_flags` 合并继承 flags 与叶子本地 flags。
5. `RestoreConfig::ParseFromFlags(&flags, false)` 解析通用与恢复配置并执行基础校验；若失败，函数把 `command.SilenceUsage` 改为 `false` 后返回带 trace 的错误，使调用方能够展示 usage。
6. `IsStreamRestore(cmdName)` 仅在 `cmdName == PointRestoreCmd` 时为真；该分支额外调用 `ParseStreamRestoreFlags`。它会解析 start/restore TS 等字段，并拒绝同时提供 `start-ts` 与 full-backup storage。
7. 函数读取默认 context 和 `EnableOpenTracing`，取得全局 `tidbGlue` mutex guard，再由 `with_tracing` 包裹 `RunRestoreAbort(g.as_task(), cmdName, &mut cfg)`。
8. 任务层错误会记录 `failed to abort restore task` 并以 `Error::Trace` 返回；成功则返回 `Ok(())`。

## 数据与状态

- 命令树状态保存在 `Command` 的 `children`、`flags`、`persistent_flags`、`RunE` 和 `PersistentPreRunE` 字段中。`DefineRestoreFlags` 写入 abort 节点的 persistent flags；`effective_task_flags` 在运行时把它们与叶子 flags 合并成任务层 `FlagSet`。
- 每次叶子执行都新建一个 `RestoreConfig`，不会在多次调用之间复用请求配置。首先显式设置的字段只有 `Config.LogProgress`，其余由默认值和 flags 填充。
- `cmdName` 决定恢复类型和是否解析流式参数：full/db/table 走 snapshot 类路径，point 走 PITR 路径。
- `GetDefaultContext()` 读取由 `main.rs::SetDefaultContext` 安装的进程级可取消上下文；本文件不创建额外任务或 context 子节点。
- `tidbGlue()` 是共享全局 glue；本文件在调用任务层期间持有其 mutex guard。`RestoreConfig` 则以 `&mut` 传入任务层，允许任务实现调整配置和管理 checkpoint manager。

## 依赖与调用关系

上游接线为 `bin_main.rs` → `lib.rs::entry`/`main.rs::main` → `NewAbortCommand`。直接结构测试 [`parity_test.rs`](./parity_test.rs) 也调用 `NewAbortCommand`，验证 abort 位于根命令的既定顺序中、唯一一级子节点是 restore，且 restore 恰有四个叶子节点。

本文件通过 `crate::cmd` 依赖 CLI 公共设施：`Init`、`GetDefaultContext`、`HasLogFile`、`log_arguments_for`、默认过滤器、`tidbGlue` 和 `with_tracing`；通过 `crate::stubs` 使用 cobra 风格的 `Command`、错误、日志及 session/build 适配层。任务 API 来自 Cargo 的路径依赖 `astersql-br-pkg-task = ../../pkg/task`，主要包括 flag 定义函数、`RestoreConfig`、命令名常量、`IsStreamRestore` 与 `RunRestoreAbort`。

核心调用边为 `NewAbortCommand → newAbortRestoreCommand → 四个叶子构造器 → runAbortRestoreCommand → RunRestoreAbort`。RustCodeGraph 还确认四个叶子构造器是 `runAbortRestoreCommand` 在 Rust 文件内的全部调用者，而 `NewAbortCommand` 的直接 Rust 测试调用者是 `contract_normal_command_tree_and_filters`。

## 错误处理与边界

- 四个叶子都设置 `no_args = true`；额外位置参数应在命令框架边界被拒绝，而不是传到任务层解释。
- `ParseFromFlags` 的错误会打开 usage 并包装为 `Error::Trace`。相比之下，`ParseStreamRestoreFlags` 失败时直接返回 trace，代码没有把 `SilenceUsage` 改为 `false`；扩展错误展示时应保留或有意调整这一差异。
- `ParseFromFlags` 会拒绝例如零 `split-region-index-step`、非法 restore phase、未启用 checkpoint 却指定分阶段恢复等配置；PITR 二次解析还会检查互斥参数。
- `tidbGlue().lock().unwrap()` 在 mutex poisoned 时会 panic，而不是返回 `Result`。这是当前实现的明确边界。
- 任务层错误先写 error log，再向上传播；本文件没有重试、降级或部分成功处理。
- 当前 Rust `br/pkg/task/restore.rs::RunRestoreAbort` 只是调用 `Adjust`、`Summary`、`CloseCheckpointMetaManager` 和 `SetSuccessStatus(true)`。它尚未实现 Go 版的 operation context 校验、集群/备份元数据读取、restore registry 原子查删、checkpoint manager 初始化与 checkpoint 清理。因此命令树已接线不等于 Rust 版已经完成真实的暂停任务中止；这是最重要的功能边界。

## 并发与资源生命周期

- `Arc` 仅用于让命令回调可克隆共享；本文件没有启动线程、异步任务或通道。
- `GetDefaultContext` 传播进程退出取消信号，但当前 Rust 任务层简化实现没有创建 Go 版 `context.WithCancel` 对应的子 context。
- `with_tracing` 在 tracing 关闭时直接运行闭包；开启时创建 span/store，并在闭包返回后调用 `TracerFinishSpan`，因此正常成功和普通 `Result` 错误路径都会结束 span。它不是 panic guard，闭包 panic 时没有完成保证。
- glue mutex guard `g` 从调用 `RunRestoreAbort` 前一直持有到 tracing 调用返回；若任务层未来执行长时间网络/存储操作，这个临界区会覆盖整个操作，扩展时需评估串行化和死锁风险。
- `RestoreConfig` 在栈上创建并在函数返回时释放。当前任务实现显式 drain 并关闭 `CheckpointMetaManagers`；Go 完整实现还通过 defer 关闭 manager、registry 和取消 context，而这些资源生命周期尚未出现在 Rust 实现中。

## 与 Go 版本的对应关系

CLI 装配部分逐项对应 [`abort.go`](./abort.go)：顶层/中间层命令名与文案一致，四个叶子使用同一组任务常量和 flag 定义，full/point 的 filter stream 布尔值分别为 `false`/`true`，前置初始化顺序和解析失败时打开 usage 的行为也一致。Rust 的 `with_tracing` 封装对应 Go 的 `TracerStartSpan`/`defer TracerFinishSpan`，`effective_task_flags` 则补偿当前本地 `Command` 桩没有完整 cobra 父链 flag 视图的问题。

真正的业务实现目前不对齐：Go `br/pkg/task/restore.go::RunRestoreAbort` 会规范化 operation context，建立可取消 context 和 manager，解析 upstream cluster ID/start TS，创建 restore registry，原子查找并删除匹配的暂停任务，设置 restore ID，初始化 storage/table checkpoint managers 并清理 checkpoint；未找到任务时会记录日志后成功返回。Rust 同名函数当前没有这些步骤，也没有相应错误分支。故本文只能确认 CLI 契约与分派已移植，不能确认 Go abort 的实际效果已移植。

## 扩展指南

- 新增恢复类型时，应同时修改 `newAbortRestoreCommand`、任务层命令名判定/配置解析、Go 对照实现和独立 Rust 测试；至少扩展 [`parity_test.rs`](./parity_test.rs) 对叶子名称与数量的断言，不要把测试写进 `abort.rs`。
- 接入 backup abort 时，应新增独立构造器并挂入 `NewAbortCommand`，同时明确它是否能复用 `RestoreConfig`/`runAbortRestoreCommand`；Go 当前也只有 future 注释，不能据此假定协议已定义。
- 调整 flags 时，要区分 abort persistent flags 与叶子 local flags，并验证 `effective_task_flags` 的 `KNOWN_FLAG_NAMES` 能传递新增 flag；point 专属字段还须同步 `DefineStreamRestoreFlags` 与 `ParseStreamRestoreFlags`。
- 完成真实 abort 能力应优先补齐 `br/pkg/task/restore.rs::RunRestoreAbort`，按 Go 顺序保留 operation context、registry 原子删除、无匹配任务幂等成功、checkpoint 初始化的 best-effort 警告以及最终清理语义，并在 `br/pkg/task/restore_test.rs` 或新的同目录独立 `*_test.rs` 中覆盖。不要把这类业务逻辑塞回 CLI 文件。
- 若缩短 glue 锁持有时间或改变 tracing 生命周期，需要先确认任务层借用边界，并为成功、解析错误、任务错误和 panic/清理行为增加独立测试；兼容风险主要是脚本可见的命令树/usage，正确性风险主要是误删 registry 项或漏清 checkpoint，性能风险主要是持锁期间的远程 I/O。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、7,032 个 Rust 文件；目标 `br/cmd/br/abort.rs` 已索引，共识别 8 个符号。
- RustCodeGraph 读取与查询：`node --file br/cmd/br/abort.rs`；查询 `NewAbortCommand`、`runAbortRestoreCommand`、`RunRestoreAbort`、`IsStreamRestore`、`effective_task_flags`、`with_tracing`；调用图确认四个叶子进入公共执行函数，并定位任务层同名 Rust/Go 实现。
- 已核对源码/配置：[`abort.rs`](./abort.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`main.rs`](./main.rs)、[`cmd.rs`](./cmd.rs)、[`stubs.rs`](./stubs.rs)、[`abort.go`](./abort.go)、`br/pkg/task/restore.rs` 与 `br/pkg/task/restore.go`。
- 已核对测试：[`parity_test.rs`](./parity_test.rs) 只覆盖命令树与默认过滤器相关公共契约；仓库搜索未发现直接执行 abort 叶子或覆盖 `RunRestoreAbort` 业务效果的 Rust/Go 测试。本文因此明确标注执行语义的未覆盖和 Rust 任务层缺口。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文存在且恰好包含 11 个固定二级章节，并人工复核源码链接、符号名、调用边与 Go/Rust 差异。
