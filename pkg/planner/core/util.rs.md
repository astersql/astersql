# `pkg/planner/core/util.rs`

## 文件定位

[`util.rs`](./util.rs) 属于 `astersql-planner-core` crate（[`Cargo.toml`](./Cargo.toml) 的 `[lib]` 指向 `lib.rs`）。[`lib.rs`](./lib.rs) 以私有 `mod util;` 装配本文件，再通过 `pub use util::*;` 把其公开项重导出到 crate 根。它提供一套规划器侧的简化 AST 与辅助函数，覆盖只读判定、聚合/窗口节点抽取、稳定字符串格式化、事务脏表判断和数据库名规范化。

当前 Rust 文件不是解析器真实 AST 的通用工具入口：它定义自己的 `AstNode`，而会话运行时与优化入口可见的真实只读判定使用 `astersql_parser_ast::util::IsReadOnly`（例如 `pkg/session/runtime/scan_adapter_runtime.rs:1760`、`pkg/planner/optimize.rs:1792`）。因此本文件应理解为 core crate 内仍在迁移中的简化兼容层；其中 `AstNode` 已被 [`plan_cacheable_checker.rs`](./plan_cacheable_checker.rs) 的缓存资格检查复用，但本文件其余函数在 Rust 生产源码中未检索到直接调用。

## 核心职责

1. `IsReadOnly` / `IsReadOnlyInternal` 对本地 `AstNode` 树递归分类，确保 DML 和受检查的 `SET GLOBAL` 被视为写操作。
2. `AggregateFuncExtractor` / `WindowFuncExtractor` 以“进入节点—递归子节点—离开节点”的顺序收集函数节点，并把子查询作为遍历边界。
3. 四个 `extractStringFrom*` 函数把集合或切片生成确定性的逗号分隔文本，供诊断或解释信息使用。
4. `tableHasDirtyContent` 根据逻辑表或物理分区 ID 查询 `PlannerContext.vars.dirty_tables`。
5. `getLowerDB` 返回 `CIString` 已缓存的小写形式。

这些职责均为纯内存计算，不执行 SQL、不访问存储，也不创建计划节点。

## 主要符号

- `pub enum AstNode`：本文件的简化 AST。`Select`、`Do`、`Subquery` 保存子节点；`Explain` 包装一个语句；`AggregateFunc`、`WindowFunc` 保存名称与参数；`Insert`、`Update`、`Delete` 保存目标表信息；`Set` 保存是否为全局赋值；`Other` 用显式 `read_only` 和子节点表达尚未建模的种类。该类型也被 `plan_cacheable_checker.rs` 使用。
- `pub fn IsReadOnly(node: &AstNode, vars: &SessionVars) -> bool`：公开便利入口，固定以 `check_global_vars = true` 委托内部函数。
- `pub fn IsReadOnlyInternal(..., check_global_vars: bool) -> bool`：递归实现。签名保留 `SessionVars`，但当前函数体不读取 `vars`。
- `pub struct AggregateFuncExtractor { pub AggFuncs: Vec<AstNode> }`：按后序收集 `AggregateFunc` 的拥有型克隆。
- `pub struct WindowFuncExtractor { pub WindowFuncs: Vec<AstNode> }`：按后序收集 `WindowFunc` 的拥有型克隆。
- 两个抽取器的 `Enter`、`Leave`、`Extract`：分别控制下钻、收集当前节点和执行完整遍历；它们是固有方法，没有实现解析器 visitor trait。
- 私有 `children(&AstNode) -> &[AstNode]`：统一返回可遍历子节点；叶子、DML、`Show`、`Set` 和 `Explain` 返回空切片。`Explain` 由只读判定直接递归，但抽取器不会经 `children` 进入它包装的语句。
- `extractStringFromStringSet`：按 `BTreeSet` 迭代顺序输出带双引号的字符串。
- `extractStringFromStringSlice`：原地排序传入的可变切片，然后连接。
- `extractStringFromUint64Slice` / `extractStringFromBoolSlice`：先转成字符串副本、按字符串字典序排序，再连接；不会改动输入。
- `pub struct TableInfo`：仅保存 `id`、`temp_table` 和 `partition_ids` 的局部元信息。当前判断逻辑不读取 `temp_table`。
- `tableHasDirtyContent`：非分区表检查逻辑表 ID；分区表只检查每个物理分区 ID。
- `getLowerDB`：消费 `CIString` 并返回其 `L` 字段；`SessionVars` 参数当前未使用。

## 执行流程

只读判定从 `IsReadOnly` 进入 `IsReadOnlyInternal`。`Select` 和 `Do` 要求所有子节点只读；`Explain` 继承内部语句的结果；`Show`、`Value` 为只读；`Insert`、`Update`、`Delete` 为非只读；`Set { global: true }` 仅在 `check_global_vars` 为 `true` 时视为写；函数和 `Subquery` 继续检查参数/内部节点；`Other` 必须同时满足自身标志和全部子节点只读。递归采用短路 `all`，发现首个写节点即停止。

抽取流程由调用方创建默认抽取器后调用 `Extract(root)`。`Extract` 先调用 `Enter`；对子查询，`Enter` 返回 `false` 并立即返回。其他节点经 `children` 深度优先遍历，最后由 `Leave` 克隆匹配的函数节点。因此同一层的结果保持子树顺序，嵌套函数按内层先于外层的后序顺序进入结果；`Subquery` 内的函数不会泄漏到外层抽取结果。

格式化函数均以空集合/空切片自然产生空串。字符串切片函数会原地改变元素顺序；数值与布尔函数只排序临时字符串，所以 `10` 会按字典序排在 `3` 前。脏表判断在 `partition_ids` 为空时查询 `table.id`，否则对物理分区执行 `any`，找到首个脏分区即返回。

## 数据与状态

本文件自身没有全局可变状态。`AstNode`、`TableInfo` 和两个抽取器都拥有各自的数据；抽取器通过可变借用向 `Vec<AstNode>` 追加克隆，重复对同一实例调用 `Extract` 会累积结果，不会自动清空。

会话相关状态来自 [`common_plans.rs`](./common_plans.rs) 的 `SessionVars` 和 `PlannerContext`。`tableHasDirtyContent` 只读访问 `PlannerContext.vars.dirty_tables: BTreeSet<i64>`；`snapshot_tables`、事务标志和大小写配置均不参与本文件当前逻辑。`CIString` 来自 [`schema_table_key.rs`](./schema_table_key.rs)，构造时保存原始值 `O` 与小写值 `L`；`getLowerDB` 直接移动出 `L`。

## 依赖与调用关系

直接依赖很小：crate 根重导入 `CIString`、`PlannerContext`、`SessionVars`，标准库提供 `BTreeSet`。这些都是同步内存类型；`Cargo.toml` 没有为本文件定义专用 feature，`nextgen` feature 也没有条件编译这里的符号。

装配路径为 `lib.rs -> mod util -> pub use util::*`。已确认的 Rust 生产调用关系是 `plan_cacheable_checker::{Cacheable, CacheableWithCtx, NonPreparedPlanCacheableWithCtx} -> AstNode`；这些函数递归匹配同一个简化枚举。对其余 util 函数的精确全库 Rust 搜索只命中独立测试 [`util_test.rs`](./util_test.rs)，所以不能把 Go 调用方当作 Rust 已接线调用方。

Go 侧的对应调用更广：`logical_plan_builder.go` 使用聚合/窗口抽取器、`tableHasDirtyContent` 和 `getLowerDB`，`plan_cache.go` 使用脏表判断，`memtable_predicate_extractor.go` / `memtable_infoschema_extractor.go` 使用格式化函数，`plan_cache_utils.go` 使用 `IsReadOnly`。这些路径用于说明原设计意图，不证明 Rust 已完成同等接线。

## 错误处理与边界

所有函数均返回普通值，没有 `Result`、日志或 panic 分支。未知语义通过 `AstNode::Other` 的显式标志交给构造者；错误地把写节点标成 `read_only: true` 会导致误判，因此新增 AST 种类时不能默认套用该分支。

关键边界包括：`SET GLOBAL` 是否为写由 `check_global_vars` 控制；DML 始终为写；子查询对只读判定会递归、对两个抽取器则是硬边界；分区表不会再检查逻辑表 ID；空输入格式化为空串；字符串内容仅加双引号，不进行引号或反斜线转义；字符串切片会被排序这一副作用属于 API 契约。

还需注意当前实现的迁移限制：抽取器的 `children` 不会展开 `Explain`；`vars`、`temp_table`、`lower_case_table_names` 均未实际参与决策。这些是源码事实，不应推断为完整 TiDB 语义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部资源。共享读取是否并发安全由调用方持有的数据决定；函数只接收不可变借用，唯有 `extractStringFromStringSlice` 和抽取器方法要求独占可变借用，Rust 借用规则阻止同一数据在调用期间被并发修改。

抽取器结果的生命周期属于抽取器实例，节点以深拷贝保存，不借用源 AST。格式化产生新的 `String`；`getLowerDB` 消费 `CIString`。脏表集合只是当前上下文的快照式读取，本函数不提交、回滚或清理事务状态。

## 与 Go 版本的对应关系

对应文件是 [`util.go`](./util.go)。四个格式化函数和 `tableHasDirtyContent` 的主要输出规则基本一致：字符串集合带引号并稳定排序，切片排序后连接，数值/布尔按格式化后的文本排序，分区表检查物理分区 ID。Rust 使用 `BTreeSet` 直接获得键顺序，Go 使用 map/set 收集后显式排序。

重要差异如下：

- Go `IsReadOnlyInternal` 接受真实 `ast.Node`；遇到 `ExecuteStmt` 时解析 prepared statement，解析失败记录警告并保守返回 `false`，其余情况委托 `ast.IsReadOnly`。Rust 只匹配局部 `AstNode`，没有 prepared statement、日志或解析失败路径，且 `vars` 未使用。
- Go 抽取器实现 `ast.InPlaceVisitor`，遇到 `SelectStmt` / `SetOprStmt` 时停止下钻；聚合抽取器还以 `skipAggMap` 跳过已在外层构建的相关聚合。Rust 用固有方法和局部枚举实现，只把 `Subquery` 当边界，也没有 `skipAggMap`。
- Go `tableHasDirtyContent` 通过 `PlanContext.HasDirtyContent` 与完整 `model.TableInfo.GetPartitionInfo()` 查询状态；Rust 直接读取简化上下文集合和 `partition_ids`。
- Go `getLowerDB` 在参数的 `L` 为空时回退到 `strings.ToLower(vars.CurrentDB)`；Rust 总是返回 `database.L`，所以空 schema 得到空串而非当前数据库。
- Go `util.go` 还包含物化视图可读/可写检查；Rust 本文件没有这些符号。

因此文档中的“对齐”仅适用于逐项明确的局部规则，不能声称该 Rust 文件已完整复刻 Go 文件。

## 扩展指南

扩展 `AstNode` 时，应同时检查 `IsReadOnlyInternal`、私有 `children` 与 [`plan_cacheable_checker.rs`](./plan_cacheable_checker.rs) 中的两套递归匹配，避免只读性、函数抽取和计划缓存对同一新节点产生不一致结论。相关回归测试应继续放在独立的 [`util_test.rs`](./util_test.rs)，计划缓存行为放在 `casetest/plancache/plan_cacheable_checker_test.rs`，不要把测试嵌入生产源文件。

若要缩小与 Go 的差距，应按具体需求分别接入真实 parser AST、prepared statement 查找、相关聚合跳过表、`PlanContext` 脏内容接口或当前数据库回退；这些是跨类型/调用链迁移，不应通过继续膨胀简化枚举来假装完成。任何新接线都应先用调用搜索确认真正入口，并同步独立 Rust 测试与相应 Go 测试意图。

性能方面，当前递归深度与 AST 深度一致；抽取会克隆完整匹配子树，格式化会分配中间 `Vec<String>`。新增大节点或高频调用时应评估栈深、克隆量和排序复杂度。兼容性方面，不能随意改动字典序、双引号格式、切片原地排序、子查询边界或分区 ID 优先规则，因为测试和诊断文本可能依赖这些细节。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点，目标文件已索引；`files --filter pkg/planner/core/util.rs` 报告该文件含 35 个符号；`node --file ... --offset 1 --limit 500` 核对了 210 行完整实现，并报告该文件被 18 个文件使用。精确 `callers/callees` 因同名符号发生全库歧义，调用事实改用限定路径的 `rg` 复核。
- 源码：[`util.rs`](./util.rs)、[`lib.rs`](./lib.rs)、[`common_plans.rs`](./common_plans.rs)、[`schema_table_key.rs`](./schema_table_key.rs)、[`plan_cacheable_checker.rs`](./plan_cacheable_checker.rs)。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 确认 crate 名、库入口、feature、直接依赖和 Go 包迁移元数据。
- Go 对照：[`util.go`](./util.go)；并以 `logical_plan_builder.go`、`plan_cache.go`、`plan_cache_utils.go`、`memtable_predicate_extractor.go`、`memtable_infoschema_extractor.go` 的符号引用核对 Go 调用意图。
- 测试：[`util_test.rs`](./util_test.rs) 覆盖 DML/全局 SET/EXPLAIN，只收集外层聚合与窗口函数，四类格式化，逻辑表/分区脏内容与非空 `CIString` 小写值；[`util_test.go`](./util_test.go) 在本文件相关范围内提供两个抽取器满足 Go visitor 接口的编译期断言。当前 Rust 测试未覆盖空 schema 的 Go `CurrentDB` 回退，因为 Rust 类型没有 `CurrentDB` 字段。
- 本任务是纯文档分析，按计划不运行 Cargo；最终结构检查用于确认目标文档存在且恰有规定的十一个二级标题。
