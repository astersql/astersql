# `pkg/session/runtime/import_file.rs`

## 文件定位

本文件属于 `astersql-session` crate（见 `pkg/session/Cargo.toml`），由 `pkg/session/runtime.rs` 以私有模块 `mod import_file` 装配，并只公开重导出 `ImportFileTask` 与 `ImportFileSubtask`。它服务于 `ConcreteSession` 这条“规范解析器 + Domain/KV”的同步运行时，而不是完整 planner/executor 会话 ABI；SQL 分派入口位于 `pkg/session/runtime/dispatch.rs`，识别 `ast::ImportIntoStmt`、完成物化视图日志与 TTL 检查后，在无 `SELECT` 数据源时调用 `ConcreteSession::execute_import_file`。

文件处理的是文件型 `IMPORT INTO`。没有 `split_file` 选项时，`execute_import_file` 立即转交 `execute_import_compression`（`pkg/session/runtime/import_compression.rs`）；有该选项时，本文件才负责 CSV 文件切块、DXF 任务/作业记录、Lightning 编码与 SST 导入，以及最终历史状态落盘。

## 核心职责

- 在会话内保存可注入的 `dump::Storage`、区域切分阈值和最近一次物理 SST 导入统计（`ImportFiles`）。存储注入保留 SQL 中的源 URI，便于测试和具体运行时共享同一规划/解析实现。
- 校验这条窄运行时支持的 `split_file` 语法，构造 `dump::DataDivideConfig` 与 `dump::MDTableMeta`，通过 `dump::MakeTableRegions` 按 CSV 行边界产生 `TableRegion`，再按 `engine_id` 组成 engine/chunk 映射。
- 创建 `mysql.tidb_import_jobs` 记录和 DXF `TaskMeta`、task、subtask，将每个 engine 同步执行为“处理 chunk -> 关闭 engine -> Import -> Cleanup”。
- 在成功或失败后同步更新 import job、task/subtask 状态，把 task 转移到历史表，并返回 `Job_ID`、`Imported_Rows` 两列结果。
- 提供查询历史摘要与测试/嵌入方控制接口：`ImportFileTaskByKeyWithHistory`、`SetImportFileStorage`、`SetImportRegionSize` 和 `LastImportSSTStats`。

## 主要符号

- `ImportFileSubtask { step, engine_id, rows, state }`：公开的历史 subtask 摘要。`engine_id` 由持久化 `Ordinal - 1` 还原，`rows` 从 subtask 的 JSON `Summary.row_count` 读取；缺失或无效 JSON 按零处理。
- `ImportFileTask { id, key, state, subtasks }`：公开的历史 task 投影，不暴露 DXF 内部完整元数据。
- `ImportFiles`：`ConcreteSession` 内部状态，定义于本文件并由 `pkg/session/runtime/session.rs` 的 `import_files: RefCell<ImportFiles>` 持有。默认没有存储，`region_size` 为 96 MiB，统计为 `kv::SSTImportStats::default()`。
- `error`：把任何可显示错误压平为 `SessionError` 文本；这条路径不会保留原错误类型/source 链。
- `ConcreteSession::LastImportSSTStats`：克隆返回最近缓存的物理导入统计。
- `ConcreteSession::SetImportFileStorage`：替换对象存储边界。
- `ConcreteSession::SetImportRegionSize`：拒绝非正数，用 `mem::replace` 返回旧值，支持作用域结束时恢复配置。
- `ConcreteSession::ImportFileTaskByKeyWithHistory`：从 DXF 历史表读取 task 和 Import step subtasks，并转换为上述公开摘要。
- `ConcreteSession::execute_import_file`：本文件唯一的 SQL 执行主入口，crate 内可见；其直接上游是 `runtime/dispatch.rs`。

## 执行流程

1. `execute_import_file` 先检查是否存在大小写不敏感的 `split_file` 选项；不存在则走 `execute_import_compression`。存在时拒绝 `SELECT`、列赋值、列/用户变量映射以及非 CSV 格式。
2. 遍历选项，用 `crate::dml_runtime::EvalExpr` 求值。支持 `split_file`、四种 CSV 分隔/包围/转义配置、`skip_rows` 和内部选项 `__max_engine_size`；其他选项立即报错。行终止符不得为空，engine 大小必须为有限正数。
3. 从 `ImportFiles` 克隆 storage 并读取 `region_size`，解析当前 schema/目标表；随后调用 `Storage::list` 精确匹配 `statement.Path` 并取得文件大小。此路径只处理一个精确 URI，不进行 glob 展开。
4. 构造 `DataDivideConfig`：`column_count` 使用目标表列数，`strict_format = true`，并写入区域大小、engine 大小和 CSV 配置。构造单文件 `MDTableMeta` 后调用 `MakeTableRegions`，再按 `TableRegion.engine_id` 分组。
5. 获取 `ImportTaskManager`，以当前进程 ID 构造 executor ID，登记当前节点 CPU 资源；随后在独立 SQL session 中插入 `mysql.tidb_import_jobs`，取得 `last_insert_id`。
6. 生成 `astersql_dxf_importinto::TaskMeta`。`Plan` 固定为 CSV、`SplitFile = true`、`ThreadCnt = 1`，并保存源路径、文件大小、忽略行数、用户和行字段配置；每个 region 被转换为 importer `Chunk`，保留字节范围和估算 row ID 范围。
7. `CreateTask` 后按 engine 建立 Import step subtask，令 `Ordinal = engine_id + 1`，再由 `SwitchTaskStep` 进入 running 状态并读取持久化 subtasks。
8. 清零会话的 SST 统计。对每个 engine：标记 subtask 开始；分别打开 data engine 与共享编号 `importer::IndexEngineID` 的 index engine；逐 chunk 调用 `importer::ProcessChunk`，通过原子 `Progress.rows` 计算行数及最大 row ID；依次关闭、导入、清理两个 engine；复制物理后端统计；非聚簇且非 common handle 表再推进运行时 auto ID；最后更新 subtask 行数并完成 subtask。
9. 根据执行闭包结果把 import job 标为 `finished` 或 `failed`。失败时调用 `FailSubtask` 与 `FailTask`，成功时调用 `SucceedTask`；之后把 imported row 总数写回 `TaskMeta`（仅成功），调用 `TransferTasks2History`。
10. 成功返回一行 `[job_id, count]`；失败在状态和历史迁移完成后重新传播原始导入错误。

## 数据与状态

会话态由 `RefCell<ImportFiles>` 保存，因此 setter 和统计更新使用运行时借用检查；方法在修改前会缩短只读 borrow 的作用域，避免跨 storage/导入调用持有借用。`storage` 是 `Arc<dyn dump::Storage>`，导入 runtime 与会话共享所有权；`sst_stats` 是最近一次 split-file 导入的快照，并非按 job 保存。它在 task/subtask 建立后、实际处理前清零，每完成一个 engine 后从 `physical.stats: Mutex<SSTImportStats>` 克隆，因此失败发生在首个 engine 完成前会留下零值，后续 engine 失败则可能留下部分统计。

持久化状态分两层：`mysql.tidb_import_jobs` 保存面向 SQL 的状态、摘要和错误文本；DXF task/subtask 表保存调度状态、序号、序列化元数据与行数。完成路径会把 active task 移入历史表；`ImportFileTaskByKeyWithHistory` 明确从历史接口读取。`TaskMeta.ChunkMap` 是 engine 到 chunk 列表的映射，chunk 的字节区间由 mydump 切分器保证相邻覆盖，row ID 区间基于目标列数估算。

## 依赖与调用关系

上游链路为 `ConcreteSession::execute`/语句分派 -> `runtime/dispatch.rs` 的 `ImportIntoStmt` 分支 -> `execute_import_file`。`dispatch.rs` 已在调用前拒绝带物化视图日志的目标表、检查 TTL，并将 `IMPORT ... SELECT` 分到 `execute_import_query`。

主要下游依赖如下：

- `astersql-lightning-mydump`：`Storage`、CSV 配置、文件元数据与 `MakeTableRegions`；实际 CSV parser 由 `runtime/import_sst.rs::Runtime::GetParser` 打开，并只在 offset 为零的首 chunk 跳过 `skip_rows`。
- `astersql-executor-importer`：`Plan`、`Chunk`、`ProcessChunk`、KV encoder 和 index engine 常量。
- `astersql-dxf-framework-storage`、`astersql-dxf-framework-proto`、`astersql-dxf-importinto`：持久化 import job/task/subtask、状态转换、历史迁移及元数据编解码。`ImportTaskManager` 的具体构造位于 `runtime/dxf_session.rs`，每次调用建立新的 session pool，表数据则保存在 Domain 的规范 KV storage 中。
- `astersql-lightning-backend` 与 `runtime/import_sst.rs::Backend`：打开/关闭 data/index engine，并把 SST 写入选定 KV store；统计跨线程共享时由互斥锁保护。
- `ConcreteSession` 的表解析、DML flags、auto ID 分配与用户身份字段：提供 schema/table、编码语义以及 `created_by`。

`pkg/session/Cargo.toml` 直接声明了上述 session、DXF、Lightning、executor/importer、KV 和 serde 依赖；本文件没有条件编译项，也不引入额外 feature 门控。

## 错误处理与边界

规划前错误（不支持的列映射/格式/选项、缺少或非法值、未配置 storage、表或源文件不存在、切分失败）直接返回 `SessionError`。其中源文件查询发生在创建 import job 之前，所以“文件不存在”不会产生 failed job；实际 engine/编码/唯一性错误发生在 job/task 创建之后，会进入失败状态收尾。`tests/realtikvtest/importintotest4/split_file_test.rs` 验证了缺失文件返回错误，以及重复导入触发唯一性错误后 job、task 和至少一个 subtask 为 failed，原表仍可通过 `ADMIN CHECK TABLE`。

需要注意的边界：storage 路径必须与 SQL URI 完全相等；只支持 CSV；不支持列映射；并发数固定为 1；不支持完整 Go 路径的 detached、global sort、冲突捕获等选项。`serde_json::from_str(...).unwrap_or_default()` 和字段读取的默认值会把损坏/旧版 subtask summary 表现为零行而非查询错误。`physical.stats.lock().unwrap()` 在锁中毒时会 panic。失败收尾或历史迁移自身若失败，可能取代最初的导入错误返回。engine 的显式 `Cleanup` 只位于每个 engine 成功 `Import` 之后；若 `ProcessChunk`、`Close` 或 `Import` 提前失败，本函数没有补偿式 cleanup，不能把 Go DXF cleaner 的全部保证推断到此同步路径。

## 并发与资源生命周期

本函数自身按 engine、chunk 顺序同步运行，`ThreadCnt` 与 subtask `Concurrency` 都写死为 1；只有底层 SST backend/transport 可能使用线程。`Progress` 以 relaxed 原子计数，适合统计而不承担跨字段同步不变量；`physical.stats` 通过 `Mutex` 读取。`SetNodeResource` 设置的是 DXF 节点资源状态，值来自 `available_parallelism`，不是仅属于当前 job 的局部变量。

data/index engine 的正常生命周期为 `OpenEngine -> ProcessChunk* -> Close -> Import -> Cleanup`。对象存储由 `Arc` 保持到 runtime 与处理结束。DXF manager 通过独立 session/事务访问系统表；测试 `task_history_uses_independent_sql_sessions_and_real_transactions`（同一 split-file 测试文件）验证历史状态跨 `ConcreteSession` 存活及事务回滚。`runtime/dxf_session.rs::Backend::drop` 会关闭 channel 并 join 其 SQL worker；本文件自身不创建异步任务。

## 与 Go 版本的对应关系

Go 没有 `pkg/session/runtime/import_file.go` 一一对应文件；等价职责分散在 `pkg/executor/importer/import.go`、`table_import.go`、`pkg/dxf/importinto/*` 和 `tests/realtikvtest/importintotest4/split_file_test.go`。Rust 复用了相同概念：Go `LoadDataController.PopulateChunks` 同样把目标列数、`SplitFile`、region/engine 大小和 CSV 配置交给 `mydump.MakeTableRegions`；Go 测试同样要求 500 行文件切成 3 个 subtask、`skip_rows = 1` 正确、CRLF 切分后索引一致。

这不是 Go 完整分布式 IMPORT INTO 的逐语句复刻，而是 ConcreteSession 的同步适配层。已确认差异包括：Rust 只支持精确单文件 CSV，固定单线程并在当前调用内执行全部 subtask；Go 的计划和 scheduler 支持更多数据源、选项、节点与清理阶段。Go `import.go` 明确限制 split-file 时 `skip_rows <= 1`，而本文件当前接受任意可解析 `u64`，Rust 回归测试还覆盖了 `skip_rows = 2`；因此不能把两边该边界描述为完全一致。Rust 的 `__max_engine_size` 直接解析为 `f64`，Go 则按 `ByteSize.UnmarshalText` 解析。后续追求完整兼容时，这些差异必须通过独立行为任务与回归测试处理，本说明不修改现状。

## 扩展指南

- 新增 split-file 选项时，先修改 `execute_import_file` 的求值/校验和 `TaskMeta.Plan` 映射；若影响解析或编码，再同步 `runtime/import_sst.rs::Runtime`。同时对照 `pkg/executor/importer/import.go`/`.rs` 的规范计划语义，避免只让窄运行时接受但底层不理解。
- 改变切分策略时，修改 `DataDivideConfig` 构造或 `MakeTableRegions` 上游输入，不要在本文件重新实现 CSV 边界算法；同步验证 chunk 连续性、列数驱动的 row-ID 估算、CRLF 对齐和 engine/subtask 对应关系。
- 改变 task 状态机时，保持 import job 与 DXF task/subtask 的成功、失败、历史迁移一致；重点检查“规划前失败不建 job”“执行后失败可查历史”和收尾错误是否遮蔽根因。
- 改变 engine 生命周期时，优先补齐失败路径的 cleanup/回滚设计，并验证 data 与 index engine 都被处理；这是资源泄漏与磁盘占用的主要风险点。
- 测试应继续放在独立文件，不嵌入生产源码。直接回归面是 `tests/realtikvtest/importintotest4/split_file_test.rs`；非 split-file 压缩分支由 `pkg/session/runtime/import_compression_test.rs` 覆盖。需要 Go 对齐时同步参考同目录 `split_file_test.go`。
- 兼容性风险集中在选项解析、URI 匹配、row ID/auto ID 推进和 task 历史格式；性能风险集中在顺序执行、每 engine 重开共享 index engine、storage 全量 `list()` 后线性查找以及 SST 清理失败。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；`files --filter pkg/session/runtime` 确认目标文件与相邻运行时文件；`node --file pkg/session/runtime/import_file.rs --offset 1/261` 阅读了 466 行全貌；`query execute_import_file`、`query MakeTableRegions`、`query ProcessChunk`、`query ImportTaskManager`、`query LastImportSSTStats` 定位了入口和关键下游。图的 `callers/callees execute_import_file` 未返回边，因此调用点再由文本检索核验，未据此虚构调用关系。
- 源码与装配：`pkg/session/runtime/import_file.rs`、`pkg/session/runtime.rs`、`pkg/session/runtime/dispatch.rs`、`pkg/session/runtime/session.rs`、`pkg/session/runtime/import_sst.rs`、`pkg/session/runtime/dxf_session.rs`、`pkg/session/runtime/import_compression.rs`。
- crate 边界：`pkg/session/Cargo.toml`；目标包范围内未找到 `doc.go`，因此以 `runtime.rs` 顶层模块说明作为最近的运行时契约。
- Go 对照：`pkg/executor/importer/import.go`、`pkg/executor/importer/table_import.go`、`pkg/dxf/importinto/scheduler.go`、`tests/realtikvtest/importintotest4/split_file_test.go`。
- Rust 测试：`tests/realtikvtest/importintotest4/split_file_test.rs` 覆盖 canonical KV/RealTiKV、三段切分、任务历史、skip rows、CRLF、非聚簇主键、缺失文件与失败状态；`pkg/session/runtime/import_compression_test.rs` 覆盖无 `split_file` 的委托分支。按任务约束未运行 Cargo 或测试二进制，只做静态事实与文档结构验证。
