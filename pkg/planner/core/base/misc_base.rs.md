# `pkg/planner/core/base/misc_base.rs`

## 文件定位

本文件属于 `astersql-planner-core-base` crate，是规划器核心抽象层的“杂项契约”文件。`pkg/planner/core/base/lib.rs` 通过私有模块 `misc_base` 装入它，再以 `pub use misc_base::*` 对外再导出其中的八个公开 trait。与 `plan_base.rs` 定义计划本体和规划上下文不同，本文件只描述访问对象、SHOW/内存表谓词提取、数据访问计划和分区表所需的最小接口，不包含具体算子、谓词算法或表访问实现。

crate 边界由 `pkg/planner/core/base/Cargo.toml` 确定：包名为 `astersql-planner-core-base`；本文件直接使用 `expression`、`tables`、`tipb`，并通过本 crate 的 `types` 再导出和 `PlanContext`、`PhysicalPlan` 契约连接其他基础类型。该分层符合 `pkg/planner/core/base/doc.go` 的约束：base 接口应保持抽象，不能反向依赖具体实现包，以免形成导入环。

当前 Rust 迁移状态需要特别区分：本文件确实被编译并公开导出，但仓库搜索未发现八个 base trait 的生产 `impl`。`pkg/planner/core/operator/logicalop/logical_show.rs` 和 `logical_mem_table.rs` 各自定义了同名但签名不同的局部 trait，不能视为本文件 trait 的实现。因此，本文件目前主要是 Go 接口的类型级移植边界，而不是已经贯通全部 Rust 规划主链的运行实现。

## 核心职责

- `AccessObject` 统一“算子访问了什么”的展示、归一化展示和二进制计划 protobuf 写入能力，对应 EXPLAIN 的 access object 列。
- `ShowPredicateExtractor` 抽取 SHOW 语句 LIKE/ILIKE 条件，使内存表读取端有机会提前过滤，并提供 EXPLAIN 描述、字段和排序规则相关模式。
- `MemTablePredicateExtractor` 从 WHERE 谓词中提取可由内存表读取阶段消费的部分，同时返回所有仍须由上层计算的剩余谓词。
- `MemTableRowLimitHintSetter` 与 `MemTableDescHintSetter` 是可选优化提示契约；它们允许提前停止或降序读取，但不能单独改变 SQL 结果。
- `DataAccesser` 将一般物理数据访问计划拆为 access object 与 operator info 两部分；`PartitionAccesser` 为需要 `PlanContext` 才能计算访问分区的计划提供替代入口。
- `PartitionTable` 暴露表的分区表达式，供规划阶段判断和裁剪。

这些职责只以 trait 方法表达。文件没有算法主体、构造器、全局注册、条件编译项或模块级常量，也不负责选择具体实现。

## 主要符号

- `pub trait AccessObject`：`string(&self) -> String` 生成用户展示文本；`normalized_string(&self) -> String` 生成适合归一化 EXPLAIN/摘要的稳定文本；`set_into_pb(&self, &mut tipb::ExplainOperator)` 原地填充二进制计划算子。三个方法共同保证文本和 protobuf 两种输出都来自同一访问对象抽象。
- `pub trait ShowPredicateExtractor`：`extract(&mut self) -> bool` 报告是否提取到可下推条件；`explain_info` 和 `field` 返回说明文本与目标字段；`field_pattern_like` 返回 `Box<dyn collate::WildcardPattern>`，把具体排序规则模式隐藏在 trait object 后。
- `pub trait MemTablePredicateExtractor`：`extract(&mut self, &dyn PlanContext, &expression::Schema, &types::NameSlice, &[expression::ExprBox]) -> Vec<expression::ExprBox>` 是核心入口；可变接收者允许实现保存提取结果，返回值是未下推谓词。`explain_info(&self, &dyn PhysicalPlan)` 根据最终物理计划生成说明。
- `pub trait MemTableRowLimitHintSetter`：唯一方法 `set_row_limit_hint(&mut self, u64)`。`u64` 与 Go 的 `uint64` 对齐。
- `pub trait MemTableDescHintSetter`：唯一方法 `set_desc(&mut self, bool)`。
- `pub trait DataAccesser`：`access_object(&self) -> &dyn AccessObject` 借用访问对象；`operator_info(&self, normalized: bool) -> String` 生成访问对象之外的算子信息。
- `pub trait PartitionAccesser`：`access_object(&self, &dyn PlanContext) -> &dyn AccessObject`，显式体现动态分区访问对象依赖会话/规划上下文。
- `pub trait PartitionTable`：`partition_expr(&self) -> Option<&tables::PartitionExpr>` 以借用加 `Option` 表达 Go `*tables.PartitionExpr` 的只读可空结果。

所有 trait 都是公开 API；文件中没有私有辅助符号。除返回 `Box<dyn WildcardPattern>` 的方法外，其余 trait 方法使用借用或拥有的标准容器传递数据。

## 执行流程

从接口设计和 Go 调用证据可还原三条流程，但 Rust 当前尚未把这些 base trait 接入对应生产算子。

1. EXPLAIN 文本流程：调用方把物理计划识别为 `DataAccesser`，分别调用 `access_object().string()` 和 `operator_info(false)`；若计划只实现 `PartitionAccesser`，则传入 `PlanContext` 计算动态分区访问对象。Go 证据位于 `pkg/planner/core/common_plans.go` 的 `getOperatorInfo` 路径。
2. 二进制计划流程：平坦物理计划序列化时取得 `AccessObject`，再调用 `SetIntoPB` 把 table/index/partition 信息写进 `tipb.ExplainOperator`；Go 证据位于 `pkg/planner/core/common_plans.go` 的 `binaryOpFromFlatNode` 相关分支。Rust 对应契约是 `AccessObject::set_into_pb`。
3. 内存表谓词流程：逻辑优化阶段把 schema、可空字段名切片和原始表达式交给 `MemTablePredicateExtractor::extract`；实现保存可下推状态，并把未识别表达式完整返回，上层继续求值。行数和方向接口只是读取端提示。Go 的完整流程说明与签名在 `misc_base.go`；Rust 独立测试只验证 `NameSlice(vec![None])` 能通过该接口。
4. 分区表达式流程：Go 的点查规划和分区规则把 table 断言为 `base.PartitionTable`，读取 `PartitionExpr()` 后判断表达式类型或参与分区处理，证据见 `pkg/planner/core/point_get_plan.go` 和 `pkg/planner/core/rule/rule_partition_processor.go`。Rust 的 `partition_expr` 契约保留了可空和只读语义，但本次搜索未找到生产实现或调用者。

## 数据与状态

本文件不拥有字段、缓存或全局状态，状态全部位于 trait 实现者及调用者：

- `AccessObject` 的表、索引、分区等内容由实现者保存；本接口仅要求三种观察/输出方式。
- 两个 extractor 的 `&mut self` 表示提取或设置提示可能更新实现内部状态。尤其 `MemTablePredicateExtractor::extract` 的输入谓词是借用切片，返回新的 `Vec<ExprBox>`；实现不能以“已提取”为由丢失仍需上层计算的条件。
- `types::NameSlice` 来自 `types_dependency::metadata` 的再导出。`misc_base_test.rs` 构造 `NameSlice(vec![None])`，证明字段名元素允许为空；实现必须处理 schema 与 names 的对应项缺失，而不能无条件解包。
- `DataAccesser` 和 `PartitionAccesser` 返回的 `&dyn AccessObject` 生命周期绑定到 `self`，调用方不能让该引用越过计划对象的生命周期。
- `PartitionTable::partition_expr` 返回 `Option<&PartitionExpr>`：`None` 是合法边界，不应被自动解释成非分区表；具体调用方须按业务约束处理。

## 依赖与调用关系

直接下游依赖如下：

- `expression::Schema` 与 `expression::ExprBox`：描述谓词的输入 schema 和表达式所有权。
- `expression::collate::WildcardPattern`：SHOW LIKE/ILIKE 模式的排序规则感知抽象。
- `crate::types::NameSlice`：字段名元数据；由 `pkg/planner/core/base/lib.rs` 从 `astersql-types` 再导出。
- `crate::PlanContext`、`crate::PhysicalPlan`：由同 crate 的 `plan_base.rs` 定义并经 `lib.rs` 再导出。
- `tables::PartitionExpr`：来自 `astersql-table-tables`；Cargo 对该依赖启用 `expression-runtime` feature。
- `tipb::ExplainOperator`：固定 Git revision 的 tipb protobuf 类型，Cargo 启用 `protobuf-codec`。

RustCodeGraph 将 `misc_base.rs` 识别为含 26 个符号的已索引文件，精确查询找到了本文件的八个 trait 以及 Go 同名接口。不过 callers/callees 对这些纯 trait 声明没有给出可用生产调用边；全仓 Rust `impl`/使用检索也只命中 `misc_base_test.rs` 对 base `MemTablePredicateExtractor` 的测试实现，另有 `logicalop` 的同名局部 trait。故当前 Rust 上游应表述为 `pkg/planner/core/base/lib.rs` 的公开再导出和契约测试，而不能宣称具体物理算子已经实现这些接口。

Go 上游则已形成真实主链：`common_plans.go` 使用 `DataAccesser`/`PartitionAccesser` 生成 EXPLAIN 与二进制计划；`point_get_plan.go` 和 `rule_partition_processor.go` 使用 `PartitionTable`；`operator/physicalop/*.go` 的 table/index/point-get/memory-table 等算子实现访问接口；`access/access_obj.go` 提供 scan、other、dynamic-partition 等访问对象实现。

## 错误处理与边界

本文件的所有方法都没有返回 `Result`，因此它本身不定义可恢复错误通道。字符串生成、模式构造、protobuf 写入和分区表达式读取中的失败策略由实现者或更上层流程承担；新增实现不应在接口允许正常空值时用 panic 代替分支处理。

关键边界包括：

- `MemTablePredicateExtractor::extract` 必须返回未消费谓词；错误地返回空向量会跳过上层过滤并改变查询结果。
- limit 与 desc 是提示而非语义算子。实现可以不支持优化，但不能因设置提示而漏行、改变 offset，或颠倒最终 SQL 所需顺序。
- `NameSlice` 元素可为 `None`，且 schema、names、predicates 都是外部输入；实现应避免基于非空或长度完全一致的未经验证假设。
- `PartitionTable::partition_expr` 明确可返回 `None`；调用方必须决定是回退、拒绝优化还是继续其他路径。
- `set_into_pb` 通过 `&mut ExplainOperator` 原地更新，调用方与实现必须保留算子其他已写字段，不能无意覆盖无关 protobuf 状态。
- `DataAccesser` 与 `PartitionAccesser` 都有名为 `access_object` 的方法但参数不同；未来若同一类型实现二者，调用处需显式消歧并确认静态/动态分区语义。

## 并发与资源生命周期

这些 trait 没有 `Send`、`Sync`、`'static` 或异步约束，因此 base 层不承诺实现可跨线程共享或移动。并发策略必须由具体计划/提取器类型和其持有者决定。

`extract`、`set_row_limit_hint`、`set_desc` 使用 `&mut self`，在一次调用期间要求对实现者的独占可变访问；这足以阻止安全 Rust 中的同对象并发修改，但不代表实现内部状态天然线程安全。`access_object` 和 `partition_expr` 返回借用，资源生命周期依附于实现者；`field_pattern_like` 返回拥有所有权的 `Box`，由调用者负责其生命周期并在离开作用域时自动释放。文件本身不创建任务、锁、通道、事务、文件句柄或网络连接。

## 与 Go 版本的对应关系

`pkg/planner/core/base/misc_base.go` 同样定义八个接口，Rust 方法与 Go 方法按 snake_case 一一对应，核心意图保持一致：AccessObject 输出展示/归一化/protobuf，内存表提取器返回剩余谓词，两个 hint setter 保持可选优化语义，数据访问和分区接口支撑 EXPLAIN 与规划。

类型映射上的显式差异为：

- Go 接口值映射为 `dyn Trait`；Go 的 `AccessObject` 返回值在 Rust 中是绑定到实现者的共享引用，避免无说明的克隆或所有权转移。
- Go `[]expression.Expression` 映射为输入 `&[ExprBox]` 和输出 `Vec<ExprBox>`；测试实现通过 `to_vec()` 保留所有输入谓词。
- Go `[]*types.FieldName` 映射为 `NameSlice`，其中 `Option` 保留元素可空性。
- Go `collate.WildcardPattern` 接口值映射为拥有的 `Box<dyn WildcardPattern>`。
- Go `*tables.PartitionExpr` 映射为 `Option<&PartitionExpr>`，显式表达空指针分支。
- Go `SetIntoPB(*tipb.ExplainOperator)` 映射为可变借用，保留原地写入副作用但排除空指针。

迁移尚未完全接线：Rust `logical_show.rs` 的局部 `ShowPredicateExtractor` 额外要求 `CloneBox`，方法使用 Go 风格大写名称，且 `FieldPatternLike` 返回 `Option<String>`；`logical_mem_table.rs` 的局部 extractor 也额外要求克隆并把 hint 方法做成默认方法，同时省略 base trait 的 `PlanContext` 和 `explain_info`。这些局部 trait 与本文件 trait 不是同一个类型，后续统一时不能仅靠同名判断兼容。

## 扩展指南

新增或修改能力时，应先判断它是否真是多数实现者共享的抽象；`pkg/planner/core/base/doc.go` 明确警告，少数实现专用的方法不应进入 base trait。若确需扩展：

1. 在对应 trait 尾部增加最小方法，避免依赖 `operator/logicalop`、`physicalop` 等具体实现 crate，先检查 Cargo 依赖方向以防循环。
2. 同步 Go `pkg/planner/core/base/misc_base.go` 或记录有意差异，并逐个更新所有真实实现者；不要只修改同名局部 trait。
3. 若统一当前迁移接线，重点处理 `logical_show.rs`、`logical_mem_table.rs` 的克隆需求、方法命名、上下文参数和 explain 接口差异。可以通过独立适配器或扩展 trait 解决对象克隆，不能删减 Go 行为来迁就现状。
4. `AccessObject` 扩展需同步 `pkg/planner/core/access` 的具体对象、物理算子及二进制计划测试；protobuf 字段变更还要核对 tipb revision 和兼容性。
5. extractor 扩展应在独立测试文件中覆盖：已识别谓词、未识别谓词保留、空/可空字段名、组合条件、hint 不改变语义。Rust base 契约测试应继续放在 `misc_base_test.rs`，不要内嵌到生产源文件；算子行为测试放在相应 crate 的独立 `*_test.rs`。
6. 分区接口扩展需同时验证静态/动态裁剪、点查和 EXPLAIN access object；性能风险主要来自重复构造字符串/Box、克隆表达式以及在动态分区路径重复计算访问对象。

兼容风险集中在公开 trait 的破坏性变更：增加必需方法会迫使所有实现者更新；改变借用为拥有值会改变分配和生命周期；收紧 `Send`/`Sync` 会排除现有实现；把 `Option` 改为必有值会丢失 Go 空指针语义。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7,032 个 Rust 文件和 4,415 个 Go 文件；`files --filter pkg/planner/core/base` 列出 `misc_base.rs`、`misc_base.go`、`misc_base_test.rs`、`lib.rs` 等 14 个文件；`node --file pkg/planner/core/base/misc_base.rs --offset 1 --limit 500` 读取了目标文件全部 113 行；对八个 trait 的 `query --kind trait` 精确确认了符号位置和同名 Go/局部 Rust 定义。
- 源码与 crate：`pkg/planner/core/base/misc_base.rs`、`pkg/planner/core/base/lib.rs`、`pkg/planner/core/base/Cargo.toml`、`pkg/planner/core/base/doc.go`。
- Go 对照与生产调用：`pkg/planner/core/base/misc_base.go`、`pkg/planner/core/common_plans.go`、`pkg/planner/core/point_get_plan.go`、`pkg/planner/core/rule/rule_partition_processor.go`、`pkg/planner/core/access/access_obj.go`、`pkg/planner/core/operator/physicalop/*.go`。
- Rust 迁移边界：全仓 `.rs` 文件的 trait `impl`/引用检索只发现 `pkg/planner/core/base/misc_base_test.rs` 对 base `MemTablePredicateExtractor` 的实现；`pkg/planner/core/operator/logicalop/logical_show.rs`、`logical_mem_table.rs` 及 `logical_mem_table_test.rs` 证明生产逻辑算子当前使用另一组同名局部 trait。
- 测试证据：`pkg/planner/core/base/misc_base_test.rs` 验证可空 `NameSlice` 契约；`pkg/planner/core/operator/logicalop/logicalop_test/logical_mem_table_predicate_extractor_test.go` 验证 Go 内存表抽取后状态与剩余谓词；`pkg/planner/core/casetest/binaryplan/binary_plan_test.go` 检查扫描/点查算子的 protobuf access objects；`pkg/planner/core/casetest/partition/integration_partition_test.go` 覆盖分区点查的 access object。
- 验证范围：本任务为纯文档分析，按计划不运行 Cargo。结构检查用于确认目标文档存在且恰有规定的十一个二级标题；代码运行行为只由上述现有源码和测试提供证据，未在本任务中重新执行。
