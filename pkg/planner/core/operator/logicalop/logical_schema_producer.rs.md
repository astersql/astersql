# `pkg/planner/core/operator/logicalop/logical_schema_producer.rs`

## 文件定位

本文件定义逻辑算子共享的 `LogicalSchemaProducer` 基座；对应的真实源码是 [`logical_schema_producer.rs`](./logical_schema_producer.rs)。它位于 `astersql-planner-core-operator-logicalop` crate 中，由同目录 `lib.rs` 声明为私有模块并通过 `pub use logical_schema_producer::*` 重新导出。`LogicalLimit`、`LogicalTopN`、`LogicalJoin`、`LogicalProjection`、`LogicalDataSource`、各类 Scan 等会把它作为具名字段嵌入自身，以复用逻辑计划基础状态、输出 Schema/列名访问和默认列裁剪、键传播行为。

与 Go 版本不同，Rust 的 `LogicalSchemaProducer` 本身只含 `BaseLogicalPlan`；Schema 和输出列名实际保存在 `BaseLogicalPlan.schema` 与 `BaseLogicalPlan.output_names`。因此它是逻辑算子公共行为的薄基座，但不是无行为的门面。证据见本文件的结构体定义、`base_logical_plan.rs` 中 `BaseLogicalPlan` 字段及 `LogicalPlan` trait 默认访问器。

## 核心职责

1. 通过 `Schema`、`Schema_mut`、`OutputNames`、`SetOutputNames`、`SetSchema` 和 `SetSchemaAndNames` 统一访问或替换逻辑算子的输出描述。
2. 通过 `InlineProjection` 在子树完成列裁剪后收紧当前算子的输出列；当父节点不需要任何列时，仍保留一个估计存储长度最小的列，避免产生零列输出。
3. 通过 `BuildKeyInfo` 先递归构建子树键信息与 `MaxOneRow`，再只为单子节点算子传播仍完整存在于自身输出中的 `PKOrUK`。
4. 实现 `LogicalPlan` 的 `as_any`/`as_any_mut` 和 `base`/`base_mut` 四个必需入口，使该基座自身也可作为 `Box<dyn LogicalPlan>` 节点使用。

Schema 的哈希与相等性属于同一类型的行为，但实现被生成到 `hash64_equals_generated.rs`：`Hash64` 哈希输出列，`Equals` 调用 `Schema::Equal`，只比较有序列集合而不比较键元数据或输出名。

## 主要符号

- `pub struct LogicalSchemaProducer { pub BaseLogicalPlan: BaseLogicalPlan }`：唯一模块级类型；`Default` 由派生实现，最终得到空子节点、空 Schema、空列名、无上下文的基座。
- `Schema(&self) -> &Schema` / `Schema_mut(&mut self) -> &mut Schema`：显式转发给 `LogicalPlan` trait，分别提供只读和独占可变访问。Rust 版本不会像 Go 的 `Schema()` 那样在首次访问时从唯一子节点惰性克隆 Schema。
- `OutputNames(&self) -> &NameSlice` / `SetOutputNames(&mut self, NameSlice)`：读取或整体替换基座中的输出名；Rust 默认值为空 `NameSlice`，也不会像 Go 版那样从唯一子节点惰性继承。
- `SetSchema(Schema)` / `SetSchemaAndNames(Schema, NameSlice)`：按值接收并替换输出元数据。后者被 `LogicalDataSource` 构造 table/index scan 的路径直接使用。
- `InlineProjection(&[Column])`：计算输出列使用位图并原地过滤 `Schema.Columns`；不会同步裁剪 `OutputNames`、`PKOrUK` 或 `NullableUK`。
- `BuildKeyInfo()`：清空本节点 `PKOrUK`、递归处理子树，并在恰有一个子节点时按当前输出列重新映射该子节点的完整键。
- `impl LogicalPlan for LogicalSchemaProducer`：提供运行时向下转型和基座访问；其余计划操作使用 `LogicalPlan` 的默认实现。

本文件没有模块级常量、枚举、独立 trait、条件编译项或返回 `Result` 的函数。

## 执行流程

`InlineProjection` 的流程如下：

1. 若 `SCtx()` 存在，取 `context.GetExprCtx().GetEvalCtx()`，调用 `expression::GetUsedList`。该函数按 `Column.UniqueID`/Schema 包含关系生成与当前 Schema 等长的布尔位图，并会处理虚拟表达式等价列的扩散。
2. 若节点尚无规划上下文，则退化为收集 `parent_used_cols` 的 `UniqueID`，逐列判断当前输出是否命中。该分支使默认构造的节点和聚焦单元测试无需上下文也能安全运行，但不包含 `GetUsedList` 的虚拟表达式扩散语义。
3. 当父使用列为空、当前 Schema 非空且位图非空时，按 `RetType.GetFlen()` 选择最小列；缺失 `RetType` 的列按 `isize::MAX` 排序。随后先清空位图，再只标记该列。若并列，迭代器的 `min_by_key` 保留最先遇到的列。
4. 用 `std::mem::take` 取走 `Schema.Columns`，按原顺序保留位图中为 `true` 的列。`used.get(index).unwrap_or(false)` 保证即便位图异常偏短，多出的列也按未使用处理而不越界。

`BuildKeyInfo` 的流程如下：

1. 先清空当前 Schema 的 `PKOrUK`，避免重复推导或拓扑变化后遗留旧键。
2. 调用 `BaseLogicalPlan.BuildKeyInfo()`，其实现先递归调用全部子节点的 `BuildKeyInfo`，再根据当前算子类型和子节点计算 `max_one_row`。
3. 只有 `Children()` 精确匹配单元素切片时才继续；零个或多个子节点直接结束，当前 `PKOrUK` 保持为空。
4. 克隆子节点 `PKOrUK`，逐个调用当前 Schema 的 `ColumnsIndices`。该方法用 `UniqueID` 定位每个键列；任一键列缺失则丢弃整个复合键。
5. 对完整命中的键，使用当前输出 Schema 中对应位置的 `Column` 克隆重建键。这样传播的是父输出列对象，而不是子节点列对象，且键内顺序保持不变。

## 数据与状态

所有持久状态都由 `BaseLogicalPlan` 拥有：本文件直接涉及 `schema.Columns`、`schema.PKOrUK`、`output_names`、`children`、`ctx` 和由基类构建的 `max_one_row`。`Schema.Columns` 的顺序是可观察状态：`InlineProjection` 稳定保序，`Hash64`/`Equals` 也按列顺序工作。

列身份主要由 `Column.UniqueID` 决定。无上下文裁剪分支用它匹配父使用列；`Schema::ColumnsIndices` 最终经 `ColumnIndex` 也优先按 `UniqueID` 找位置，并在完整列与前缀列重名时优先完整列。键传播仅写 `PKOrUK`，不传播或清空 `NullableUK`；列裁剪也不自动修复任何键集合，因此调用方必须在适当阶段重新执行 `BuildKeyInfo`。

`SetSchema` 和 `SetSchemaAndNames` 是整体所有权转移，不做克隆或一致性检查。调用方若要求 Schema 与列名逐项对齐，需要在调用前构造一致的数据；本文件没有强制 `Columns.len() == OutputNames.len()` 的不变量。

## 依赖与调用关系

直接内部依赖包括：

- `BaseLogicalPlan` 与 `LogicalPlan`：提供计划上下文、子节点、Schema/列名存储、递归键构建和 trait 对象协议。
- `expression::GetUsedList`：有规划上下文时提供完整的列使用判定；Cargo 中对应依赖为本地 `astersql-expression`。
- `Column`、`Schema`、`NameSlice`：分别从本 crate 对 expression/types 的 re-export 引入。
- `HashSet`：仅用于无上下文时按 `UniqueID` 构造简化使用集合；`Any` 用于 trait 对象向下转型。

可确认的上游调用包括：`LogicalLimit::PruneColumns`、`LogicalTopN::PruneColumns` 和 `LogicalJoin::PruneColumns` 在裁剪子节点并刷新自身 Schema 后调用 `InlineProjection`；这些顺序保证过滤的是最新输出。`LogicalLimit`、`LogicalTopN`、`LogicalTableDual`、`LogicalTiKVSingleGather`、`LogicalProjection` 及部分聚合路径调用 `BuildKeyInfo`。`LogicalDataSource` 在构造派生 scan 时调用 `SetSchemaAndNames`。大量逻辑算子通过 `LogicalSchemaProducer.BaseLogicalPlan` 实现自己的 `LogicalPlan::base/base_mut`。

RustCodeGraph 能识别本文件符号以及 `InlineProjection -> SCtx/Schema/Schema_mut`、`BuildKeyInfo -> Schema/Schema_mut` 等下游边，但对具名字段内嵌后的方法调用没有返回 callers；上述上游关系因此由相邻 Rust 调用点直接核验，而不是据空 callers 输出推断不存在调用者。

## 错误处理与边界

本文件不返回 `Result`，也不显式产生业务错误。无 `SCtx` 时 `InlineProjection` 使用 `UniqueID` 简化匹配，而不是报错；无子节点或多子节点时 `BuildKeyInfo` 清空当前 `PKOrUK` 后返回，要求多子节点算子自行实现其特定键规则。

空 Schema 下，空父使用列不会选出保底列，最终仍为空。非空 Schema 下，空父使用列会保留一列；这是一项执行安全策略，不表示父节点实际引用该列。`RetType == None` 不会解引用失败，而是被视为最大长度候选。位图长度不足不会 panic，但相应尾部列会被删除。

键传播是全键匹配：复合键只缺一列就整体丢弃。它不会传播 `NullableUK`。`InlineProjection` 单独调用后可能留下引用已删除列的键或不同步的输出名，因此正常流程应由算子在列裁剪阶段刷新 Schema，并在键推导阶段重建键；新增调用方不应把该函数误当成完整的输出元数据裁剪器。

## 并发与资源生命周期

该类型没有锁、原子变量、通道、后台任务、文件句柄或事务资源。所有变更都要求 `&mut self`，由 Rust 借用规则保证同一时刻的独占修改；它自身未声明 `Send`/`Sync` 语义。

`InlineProjection` 通过 `std::mem::take` 暂时取得列向量所有权并构造新向量，旧向量和未保留列在函数结束前正常释放；保留列被移动而非深拷贝。`BuildKeyInfo` 为避免同时借用父子状态，先克隆子键，再克隆父 Schema 中命中的列形成新键。规划上下文只被短暂共享借用，不在本文件中延长其生命周期。

## 与 Go 版本的对应关系

对应文件为 `logical_schema_producer.go`。两版共同保留了设置 Schema/列名、内联裁剪、单子节点键传播以及按输出列参与哈希/相等判断的意图，但存储布局与若干边界不同：

- Go 结构体有独立的 `schema *expression.Schema` 和 `names types.NameSlice`；Rust 把二者收敛到 `BaseLogicalPlan` 的按值字段，因此没有 nil Schema/列名状态。
- Go 的 `Schema()` 和 `OutputNames()` 会在 nil 且只有一个子节点时惰性继承；Rust 访问器只返回当前基座内容，调用方必须显式刷新或设置。`LogicalLimit`/`LogicalTopN` 的裁剪实现已在调用 `InlineProjection` 前克隆子 Schema，体现了这一接线要求。
- Go 的 `InlineProjection` 依赖非空 `SCtx`，Rust 在无上下文时增加按 `UniqueID` 的退化路径。两版在父使用列为空时都尝试保留最小 `flen` 列；Rust 对缺失返回类型和空 Schema 有显式安全处理。
- Go 会收集被裁剪列但不返回该集合；Rust 直接过滤列向量，未保留这一无外部可见效果的临时集合。
- Go 的 `BuildKeyInfo(selfSchema, childSchema)` 由框架传入 Schema；Rust 从自身和子计划读取。两版都先清空 `PKOrUK`，只传播单子节点中仍完整出现在输出里的键。
- Go 的 `Hash64`/`Equals` 位于同一文件并处理 nil；Rust 对应实现在 `hash64_equals_generated.rs`，因为 Schema 永不为 nil，无需 nil 分支。Rust `Schema::Equal` 与 Go 意图一致，只比较列序列，不比较名称和键元数据。

当前 Rust 独立测试覆盖键传播边界，生成哈希测试覆盖 Schema 变化；Go 的 `logicalop_test/hash64_equals_test.go` 和 `logical_operator_test.go` 分别提供哈希语义与 TopN 在内联裁剪前刷新 Schema 的对照证据。

## 扩展指南

新增输出元数据行为时，优先判断状态是否应属于所有逻辑计划；若是，应修改 `BaseLogicalPlan`/`LogicalPlan` 契约并让本类型转发，而不是再在 `LogicalSchemaProducer` 中建立第二份 Schema 状态。修改 `InlineProjection` 时必须保持列顺序、重复 `UniqueID` 出现次数、空使用集合保留最小列的规则，并同时评估 `LogicalLimit`、`LogicalTopN`、`LogicalJoin` 三条直接调用路径。

若要让裁剪同步输出名或键元数据，需要先明确 Go 兼容语义和优化阶段顺序；不能仅按列下标删除，因为当前调用者可能在稍后统一重建键。若扩展键传播，应区分 `PKOrUK` 与 `NullableUK`，并为零、单、多子节点以及复合键部分缺失分别定义行为。

测试必须放在独立文件：本类型行为优先扩展 `logical_schema_producer_test.rs`；哈希/相等性扩展 `logicalop_test/hash64_equals_test.rs`；涉及具体算子裁剪顺序时扩展该算子的 `*_test.rs`。至少覆盖有/无规划上下文、空父使用列、相同 `UniqueID` 的重复列、缺失 `RetType`、复合键完全/部分命中，以及多子节点不错误传播键。同步核对 Go 的 `logical_schema_producer.go` 及相关 Go 测试，避免无依据简化移植逻辑。

## 验证依据

- 目标源码：`pkg/planner/core/operator/logicalop/logical_schema_producer.rs`，核对结构体、全部 8 个固有方法与 `LogicalPlan` 实现；文件无条件编译项。
- crate 与模块边界：`pkg/planner/core/operator/logicalop/Cargo.toml`、`pkg/planner/core/operator/logicalop/lib.rs`，核对 crate 名、`expression`/`types` 等本地依赖、模块声明及公开重导出。
- 基座与数据语义：`base_logical_plan.rs` 的 `LogicalPlan`、`BaseLogicalPlan`、默认 `BuildKeyInfo`；`pkg/expression/schema.rs` 的 `Schema::Equal`、`ColumnIndex`、`ColumnsIndices`、`GetUsedList`。
- RustCodeGraph：`query LogicalSchemaProducer`、`query InlineProjection`、`query BuildKeyInfo`、`node LogicalSchemaProducer`、`callers/callees InlineProjection`、`callees BuildKeyInfo`。索引确认目标符号和下游边；callers 对字段内嵌调用为空，故上游另由源码调用点核验。
- Rust 上游与测试：`logical_limit.rs`、`logical_top_n.rs`、`logical_join.rs`、`logical_datasource.rs`；`logical_schema_producer_test.rs` 覆盖陈旧键清除、零/多子节点和单子节点键重映射；`logicalop_test/hash64_equals_test.rs` 覆盖 Schema 对哈希/相等性的影响。
- Go 对照：`logical_schema_producer.go`、`logicalop_test/hash64_equals_test.go`、`logicalop_test/logical_operator_test.go`，核对惰性 Schema/列名、内联裁剪、键传播和哈希意图。
- 本任务只新增说明文档，未运行 Cargo。交付前使用任务指定命令验证恰有 11 个固定二级章节，并人工复查未修改 Rust、Go、Cargo 或只读 `plan.md`。
