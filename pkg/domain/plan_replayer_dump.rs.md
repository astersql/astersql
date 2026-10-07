# `pkg/domain/plan_replayer_dump.rs`

## 文件定位

本文件属于 `astersql-domain` crate；`pkg/domain/lib.rs` 以公开模块 `plan_replayer_dump` 暴露它，`pkg/domain/Cargo.toml` 则声明了直接用于 ZIP 编解码的 `zip = 0.6.6` 依赖。它位于 Plan Replayer 任务模型（`pkg/domain/plan_replayer.rs` 中的 `PlanReplayerDumpTask`、`PlanReplayerStatusRecord`）与具体会话/存储实现之间：负责决定归档内容和目录布局，但不直接访问会话、InfoSchema、统计服务或外部存储。

当前主要生产接线在 `pkg/session/runtime/dispatch.rs`：`SessionPlanReplaySource` 实现本文件的 `PlanReplaySource`，命令处理路径构造任务，调用 `dump_plan_replayer_info`，再以 `encode_replay_archive` 生成 ZIP 并写入 Plan Replayer 外部存储目录。`decode_replay_archive` 还被 session 导入路径、server 处理路径及其测试用于读取归档。

## 核心职责

- 固定归档协议：九个 `PLAN_REPLAYER_*_FILE` 常量定义 SQL 元数据、配置、实例信息、变量、TiFlash 副本、两类 binding、schema 清单和错误文件的路径。
- 隔离数据获取与内容编排：`PlanReplaySource` 把表解析、依赖展开、schema/统计/config/binding/explain 获取留给调用者；`dump_plan_replayer_info` 只负责编排和归档布局。
- 构造可重放材料：从 `PlanReplayerDumpTask` 写入 SQL meta、schema/view、stats、statsMem、variables、SQL、bindings、explain/decoded plan、debug trace，并生成状态记录。
- 提供归档容器及 ZIP 边界：`ReplayArchive::write` 保证条目路径合法且唯一；`encode_replay_archive`/`decode_replay_archive` 在内存表示与 ZIP 字节之间转换。
- 提供轻量表引用发现：`extract_table_references` 从 SQL 文本识别 `FROM`、`JOIN`、`UPDATE`、`INTO` 后的标识符，并过滤它能识别的 CTE 名称。

## 主要符号

- `TableNamePair { database, table, is_view }`：已由数据源规范化的对象标识。派生 `Ord`，因此可放入 `BTreeSet` 去重并稳定排序；`is_view` 控制归档路径及是否导出统计。
- `ReplayArchive { files: BTreeMap<String, Vec<u8>> }`：完整归档的内存模型。`BTreeMap` 使遍历和 ZIP 条目顺序确定，但全部内容会同时驻留内存。
- `ReplayArchive::write(path, body) -> Result<(), String>`：拒绝空路径、以 `/` 开头的绝对路径、任一等于 `..` 的路径段和重复路径。重复写入检测发生在 `insert` 后；返回错误时映射中已经保留新 body，这是当前实现事实，因此调用者不应在错误后继续复用该归档。
- `encode_replay_archive(&ReplayArchive)`：用 Stored（不压缩）方式写 ZIP，文件权限设为 `0644`，完成后返回字节。
- `decode_replay_archive(&[u8])`：跳过目录项，读取所有文件条目，再通过 `ReplayArchive::write` 复用路径与重复名校验。
- `PlanReplaySource`：同步数据源 trait。除 `table_dependencies` 默认返回空列表外，其余方法都必须由实现者提供；错误统一为 `String`。
- `dump_plan_replayer_info(source, task, historical_stats_for_capture_enabled)`：文件核心入口，返回内存归档和每条 SQL 对应的 `PlanReplayerStatusRecord`。
- `extract_table_references(sql, current_database)`：轻量 SQL 扫描器；返回排序去重的 `(database, table)` 集合。
- `trim_identifier`：内部辅助函数，仅裁掉标识符两端的反引号、单双引号、分号和逗号。

## 执行流程

`dump_plan_replayer_info` 的顺序是协议的一部分：

1. 由任务字段生成 `sql_meta.toml`；`historicalStatsTS` 仅在大于零时出现。随后通过数据源取得并写入 `config.toml` 与 `meta.txt`。
2. 对每条 SQL 调用 `extract_table_references`，再用 `resolve_table` 丢弃不存在的对象并取得 view 标志。随后以待处理栈迭代展开 `table_dependencies`；对 view 额外展开 `view_dependencies`。`BTreeSet::insert` 同时承担去重和循环终止保护。
3. 按确定顺序遍历对象：表和 view 都写 `SHOW CREATE` 结果及可能的 TiFlash 副本；只有普通表进入 `schema/schema_meta.txt`、`statsMem/` 和 `stats/`。当任务同时是 capture、continuous capture 且历史统计开关开启时，跳过即时 stats。
4. 写会话变量、逐条 SQL、session bindings 和 global bindings。变量采用 Rust debug 字符串字面量格式；session binding 会追加分号，global binding 只追加换行。
5. 若 `encoded_plan` 为空，逐条调用 `explain(sql, analyze)`：单 SQL 写 `explain.txt`，多 SQL 写 `explain/explainN.txt`；成功返回的 trace 依次写入 `debug_trace/`，完全没有 trace 时仍写一个空的 `debug_trace0.json`。单条 explain 失败会被收集而不是中止后续 SQL。
6. 若已有 `encoded_plan`，调用 `decode_plan` 并写 `explain/sql.txt`；debug trace 使用任务携带的列表，空列表同样补一个空文件。
7. 非致命的统计 fallback 与 explain 错误合并写入 `errors.txt`。最后按输入 SQL 次序生成状态记录；只有 encoded-plan 分支填入 SQL/plan digest，token 来自 `task.file_name`。

ZIP 本身是独立的后续阶段：调用方把返回的 `ReplayArchive` 传给 `encode_replay_archive`。反向读取由 `decode_replay_archive` 完成，归档内容组装不拥有文件或网络写入步骤。

## 数据与状态

本文件没有全局可变状态。一次 dump 的输入状态集中在不可变借用的 `PlanReplayerDumpTask` 与 `historical_stats_for_capture_enabled`，外部环境通过共享借用的 `dyn PlanReplaySource` 读取；输出是全新 `ReplayArchive` 与状态记录向量。

归档路径由常量和对象名拼接产生：`schema/<db>.<table>.schema.txt`、`view/<db>.<view>.view.txt`、`stats/<db>.<table>.json`、`statsMem/<db>.<table>.txt`、`sql/sqlN.sql`、`explain[/explainN].txt`、`debug_trace/debug_traceN.json`。`BTreeMap`/`BTreeSet` 带来稳定的字典序，但对象名在此层不会被转义或净化；安全性依赖 `resolve_table` 提供合法名称，且 `ReplayArchive::write` 只能阻止完整 `..` 段和绝对路径。

`table_stats`、`presigned_url` 等任务字段未在本文件消费；外部存储路径和 URL 由调用层处理。状态记录的 `failed_reason` 在成功返回时恒为空，本文件不会持久化这些记录。

## 依赖与调用关系

上游调用关系（精确仓库搜索证据）：

- `pkg/session/runtime/dispatch.rs` 的 `SessionPlanReplaySource` 是当前生产数据源适配器；同文件的 Plan Replayer 命令路径调用 `dump_plan_replayer_info` 和 `encode_replay_archive`，然后写外部存储。
- `pkg/session/runtime/dispatch.rs` 的导入/处理路径调用 `decode_replay_archive`；`pkg/server/extract_runtime.rs` 复用 `ReplayArchive` 和 `encode_replay_archive`；session/server 的运行时测试也解码归档验证协议。
- `pkg/domain/plan_replayer_dump_test.rs` 直接覆盖核心入口、抽取器和 ZIP 往返。

下游依赖只有标准库、`zip` 和相邻任务模型：`std::collections::{BTreeMap, BTreeSet}` 提供确定顺序与去重，`std::io::{Cursor, Read, Write}` 支撑内存 ZIP I/O，`zip::{ZipWriter, ZipArchive}` 实现格式，`crate::plan_replayer` 提供任务和 status 类型。数据源的真正下游（InfoSchema、统计、SQL executor、配置、bindings）位于 `SessionPlanReplaySource`，不形成 domain crate 对 session crate 的反向依赖。

RustCodeGraph 可定位 `dump_plan_replayer_info`（`pkg/domain/plan_replayer_dump.rs:275`）和任务类型（`pkg/domain/plan_replayer.rs:618`），但本次 `callers`/`callees` 对这些自由函数没有输出，因此生产调用边由上述精确符号搜索和源码读取补证，不能把“图中无边”解释为“未接线”。

## 错误处理与边界

大多数错误是致命的：ZIP 读写、非法或重复归档路径、config/meta、表解析和依赖展开、schema、TiFlash、statsMem、stats、bindings、encoded-plan 解码任一失败都会以 `Result<_, String>` 立即返回，调用方只能得到错误，拿不到部分归档。例外是 `source.explain` 的逐 SQL 错误和 `source.stats` 返回的 fallback 消息，它们写入 `errors.txt`，允许 dump 成功。

`resolve_table` 返回 `Ok(None)` 时引用被静默忽略，这是接口明确表达的“不存在/不可解析”分支。view 不写 schema meta、stats 或 statsMem，但仍写 view DDL 与 TiFlash 数据源返回值。零条 SQL 仍会生成元数据、空 schema/TiFlash/binding 内容及一个空 debug trace；是否允许空任务应由上游命令校验。

`extract_table_references` 不是 SQL parser：它不处理注释/字符串语义、复杂 quoting、逗号连接中的后续表、嵌套查询的所有形式、`DELETE FROM` 之外的更多语法，也只识别形如 `name AS (` 的 CTE。它适合当前已覆盖输入，但任何扩语法工作都必须先与 Go AST Visitor 行为对照并增加独立测试，不能假设对任意 SQL 完整。

解码会忽略 ZIP 目录项，但不会设置解压总量、单文件大小或条目数限制；对不可信超大 ZIP 使用时存在内存/资源放大风险。路径检查也不拒绝反斜杠、`.` 段或平台盘符，调用者不应把 `ReplayArchive.files` 直接落盘而不做额外安全处理。

## 并发与资源生命周期

所有接口均为同步调用；本文件不创建线程、任务、channel、锁或事务。`dump_plan_replayer_info` 只持有 `&dyn PlanReplaySource`，因此 trait 没有要求 `Send`/`Sync`，是否可跨线程共享完全取决于具体实现。当前 session 适配器借用 `ConcreteSession`，调用生命周期受会话命令限制。

归档构建和 ZIP 编解码都在内存中完成，峰值内存至少包含所有条目 body 与最终 ZIP 字节；Stored 模式不消耗压缩 CPU，但也不减少输出尺寸。`ZipWriter::finish` 显式结束中央目录；`ZipArchive` 中每个文件借用在循环迭代结束时释放。发生早退时 Rust RAII 回收内存和 cursor，但本文件没有需要显式关闭的外部文件、结果集或存储句柄；这些资源由 `PlanReplaySource` 实现负责。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/domain/plan_replayer_dump.go`。路径常量、SQL meta 字段、schema/view/stats/statsMem/TiFlash/variables/SQL/bindings/explain/debug trace/errors 的总体布局，以及 continuous capture + historical stats 开关跳过即时统计的分支，均来自 Go `DumpPlanReplayerInfo` 及其 `dump*` 辅助函数。

主要结构差异如下：

- Go 直接持有 `sessionctx.Context`、`Domain`、`zip.Writer` 和文件，Rust 用 `PlanReplaySource` 与 `ReplayArchive` 把采集、内容编排、ZIP、外部存储分层；Rust 的生产实现位于 session runtime。
- Go 的 `tableNameExtractor` 遍历解析后的 AST，递归解析 view SQL，并由 `findFK` 补外键表；Rust 核心用文本扫描，规范化和 view/外键依赖通过 trait 回调完成。两者目标相同，但 Rust 语法覆盖明显更窄。
- Go 在 defer 中关闭 zip/文件、记录成功失败指标并插入 status；Rust 本文件只返回 archive/records，编码、持久化和 status 生命周期由上层负责。
- Go 以 TOML encoder/真实会话变量遍历生成内容；Rust 核心按字符串拼接和任务已收集字段写入。格式兼容性必须由跨层测试维持，不能仅以文件名相同推断完全等价。
- `schema_meta.txt` 的当前文本也不是逐字复刻：Go `dumpSchemaMeta` 写 `db.table;`，Rust 写 `database;table\n`，而 Rust 独立测试固定了后者。消费者若依赖此文件，修改前必须先确认实际读取协议。
- Go explain 主流程失败时把错误写入 errors 后继续完成归档；Rust逐条容错，因此多 SQL 情况下可以保留其他 explain。encoded plan 在两侧都写 `explain/sql.txt`。

## 扩展指南

新增归档条目时，应在本文件增加明确常量或集中路径逻辑，接入 `dump_plan_replayer_info` 的对应阶段，并在 `pkg/domain/plan_replayer_dump_test.rs` 增加布局、内容、错误和重复路径断言；测试逻辑必须继续留在独立测试文件。若数据来自会话环境，应先扩展 `PlanReplaySource`，同步实现 `pkg/session/runtime/dispatch.rs::SessionPlanReplaySource`，避免把 session 依赖引入 domain crate。

扩展 SQL 语法或依赖发现时，优先替换/增强 `extract_table_references`，并覆盖 quoted identifier、CTE、嵌套查询、多表 DML、注释和字符串等边界；同时核对 Go `tableNameExtractor`、`extractTableNames`、`findFK`。变更对象路径前必须评估 path traversal、重复路径和旧归档消费者兼容性。

更改 capture/历史统计、encoded plan、debug trace 或 status 语义时，必须同步检查 `PlanReplayerDumpTask`、session 调用层及 Go `DumpPlanReplayerInfo`，并保持已有空 debug trace、单/多 SQL explain 路径和 digest 分支。对大归档优化可考虑流式接口，但这会改变 `ReplayArchive` 的确定顺序、错误原子性和调用契约，需要先补兼容测试与资源上限设计。

## 验证依据

- Rust 主体：`pkg/domain/plan_replayer_dump.rs`，重点符号为 `ReplayArchive::write`、`encode_replay_archive`、`decode_replay_archive`、`PlanReplaySource`、`dump_plan_replayer_info`、`extract_table_references`。
- 任务/status 模型：`pkg/domain/plan_replayer.rs` 的 `PlanReplayerDumpTask`、`PlanReplayerStatusRecord`；模块边界：`pkg/domain/lib.rs`；crate 与 ZIP 依赖：`pkg/domain/Cargo.toml`。
- 生产接线：`pkg/session/runtime/dispatch.rs` 的 `SessionPlanReplaySource` 和对 dump/encode/decode 的调用；其他消费者由 `rg` 定位到 `pkg/server/extract_runtime.rs`、`pkg/session/runtime_test/session.rs`、`pkg/server/runtime_test.rs` 及 server handler 测试。
- Rust 独立测试：`pkg/domain/plan_replayer_dump_test.rs` 验证 view 依赖和普通表过滤、fallback 错误、TiFlash、空 debug trace、encoded-plan 路径与 digest、历史统计 meta、别名非 CTE、ZIP 往返。
- Go 对照：`pkg/domain/plan_replayer_dump.go` 的 `DumpPlanReplayerInfo`、`tableNameExtractor`、`findFK`、`dumpSQLMeta`、`dumpSchemas`、`dumpStats`、`dumpVariables`、bindings、`dumpExplain`、`dumpDebugTrace` 和 `dumpErrorMsgs`。
- RustCodeGraph：`status` 显示索引包含目标 Rust/Go 文件；`files --filter pkg/domain` 列出目标、Go 对照和独立测试；`query dump_plan_replayer_info --kind function` 定位核心入口。图调用边未返回结果，已用精确源码搜索补证。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核无运行时“已支持”臆测。
