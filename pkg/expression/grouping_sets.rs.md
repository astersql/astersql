# `pkg/expression/grouping_sets.rs`

## 文件定位

[`grouping_sets.rs`](./grouping_sets.rs) 是 `astersql-expression` crate 内的 GROUPING SETS / ROLLUP 表达式侧算法实现。它定义三层布局数据结构，提供前缀合并、普通聚合目标布局选择、列克隆判定、protobuf 编码、ROLLUP 展开、可空性修正、GROUP BY 表达式去重与还原，以及 distinct grouping id 计算。

模块由 [`lib.rs`](./lib.rs) 以私有的 `grouping_sets_kernel` 编译进 crate；`lib.rs` 只在 `#[cfg(test)]` 的 `expression_files_36::grouping_sets` 中再导出这些符号。仓库搜索未发现其他 Rust 生产文件调用本文件 API，因此当前可确认的 Rust 接线是“crate 内已编译、独立测试已覆盖”，不能据此声称它已经接入 Rust 规划或执行主链。Go 侧的同路径实现 [`grouping_sets.go`](./grouping_sets.go) 则是移植语义基准。

crate 边界见 [`Cargo.toml`](./Cargo.toml)：包名为 `astersql-expression`，`[lib]` 入口是 `lib.rs`，并关闭自动测试发现（`autotests = false`）。本文件直接使用 crate 根再导出的表达式、Schema、KV 客户端、MySQL 标志位、内存常量和错误类型，并使用 `tipb` protobuf 依赖。

## 核心职责

1. 用 `GroupingSets(Vec<GroupingSet>)`、`GroupingSet(Vec<GroupingExprs>)`、`GroupingExprs(Vec<ExprBox>)` 表达“多套布局—单条前缀链—一组表达式”的三层结构。
2. 将普通分组列构造成初始布局，并通过 `GroupingSets::merge` / `merge_one` 把存在子集关系的表达式组组织成由窄到宽的链。
3. 通过 `target_one` 为普通聚合选择不会把其引用列补成 NULL 的布局，通过 `need_clone_column` 判断不同布局共享列时是否需要克隆列。
4. 生成 `(), (a), (a,b), ...` 形式的 ROLLUP 布局，并通过 `adjust_nullability_from_grouping_sets` 修正 Expand 输出 Schema 的 `NotNullFlag`。
5. 用表达式的 `CanonicalHashCode` 去重 GROUP BY 项、记录原位置并在投影成列后恢复重复序列。
6. 按列 `UniqueID` 集合判定逻辑重复布局；当 distinct 布局数超过阈值时，为每个原始布局分配 grouping id，并建立“列 ID → 使用该列的 gid 集合”。
7. 提供空值判定、深克隆、调试字符串、近似内存计量和 `tipb::GroupingSet` 序列化等支撑操作。

## 主要符号

- `GroupingSets(pub Vec<GroupingSet>)`：全部并列布局。`Clone + Default`；其方法承载跨布局算法。
- `GroupingSet(pub Vec<GroupingExprs>)`：一条按集合包含关系排列的布局链。`all_col_ids` 以列 `UniqueID` 计算整条链的并集。
- `GroupingExprs(pub Vec<ExprBox>)`：单个分组表达式集合。手写 `Clone` 会逐项调用 `CloneExpr`，避免只复制 trait object 容器。
- `GroupingIds = BTreeSet<i64>`：稳定有序的列 ID 集合，用于子集、交集、差集和布局等价判断。
- `IdToGids = BTreeMap<i64, BTreeSet<u64>>`：列 ID 到包含该列的 grouping id 集合的反查表；有序容器使遍历结果稳定。
- `new_grouping_sets(Vec<ExprBox>) -> GroupingSets`：每个输入列各建一个 `GroupingSet`，每个 set 初始只含一个单列表达式组。
- `GroupingSets::{merge, merge_one}`：按 `GroupingExprs::subset_of` 将可比较的集合并为由窄到宽的链，不可比较时新增一条链。
- `GroupingSets::target_one(&[ExprBox]) -> isize`：抽取普通聚合参数中的列 ID；无列参数返回 0，有列时返回第一个不会补 NULL 的布局，找不到返回 -1。
- `GroupingSets::need_clone_column() -> bool`：任意两套布局的列并集相交即返回 `true`。
- `GroupingSets::{distinct_size, distinct_size_with_threshold}`：计算 distinct 布局数；默认阈值为 64，只有数量严格大于阈值时才返回 gid 和 `IdToGids`。
- `GroupingSet::{extract_cols, clone_set, to_pb}`：分别执行强制 Column 提取、深拷贝和 protobuf 编码。
- `GroupingExprs::{subset_of, id_set}`：以 Column `UniqueID` 而非表达式对象地址进行集合比较；非 Column 会 panic。
- `rollup_grouping_sets`：对长度为 `n` 的输入生成 `n + 1` 个前缀布局，并逐项深克隆表达式。
- `adjust_nullability_from_grouping_sets`：仅对属于 grouping sets、且至少在一个布局缺失的 Schema 列清除 `NotNullFlag`。
- `deduplicate_gby_expression`：按 `CanonicalHashCode` 保留首次出现项的克隆，并返回每个原始项指向 distinct 列表的位置。
- `restore_gby_expression`：按位置数组从投影后的 `Column` 列表克隆并恢复原 GROUP BY 序列。

本文件没有模块级常量、trait、enum 或条件编译项；条件编译位于 `lib.rs` 的测试模块声明处。

## 执行流程

典型的 ROLLUP 数据准备流程可由源文件和 Go 测试还原为：

1. `deduplicate_gby_expression` 对原始 GROUP BY 表达式做语义哈希去重，产生 distinct 表达式和原位置映射。
2. 复杂表达式在上层投影为 `Column` 后，`restore_gby_expression` 用位置映射恢复重复项；重复项必须保留，因为 ROLLUP 输出层级数和 grouping position 仍依赖原始顺序。
3. `rollup_grouping_sets` 从空前缀开始，依次克隆前 `0..=n` 个表达式，形成 `(), (a), (a,b), ...`。
4. `distinct_size_with_threshold` 把每个布局压缩为列 ID 集合，识别因重复 GROUP BY 项形成的逻辑重复布局。distinct 数不超过阈值时只返回数量；超过阈值时额外分配连续 gid，并生成列到 gid 的反查表。
5. `adjust_nullability_from_grouping_sets` 遍历 Schema：与 grouping sets 无关的列保持原标志；分组列只要在任一布局中缺失，就可能被 Expand 填 NULL，因此清除 `NotNullFlag`。
6. 如需下推，`GroupingSets::to_pb` 逐 set 调用 `GroupingSet::to_pb`；后者逐表达式组调用 `ExpressionsToPBList` 并组装 `tipb::GroupingExpr` / `tipb::GroupingSet`。

前缀合并流程是另一条独立路径：`merge` 依输入顺序展开所有 `GroupingExprs`，第一项直接建链，后续项交给 `merge_one`。`merge_one` 从每条链尾部向前比较：目标是当前项子集时继续左移或插到链头；当前尾项是目标子集时追加；处于链中间时插在当前项之后；与整条链不可比较则尝试下一条链，最终仍不匹配便新建 set。结果保证同一链内按集合包含关系由窄到宽排列，但不保证全局最优搜索，也不会重新排序输入链。

## 数据与状态

所有长期状态都由调用者持有，本文件没有全局可变状态。

- `GroupingSets` 的外层位置同时是 `target_one` 返回的布局下标；更改布局顺序会改变选择结果。
- `GroupingSet` 内部顺序表示扩展链的层级顺序，`merge_one` 依赖该链已经满足子集递增关系。
- 集合语义以 `Column.UniqueID` 为身份；同一 ID 的重复表达式在 `GroupingIds` 中折叠。因此 `distinct_size_with_threshold` 比较的是逻辑列集合，不比较表达式出现次数或顺序。
- `distinct_size_with_threshold` 保留每个 distinct 集合第一次出现的位置，并按首次出现顺序从 0 分配 gid；逻辑等价的原布局共享 gid。
- `deduplicate_gby_expression` 的 key 是 `Vec<u8>` 形式的 `CanonicalHashCode`；distinct 列表保持首次出现顺序，返回的位置数组长度与输入长度一致。
- `GroupingExprs::clone`、`rollup_grouping_sets`、去重和还原路径都克隆表达式/列，避免不同层级直接共享可变表达式对象。
- `memory_usage` 是近似值：按 Vec 容器、capacity、指针/接口大小以及子表达式的 `MemoryUsage()` 求和，不代表分配器的精确驻留内存。

## 依赖与调用关系

上游和装配关系：

- [`lib.rs`](./lib.rs) 的 `#[path = "grouping_sets.rs"] mod grouping_sets_kernel;` 将本文件编入 `astersql-expression`。
- 同文件以 `#[cfg(test)]` 挂载 [`grouping_sets_test.rs`](./grouping_sets_test.rs) 和 `grouping_sets_runtime_aster_unit_test.rs`，并在测试辅助模块内再导出 `grouping_sets_kernel::*`。
- RustCodeGraph 的调用者结果显示 `merge`、`rollup_grouping_sets`、`adjust_nullability_from_grouping_sets` 等入口由上述 Rust 测试调用；普通仓库搜索未发现本文件符号被其他 Rust 生产文件引用。`pkg/planner/core/operator/logicalop/logical_expand.rs` 另有同名 `GroupingSet` / `GroupingSets`，是独立类型，不能视为本文件的调用者。

主要下游依赖：

- `crate::util_kernel::ExtractColumns`：`target_one` 从任意聚合参数表达式中提取列引用。
- `ExprBox` / `Expression` 方法 `CloneExpr`、`CanonicalHashCode`、`StringWithCtx`、`MemoryUsage`：负责深克隆、语义去重、诊断输出和内存估算。
- `Column` 的 `UniqueID`、`RetType`、`CloneColumn`：提供集合身份、Schema 类型标志修改和还原克隆。
- `Schema.Columns`、`mysql::NotNullFlag`：可空性调整目标。
- `expr_to_pb_kernel::ExpressionsToPBList`、`EvalContext`、`kv::Client`、`tipb::{GroupingExpr, GroupingSet}`：protobuf 编码链。
- 标准库 `BTreeSet` / `BTreeMap` / `HashMap`：集合运算、稳定映射和哈希去重。

Cargo 层面，`kv-dependency`、`parser-mysql-dependency` 和 Git `tipb` 是上述路径的直接相关依赖；大量表达式类型经 crate 根的 `use crate::*` 获得。当前没有本文件专属 feature gate。

## 错误处理与边界

- `GroupingSet::to_pb` / `GroupingSets::to_pb` 是本文件主要的可恢复错误边界。任一 `ExpressionsToPBList` 失败即通过 `?` 原样向上传播 `Error`，不会返回部分编码结果。
- `GroupingExprs::id_set` 和 `GroupingSet::extract_cols` 使用 `downcast_ref::<Column>().expect(...)`。因此参与集合运算、合并、distinct 计算和 Column 提取的 grouping 表达式必须已经被上层投影/规范化为 `Column`；违反前置条件会 panic。独立测试 `extract_cols_rejects_non_column_grouping_expressions` 固定了此行为。
- `target_one` 对不引用列的参数（如常量聚合参数）直接返回 0。若 `GroupingSets` 为空且参数引用列，循环无法命中并返回 -1；若输入为空且参数不引用列，仍返回 0，调用方必须确保布局 0 实际存在。
- `restore_gby_expression` 直接用 `expressions[*index]` 索引，没有边界检查错误类型；损坏或不匹配的位置数组会 panic。它要求 indexes 来自配套的去重阶段，并与投影后列列表一致。
- `adjust_nullability_from_grouping_sets` 仅在 `RetType` 为 `Some` 时修改标志；缺失类型信息的列被静默保留。与 grouping sets 无关、或存在于每个布局中的列也保持 `NotNullFlag`。
- 阈值判断是 `distinct_count > threshold`，等于阈值不会分配 gid；测试分别覆盖阈值 1 和 2 时的两侧行为。
- `merge_one` 假定既有链已经按子集关系组织；对任意未规范化链调用时，其插入位置语义不受保证。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务、文件或网络连接，也不持有 `EvalContext` / `kv::Client` 的所有权。全部函数同步执行；上下文和客户端仅在 `to_pb` 调用期间以共享引用借用。

结构体由 `Vec`、树集合和表达式所有权组成，生命周期由 Rust 所有权管理。克隆路径会产生新的表达式对象；临时 `BTreeSet`、`BTreeMap`、`HashMap` 和 protobuf 组装缓冲在函数返回或错误传播时释放。并发安全性取决于 `ExprBox`、`EvalContext` 和客户端的具体实现，本文件既未增加同步，也未声明跨线程使用保证。

性能上需关注：`merge_one` 最坏遍历全部链和链元素；`need_clone_column` 做两两布局交集检查；`distinct_size_with_threshold` 用线性扫描判重，并在超阈值后对 distinct 布局和原布局做嵌套匹配。这些都适合当前以布局数量较小为前提的调用场景，扩展到大量 grouping sets 时应单独基准验证。

## 与 Go 版本的对应关系

[`grouping_sets.go`](./grouping_sets.go) 是逐项对照基准。Rust 的 `new_grouping_sets`、`merge`、`merge_one`、`target_one`、`need_clone_column`、空值/集合/克隆/字符串/内存/PB 方法、ROLLUP、可空性、去重、还原与 distinct-size 算法均有同名或 snake_case 对应。

已核对的共同语义包括：

- 合并按列 `UniqueID` 的集合包含关系组织前缀链，不可比较的表达式组进入新 set。
- `target_one` 忽略聚合参数中不引用列的部分；无列参数默认布局 0，跨布局无法同时满足时返回 -1。
- ROLLUP 对 `n` 个输入产生 `n + 1` 个从空集到完整前缀的布局。
- 只要分组列在某一布局中缺失，就清除其 Schema `NotNullFlag`；无关列和每层都存在的列不变。
- GROUP BY 去重按 `CanonicalHashCode`，位置数组可在投影为 Column 后恢复重复次序。
- distinct 布局数严格超过默认 64（或注入阈值）才生成 gid；等价布局共享 gid。

实现层面的差异：Go 使用 `intset.FastIntSet` 和普通 map，Rust 使用有序的 `BTreeSet` / `BTreeMap`；Go 的类型断言 `one.(*Column)` 与 Rust 的 `downcast_ref::<Column>().expect(...)` 都把“必须为 Column”作为强前置条件；Go 去重直接保留首次出现的表达式接口值，而 Rust 调用 `CloneExpr`；Go 的 `AdjustNullabilityFromGroupingSets` 假定 `RetType` 存在，Rust 对 `Option` 做条件修改；Rust 的字符串接口固定传入 `None` 和 `RedactLogDisable`，没有暴露 Go 版本的 ctx/redact 参数。以上差异不改变已测试的核心布局语义，但调用者不应假设对象共享或诊断参数完全一致。

[`grouping_sets_test.go`](./grouping_sets_test.go) 覆盖更广的 Go 语义场景，包括复合聚合参数、更多合并排列、ROLLUP 可空性和阈值以上 gid 映射；Rust 独立测试覆盖核心等价路径，但不是 Go 测试矩阵的逐例完整复制。

## 扩展指南

- 新增或修改布局算法时，优先在 `GroupingSets` / `GroupingSet` / `GroupingExprs` 对应层实现，保持三层结构职责清晰；集合身份仍应统一使用 `UniqueID`。
- 修改 `merge_one` 时必须维持同一链由窄到宽的包含关系，并在独立的 [`grouping_sets_test.rs`](./grouping_sets_test.rs) 增加链头、链中、链尾、不可比较新建链和重复集合用例。不要把测试写回生产 `.rs` 文件。
- 扩展 `target_one` 的多列跨布局策略时，要同步验证常量参数、嵌套表达式、无可用布局和空输入，并与 Go 的限制/后续变化对齐。
- 支持非 Column grouping 表达式前，必须统一改造 `id_set`、`extract_cols`、`all_col_ids`、merge 和 distinct-size 的身份规则；仅把 panic 改成跳过会破坏布局等价性，不能作为兼容实现。
- 修改去重 key 或还原协议时，应同时更新 `deduplicate_gby_expression` 与 `restore_gby_expression`，并验证哈希碰撞假设、首次出现顺序、重复表达式和索引有效性。
- 修改 protobuf 结构时，从 `GroupingSet::to_pb` 接入并保留错误传播；同步检查 `tipb` schema、`ExpressionsToPBList` 行为和下推端兼容性。
- 若要把本模块接入 Rust 生产主链，需要显式选择导出边界并在真实 planner/Expand 调用处完成接线；当前同名 planner 类型不是本模块类型。此类改动应补独立集成测试，而不能以现有单元测试代表生产接线。
- 大量布局场景下，修改嵌套扫描或容器类型前应对 `merge`、`need_clone_column`、`distinct_size_with_threshold` 做基准和顺序稳定性验证。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标 Rust、Go 和测试文件均可查询。
- RustCodeGraph 源码与符号查询：`explore "pkg/expression/grouping_sets.rs GroupingSets GroupingSet TargetOne"`；`query GroupingSets --kind struct`；`query rollup_grouping_sets --kind function`；按文件分段读取 `grouping_sets.rs` 全部 417 行。
- RustCodeGraph 调用边：`merge` 调用 `merge_one` 和 `GroupingExprs::clone`；`GroupingSets::to_pb` 调用 `GroupingSet::to_pb`；`adjust_nullability_from_grouping_sets` 调用 `all_sets_col_ids`；`deduplicate_gby_expression` 调用 `CanonicalHashCode` 与 `CloneExpr`。图中的直接上游调用者集中在独立 Rust 测试。
- 读取的 Rust 装配与依赖文件：[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)。仓库搜索确认目标模块是私有 `grouping_sets_kernel`，测试态再导出，且未找到其他 Rust 生产调用点。
- 读取的独立 Rust 测试：[`grouping_sets_test.rs`](./grouping_sets_test.rs)，覆盖前缀合并与目标布局、ROLLUP 可空性、去重/还原、阈值 gid 和非 Column panic；`lib.rs` 还挂载 `grouping_sets_runtime_aster_unit_test.rs`。
- 读取的 Go 对照：[`grouping_sets.go`](./grouping_sets.go) 全部核心符号及 [`grouping_sets_test.go`](./grouping_sets_test.go) 的目标布局、合并、ROLLUP、去重/还原和 distinct gid 场景。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构验证要求本文件存在且恰有十一个规定的二级标题；验证命令及退出码在任务交付时报告。
