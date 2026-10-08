# `pkg/session/runtime/import_sst.rs`

## 文件定位

本文件位于 `astersql-session` crate 的 session 运行时层，是 session 发起物理 KV 导入时的适配边界。它不负责 SQL 解析、导入任务规划或 SST 排序算法本身，而是把三组既有抽象接起来：`astersql_executor_importer::TableImporterRuntime`、`astersql_lightning_backend::Backend`，以及 `astersql_ingestor_ingestctrl` 的本地/外部 engine 与导入客户端接口。

模块由 `pkg/session/runtime.rs` 以私有 `mod import_sst` 装配，只把 `NewImportLocalBackend` 重新导出。其余 `pub(super)` 类型和方法仅供 session runtime 的相邻模块使用。直接使用点包括文件型 `IMPORT INTO` 的 `pkg/session/runtime/import_file.rs`、查询型导入的 `pkg/session/runtime/import_query.rs`、DDL 本地回填的 `pkg/session/runtime/modify_column_backfill.rs`，以及云端全局排序回填的 `pkg/session/runtime/modify_column_cloud_executor.rs`。

crate 边界由 `pkg/session/Cargo.toml` 确认：本文件直接依赖 session crate 声明的 executor importer、Lightning backend/encode/kv/mydump、ingestor ingestctrl/globalsort、DXF collector、KV、meta model、types 和 `uuid`。这些都是普通依赖，不受 `nextgen` feature 条件控制；本文件自身也没有条件编译项。

## 核心职责

1. `Progress` 把 importer 的字节/行进度回调累计成无锁原子计数，供 `import_file.rs` 统计每个 chunk 和 subtask 的结果。
2. `Runtime` 为文件数据源提供表定义、CSV parser 和 KV encoder，使通用 `ProcessChunk` 能把源文件解析、编码并写入 Lightning engine。
3. `StoreBridge` 把 ingestctrl 的写入请求落到当前 `Domain` 持有的 KV store，同时执行取消传播、keyspace 前缀处理、目标键冲突检查和导入统计累计。
4. `Backend` 把 ingestctrl 的本地 engine 生命周期包装成 Lightning `Backend`，并额外接入 globalsort 的外部 engine，供云端回填直接导入已排序文件。
5. `Writer` 只接受已经编码的 KV rows，并在进入底层 writer 前执行 engine 内重复键检测。

因此，本文件存在的原因是让上层 session 流程复用 importer、Lightning engine 管理和 ingestctrl 的 region/engine 管线，同时仍通过 `Domain::storage()` 使用 AsterSQL 当前选择的实际 KV store，而不是在 session 内重新实现这些子系统。

## 主要符号

- `Progress { rows, read, processed }`：实现 `Collector::Accepted` 与 `Collector::Processed`。三个 `AtomicI64` 均以 `Ordering::Relaxed` 累加；只有 `rows` 对父模块可见。
- `Runtime { table, storage, config, skip_rows, flags }`：文件导入运行时。`GetKVEncoder` 用表元信息、chunk 时间戳和前一最大 RowID 构造 encoder；`GetParser` 打开 CSV parser、恢复 offset/RowID，并只在首个 offset 上跳过配置的头部行；`TakeQueryChunks` 明确拒绝 SELECT 数据源。
- `local_error` / `backend_error`：边界错误适配函数，均保留显示文本，但会归一化为目标子系统的通用错误类型。
- `StoreBridge { domain, options, stats, key_prefix }`：同时实现 `StoreHelper`、`ImportClient` 和 `ImportClientFactory`。所有 factory 实例共享选项、统计与 `Domain`。
- `ImportMonitorStop`：RAII 停止哨兵。离开 `WriteAndIngestData` 作用域时以 Release 顺序置位，保证监控线程不会被早退路径遗留。
- `Backend { local, options, token, seen, stats, directory }`：持有 ingestctrl backend、可热替换的 SST 选项、默认取消 token、每个 UUID 的已见键集合、累计 store 统计和任务临时目录。
- `CloudEngine` / `CloudPool`：把 globalsort `ExternalEngineAdapter` 与 worker pool 调节器转换成 ingestctrl 所需接口，并把可识别的 duplicate/cancelled/closed 错误恢复成 ingestctrl 领域错误。
- `Writer { writer, seen, id, closed }`：Lightning `EngineWriter` 适配器。支持 `encoding::Pairs` 和 `encoding::GroupedPairs`，其他 `Rows` 实现会被拒绝。
- `NewImportLocalBackend(domain, task_id)`：本模块唯一经 `runtime.rs` 对外再导出的构造函数，擦除具体类型为 `Arc<dyn astersql_lightning_backend::Backend>`。

## 执行流程

文件导入路径由 `import_file.rs` 创建 `Backend::new` 与 `Runtime`，再通过 `MakeEngineManager` 打开 data/index engine。每个 chunk 进入 importer `ProcessChunk`：`Runtime::GetParser` 从 `dump::Storage` 打开 CSV，恢复断点并按需跳过头部；`GetKVEncoder` 按表元信息把行转为 KV；writer 最终调用本文件 `Writer::AppendRows`。engine 关闭后，Lightning 门面调用 `Backend::ImportEngine`，转交 ingestctrl 完成分片、写入与 ingest，随后清理 engine。`Progress` 的行数还被用于更新 auto ID 和 DXF subtask 汇总。

查询导入路径在 `import_query.rs` 使用其自己的 `QueryRuntime` 供行数据，但复用本文件的 `Backend`、engine 和 writer 生命周期。因此 `Runtime::TakeQueryChunks` 返回错误不是查询导入整体不受支持，而是明确规定“本文件的 `Runtime` 只代表文件源”。

本地 engine 导入时，`StoreBridge::WriteAndIngest` 先检查取消，再从 engine snapshot 中筛选半开区间 `[start, end)`（空 `end` 表示无上界）的 KV。写入前对 `MaxVersion` snapshot 执行批量查重，只要任一目标键已经存在就返回错误；随后以 engine 元数据中的时间戳调用 `ImportSSTWithOptions`，并把返回统计合并到共享 `stats`。

云端外部 engine 路径由 `modify_column_cloud_executor.rs` 在 write-and-ingest 阶段以 `new_with_key_prefix` 初始化。`register_external` 将 globalsort engine 包装为 `CloudEngine` 并注册到 ingestctrl；`import_external_native` 调用 `ImportEngine`。ingestctrl 再进入 `StoreBridge::WriteAndIngestData`，逐 range 读取外部数据，剥离 session 提供的 keyspace 前缀，检查已有键：已有且值相同允许幂等继续，已有但值不同返回带实际 key/value 的 `Conflict`；之后按外部数据时间戳写入。完成后由 `cleanup_external` 清理注册 engine。

Lightning writer 路径中，`OpenEngine` 先打开底层 engine 并建立 UUID 对应的空 `BTreeSet`；`LocalWriter` 创建底层 writer；`AppendRows` 检查取消和 rows 具体类型，并在持锁期间逐 KV 去重、追加；`Close` 刷闭 writer 并把 `closed` 置为真。`CleanupEngine` 同时删除底层 engine 和 `seen` 条目。

## 数据与状态

`Runtime` 的 `table`、`config`、`storage`、`skip_rows` 与 `flags` 在一次文件导入期间只读。encoder 的 `Timestamp` 取自 chunk，`AutoRandomSeed` 取 `PrevRowIDMax`；parser 的物理位置用 `Offset` 恢复，而逻辑 RowID 在跳过首段行后重置回 `PrevRowIDMax`，避免跳过行污染导入 RowID 序列。

`Backend::new_with_key_prefix` 为每个 backend 创建形如 `astersql-import-<pid>-<task_id>-<uuid>` 的独立临时目录。`concurrency.max(1)` 保证底层 worker 数不为零，`duplicate_detection` 固定开启。`options`、`stats` 和 `seen` 分别由 `Arc<Mutex<_>>` 共享：`set_import_options` 支持相邻 subtask 更换已经取消的 KV context；`stats` 累计 keys、bytes、write RPC 和 ingest RPC；`seen` 以 engine UUID 隔离导入前的内存去重集合。

`StoreBridge::GetTS` 从当前 store 获取全局事务版本，并按 18 位逻辑部分拆成 `(physical, logical)`。`GetTiKVCodec` 固定返回 `v1`；真正的 keyspace 转换由构造时传入的 `key_prefix` 控制。普通 session 导入传空前缀，云回填根据 `DDLKeyspaceID` 构造 v1 或 v2 codec 前缀。

外部 engine 自身持有数据时间戳、key range、边界和统计；`CloudEngine` 不复制这些状态，只转发查询。`CloudPool` 同样只把 ingestctrl 动态并发数传给 globalsort resource handle。

## 依赖与调用关系

上游主链可概括为：

- `import_file.rs` → `Backend::new` / `Runtime` / `Progress` → importer `ProcessChunk` → Lightning `EngineManager` → 本文件 `Writer` / `Backend`。
- `import_query.rs` → `Backend::new` → Lightning engine；查询行提供者是该文件自己的 `QueryRuntime`。
- `modify_column_backfill.rs::ingest_with_options` → `Backend::new_with_options` → `Writer::AppendRows` → 本地 engine 导入。
- `modify_column_cloud_executor.rs::CloudStep::Init` → `Backend::new_with_key_prefix`；后续调用 `set_import_options`、`register_external`、`import_external` 和 `cleanup_external` 管理各 subtask。
- `runtime.rs` → 再导出 `NewImportLocalBackend`，供需要 `dyn backend::Backend` 的宿主/测试接线使用。

主要下游是 `astersql_executor_importer`（chunk 与 encoder）、`astersql_lightning_mydump`（CSV parser/storage）、`astersql_lightning_backend*`（engine 门面与编码 KV）、`astersql_ingestor_ingestctrl`（本地 engine、region job、外部导入管线）、`astersql_ingestor_globalsort`（云端外部 engine），以及 `Domain` 暴露的 KV store（snapshot、版本、`ImportSSTWithOptions`）。

RustCodeGraph 对 `NewImportLocalBackend` 的节点定位为 `import_sst.rs::NewImportLocalBackend`，并显示由 `runtime.rs` 经模块导入关联；精确的跨 trait 动态分派调用边没有全部物化，所以本说明同时以以上相邻调用点和 trait impl 源码为直接证据，不把缺失图边解释为“无人调用”。

## 错误处理与边界

文件 parser 将底层错误转为字符串；取消、非文件查询源、非 KV rows、engine 已关闭、重复键、错误 keyspace 前缀和 poisoned mutex 都有显式错误。`GetParser` 把首段跳行遇到的 EOF 当正常结束，但传播其他读错误。

两条物理写路径的重复语义不同且不可随意合并：本地 engine 的 `WriteAndIngest` 对目标 store 中任意已存在键都报 `duplicate key during physical import`；外部 engine 的 `WriteAndIngestData` 允许目标中同键同值，只有同键异值才返回携带冲突 payload 的 `local::Error::Conflict`。在进入 store 前，`Writer` 还会拒绝同一 open engine 内重复追加的 key。

外部迭代器先保存编码结果，再无条件尝试 `Close`；编码错误优先于 close 错误，编码成功时 close 错误会传播。每个 range 的空结果被跳过。底层 snapshot、写入、前缀和取消错误都在导入结果中返回。

仍有少数有意的 panic 边界：监控线程 `join().unwrap()` 假设内部线程不会 panic；`CloudPool::Tune` 用 `expect` 要求 pool 调节成功；writer 的 `seen.lock().unwrap()` 假设 mutex 未 poisoned。这些是当前事实，扩展时不应把它们误写为可恢复错误路径。`backend_error`/`local_error` 只保留展示文本，可能丢失原错误的结构化类型。

## 并发与资源生命周期

`Progress` 使用 Relaxed 原子操作，适合只需最终/近似累计值而不依赖跨字段顺序的统计。`stats`、`options` 和 `seen` 用互斥锁保证共享修改；其中 `AppendRows` 在整批检查和追加期间持有 `seen` 锁，从而保证同一 backend 内并发 writer 的查重原子性，但批次很大时会形成串行热点。

`WriteAndIngestData` 用 scoped thread 每 1 ms 观察 ingestctrl token。一旦取消，它同时取消 engineapi context 与 `SSTImportOptions.context`，用于唤醒正在读外部文件或等待 store 限速器的操作。`ImportMonitorStop` 和显式 `done.store` 覆盖正常返回与 `?` 早退，随后 join，确保方法返回时监控线程已经结束。`WriteAndIngest` 没有额外线程，依赖入口、读取循环和结束前的 token 检查。

`Backend` drop 时依次清理所有本地 engine、关闭 ingestctrl backend 并尽力删除专属临时目录；删除失败被忽略。显式 `Backend::Close` 只关闭底层 backend，不删除目录，最终仍由 drop 收尾。外部 engine 需要按 `register_external` → `import_external` → `cleanup_external` 配对；测试还证明跨 subtask 重用 backend 前必须用 `set_import_options` 替换已取消 context。

`Writer::IsSynced` 实际报告本 writer 是否已经 `Close`，而非独立查询磁盘同步状态。`Close` 返回 `flushed: true`。调用者仍应遵循 engine writer、engine close、import、cleanup 的完整顺序。

## 与 Go 版本的对应关系

仓库中没有 `pkg/session/runtime/import_sst.go` 的一对一 Go 文件；该 Rust 文件是把原本分散在 Go 子系统中的接口语义接入 Rust session，而不是同路径机械翻译。Go 对照证据主要来自 `pkg/lightning/backend/backend.go` 的 `Backend`/`EngineWriter` 生命周期接口、`pkg/ingestor/ingestctrl/local.go` 的 local backend 构造与 engine 生命周期，以及 `pkg/ingestor/ingestctrl/engine.go` 的 writer 行为。

Rust `Backend` 保留了 Go Lightning backend 的 Open/Close/Import/Cleanup/Flush/LocalWriter 形状，也让上层遵循打开 engine、写 rows、关闭、导入、清理的顺序。区别是 Rust 版本通过 crate 内 trait、`Arc`/`Mutex` 和 `CancellationToken` 组合这些能力，并让 `StoreBridge` 直接调用抽象 KV store，而 Go `ingestctrl.NewBackend` 通常从 PD/TiKV 客户端与 import client factory 构建完整后端。

`Runtime` 对应 Go `pkg/executor/importer/table_import.go` 中 TableImporter 的 parser/encoder/engine 协作意图，但 Rust 将数据源相关能力抽成 `TableImporterRuntime`，由文件源和查询源分别实现。云端 external engine、keyspace 前缀剥离、同键同值幂等和 store 统计是当前 Rust 接线的具体行为，不能仅凭 Go backend 接口推断；应以本文件和独立 Rust 测试为准。

## 扩展指南

新增文件格式或 parser 行为，应优先修改/扩展 importer 与 mydump 抽象，并在 `Runtime::GetParser` 选择实现；不要把格式解析塞进 `StoreBridge`。同时补充独立的 `pkg/session/runtime/import_sst_test.rs` 或 importer 对应测试，覆盖 offset、首段跳行、RowID 和取消。

新增 rows 编码容器时，需要同步 `Writer::AppendRows` 的 downcast 分支，并确认重复键语义、批次锁范围和 Lightning `Rows` 接口兼容。测试逻辑必须继续放在独立测试文件，不能内嵌到本生产文件。

修改物理写入时，应分别审查 `WriteAndIngest` 与 `WriteAndIngestData`：前者面向本地 engine，后者面向 globalsort external engine，现有目标键冲突规则不同。任何 keyspace 改动都必须验证前缀只剥离一次、错误 keyspace 被拒绝、写入 snapshot 可按 external engine 时间戳读取。

新增可变导入控制项时，沿用 `set_import_options` 的共享更新边界，并检查正在运行与下一 subtask 的可见性；若要求无竞态快照，保持“每次写入开始时 clone options”的约束。调整取消桥时必须保证限速等待和外部 reader 都能被唤醒，且所有早退路径 join 监控线程。

修改 engine 生命周期或清理策略时，同步检查 `Drop for Backend`、`CleanupEngine`、`seen` 条目和临时目录。性能风险集中在全批次 KV 收集、snapshot `BatchGet`、writer 全批持锁、1 ms 取消轮询与 `BTreeSet` 内存占用；正确性风险集中在时间戳、keyspace、重复键和 option context 的跨 subtask 生命周期。

## 验证依据

- 主源码：`pkg/session/runtime/import_sst.rs`，逐项核对 `Progress`、`Runtime`、`StoreBridge`、`ImportMonitorStop`、`Backend`、`CloudEngine`、`CloudPool`、`Writer` 及其 trait impl。
- 模块与上游：`pkg/session/runtime.rs`、`pkg/session/runtime/import_file.rs`、`pkg/session/runtime/import_query.rs`、`pkg/session/runtime/modify_column_backfill.rs`、`pkg/session/runtime/modify_column_cloud_executor.rs`。
- crate 声明：`pkg/session/Cargo.toml` 的 package、feature 与相关 path dependencies。
- 独立 Rust 测试：`pkg/session/runtime/import_sst_test.rs`。四个测试分别覆盖运行中限速更新、framework 取消唤醒物理写限速器、带 keyspace 前缀的 external engine 按 subtask 时间戳导入及 context 更新、duplicate key/value 的结构化冲突保留。
- Go 语义参照：`pkg/lightning/backend/backend.go`、`pkg/ingestor/ingestctrl/local.go`、`pkg/ingestor/ingestctrl/engine.go`、`pkg/executor/importer/table_import.go`；仓库内未发现同路径 `pkg/session/runtime/import_sst.go`。
- RustCodeGraph：`status` 报告索引包含 11,467 个文件；`query NewImportLocalBackend --kind function` 定位到本文件第 514 行；`node import_sst.rs::NewImportLocalBackend` 给出源码及 `runtime.rs` 模块导入轨迹；对图未覆盖的 trait 动态分派与 Cargo/Go 内容使用 `rg` 和直接文件读取核验。
- 本任务是纯文档分析，未运行 Cargo。结构验收要求目标文档存在，且固定的十一个二级标题各出现一次；人工复核重点是文件为何存在、三条导入路径如何运行、重复/取消/keyspace 边界以及安全扩展位置。
