# `pkg/executor/show.rs`

## 文件定位

通过 `ShowRuntimeContext` 隔离会话、权限、InfoSchema、PD/TiKV、分布式任务和系统会话等外部能力；执行器自身不保存这些系统对象（`show.rs:413-467`）。

## 核心职责

- 定义 SHOW 领域数据：`ShowValue`、`ShowRequest`、`TableInfo`、Region/Distribution/Import 作业等轻量模型（`show.rs:32-405`）。
- 通过 `ShowRuntimeContext` 隔离会话、权限、InfoSchema、PD/TiKV、分布式任务和系统会话等外部能力；执行器自身不保存这些系统对象（`show.rs:413-467`）。
- `ShowExec::Next` 首次拉取全量结果，之后按 Chunk 容量分页；`fetchAll` 把 `ShowStmtType` 分发到专用实现或通用 `FetchRows` 边界（`show.rs:593-703`）。
- 在本地实现需要稳定顺序或聚合的语义：库/表过滤与可见性、PROCESSLIST 权限、Region 去重与行编码、Distribution/Import 作业排序和聚合（`show.rs:469-499, 720-858, 1025-1221, 1398-1527`）。
- 提供 SHOW CREATE 文本、LIKE 匹配、时间/大小/持续时间格式化和标识符转义辅助函数（`show.rs:1270-1396, 1588-1690`）。

## 主要符号

- `ShowStmtType` 是语句级枚举；`ShowOperation` 是面向运行时 `FetchRows` 的取数操作码。两者不是一一机械映射：例如 `MasterStatus | BinlogStatus` 共用 `MasterStatus`，`Errors` 由 `fetchShowWarnings(true)` 映射（`show.rs:60-121, 169-226, 623-690`）。
- `ShowValue` 是中间单元格类型，`appendRowToChunk` 把 Null/有符号整数/无符号整数/浮点/字符串/字节映射到 Chunk 列（`show.rs:37-57, 1554-1574`）。
- `ShowRequest` 是 `ShowExec` 公开参数的快照，交给 runtime 时避免暴露可变结果缓存和游标（`show.rs:145-167, 568-591`）。
- `ShowRuntimeContext: Send + Sync` 是核心端口；`FetchRows` 承载通用 SHOW，其他方法为库表、Region、Import/Distribution 作业、系统会话和 SPLIT 表达式提供强类型边界（`show.rs:416-467`）。
- `ShowExec` 持有 SHOW 参数、`Arc<dyn ShowRuntimeContext>`、惰性 `result` 和 `cursor`；公开字段沿用 Go 命名以对齐迁移模型（`show.rs:513-590`）。
- `SelectLimitGuard` 是私有 RAII 守卫，保证 `fetchAll` 正常或错误返回时都恢复原 `SelectLimit`（`show.rs:501-511, 613-622`）。
- `ShowCreateFormatter` 将 TABLE/SEQUENCE/VIEW 的 DDL 格式化外置；`ConstructResultOfShowCreateDatabase` 等辅助函数则在本地组装数据库、Placement Policy 和 Resource Group 文本（`show.rs:374-410, 1292-1384`）。
- `visibleChecker` 在视图表引用遍历时忽略不存在对象，但遇到存在且不可见的表就将 `ok` 置为 `false`（`show.rs:1224-1268`）。

## 执行流程

1. 调用方用 `ShowExec::new(runtime, statement)` 创建空参数执行器，再填充库、表、索引、权限身份和过滤条件（`show.rs:539-566`）。
2. `Next` 先以 `MaxChunkSize` 重置输出 Chunk。若 `result` 未初始化，它创建缓存并调用 `fetchAll`；后续调用只从 `cursor` 处拷贝至当前 Chunk 容量（`show.rs:593-610`）。
3. `fetchAll` 把 runtime 的 `SelectLimit` 临时设为 `u64::MAX`，建立 `SelectLimitGuard`，然后对 `Tp` 做穷尽分发（`show.rs:613-691`）。
4. 简单分支调用 `fetchOperation`，它由 `request()` 制作参数快照，调用 runtime `FetchRows`并扩展缓存（`show.rs:568-591, 693-703`）。库表、ProcessList、Region、Distribution Jobs 和 Import Jobs/Groups 等分支在本地完成额外规则。
5. `appendRowToChunk` 先校验行宽与 Chunk 列数相等，再逐列追加；成功后 `Next` 推进游标，越过末尾时返回空 Chunk（`show.rs:600-610, 1554-1574`）。

关键专用流程：`fetchShowDatabases` 不区分大小写排序、将 `information_schema` 前置，再依次做库可见性、精确字段和 LIKE 过滤（`show.rs:720-745`）。`fetchShowTables` 先拒绝不可见库、报告不存在库，排除会话临时表，按表名排序并根据 `Full` 决定是否输出 TableType（`show.rs:777-858`）。`fetchShowProcessList` 从 SessionManager 获取快照，无 PROCESS 权限时仅保留当前登录用户（`show.rs:469-499, 748-759`）。

## 数据与状态

`ShowExec` 的请求状态由 `Tp`、`DBName`、`Table`、`Partition`、`Column`、`IndexName`、角色/用户、`Extractor`、多个布尔开关及 Import/Distribution 作业标识组成。结果状态是 `Option<Vec<Vec<ShowValue>>>`和 `cursor`：`None` 表示尚未取数，`Some(empty)` 则可表示已执行但无行（`show.rs:514-537, 593-610`）。这是全量物化而非流式拉取，结果集大小直接影响内存。

`fetchShowImportGroups` 使用 `BTreeMap<String, groupInfo>`，因此输出按 group key 稳定有序；空 group key 被排除，状态计数仅识别 pending/running/finished/failed/cancelled，创建时间取最早、更新时间取最晚（`show.rs:1138-1193`）。`fetchShowImportJobs` 按 job id 排序，只为 running 作业读 runtime；若 runtime 有 `awaiting_resolution_error`，展示状态与错误信息被覆盖（`show.rs:1196-1221`）。`FillOneImportJobInfo` 固定产生 21 列，非运行作业的六个进度列为 NULL（`show.rs:1417-1489`）。

Region 路径通过 `HashSet` 按 region id 去重并保留首次出现顺序（`show.rs:1398-1415, 1576-1586`）。时间以 `SystemTime` 保存，编码为 Unix 秒加 9 位纳秒字符串；早于 epoch 的值因 `unwrap_or_default` 会变成 `0.000000000`（`show.rs:1630-1654`）。

## 依赖与调用关系

- 上游：crate 入口 `pkg/executor/lib.rs:202` 公开 `show` 模块。RustCodeGraph 索引识别 `show.rs` 及 272 个符号，但对 Rust impl method 的 `callers/callees` 查询没有返回可用边；使用 `rg` 核验后，`ShowExec::new` 的仓库内直接调用仅见 `pkg/executor/show/import_groups_test.rs`。这证明模块导出存在，但不证明生产调用链已接通。
- 当前 Rust 会话主链：`pkg/session/runtime/dispatch.rs` 直接匹配 parser `ast::ShowStmtType`，对 Databases/Tables/CreateDatabase/Columns/Regions/Variables 等执行分支逻辑；它没有引用本文件的 `ShowExec`。
- 下游：`ShowExec` 只直接调用 `ShowRuntimeContext`、`SessionManager::ShowProcessList`、`ProcessInfo::ToRowForShow` 及 `chunk::Chunk` 追加 API。具体 InfoSchema、PD/TiKV、权限和任务存储实现应由 runtime adapter 提供，本文件中没有 adapter 实现（`show.rs:416-467`）。
- Go 下游对照：`pkg/executor/show.go` 直接依赖 session context、InfoSchema、privilege manager、PD helper、distributed task/importer 和系统会话；Rust 将这些耦合收敛到 `ShowRuntimeContext` 和 `ShowCreateFormatter`。

## 错误处理与边界

公开可失败路径统一返回 `ShowResult<T> = Result<T, errors::SharedError>`。runtime 错误通过 `?` 原样向上传播；本地生成的错误包括：缺少表参数、库/表不存在或权限被拒、已移除的 extended statistics、空库名/字符集或校对名、Distribution alias 不是三段，以及行宽与 Chunk 列数不等（`show.rs:649-652, 819-826, 977-1013, 1338-1341, 1387-1390, 1504-1509, 1555-1562`）。

明确的空结果边界有：`Events`、`Profiles`和 `fetchShowOpenTables` 直接成功但不写行；SessionManager 缺失时 PROCESSLIST 为空；`visibleChecker::Enter` 对不存在表继续遍历（`show.rs:477-499, 649, 762-765, 1248-1261`）。`sqlLike` 大小写不敏感，支持 `%`、`_` 和反斜线转义；末尾单独反斜线不产生匹配（`show.rs:1588-1628`）。

兼容性边界也是迁移缺口：Rust `fetchShowTableRegions` 只使用单个 `TableInfo.id` 作为 physical id，分区、global index、非分区表带 partition clause 及索引不存在的 Go 细节必须由 runtime 或未来扩展补齐（Rust `show.rs:1025-1045`；Go `show.go:2303-2432`）。

## 并发与资源生命周期

`ShowRuntimeContext` 和 `ShowCreateFormatter` 要求 `Send + Sync`，并以 `Arc` 在 `ShowExec`、`visibleChecker` 与 runtime guard 之间共享（`show.rs:407, 416, 503-504, 534-536, 1232-1236`）。但 `ShowExec::Next` 需要 `&mut self`，`result` 和 `cursor` 无内部锁，因此单个执行器实例应由一个消费流串行驱动，不应并发调用 `Next`。

`SelectLimitGuard` 是本文件唯一显式 RAII 资源：它在 `fetchAll` 分发前记录旧值，作用域结束自动恢复，包括 fetch 返回错误的路径（`show.rs:501-511, 613-622`）。`runWithSystemSession` 在本层不直接获取或释放会话，它把回调交给 runtime 管理（`show.rs:1541-1547`）；Go 对照会获取 sys session、加载 snapshot InfoSchema、开启事务，并用 defer rollback/release（`pkg/executor/show.go:2953-2973`），所以 Rust runtime adapter 必须保持这些生命周期保证。

## 与 Go 版本的对应关系

Rust 不直接持有 Go `BaseExecutor`/session context，而是用 `ShowRuntimeContext` 转译环境能力。Rust 用 RAII 恢复 SelectLimit；Go `fetchAll` 使用 defer。

- Databases/Tables 的排序、`information_schema` 前置、可见性、LIKE 和 FULL TableType（Rust `show.rs:720-858`；Go `show.go:473-700`）。
- PROCESSLIST 的 PROCESS 权限与当前用户过滤（Rust `show.rs:469-499`；Go `show.go:515-540`）。
- CREATE DATABASE 的 `IF NOT EXISTS`、反引号转义、字符集/非默认 collation 和 placement policy 注释（Rust `show.rs:1337-1366`；Go `show.go:1757-1847`）。
- Region 去重、Distribution job alias 三段解析、Import job 列顺序/进度与 Import group 状态聚合（Rust `show.rs:1025-1221, 1398-1527`；Go `show.go:2303-2480, 2561-2919`）。
- 视图列类型需在独立系统会话重建计划，SPLIT 值需先解析为表达式再格式化；Rust 仅定义调度边界，真实逻辑必须由 runtime 实现（Rust `show.rs:1529-1552`；Go `show.go:2922-2983`）。

差异/未完全证明的部分：Rust 当前未接入会话生产主链；多数 SHOW 通过抽象 `FetchRows` 委派，本仓库未找到实现全部端口的生产 adapter；`Events`/`Profiles`/`OpenTables` 是空结果；Region 分区与 global-index 细节少于 Go；Rust 时间输出是 epoch 字符串而 Go 是具有 SQL 时区/类型语义的 `types.Time`。这些均应视为扩展前需要逐项校验的迁移边界，不是“已完整支持”。

## 扩展指南

1. 新增 SHOW 类型时，同步扩展 `ShowStmtType`、必要的 `ShowOperation`、`ShowRequest` 参数和 `fetchAll` 穷尽分支。若只需 runtime 取行，复用 `fetchOperation`；若存在排序、权限、聚合或跨行不变量，在专用 `fetchShow*` 中实现并保持 runtime 边界（`show.rs:568-703`）。
2. 列布局变更要同时更新行构造函数、planner 输出 schema 和独立测试；`appendRowToChunk` 会在列数不等时报错，但不校验每列的 SQL 类型兼容性（`show.rs:1554-1574`）。
3. 补生产接线时，需在 session/executor builder 层实现 `ShowRuntimeContext`，把 parser AST 的 SHOW 类型与参数无损转成本文件模型，并验证与当前 `pkg/session/runtime/dispatch.rs` 主链的等价性；不应仅因 crate 已导出就移除现有分支。
4. 保持 Go 对齐时重点比较分区/global index Region、权限错误文本、时区和 SQL datetime 类型、IMPORT 未知文件大小、系统会话的 snapshot/事务/rollback 生命周期。
5. Rust 测试逻辑保持在独立文件：普通辅助函数和 Go 格式对齐扩展 `pkg/executor/show_test.rs`；Import group/job 执行器行为扩展 `pkg/executor/show/import_groups_test.rs`；不要把新测试内嵌到 `show.rs`。Go 回归对照主要在 `pkg/executor/show_test.go` 和 `pkg/executor/test/showtest/show_test.go`。

主要风险：兼容性风险来自 MySQL/TiDB 的精确排序、列型、错误码/文本和权限语义；正确性风险来自将 runtime adapter 的快照、事务或可见性实现得过于简化；性能风险来自 `fetchAll` 全量物化和大量 LIKE 匹配的 `O(pattern_len * value_len)` 动态规划内存/时间开销（`show.rs:593-610, 1588-1628`）。
