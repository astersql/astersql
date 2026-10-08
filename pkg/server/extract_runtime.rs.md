# `pkg/server/extract_runtime.rs`

## 文件定位

`pkg/server/extract_runtime.rs` 是 `astersql-server` crate 内部的生产 Extract 运行时适配层。模块由 `pkg/server/lib.rs` 以私有 `mod extract_runtime` 装配，只把 `pub(crate) struct CanonicalExtractRuntime` 暴露给同一 crate；它不是 Extract 算法本身，也不是 HTTP 参数解析器，而是把真实 `astersql_domain::Domain`、语句摘要、统计信息、内部 SQL 会话和全局外部存储接到 `astersql_server_handler_extractorhandler::extractor::ExtractRuntime` 接口上。

生产接线位于 `pkg/server/runtime.rs`：`CanonicalServerDomain::new` 创建一个共享的 `Arc<CanonicalExtractRuntime>`，其 `ServerDomain::extract_runtime` 实现再以 `Arc<dyn ExtractRuntime>` 返回。`pkg/server/extract.rs::ExtractTaskServeHandler::new` 从 Server Domain 取得该对象，交给 handler crate 的 `NewExtractTaskServeHandler`，因此完整入口链是“status HTTP 路由 → handler → 本文件运行时 → `pkg/domain/extract.rs::ExtractHandle`”。

`pkg/server/Cargo.toml` 将本文件归入包名 `astersql-server`、库入口 `lib.rs`；本文件直接使用的内部 crate 包括 `astersql-domain`、`astersql-session`、`astersql-planner-extstore`、`astersql-statistics(-handle)`、`astersql-infoschema`、`astersql-meta-model`、`astersql-util-stmtsummary`、`astersql-util-plancodec`、`astersql-util-printer` 和 extractor handler crate，序列化与时间处理还依赖 `base64`、`chrono`、`serde_json`、`toml`。

## 核心职责

1. `CanonicalExtractRuntime` 实现 HTTP handler 所需的时钟、时间解析、任务提交、产物目录、产物打开和日志边界；它把 handler 层 `ExtractTask` 转换为 domain 层 `DomainExtractTask`。
2. `ProductionExtractSource` 实现 `pkg/domain/extract.rs::ExtractSource`，向通用 `ExtractHandle` 提供真实语句摘要、表/视图元数据、binary plan 解码、持久化摘要开关和归档落盘能力。
3. `table_stats_json`、`physical_stats_json` 及三个 sketch/histogram helper 将 Domain 中的内存统计对象编码为与计划回放归档兼容的 JSON。
4. `dump_package` 组装 replay archive：写入元信息、配置、变量、绑定、schema/view DDL、TiFlash 副本、统计和 SQL 记录，然后一次编码并写到全局外部存储的 `extract/` 目录。
5. `StorageExtractReader` 将对象存储 reader 适配为 handler 的流式 `ExtractReader`，使下载路径不必一次把整个 zip 读入响应内存。

记录筛选、digest 去重、截断 SQL 标记、视图依赖递归和串行互斥不在本文件重复实现，而由 `pkg/domain/extract.rs::ExtractHandle::extract_task` 负责；本文件通过 `ExtractHandle::new_with_domain` 启用 Domain AST 包装器，因此 `ProductionExtractSource::view_dependencies` 的显式报错不会成为生产路径的最终视图解析实现。

## 主要符号

- `EXTRACT_DIRECTORY: &str = "extract"`：产物根目录，同时由 `extract_task_directory` 返回并由 `dump_package` 拼接写入路径。
- `sql_identifier(&str) -> String`：用反引号包围 SQL 标识符，并把内部反引号翻倍；用于 `SHOW CREATE TABLE`，避免库表名破坏 SQL 结构。
- `sql_rows(&Arc<Domain>, &str) -> Result<Vec<Vec<String>>, String>`：为一条内部 SQL 新建 `ConcreteSession`，依次拉取所有 record set 的所有行，并显式关闭每个 record set。
- `stats_histogram`：编码 NDV 和 bucket；bucket 的二进制上下界使用标准 Base64，空上下界不输出相应字段。
- `stats_sketch`：把非空 TopN 编码成 `cm_sketch` 形状；空 TopN 返回 `None`。
- `stats_fm_sketch`：空字节返回 `Ok(None)`；否则经 `DecodeFMSketch` 与 `FMSketchToProto` 输出 mask/hashset，解码失败返回错误。
- `table_stats_json`：定位逻辑表。分区表逐个导出已有物理统计，并在存在时加入 `global`；顶层保留空 columns/indices、零计数和 partitions map。非分区表直接序列化 `physical_stats_json`。
- `physical_stats_json`：按 `TableInfo.Columns`/`Indices` 与物理统计 ID 对齐，只输出实际存在的统计项；列支持 histogram、TopN、FM sketch，索引支持 histogram、TopN，并输出 count、modify_count、version 等表级字段。
- `ProductionExtractSource { domain: Arc<Domain> }`：生产数据源。`statement_records` 从全局 `StmtSummaryByDigestMap` 快照读取窗口；`table` 查询当前 Domain 元数据；`decode_binary_plan` 使用 plan codec；`dump_package` 负责归档；`persistent_statement_summary_enabled` 通过内部 SQL读取系统变量。
- `StorageExtractReader(Box<dyn ObjectReader>)`：对象存储 reader 的薄适配，`read` 和 `close` 都把底层错误转为 `ExtractError`。
- `CanonicalExtractRuntime { handle: ExtractHandle }`：本文件唯一 crate 可见类型。`new` 用 `ProductionExtractSource` 和 `ExtractHandle::new_with_domain` 建立生产句柄；其余方法实现 `ExtractRuntime` 契约。

RustCodeGraph 对该文件识别出 40 个符号；主要可见入口为 `CanonicalExtractRuntime::new`，上游直接调用者是 `pkg/server/runtime.rs::CanonicalServerDomain::new`。图中还确认 `ExtractHandle::new_with_domain` 的调用者包含本文件的 `new`，而 `ExtractHandle::extract_task` 下游到 `dump_package`。

## 执行流程

1. `CanonicalServerDomain::new` 用共享 `Arc<Domain>` 构造 `CanonicalExtractRuntime`；构造器创建 `ProductionExtractSource`，再用 `ExtractHandle::new_with_domain` 包装。Domain 包装器会从当前 InfoSchema 读取视图定义、用真实 parser AST 递归展开嵌套视图并防止循环。
2. HTTP handler 在 `pkg/server/handler/extractorhandler/extractor.rs` 中解析 `type/begin/end/is_dump/is_skip_stats/is_history_view`。提交时调用本文件 `extract_task`；若 `RequestContext.cancelled` 已为真，立即返回 `extract task canceled`。
3. `extract_task` 把秒级 `Timestamp` 转成 `SystemTime`。负数以 `max(0)` 钳到 Unix epoch，然后复制 background、skip-stats、history-view 标志并调用 `ExtractHandle::extract_task`。
4. domain handle 对后台任务直接返回 `None`；history view 在持久化摘要未开启时拒绝；正常任务在互斥区内调用 `statement_records`，过滤内部库、缺表、非 Select、空 schema、空 plan digest，并按 `(digest, plan_digest)` 去重。随后把含 `(len:` 的截断 SQL 标记为 skipped，解码其他 binary plan，递归展开视图依赖，再调用本文件 `dump_package`。
5. `statement_records` 把任务边界转为 Unix 秒；边界早于 epoch 时用零。它锁定全局摘要 map，按 `use_history_view` 选择全部历史窗口或仅每条 summary 的最后窗口，并保留满足 `window.endTime > begin && window.beginTime < end` 的重叠窗口；表名仅接受可用 `split_once('.')` 拆分的项目，用户取 `authUsers` 迭代器首项或空串。
6. `dump_package` 先在内存 `ReplayArchive` 中写固定文件，再执行 `SHOW VARIABLES`、`SHOW GLOBAL BINDINGS` 和每张表的 `SHOW CREATE TABLE`。非视图且未跳过 stats 时加入统计 JSON；记录按 `skipped` 写入 `skippedSQLs/<digest>.json` 或 `SQLs/<digest>.json`。
7. `encode_replay_archive` 得到完整归档字节；`GetGlobalExtStorage(...).WriteFile` 将其写到 `extract/<file_name>`。若 HTTP 请求是 dump 模式，handler 随后调用 `open_extract`，经 `StorageExtractReader` 以 32 KiB 缓冲流式响应。

## 数据与状态

`CanonicalExtractRuntime` 只长期持有一个 `ExtractHandle`；后者内部持有 `Arc<dyn ExtractSource>` 与互斥锁。`ProductionExtractSource` 共享 `Arc<Domain>`，不复制 Domain 状态。源文件自身没有可变静态状态，但读取两个全局设施：`StmtSummaryByDigestMap` 和 `GetGlobalExtStorage`。

`StatementRecord` 的原始数据来自语句摘要窗口；本文件初始填充 statement type、schema、表名、两个 digest、示例 SQL、binary plan 和一个用户。domain handle 再校验表、设置最终 schema、去重、解码 plan 和分类截断 SQL。归档表集合使用有序集合，记录使用有序 map，因此下游 package 的遍历顺序由 domain 数据结构稳定化。

统计导出按“逻辑表 → 物理 ID → 列/索引统计项”组织。分区表顶层只充当 partitions 容器，实际 count/version 等保存在每个分区或 `global` 节点；普通表直接输出物理统计。二进制 histogram 边界、TopN data 使用 Base64，FM sketch 先解码再转 proto 字段。

归档完全在 `ReplayArchive` 内存对象中组装，最后整体编码为 `Vec<u8>` 再 `WriteFile`；下载阶段才是流式读取。扩展超大归档时需要把这一区别纳入内存风险评估。

## 依赖与调用关系

上游生产调用关系：

- `pkg/server/runtime.rs::CanonicalServerDomain::new` → `CanonicalExtractRuntime::new`。
- `CanonicalServerDomain::extract_runtime` → 返回共享 trait object。
- `pkg/server/extract.rs::ExtractTaskServeHandler::new/handle` → `NewExtractTaskServeHandler` → trait 方法 `extract_task`、`extract_task_directory`、`open_extract`。
- `pkg/server/handler/extractorhandler/extractor.rs::streamExtractResponse` → `StorageExtractReader::read/close`。

下游关系：

- `CanonicalExtractRuntime::new` → `pkg/domain/extract.rs::ExtractHandle::new_with_domain`，获得真实 AST 视图依赖解析。
- `CanonicalExtractRuntime::extract_task` → `ExtractHandle::extract_task` → `ProductionExtractSource` 的各 trait 方法。
- `statement_records` → `StmtSummaryByDigestMap.lock().Summaries()`；`table`/统计 helper → `Domain::stats_table` 与 `stats_context().physical_stats`。
- `sql_rows` → `ConcreteSession::new(...).execute(...)`；它被变量、绑定、DDL 与持久化摘要开关查询复用。
- `dump_package` → `ReplayArchive`/`encode_replay_archive` → `GetGlobalExtStorage().WriteFile`；`open_extract` → 同一 storage 的 `Open`。

`pkg/server/Cargo.toml` 直接声明上述 crate 路径依赖，且 `autotests = false`；本文件的集成证据因此位于显式装配的独立 `pkg/server/runtime_test.rs`，而不是源文件内嵌测试。

## 错误处理与边界

- 所有外部错误在本文件边界被压成字符串或 `ExtractError`；这保留对 HTTP 层可显示的消息，但丢失结构化错误类型与 error source 链。
- `SystemTime::duration_since(UNIX_EPOCH)` 在 `now` 和摘要窗口转换中用 `unwrap_or_default`；epoch 前时间视为零。handler 时间戳转回 `SystemTime` 时负值也钳到零。
- `statement_records` 在全局摘要锁 poisoned 时返回明确错误；无法拆成 `db.table` 的表名条目被忽略。之后 domain handle 会过滤没有有效表的整条记录。
- `table_stats_json` 对不存在的逻辑表、缺失的物理统计和 FM sketch 解码失败返回错误；某个列/索引没有统计项时仅跳过该项，不导致整表失败。
- `SHOW CREATE TABLE` 没有返回第二列时任务失败；SQL identifier 已转义反引号，但归档文件路径直接使用库表名，新增命名策略时需审查路径分隔符与归档条目规范。
- `persistent_statement_summary_enabled` 有意把 SQL 查询失败、空结果和非 ON/1 值都折叠为 `false`；当 history view 开启时，上层会报与 Go 一致的“应开启持久化摘要”错误。
- `decode_binary_plan` 对 codec 错误取空字符串，对齐 Go SQL builtin“返回空值并附 warning”中 Extract 只读取返回值的行为；这里不会向调用者暴露 warning。
- `open_extract` 只在打开前检查一次取消状态；后续对象读取不轮询取消。`StorageExtractReader::close` 的错误能由直接调用者看到，但 handler 的 `streamExtractResponse` 明确忽略 close 结果并优先返回拷贝结果。
- `failpoint_enabled` 固定返回 `false`，说明生产 runtime 不启用 handler 的 mock 短路；failpoint 行为由测试 runtime 单独注入。日志当前通过 `eprintln!`，没有结构化字段。

## 并发与资源生命周期

`ExtractRuntime: Send + Sync` 且生产实例放在 `Arc` 中供 HTTP 请求共享。`ExtractHandle` 的 mutex 只覆盖 statement record 的收集、表解析与 digest 去重，匹配 `pkg/domain/extract.rs` 注释所述 Go worker 锁范围；plan 解码、视图展开和 dump 不在该锁内，因此多个请求可能并发进行这些较重阶段并并发访问全局外部存储。

`statement_records` 在复制 `Summaries()` 后释放全局 statement-summary 锁，随后遍历快照，避免在 SQL/存储操作期间持锁。若锁 poisoned，整个任务失败。

每次 `sql_rows` 创建独立 `ConcreteSession`，遍历每个 record set，并在读取完成后显式 `close`；读取或关闭失败都立即终止。`StorageExtractReader` 拥有 boxed object reader，handler 在流式循环结束后调用 `close`。Domain 与 source 均由 `Arc` 维持到 runtime 销毁。

归档写入通过一次 `WriteFile` 完成，没有本文件级临时文件或显式 rollback；编码或写入失败时返回错误，但是否存在部分对象由外部存储实现决定。后台任务当前没有 spawn：`is_background_job = true` 由 domain handle 直接返回 `Ok(None)`，本文件再将其转为空字符串。

## 与 Go 版本的对应关系

直接 Go 对照分为三层：`pkg/server/extract.go` 负责把 `Domain.GetExtractHandle()` 接到 server；`pkg/server/handler/extractorhandler/extractor.go` 负责 HTTP 构建与下载；核心 worker 与归档逻辑位于 `pkg/domain/extract.go`。Rust 把 handler 的副作用抽成 trait，并把 Go worker 的生产数据访问拆到本文件。

已对齐的语义包括：目录名 `extract`；仅支持 Plan；后台任务返回空结果；history view 要求持久化摘要；时间窗口使用严格重叠判断；只接受 Select、非空 schema 和非空 plan digest；按 digest+plan digest 去重；含 `(len:` 的 SQL 写入 skipped；binary plan 去首尾换行；归档的 meta/config/variables/bindings/schema/view/stats/replica/SQL 分类；skip-stats 不生成 `stats/`；dump 下载采用流式 reader。

实现差异与边界必须保留为当前事实：

- Go `collectRecords` 通过 INFORMATION_SCHEMA restricted SQL 查询；Rust `ProductionExtractSource::statement_records` 直接读取 `StmtSummaryByDigestMap` 快照，再由 domain handle做表有效性过滤。
- Go 文件名使用 16 个随机字节加纳秒时间；Rust `pkg/domain/extract.rs::generate_extract_file_name` 使用纳秒与进程内 atomic sequence 混合，目标是唯一性而非复刻随机格式。
- Go 逐条向 zip writer 输出并直接写 external storage；Rust 本文件先构造内存 `ReplayArchive`，再整体编码、整体写入。
- Go 通过 restricted SQL builtin 解码 binary plan；Rust直接调用同一 plan codec 并在失败时得到空字符串。
- 本文件 `ProductionExtractSource::view_dependencies` 自身返回“requires the Domain AST wrapper”；生产构造器总是 `new_with_domain`，真正行为由 `DomainAstExtractSource` 完成。绕过生产构造器时不可宣称具备视图解析。
- Go 使用配置项判断持久化摘要；Rust通过 `SELECT @@tidb_stmt_summary_enable_persistent` 判断，查询失败视为未开启。

Go 测试 `pkg/domain/extract_test.go` 验证非 history view 可执行、持久化摘要关闭时 history view 失败，以及时间窗内任务产生文件名；`pkg/server/handler/extractorhandler/extract_test.go` 验证 server/handler 与 failpoint HTTP 路径。Rust 对应独立测试见下一节。

## 扩展指南

- 新增 Extract 类型时，需要同时扩展 handler crate 的 `ExtractType`/任务构建、domain crate 的 `ExtractType` 与 `ExtractHandle::extract_task` 分派，以及本文件从 handler task 到 domain task 的转换；不能只在本文件增加分支。
- 改语句筛选或摘要字段时，优先修改 `ProductionExtractSource::statement_records`，并同步审查 `pkg/domain/extract.rs::ExtractHandle::extract_task` 的内部库、表存在性、有效记录、去重和截断逻辑，避免在两层重复或改变过滤顺序。
- 改视图解析应落在 `pkg/domain/extract.rs::DomainAstExtractSource`/parser visitor；不要把完整 AST 逻辑塞回本文件当前故意报错的 `view_dependencies`。同步覆盖嵌套视图、CTE、循环和依赖消失。
- 改统计 JSON 应从 `table_stats_json`/`physical_stats_json` 及三个 helper 入手，并同步验证普通表、分区/global、空统计、TopN、bucket 二进制边界和 FM sketch 解码失败；字段变化会影响 plan replay 兼容性。
- 改 archive 格式应从 `ProductionExtractSource::dump_package` 入手，同时核对 Go `pkg/domain/extract.go::dumpExtractPlanPackage`、消费者 `decode_replay_archive` 与既有路径/JSON/TOML 契约。大文件场景应评估由内存 archive 改为真正流式写入，但需保持失败清理和对象存储语义。
- 改取消语义时需贯穿 handler `RequestContext`、domain handle 和外部存储读写；当前只在任务提交/打开前检查，不能假设长任务或下载会被中途取消。
- 改资源或并发模型时需保持 `ExtractHandle` 的串行采集不变量，并关注全局摘要锁、独立 session、全局 storage 和内存峰值。
- 测试必须继续放在独立文件。优先扩展 `pkg/server/runtime_test.rs` 覆盖生产 runtime 与真实 Domain/归档，扩展 `pkg/domain/extract_test.rs` 覆盖通用 worker/视图算法，扩展 `pkg/server/handler/extractorhandler/extract_test.rs` 覆盖 HTTP trait 契约；不要把测试嵌入 `extract_runtime.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，其中 Rust 7,032 个；`files --filter pkg/server/extract_runtime.rs` 命中目标文件；`node --file ... --offset 1/200` 读取完整 511 行并识别 40 个符号；`query CanonicalExtractRuntime`、`query ProductionExtractSource` 定位两个核心结构；`explore pkg/domain/extract.rs ...` 给出 `new_with_domain`、`extract_task`、`view_dependencies`、`dump_package` 的调用关系和相关测试。针对 impl 方法的独立 callers/callees 命令未产生额外可用输出，因此接线以精确源码引用补证。
- 目标源码：`pkg/server/extract_runtime.rs`，确认所有常量、helper、两个结构、两个 trait impl 和 `CanonicalExtractRuntime` 的完整实现。
- crate/模块接线：`pkg/server/Cargo.toml`、`pkg/server/lib.rs`、`pkg/server/runtime.rs`、`pkg/server/extract.rs`、`pkg/server/handler/extractorhandler/extractor.rs`、`pkg/server/server.rs`。
- domain 实现：`pkg/domain/extract.rs`，确认互斥范围、过滤/去重、视图 AST 包装、文件名生成和 dump 调用顺序。
- Go 对照：`pkg/server/extract.go`、`pkg/server/handler/extractorhandler/extractor.go`、`pkg/domain/extract.go`。
- Rust 测试：`pkg/server/runtime_test.rs::go_merge_43_canonical_server_domain_serves_extract_archive` 覆盖真实表、归档固定条目、SQL JSON、统计 JSON、skip-stats 与后台任务；`go_merge_43_extract_archive_view_and_partitions` 覆盖嵌套视图/基表、分区与 global 统计形状。`pkg/domain/extract_test.rs` 覆盖 AST/CTE/循环视图依赖、记录有效性、截断与重复记录、反向时间窗和持久化摘要要求。`pkg/server/extract_test.rs` 与 handler crate 的 `extract_test.rs`/`extractor_test.rs` 覆盖 HTTP 适配、failpoint、参数和流式 reader 契约。
- Go 测试：`pkg/domain/extract_test.go`、`pkg/server/handler/extractorhandler/extract_test.go`，用于核对历史摘要开关、任务产物与 HTTP 入口原始意图。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定的精确标题计数命令验证恰有 11 个固定二级章节，并人工检查所有生产结论均可回指上述符号和路径。
