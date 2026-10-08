# `pkg/util/hint/hint_processor.rs`

## 文件定位

[`hint_processor.rs`](./hint_processor.rs) 属于 `astersql-util-hint` crate。该 crate 由 [`Cargo.toml`](./Cargo.toml) 声明，以 [`lib.rs`](./lib.rs) 为入口；`lib.rs` 公开 `hint_processor` 模块并再导出其 API。因此，本文件位于 SQL parser 产出的 AST 与 planner、session、SQL binding 消费的 hint 表示之间，负责把优化器 hint 从 AST 中抽取为稳定的中间集合、把集合按原遍历顺序写回 AST、恢复为 SQL 文本，并规范化 binding SQL 中的查询块名。

生产接线可在以下位置直接复核：

- [`pkg/session/hint_runtime.rs`](../../session/hint_runtime.rs) 的 `StartStatementHints` 从当前语句抽取 table hint，解析成语句运行期配置；同文件的 binding 匹配路径用 `ParseHintsSet` 解析绑定 SQL。
- [`pkg/planner/optimize.rs`](../../planner/optimize.rs) 的 `optimizeNoCacheInner` 抽取 hint 后安装到规划上下文；`containUsePlanCacheHintInSQLOrBinding` 用递归查找入口判断计划缓存 hint。
- [`pkg/bindinfo/binding.rs`](../../bindinfo/binding.rs) 的 `prepare_hints_inner` 调用 `ParseHintsSet`，再通过 `HintsSet::Restore` 生成 binding ID 并保存规范化 hint。
- [`pkg/session/runtime/explain_select.rs`](../../session/runtime/explain_select.rs) 与 [`pkg/session/runtime/explain_query.rs`](../../session/runtime/explain_query.rs) 解析匹配到的 binding SQL，使 EXPLAIN 使用其 hint 或规范化查询块 offset。

本文件不是 parser：语法解析由 `astersql-parser` 提供；查询块名称的建立和映射由相邻的 [`hint_query_block.rs`](./hint_query_block.rs) 负责。

## 核心职责

1. `HintsSet` 保存按 AST 遍历次序分组的 table optimizer hint 和 index hint，并提供语句级筛选、包含判断和文本恢复。
2. `ExtractTableHintsFromStmtNode`、`ContainTableHintInStmtNode` 处理 SELECT、UPDATE、DELETE、INSERT 与集合运算的语句级查询；INSERT 会合并其 SELECT 侧的语句级 hint，并可报告 `memory_quota` 冲突。
3. `RestoreTableOptimizerHint` 与 `RestoreIndexHint` 将结构化 AST hint 恢复成小写 SQL 片段，涵盖查询块、表、分区、索引、数值、布尔、字符串、存储引擎和 `LEADING` 嵌套列表等参数形态。
4. `CollectHint` 与 `BindHint` 构成互逆的顺序协议：前者深度优先收集，后者按相同路径和计数器消费并回写。
5. `ParseHintsSet` 用真实 parser 解析一条 binding SQL，运行查询块处理器，校验查询块引用，补齐缺省数据库名，并只保留相关 parser 警告。
6. `CheckBindingFromHistoryComplete` 对历史计划自动生成的 binding 做保守筛查：TiFlash、子查询以及达到三张不同表的 join 会被视为可能不完整。

## 主要符号

- `supportedHintNameForInsertStmt() -> HashSet<&'static str>`：当前只返回 `memory_quota`，限定 INSERT 与 SELECT 两侧需要检查冲突的 hint 范围。
- `HintsSet { tableHints, indexHints }`：公开的数据载体。`tableHints` 的外层下标是 SELECT/UPDATE/DELETE 查询块的遍历次序；`indexHints` 的外层下标是查询块内真实 `TableName` 的遍历次序。
- `HintsSet::GetStmtHints`：首块全部保留；后续块只保留 `isStmtHint` 判定的语句级 hint。
- `HintsSet::ContainTableHint`：比较 `HintName.O`，因此保留原始大小写语义；与内部按 `HintName.L` 比较的 `containTableHint` 不同。
- `HintsSet::Restore`、`RestoreOptimizerHints`、`RestoreTableOptimizerHint`、`RestoreIndexHint`：分别恢复完整集合、去重的 table hint 列表、单个 table hint 和单个 index hint。
- `ExtractTableHintsFromStmtNode`、`ContainTableHintInStmtNode`、`checkInsertStmtHintDuplicated`：语句级提取、递归包含判断与 INSERT 冲突告警入口。
- `hintProcessor`：收集/绑定共享状态，持有 `HintsSet`、`tableCounter`、`indexCounter` 与嵌套 `blockCounter`。`bindHint2Ast` 由 `BindHint` 置为 `true`，但当前手写的 `collectNode`/`bindNode` 已分别固定模式，不再读取该字段；它是与 Go 统一 visitor 结构对应的兼容状态。
- `collectNode` / `bindNode`：分别实现 SELECT、UPDATE、DELETE、INSERT、`SetOprStmt` 的深度优先遍历，是顺序不变量的核心。
- `collectExprQueries` / `bindExprQueries`：通过 `ExprNodeVisitor` 找到表达式中的 `Subquery`，使表达式子查询参与同一收集/绑定序列。
- `CollectHint` / `BindHint`：公开的集合收集与 AST 回写入口。
- `ParseHintsSet` / `extractHintWarns`：binding SQL 解析、查询块规范化与 hint 警告筛选入口。
- `NodeType` / `nodeType4Stmt`：将顶层语句映射到 SELECT、UPDATE、DELETE 命名空间；INSERT 按 SELECT 处理。
- `bindableChecker` / `CheckBindingFromHistoryComplete`：历史 binding 完整性检查器及公开入口。

## 执行流程

收集流程由 `CollectHint` 创建空 `hintProcessor` 后调用 `collectNode`：

1. 遇到 SELECT、UPDATE 或 DELETE，先把该块的 `TableHints` 推入 `HintsSet.tableHints`，再增加 `blockCounter`。
2. 依语法顺序遍历 WITH/CTE、字段、FROM join、过滤表达式、分组、HAVING、窗口、排序、LIMIT、锁表或 RETURNING 等位置。表达式中的 `Subquery` 会递归回到 `collectNode`。
3. `collectTableSource` 对派生表只进入 `QuerySource`，不会为派生表虚构一个 `TableName`；对真实表才通过 `collectTableName` 记录 index hint。只有 `blockCounter > 0` 时记录，因而 INSERT 目标表不会被误算为查询块内索引 hint。
4. 离开 SELECT/UPDATE/DELETE 时减少 `blockCounter`。INSERT 先遍历 SELECT，再处理目标表、VALUES、ON DUPLICATE 与 RETURNING 表达式；集合运算按 statement WITH、select-list WITH、各分支、排序和 LIMIT 的顺序遍历。

绑定流程由 `BindHint` 把给定集合放入处理器后调用 `bindNode`。它复刻上述遍历顺序：`nextTableHints` 依 `tableCounter` 取下一块，`bindTableName` 依 `indexCounter` 取下一组索引 hint；集合长度不足时写入空列表，长度超出时剩余项不会被消费。表达式子查询通过取出 `NodeRef` 中的节点、递归绑定、再构造引用的方式更新。由此，`HintsSet` 并不携带节点标识，正确性依赖收集与绑定 AST 形状及遍历次序一致。

`ParseHintsSet` 的流程是：

1. 用调用方给出的 charset/collation 调用 `Parser::ParseSQL`；parser 错误直接返回。
2. 强制结果恰好一条语句，否则返回 `bind_sql must be a single statement`。
3. 对原 AST 执行 `CollectHint`，记录顶层 `NodeType`，再用 `NewQBHintHandler(None).Process` 生成和登记查询块信息。
4. 逐块规范化 table hint。SELECT 块当前 offset 为 `index + 1`；UPDATE/DELETE 顶层减一。裸 `qb_name`（没有 tables）只参与命名而不进入结果，视图 hint 保留原状。
5. 通过 `GetHintOffset` 和 `checkTableQBName` 校验引用；失败时返回包含恢复后 hint 文本的 unknown-query-block 错误。成功时用 `GenerateQBName` 写成规范名称，并给没有 DBName 的表补入参数 `db`。
6. 返回规范化 `HintsSet`、查询块处理后的 AST，以及 `extractHintWarns` 筛出的至多一个相关警告。

历史 binding 完整性检查先对 `hintStr` 做大小写敏感的 `"tiflash"` 子串检查；未命中时，`bindableChecker` 遍历支持的语句和 result-set 节点。发现子查询表达式或表集合达到三个元素后，保存固定原因并停止继续深入。

## 数据与状态

`HintsSet` 拥有克隆后的 `TableOptimizerHint` / `IndexHint`，不借用源 AST。它的两个二维向量都是有序协议而非按查询块名或表名索引的映射：同一集合只有绑定到遍历形状兼容的 AST 才有确定含义。

`hintProcessor` 的三个计数器含义不同：`tableCounter` 和 `indexCounter` 只在绑定时单调递增；`blockCounter` 同时服务于收集和绑定，用于判断当前 `TableName` 是否处于可记录的 SELECT/UPDATE/DELETE 查询块。所有状态均局限在单次函数调用栈中。

文本恢复过程中，`RestoreOptimizerHints` 用 `HashMap<String, ()>` 判断重复，同时用独立 `Vec` 保留首次出现顺序；不能依赖 `HashMap` 本身的迭代顺序。标识符由 `quoteName` 用反引号包裹并把内部反引号加倍；字符串由 `quoteString` 处理 NUL、换行、回车、反斜杠和单引号。`memory_quota` 的 `HintData::Signed` 被视为字节并以整数除法恢复为 MB。最终 table/index hint 文本统一转为小写。

`bindableChecker.tables` 保存 `(原始名, 小写名)` 元组。`visitTable` 保留了 Go 实现的判断次序：先用 schema 元组测试是否存在，再插入 table-name 元组；这不是按 `(schema, table)` 组成复合键，扩展或修正前必须先核对 Go 兼容预期和对应边界测试。

## 依赖与调用关系

crate 依赖由 [`Cargo.toml`](./Cargo.toml) 明确：`parser` 提供 AST、visitor、SQL parser 和 parser 错误；`dbterror` 与 `plannererrors` 提供错误遍历和冲突警告；`errno` 提供精确 warning code；`meta-model`、`types` 由 crate 入口为其他 hint 逻辑再导出，本文件不直接使用。

主要下游调用边经 RustCodeGraph 核对：

- `ExtractTableHintsFromStmtNode → extractTableHints → tableHints / warnInsertHintDuplicated / isStmtHint`。
- `RestoreOptimizerHints → RestoreTableOptimizerHint`；`HintsSet::Restore → RestoreTableOptimizerHint / RestoreIndexHint`。
- `CollectHint → collectNode`，后者继续进入 join、table source、CTE、各表达式位置和表达式子查询 visitor。
- `BindHint → bindNode`，后者与收集路径一一对应地调用绑定辅助函数。
- `ParseHintsSet → Parser::ParseSQL / CollectHint / nodeType4Stmt / NewQBHintHandler::Process / GetHintOffset / checkTableQBName / GenerateQBName / extractHintWarns`。
- `CheckBindingFromHistoryComplete → bindableChecker::visitNode`。

主要上游接线由 `rg` 补充确认：session 和 planner 调用提取/包含接口；bindinfo 与 EXPLAIN 路径调用 `ParseHintsSet`。当前仓库中 `CollectHint`、`BindHint` 和 `CheckBindingFromHistoryComplete` 的直接 Rust 调用主要出现在本 crate 的独立测试中；它们仍由 `lib.rs` 公开再导出，不能据此断言是私有或无用 API。

## 错误处理与边界

- `ParseHintsSet` 传播 parser、查询块名生成和 dbterror 错误；多语句输入显式失败。未知查询块名和表上的非法查询块引用合并为同一种错误路径。
- `extractHintWarns` 不按错误文本模糊匹配，而是用 `dbterror::errors::Find` 检查 parser RFC 前缀及六个精确错误码，或用 `parser::ErrParse.Equal` 检查解析错误；只返回第一个匹配项。普通文本中出现 “memory quota” 不会被误判。
- INSERT 重复检查只关注本文件声明支持的名字，当前为 `memory_quota`；没有 INSERT hint、没有 SELECT、没有同名项或没有 warning handler 时均静默返回。
- `setTableHints4StmtNode` 只处理 SELECT/UPDATE/DELETE；其他节点原样返回。`nodeType4Stmt` 对未支持节点返回 `TypeInvalid`。
- `RestoreTableOptimizerHint` 对 `HintData` 变体与 hint 名不匹配的情况通常输出空参数，而不是返回错误；这是一个宽容的格式化边界。`RestoreIndexHint` 的当前分支对所有已知枚举值都能生成字符串，但保留 `Result` 以匹配 crate/Go API。
- `BindHint` 对 hints 数量不足采用空列表，不报错；对数量过多不会检查剩余项。因此调用方若需要验证完全匹配，应在外层比较 AST 形状或计数，而不能把成功返回当成完整消费证明。
- 完整性检查中的 TiFlash 字符串检测区分大小写；表数原因文案写作 “more than 3 table join”，而触发条件是集合长度 `>= 3`，与 Go 当前实现和 Rust 测试保持一致，修改时不可只按自然语言理解阈值。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或全局可变状态。每次收集、绑定、解析和完整性检查都创建栈上局部状态；`HintsSet` 及其 AST 值由调用方拥有，天然适合按语句隔离使用。

需要关注的是 AST 内部的可变/共享容器生命周期：WITH 使用 `borrow`/`borrow_mut`，子查询使用 `NodeRef::with_node`、`take` 与重新装配。绑定表达式子查询时，节点被临时取出再放回；这要求调用期间没有并行借用同一引用。`ParseHintsSet` 接收 `&mut parser::Parser`，parser 实例的复用和并发隔离由调用方负责。

遍历递归深度由 SQL AST 嵌套深度决定，没有显式深度限制。大语句的主要资源成本来自 AST 克隆、hint 克隆、恢复文本的临时 `Vec<String>`，以及 `RestoreOptimizerHints` 的去重映射；正常复杂度随访问的 AST 节点和 hint 数量线性增长。

## 与 Go 版本的对应关系

直接对照文件是 [`hint_processor.go`](./hint_processor.go)。公开概念和主要语义一一对应：`HintsSet`、语句级筛选、INSERT 冲突检测、恢复接口、visitor 状态、收集/绑定、binding SQL 解析、`NodeType` 与历史 binding 完整性检查均来自该 Go 实现。

Rust 没有直接调用 Go 的 `ast.Walk`，而是在 `collectNode`/`bindNode` 中显式枚举 AST 字段，并用独立表达式 visitor 补足子查询。这使迁移语义可见，但也带来同步风险：parser AST 新增可含子查询或表名的字段时，Go 的通用 visitor 可能自动进入，而 Rust 必须显式更新收集和绑定两条路径。独立测试 `expression_subqueries_are_collected_bound_and_numbered_in_go_order`、`derived_tables_do_not_shift_index_hint_binding_with_a_dummy_table` 和 `set_operation_ctes_follow_go_accept_order` 专门锁定这些顺序差异。

恢复实现也不同：Go 委托 AST `Restore` 并在失败时写日志；Rust 在本文件按 hint 名和 `HintData` 手写格式，最终小写化。增加 parser 支持的 hint 时，必须同步扩充 `RestoreTableOptimizerHint`，否则结构可被解析但可能恢复成缺参数文本。

Rust `ParseHintsSet` 与 Go 一样：只接受一条语句、先收集再处理查询块、丢弃无 tables 的 `qb_name`、保留视图 hint、补默认库名、只留一个相关 warning。Rust 错误类型收敛为 crate 的 `errors::Error`，而 Go 使用多个返回值中的 `error`。Rust 的 AST 采用拥有值和 trait object，因此 `BindHint` 返回新的根 `Box<dyn ast::Node>`；Go 原地 walk 后返回同一 `StmtNode`。

Go 同目录没有专门的 `hint_processor_test.go`。可见的 Go 回归使用面主要在 planner 测试中调用 `ExtractTableHintsFromStmtNode` / `RestoreOptimizerHints`；Rust 的对应细粒度回归集中在独立文件 [`hint_processor_1_aster_unit_test.rs`](./hint_processor_1_aster_unit_test.rs)，符合测试与源文件分离要求。

## 扩展指南

- 增加新 table optimizer hint 时，先确认 parser AST 的 `HintData` 形态，再更新 `RestoreTableOptimizerHint`；若属于无参数、表列表、索引列表或语句级 hint，还需同步相应名称集合或 [`hint.rs`](./hint.rs) 中的 `isStmtHint` 判定。新增回归应放在 [`hint_processor_1_aster_unit_test.rs`](./hint_processor_1_aster_unit_test.rs)，不要内嵌进生产文件。
- 增加 INSERT 可接受的语句级 hint 时，更新 `supportedHintNameForInsertStmt`，并覆盖 INSERT 自身、SELECT 侧、重复告警和无 handler 四类路径。
- parser AST 新增承载表达式、表源、CTE、排序或限制的字段时，必须成对更新 `collectNode`/`bindNode` 或对应辅助函数，并用“先收集、绑定到同形空 AST、再断言位置”的测试验证序列。只改其中一侧会令计数器错位。
- 修改遍历次序、派生表行为或 INSERT 目标表规则时，应保持 `tableCounter`、`indexCounter`、`blockCounter` 的不变量，并与 Go `ast.Walk` 的 Enter/Leave 次序逐项对照。
- 修改查询块规范化时，应同时检查 [`hint_query_block.rs`](./hint_query_block.rs) 的 `Process`、`GetHintOffset`、`checkTableQBName`、`isHint4View` 和 `GenerateQBName`，覆盖 SELECT 与 UPDATE/DELETE 不同的顶层 offset。
- 扩大 warning 筛选时应添加精确 error identity/code 测试，避免退化为字符串匹配。
- 修改历史 binding 完整性规则时，应同步 Go 实现或明确记录迁移差异，尤其要澄清三表阈值、schema/name 去重方式、大小写以及表达式子查询变体；还要评估是否影响自动 binding 的计划稳定性。
- 性能上应避免在深层遍历中增加重复全树扫描；恢复新参数时可按现有模式一次分配输出，并保持线性处理。兼容性上，恢复文本会参与 binding ID，哪怕等价 SQL 的大小写、空格、引号或顺序变化也可能影响匹配与持久化数据。

## 验证依据

本说明基于以下直接证据完成：

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被索引为 1,351 行、70 个符号，并显示被 `pkg/bindinfo/binding.rs`、`pkg/planner/optimize.rs` 等文件使用。
- RustCodeGraph 源码与调用查询：完整读取 [`hint_processor.rs`](./hint_processor.rs)；对 `CollectHint`、`BindHint`、`ParseHintsSet`、`RestoreOptimizerHints`、`CheckBindingFromHistoryComplete`、`ExtractTableHintsFromStmtNode` 执行 `query`、`callers`、`callees`。图查询确认了核心下游边；因 callers 输出为空，使用 `rg` 搜索公开符号补齐生产上游。
- crate 与模块证据：读取 [`Cargo.toml`](./Cargo.toml) 和 [`lib.rs`](./lib.rs)，确认 crate 名、parser/dbterror/errno/plannererrors 依赖、模块声明、公开再导出及独立测试装配。
- Go 对照：完整读取 [`hint_processor.go`](./hint_processor.go)，逐项核对提取、恢复、visitor 顺序、解析规范化和完整性检查。
- Rust 测试：完整读取 [`hint_processor_1_aster_unit_test.rs`](./hint_processor_1_aster_unit_test.rs)。其覆盖 `HintsSet` 筛选和大小写、恢复去重及 index scope、收集/绑定顺序、表达式子查询、派生表、集合运算与 CTE、精确 warning 筛选、真实 parser、三表和 TiFlash 边界。
- 上游源码：读取 `pkg/bindinfo/binding.rs::prepare_hints_inner`、`pkg/session/hint_runtime.rs::{StartStatementHints,binding_sql_for_warning,StartStatementWithBindings}`、`pkg/planner/optimize.rs::{containUsePlanCacheHintInSQLOrBinding,optimizeNoCacheInner}` 以及两个 EXPLAIN 接线路径。
- 按任务约束未运行 Cargo 或代码测试。本任务的机械验收是目标文档存在且恰好具有本页这 11 个固定二级章节；结构命令的结果记录在任务交付报告中。
