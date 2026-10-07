# `pkg/planner/core/fts_resolve_index.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，crate 边界由 `pkg/planner/core/Cargo.toml` 定义；`pkg/planner/core/lib.rs` 以私有模块 `mod fts_resolve_index` 装配它，再通过 `pub use fts_resolve_index::*` 导出公开符号。文件处理轻量 `PlanNode` 逻辑计划中的全文检索解析：把 `FTS_MATCH_WORD(query, column)` 从 WHERE、ORDER BY 和 SELECT 的字符串表示转换为扫描节点可携带的全文下推元数据，并拒绝没有被规则消费的用法。

当前可确认的应用入口是 `pkg/session/fts_runtime.rs` 的 `FtsSession::plan_select`：它先调用 `BuildFullTextPlan` 构造计划，再以表元数据中的全文索引和事务 dirty 状态调用 `ResolveFullTextPlan`。因此本文件位于 SQL AST 已被构造成轻量计划之后、会话返回已解析计划之前。它不负责执行 TiFlash 请求、建立全文索引或解析完整 SQL 语法。

## 核心职责

1. `ResolveFullTextPlan` 固定按 WHERE、TopN、Projection、Reject 四阶段处理计划，后阶段依赖前阶段写入扫描节点的载荷。
2. WHERE 阶段只接受可解释为 `FTS_MATCH_WORD('常量', 单列)` 的独立条件，并要求存在恰好覆盖该列的单列 `FullTextIndex`；匹配后从 Selection 中删除该条件。
3. TopN 与 Projection 阶段要求自身的查询文本、列名与 WHERE 已下推的内容完全一致，然后把表达式改写为 `_FTS_SCORE` 并把查询类型切换为 `WithScore`。
4. Reject 阶段遍历整棵树，对仍含 `FTS_MATCH_WORD` 的 Projection、Selection、TopN、Sort、UnionScan 或未编码 DataSource 给出针对性错误，避免不支持的表达式静默进入后续流程。
5. `FTSPushDown::encode/decode` 在 `PlanNode.operator_info` 中保存或恢复索引名、列名、查询文本、评分模式和 Top-K；`FullTextPushDown` 是公开只读解码入口。

## 主要符号

- `maxFTSTopK: u32`：未能安全收紧 Top-K 时使用的上界 `u32::MAX`。
- `ftsMatchWordDirtyTxnErrMsg`：存在未提交写时统一返回的错误文本。
- `FullTextIndex { name, columns }`：解析器接收的最小全文索引描述。当前匹配逻辑只使用名字和列名列表。
- `FTSQueryType::{NoScore, WithScore}`：区分只做布尔匹配和还需产出相关性分数的查询。
- `FTSPushDown`：扫描下推载荷，字段为 `index_name`、`column_name`、`query_text`、`query_type`、`top_k`。其私有 `encode/decode` 使用 `fts_push_down` 标记和 U+001F 分隔符。
- `FTSInfo`：`interpret_fts_expression` 的内部结果，只保存解析出的常量查询文本和列名。
- `FullTextIndexResolverWhere::Optimize`：先做 dirty transaction 全树检查，再调用 `resolve_where`。
- `FullTextIndexResolverTopN::Optimize`：调用 `resolve_top_n`，处理带 LIMIT 的排序评分。
- `FullTextIndexResolverProjection::Optimize`：调用 `resolve_projection`，处理 SELECT 评分表达式。
- `FullTextIndexResolverRejectRemaining::Optimize`：调用 `reject_remaining`；成功时永远报告 `changed = false`。
- `ResolveFullTextPlan`：本文件的一体化公开入口，依次串联四个 resolver。
- `FullTextPushDown`：尝试从任意 `PlanNode.operator_info` 解码载荷，格式不符时返回 `None`。

四个 resolver 的 `Name` 分别返回 `fts_resolve_index_where`、`fts_resolve_index_topn`、`fts_resolve_index_projection` 和 `fts_resolve_reject_remaining`，与 Go 优化器规则名保持一致。

## 执行流程

1. 会话层在 `FtsSession::plan_select` 中取得表的 `FullTextIndex` 列表，检查当前事务的表前缀是否有未提交写，构建计划后调用 `ResolveFullTextPlan`。
2. `FullTextIndexResolverWhere::Optimize` 在 `dirty_txn && contains_fts(plan)` 时立即报错；否则 `resolve_where` 以后序方式先处理子树。它只处理恰有一个 DataSource 子节点的 Selection，并只取第一个能被 `interpret_fts_expression` 解析的条件。
3. `interpret_fts_expression` 要求函数名大小写不敏感地等于 `FTS_MATCH_WORD`、表达式以右括号结束、实参能在引号感知下拆成两项、查询是单引号字面量、列名非空且不含额外逗号。`parse_query_literal` 将 SQL 风格的 `''` 还原为单引号；列名两侧反引号会被去掉。
4. `find_matching_full_text_index` 只匹配 `columns.len() == 1` 且唯一列名与查询列名相等的索引。成功后 DataSource 的 `access_object` 改为索引名，`operator_info` 写入 `NoScore`、`top_k = u32::MAX` 的载荷；Selection 删除该 FTS 条件。非根 Selection 若变空会被其 DataSource 子节点替换，根 Selection 则保留空条件节点。
5. `resolve_top_n` 同样先递归子树，再只检查 TopN 的第一个排序项。其子树必须紧邻 DataSource，或只隔一个 Selection。排序 FTS 的列和查询文本必须与已编码载荷一致；成功后首项改为 `_FTS_SCORE`（保留 DESC），载荷切换为 `WithScore`。只有不存在中间 Selection、第一项为 DESC、且排序项总数为一时，才把 Top-K 收紧为 `min(offset.saturating_add(count), u32::MAX)`。
6. `resolve_projection` 在 Projection 子树中沿单子节点的 Selection/TopN 链寻找已编码的 DataSource。Projection 的 `operator_info` 若是匹配的 FTS 表达式，则校验列和查询文本，切换为 `WithScore`，并把投影文本改成 `_FTS_SCORE`。
7. `reject_remaining` 对节点类型作最后检查：Projection 区分裸 FTS 与表达式包裹；UnionScan 和其紧邻 Selection 视为脏事务形态；Selection 优先识别非常量查询；TopN 区分 FTS 是否位于首个排序项；Sort 明确拒绝无 LIMIT 的 FTS 排序。检查完成后递归子节点。

## 数据与状态

解析过程拥有并返回 `PlanNode`，通过 `&mut PlanNode` 原地改写局部树。关键状态跨阶段存放在 DataSource 的两个通用字符串字段中：`access_object` 保存命中的索引名，`operator_info` 保存编码后的 `FTSPushDown`。`PlanNode` 和 `PlanKind` 定义在 `pkg/planner/core/common_plans.rs`；本文件实际读取或改写的变体是 DataSource、Selection、TopN、Projection、UnionScan 和 Sort。

载荷编码依次包含 marker、索引名、列名、查询文本、查询类型和十进制 Top-K。`decode` 要求字段数量恰好为六、marker 和查询类型合法、Top-K 可解析；否则返回 `None`。当前编码没有转义机制，因此若索引名、列名或查询文本自身含 U+001F，解码会失败；调用方不能把 `None` 与“从未下推”之外的损坏原因区分开。

`changed` 由各递归函数对所有子树的结果做逻辑或；匹配节点被改写时返回 `true`。`ResolveFullTextPlan` 只返回最终计划并丢弃各阶段的 `changed` 标志。文件没有全局可变状态，也没有缓存；常量和只读索引切片可在调用之间复用。

## 依赖与调用关系

上游链路为 `pkg/session/fts_runtime.rs::FtsSession::plan_select` → `pkg/planner/core/fts_plan_builder.rs::BuildFullTextPlan` → `ResolveFullTextPlan`。会话层提供 `dirty_txn` 和从表元数据抽取的 `Vec<FullTextIndex>`。`pkg/planner/core/lib.rs` 负责将入口导出给 session crate 使用。

文件内部主调用链为：

- `ResolveFullTextPlan` → 四个 resolver 的 `Optimize`；
- WHERE `Optimize` → `contains_fts` / `resolve_where` → `interpret_fts_expression`、`find_matching_full_text_index`、`FTSPushDown::encode`；
- TopN `Optimize` → `resolve_top_n` → `split_order_direction`、`interpret_fts_expression`、`adjacent_data_source_mut`、`FTSPushDown::decode/encode`；
- Projection `Optimize` → `resolve_projection` → `interpret_fts_expression`、`find_push_down_mut`、`FTSPushDown::decode/encode`；
- Reject `Optimize` → `reject_remaining` → `interpret_fts_expression`、`contains_fts_text`、`contains_non_constant_fts_query`。

crate manifest 没有为本模块单独设置 feature；`nextgen` 是 crate 级 feature。本文件自身仅直接导入同 crate 的 `PlanKind` 和 `PlanNode`，不直接依赖外部 crate。运行时是否具备 TiFlash 副本由相邻的计划构造/会话路径检查，而非本文件判断。

## 错误处理与边界

公开优化入口均使用 `Result<..., String>`，没有结构化错误类型。明确报错的边界包括：dirty transaction、没有匹配的单列全文索引、ORDER BY 或 SELECT 与 WHERE 的查询/列不一致、SELECT 中裸用或包裹 FTS、多个或嵌套 FTS 条件、非常量查询文本、FTS 不是首个 ORDER BY 项，以及无 LIMIT 的 Sort。

不符合某一阶段预期树形时，多数解析函数返回“未改变”而非立即报错，例如 TopN 子节点不相邻、Projection 链上出现其他算子、Selection 不是 Selection→DataSource。此设计依赖最后的 `reject_remaining` 捕获仍可见的 FTS 文本；若通用字符串字段不再保存原表达式，必须重新评估该保障。

解析器是轻量文本解析而非完整 SQL expression parser：只识别单引号常量和简单列名，大小写只对函数名/排序方向宽松，索引列匹配是字符串精确比较。`split_function_arguments` 能跳过字符串中的逗号和成对单引号，但不解析任意嵌套表达式。WHERE 每次只消费第一个可解释条件，剩余 FTS 由 Reject 阶段报错。`unreachable!()` 仅用于同一函数内已用模式匹配建立的不变量。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄、网络连接或事务。事务生命周期由 session 层拥有，本文件只接收已经计算好的 `dirty_txn: bool`；它不会自行读取或提交事务。

计划和字符串均按 Rust 所有权管理。`Optimize` 接收并返回拥有所有权的 `PlanNode`；递归改写借用子节点，空的非根 Selection 通过 `children.remove(0)` 把唯一子节点移动到当前位置。索引匹配会 clone 一个 `FullTextIndex`，随后将其中字符串移动进下推载荷。执行完成后无额外资源需要清理。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/fts_resolve_index.go`，行为测试是 `pkg/planner/core/fts_resolve_index_test.go`。Rust 保留了 Go 的四条规则及顺序、规则名、仅匹配单列全文索引、评分模式切换、DESC 单项 TopN 优化、脏事务限制和主要错误文案；`pkg/planner/core/fts_resolve_index_test.rs` 也覆盖这些对应行为。

两者不是数据结构级等价实现。Go 在真实 `base.LogicalPlan`、typed `expression.Expression`、`model.IndexInfo` 和 `tipb.FTSQueryInfo` 上工作，并检查索引具有 `FullTextInfo`、状态公开、列 ID 匹配；它还写入 index ID、tokenizer、函数签名，并给 DataSource schema/columns 追加 `_FTS_SCORE` 虚拟列。Rust 在轻量 `PlanNode` 的字符串字段上工作，只以索引列名匹配并编码五项业务字段，不验证索引公开状态或 parser type，也不追加 typed score column。

Go 通过 statement context 的 `FTSFunctionIsUsed` 跳过无 FTS 计划，Rust 则直接遍历计划文本；Go 的 visitor 保存任意父链，Rust 对 TopN/Projection 只接受明确的窄树形。Go Projection 可改写多个投影表达式，Rust 的 `PlanNode::Projection` 只有一个 `operator_info` 字符串。因此新增行为时不能仅凭相同规则名假设两端已经完全对齐，必须逐项比较树形、元数据和错误顺序。

## 扩展指南

- 扩展表达式语法时，优先修改 `split_function_arguments`、`parse_query_literal` 和 `interpret_fts_expression`，并在独立测试 `pkg/planner/core/fts_resolve_index_test.rs` 增加引号、逗号、标识符、嵌套和非常量边界；不要把测试嵌入生产源文件。
- 扩展多列全文索引或索引选择规则时，修改 `FullTextIndex` / `find_matching_full_text_index`，同时核对 Go 的 `findMatchingFullTextIndex` 对公开状态、列 ID 和 `FullTextInfo` 的约束，避免只按名称造成错误匹配。
- 改变下推载荷时，应同步 `FTSPushDown::encode/decode` 和所有消费者。建议先定义版本或可靠转义策略；否则新字段、分隔符碰撞和旧计划兼容都可能让 `FullTextPushDown` 静默返回 `None`。
- 支持新的中间算子树形时，分别检查 `adjacent_data_source_mut`、`find_push_down_mut` 和 `reject_remaining`。放宽遍历范围可能把评分关联到错误的扫描，尤其在多子树、多个 DataSource 或多个 FTS 查询场景中。
- 修改 Top-K 规则时保持 `offset + count` 溢出和 `u32` 截断保护，并与 Go 的优化条件逐项核对；放宽条件可能改变 TiFlash 返回量及排序正确性。
- 调整生产入口或四阶段顺序时，同步 `ResolveFullTextPlan`、session 的 `plan_select` 以及 Rust 独立测试；Reject 必须保持在消费规则之后。用户可见语义还应对照 Go 集成测试 `pkg/planner/core/fts_resolve_index_test.go`。
- 性能风险主要来自对每个阶段整树递归和重复字符串大小写转换/编码；正确性风险主要来自字符串计划模型、树形限制及载荷分隔格式，兼容风险主要来自与 Go 错误顺序和元数据字段的漂移。

## 验证依据

- 目标源码：`pkg/planner/core/fts_resolve_index.rs`，核对全部 557 行、公开/私有符号、四阶段流程及错误分支。
- crate 与模块装配：`pkg/planner/core/Cargo.toml`、`pkg/planner/core/lib.rs`；确认 crate 名、feature、模块声明、再导出和独立测试模块。
- 数据模型：`pkg/planner/core/common_plans.rs` 的 `PlanKind`、`PlanNode`，确认本文件实际操作的节点变体和通用字段。
- 直接入口：`pkg/session/fts_runtime.rs::FtsSession::plan_select`，确认 `BuildFullTextPlan` 后以全文索引列表及 dirty 状态调用 `ResolveFullTextPlan`。
- Rust 独立测试：`pkg/planner/core/fts_resolve_index_test.rs`，覆盖规则名、单列索引、引号转义、根/非根 Selection、dirty transaction、TopN/Projection、查询不一致、UnionScan、残留用法、多谓词和参数化查询。
- Go 对照与测试：`pkg/planner/core/fts_resolve_index.go`、`pkg/planner/core/fts_resolve_index_test.go`，核对 visitor、typed 元数据、规则顺序、Top-K 条件、错误语义和集成场景。
- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件和 4,415 个 Go 文件；`node --file` 读取目标文件与 session 入口；`query ResolveFullTextPlan` 定位入口；文件限定 `callees` 核对 `plan_select`、`resolve_where`、`resolve_top_n`、`resolve_projection`、`reject_remaining` 的直接调用边。未限定的 `callers/callees` 两次超时且无输出，因此调用者另以仓库搜索 `ResolveFullTextPlan(` 核实，唯一生产调用位于 `pkg/session/fts_runtime.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务给定命令验证目标文档存在且恰有十一个固定二级章节，并人工复核所有“当前支持”结论均可回溯到上述源码或测试。
