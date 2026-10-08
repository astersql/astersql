# `pkg/planner/util/path.rs`

## 文件定位

本文件属于 `astersql-planner-util` crate。crate 根 `pkg/planner/util/lib.rs` 以私有模块 `mod path` 装入本文件，再通过 `pub use path::*` 向规划器其他 crate 暴露公开 API；`pkg/planner/util/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/planner/util`。它不是最终物理计划的实现，而是 SQL 优化阶段在候选路径生成、谓词转访问范围、基数估算、排序属性匹配和 Index Join 筛选之间传递“怎样访问一张表”的共享数据模型。

直接的 Rust 主链证据是 `pkg/planner/core/operator/logicalop/logical_datasource.rs:979-985`：数据源为索引路径构造访问条件时调用 `AccessPath::SplitCorColAccessCondFromFilters`，把可在执行时重建范围的相关列条件追加到 `AccessConds`，并回写剩余的 `TableFilters`。后续 `pkg/planner/core/operator/logicalop/logical_index_scan.rs:196-227` 使用路径携带的 `ConstCols` 判断索引序是否满足物理排序属性；`pkg/planner/core/operator/physicalop/index_join_probe.rs:184-204` 又用 `IsTablePath` 去重，并用 `IsIndexJoinUnapplicable` 排除不能安全用于 Index Join 的路径。

## 核心职责

1. 用 `AccessPath` 汇集一条表路径、单索引路径或 IndexMerge 路径所需的索引元数据、range、过滤条件、行数估计、存储引擎、排序提示和计划缓存限制。
2. 判断路径类别和可用性：`IsTablePath`、`IsTiKVTablePath`、`IsTiFlashSimpleTablePath`、`IsUndetermined` 与 `IsIndexJoinUnapplicable`。
3. 从表侧残留条件中识别连续索引前缀上的“索引列 = 相关列/常量”以及末尾相关范围条件，供 Apply/相关子查询在执行时重建范围；同时标记被等值固定的完整索引列。
4. 判断 range 是否全为点范围、是否覆盖全表，并为访问条件提取“列唯一 ID → 索引前缀长度”的摘要。
5. 用 `CompareCol2Len` 比较两份列覆盖摘要，为候选路径覆盖/优劣判断提供偏序结果。

本文件不负责生成完整逻辑/物理计划，也不执行存储读取。它保存并解释规划阶段的候选路径状态；range 的具体判定委托给 `ranger`，表达式类型、等价和 collation 判定委托给 `expression` 与 `collate`。

## 主要符号

- `IndexLookUpPushDownByType`：`#[repr(i32)]` 的公开枚举。`IndexLookUpPushDownNone`、`IndexLookUpPushDownByHint`、`IndexLookUpPushDownBySysVar` 分别表示未下推、由 hint 触发和由系统变量触发；三个变体在本模块再次公开导出。
- `AccessPath`：核心公开结构。字段可按语义分组：
  - 索引形状：`Index`、`FullIdxCols`/`FullIdxColLens`、`IdxCols`/`IdxColLens`、`ConstCols`；`-1` 表示完整列而非前缀索引。
  - 范围与过滤：`Ranges`、`AccessConds`、`EqCondCount`、`EqOrInCondCount`、`IndexFilters`、`TableFilters`。
  - 基数：`CountAfterAccess`、`MinCountAfterAccess`、`MaxCountAfterAccess`、`CountAfterIndex`。
  - IndexMerge：`PartialIndexPaths`、三层的 `PartialAlternativeIndexPaths`、`KeepIndexMergeORSourceFilter`、`IndexMergeORSourceFilter`、`IndexMergeIsIntersection`、`IndexMergeAccessMVIndex`。三层分别表达 OR 分支、该分支的备选方案、一个方案包含的部分路径。
  - 路径/执行属性：`StoreType`、`IsDNFCond`、`MinAccessCondsForDNFCond`、句柄路径标志、强制排序标志、`IsSingleScan`、`IsUkShardIndexPath`、IndexLookUp 下推来源、分组 range 信息及 `NoncacheableReason`。
- `Default for AccessPath`：构造空的 TiKV 路径；容器为空、计数为零、布尔值为假、`Index` 和源过滤为空、IndexLookUp 下推来源为 `None`。
- `Clone for AccessPath` 与公开别名 `Clone`：复制索引元数据、表达式/range 向量和递归的 IndexMerge 路径。为保持 Go 结构体字面量遗漏字段的零值语义，副本的 `IndexMergeAccessMVIndex` 固定为 `false`，`IndexLookUpPushDownBy` 固定为 `IndexLookUpPushDownNone`，而不是照抄源对象。
- 路径判断方法：`IsTablePath`、`IsTiKVTablePath`、`IsTiFlashSimpleTablePath`、`OnlyPointRange`、`IsFullScanRange`、`IsUndetermined`、`IsIndexJoinUnapplicable`。
- 相关条件处理：公开的 `SplitCorColAccessCondFromFilters`，内部的 `mark_const_col`、`isColEqConstant`、`isColEqCorCol`、`isColRangeCorCol`、`isColEqExpr`、`columnComparedWith`。
- 列长度摘要：`Col2Len = HashMap<i64, isize>`、`ExtractCol2Len`、递归辅助 `extractCol2LenFromExpr`、内部比较函数 `compareLength`/`dominate`，以及公开的 `CompareCol2Len`。

本文件没有 trait、宏或条件编译项。公开 API 使用 Go 风格命名，crate 根的 `#![allow(non_snake_case, non_upper_case_globals)]` 明确允许这种移植接口形式。

## 执行流程

### 相关列条件进入访问路径

`SplitCorColAccessCondFromFilters(context, eq_or_in_count)` 从已经由等值/IN 条件占用的索引前缀之后开始扫描 `IdxCols`：

1. 为返回的访问条件和每个 `TableFilters` 的“已消费”标记分配容器。
2. 对当前索引列遍历尚未消费的表过滤。若第一待匹配列首先遇到“列 = 常量”，说明前面的 range detach 已发生 fallback；函数立即返回空访问条件和原过滤副本，避免在没有相关列条件时触发错误的范围重建。
3. “列 = 常量”或“列 = 相关列”均可延长连续前缀。每次匹配都会通过 `PlanContext -> ExprCtx` 设置跳过计划缓存的原因。只有完整索引列（`IdxColLens[index] == -1`）才从残留过滤中移除并由 `mark_const_col` 标记；前缀索引必须保留过滤以复核被截断的值。
4. 若等值未匹配，再寻找 `<`、`<=`、`>`、`>=` 与相关列的比较。命中后把它作为前缀末端访问条件，但始终保留在残留过滤中，然后提前返回；范围条件不能继续扩展等值前缀。
5. 某一列完全未匹配时终止前缀扫描。随后额外检查 `IndexFilters`：Rust detacher 可能把完整索引列的常量等值保留在这里，因此只补记 `ConstCols`，不移动或删除该过滤。
6. 最终返回新访问条件和所有未消费的 `TableFilters`。直接调用者 `logical_datasource.rs:979-985` 把前者追加到 `AccessConds`，把后者写回路径。

表达式识别先要求 `ScalarFunction` 和正确操作符，再通过 `columnComparedWith` 同时检查二元参数数量、列可位于任一侧、字符串 collation 兼容性、另一侧的具体类型以及 `EqualColumn`。因此“函数名相似”或 collation 不兼容的表达式不会被提升为访问条件。

### 点查、全扫与路径可用性

- `OnlyPointRange`：整数句柄路径要求每个 range 都满足 `IsPointNullable`；普通索引路径必须存在 `Index`，每个 range 都满足 `IsPointNonNullable`，且 `HighVal` 的列数等于索引声明列数。由迭代器 `all` 的语义可知，空 range 集对整数句柄/有索引路径返回 `true`；无索引且非整数句柄返回 `false`。
- `IsFullScanRange`：仅当路径是整数句柄、表以主键为句柄且主键列带 unsigned 标志时，向 `ranger::HasFullRange` 传入 unsigned 句柄语义；其他路径按普通有符号范围判断。
- `IsUndetermined`：表路径确定；非表路径若索引是 MV 索引，或带非空 `ConditionExprString`（条件/部分索引表达式），则不保证在所有上下文都可用。`IsIndexJoinUnapplicable` 当前直接复用此结果。

### 列长度摘要与比较

`GetCol2LenFromAccessConds` 对表路径调用 `ExtractCol2Len` 时不传索引列/长度，使发现的列长度统一为 `-1`；索引路径则传入 `IdxCols`/`IdxColLens`。`ExtractCol2Len` 对每个表达式递归：遇到 `Column` 就按 `EqualByExprAndID` 查找对应索引列并记录其前缀长度，遇到 `ScalarFunction` 就遍历参数，其他表达式类型不产生条目。相同 `UniqueID` 后写入会覆盖先前值。

`CompareCol2Len(left, right)` 先按列数返回 `1/-1`，并通过 `dominate` 判断列更多的一方是否覆盖另一方全部列且每列都不短；列数相同时，缺列返回 `(0, false)`，同列长度不一致时按存储的整数值返回方向但标为不可比，完全一致才返回 `(0, true)`。`dominate` 内部通过 `compareLength` 赋予 `-1`“完整列、强于任意有限前缀”的语义；相同列集合但长度不同的主循环则有意保持 Go 的数值比较与“不可比”约定。

## 数据与状态

`AccessPath` 是可变的规划期值对象。它同时包含：静态元数据（索引和列）、由谓词推导的条件/range、估算结果，以及影响后续计划选择的布尔状态。调用方会就地更新它，例如 `logical_datasource.rs` 追加 `AccessConds`、替换 `TableFilters`，并通过本文件设置 `ConstCols`。这要求以下并行数组保持一致：

- `FullIdxCols[i]` 对应 `FullIdxColLens[i]`；
- `IdxCols[i]` 对应 `IdxColLens[i]`，`ConstCols` 非空时也按相同下标解释；
- `GroupedRanges` 的重建依据保存在 `GroupByColIdxs`。

本文件没有封装这些字段，也没有构造期校验。尤其 `SplitCorColAccessCondFromFilters` 会按 `IdxCols` 下标访问 `IdxColLens`，`extractCol2LenFromExpr` 会按找到的列下标访问传入的长度切片；调用方必须保证长度数组不短于对应列数组。`mark_const_col` 会在首次需要时按 `IdxCols.len()` 延迟创建 `ConstCols`，并以 `get_mut` 防止已有短向量导致越界写入。

`ExprBox`、`IndexInfo`、`Range` 和嵌套 `AccessPath` 均由所有权容器持有。`Clone` 生成独立容器和元素副本；但是两个特意遗漏的字段会恢复 Go 零值，调用者不能把 `Clone` 当作所有位状态的逐位复制。

## 依赖与调用关系

上游与消费方：

- `pkg/planner/core/operator/logicalop/logical_datasource.rs` 创建和筛选 `planner_util::AccessPath`，调用 `SplitCorColAccessCondFromFilters` 和 `IsTablePath`；这是相关列访问条件接入候选路径的直接生产链。
- `pkg/planner/core/operator/logicalop/logical_index_scan.rs` 消费从路径传播来的 `ConstCols`，跳过被单值固定的索引列来匹配 `ORDER BY` 所需的物理属性。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs` 使用 `IsTablePath` 区分候选，并以 `IsIndexJoinUnapplicable` 排除 MV/条件索引路径。
- `pkg/planner/cardinality/selectivity.rs:418-452` 接收 `&planutil::AccessPath`，依据 `IsTablePath`、`Index.MVIndex`、`AccessConds` 和 `CountAfterAccess` 估算 MV IndexMerge 部分路径选择率。
- RustCodeGraph 与精确文本搜索显示 `IsTablePath` 已被 planner core/cardinality 多处使用；`CompareCol2Len`、`OnlyPointRange` 在当前 Rust 树中有独立测试证据，但未检出本文件外的 Rust 生产调用。`ExtractCol2Len` 当前由本文件的 `GetCol2LenFromAccessConds` 包装调用，而该包装同样未检出 Rust 生产调用。这反映当前接线状态，不代表 API 应删除。

下游依赖：

- `expression`/`plan-base`：表达式动态类型、列等价、类型上下文、求值上下文和计划缓存标记。
- `ranger`：`Range` 存储、点范围判定与全范围判定。
- `model`、`mysql`、`kv`：索引/表元数据、unsigned 主键标志和 TiKV/TiFlash 存储类型。
- `collate`、`parser-ast`：字符串 collation 兼容性与比较操作符常量。
- 标准库 `HashMap`：`Col2Len` 的无序映射存储。

Cargo 证据见 `pkg/planner/util/Cargo.toml`：上述 crate 都是 `astersql-planner-util` 的直接 path dependency；该 manifest 没有 feature 声明，本文件也没有 feature gate。

## 错误处理与边界

本文件的公开函数均不返回 `Result`，也不主动构造业务错误。无法识别或不满足安全条件时通常采用保守结果：表达式匹配返回 `false`，缺失索引的普通路径不是点查，未知/条件索引不用于 Index Join，列摘要中不认识的表达式被忽略。

需要调用方维护的边界包括：

- `IdxColLens` 与 `IdxCols`、传给 `ExtractCol2Len` 的索引列与长度必须同下标且长度足够，否则直接下标访问可能 panic。
- `columnComparedWith` 明确拒绝非二元函数，避免读取不存在的参数；字符串列还必须通过 `CompatibleCollate`。
- 前缀索引上的等值和相关范围过滤会保留为 residual filter，以防截断后的 range 只是原谓词的超集。
- `SplitCorColAccessCondFromFilters` 一旦提升相关条件就调用 `SetSkipPlanCache("Correlated subquery is not cached currently")`；这是兼容性/正确性保护，不是可忽略的性能提示。
- `IsFullScanRange` 的 unsigned 语义只用于整数主键句柄；扩大该条件会改变边界值是否被视为全范围。
- `CompareCol2Len` 的第一个返回值并不总表示可用于裁决的全序；第二个返回值为 `false` 时，调用者必须用其他准则决定胜者。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务、文件句柄或网络资源。`AccessPath` 在规划器内按所有权移动、借用或克隆；`SplitCorColAccessCondFromFilters` 需要 `&mut self`，因此 Rust 借用规则保证其就地修改期间不存在另一个并发可变访问。

生命周期上的关键阶段是：候选路径构造并填充列/range/过滤状态；相关条件拆分可能修改计划缓存状态和 `ConstCols`；路径随后被基数估算、属性匹配和物理算子选择消费；需要隔离候选修改时调用 `Clone` 复制。真正的相关范围是在内层子树执行时重建，本文件只保存触发重建的表达式和单值列标记，不持有执行期资源。

递归克隆 `PartialIndexPaths` 和 `PartialAlternativeIndexPaths` 的成本随 IndexMerge 候选树大小增长；递归提取列长度的成本随表达式树节点数增长。代码没有内部缓存或同步机制，因此上层若共享路径快照，应共享不可变借用或显式克隆。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/util/path.go`，Rust 基本保留其公开名称、字段分组和算法：

- `IndexLookUpPushDownByType` 三个值和判别值 `0/1/2` 一致；Rust 用 `#[repr(i32)]` 固定表示。
- `AccessPath` 字段覆盖 Go 的索引列、range、过滤、估算、IndexMerge、存储类型、排序强制、分组 range 和不可缓存原因；Go 指针/切片分别映射为 Rust 的 `Option`/`Vec`/拥有型元素。
- `Clone` 复制同一组状态，并刻意让 Go 结构体字面量没有列出的 `IndexMergeAccessMVIndex`、`IndexLookUpPushDownBy` 保持零值。Rust 测试 `test_access_path_clone_preserves_go_zero_value_semantics` 专门锁定这一点。
- 三个表路径判定、相关列条件拆分、比较表达式的双向操作数与 collation 检查、点查/全扫/不确定路径判定、`Col2Len` 提取和比较均对应同名 Go 实现。

存在一项有注释依据的 Rust 局部接线差异：`SplitCorColAccessCondFromFilters` 末尾还扫描 `IndexFilters` 中完整索引列的常量等值并补记 `ConstCols`，但不移动或删除过滤；源码注释说明 Rust detacher 会把部分此类等值留在 `IndexFilters`。此外，Go 测试需要创建 mock context 并关闭 stats handle，Rust 的独立测试使用包级 `setup_for_planner_util_test`，本文件本身没有资源差异。

测试对照：`pkg/planner/util/path_test.go` 覆盖 `CompareCol2Len` 与 `OnlyPointRange`；`pkg/planner/util/path_test.rs` 保留相同用例，并增加 Clone 零值语义回归。Rust 测试还明确验证：整数句柄允许 NULL 点；普通索引拒绝 NULL 点和区间；多列索引若 range 只覆盖一列也不是完整点查。

## 扩展指南

- 新增 `AccessPath` 字段时，应同步检查 `Default`、`Clone`、Go `AccessPath.Clone`、所有结构体字面量及上层路径转换。先决定它应复制源值还是必须保持 Go 零值语义，并在独立的 `pkg/planner/util/path_test.rs` 增加回归测试；不要把测试嵌入 `path.rs`。
- 扩展相关列访问条件时，入口是 `SplitCorColAccessCondFromFilters` 及 `isCol*`/`columnComparedWith`。必须保持连续索引前缀、前缀索引 residual filter、范围条件终止前缀、collation 兼容和跳过计划缓存这些不变量；同时验证 `logical_datasource.rs` 的追加/回写流程和 `logical_index_scan.rs` 的排序属性行为。
- 增加新的比较操作符或表达式节点时，要明确是否可以安全用于执行期范围重建；仅让表达式匹配成功而没有对应 ranger 重建能力会造成错误结果。
- 调整 `OnlyPointRange` 时，至少同步覆盖整数句柄 NULL、普通索引 NULL、区间、多列未完全匹配和空 range 语义，并评估点查优化调用方。
- 调整 `Col2Len` 时，要同时维护 `ExtractCol2Len`、`compareLength`、`dominate`、`CompareCol2Len` 以及 Go 对照。特别注意 `-1` 的“整列”语义与同列数不可比规则；不要只按普通整数大小理解所有分支。
- 性能方面，避免在热路径中无条件深克隆大型 IndexMerge 候选树；添加嵌套字段会进一步放大 `Clone` 成本。兼容性方面，公开 API 的 Go 风格名称和字段被多个 planner crate 直接使用，重命名或收紧可见性需要先核对全部调用者。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 索引状态：项目索引有效，包含 `pkg/planner/util/path.rs`；`node --file` 读取该文件 1-582 行，确认 32 个索引符号及全部实现。
- RustCodeGraph `query`：确认 Rust 符号 `path.rs::SplitCorColAccessCondFromFilters`、`path.rs::OnlyPointRange`、`path.rs::GetCol2LenFromAccessConds`、`path.rs::ExtractCol2Len`、`path.rs::CompareCol2Len`，并确认对应 Go 符号。图的 Rust `callers/callees` 对这些 `impl`/函数未返回边，因此调用关系按技能规则用下述精确文本搜索补证。
- Rust 上游与消费方：`pkg/planner/core/operator/logicalop/logical_datasource.rs:950-1019`、`pkg/planner/core/operator/logicalop/logical_index_scan.rs:180-227`、`pkg/planner/core/operator/physicalop/index_join_probe.rs:173-224`、`pkg/planner/cardinality/selectivity.rs:416-452`。
- crate 边界：`pkg/planner/util/Cargo.toml`、`pkg/planner/util/lib.rs`。
- Go 对照：`pkg/planner/util/path.go:32-598`。
- 独立测试：`pkg/planner/util/path_test.rs:1-171`、`pkg/planner/util/path_test.go:1-123`。
- 精确调用搜索：对 `pkg/**/*.rs` 搜索本文件公开方法和 `AccessPath`，确认 `IsTablePath`、相关条件拆分和 Index Join 判定的生产接线，以及部分公开 API 当前只有测试/内部包装调用的事实。

本任务是纯文档分析，没有运行 Cargo 或代码测试。交付检查只验证文档存在、固定 11 个二级标题齐全，并人工复核上述源码、调用边、Cargo、Go 和测试证据；运行结果记录在任务交付信息中。
