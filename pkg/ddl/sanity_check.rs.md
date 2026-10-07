# `pkg/ddl/sanity_check.rs`

## 文件定位

`pkg/ddl/sanity_check.rs` 属于 `astersql-ddl` crate；crate 根由 `pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/ddl/lib.rs` 通过 `pub mod sanity_check` 将本模块公开。文件实现 DDL 完成后的两类纯健全性校验：根据抽象后的删除范围作业推算应产生的 DeleteRange 数量，以及校验历史 DDL 作业保存的查询文本和语句种类。

这不是 DDL 作业执行器、持久化层或 SQL 解析器。它不创建 DeleteRange 任务、不查询 `mysql.gc_delete_range`、不解析 SQL，也不参与 schema 状态转换、reorg、版本发布或同步。仓库级 Rust 使用搜索只找到 `pkg/ddl/sanity_check_test.rs`；因此当前 Rust 实现虽由 crate 公开，但尚未像 Go 的 `pkg/ddl/executor.go:8125` 那样接入“作业进入历史后”的运行时主链。它当前是可复用的校验核心和移植中的边界，而不是已经生效的线上检查。

## 核心职责

1. `expected_delete_range_count` 根据 `DeleteRangeJob.action` 及动作参数计算期望的删除范围记录数，用来发现异步 GC 范围的遗漏或重复；输入模型来自 `pkg/ddl/delete_range.rs`，与真正生成范围任务的代码保持同一动作词汇。
2. `DeleteRangeCountContext::deduplicate_index_count` 在多 schema 子作业递归期间共享已计数索引 ID，避免 `ModifyColumn` 对同一索引重复计数。
3. `check_delete_range_count` 把期望值与调用者提供的实际值比较，将不一致转换为包含作业 ID、期望数和实际数的结构化错误。
4. `check_history_job` 对调用者已经解析、分类后的历史作业 SQL 做形态检查，包括内部空查询动作、显式 `"skip"`、语句数量、非 DDL 排除和 CREATE 动作与语句种类匹配。

该文件只做确定性的内存计算。数据库读取、SQL parser 配置、日志和 panic 策略仍在 Go 对照实现中，Rust API 刻意把“实际数量”和“解析后的语句类别”作为参数传入。

## 主要符号

- `DeleteRangeCountContext { index_ids: BTreeSet<i64> }`：跨递归调用的私有去重状态；类型公开，但集合字段不公开，只能通过方法更新。
- `DeleteRangeCountContext::deduplicate_index_count(&mut self, index_ids: &[i64]) -> usize`：插入每个 ID，并返回本次首次出现的 ID 数。输入内重复和前序子作业已出现的 ID 都不重复计数。
- `expected_delete_range_count(&mut DeleteRangeCountContext, &DeleteRangeJob) -> usize`：DeleteRange 期望计数的核心分派器；`MultiSchemaChange` 会递归调用自身并复用同一个上下文。
- `DeleteRangeCountMismatch { job_id, expected, actual }`：`check_delete_range_count` 的可比较错误值；没有实现 `Display` 或 `std::error::Error`。
- `check_delete_range_count(&DeleteRangeJob, usize) -> Result<(), DeleteRangeCountMismatch>`：每次以默认上下文计算一个顶层作业，匹配返回 `Ok(())`，否则返回结构化差异。
- `HistoryStatementKind`：调用者对 parser AST 的归类结果，包括五类 CREATE、一般 `Ddl` 和 `NonDdl`；本文件不执行解析。
- `HistoryJobAction`：影响历史查询校验规则的动作子集。未单列的动作应由适配层映射为 `Other`。
- `HistorySanityError`：历史校验的结构化失败原因。当前实际可返回 `EmptyQuery`、`QueryMustBeEmpty`、`InvalidStatementCount`、`NonDdlStatement` 和 `UnexpectedStatementKind`；`UnexpectedCreateStatement` 在本文件中没有产生路径。
- `check_history_job(i64, HistoryJobAction, &str, &[HistoryStatementKind]) -> Result<(), HistorySanityError>`：历史作业校验入口，错误均携带作业 ID。

本文件没有模块级常量、trait、条件编译、异步函数或后台任务。

## 执行流程

DeleteRange 计数流程如下：

1. `expected_delete_range_count` 先检查 `job.cancelled`；取消作业立即返回 `0`。
2. 整表类动作按旧物理表数量计数：`DropSchema` 和 `TruncatePartition` 只计旧物理表；`DropTable`、`TruncateTable` 再为表本身加一。
3. 分区重组类动作（包括 `DropPartition`）把旧物理表数与旧全局索引数相加。
4. `AddIndex`/`AddPrimaryKey` 逐个索引计算：全局索引按一个物理范围，本地索引按 `max(partition_ids.len(), 1)`；回滚完成时同时计算正式与临时索引，数量乘二。
5. `DropIndex`/`DropPrimaryKey` 要求完成态参数中至少有一个索引。列存索引返回零，其他索引按分区数、至少一个物理表计数。
6. `DropColumn` 用物理表数乘索引 ID 数；`ModifyColumn` 则乘“上下文中新出现的索引 ID 数”。
7. `MultiSchemaChange` 顺序递归所有 `subjobs` 并求和；共享上下文保证跨子作业去重。`Other` 返回零。
8. `check_delete_range_count` 创建全新的默认上下文，计算一次期望值，再与 `actual` 比较。

历史作业流程如下：

1. `UpdateTiFlashReplicaStatus` 和 `UnlockTable` 必须具有严格为空的查询字符串；此分支在语句检查前返回。
2. 查询严格等于 `"skip"` 时跳过其余校验。
3. 其余查询经 `trim()` 后为空则返回 `EmptyQuery`。
4. 除 `CreateTables` 外，必须恰好有一个已分类语句；`CreateTables` 允许多条，也允许调用者传入空切片。
5. 逐条检查动作与语句类型：三个具体 CREATE 动作要求精确匹配；`CreateTables` 只允许表、序列、视图；`Other` 允许除 `NonDdl` 外的任何枚举值。

## 数据与状态

唯一的可变状态是 `DeleteRangeCountContext.index_ids`。它使用有序集合 `BTreeSet<i64>`，但算法只依赖集合的唯一性，不依赖遍历顺序；集合生命周期由调用者控制。在 `check_delete_range_count` 中上下文只覆盖一个顶层作业，在 `MultiSchemaChange` 内则覆盖所有递归子作业。

`DeleteRangeJob` 是从 DDL 作业抽取的值对象，定义于 `pkg/ddl/delete_range.rs`。本文件读取 `cancelled`、`action`、`rollback_done`、旧物理表、分区、索引参数、索引 ID、旧全局索引和子作业，不修改作业。`job.id` 只在构造 `DeleteRangeCountMismatch` 时使用。

历史校验不保存 parser AST，只接收 `HistoryStatementKind` 切片。因此 SQL 文本和分类结果的一致性是上游适配层的责任；本函数仅把 `query` 用于空值和 `"skip"` 判断。所有错误枚举均是拥有作业 ID 的普通值，不包含查询文本或底层错误。

## 依赖与调用关系

直接 Rust 依赖很小：标准库 `BTreeSet`，以及同 crate 的 `crate::delete_range::{DeleteRangeAction, DeleteRangeJob}`。`pkg/ddl/Cargo.toml` 没有为该文件引入专属第三方依赖；模块归属于 `astersql-ddl`，并通过 `pkg/ddl/lib.rs` 导出。

RustCodeGraph 将 `check_delete_range_count -> expected_delete_range_count`、`check_delete_range_count -> DeleteRangeCountMismatch`、`expected_delete_range_count -> deduplicate_index_count` 识别为直接边；`expected_delete_range_count` 还存在对自身的源码递归边（`MultiSchemaChange`）。图查询没有返回生产调用者，仓库级 `rg` 也确认 Rust 调用仅在 `pkg/ddl/sanity_check_test.rs`。这意味着当前上游是测试或未来适配层，而非 Rust `executor`。

概念上的下游事实来源是 `pkg/ddl/delete_range.rs`：其 `add_delete_range_job`/`insert_job_into_delete_range` 生成真实范围任务，本文件只预测数量。两者必须同步维护。值得注意的是，当前生成器把 `DropPartition` 与 `TruncatePartition` 一样只生成旧物理表任务，而本文件与 Go 版本都还计入 `old_global_indexes`；这是现有代码间需要后续核查的差异，本文不把它描述为已经一致。

Go 运行时链为 `pkg/ddl/executor.go` 取得历史作业后调用 `(*executor).checkHistoryJobInTest`，后者在内部检查开关启用时按需调用 `checkDeleteRangeCnt`，并解析、分类历史 SQL。Rust 尚缺少这一取数、解析、动作映射和调用接线。

## 错误处理与边界

- 取消作业无条件期望零个 DeleteRange。
- 空分区列表按非分区表处理，物理范围数至少为一；全局索引始终只计一次。
- `DropIndex`/`DropPrimaryKey` 对空 `index_arguments` 使用 `expect` 并 panic，消息为 `finished drop-index job must contain an index argument`。独立 Rust 测试专门固定了这一行为；它假设输入是已完成作业，而不是容错解析任意作业。
- `expected_delete_range_count` 不返回参数解码错误，因为 Rust 输入已经是抽取完成的强类型 `DeleteRangeJob`；这与 Go 在函数内调用 `model.GetFinished*Args` 并传播错误不同。
- `check_delete_range_count` 不查询数据库，也不区分 mock session；`actual` 的来源和查询失败策略完全属于调用者。
- 历史动作的两个内部空查询分支检查 `query.is_empty()`，空白字符串也会失败为 `QueryMustBeEmpty`；其他动作对空白字符串返回 `EmptyQuery`。
- `"skip"` 必须大小写和内容精确匹配，带空白不会跳过。
- 非批量建表动作要求语句数恰好为一；批量建表只校验已有元素类型，当前没有“至少一条”的约束。
- `Other` 遇到任何非法种类都统一返回 `NonDdlStatement`；`UnexpectedCreateStatement` 目前未使用。新增错误分支前应避免让公开枚举语义与实际返回路径继续漂移。

## 并发与资源生命周期

本文件没有锁、原子量、通道、线程、异步任务、事务、会话或 I/O 资源。所有函数同步执行，除显式传入的 `&mut DeleteRangeCountContext` 外没有共享状态，因此不同上下文可并行调用。

同一个上下文不能被并发可变借用；这由 Rust 借用规则保证。调用者若错误地跨互不相关的顶层作业复用上下文，会让后续 `ModifyColumn` 计数偏小，所以推荐顶层作业一上下文，仅在该作业的多 schema 子作业间共享。Go 对照实现的上下文按值传递，但内部 map 引用共享；Rust 的显式可变引用复现了该递归共享生命周期。

真实 Go 校验会临时从 session pool 借用 session、查询两张 GC 表并关闭 record set；这些资源生命周期不在当前 Rust 文件内。若未来接线，应在适配层承担获取/归还、查询错误和测试开关，不应塞入这些纯函数。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/sanity_check.go`：

- `DeleteRangeCountContext` / `deduplicate_index_count` 对应 `delRangeCntCtx` / `deduplicateIdxCnt`。
- `expected_delete_range_count` 对应 `expectedDeleteRangeCnt`，现有共同覆盖取消作业、删库/表/分区、索引增删、列变更和多 schema 递归的主要公式。
- `check_delete_range_count` 只覆盖 Go `checkDeleteRangeCnt` 的“比较并报告差异”部分；Go 还会通过 session pool 查询 `mysql.gc_delete_range` 与 `mysql.gc_delete_range_done`，并对错误或不一致 panic。
- `check_history_job` 加上上游的动作/AST 分类才对应 Go `checkHistoryJobInTest` 与 `checkHistoryJobStmtType`。Go 会读取会话 SQL mode 和 parser config 自行解析文本，Rust 只验证传入类别。

当前并非完整等价移植。Go 已覆盖 `ActionDropMaterializedView*`、`ActionCreateMaterializedView*`、刷新切换等物化视图动作；Rust 的 `DeleteRangeAction` 和 `HistoryJobAction` 没有这些专门变体。Go 还检查 `historyJob.BinlogInfo.FinishedTS != 0`，Rust 无对应字段或逻辑。Go 的 parser 错误与作业参数解码错误也没有 Rust 表示。反过来，Rust 提供了结构化 `Result` 错误，便于调用者决定 panic、日志或测试断言，而不是在纯逻辑层直接中止。

## 扩展指南

新增或修改 DDL 动作时，应把以下位置作为一个一致性单元审查：

1. 在 `pkg/ddl/delete_range.rs` 的 `DeleteRangeAction`、`DeleteRangeJob` 抽取字段和 `insert_job_into_delete_range` 中定义真实任务生成语义。
2. 同步修改本文件 `expected_delete_range_count` 的计数公式；涉及同一索引跨子作业时继续复用 `DeleteRangeCountContext`，不要在递归层重新初始化。
3. 在独立文件 `pkg/ddl/sanity_check_test.rs` 增加边界测试；不要把测试嵌入生产源文件。至少覆盖取消、非分区、分区、全局/本地/列存索引、回滚完成、多 schema 重复索引和缺失完成态参数。
4. 历史动作扩展要同时维护 `HistoryJobAction`、上游 AST 分类适配、`check_history_job` 和 Go 的 `checkHistoryJobStmtType` 对应测试；物化视图是当前明确未覆盖区域。
5. 若接入运行时，应在独立适配层实现测试开关、历史作业到精简模型的转换、GC 表实际计数、SQL 解析和错误策略，再调用这里的纯函数。不要声称仅导出模块就等价于 Go 的 `checkHistoryJobInTest` 接线。
6. 兼容风险主要是漏计导致删除范围缺失未被发现、重复计数导致假阳性，以及动作到语句类型映射过宽。性能风险低于实际数据库查询；纯计数复杂度近似输入表/索引/子作业总量乘以 `BTreeSet` 插入的对数因子。

由于这是 DDL 完成后的测试健全性检查，而不是作业推进逻辑，它本身不需要新增 schema 状态、reorg checkpoint、回滚持久化或 schema version 更新；这些仍由 DDL 主链负责。若新增动作改变上述生命周期，必须先在对应执行模块完成行为，再让本文件忠实校验结果。

## 验证依据

- 源码与符号：`pkg/ddl/sanity_check.rs` 的 `DeleteRangeCountContext`、`expected_delete_range_count`、`check_delete_range_count`、`HistoryStatementKind`、`HistoryJobAction`、`HistorySanityError`、`check_history_job`。
- 输入模型与真实任务生成：`pkg/ddl/delete_range.rs` 的 `DeleteRangeAction`、`DeleteRangeJob`、`add_delete_range_job`、`insert_job_into_delete_range`。
- crate 边界：`pkg/ddl/Cargo.toml` 的 package、dependencies 与 `[lib]`，以及 `pkg/ddl/lib.rs` 的模块声明。
- Rust 独立测试：`pkg/ddl/sanity_check_test.rs`，覆盖 drop-index 缺参数 panic、删表/加索引公式、跨子作业去重、内部空查询、`"skip"`、语句数量和动作特定类别。
- Go 对照及调用链：`pkg/ddl/sanity_check.go`、`pkg/ddl/executor.go:8125`；Go 类型匹配测试见 `pkg/ddl/ddl_test.go` 的 `TestCheckHistoryJobStmtType`。
- 包级契约：`pkg/ddl/doc.go` 的 Online DDL 和 schema version 同步不变量；本文件只在作业完成后验证派生结果，不推进该状态机。
- RustCodeGraph：索引状态显示本仓库 Rust/Go 均已索引；`node --file pkg/ddl/sanity_check.rs` 返回完整 243 行；`query` 定位四个核心函数；`callees` 确认 `check_delete_range_count -> expected_delete_range_count`、错误结构构造和去重调用；`callers` 未返回生产调用者，仓库级 Rust 使用搜索进一步确认只有独立测试引用。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定命令验证恰有十一个固定二级章节，并人工复核所有“已接线/已支持”结论均有上述源码或搜索证据。
