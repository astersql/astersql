# `pkg/lightning/backend/backend.rs`

## 文件定位

本文件是 `astersql-lightning-backend` crate 的后端协议与引擎生命周期门面。`pkg/lightning/backend/lib.rs` 将 `backend` 模块整体再导出，因此调用方通常直接从 crate 根使用这里的 `Backend`、`EngineManager`、`OpenedEngine`、`ClosedEngine` 和 `EngineWriter`。`pkg/lightning/backend/Cargo.toml` 声明该 crate 只直接依赖同目录的 `encode` crate 与启用 `v4`/`v5` 的 `uuid`，并用 `package.metadata.porting.go-package` 指向 Go 包 `pkg/lightning/backend`。

它位于 Lightning/IMPORT 数据通路的“编排与具体存储实现”之间：上层通过统一状态句柄执行打开、写入、关闭、导入、清理，下层实现 `Backend` 和 `EngineWriter`。例如 `pkg/executor/importer/table_import.rs::NewTableImporter` 创建 `EngineManager`，`OpenDataEngine`/`OpenIndexEngine` 打开引擎，`ImportAndCleanup` 导入后清理；`pkg/session/runtime/import_query.rs` 也按相同流程处理 `IMPORT FROM SELECT`。具体物理 SST 适配见 `pkg/session/runtime/import_sst.rs`，逻辑 SQL 后端见 `pkg/lightning/backend/tidb/tidb.rs`。

## 核心职责

1. 用 `Backend` 和 `EngineWriter` 定义后端必须提供的线程安全生命周期及写入能力，并用 `TargetInfoGetter` 定义导入前远端元数据/要求检查能力。
2. 用 `EngineManager`、私有 `engine`、`OpenedEngine`、`ClosedEngine` 将正常生命周期表达为 `Open -> LocalWriter/Flush -> Close -> Import -> Cleanup`，减少上层直接传 UUID 的重复代码。
3. 用 `MakeUUID` 根据 `tableName:engineID` 生成稳定 UUID v5，使断点恢复时能重新定位相同引擎；索引引擎等特殊引擎仍由数值 ID 区分。
4. 集中承载跨后端配置与结果类型，包括 `EngineConfig`、`LocalWriterConfig`、`ExternalEngineConfig`、`EngineFileSize`、`ChunkFlushStatus` 和 `BackendError`。
5. 在 `ClosedEngine::Import` 中统一执行最多三次的可重试导入；具体何种错误可重试由后端构造 `BackendError.retryable` 时决定。

## 主要符号

- `importMaxRetryTimes = 3`：`ClosedEngine::Import` 的总尝试次数，不是“首次调用后再重试三次”。
- `ENGINE_NAMESPACE`、`makeTag`、`MakeUUID`：将表名和引擎 ID 拼成标签，再以固定命名空间生成确定性 UUID v5。测试 `backend_test.rs::TestOpenCloseImportCleanUpEngine` 固定校验 `` `db`.`table`:1 `` 对应 `902efee3-a3f9-53d4-8c82-f12fb1900cd1`。
- `Logger`、`makeLogger`：轻量键值字段容器；管理器为引擎附加 `engineTag` 和 `engineUUID`。它不是 Go 版完整的 zap 日志器，仅保留可随 `ClosedEngine` 传递的上下文字段。
- `Backend: Send + Sync`：定义后端关闭、重试延迟、后处理判断、引擎打开/关闭/导入/清理/刷写以及 writer 创建。`Send + Sync` 约束允许 `Arc<dyn Backend>` 跨线程共享。
- `TargetInfoGetter: Send + Sync`：读取远端库表模型并执行导入要求检查；其 `TableInfo`/`ColumnInfo` 是 Rust 侧简化但显式的远端模型。
- `EngineManager`：无额外可变状态，仅持有 `Arc<dyn Backend>`；`MakeEngineManager` 构造它，`OpenEngine` 和两种 `UnsafeCloseEngine*` 创建带类型状态的句柄。
- `OpenedEngine`：保存引擎公共句柄、表名和打开配置；提供 `LocalWriter`、`Flush`、`Close`、UUID/ID 读取。
- `ClosedEngine`：只暴露导入、清理、日志器和标识读取；`NewClosedEngine` 支持恢复或已有组件直接组装。
- `EngineWriter: Send`：按列名和 `encode::Rows` 追加批次，报告同步状态，关闭时可返回 `ChunkFlushStatus`。
- `BackendError`：保存消息、`retryable` 和 `duplicate` 三项语义；`new` 创建普通错误，`retryable` 创建可重试错误，并实现 `Display`/`Error`。
- 配置/状态 DTO：`LocalWriterLocalConfig` 控制有序 KV 与缓存，`LocalWriterTiDBConfig` 提供目标表名；`LocalEngineConfig` 控制 compact/块大小；`ExternalEngineConfig` 描述外部文件、键范围、切分键、容量与重复键策略；`OnDuplicateKey` 为 `Error`/`Replace`/`Ignore`。

## 执行流程

正常物理导入流程如下：

1. 上层把具体实现包装为 `Arc<dyn Backend>`，调用 `MakeEngineManager`。`pkg/executor/importer/table_import.rs::NewTableImporter` 是生产入口之一。
2. `EngineManager::OpenEngine` 调用 `MakeUUID`，再先执行后端 `OpenEngine`。只有后端成功后才返回 `OpenedEngine`；打开失败不会产生可继续使用的句柄。
3. 上层经 `OpenedEngine::LocalWriter` 获取 writer，调用 `EngineWriter::AppendRows` 写入一批或多批行；必要时 `OpenedEngine::Flush` 将引擎缓存交给后端同步。
4. `OpenedEngine::Close(self, ctx)` 消耗打开句柄，克隆其配置并调用私有 `engine::unsafeClose`；后端 `CloseEngine` 成功后才构造 `ClosedEngine`。所有权转换阻止正常调用路径继续使用已关闭的 `OpenedEngine`。
5. `ClosedEngine::Import` 调用后端 `ImportEngine`。成功立即结束；不可重试错误立即透传；可重试错误记录为 `last`，按 `RetryImportDelay` 阻塞当前线程后重试。三次都失败时返回带 UUID、最大次数和末次错误消息的新 `BackendError`，并保留末次错误的 `retryable`/`duplicate` 标志。
6. `ClosedEngine::Cleanup` 委托后端删除中间状态。`TableImporter::ImportAndCleanup` 即使导入失败仍尝试清理，并在清理成功后从本地清理登记中移除引擎。

断点恢复等场景无法再持有 `OpenedEngine` 时，`UnsafeCloseEngine` 重新计算 tag/UUID，或 `UnsafeCloseEngineWithUUID` 使用已知标识，然后直接调用 `CloseEngine` 取得 `ClosedEngine`；名称中的 “Unsafe” 表示调用者必须自行证明该引擎此前确已打开。

## 数据与状态

`EngineManager` 与私有 `engine` 都克隆 `Arc<dyn Backend>`，因此各句柄共享同一后端实例。`engine` 同时保存不可变的 `Logger`、UUID 和 `i32` ID。`OpenedEngine` 额外保存表名和一份克隆的 `EngineConfig`，以便关闭时把与打开相同的配置交回后端；`ClosedEngine` 则只保留导入/清理所需公共句柄。

状态迁移主要由 Rust 所有权表达：正常的 `OpenedEngine::Close` 接收 `self`，成功后产生 `ClosedEngine`。但这不是全局状态机证明：`EngineManager::UnsafeCloseEngine*` 和 `NewClosedEngine` 能绕过正常迁移，且实际后端仍需检查 UUID 对应资源是否存在。

配置结构均拥有数据而非借用数据，便于跨生命周期保存和克隆。`ExternalEngineConfig` 的 `StartKey`/`EndKey` 定义扫描范围，`JobKeys`/`SplitKeys` 定义任务和 Region 边界；`TotalFileSize`/`TotalKVCount` 是总量信息，`MemCapacity` 是子任务容量预算。`BackendError.duplicate` 只携带分类，本文件的重试控制只读取 `retryable`。

## 依赖与调用关系

上游直接证据包括：

- `pkg/executor/importer/table_import.rs`：`NewTableImporter -> MakeEngineManager`；`OpenIndexEngine`/`OpenDataEngine -> EngineManager::OpenEngine`；`ImportAndCleanup -> ClosedEngine::Import -> ClosedEngine::Cleanup`。
- `pkg/session/runtime/import_query.rs`：创建物理后端后打开数据和索引引擎，经 `ProcessChunk` 写入，再逐个 `Close -> Import -> Cleanup`。
- `pkg/session/runtime/modify_column_backfill.rs`、`pkg/session/runtime/import_file.rs` 也创建 `EngineManager`；`pkg/executor/importer/engine_process.rs` 消费 `OpenedEngine`/`EngineWriter`；`pkg/dxf/importinto/encode_and_sort_operator.rs` 直接依赖 writer/error/status 协议。

下游依赖包括：

- `encode::{Context, Rows}`：上下文和后端无关的行集合协议。
- `uuid::Uuid`：稳定引擎标识。
- `std::sync::Arc`：共享后端对象；`std::thread::sleep` 与 `Duration`：导入重试等待。
- `pkg/session/runtime/import_sst.rs::Backend`：把这里的 UUID 转为本地 `EngineId`，委托实际 local backend 打开、关闭、导入、清理、刷写；其 writer 接受编码后的 `Pairs`/`GroupedPairs`。
- `pkg/lightning/backend/tidb/tidb.rs::tidbBackend`：生命周期操作多为无操作，真正写入发生在 `Writer::AppendRows -> WriteRows`，并使用 `LocalWriterConfig.TiDB.TableName`。

RustCodeGraph 对目标文件识别出 64 个符号，并确认 `MakeEngineManager`、`MakeUUID`、`NewClosedEngine` 与同路径 Go 符号对应；图的精确 callers/callees 命令未返回边，因此上述生产调用关系由已索引源码和定向 `rg` 交叉确认，不把图缺边解释为“没有调用者”。

## 错误处理与边界

- `EngineManager::OpenEngine`、`engine::unsafeClose`、`OpenedEngine::Flush/LocalWriter`、`ClosedEngine::Cleanup` 都用 `?` 原样传播后端错误，不在门面层吞错或补偿。
- `ClosedEngine::Import` 只依据 `BackendError.retryable` 判断重试。不可重试错误只调用一次；测试 `TestImportFailedNoRetry` 验证该边界。
- 可重试错误每次失败后都会调用 `RetryImportDelay` 并 `thread::sleep`，包括第三次（最后一次）失败后仍会 sleep，随后才组装“reach max retry 3”错误。这是当前源码事实，扩展时若调整等待时机必须同步测试和 Go 兼容判断。
- 重试成功立即返回；`TestImportFailedRecovered` 覆盖一次可重试失败后成功。三次均失败时，`TestImportFailedWithRetry` 验证调用次数为 3 且末次错误语义仍为可重试。
- 重试循环固定执行三次，因此 `last.expect(...)` 在当前常量下可达性安全；若未来允许最大次数为 0，必须先消除该假设。
- `BackendError::new`/`retryable` 默认 `duplicate=false`；重复键错误需要具体后端显式构造。Go 版会用 `common.IsRetryableError` 沿错误链分类，而 Rust 版依赖扁平布尔字段，错误转换层必须保留分类。
- 本文件不校验列名与 `Rows` 的列数、键范围顺序、文件列表或配置数值；这些属于具体 writer/backend 的责任。Rust 测试明确允许空列名列表传给 mock writer。

## 并发与资源生命周期

`Backend` 和 `TargetInfoGetter` 要求 `Send + Sync`，与 Go 注释中“后端实例可在任意 goroutine 共享”对应；`EngineManager` 通过 `Arc` 共享后端。`EngineWriter` 只要求 `Send`，方法又需要 `&mut self`，因此可在线程之间转移，但同一 writer 的并发调用需要上层另行同步。

本文件自身不创建后台任务、锁或通道。唯一主动等待是 `ClosedEngine::Import` 的同步 `thread::sleep`，会占用当前线程；取消与超时是否生效取决于后端是否在每次调用中检查 `encode::Context`。例如 `pkg/session/runtime/import_sst.rs::Writer::AppendRows` 检查 `context.cancelled`，但这里的 sleep 本身不可被 context 中断。

资源顺序由调用者负责完成：`Backend::Close` 没有 RAII 保证，writer 也必须显式 `Close`；`OpenedEngine`/`ClosedEngine` 没有 `Drop` 清理。具体 SST 后端在 `pkg/session/runtime/import_sst.rs::Drop` 中额外执行全部引擎清理、关闭本地后端和删除临时目录，但其他实现不一定具备该兜底。安全路径应始终显式关闭 writer、关闭引擎、导入、清理，最后关闭 backend。

## 与 Go 版本的对应关系

总体结构直接对应 `pkg/lightning/backend/backend.go`：固定命名空间、三次导入尝试、`Backend`/`TargetInfoGetter`/`EngineWriter` 接口、管理器和打开/关闭句柄均保留；Rust 独立测试与 `backend_test.go` 使用同名场景验证生命周期和重试意图。

当前可见差异必须在扩展时考虑：

- Go `MakeUUID` 使用 `uuid.NewSHA1(namespace, tag)`；Rust `Uuid::new_v5` 同样使用 SHA-1 的 UUID v5，固定测试值相同。
- Go `Backend` 的线程安全是注释契约；Rust 用 `Send + Sync` 编译期约束。Go `OpenedEngine`/`ClosedEngine` 的 goroutine-safe 注释在 Rust 中主要落到共享 `Backend`，句柄本身没有额外锁。
- Go `OpenEngine`/关闭/导入/清理附带 zap 日志、指标和 failpoint；Rust 门面目前没有指标或 failpoint，`Logger` 只是字段容器。不能据此声称 Rust 已复刻 Go 的可观测性。
- Go 错误分类使用 `common.IsRetryableError` 和错误链，重复键还有特殊日志级别；Rust 使用 `BackendError` 布尔分类，不实现 Go 的日志分级。
- Go 的 `EngineConfig.TableInfo`、外部存储、元数据类型来自完整 TiDB 模型/接口；Rust 使用本 crate 的拥有型简化结构（例如 `ExtStore: Option<String>`），接口形状对应但并非对象能力完全等价。
- Go `EngineWriter::Close` 返回 `common.ChunkFlushStatus` 接口值；Rust 返回 `Option<ChunkFlushStatus>`，允许 TiDB writer 返回 `None`、SST writer 返回 `Some { flushed: true }`。
- Go `OpenedEngine` 保存配置指针；Rust 克隆 `EngineConfig`。因此新增不可轻量克隆或具有外部身份的字段时，需重新评估关闭阶段语义。

## 扩展指南

- 新增后端能力时，先修改 `Backend` 或 `EngineWriter` 的最小必要接口，再同步所有实现（至少检查 `pkg/session/runtime/import_sst.rs` 与 `pkg/lightning/backend/tidb/tidb.rs`）及 `pkg/lightning/backend/backend_test.rs` 中的 `MockBackend`/`MockWriter`。不要把测试实现放回生产源文件。
- 新增引擎配置字段时，明确它属于通用、Local、TiDB 还是 External 分支；检查 `OpenedEngine` 的配置克隆是否保持 Go 指针语义，并同步同路径 Go 对照或明确迁移差异。
- 修改 UUID 标签或命名空间会破坏 checkpoint/恢复定位兼容性；必须保留旧 UUID 兼容路径，并更新 Rust/Go 的固定向量测试。
- 修改导入重试时，应覆盖：立即成功、不可重试只调用一次、失败后恢复、耗尽三次、分类位保留，以及最后一次失败是否等待。还应评估同步 sleep 对运行时线程和取消延迟的性能影响。
- 新增可观测性不能只扩充 `Logger` 字段；应先确认 Rust 上层实际消费方式，并对照 Go 的 open/closed 指标、任务日志和 failpoint，避免表面同名而无真实输出。
- 与真实导入链有关的回归优先扩展独立的 `pkg/lightning/backend/backend_test.rs`，再按具体实现扩展 `pkg/lightning/backend/tidb/tidb_test.rs`、`pkg/session/runtime` 或 `pkg/executor/importer` 的相邻独立测试。

## 验证依据

- 目标源码：`pkg/lightning/backend/backend.rs`（550 行；RustCodeGraph 文件节点报告 64 个符号）。
- crate 边界：`pkg/lightning/backend/Cargo.toml`、`pkg/lightning/backend/lib.rs`；根 workspace 与多个调用 crate 的 Cargo manifest 也引用 `astersql-lightning-backend`。
- Go 对照：`pkg/lightning/backend/backend.go`；Go 测试：`pkg/lightning/backend/backend_test.go`。
- Rust 独立测试：`pkg/lightning/backend/backend_test.rs`，覆盖稳定 UUID、Open/Close/Import/Cleanup、两种 unsafe close、writer 成功/空输入/失败、不可重试、重试耗尽、重试恢复和 backend 关闭。
- 生产调用：`pkg/executor/importer/table_import.rs::{NewTableImporter, OpenIndexEngine, OpenDataEngine, ImportAndCleanup}`，`pkg/session/runtime/import_query.rs` 的引擎闭环。
- 具体实现：`pkg/session/runtime/import_sst.rs` 的 `impl backend::Backend for Backend` 与 writer，`pkg/lightning/backend/tidb/tidb.rs` 的 `impl Backend for tidbBackend`、`impl EngineWriter for Writer`。
- RustCodeGraph：执行了 `status`、目标目录 `files`、目标/Go/测试/调用方/实现文件的 `node`、主要符号 `query`，并尝试核心符号 `callers`/`callees`；索引可用但后两类精确查询无输出，故以已索引源码配合定向文本检索补足直接调用证据。
- 按任务约束未运行 Cargo；本任务只新增说明文档，验证采用固定章节结构检查与人工事实复核。
