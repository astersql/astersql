# `pkg/expression/schema.rs`

## 文件定位

`schema.rs` 属于 `astersql-expression` crate，crate 边界由 `pkg/expression/Cargo.toml` 定义。`pkg/expression/lib.rs:328-329` 以 `expression_schema` 私有模块挂载本文件，并在 `pkg/expression/lib.rs:390` 将其 API 整体再导出，因此规划器可以通过 `expression::Schema`、`expression::MergeSchema` 等名称使用它。

这里的 `Schema` 不是数据库目录中的“数据库模式”，而是一个关系算子输出行的表达式模式：它保存有序输出列，以及能够证明输出唯一性的键元数据。它位于表达式层与规划器之间；例如物理连接在 `pkg/planner/core/operator/physicalop/physical_hash_join.rs:630` 合并左右输出，投影在 `pkg/planner/core/operator/logicalop/logical_projection.rs:138` 计算仍被上层使用的列，基数估算在 `pkg/planner/cardinality/ndv.rs:163,189` 将列映射为 schema 下标。

## 核心职责

- 用 `Schema::{Columns, PKOrUK, NullableUK}` 表示有序输出列、非空主键/唯一键和允许 `NULL` 的唯一键；`KeyInfo` 是一个键所含的 `Vec<Column>`。
- 以 `Column::UniqueID` 为身份执行列检索、包含判断和批量下标映射，并对同一 `UniqueID` 同时出现前缀列与完整列的情况优先选择完整列（`Schema::ColumnIndex`）。
- 判断一组候选列是否覆盖某个强唯一键或可空唯一键（`Schema::IsUnique`），以及表达式树中的普通列是否引用/完全属于某个 schema（`ExprReferenceSchema`、`ExprFromSchema`）。
- 克隆、拼接、筛选 schema，并在 `MergeSchema` 中刻意不传播键元数据，留给后续 key-info 构建阶段重算。
- 为列裁剪生成与 schema 等长的使用位图（`GetUsedList`），并让具有相同虚拟表达式和返回类型的生成列共享命中状态。
- 提供调试文本、深层内存估算、额外句柄列定位，以及 Rust 特有的计划缓存拥有型快照入口（`Schema::ToCacheSnapshot`）。

## 主要符号

- `pub type KeyInfo = Vec<Column>`：一个主键或唯一键的有序列集合。`KeyInfoExt::CloneKey` 克隆全部列，`KeyInfoExt::String` 输出 `[col,...]`。
- `pub struct Schema`：核心数据结构。`Columns` 的顺序就是输出行顺序；`PKOrUK` 只保存不允许 `NULL` 的主键/唯一键，`NullableUK` 保存允许 `NULL` 的唯一键。
- `Schema::Clone`：克隆列和两类键。Rust 的 `Column` 按值保存，三个外层 `Vec` 和键内 `Vec` 都由克隆结果拥有；测试用底层指针不相等验证容器独立。
- `Schema::Equal`：只逐位置调用 `Column::EqualColumn` 比较列序列，不比较两类键元数据。
- `Schema::{RetrieveColumn, ColumnIndex, Contains}`：单列定位族；缺失返回 `None`/`false`。
- `Schema::{ColumnsIndices, ColumnsByIndices, ExtractColGroups}`：批量位置转换。前者任一列缺失即整体 `None`；第二个函数信任调用方给出的下标；第三个函数跳过不能完整映射的列组并保留原组序号。
- `Schema::{SetKeys, SetUniqueKeys, IsUnique}`：键元数据写入与覆盖检查。`IsUnique` 不要求候选列顺序等于键顺序，也允许候选列多于键列。
- `Schema::{Append, Len, String, MemoryUsage, GetExtraHandleColumn}`：容器操作、诊断和辅助查询。额外句柄仅检查最后一列及倒数第二列的 `ID == model::ExtraHandleID`。
- `ExprReferenceSchema`：递归遍历 `ScalarFunction::GetArgs`，遇到 schema 内普通 `Column` 即为真；常量、关联列和其他表达式类型不算引用。
- `ExprFromSchema`：递归要求所有普通列都属于 schema；`Constant` 与 `CorrelatedColumn` 天然通过，未识别的表达式类型返回 `false`。
- `MergeSchema`：处理双空、单侧和双侧输入；单侧返回该侧的完整克隆，双侧只拼接克隆后的列并创建空键集合。
- `GetUsedList`：根据 `used_columns` 生成布尔位图，并扩散等价生成列的使用标记。
- `NewSchema`：按传入顺序建立 schema，并把两类键初始化为空 `Vec`。
- `Schema::ToCacheSnapshot`：调用 `CachedSchema::try_from_schema`，把列和键转换为不保留运行时共享状态的拥有型快照；转换失败以 `CacheSnapshotError` 返回。

## 执行流程

典型规划流程先用 `NewSchema` 为算子建立输出列，再由算子或 key-info 阶段填充键。上层需要列位置时，`ColumnIndex` 逐项比较 `UniqueID`：每次命中都记为回退位置；前缀列继续扫描，完整列立即返回；如果最终只有前缀列，则返回最后一次前缀命中。这一规则覆盖聚簇索引场景中同一逻辑列同时作为索引键和句柄出现的布局。

列裁剪时，`GetUsedList` 先把 `used_columns` 包装成临时 schema，再按目标 schema 顺序调用 `Contains`。普通命中直接将对应位设为 `true`。若命中列的 `VirtualExpr` 是 `ScalarFunction`，函数还会扫描其余列：只有虚拟表达式在给定 `EvalContext` 下 `Equal`，且 `RetType` 完全相等，才同步标记为使用，避免把表达式相似但类型不同的生成列误认为等价。

连接或子查询组合输出时，`MergeSchema` 先处理 `None`：两侧均空返回 `None`，只有一侧则克隆该侧（包括键）。两侧均存在时分别克隆列、保持“左列在前、右列在后”的顺序，再经 `NewSchema` 创建结果；此分支有意丢弃两侧键，避免在连接语义尚未重新推导前携带错误唯一性。物理 hash/merge/index/apply join 和表达式改写器均有直接调用证据。

表达式归属判断只递归标量函数参数。`ExprFromSchema` 采用全称条件，供 `pkg/planner/cascades/old/transformation_rules.rs`、`pkg/planner/core/expression_rewriter.rs` 和 `pkg/expression/constant_propagation.rs` 判断谓词应落在哪一侧；`ExprReferenceSchema` 采用存在条件，但当前 Rust 仓库仅检出其自身递归调用，未检出文件外调用。

## 数据与状态

`Schema` 的状态完全由三个拥有型 `Vec` 构成，没有全局变量或内部缓存。`Columns`、键中的列以及虚拟表达式可能包含进一步拥有的数据；`Clone` 通过 Rust 的 `Clone` 递归复制它们。公开字段允许规划器直接读取或替换这些集合，因此调用方必须自行维护“键列确实来自当前输出”和“键分类正确”的语义不变量。

列身份主要由 `UniqueID` 决定，而额外句柄识别使用物理列 `ID`。`Equal` 不比较键，`IsUnique` 也只判断候选列是否覆盖某个键，不要求两者完全相等。这些差异是调用者选择 API 时的重要边界。

`MemoryUsage` 从 `emptySchemaSize` 开始，按三个 `Vec` 的 capacity 计入指针/切片槽位，再累加输出列和两类键内每个列的 `Column::MemoryUsage`。同一个逻辑列若同时存在于输出与键中会被分别计费；这是与 Go 实现一致的容器拥有量估算，而不是去重后的对象图大小。`pkg/planner/util/tablesampler/sample.rs:50` 和物理索引连接的内存统计会消费这一结果。

## 依赖与调用关系

本文件通过 `use crate::*` 使用同 crate 再导出的 `Column`、`Expression`、`ScalarFunction`、`CorrelatedColumn`、`Constant`、`EvalContext`、`CachedSchema`、`CacheSnapshotError`，以及 `model::ExtraHandleID` 和 `size` 常量。对应直接依赖由 `pkg/expression/Cargo.toml` 声明，其中列类型需要 `astersql-types`，额外句柄常量来自 `astersql-meta-model`；本文件本身没有 feature 或条件编译分支。

主要上游关系如下：

- `GetUsedList` 被逻辑投影、table dual 和通用 schema producer 的列裁剪调用（`logical_projection.rs:138`、`logical_table_dual.rs:70`、`logical_schema_producer.rs:69`）。
- `MergeSchema` 被多个物理 join、apply、优化器运行时和表达式重写器用于拼接输出（例如 `physical_hash_join.rs:630`、`physical_apply.rs:230`、`expression_rewriter.rs:2508`）。
- `ExprFromSchema` 被级联规则、表达式改写和常量传播用于判定谓词归属（例如 `transformation_rules.rs:1907-1942`、`constant_propagation.rs:1103-1152`）。
- `ColumnsIndices` 被 NDV 估算、投影键传播和聚合分组列映射调用；`ColumnIndex` 还广泛用于投影、join、属性检查和表达式列解析。
- `ExtractColGroups` 的生产调用位于逻辑投影；`IsUnique` 当前检出的直接使用位于本文件测试、表达式对齐测试和缓存快照测试，未检出生产调用。
- `ToCacheSnapshot` 当前未检出文件外直接调用；实际转换逻辑位于 `pkg/expression/cache_snapshot.rs:362-400`，缓存快照独立测试在 `pkg/expression/cache_snapshot_test.rs`。

下游最关键的调用是 `Column::{EqualColumn,String,MemoryUsage}`、`ScalarFunction::{GetArgs,Equal}` 和 `CachedSchema::try_from_schema`。RustCodeGraph 的文件节点还显示本文件被 53 个文件使用；精确 `callers/callees` 查询未产出边，因此上述边均由限定 Rust 源文件的调用点搜索复核。

## 错误处理与边界

大多数 API 用 `Option` 或布尔值表达正常缺失：`RetrieveColumn`/`ColumnIndex` 找不到列返回 `None`，`ColumnsIndices` 中任何列缺失则不返回部分结果，`MergeSchema(None, None)` 返回 `None`。`ExtractColGroups` 则选择跳过不完整组，而非使整个操作失败。

`ColumnsByIndices` 直接以 `self.Columns[offset]` 索引；越界会 panic，和 Go 版本依赖调用方提供合法 offset 的契约一致。`GetUsedList` 只在虚拟表达式能下转为 `ScalarFunction` 时做等价列扩散；非标量虚拟表达式不会扩散。`ExprFromSchema` 对 `Column`、`ScalarFunction`、`CorrelatedColumn`、`Constant` 之外的表达式返回 `false`，新增表达式种类时不能假定它会自动获得正确归属语义。

唯一显式错误类型来自 `ToCacheSnapshot`。`CachedSchema::try_from_schema` 会递归捕获列及虚拟表达式；不在计划缓存白名单中的表达式、扩展函数、缺失返回类型等情况通过 `CacheSnapshotError` 传播，而不是生成不完整快照。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄或事务。`Schema` 是调用方拥有的普通值，生命周期随规划节点或表达式上下文结束；修改方法都要求 `&mut self`，共享只读访问使用 `&self`，并发同步责任不隐藏在本类型内部。

`Clone` 和双侧 `MergeSchema` 产生独立拥有的列/键容器，避免后续修改输入容器影响结果。用于跨实例计划缓存边界时，应使用 `ToCacheSnapshot`；`CachedSchema` 会递归拥有列和键数据，并在恢复时重新构造 `Schema`。这条路径用于隔离运行时共享状态，但当前未检出 `Schema::ToCacheSnapshot` 的生产调用，不能据此声称 schema 快照已在完整计划缓存主链接线。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/schema.go`，独立测试分别是 `pkg/expression/schema_test.go` 与 `pkg/expression/schema_test.rs`。Rust 保留了 Go 的三字段模型、键分类、按 `UniqueID` 定位、前缀列回退、表达式递归、组筛选、内存估算、extra handle 两个候选位置、合并时不传播双侧键，以及 `GetUsedList` 的生成列表达式扩散规则。

表示层差异是 Go 使用 `*Column`、`*Schema` 和 `nil`，Rust 使用拥有型 `Column`/`Schema`，只在可空 schema 边界使用 `Option<&Schema>`/`Option<Schema>`。因此 Rust 的 `Schema::Clone` 无需处理空接收者，`NullableUK` 也不能区分 Go 的 `nil slice` 与空 slice；`ExtractColGroups` 对空输入返回两个空 `Vec`，而 Go 返回两个 `nil`，对长度和迭代语义等价但空值形状不同。

Rust 的 `ColumnsByIndices` 只接受 `usize`，从类型层排除了 Go 文档所说的负 offset，但大于等于列数仍会 panic。Rust `RetType` 使用结构相等判断来约束生成列扩散；Go 调用 `FieldType.Equal`。`ToCacheSnapshot`/`CachedSchema` 是 Rust 为实例计划缓存增加的拥有型边界，在 `schema.go` 中没有同名实现。

当前 Rust 测试覆盖克隆/字符串、前缀列优先、强弱唯一键、批量下标、列组筛选、三种 schema 合并形状、外部及重复 used column、extra handle 三个分支。Go 测试提供相同主体行为的来源证据；快照的键顺序与重复形状由 `pkg/expression/cache_snapshot_test.rs:234-235` 等测试补充验证。

## 扩展指南

- 新增字段时，应同步更新 `Schema::Clone`、`String`、`Equal` 的语义决策、`MemoryUsage`、`MergeSchema`，以及 `CachedSchema::{try_from_schema,restore}`；同时在独立的 `pkg/expression/schema_test.rs` 与需要时的 `cache_snapshot_test.rs` 增加回归，不能把测试内嵌进生产文件。
- 改变列身份规则应集中审查 `ColumnIndex`，并验证前缀列/完整列重复 `UniqueID`、`RetrieveColumn`、`Contains`、`ColumnsIndices` 及所有 planner 下标消费者。错误选择会直接造成谓词归属、列裁剪或属性匹配错位。
- 增加表达式实现类型时，应明确它在 `ExprReferenceSchema` 与 `ExprFromSchema` 中的语义，并补充嵌套标量函数、常量、关联列和未知类型测试；默认 `false` 可能使新类型被保守拒绝。
- 改变键传播时，不要只修改 `MergeSchema`。双侧合并后键由后续 key-info 阶段重建是现有契约；直接拼接输入键可能在 join 改变唯一性时产生错误优化。
- 扩展 `GetUsedList` 的等价生成列规则时，应同时核对虚拟表达式相等、返回类型、重复 used column 和 schema 外列。这里是按 schema 二次扫描，新增更宽泛等价规则还需评估大 schema 上的二次复杂度。
- 若让 `ToCacheSnapshot` 接入生产缓存主链，应先验证所有可能出现的虚拟表达式都被快照白名单覆盖，并保持失败可回退，不能吞掉 `CacheSnapshotError`。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；文件节点读取了 `pkg/expression/schema.rs` 全部 304 行，并报告该文件被 53 个文件使用。
- RustCodeGraph 源码节点：`pkg/expression/lib.rs:310-414` 确认模块挂载与公开再导出；`pkg/expression/cache_snapshot.rs:330-400` 确认 schema 快照捕获和恢复；对 `MergeSchema`、`ExprFromSchema`、`ExprReferenceSchema`、`GetUsedList`、`ToCacheSnapshot`、`GetExtraHandleColumn` 执行了精确符号查询。图的 `callers/callees` 对这些符号未返回结果，已用限定 `*.rs` 的文本调用搜索补证，未把缺失边推断为无调用。
- 读取的边界与对照：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`、`pkg/expression/schema.go`、`pkg/expression/schema_test.go`、`pkg/expression/schema_test.rs`、`pkg/expression/cache_snapshot.rs`、`pkg/expression/cache_snapshot_test.rs` 的相关调用/断言。
- 代表性调用点：`pkg/planner/core/operator/logicalop/logical_projection.rs:138,203,297`，`pkg/planner/core/operator/physicalop/physical_hash_join.rs:630`，`pkg/planner/core/expression_rewriter.rs:2405-2508`，`pkg/planner/cardinality/ndv.rs:163,189`，`pkg/expression/constant_propagation.rs:1103-1152`。
- 人工复核结论：本文区分了“当前有生产调用”“仅公开但未检出外部调用”和“Go 对照行为”；未把 `Schema` 误写成数据库目录 schema，未声称合并会保留双侧键，也未建议把 Rust 测试放入生产源文件。
