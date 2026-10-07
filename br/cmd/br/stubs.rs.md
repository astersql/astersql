# `br/cmd/br/stubs.rs`

## 文件定位

本文件属于 `astersql-br-cmd-br` crate，由 `br/cmd/br/lib.rs` 以 `pub mod stubs` 暴露，服务于同 crate 的 `main.rs`、`cmd.rs`、`backup.rs`、`restore.rs`、`stream.rs`、`operator.rs`、`abort.rs` 与 `debug.rs`。`br/cmd/br/Cargo.toml` 同时声明 library 入口 `lib.rs` 和 binary 入口 `bin_main.rs`，并注明 arm64 Darwin 构建不直接链接完整的 kv/domain/kvproto/grpcio，而使用 slim BR crates 与本地 traits/stubs；因此本文件是 CLI 迁移期的适配边界，不是备份恢复业务实现，也不是可连接真实 PD、TiKV 或 TiDB domain 的完整运行时。

源码顶部按职责把内容分为错误、context、Cobra 命令、日志/构建/summary、session/config/资源钩子、TiDB/TiKV glue 和 debug 元数据辅助。它已经带有 `// Copyright 2026 AsterSQL.`，且当前没有条件编译项。目标目录不存在 `doc.go`；最近的模块契约来自 `lib.rs`、Cargo manifest、各命令文件及独立 Rust 测试。

## 核心职责

本文件提供一组可在本地和单元测试中运行的最小替身，使 Rust BR 命令树能够完成装配、flag 读取、回调分派、错误断言和有限的 debug 数据处理，而不引入完整 Go 依赖图。最重要的三条边界是：`Command`/`execute_command` 模拟 Cobra 子命令递归与 persistent flag 继承；`Context`、全局配置及资源模块提供可观测但精简的进程状态；`TidbGlue`/`TikvGlue` 把命令层接到 `astersql-br-pkg-task` 与 `astersql-br-pkg-task-operator` 的 trait。

文件后半段还为 `debug.rs` 定义精简 `backuppb` 形状、`metautil`、范围重叠检测、ID 分配、rewrite rule 与日志搜索。这里存在有意的功能缺口：`LoadBackupTables` 恒返回空 map，`DecodeMetaFile`/`DecodeStatsFile` 恒成功，`Encrypt` 返回明文和零 IV，status listener 不监听端口，`OpMemGlue` 的 domain/session 永远报不可用，日志搜索不读取真实日志内容。调用方只能把它们当作结构和测试替身。

## 主要符号

- `Result<T>`、`Error` 与 `berrors`：统一本地错误类型；`Annotate`/`Wrapf` 只拼接消息，`Trace` 原样返回，`ErrorEqual` 使用整串或子串匹配。四个错误码只覆盖当前 cmd 使用面。
- `Context`：持有本级 `AtomicBool`、祖先取消标记、以 `u64` 为键的 `Any` 值 map；`WithCancel` 创建独立子标记并保留父取消链，`WithValue` 复制 map 后写入，避免反向污染父 context。
- `Command`、`RunEFn`、`PreRunEFn`、`HelpFn`：Cobra 替身。`Command::Execute` 进入私有 `execute_command`，`CMD_ID` 为每个默认命令分配单调 id，stdout/stderr 使用共享字节缓冲。
- `FlagSetExt`、`effective_task_flags`、`KNOWN_FLAG_NAMES`：在 task crate 的 `FlagSet` 上补充 clone/set 能力，并合并 local 与 persistent flag；已定义但未被 Visit 的默认值通过已知名称表补齐。
- `log`、`build`、`summary`、`session`、`config`、`gctuner`、`redact`、`memory`、`metricsutil`、`kv`、`logutil`：记录或暴露命令初始化所需的最小全局状态。它们多为原子量、mutex 或 no-op，而非真实日志、指标、GC tuner 和 TiDB 配置系统。
- `utils`、`TLS`、`ServeMux`、`NewTLS`：提供临时库名、去引号、退出监听、内存监控钩子、status handler/TLS 外观和 backup meta JSON 编解码。
- `InfoSchemaFilter`、`FilterLoadSysDBs`、`FilterLoadSpecifiedDBAndSysDBs`、`TidbGlue`、`OpMemGlue`、`TikvGlue`：命令层 glue。`TidbGlue::as_task/as_op` 返回两个下游 trait 视图；`TikvGlue` 提供版本、内存进度和空输出；`OpMemGlue` 明确拒绝 domain/session。
- `backuppb` 与 `metautil`：定义 debug 命令用的文件、BackupMeta、schema、表/库视图和 `tables.json` 旁路读取；`NewMetaReader` 仅保存 meta 与 storage。
- `rtree::RangeTree`、`mockid::IDAllocator`、`restoreutils`：分别执行线性半开区间重叠检测、从 1 开始的原子 ID 分配、按新旧表 ID 生成最小 rewrite rule 并做粗校验。
- `stream_search::StreamBackupSearch`：保存 storage、搜索 key 与时间上下界；`Search` 只检查 key 非空并返回 key 的十六进制表示，不扫描 storage。
- `debug_runtime`、`os_stub`、`once_do`：分别模拟 `debug.SetMemoryLimit`、环境/参数/退出副作用和 `Once::call_once`；`os_stub::Exit` 只记录退出码，不结束进程。

## 执行流程

命令主链从 `br/cmd/br/main.rs::main` 开始：创建 `Context::Background`，经 `utils::StartExitSingleListener` 派生可取消 context，构建根 `Command`，加入各业务子命令后设置参数并调用 `Command::Execute`。`execute_command` 在每一层先运行 `PersistentPreRunE`；若参数为空则运行本层 `RunE`，否则取得第一个参数，收集父命令 persistent flags（包括已修改值和已知默认值），找到 `Use` 首词或 `Aliases` 匹配的子命令，写入继承值并递归。没有子命令但本层有 `RunE` 时把剩余参数交给它；否则返回 `unknown command <name>`。

业务回调通过 `effective_task_flags` 获取 persistent 与 local flag 的合并视图。`backup.rs`、`restore.rs`、`abort.rs`、`stream.rs`、`cmd.rs` 与 `debug.rs` 都调用该函数；raw/txn 路径构造 `TikvGlue`，普通 TiDB 路径经 `cmd.rs::tidbGlue` 使用 `TidbGlue`。`debug.rs` 的 checksum/backupmeta 路径使用 `metautil::LoadBackupTablesFromStorage` 读取可选 `tables.json`，搜索路径按 flag 构造 `StreamBackupSearch`，设置 start/end TS 后调用 `Search`。

初始化辅助由 `cmd.rs::Init` 消费：读取 logging/config/memory/redact 状态，调用 `memory::MemTotal/MemUsed`、`debug_runtime::SetMemoryLimit`、`utils::RunMemoryMonitor` 及 status server 替身。`main.rs` 捕获执行错误后写入 `log::Error` 并调用只记录退出码的 `os_stub::Exit(1)`，所以测试进程不会被终止。

## 数据与状态

命令树本身按值持有子命令；回调、输出缓冲和 context 内值通过 `Arc` 共享。`Command::Clone` 会共享 `out`、`err_out` 与回调对象，但复制 flags 和 children 的容器状态。`CMD_ID`、mock ID 与内存限制使用原子递增/交换；取消、summary、stats、redact、monitor 等开关使用 `SeqCst` 原子操作。

进程级状态散布在 `OnceLock<Mutex<_>>` 或静态 `Mutex` 中：日志错误缓冲、全局配置、测试内存值、status/环境/退出记录等跨调用保留，部分模块提供 `take_*` 或 `reset_for_test` 清理接口。多数锁直接 `unwrap`，poison 后会 panic。`Context::values` 保存 `Arc<dyn Any + Send + Sync>`；派生 value context 会复制 map，但其中的值对象仍共享。字段 `next_key` 当前只在派生 context 间共享，源码中没有分配键的消费者。

`LoadBackupTablesFromStorage` 把缺失或读取失败的 `tables.json` 视为空结果；成功读取时从宽松 JSON 默认值构造 database/table/file，所有文件暂放入 physical id `1`。`RangeTree` 用 `Vec` 线性保存区间；`InsertRange` 返回第一个重叠旧区间，否则追加新值，复杂度随已存区间线性增长。

## 依赖与调用关系

RustCodeGraph 将目标文件识别为 289 个符号，并报告被 30 个文件使用。直接生产调用者集中在 `br/cmd/br`：`lib.rs` 暴露模块，`main.rs` 使用 context、Command、log/os/config，`cmd.rs` 使用初始化与 glue，`backup.rs`/`restore.rs` 使用 filter、glue 和 flag 合并，`stream.rs`/`abort.rs`/`operator.rs` 使用 CLI/context 边界，`debug.rs` 使用 backuppb/metautil/rtree/stream_search。

外部 Rust 下游由 Cargo manifest 限定为 `astersql-br-pkg-task`、`astersql-br-pkg-task-operator`、trace/config 等本地 crate，以及 `serde`/`serde_json`、`sha2`、`hex`。本文件直接复用 task crate 的 `FlagSet`、`Glue`、`MemGlue`、`Storage` 和 protobuf/cipher 外观，复用 operator crate 的 `FlagSet`、`Glue`、`KVStorage`、`Domain`、`Session`；因此 flag 与 trait 的真实定义不在本文件。

RustCodeGraph 对常见名称存在跨 Go/Rust 的大量同名候选；精确 `query` 定位了本文件的 `execute_command`、`effective_task_flags`、`LoadBackupTablesFromStorage` 与 `NewStreamBackupSearch`，但本地 `callers/callees` 命令在这些节点上 30 秒内无输出。直接调用边因此由模块引用和源码调用点补证：`Command::Execute -> execute_command`，`execute_command -> PersistentPreRunE/RunE/find_child/FlagSetExt`，`debug.rs -> LoadBackupTablesFromStorage/NewStreamBackupSearch`，各命令解析路径 `-> effective_task_flags`。

## 错误处理与边界

本地 `Error` 只有消息，没有 Go `errors` 的 cause、stack、类型链或结构化错误码；`ErrorEqual` 的子串匹配可能产生比 Go 错误身份比较更宽松的结果。flag 解析、JSON 解析、空搜索 key 和缺 rewrite rule 会返回 `Result::Err`，而许多桩刻意不报错：logger/TLS/metrics/status 启动、meta/stats decode 及明文“加密”均恒成功。

边界能力必须按符号逐项理解。`Command` 不是完整 Cobra：不自行解析原始 flag token，不维护父指针，`Root()` 只返回当前对象，`SetOut` 不接收真实 writer，`TraverseChildren`/`Version` 等字段主要供契约观测，`no_args` 字段也不会由 `execute_command` 自动调用 `cobra::NoArgs`。persistent pre-run 在递归的每一层执行，而真实 Cobra 的精确钩子选择规则更丰富。

安全相关桩不提供生产保证：`metautil::Encrypt` 不加密，`DecodeMetaFile`/`DecodeStatsFile` 不校验内容，`LoadBackupTables` 不解析 meta，status listener 不监听网络，`TikvGlue` 不访问 TiKV，`OpMemGlue` 无 domain/session。`LoadBackupTablesFromStorage` 对缺字段大量采用默认值，对非法 sha256 使用空字节，对 storage 读取错误退化为空 map；这些都是测试便利行为，不应作为真实备份校验语义。

## 并发与资源生命周期

`Context::WithCancel` 返回捕获本级 `Arc<AtomicBool>` 的一次或多次可调用闭包；子 context 的 `is_cancelled` 同时检查自身和所有祖先，因此父取消能向下传播。值 map 由 mutex 保护。`parity_test.rs::context_child_observes_parent_cancellation_and_values_are_scoped` 直接验证父取消传播和值不反向污染，`operator_context_observes_command_cancellation` 验证取消标记能桥接至 operator context。

全局状态的并发访问大多由原子量或 mutex 串行化；`OnceLock` 延迟初始化日志与 Config，`once_do` 直接委托 `Once::call_once`。这些实现没有后台任务生命周期：`RunMemoryMonitor` 仅置位，status server 不启动线程，`StreamBackupSearch` 同步返回，`os_stub::Exit` 不退出。真正的 goroutine、网络连接、PD/TiKV client、domain/session 与 storage 关闭责任都位于 Go 实现或下游真实 crate，而非本文件。

测试必须清理跨用例状态，优先调用 `take_errors`、`take_logged`、`take_exit`、`reset_for_test` 或相应注入器；否则并行测试可能观察到共享状态。源码用 `Mutex::unwrap` 且没有 poison 恢复，闭包 panic 可能使后续测试继续 panic。

## 与 Go 版本的对应关系

Go 侧没有与本文件一一对应的 `stubs.go`。Rust 将多个真实 Go 包的边界集中到一个文件：`context` 对应 `Context`，`github.com/spf13/cobra` 对应 `Command`/`cobra`，`pingcap/errors` 与 `br/pkg/errors` 对应 `Error`/`berrors`，`br/pkg/gluetidb`/`gluetikv` 对应 glue 类型，`br/pkg/metautil`、`br/pkg/rtree`、`br/pkg/stream` 和 protobuf 类型对应 debug 辅助模块，`os`、`runtime/debug`、config/log/memory/summary 则对应同名外观。

可以确认的对齐意图包括：根命令装配与 `StartExitSingleListener` 来自 `br/cmd/br/main.go`；日志、内存、status、全局 context 与 TiDB glue 的调用位置来自 `cmd.go`；系统库过滤及 TiKV glue 选择来自 `backup.go`/`restore.go`；backup meta checksum、范围校验和日志搜索入口来自 `debug.go`。独立 `parity_test.rs` 还验证 ErrorEqual、取消链、value 作用域、version flag、本地临时库名和去引号等迁移契约。

语义差异是当前实现的重要事实：Go Cobra 负责完整解析和命令生命周期，Rust 仅递归已拆分参数并复制已知 flags；Go glue 连接真实 TiDB/TiKV，Rust `MemGlue`/`TikvGlue` 主要记录或返回固定值；Go metautil/stream 执行 protobuf、加密、storage 扫描和校验，Rust 多数只实现 JSON sidecar 或形状；Go `os.Exit` 终止进程，Rust 只记录 code。后续扩展不能用已有符号同名来推断能力已经等价。

## 扩展指南

扩展 CLI flag 时，应先在 task/operator 的 `FlagSet` 定义和命令构造器中注册，再评估是否要把名称加入 `KNOWN_FLAG_NAMES`；否则未被修改的默认 flag 可能无法经 `effective_task_flags` 或父子继承传递。修改命令分派时应补充独立测试覆盖 alias、未知命令、pre-run 顺序、persistent/local 同名覆盖和 no-args 约束，不要把测试写进 `stubs.rs`。

若目标是生产可用，应优先替换边界而不是继续扩大万能桩：真实 domain/session 应在独立上游 crate 中实现并通过带 tag 的 Git 依赖接入；真实 encryption/meta decode/log search/status listener 也应迁移到各自 canonical 模块。本文件只保留最薄适配。涉及安全或数据正确性的替换必须新增错误、损坏输入、加密 round-trip、storage 失败和资源清理测试，不能把恒 `Ok` 视为完成。

现有最近测试位于独立文件：`br/cmd/br/parity_test.rs` 覆盖公共迁移契约和 context，`main_test.rs` 覆盖入口返回与退出副作用，`cmd_test.rs` 覆盖 status preparer，`debug_test.rs` 覆盖 debug 命令树/字段，`stream_test.rs` 覆盖 usage/help，`backup_test.rs` 覆盖 filter 清理。新增桩行为应扩展这些文件或新建同目录 `*_test.rs`；Go 对照应同步查阅 `main.go`、`cmd.go`、`backup.go`、`restore.go`、`debug.go` 及相应 Go 测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/cmd/br/stubs.rs` 报告目标文件有 289 个符号；`node --file` 分段读取完整 1705 行源码并报告 30 个使用文件；`query --json` 精确定位 `stubs.rs::execute_command`（372）、`effective_task_flags`（1620）、`LoadBackupTablesFromStorage`（1261）、`NewStreamBackupSearch`（1516）。`callers/callees` 对精确节点未返回内容，故未将空输出解释为“无调用者”。
- crate/入口与直接调用点：`br/cmd/br/Cargo.toml`、`lib.rs`、`main.rs`、`cmd.rs`、`backup.rs`、`restore.rs`、`stream.rs`、`operator.rs`、`abort.rs`、`debug.rs`；目录中无 `doc.go`。
- Go 对照：`br/cmd/br/main.go`、`cmd.go`、`backup.go`、`restore.go`、`debug.go`、`operator.go`、`stream.go`、`abort.go`；它们证明 Rust 集中桩对应的是多个真实 Go 包而非单文件复刻。
- 测试证据：`br/cmd/br/parity_test.rs`、`main_test.rs`、`cmd_test.rs`、`debug_test.rs`、`stream_test.rs`、`backup_test.rs`；搜索确认没有同名 `stubs_test.rs`，直接契约主要集中在 `parity_test.rs`。
- 本任务只新增说明文档，按计划不运行 Cargo。交付时执行任务指定的 11 章节结构检查，并人工确认文档区分了已接线行为、固定值/no-op、显式错误和未实现的真实集群能力。
