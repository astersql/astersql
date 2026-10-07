# `lightning/pkg/importer/import.rs`

## 文件定位

本文件是 `astersql-lightning-pkg-importer` crate 的顶层导入编排模块。crate 根在 `lightning/pkg/importer/lib.rs` 中以 `#[path = "import.rs"] mod import; pub use import::*;` 暴露这里的 API；`lightning/pkg/importer/Cargo.toml` 将该 crate 标为 Go 包 `lightning/pkg/importer` 的 Rust library 移植。上层真实入口是 `lightning/pkg/server/lightning.rs` 的 `LegacyImporter::Run`：它把 server 配置转换成 `ControllerParam`，依次调用 `NewImportController`、`Controller::Run` 和 `Controller::Close`。因此，本文件连接 Lightning server 与 schema 恢复、前置检查、checkpoint、逐表导入、压缩和清理等子模块。

该文件不是薄门面：它定义控制器状态、任务主链、local/TiDB backend 分支、checkpoint 兼容性、扩展列和列过滤等逻辑。不过当前 Rust 迁移仍有明确占位：`restoreSchema` 仅从 dump 元数据补齐最小 `dbInfos`，`loadDesiredTableInfos`、`estimateChunkCountIntoMetrics`、`doCompact` 基本为空，`enforceDiskQuota` 只维护原子状态，`cleanCheckpoints` 尚未真正移动或删除 checkpoint。阅读时不能把对应 Go 文件的完整能力视为 Rust 已实现能力。

## 核心职责

1. 构造并持有导入任务资源。`NewImportController` 注入全局 `DeliverPauser`，`NewImportControllerWithPauser` 根据 backend 和并行导入配置选择 meta manager，并初始化 worker pool、backend、checkpoint DB、错误管理器和状态机。
2. 编排一次完整任务。`Controller::Run` 固定执行 `setGlobalVariables → restoreSchema → preCheckRequirements → initCheckpoint → importTables → fullCompact → cleanCheckpoints`，同时广播任务开始、初始化进度、表 checkpoint、错误及任务结束事件。
3. 执行 local backend 的元数据生命周期。`importTables` 必要时创建禁用 GC 的 TiKV store，借用其 PD/codec 信息创建 namespaced etcd client，并用 `registerTaskToPD` 注册 Lightning 后台任务；RAII guard 在所有返回路径关闭注册、client 和自建 store。
4. 管理错误与 checkpoint 辅助数据。`errorSummaries` 按表保存最后一个错误；`verifyCheckpoint` 检查恢复兼容性；`saveCheckpoint` 将 chunk 合并信息放入待保存队列。
5. 维持数据列契约。`addExtendDataForCheckpoint` 从文件名和 route extractor 生成扩展列；`filterColumns` 组合显式表头、忽略列、隐藏列及扩展列，并生成对应字符串 `Datum`。
6. 管理资源和互斥状态。`Close` 幂等关闭 backend、engine manager、可选外部存储、DB 和 task manager；原子状态防止压缩或磁盘配额检查重复进入。

## 主要符号

- 常量 `FullLevelCompact`、`Level1Compact`：传给压缩逻辑的层级标识；当前 Rust 的 `doCompact` 尚未使用参数。`compactStateIdle/Doing` 与 `diskQuotaStateIdle/Checking/Importing` 是两个独立原子状态机。
- 静态量 `DeliverPauser`：通过 `once_pauser::Lazy` 和 `OnceLock<Pauser>` 延迟创建的进程级 pauser。`NewImportController` 会覆盖调用参数中的 pauser；需要注入专用 pauser 时使用 `NewImportControllerWithPauser`。
- `saveCp`：一条待保存 checkpoint 消息，含表名、可选 `TableCheckpointMerger` 和可选完成通知 sender。当前监听器只清空队列，未执行 Go 版的异步持久化协议。
- `errorSummary` / `errorSummaries`：表级错误及其 checkpoint 状态；内部 `Mutex<HashMap<...>>` 允许并发记录，`record` 对同一表执行覆盖，`emitLog` 输出汇总。
- `MetadataCleanup`：包装一次性清理闭包，`Drop` 时执行。`LocalMetadataResources` 持有 namespaced etcd client 和可选自建 TiKV store，`Drop` 时关闭二者并将关闭错误写到标准错误。
- `Controller`：核心状态聚合体。它持有配置、dump/目标表元数据、各类 worker pool、backend/engine、SQL/PD/storage 句柄、checkpoint/error/meta manager、进度状态、原子门闩以及 local backend 所需的 keyspace/API/resource group 信息。
- `LightningStatus`：对调用者公开完成字节数和总字节数；字段是原子整数。Rust 当前主要在构造和 server bridge 中传递，尚未复刻 Go 版全部更新点。
- `ControllerParam`：构造输入，包括 dump 元数据、状态、dump/checkpoint storage、pauser、DB、重复键指示器及 keyspace/resource-group/task-type。
- 构造与生命周期 API：`NewImportController`、`NewImportControllerWithPauser`、`Controller::{Run, Close, Pause, Resume}`。
- 主步骤：`restoreSchema`、`preCheckRequirements`、`DataCheck`、`initCheckpoint`、`importTables`、`fullCompact`、`cleanCheckpoints`。
- local backend 边界：`newEtcdClientForLocalBackend`、`registerTaskToPD`。
- 纯辅助函数：`firstErr`、`verifyCheckpoint`、`verifyLocalFile`、`isLocalBackend`、`isTiDBBackend`、`addExtendDataForCheckpoint`、`saveCheckpoint`、`filterColumns`、`initGlobalConfig`，以及私有转换函数 `checkpoints_config_from`、`progress_mydump_db`。
- `deliveredKVs` / `deliverResult`：chunk 投递过程使用的数据载体；前者携带 KV、文件偏移和 row ID，后者携带可选错误。
- 本文件没有 `trait` 定义，也没有 `#[cfg(...)]` 条件编译项；主要抽象均来自相邻模块的 trait object。

## 执行流程

构造阶段：

1. `NewImportController` 将 `ControllerParam.Pauser` 替换为全局 `DeliverPauser`，转交 `NewImportControllerWithPauser`。
2. 后者选取调用方 pauser 或新建 pauser，创建检查模板和错误汇总。
3. local backend 且开启 `ParallelImport` 时使用 `dbMetaMgrBuilder`；local 非并行使用 `singleMgrBuilder`；其他 backend 使用 `noopMetaMgrBuilder`。
4. 若提供 DB，则构建 errormanager 配置；随后按配置并发度创建 table/index/region/io worker，创建单 worker 的 checksum pool，并填入默认 local backend、空 checkpoint DB、noop encoder/mode switcher 等当前移植边界。

运行阶段由 `Controller::Run` 严格短路：任一步返回错误，后续步骤不执行，但结束广播和 `outputErrorSummary` 始终执行。

1. `setGlobalVariables` 在存在 DB 时调用 `tidb::ObtainImportantVariables`，参数指示是否为 TiDB backend。
2. `restoreSchema` 仅在 `dbInfos` 为空时按 `dbMetas` 建立数据库条目；它没有执行 Go 版的 schema importer、远端结构读取和 local backend DB ID 回填。
3. `preCheckRequirements` 先执行 `DataCheck`。开启 requirements 时再检查集群可用性和自有 storage 权限；随后总是初始化 meta manager。local backend 还检查本地/集群资源、CDC/PiTR 和 PD/TiDB 同集群，唯独 `checkClusterRegion` 的错误被显式忽略。最后以 `checkTemplate.Success()` 汇总校验结果。
4. `initCheckpoint` 调用 checkpoint DB 的 `Initialize`，配置由 `checkpoints_config_from` 投影；随后把 dump 数据库/表和总大小映射为 progress crate 的类型并广播初始化进度。
5. `importTables` 对 local backend 建立元数据资源与 PD 注册。之后逐数据库、逐表串行补齐最小 `DBInfo/TableInfo`，构造 `TableImporter`，以默认 checkpoint 调用 `importTable`，并广播表 checkpoint。任一表失败立即返回。
6. `fullCompact` 用 compare-and-swap 保证同一时刻至多一个调用进入 `doCompact`，并在返回前恢复 idle；当前 `doCompact` 是空成功，因此尚无真实 TiKV compact。
7. `cleanCheckpoints` 当前仅检查配置并返回；还没有 Go 版等待异步写入完成、rename/remove checkpoint 的行为。

local backend 的资源顺序尤其重要：`importTables` 先解析 PD 地址，必要时创建禁用 GC、带 keyspace/TLS 的 `TikvStore`；再借用 store 创建 namespaced etcd client；然后注册 Lightning task。函数退出时 `_registration` 先于 `_local_metadata` 逆序析构，先注销任务并关闭注册 client，再关闭本地元数据 client 和自建 store。

## 数据与状态

- `dbMetas` 是输入 dump 视图，`dbInfos` 是目标结构视图。Rust 当前在 `restoreSchema`/`importTables` 中按名称惰性补齐，并为新表赋固定 `ID: 1` 和 `StatePublic` 的最小 `model::TableInfo`；这不是完整远端 schema 探测。
- worker pool 并发度来自 `cfg.App.TableConcurrency`、`IndexConcurrency`、`RegionConcurrency`，并用 `.max(1)` 避免零大小；checksum 固定为 1。当前 `importTables` 自身逐表串行，真正的 chunk/engine 并发由 `TableImporter` 及相邻模块消费这些池。
- `saveCpCh` 在 Rust 是 `Mutex<Vec<saveCp>>`，生产者 `saveCheckpoint` push，消费者 `listenCheckpointUpdates` 直接清空；Go 版则是 channel、wait group 和异步持久化器。扩展这里必须先决定是否恢复 Go 的确认与错误传播协议。
- `diskQuotaState` 和 `compactState` 都通过 `CompareAndSwap` 防重入。`fullCompact` 在同步调用结束后复位；`enforceDiskQuota` 当前立即复位，没有后台任务、writer lock 或 engine 导入。
- `closed` 使 `Close` 幂等；`ownStore` 决定是否关闭 dump storage，防止释放调用方拥有的句柄。
- `checkTemplate` 累积前置检查结果；`errorSummaries.summary` 以表名为 key，只保留每表最后一次记录。`saveStatusCheckpoint` 在传入错误时同时记录汇总并广播错误，成功路径当前不更新 checkpoint。
- `filterColumns` 的不变量是扩展列总被追加在返回列尾部，扩展值按相同顺序转换为字符串 Datum。没有输入 header、但存在忽略列或扩展列时，会从目标 schema 构造显式列列表，并排除 hidden、被忽略及将由扩展值提供的列。

## 依赖与调用关系

上游调用关系：

- `lightning/pkg/server/lightning.rs::LegacyImporter::Run` 是生产主调用者：创建 `ControllerParam`，调用 `NewImportController → Controller::Run → Controller::Close`。`newImporter` 还会先构造并关闭一个 probe controller，验证 local/TiDB backend 是否可构造。
- `lightning/pkg/importer/meta_service_group_test.rs::controller` 直接调用 `NewImportControllerWithPauser`，验证 meta service group 行为。
- `lightning/pkg/importer/chunk_process.rs` 调用 `saveCheckpoint`，并使用 `Controller`、`deliveredKVs`、`deliverResult`；`dup_detect.rs` 调用 `filterColumns`。
- RustCodeGraph 将本文件列为被 `meta_service_group_test.rs` 和 `precheck_impl.rs` 使用；对精确 Rust 符号执行 `callers/callees` 返回空边，因此上述跨文件边又以源码引用搜索核验，不能把空图误解成“没有调用者”。

下游依赖关系：

- `table_import::{NewTableImporter, TableImporter}` 承担单表导入；`Controller::importTables` 是其任务级上游。
- `meta_manager` 提供不同 backend/并行模式的 meta manager builder；`precheck` 与 `precheck_impl.rs` 提供 `DataCheck` 中调用的具体检查方法。
- checkpoints、progress、errormanager 是独立 workspace crate；Cargo 还声明 metaservice、store driver、BR utils、router 等路径依赖，以及带 tag 的 parquet Git 依赖。
- `astersql_metaservice`、`astersql_store_driver` 和 `astersql_br_pkg_utils::register` 构成 local backend 的 TiKV store、etcd namespace 和任务注册边界。
- `regexpr-router` 从文件名中的 schema/table 及 `SourceID` 提取扩展列；`types::NewStringDatum` 把提取值送入行编码路径。

RustCodeGraph 对 Go/Rust 同名符号会返回两组节点，例如 `NewImportControllerWithPauser`、`verifyCheckpoint`、`filterColumns`；使用时必须按 `filePath` 区分。索引显示 `import.rs` 共 1207 行、95 个符号，索引状态为 11467 文件/307296 节点。

## 错误处理与边界

- `Run` 使用 `?` 保留第一个阶段错误，仍将该错误转换为 progress error 并执行结束广播、错误汇总；它不在内部调用 `Close`，资源释放由上层 `LegacyImporter::Run` 保证。
- `NewImportControllerWithPauser` 中 `param.DB` 缺失时使用内存 DB 创建并行 meta builder；error manager 则只有在 DB 存在时创建。当前构造器没有复刻 Go 版打开/读取/验证 checkpoint、TLS/PD/backend 初始化失败回滚等完整路径。
- `preCheckRequirements` 对大多数错误 fail-fast；`checkClusterRegion` 是例外，其结果被 `.ok()` 丢弃。模板失败以 `FailedMsg()` 合成普通错误。
- `verifyCheckpoint` 无条件检查 backend；仅在 `CheckRequirements` 开启时检查 Lightning 版本、源目录，以及 local backend 的 sorted-KV 目录。文件 checkpoint 的修复提示包含具体 DSN；其他 driver 提示销毁全部 checkpoint。它不把 TaskID 作为兼容条件，Rust 测试明确覆盖这一点。
- `verifyLocalFile` 目前只拒绝空目录；Go 版会枚举 checkpoint 中仍在本地保存的 engine 并检查 SST 文件存在性，因此 Rust 的保护范围明显较窄。
- `addExtendDataForCheckpoint` 在所有 route 都没有 extractor 时直接返回；否则 router 创建失败、路径没有合法 UTF-8 文件名或不能解析出非空 schema/table 都返回错误。它当前一次处理单个 `ChunkCheckpoint`，而 Go 版遍历 `TableCheckpoint` 下所有 engine/chunk，并使用默认 file router 解析文件名。
- 多处锁使用 `.unwrap()`：`errorSummaries`、`saveCpCh`、注册清理闭包若遇 poisoned mutex 会 panic。`importTables` 中 metadata store 在成功打开后按逻辑必然存在，因此使用 `expect`；若该不变量被未来重构破坏会 panic。
- `Close`、RAII 清理和 error manager 输出中的部分关闭/输出错误被忽略或仅写日志，符合“主导入结果优先”的边界，但会降低清理失败的可观测性。

## 并发与资源生命周期

- `DeliverPauser` 由 `OnceLock` 保证全局只初始化一次；`Pause`/`Resume` 只转发到 pauser，且返回成功。
- `errorSummaries` 和 checkpoint 队列分别由独立 mutex 保护；原子整数用于压缩、磁盘配额和进度状态。文件自身没有启动线程，worker pool 的执行由下游模块触发。
- `fullCompact` 的 CAS 是非阻塞互斥：竞争者直接成功返回，不等待当前压缩完成。这与 Go 版循环等待已有 level-1 compact 完成不同。
- local metadata 资源采用栈上 RAII。若 `newEtcdClientForLocalBackend` 失败，代码显式关闭刚创建的 store；成功后由 `LocalMetadataResources::drop` 关闭。任务注册成功后由 `MetadataCleanup::drop` 恢复注册状态，即使逐表导入中途 `?` 返回也执行。
- `registerTaskToPD` 把 register 放进 `Mutex<Option<_>>`，清理闭包通过 `take()` 保证注销逻辑最多执行一次；闭包随后关闭共享 client。
- `Controller::Close` 的 `closed` 标记保证幂等，但类型未实现 `Drop`，调用者若漏掉显式 `Close`，controller 所管理的逻辑关闭动作不会自动全部发生。server bridge 已显式遵守 Run 后 Close 的顺序。
- Go 版 checkpoint listener 有 channel 与 wait group，磁盘配额检查有 goroutine、RWMutex 和周期重试；当前 Rust 对应逻辑没有异步生命周期，文档和新增功能不得假定这些机制已移植。

## 与 Go 版本的对应关系

`lightning/pkg/importer/import.go` 是直接语义来源，Rust 保留了大量 Go 风格符号名和主流程顺序。已对齐的可观察行为包括：全局 pauser 注入；backend/requirements 分支；错误按表汇总；checkpoint backend/版本/源目录/sorted-KV 兼容检查；route extractor 产生扩展列；`filterColumns` 对 header、ignore、hidden 和 extend 列的组合规则；TLS 字段写入全局配置；server 端构造、运行、关闭的生命周期。

主要差异和迁移缺口如下：

- Go 构造器打开 checkpoint、读取并验证已有任务、初始化 error manager/TLS/PD/backend/pre-info/check builder，并在失败时回滚资源；Rust 构造器大多使用内存或 noop 实现。
- Go `restoreSchema` 真正执行 schema importer、读取目标表结构、回填 local backend 数据库 ID 和系统变量；Rust 只创建最小映射。
- Go `initCheckpoint` 可加载 desired table info、启动 checkpoint goroutine 并估算 chunk；Rust 只初始化 DB 并广播 progress。
- Go `importTables` 管理 PD scheduler、GC、checksum manager、并发导入和 post-process；Rust 当前逐表串行调用 `TableImporter`，但已接入 local metadata store 和 task registration 的真实生命周期。
- Go `fullCompact/doCompact` 遍历 TiKV store 发起压缩，`enforceDiskQuota` 会阻塞 writer、flush engine 并导入大 engine；Rust 对应实现仍为空或只维护状态。
- Go `cleanCheckpoints` 等待异步写入并按配置 rename/remove；Rust 尚未执行清理。
- Go `addExtendDataForCheckpoint` 输入整张表 checkpoint 并遍历所有 chunks；Rust 输入单一 chunk，且以点号切分文件名，覆盖面更窄。

独立 Rust 测试 `lightning/pkg/importer/import_test.rs` 与 Go 测试 `import_test.go` 都覆盖 error summary、checkpoint 校验、预检失败、扩展列、列过滤和全局 TLS 配置。Rust 测试还明确断言 TaskID 不参与 checkpoint 兼容判断。不要以 Rust 测试名称与 Go 对齐为理由推断未断言的完整等价性。

## 扩展指南

- 扩展任务阶段时，修改 `Controller::Run` 并保持结束广播和 `outputErrorSummary` 的 finally-like 行为；同步在独立的 `import_test.rs` 增加“前一步失败时后一步不执行”和错误广播断言。
- 补齐构造器时，优先围绕 `NewImportControllerWithPauser` 恢复 checkpoint/TLS/PD/backend/pre-info 初始化及失败回滚，不要把逻辑塞进 server bridge。需要与 Go `NewImportControllerWithPauser` 逐分支比对，并覆盖 local、TiDB、未知 backend 和已有 checkpoint。
- 补齐 schema/checkpoint 时，修改 `restoreSchema`、`initCheckpoint`、`loadDesiredTableInfos`、`estimateChunkCountIntoMetrics`、`listenCheckpointUpdates`、`cleanCheckpoints`；保留测试在独立 `import_test.rs` 或 checkpoint crate 测试文件中，不能内嵌到生产源文件。
- 补齐 local backend 导入时，以 `importTables` 为任务级接入点，以 `table_import.rs`/`chunk_process.rs` 为表和 chunk 级实现点。特别检查 scheduler、GC、checksum、PD 注册的逆序清理和取消路径。
- 补齐配额/压缩时，修改 `enforceDiskQuota`、`fullCompact`、`doCompact`，同步 Go 的锁定范围、后台任务退出、失败重试与状态复位；性能风险集中在 writer 全局阻塞、engine flush/import 和对全部 store 的 RPC。
- 改 checkpoint 数据结构时，同时检查 `saveCp`、`saveCheckpoint` 和 `chunk_process.rs` 的两个调用点；若恢复异步 channel，必须定义 sender 关闭、listener drain、确认消息和 `Close/cleanCheckpoints` 等待顺序。
- 改列映射时，同时更新 `addExtendDataForCheckpoint`、`filterColumns`、`dup_detect.rs` 和 `chunk_process_test.rs`/`import_test.rs`，覆盖大小写、hidden 列、空 header、ignore 与 extend 重名、非法路径；兼容风险是列与 Datum 错位导致静默写错数据。
- 新增 Controller 资源字段时，必须同时更新构造器、`Close` 或 RAII guard、server bridge 参数映射及测试中手工构造的 `Controller`。保持 Rust 源文件与测试文件分离。

## 验证依据

- RustCodeGraph 状态：项目索引存在且可用，包含 11467 个文件、307296 个节点、1848419 条边；`files --filter lightning/pkg/importer` 确认目标、Go 对照和独立测试均已索引。
- RustCodeGraph 源码读取：完整读取 `lightning/pkg/importer/import.rs` 第 1–1207 行；读取生产入口 `lightning/pkg/server/lightning.rs` 第 1438–1657 行；读取 Rust 测试 `lightning/pkg/importer/import_test.rs` 和 Go 对照测试 `import_test.go` 的相关测试。
- RustCodeGraph 符号查询：核对 `NewImportController`、`NewImportControllerWithPauser`、`importTables`、`preCheckRequirements`、`enforceDiskQuota`、`verifyCheckpoint`、`addExtendDataForCheckpoint`、`filterColumns` 的 Go/Rust 节点。精确 Rust `callers/callees` 查询未返回边，故调用关系另以已索引文件的使用者信息和源码引用搜索交叉核对。
- 配置与模块证据：读取 `lightning/pkg/importer/Cargo.toml` 和 `lightning/pkg/importer/lib.rs`，确认 crate 类型、模块导出、workspace/path/Git 依赖及测试模块边界；目标包没有 `doc.go`。
- Go 对照：读取 `lightning/pkg/importer/import.go` 中 Controller/ControllerParam、构造器、schema/checkpoint、importTables、扩展列、压缩、磁盘配额、预检、列过滤和 TLS 全局配置对应段落。
- 测试证据：`lightning/pkg/importer/import_test.rs` 覆盖错误汇总、checkpoint 条件、预检短路、扩展列、列过滤、全局配置；`chunk_process_test.rs`、`table_import_test.rs`、`parity_test.rs` 和 `meta_service_group_test.rs` 提供直接调用证据；Go 侧 `import_test.go` 提供原测试意图。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定 shell 命令验证本文档存在且恰有 11 个固定二级标题，并人工复核所有“已实现/未实现”判断均对应上述源码证据。
