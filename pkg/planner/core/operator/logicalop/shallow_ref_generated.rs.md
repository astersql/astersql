# `pkg/planner/core/operator/logicalop/shallow_ref_generated.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-logicalop` crate；`pkg/planner/core/operator/logicalop/lib.rs` 以私有模块 `mod shallow_ref_generated` 装入它，而其中的固有 `impl` 方法会随 `LogicalJoin`、`LogicalProjection`、`LogicalAggregation`、`LogicalSort` 四个公开类型一同对调用方可见。它位于逻辑计划构造和优化规则之间：规则可以先复制算子本体，再只改条件、表达式、物理属性候选或排序项，避免直接污染 memo/原计划中的节点。

文件名表明它是生成代码的 Rust 落地版本，但当前 Rust 文件没有生成器产物常见的 `DO NOT EDIT` 声明；同仓库 Rust 生成器 `pkg/planner/core/generator/shallow_ref/shallow_ref_generator.rs` 实际生成并校验的是 Go 文件 `shallow_ref_generated.go`。因此修改 Rust 侧字段时，不能假定重新运行该生成器会自动更新本文件。

## 核心职责

本文件提供两层复制接口：

1. 四个 `Logical*ShallowRef` 方法从已有算子构造一个可独立改写的新算子。它们复制该算子的业务字段，但通过 `..Default::default()`（`LogicalSort` 则显式默认化 `BaseLogicalPlan`）清空基类计划、Schema 生产者、上下文、孩子等未列出状态。调用者若要把副本重新接入计划树，必须补回必要的基类、Schema、输出名和孩子。
2. 十个字段级 `*ShallowRef` 方法先 `clone()` 对应 `Vec`，再返回容器的 `&mut Vec<_>`。这建立了“先复制、再改写”的固定入口；其中 `Expression = Box<dyn expression::Expression>` 的 `Clone` 会调用 `CloneExpr()`，所以表达式元素也按表达式协议复制，而不仅是复制 `Vec` 缓冲区。

这组 API 不执行优化、不遍历孩子、不重新计算 Schema/统计信息，也不检查副本是否已完成重新接线。

## 主要符号

- `LogicalJoin::LogicalJoinShallowRef(&self) -> LogicalJoin`：复制 Join 类型、重排/Hint 标志、五组条件、左右属性、完整 Schema/名称、冗余列映射、估算值和三个改写来源标志；`LogicalSchemaProducer` 等未列字段恢复默认。`FullSchema` 通过 `Schema::Clone` 复制，`FullNames` 通过 `NameSlice::Shallow` 复制其 `Arc<FieldName>` 引用，映射和向量通过 `clone()` 复制。
- `LogicalJoin::{EqualConditions,NAEQConditions,LeftConditions,RightConditions,OtherConditions}ShallowRef(&mut self)`：分别复制并返回五种 Join 条件容器。返回借用受 Rust 可变借用规则约束，调用方不能在该借用存活期间并行访问同一算子。
- `LogicalProjection::LogicalProjectionShallowRef(&self) -> LogicalProjection`：复制 `Exprs`、`CalculateNoDelay` 和 `Proj4Expand`，默认化 `LogicalSchemaProducer`。
- `LogicalProjection::ExprsShallowRef(&mut self)`：复制投影表达式列表并返回可变引用。
- `LogicalAggregation::LogicalAggregationShallowRef(&self) -> LogicalAggregation`：复制聚合函数、分组表达式、聚合/下推偏好、可能属性、输入行数估计和 `NoCopPushDown`，默认化 `LogicalSchemaProducer`。
- `LogicalAggregation::{AggFuncs,GroupByItems,PossibleProperties}ShallowRef(&mut self)`：分别复制聚合描述、分组表达式和二维列属性。二维 `Vec<Vec<Column>>` 的 `clone()` 同时复制外层和内层向量。
- `LogicalSort::LogicalSortShallowRef(&self) -> LogicalSort`：复制 `ByItems`，明确将 `BaseLogicalPlan` 重置为默认值。
- `LogicalSort::ByItemsShallowRef(&mut self)`：复制排序项列表并返回可变引用。

文件没有模块级常量、trait、自由函数、错误类型或条件编译分支；14 个方法全部是公开固有方法。

## 执行流程

典型流程是“取原节点—复制本体—修改局部—重新接线”：

1. 优化规则从已有逻辑计划取得四类算子之一。
2. 调用算子级 `Logical*ShallowRef`，得到没有原基类/孩子所有权关系的新值。
3. 若要修改受保护的列表，调用相应字段级 `*ShallowRef`，该方法先替换为克隆后的容器，再把可变引用交给调用者。
4. 规则补建基类状态和输出信息，并把新算子装入新的 memo/group expression 或逻辑计划树。

真实例子见 `pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs` 的 `XForm`：它复制 `apply.LogicalJoin`，随后重建 `BaseLogicalPlan`、设置 Schema/输出名、重新分配 Cascades ID，最后返回新 Join。`pkg/planner/cascades/old/transformation_rules.rs` 的 `PushSelDownJoin::on_transform` 和 `TransformJoinCondToSel::on_transform` 也先复制 Join，再让谓词下推逻辑修改副本并用原孩子构造新 group expression。

## 数据与状态

四个算子级方法的关键不变量是“业务配置被复制，树身份与运行上下文不沿用”。具体包括：

- Join 保留条件、Hint、属性及改写来源标志，但默认化内嵌 `LogicalSchemaProducer`；调用方不能把返回值视为可立即执行或完整接线的计划节点。
- Projection 和 Aggregation 同样默认化 `LogicalSchemaProducer`；它们的输出 Schema、计划 ID、上下文和孩子不由本文件迁移。
- Sort 明确默认化 `BaseLogicalPlan`，仅保留排序项。
- 字段级方法总会执行一次复制，即使向量为空；它们不延迟复制，也不检查是否已独占。
- `ExprBox::clone` 调用 `CloneExpr()`，因此 Join/Projection/Aggregation 中表达式列表的复制语义由各具体表达式实现决定；`NameSlice::Shallow` 则只克隆 `Arc`，字段名对象仍共享不可变所有权。

本文件不维护全局状态、缓存、计数器或持久化数据。

## 依赖与调用关系

`use crate::*` 从 crate 根取得四类算子以及 `Expression`、`Schema`、`Column`、`AggFuncDesc`、`ByItems`、`BaseLogicalPlan` 等类型。`Cargo.toml` 显示这些类型最终来自本 crate 自身模块及 `astersql-expression`、`astersql-expression-aggregation`、`astersql-planner-core-base`、`astersql-planner-util`、`astersql-types` 等路径依赖；本文件没有额外第三方依赖或 feature 门控。

RustCodeGraph 对目标文件报告 15 个节点，但没有建立这些固有方法的静态调用边；按技能规则，对图未覆盖部分使用文本引用补证。已确认的 Rust 上游包括：

- `xf_decorrelate_simple_apply.rs::XForm` 调用 `LogicalJoinShallowRef`，并显式恢复基类、Schema 与输出名。
- `old/transformation_rules.rs::{PushSelDownJoin::on_transform, TransformJoinCondToSel::on_transform}` 调用 `LogicalJoinShallowRef` 后改写谓词并重建 memo 节点。
- `logical_generated_aster_unit_test.rs` 调用 Join/Projection 算子级和字段级方法。
- `logicalop_test/logical_operator_test.rs::TestLogicalApplyClone` 通过 `LogicalApply.LogicalJoin` 调用 Join 复制方法。

当前搜索未发现生产代码调用 Aggregation、Projection、Sort 的算子级方法，也未发现除 `LeftConditionsShallowRef`、`ExprsShallowRef` 外其他字段级方法的 Rust 调用；这表示 API 已提供但其接线/覆盖仍有限，不表示这些方法可以删除。

## 错误处理与边界

所有方法都是无返回错误的内存复制；内存分配失败沿 Rust 标准分配器行为处理，不转换为 `PlannerError`。方法不会检查：

- 返回算子的默认基类是否已由调用方重新初始化；
- Schema、输出名与复制后的表达式数量是否一致；
- Join 条件是否适合当前 `JoinType`；
- `PossibleProperties`、排序项或统计字段是否需要重新推导。

因此最重要的边界是生命周期/接线边界，而不是可恢复错误边界。尤其不能在复制后遗漏 `Init`、`SetSchema`、`SetOutputNames`、孩子重建或等价步骤；否则得到的是结构上可构造、语义上不完整的节点。另一个兼容边界是 Rust 算子级方法仅复制显式列出的字段，新增算子字段不会自动进入副本，必须人工审查。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源。算子级方法只借用 `&self` 并返回拥有数据的新值，适合在原借用结束后独立接线；字段级方法需要 `&mut self`，由编译器保证修改期间不存在同一对象的其他活动可变访问。

复制会产生新的 `Vec`/`HashMap` 所有权和相应分配成本；表达式按 `CloneExpr` 复制，字段名通过 `Arc` 共享，其他具体类型遵循各自 `Clone`。这里没有跨线程同步承诺；能否跨线程发送或共享由完整算子及成员类型的 `Send`/`Sync` 实现决定，而不是这些方法保证。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/logicalop/shallow_ref_generated.go`。双方都覆盖 LogicalJoin、LogicalProjection、LogicalAggregation、LogicalSort，并为 Go 标签 `shallow-ref:"true"` 标记的切片提供字段级复制入口。Rust 的 `Vec` 对应 Go slice，`Vec<Vec<Column>>` 对应 Go `PossiblePropertiesInfo.Orders` 的二维切片复制。

需要特别注意两项差异：

1. Go 的算子级方法执行 `shallow := *op`，会保留整个结构的所有字段，包括嵌入基类和没有 `shallow-ref` 标签的字段；Rust 方法只列举业务字段，并默认化基类/Schema 生产者。Rust 调用方因此承担明确的重新接线责任。
2. Go 字段级方法复制 slice 容器但保留指针/接口元素；Rust `Expression` 是 `Box<dyn Expression>`，其 `Clone` 调用 `CloneExpr()`，通常比 Go 的元素指针复制更强。`FullNames` 则通过 `Arc` 保持与 Go 指针元素近似的浅共享语义。

Go 生成器由结构标签和反射驱动；Rust 生成器显式描述同一组 Go 字段，并在 `pkg/planner/core/generator/shallow_ref/shallow_ref_test.rs::TestHash64Equals` 中重生成、比对 `shallow_ref_generated.go`。该测试不生成或逐字校验本 Rust 文件，所以 Go/Rust 字段漂移仍需人工或新增独立测试防护。

## 扩展指南

新增或调整这四类算子字段时，应先判断字段属于“复制后仍有效的业务配置”还是“必须重新建立的计划身份/派生状态”。前者需要加入对应 `Logical*ShallowRef` 初始化器；后者应继续走默认值，并在调用方重新计算。不要仅因类型实现 `Clone` 就复制缓存、孩子、上下文或计划 ID。

若字段是规则将就地修改的 `Vec`/映射，应同步提供明确的字段级复制入口，并写独立测试证明修改副本不影响原对象。测试应放在同目录独立 `*_test.rs` 文件中，优先扩展 `logical_generated_aster_unit_test.rs`；Aggregation 的三项入口、Sort 的两项入口，以及 Join 其余条件入口目前缺少直接 Rust 回归覆盖，修改相关逻辑时应补齐。若改变 Go 对齐字段，还需同步审查 `shallow_ref_generated.go`、Go 结构标签、Rust 生成器元数据和 `shallow_ref_test.rs`。

兼容风险主要是漏复制新字段或错误复制派生状态；正确性风险是产生带旧 Schema/新表达式或缺失上下文的半接线节点；性能风险是热路径中无条件深克隆表达式和嵌套向量。扩展时应保留“副本局部可改、原节点不受污染”的核心测试，并用实际规则调用证明重新接线完整。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被索引为 131 行、15 个符号。读取了目标文件全貌，并查询了 `LogicalJoin`、`LogicalProjection`、`LogicalAggregation`、`LogicalSort`、`ExprBox`、`NameSlice` 及生成器源码。
- 源码与入口：`pkg/planner/core/operator/logicalop/shallow_ref_generated.rs`、`lib.rs`、`logical_join.rs`、`logical_projection.rs`、`logical_aggregation.rs`、`logical_sort.rs`。
- crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`。
- Go 对照与生成依据：`pkg/planner/core/operator/logicalop/shallow_ref_generated.go`、`pkg/planner/core/generator/shallow_ref/shallow_ref_generator.rs`、`pkg/planner/core/generator/shallow_ref/shallow_ref_test.rs`。
- 调用与测试：`pkg/planner/cascades/rule/apply/decorrelateapply/xf_decorrelate_simple_apply.rs`、`pkg/planner/cascades/old/transformation_rules.rs`、`pkg/planner/core/operator/logicalop/logical_generated_aster_unit_test.rs`、`pkg/planner/core/operator/logicalop/logicalop_test/logical_operator_test.rs`。
- RustCodeGraph 未解析出目标固有方法的调用边，相关调用者由 `rg` 精确符号搜索补证；未运行 Cargo，符合本纯文档任务约束。
