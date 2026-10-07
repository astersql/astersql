# `pkg/planner/core/common_plans.rs`

## 文件定位

[`common_plans.rs`](common_plans.rs) 属于 `astersql-planner-core` crate。该 crate 的入口 [`lib.rs`](lib.rs) 以私有模块 `mod common_plans` 装配本文件，再用 `pub use common_plans::*` 将其中的公开符号提升为 crate API；因此外部 crate 使用的是 `astersql_planner_core::{PlanNode, Explain, ...}`，而不是直接访问模块路径。[`Cargo.toml`](Cargo.toml) 将 crate 根设为 `lib.rs`、关闭自动测试发现，并声明 `nextgen` feature；本文件自身没有条件编译项，唯一直接的外部类型依赖是 `parser_ast_dependency::{FieldsClause, LinesClause}`，其余导入来自同一 crate 的扁平计划与 EXPLAIN 渲染层。

它同时承担两类职责。第一类是可执行规划骨架：`PlanKind`、`PlanNode`、`PlannerContext` 和 `SessionVars` 为 Rust 规划、计划变换、扁平化及展示提供一套轻量计划树。第二类是与 Go `pkg/planner/core/common_plans.go` 对齐的语句计划数据结构及帮助函数，例如 ANALYZE、LOAD DATA、DDL 管理、EXPLAIN JSON 和自动提交点查判定。大量字段目前以 `String`、`CIString` 或轻量集合表示，说明这是迁移边界上的共享模型，不等同于 Go 版本完整的强类型计划体系。

## 核心职责

1. 用 `JoinType`、`StoreType` 和 `PlanKind` 描述计划节点类别、Join 语义以及执行落点；`PlanKind::name` 给 EXPLAIN/字符串化提供稳定显示名，并特别区分普通 `UnionAll` 与 `PartitionUnionAll`。
2. 用递归的 `PlanNode` 保存子树、估算行数与代价、访问对象、运行时行数、内存/磁盘信息、probe 次数和 build side；`New` 建立一致默认值，`MemoryUsage` 递归估算计划树占用，`IsPhysical` 排除当前模型中仅有的逻辑 `DataSource` 与 `Join`。
3. 为 SHOW、DDL 管理、PREPARE/EXECUTE、SQL Binding、ANALYZE、LOAD DATA/IMPORT INTO、统计、Region、DDL 和 SELECT INTO 等语句保存构建阶段需要的参数。`schema_plan!`、`table_index_plan!`、`ddl_jobs_plan!` 和 `path_plan!` 仅消除重复结构声明，不引入运行时分派。
4. 将 AST 的 `FieldsClause`/`LinesClause` 归一化为 `LineFieldsInfo`，并维持 MySQL/TiDB 兼容默认值。
5. 将递归 EXPLAIN 信息编码成符合 Go `json.Encoder` 字段名、`omitempty`、缩进和尾随换行契约的 JSON；将计划树扁平化后路由到普通格式或 RU 格式渲染。
6. 提供事务快速路径判定：只有自动提交且未进入显式事务时，才继续判断计划是否是当前轻量模型可识别的主键/唯一键点查。

## 主要符号

- `JoinType`：七种 Join 语义，包括半连接、反半连接、左右外连接和内连接。它被 `PlanKind::MergeJoin` 等计划元数据使用。
- `StoreType::{Root, TiKV, TiFlash}`：执行位置枚举；默认是 `Root`，随后由计划构建或下推阶段覆盖。
- `PlanKind`：约四十种逻辑/物理或语句算子。带数据的变体保存最小展示或变换信息，例如 `HashJoin.inner_child`、`Limit.{offset,count}`、`TopN.by_items`、`IndexMergeReader` 的 partial/table 子计划，以及 `Generic(String)` 兼容尚未建模的名称。
- `PlanNode::{New, MemoryUsage, IsPhysical}`：递归计划树及其基础操作。`New` 将 `probe_count` 设为 `1.0`、`store_type` 设为 `Root`；`MemoryUsage` 计入节点静态大小、全部子树、`access_object` 和 `operator_info` 的字符串长度，但不是堆内存精确核算；`IsPhysical` 只按 `PlanKind` 判断。
- `SessionVars` 与 `PlannerContext`：前者保留自动提交、事务状态、受限 SQL、表名大小写以及脏表/快照表集合；后者组合会话变量与算子计数。集合使用 `BTreeSet`，计数使用 `Vec<u64>`。
- `SchemaProducer` 及管理计划结构：`ShowDDL`、`CheckTable`、`AlterDDLJob`、`Prepare`、`Execute`、`SetConfig`、`SQLBindPlan` 等主要是数据载体。`allowedAlterDDLJobParams` 每次构造仅含 `thread`、`batch_size`、`max_write_speed` 的有序集合。
- ANALYZE 组：`AnalyzeInfo`、`V2AnalyzeOptions`、`AnalyzeColumnsTask`、`AnalyzeIndexTask` 与 `Analyze` 分别保存目标表、V2 选项、列/索引任务及汇总选项。
- 导入导出组：`LoadData`、`ImportInto`、`SelectInto`、`LineFieldsInfo` 与 `NewLineFieldsInfo` 保存路径、目标表、初始化计划、选项及字段/行格式。
- `ExplainInfoForEncode`、`json_escape`、`encode_explain`、`JSONToString`：构成无 serde 依赖的专用 JSON 编码器；`ExecuteInfo`/`SubOperators` 是当前字段，`ExecutionInfo`/`Children` 是旧 Rust 调用方兼容别名。
- `Explain::{SetRUResult, RenderResult}`：保存目标计划、格式、分析标记、结果行和 occurrence-aligned RU 结果。`SetRUResult(None)` 也会把 `RUResultSet` 置为真，以记录“计算尝试过但不可用”，阻止旧值泄漏。
- `GetBriefBinaryPlan`、`GetExplainAnalyzeRowsForPlan`：前者把计划的 `ToString` 字节逐字节转为小写十六进制，后者触发渲染后克隆行结果；后者有意忽略渲染错误，因此不适合作为需要错误诊断的新入口。
- `IsAutoCommitTxn`、`IsPointGetWithPKOrUniqueKeyByAutoCommit`：快速路径守卫。点查函数支持单 range 的 `IndexScan`、名为 `PointGet`/`BatchPointGet` 的 `Generic`，以及一层层递归穿透 `Projection`/`Lock` 的首个子节点。

## 执行流程

计划主链从构建或反序列化开始。`pb_to_plan.rs::PBPlanBuilder::build_executor` 和其他 planner 文件调用 `PlanNode::New` 产生节点，随后填充 store、代价和展示字段；`plan.rs` 根据 `PlanKind::Window`、`StreamAgg`、`MergeJoin` 等形态包入 Shuffle sender/receiver；`flat_plan.rs::FlattenPhysicalPlan` 遍历 `PlanNode.children`，根据 Join、Reader、CTE 等种类生成展示所需的扁平森林。`PlanNode` 因此是构建、局部优化和 EXPLAIN 之间的公共数据面，而不是完整优化器 trait 层的替代品。

`NewLineFieldsInfo` 的流程是先写入字段制表符、反斜杠转义和换行终止等默认值，再分别读取可选的 `FieldsClause` 与 `LinesClause`。只有 AST 字段为 `Some` 时才覆盖对应字符串；`OptEnclosed` 在 fields 子句存在时直接复制。Rust 测试从真实 LOAD DATA SQL 解析 AST 后验证默认值及每一种显式覆盖。

`JSONToString` 对每个根行调用 `encode_explain`。编码器固定输出 `id`、`estRows`、`taskType`，对其余字段实施空字符串省略；`ExecuteInfo` 为空时回退到旧别名 `ExecutionInfo`，`SubOperators` 为空时回退到 `Children`。子节点递归增加缩进，`json_escape` 处理引号、反斜杠、常用控制字符、U+2028/U+2029 及 U+001F 以下字符，最终数组后保留换行。空输入返回 `[]\n`。

`Explain::RenderResult` 首先要求 `TargetPlan` 存在，再调用 `FlattenPhysicalPlan(Some(target), false)`。格式大小写无关地等于 `ru` 时，传入已保存的 `ExplainRUResult`；否则调用 `ExplainFlatPlanInRowFormat` 并同时传递格式与 `Analyze`。成功结果写回 `Rows`。RU 结果按扁平森林中的 occurrence 对齐，同 ID 出现在主树、CTE 或标量子查询时不会互相覆盖；测试还验证无效 operator 或显式清空结果时不泄漏旧 RU 值。

点查快速路径先调用 `IsAutoCommitTxn`。不满足 `autocommit && !in_txn` 立即返回 false；满足后按计划种类判断，并只对 `Projection`/`Lock` 的第一个子节点递归。空包装节点安全返回 false。

## 数据与状态

本文件的计划对象均为拥有所有权的 Rust 值：树边使用 `Vec<PlanNode>`/`Box<PlanNode>`，可选运行时数据使用 `Option`，没有裸指针或内部可变性。`PlanNode` 的估算字段和执行字段可在构建、优化、执行统计回填阶段原地更新；`Explain.Rows` 也是每次渲染覆盖，而不是追加。

状态不变量包括：`PlanNode::New` 生成 Root 节点且默认 probe 次数为一；`StoreType::default()` 同样为 Root；`Explain::SetRUResult` 无论参数是否为空都设置 `RUResultSet`；JSON 当前字段优先于兼容别名；`NewLineFieldsInfo` 的默认字段分隔、转义和行终止分别为 `\t`、`\\`、`\n`。`BTreeMap`/`BTreeSet` 用于选项、表集合和允许参数，使迭代结果稳定，但文件没有依赖这种顺序实施业务分支。

若估算内存，必须注意 `PlanNode::MemoryUsage` 只递归计入 `children` 并加两个字符串的字节长度；它没有把 `cost_formula`、`fd`、`execution_info`、`PlanKind` 内部字符串/向量及 `Vec` 容量逐项计入，因此只能视为现有兼容算法，不可解释为精确分配量。

## 依赖与调用关系

上游装配点是 `lib.rs`，它公开再导出本文件符号，并在 `#[cfg(test)]` 下以独立文件装配 `common_plans_test.rs`。主要直接使用关系如下：

- `pb_to_plan.rs` 从 protobuf executor 构造 `PlanNode`/`PlanKind`，并使用 `PlannerContext`；`plan.rs` 消费并重写这棵树；`optimizer.rs`、`stats.rs`、`runtime_filter_generator.rs`、`fts_plan_builder.rs` 等也共享该模型。
- `flat_plan.rs` 读取 `PlanKind`、`PlanNode`、`StoreType`，生成 `FlattenPhysicalPlan` 所返回的森林；本文件的 `Explain::RenderResult` 又反向调用该扁平化和格式化 API，构成“树模型 -> 扁平展示模型 -> 行结果”的闭环。
- `encode.rs`、`hint_utils.rs`、`telemetry.rs` 和 `stringer.rs` 分别消费计划节点做编码、提示/摘要、遥测和文本表示；`GetBriefBinaryPlan` 通过 crate 内的 `ToString` 进入字符串化实现。
- `pkg/executor/explain_test.rs` 直接从 crate 公共 API 使用 `ExplainInfoForEncode` 与 `JSONToString`，证明 JSON 契约跨越 planner/executor 边界。
- `parser_ast_dependency` 提供 `FieldsClause` 与 `LinesClause`；`CIString`、`InitializedPlan`、扁平计划类型与 EXPLAIN 格式化函数均通过 crate 根导入。本文件未直接创建线程、访问存储或执行 SQL。

RustCodeGraph 索引确认目标文件含 138 个符号并被至少六个文件使用；精确 `callers` 查询在本次环境中未完成，因此上述调用边由索引的文件使用关系与逐符号 `rg` 结果交叉验证，不把未返回的图边作为已证实事实。

## 错误处理与边界

`Explain::RenderResult` 是主要可失败入口：没有目标计划返回 `Err("explain target plan is missing")`；扁平化返回空则返回 `Err("cannot flatten an empty plan")`。格式化函数本身在当前签名中不返回错误。相反，`GetExplainAnalyzeRowsForPlan` 丢弃 `RenderResult` 的错误并返回当前 `Rows` 克隆，调用者若需要区分“成功但零行”和“渲染失败”应直接调用 `RenderResult`。

`JSONToString` 返回 `String` 而不是 `Result`，因为专用编码只拼接内存字符串；它必须持续覆盖所有需要的 JSON 转义。兼容别名的优先级意味着若新旧两个字段都非空，只有 `ExecuteInfo` 和 `SubOperators` 生效。`PlanKind::Generic` 的名字原样用于展示，也参与点查字符串判断，新增泛型名字时不能假设自动获得专门语义。

点查判定的 Rust 实现比 Go 窄且抽象层不同：单个 `IndexScan` range 只检查数量，不验证 nullable、组合主键长度、unique double-read 或缓存表限制；`Generic("PointGet")` 也没有携带这些信息。因此该函数只能按当前 Rust 轻量模型的契约使用，不能据其结果推导 Go 实现中的全部一致性保证。包装节点只读取第一个 child，空 child 返回 false，多 child 的其余分支被忽略。

`NewLineFieldsInfo` 依赖 parser 保证 enclosed/escaped 的语法长度约束；Rust 函数自身不重复验证。`allowedAlterDDLJobParams` 只提供允许名称集合，不校验值的类型、范围或单位。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、文件句柄或网络资源；所有 API 都是同步、内存内操作。`Clone` 广泛派生在计划与 EXPLAIN 数据上，克隆 `PlanNode` 会深拷贝整个子树，克隆 `Explain.Rows` 或 `ExplainRUResult` 也会复制容器，因此大计划上的无必要克隆有内存与延迟成本。

`Explain::SetRUResult` 通过值移动取得 RU 快照所有权，随后 `RenderResult` 只借用它；这使结果与一次扁平计划 occurrence 对齐，无需共享锁。调用者若在其他线程并发读写同一个 `Explain`，必须在本文件之外提供同步。递归函数 `MemoryUsage`、`encode_explain` 和点查包装穿透的栈深与计划/JSON 树深成正比；当前没有深度限制或迭代退化保护。

## 与 Go 版本的对应关系

直接对照文件是 [`common_plans.go`](common_plans.go)，直接 Go 测试是 [`common_plans_test.go`](common_plans_test.go)。Rust `NewLineFieldsInfo` 与 Go 版本的默认值、可选字段覆盖顺序一致；Rust/Go 测试使用相同七类 LOAD DATA 场景。`ExplainInfoForEncode` 的 JSON 字段名和省略规则与 Go struct tag 对齐，Rust 测试额外锁定了缩进、转义和尾随换行。

Rust `ExplainRUResult` 及 `SetRUResult` 保留 Go 的 occurrence ownership 与清空旧快照语义；独立 Rust 测试覆盖主树、CTE、标量子查询中重复 ID、无效 operator 和 `None` 清空。Rust `RenderResult` 只覆盖“扁平化后按 RU 或普通行格式输出”的子集；Go 版本还处理 EXPLAIN EXPLORE、true-cardinality cost、binary plan、runtime stats、schema 和更多格式错误，因此不能把 Rust 结构视为 Go `Explain` 的完整移植。

管理计划也多为形状对齐而非类型等价：Go 使用 `table.Table`、`model.TableInfo`、AST 节点、真实 session context 和 physical plan interface，Rust 多处暂用 `String`、`CIString`、`InitializedPlan` 或本文件的 `PlanNode`。例如 `DDL.Statement` 在 Rust 是字符串，而 Go 是 `ast.DDLNode`；`SplitRegion.TableInfo` 在 Rust 是字符串，而 Go 是 `*model.TableInfo`。

`IsAutoCommitTxn` 的布尔意图一致；`IsPointGetWithPKOrUniqueKeyByAutoCommit` 则是收窄迁移。Go 能识别 `PhysicalIndexReader`、`PhysicalTableReader`、`PointGetPlan` 并验证 range、common handle、二次读取和 cache table，Rust 只依据轻量 `PlanKind`。后续对齐必须补足模型信息和独立测试，不能仅增加一个名字匹配分支。

## 扩展指南

- 新增计划算子时，首先扩展 `PlanKind` 及 `PlanKind::name`，再检查 `PlanNode::IsPhysical`、`flat_plan.rs` 的 child label/reader/CTE 规则、`stringer.rs`、`encode.rs`、`plan.rs` 和所有穷举匹配；测试应放在对应独立 `*_test.rs`，不要内嵌进源文件。
- 扩展 EXPLAIN 字段时，同步修改 `ExplainInfoForEncode`、`encode_explain` 的固定顺序/省略规则、Go struct tag 契约以及 `common_plans_test.rs` 和 `pkg/executor/explain_test.rs`。若引入通用 JSON 类型，仍要保留 `SetEscapeHTML(false)` 等价行为与尾随换行。
- 扩展 RU 渲染时应维持 occurrence 对齐和 fail-closed 语义：同计划 ID 的不同出现位置不得合并，失败重算必须清除旧快照。同步覆盖主树、多个 CTE、多个标量子查询和无效 operator。
- 扩展 LOAD DATA/SELECT INTO 格式时，修改 `LineFieldsInfo` 与 `NewLineFieldsInfo`，并在独立 Rust/Go 测试中同时覆盖默认值、单项覆盖和组合子句；parser 已有约束仍应在文档或类型边界注明。
- 对齐点查快速路径时，先给 `PlanKind`/`PlanNode` 增加表达 unique、nullable、common handle、double-read 与 cache 状态所需的数据，再移植 Go 判定和回归用例。只扩大 `ranges.len() == 1` 会产生错误的快速路径资格。
- 若新增大容量字段，应同步评估 `MemoryUsage`，明确计入长度还是容量；深树处理若进入不可信输入边界，需要评估递归深度和克隆成本。

兼容风险主要在公开再导出的字段名和 Go 风格方法名，改变它们会影响 crate 外部调用者；正确性风险集中在点查资格、RU 结果复用和 JSON 契约；性能风险集中在深递归、计划深拷贝和不完整的内存估算。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件；`files --filter pkg/planner/core/common_plans.rs` 确认目标文件被索引；`node --file ... --symbols-only` 列出本文件 138 个符号；分段 `node --file ... --offset/--limit` 读取了全部 911 行。`query` 分别定位 `PlanNode`、`NewLineFieldsInfo`、`JSONToString`、`RenderResult` 与点查函数。`callers NewLineFieldsInfo --file ...` 在 60 秒无输出后中止，调用边改由精确源码搜索补证。
- 源与边界：完整读取 `pkg/planner/core/common_plans.rs`；读取 `pkg/planner/core/lib.rs` 的模块声明、公开再导出和测试装配；读取 `pkg/planner/core/Cargo.toml` 的 crate 根、feature、依赖与 porting 元数据。目标包不存在 `doc.go`。
- 调用关系：检查 `pb_to_plan.rs`、`plan.rs`、`flat_plan.rs`、`encode.rs`、`hint_utils.rs`、`telemetry.rs`、`stringer.rs`、`fts_plan_builder.rs` 和 `pkg/executor/explain_test.rs` 中对目标符号的直接引用。
- Go 对照：读取 `pkg/planner/core/common_plans.go` 中 LineFields、JSON、Explain/RU 与 point-get 实现，并读取 `pkg/planner/core/common_plans_test.go` 的对应测试；确认完整 Go EXPLAIN/point-get 能力超出当前 Rust 实现。
- Rust 测试：完整读取 `pkg/planner/core/common_plans_test.rs`，并读取 `pkg/planner/core/tests/pointget/point_get_plan_test.rs` 的直接点查用例。测试覆盖默认/覆盖字段格式、JSON 编码、RU 路由、occurrence ownership 与旧值清除；本任务按计划只做文档分析，没有运行 Cargo。
- 人工复核：本文明确回答了文件存在目的、计划/EXPLAIN 主流程、拥有状态、失败边界、Go 迁移差异以及安全扩展入口，且未把字符串占位或简化判定描述为完整 Go 能力。
