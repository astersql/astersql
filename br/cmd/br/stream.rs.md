# `br/cmd/br/stream.rs`

## 文件定位

[`br/cmd/br/stream.rs`](stream.rs) 是 Rust 版 BR 命令行中 `br log` 命令族的装配层。crate 入口 `br/cmd/br/lib.rs` 以 `pub mod stream` 导出本模块，进程入口 `br/cmd/br/main.rs::main` 在根命令中调用 `NewStreamCommand()`，因此用户执行 `br log ...` 时会进入这里。该文件属于 Cargo 包 `astersql-br-cmd-br`；`br/cmd/br/Cargo.toml` 同时把 `lib.rs` 声明为库入口、把 `bin_main.rs` 声明为二进制入口，并通过路径依赖连接 `astersql-br-pkg-task`、`astersql-br-pkg-streamhelper-config` 和 `astersql-br-pkg-trace`。

本文件只负责命令树、标志解析、公共初始化后的分流和 tracing 包装，不实现日志备份状态机。真正的任务分派位于 `br/pkg/task/stream.rs::RunStreamCommand`，更深的 start/stop/pause/resume/status/truncate/metadata/advancer 行为也在该 task crate。当前 Rust task 实现明确包含若干 slim stub，因此“命令已接入”不等于 Go 版集群侧行为已经完整移植。

## 核心职责

- `NewStreamCommand` 构造可见的顶层 `log` 命令，安装持久前置回调、八个叶子命令和定制 help 回调。
- 八个 `newStream*Command` 函数分别声明命令名、无位置参数约束、执行回调以及该子命令需要的 flag 集合；`advancer` 被注册但保持隐藏。
- `streamCommand` 将命令上的 persistent/local flags 合并成 task crate 可消费的视图，先解析公共 `Config`，再根据命令类型解析 `StreamConfig` 的专有字段。
- `map_stream_cmd` 把 CLI 层保留的 Go 风格常量（如 `"log start"`）转换为当前 Rust task 分派器接受的短名（如 `"start"`）。
- `install_stream_help` 在委托原 help 回调之前调用 `HiddenFlagsForStream`，隐藏 backup/restore 场景专用而 stream 不适用的根级标志。
- 所有执行错误都会把 `Command::SilenceUsage` 设为 `false`，让调用方在失败时重新展示 usage；成功路径保留默认的静默设置。

## 主要符号

- `StreamStart`、`StreamStop`、`StreamPause`、`StreamResume`、`StreamStatus`、`StreamTruncate`、`StreamMetadata`、`StreamCtl`：公开的 Go 风格命令标识，值分别为 `log` 加叶子命令名。它们也是各 `RunE` 闭包传入 `streamCommand` 的稳定判别值。
- `map_stream_cmd(&str) -> &str`：内部归一化函数。已知八个常量被映射成 task crate 的短命令名；未知字符串原样返回，随后由 `RunStreamCommand` 报未知命令。
- `NewStreamCommand() -> Command`：公开命令工厂。设置 `Use = "log"`、`SilenceUsage = true`，安装 `PersistentPreRunE`，按 start、stop、pause、resume、status、truncate、metadata、advancer 的顺序注册子命令。
- `install_stream_help(&mut Command)`：crate 内可见的 help 装饰器。它先保存旧回调，避免覆盖后递归调用自身；新回调先隐藏 stream 不适用的 flags，再调用旧回调。
- `newStreamStartCommand`：注册全表默认过滤、stream 过滤模式及 start 专用 flags。
- `newStreamStopCommand`、`newStreamResumeCommand`：只注册 stream 公共 flags。
- `newStreamPauseCommand`：注册 pause flags，其中包括公共任务名、暂停消息和 GC TTL。
- `newStreamStatusCommand`：注册 status flags，包括任务选择和 JSON 输出设置。
- `newStreamTruncateCommand`：注册截断时间、确认、dry-run、compaction 清理等 flags。
- `newStreamCheckCommand`：构造 `metadata` 命令；没有额外的 stream 专用解析步骤。
- `newStreamAdvancerCommand`：构造隐藏的调试命令，注册公共 stream flags 和 checkpoint advancer 的 duration 配置。
- `streamCommand(&mut Command, &str) -> Result<()>`：公开公共执行路径，也是本文件唯一真正触发 task 层工作的函数。

本文件没有自定义 struct、enum、trait 或条件编译项；命令节点和错误类型均来自 `crate::stubs`，业务配置来自 `astersql_br_pkg_task::StreamConfig`。

## 执行流程

1. `br/cmd/br/main.rs::main` 创建根命令、定义公共 flags，并把 `NewStreamCommand()` 的返回值加入根命令。
2. `NewStreamCommand` 建立 `log` 节点。其 `PersistentPreRunE` 依次调用 `Init`、打印 BR 构建信息、记录环境变量和经脱敏/整理后的命令参数；任一步返回错误都会阻止叶子命令执行。
3. 用户选择叶子命令后，对应 `RunE` 闭包用固定的 `Stream*` 常量调用 `streamCommand`。所有叶子都标记 `no_args = true`，不接受位置参数。
4. `streamCommand` 以 `HasLogFile()` 初始化 `cfg.Config.LogProgress`，再通过 `effective_task_flags` 合并命令的 persistent flags 和 local flags。
5. 首先执行 `cfg.Config.ParseFromFlags(&flags)`，解析存储、PD、TLS、日志、校验等公共 BR 配置。失败时立即恢复 usage 输出并返回带 trace 包装的错误。
6. 然后按 `cmdName` 分流：`metadata` 不做额外解析；`truncate`、`status`、`start`、`pause` 使用各自的 `ParseStream*FromFlags`；`advancer` 先解析公共 stream 字段，再读取 advancer flags 并逐字段复制到 `cfg.AdvancerCfg`；其他值（正常包括 stop、resume，也包括未知值）先走 `ParseStreamCommonFromFlags`。
7. 专有解析失败时设置 `SilenceUsage = false` 并返回错误。解析成功后读取 `GetDefaultContext()`，按 `cfg.Config.EnableOpenTracing` 选择是否由 `crate::cmd::with_tracing` 包裹任务调用。
8. 调用 task 层前，`map_stream_cmd` 将长命令名转换成短名。闭包锁定全局 `tidbGlue`，把 `g.as_task()`、短命令名和可变 `StreamConfig` 传给 `astersql_br_pkg_task::RunStreamCommand`。
9. task 层再按短名调用对应 `RunStream*`。若 task 返回错误，`streamCommand` 同样恢复 usage；最终把原结果传回 CLI 框架。

## 数据与状态

主要临时状态是栈上的 `StreamConfig`。其内嵌 `Config` 保存公共 BR 参数，扩展字段保存任务名、起止 TS、GC TTL、truncate 模式、JSON 输出、pause 消息和 advancer 配置。配置在每次叶子执行时重新创建，没有跨命令缓存。

`Command` 同时承载本地 flags、persistent flags、回调、子节点、输出缓冲和 `SilenceUsage`。`effective_task_flags` 复制 persistent 与 local flag 值，并让 local 值覆盖同名 persistent 值；这一步是 Rust stub 命令模型与 task crate `FlagSet` 之间的桥接。

进程级共享状态有三处：`Init` 使用 once 语义初始化日志、内存监控和脱敏配置；`GetDefaultContext` 返回根入口预先保存的可取消上下文；`tidbGlue()` 返回受 mutex 保护的全局 glue。`HasLogFile()` 读取初始化期间设置的原子状态，用来决定任务进度是否写日志。

`advancer` 的配置先由 `DefaultCommandConfig()` 建立默认值，再由 `GetFromFlags` 更新，最后复制进 `StreamConfig::AdvancerCfg`。这不是借用外部 flag map：执行 task 时配置已经成为 `StreamConfig` 的自有值。

## 依赖与调用关系

上游主链为 `br/cmd/br/bin_main.rs` 的二进制包装层、`br/cmd/br/lib.rs::main`、`br/cmd/br/main.rs::main`，最终由后者直接调用 `NewStreamCommand`。测试上游包括 `br/cmd/br/parity_test.rs` 与 `br/cmd/br/stream_test.rs`；RustCodeGraph 的文件节点也把这两个文件列为目标文件的使用者。

本文件的主要下游分为三组：

- `crate::cmd`：`Init`、`log_arguments_for`、`GetDefaultContext`、`HasLogFile`、`with_tracing`、`tidbGlue` 和默认全表过滤器 `acceptAllTables`。
- `astersql_br_pkg_task`：flag 定义函数、`HiddenFlagsForStream`、`StreamConfig` 的解析方法以及最终的 `RunStreamCommand`。`br/pkg/task/stream.rs` 显示该分派器接受短名并调用八个 `RunStream*` 函数。
- `astersql_br_pkg_streamhelper_config`：为隐藏的 `advancer` 命令定义并解析 checkpoint 推进周期参数。

`br/cmd/br/Cargo.toml` 直接声明上述 task、streamhelper-config 和 trace 路径依赖。命令对象、错误、build/log 工具及 glue 接口当前经 `crate::stubs::*` 提供；因此文档和扩展代码都应先确认使用的是 stub API 还是已经迁移到独立 crate 的真实实现。

## 错误处理与边界

- 公共 flag 解析错误经 `Error::Trace` 返回；子命令专有解析错误也转成统一 `Error`。两类解析错误都会设置 `SilenceUsage = false`。
- task 执行错误原样作为 `Result` 返回，并在返回前恢复 usage。`br/cmd/br/stream_test.rs::execution_errors_restore_usage_output_like_go_defer` 用未知命令验证了这一契约。
- `map_stream_cmd` 对未知字符串不拒绝，而是交给 task 层。由于默认分支会先解析公共 stream flags，缺少 `task-name` 时可能先得到参数错误；公共字段合法时才会得到 `unknown stream command`。
- `metadata` 只跳过 stream 专有解析，不跳过最前面的公共 `Config::ParseFromFlags`；不能据此认为 metadata 完全无参数要求。
- `truncate` 的 `ParseStreamTruncateFromFlags` 当前没有调用 `ParseStreamCommonFromFlags`，因此其边界由 task crate 的 truncate 解析和执行逻辑共同决定，不能套用 stop/resume 的任务名要求。
- `advancer` 是隐藏调试命令而非稳定用户接口。它会解析 task-name 与 duration flags，但当前 task crate 的 `RunStreamAdvancer`/`runOwnershipCycle` 仍是占位式实现。
- `tidbGlue().lock().unwrap()` 在 mutex 中毒时会 panic，而不是返回 CLI 错误；这是当前 Rust 实现与普通可传播错误路径不同的边界。
- `install_stream_help` 依赖先捕获旧 help 回调再替换。直接反复安装会形成回调包装链，虽会逐层委托，但没有必要重复安装。

## 并发与资源生命周期

命令树和单次 `StreamConfig` 在当前调用栈内创建并按值拥有。各 `RunE`、`PersistentPreRunE` 和 help 回调使用 `Arc<dyn Fn... + Send + Sync>`，允许 `Command` 克隆并满足线程安全回调接口；本文件自身不创建线程或异步任务。

执行 task 时会持有全局 `tidbGlue` 的 mutex guard，guard 覆盖整个 `RunStreamCommand` 调用，返回后自动释放。这使同一进程内经该 glue 发起的 stream 任务串行化，也意味着 task 层若反向尝试获取同一锁会有死锁风险，扩展时不应在下游形成重入。

tracing 生命周期由 `with_tracing` 封装：关闭时直接执行闭包；开启时先创建 span/store，闭包返回后调用 finish，再返回结果，因此正常成功和普通 `Err` 都会结束 tracing。panic 不在该函数的错误模型内，无法保证 finish。

默认上下文由根入口设置并可传播退出取消信号，但本文件只取得并传入包装层。当前 `with_tracing` 闭包参数名为 `_ctx`，实际 task Rust 接口也不接收 context，所以这里不能宣称取消信号已经贯穿到 task 实现；这与 Go `task.RunStreamCommand(ctx, ...)` 仍有结构差异。

## 与 Go 版本的对应关系

直接对照文件是 `br/cmd/br/stream.go`。命令名、简介、八个子命令及其顺序、无位置参数约束、隐藏 advancer、flag 定义函数、公共初始化顺序、help 隐藏逻辑和 `streamCommand` 的解析分支都按 Go 版保留。

关键差异如下：

- Go 版直接使用 `task.StreamStart` 等长命令名，并由 `task.StreamCommandMap` 按长名分派；Rust CLI 在 `map_stream_cmd` 中转换为短名，因为当前 `br/pkg/task/stream.rs::RunStreamCommand` 按 `"start"` 等短名匹配。
- Go 版把 `context.Context` 和 glue 直接传给 task，并用 appdash span 包裹；Rust 版通过 `with_tracing` 管理 span，但 task 接口只有 glue、命令名和配置，取到的默认 context 尚未传入 task。
- Go 版在 `streamCommand` 中使用 `defer` 统一根据命名返回值 `err` 恢复 usage；Rust 版在三类显式错误出口分别设置 `SilenceUsage = false`。
- Go `advancer` 直接让 `cfg.AdvancerCfg.GetFromFlags(command.Flags())` 填充配置；Rust 版先解析独立配置对象，再逐字段复制到 task crate 的 `AdvancerCommandConfig`。
- Go task 层包含完整的 etcd、PD、TiKV、对象存储和 streamhelper 生命周期；当前 `br/pkg/task/stream.rs` 明确将 `buildObserveRanges`、锁操作、ownership cycle 等部分实现标为 stub。因此本 CLI 文件的对齐重点是命令契约和接线，不是底层行为等价证明。

Go 同目录没有独立的 `stream_test.go`。Rust 的直接测试位于 `br/cmd/br/stream_test.rs`，另有 `br/cmd/br/parity_test.rs` 覆盖命令树、flag 默认值及若干错误入口；task 层更细的边界测试位于 `br/pkg/task/stream_test.rs`，其 Go 对照是 `br/pkg/task/stream_test.go`。

## 扩展指南

新增 stream 子命令时，至少需要同步四层：在本文件新增稳定常量和 `newStream*Command` 工厂、把工厂加入 `NewStreamCommand`、在 `streamCommand` 增加正确的解析分支并在 `map_stream_cmd` 增加映射、在 `br/pkg/task/stream.rs::RunStreamCommand` 增加同名短命令分派。若 Go 是语义来源，还应同步 `br/cmd/br/stream.go` 对应增量，而不是只做能通过 Rust 测试的简化路径。

新增或改变 flags 时，应先判断它属于根级 persistent、stream 公共还是叶子专有集合，并确认 `effective_task_flags` 能复制该名称。该 helper 对未列入 `KNOWN_FLAG_NAMES` 且未出现在已定义值集合中的默认 flag 可能无法补齐，因此新 flag 的默认值解析需要单独验证。

修改错误路径时应保持“失败展示 usage、成功保持静默”的契约，并扩展独立的 `br/cmd/br/stream_test.rs`；不要把测试放回生产源文件。命令树、可见性和默认 flag 契约适合扩展 `br/cmd/br/parity_test.rs`，task 配置或底层行为则应扩展 `br/pkg/task/stream_test.rs` 及相应 Go 测试。

修改 tracing、全局 glue 或初始化流程时要评估锁持有范围、panic 时清理、上下文取消传播和一次性初始化的兼容风险。性能上，本文件本身开销主要是 flags 复制和全局 mutex；真正的网络、存储与长任务成本位于 task 层，不应通过在 CLI 层缓存可变 `StreamConfig` 来规避。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/cmd/br/stream.rs` 确认目标文件已索引且有 26 个符号；`node --file br/cmd/br/stream.rs --offset 1 --limit 500` 返回完整 299 行源码，并标出 `br/cmd/br/parity_test.rs`、`br/cmd/br/stream_test.rs` 两个使用者；`query NewStreamCommand` 与 `query streamCommand` 同时定位 Rust/Go 对照定义及 task 分派入口。精确 `callers` 查询长时间无输出后被终止，因此调用边又由以下源文件逐点核验，而未把超时当作成功证据。
- 目标与入口：`br/cmd/br/stream.rs`、`br/cmd/br/lib.rs`、`br/cmd/br/main.rs`。
- crate 与依赖：`br/cmd/br/Cargo.toml`。
- Rust 下游：`br/cmd/br/cmd.rs::with_tracing`、`br/cmd/br/stubs.rs::Command`、`br/cmd/br/stubs.rs::effective_task_flags`、`br/pkg/task/stream.rs::{StreamConfig, RunStreamCommand}`。
- Go 对照：`br/cmd/br/stream.go`、`br/pkg/task/stream.go::RunStreamCommand`。
- 测试证据：`br/cmd/br/stream_test.rs` 验证错误恢复 usage 与 help 委托；`br/cmd/br/parity_test.rs` 验证命令树、隐藏 advancer、常量和 flag 默认值；`br/pkg/task/stream_test.rs` 与 `br/pkg/task/stream_test.go` 提供 task 配置和边界语义证据。
- 本任务只新增文档，按计划不运行 Cargo；交付验证只检查文档存在、固定章节数量和人工事实一致性。
