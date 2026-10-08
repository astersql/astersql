# `pkg/planner/core/operator/physicalop/physical_common_plans.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`。包根 `pkg/planner/core/operator/physicalop/lib.rs` 以 `pub mod physical_common_plans` 暴露它；同一包内的属性强制、任务转换、CTE、Shuffle、Index Join、Local Index Lookup、TiFlash 谓词下推和计划缓存克隆等模块直接引用其中的公共类型。它不是 SQL 解析入口或执行器，而是物理规划层的一组可运行公共数据模型：用轻量表达式、物理属性和递归计划节点把多个尚未统一到完整 `base::PhysicalPlan` trait 体系的 Rust 规划模块连接起来，并提供简化的 Insert/Update/Delete 模型。

文件必须分成两部分理解：第 16—311 行是被块注释包围的 Go 风格迁移设计稿，记录完整 TiDB 类型预计具备的字段与算法，不参与 Rust 编译；真正生效的实现从 `// --- 可运行的简化实现 ---` 后开始。以下行为说明均以可编译部分为准，注释设计稿只用于解释迁移目标，不能视为当前能力。

## 核心职责

1. `Datum` 与 `PhysicalExpr` 提供规划期可比较、可克隆的表达式表示，并实现列依赖收集和相对输入 schema 的引用校验。
2. `TaskType`、`PartitionType`、`CteProducerStatus`、`SortItem`、`PhysicalProperty` 和 `Stats` 描述候选物理任务的执行层级、排序/分区要求及估算行数。
3. `PhysicalKind` 与 `PhysicalPlanNode` 组成递归物理计划树。相邻模块通过增加 `Sort`、`ExchangeSender/Receiver`、`Selection`、`Projection`、`Sequence`、`Shuffle` 等节点完成任务组装；本文件提供递归内存估算与 ID 重编号。
4. `InsertGeneratedColumns`、`Assignment`、`Insert`、`Update`、`Delete` 保存 DML 规划的简化状态，处理表达式索引校验、重复键列名检查、删除表 handle 过滤，并由 `plan_clone_generated.rs` 实现计划缓存克隆策略。
5. `TableColumnPosition`、`find_table_index` 和 `is_default_expr_same_column` 固化两项与 Go 一致、容易被直觉实现改变的边界语义：按最后一个不大于序号的区间起点选表，以及识别裸 `DEFAULT`/当前列 `DEFAULT(name)`。

## 主要符号

- `Datum`：规划期常量联合体，覆盖空值、布尔、有符号/无符号整数、浮点和文本；浮点使它只能实现 `PartialEq`。
- `PhysicalExpr`：`Column`、`CorrelatedColumn`、`Constant`、递归 `Scalar` 和 `Default` 五类表达式。`columns()` 去重收集普通列及相关列 ID；`resolve_indices()` 递归验证所有列 ID 是否存在于传入 schema。
- `TaskType` / `PartitionType` / `CteProducerStatus`：分别表示 Root/Cop/MPP 任务、Any/Hash/Broadcast/Single 分区，以及 CTE producer 的 MPP 可用状态。三者默认值依次为 `Root`、`Any`、`Unknown`。
- `PhysicalProperty`：孩子计划必须满足的任务、排序、期望行数、分区列、enforcer 和 CTE/下推开关。默认期望行数是 `f64::MAX`，且允许增加 enforcer。
- `Stats`：仅保存 `row_count` 与 `version` 的简化统计。
- `PhysicalKind`：计划节点分类。部分变体携带语义数据，如 Scan 表 ID、Sort 排序项、Exchange 分区/压缩、Selection 谓词和 Projection 表达式；尚无专门结构的节点使用 `Other(String)`。
- `PhysicalPlanNode`：保存 `id`、`kind`、输出 `schema`、递归 `children`、`stats` 和每个孩子的 `required_properties`。`memory_usage()` 递归累计节点与 schema 容量；`reset_ids()` 用前序遍历分配连续 ID。
- `InsertGeneratedColumns` / `Assignment`：分别保存生成列 ID、生成列表达式、重复键侧生成列表达式，以及“目标列名 + 右值表达式”。
- `Insert`：保存表 ID/列名、VALUES/SET/ON DUPLICATE、生成列、可选 SELECT 子树与外键动作。`resolve_indices()` 校验表达式引用，`resolve_on_duplicate()` 校验目标列并返回规范化的小写集合。
- `Update`：保存逻辑赋值、执行顺序赋值、常量标记与外键动作；`resolve_indices()` 对两份赋值列表统一校验。
- `Delete`：保存表 ID 到 handle 列 ID 的映射与外键动作；`clean_table_handles()` 原地去除非删除目标表。
- `TableColumnPosition` / `find_table_index()`：描述混合行中表的半开区间，并按 `start` 找到最后一个候选；函数不检查 `end`。
- `is_default_expr_same_column()`：裸 `DEFAULT` 总为真；带名称时只与 `names.first()` 做 ASCII 不区分大小写比较。

## 执行流程

公共计划树的典型流程是：上游模块先产生叶子或子树，再根据物理属性包裹节点。例如 `enforce::enforce_property()` 在需要排序时把原计划作为唯一孩子包入 `PhysicalKind::Sort`；`enforce_exchanger()` 依次包入 `ExchangeSender` 和 `ExchangeReceiver`。`task_base::MppTask::convert_to_root_task()` 用同样的发送/接收边界把 MPP 计划收敛到 Root，并在需要时再包一层 `Selection`；`CopTask::convert_to_root_task()` 根据 index/table 子树组合 Reader，再按需添加 `Projection` 和 `Selection`。这些调用点直接读取或复制 `schema`、`stats`，并构造 `required_properties`。

表达式校验从 `PhysicalExpr::resolve_indices()` 开始：列或相关列必须能在 schema ID 列表中找到；标量函数递归处理参数；常量和 DEFAULT 不依赖输入列。`Insert::resolve_indices()` 选择 `select_plan.schema`，无 SELECT 时使用空 schema，随后依次检查 SET、ON DUPLICATE、生成列和生成列重复键表达式；第一个错误通过 `try_for_each` 立即返回。`Update::resolve_indices()` 对 `assignments` 与 `ordered_list` 采用相同的短路流程。

重复键列处理由 `Insert::resolve_on_duplicate()` 遍历赋值：在 `table_columns` 中进行 ASCII 不区分大小写匹配，未知列返回错误；合法列名转为 ASCII 小写放入 `BTreeSet`，因此结果有序且自动去重。删除 handle 清理由 `Delete::clean_table_handles()` 对 `BTreeMap` 原地 `retain`，仅保留 `deleting_tables` 中的表 ID。

计划复制时，`plan_clone_generated.rs` 的 `CloneForPlanCache for PhysicalPlanNode` 递归克隆 children；`Insert` 还递归克隆可选 select 子树。Insert/Update/Delete 只要存在外键检查或级联即拒绝缓存并返回 `None`，避免缓存携带执行期外键状态。

## 数据与状态

所有可运行类型都是普通拥有型 Rust 值，没有全局状态。集合有明确选择：列集合和重复键结果使用 `BTreeSet` 以去重并保持确定顺序，删除映射使用 `BTreeMap`，计划子树与表达式参数使用 `Vec`。计划节点拥有 children，DML 结构拥有表达式、可选子树以及 `crate::foreign_key::{FkCheck,FkCascade}`，克隆会复制这些值。

关键不变量包括：`PhysicalPlanNode.schema` 用列 ID 表示输出；表达式中的列 ID 必须属于相应输入 schema；`required_properties` 表示孩子需求而非当前节点属性；`TableColumnPosition` 的概念区间是 `[start,end)`，但 `find_table_index()` 只使用有序的 `start`，调用者必须先保证 positions 按起点升序；计划 ID 的唯一性只有在构造方正确分配或显式调用 `reset_ids()` 后成立。

内存估算是简化口径而非精确堆分析。`PhysicalPlanNode::memory_usage()` 计算节点静态大小、schema 的 capacity 和孩子递归值，但没有显式加入 `PhysicalKind` 内部字符串/向量、`required_properties` 等动态分配。`InsertGeneratedColumns::memory_usage()` 按三个向量的 capacity 估算元素槽；`Insert::memory_usage()` 只额外统计 VALUES 中表达式数量和生成列估算，未完整遍历字符串、赋值、SELECT 子树和外键对象。因此结果只适合同一简化模型内部的近似比较。

## 依赖与调用关系

编译部分仅直接依赖标准库 `BTreeMap/BTreeSet` 和同 crate 的 `foreign_key` 模块；所在 crate 的 `Cargo.toml` 还声明了 planner base/property/statistics、expression、table、parser AST、KV、tipb 等完整 physicalop 包依赖，但不能据此推断本文件已经使用全部这些系统类型。crate 元数据 `go-package = "pkg/planner/core/operator/physicalop"` 明确 Go 对照目录。

直接上游包括：

- `enforce.rs` 消费 `PhysicalProperty`、`TaskType`、`PartitionType`，构造 Sort 和 Exchange 节点。
- `task_base.rs` 以 `PhysicalPlanNode` 承载 Root/MPP/Cop 任务，读取统计和递归内存，并构造 Reader、Projection、Selection、Exchange。
- `physical_cte.rs`、`physical_cte_table.rs`、`physical_sequence.rs`、`physical_shuffle.rs`、`physical_expand.rs`、`physical_lock.rs`、`physical_indexlookup.rs`、`physical_index_hash_join.rs` 和 `physical_index_merge_join.rs` 构造或变换对应 `PhysicalKind` 子树。
- `tiflash_predicate_push_down.rs` 使用 `PhysicalExpr` 保存、分组并分析 TiFlash 过滤条件，通过 `columns()` 收集列依赖。
- `plan_clone_generated.rs` 为通用节点和三个 DML 类型补充计划缓存克隆行为。

RustCodeGraph 对目标文件标记了大量文件级“used by”，但对本次精确 `callers/callees` 命令没有产生可用的函数级边；上面的直接关系因此由索引的精确符号结果和对应源码引用共同核实，不把文件级数量当作精确调用次数。

## 错误处理与边界

`PhysicalExpr::resolve_indices()` 和 DML 的索引解析使用 `Result<(), String>`。缺列错误固定为 `column {id} is absent from child schema`，递归/迭代在第一个错误处停止；这比 Go 的结构化 planner error 简化，调用者不能按错误码分类。Insert 没有 select 子计划时 schema 为空，因此任何列引用都会失败，而常量和 DEFAULT 可通过。

`Insert::resolve_on_duplicate()` 只验证目标列存在并规范化名称；它没有实现 Go 版本的隐藏列拒绝、生成列只能赋 DEFAULT、裸 DEFAULT AST 改写、表达式 yield、`ErrSubqueryMoreThan1Row` 延迟错误及 `LazyErr`。未知列错误是普通字符串。`eq_ignore_ascii_case`/`to_ascii_lowercase` 只覆盖 ASCII 大小写，不能替代 TiDB 标识符的完整排序规则。

`find_table_index()` 在空切片或 ordinal 小于第一个 start 时返回 `None`；序号超过最后一个 `end` 时仍返回最后一个 start 的下标，这是对 Go `sort.Search(Start > ordinal)-1` 的刻意复刻。函数不校验区间排序、重叠或 `start <= end`。`is_default_expr_same_column()` 对空 names 的带名 DEFAULT 返回 false，对裸 DEFAULT 则与 names 无关地返回 true。

浮点统计和 Datum 可能包含 NaN，类型只承诺部分相等。`memory_usage()` 的加法没有溢出处理；计划树方法假定树是有限的拥有型结构，Rust 类型本身不能直接形成无间接包装的循环。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部连接，也没有 `unsafe` 和显式资源句柄。方法均同步执行，生命周期由所有权决定：`reset_ids()`、`resolve_indices()`、`clean_table_handles()` 原地独占修改；`columns()`、`resolve_on_duplicate()` 返回新集合；计划缓存克隆产生独立拥有的树。

并发安全没有在本文件中通过共享容器表达，也没有显式 `Send`/`Sync` 契约。是否跨线程传递取决于所有字段的自动 trait，尤其是外键类型；调用者不应仅凭这些数据结构“看似纯数据”就宣称具备并发共享保证。递归计划树释放由 Rust 在所有者离开作用域时完成，深树的递归遍历和递归析构可能带来栈深风险，但文件内没有深度限制。

## 与 Go 版本的对应关系

Go 同路径文件的主体是 `InsertGeneratedColumns`、`Insert`、`Update`、`Delete`、`TblColPosInfoSlice` 及 DML 辅助方法；Rust 文件前半段注释设计稿较完整地逐字段记录这些语义。可运行 Rust 后半段只保留可供当前 Rust 子系统使用的简化子集，并额外集中定义了 Go 文件中并不存在的轻量 `PhysicalExpr`、`PhysicalProperty`、`PhysicalKind` 和 `PhysicalPlanNode`。

已对齐的关键语义包括：生成列/赋值的基本承载；表达式相对 schema 校验；重复键目标列去重；删除表 handle 过滤；表列区间按最后一个 `Start <= ordinal` 查找；裸 `DEFAULT` 和 `DEFAULT(当前列)` 判断；计划缓存遇到外键动作时拒绝克隆。`physical_semantic_aster_unit_test.rs` 特别验证了超出末区间 end 仍返回末项，以及 `DEFAULT(second)` 在候选首列为 `first` 时为 false；`physical_common_plans_test.rs` 验证裸 DEFAULT 不依赖名称切片。

尚未对齐的部分不能按注释设计稿宣称已实现：Go DML 计划嵌入 `SimpleSchemaProducer`、真实 `table.Table`/`expression.Schema`/`base.PhysicalPlan`，拥有更多标志、表位置和分区表映射；Go 内存统计遍历更多字段；Go Insert 索引解析区分 TableSchema 与 Schema4OnDuplicate；Go `ResolveOnDuplicate` 实现隐藏列/生成列/MySQL 错误码/延迟子查询错误；Go Delete 按数据库名、表名和实际 handle 列匹配，而 Rust 只按表 ID 过滤。Rust 的 `Update` 也没有 Go 的 SelectPlan、分区表、表 ID 映射和多表列区间。

## 扩展指南

扩展前先判断功能属于轻量公共模型还是完整 physicalop trait 体系。若给现有简化算子增加公共计划节点，应扩展 `PhysicalKind`，在构造该节点的相邻模块设置正确 schema、stats、children 和 required_properties，并同步所有对 kind 的模式匹配及计划缓存测试。若新增表达式种类，必须同时审查 `PhysicalExpr::columns()`、`resolve_indices()`、TiFlash 谓词下推中的分类/格式化逻辑以及相关独立测试。

修改 DML 时应以 `physical_common_plans.go` 为行为基准，不要把注释设计稿直接视为完成代码。增加 Go 语义必须补充同目录独立 `*_test.rs`，重点覆盖错误类别、隐藏/生成列、DEFAULT、延迟错误、schema 选择、外键导致的缓存拒绝以及内存口径；Rust 源与测试不得合并到同一文件。若迁移到真实 expression/table/base 类型，还需评估与 `physical_insert.rs` 中另一套 `InsertGeneratedColumns` 命名和 crate 根再导出的冲突。

修改 `find_table_index()` 时必须保留 Go 的严格 `Start > ordinal` 搜索等价语义，不能擅自用 `end` 拒绝尾部 ordinal；若需要真正的区间包含查询，应新增命名明确的 API。修改 `reset_ids()` 要确认前序顺序和调用方传入的 next 约定。扩大 `memory_usage()` 口径时应一次性梳理所有动态字段，避免不同节点间比较失真，并评估递归深度与性能。

建议同步检查的测试文件包括 `physical_common_plans_test.rs`、`physical_semantic_aster_unit_test.rs`、`plan_clone_generated_test.rs`，以及具体消费者对应的 `enforce_test.rs`、`task_base_test.rs`、CTE/Shuffle/Index Lookup 测试。当前任务仅写说明文档，未改变这些代码或测试。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`physical_common_plans.rs` 被完整读取为 662 行、75 个符号。精确查询确认 `PhysicalPlanNode` 位于该文件，并定位到 CTE、Sequence、Shuffle、Index Join/Lookup 等构造者；精确 `callers/callees` 无输出的限制已在“依赖与调用关系”说明。
- 目标实现：`pkg/planner/core/operator/physicalop/physical_common_plans.rs`，重点核对可编译区第 313—662 行；第 16—311 行仅作迁移目标对照。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 和 `pkg/planner/core/operator/physicalop/lib.rs`；前者确认包名、依赖及 Go 包元数据，后者确认公共模块声明和测试模块接线。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_common_plans.go`，核对 DML 字段、内存估算、索引解析、表区间查找、DEFAULT 与重复键处理。
- 直接调用证据：`enforce.rs`、`task_base.rs`、`physical_cte.rs`、`physical_cte_table.rs`、`physical_sequence.rs`、`physical_shuffle.rs`、`physical_expand.rs`、`physical_lock.rs`、`physical_indexlookup.rs`、`physical_index_hash_join.rs`、`physical_index_merge_join.rs`、`tiflash_predicate_push_down.rs` 和 `plan_clone_generated.rs`。
- 测试证据：`physical_common_plans_test.rs` 验证裸 DEFAULT；`physical_semantic_aster_unit_test.rs` 验证 Go 风格区间查找和带名 DEFAULT；相邻消费者测试通过精确符号搜索确认覆盖计划树构造。按任务约束未运行 Cargo。
- 结构验证要求：目标文档必须存在，并且固定的十一个二级标题各出现一次；交付时使用任务文件给定的 `test`/`rg -c` 命令验证。
