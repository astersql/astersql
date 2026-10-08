# `pkg/planner/core/operator/physicalop/physical_index_join.rs`

## 文件定位

本文件属于 `astersql-planner-core-operator-physicalop` crate；crate 入口由同目录 `lib.rs` 声明 `mod physical_index_join` 并以 `pub use physical_index_join::*` 导出。它定义 Index Lookup Join 家族共享的物理计划状态，以及“最后一个索引列由外表行动态决定范围”所需的比较过滤管理器。`PhysicalIndexHashJoin` 和 `PhysicalIndexMergeJoin` 都内嵌这里的 `PhysicalIndexJoin`，因此这里不是单一算子的孤立实现，而是三个 Index Join 变体的公共基座。

在规划主链中，`base_physical_plan.rs` 根据连接提示、任务类型、存储引擎、内表可用索引和连接键可解析性创建 `PhysicalIndexJoin`，随后由 `index_join_probe.rs` 选择内表探测路径、构造模板范围及可选的末列比较管理器，最后在挂接阶段克隆计划并补齐 `InnerPlan`。本文件保存和转换这些信息，但不负责真正读取内表数据。

`Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/planner/core/operator/physicalop`；本文件的直接 Go 对照是同目录 `physical_index_join.go`。

## 核心职责

1. `PhysicalIndexJoin` 保存连接基座、内表探测计划、模板范围、连接键到索引列的映射、前缀索引长度、哈希键及解相关来源标记。
2. `CompleteHashKeys` 以 `OuterJoinKeys`/`InnerJoinKeys` 为前缀，从原始 `EqualConditions` 中补收跨内外孩子的裸列等值条件，供 Index Hash Join 使用；`append_outer_hash_key_pair` 负责方向校正、`IN` 操作数排除和重复对去除。
3. `ResolveIndices` 把连接键、三类条件、哈希键和动态比较表达式解析到实际孩子 Schema，并重新建立输出列到合并孩子 Schema 的位置映射。
4. `ColWithCmpFuncManager` 收集末个索引列的 `lt/le/gt/ge` 比较；它可以按外表行计算比较参数、生成列范围，并按受影响列对外表行做字典序比较。
5. `Clone`、`MemoryUsage`、`ExplainInfoInternal`、代价和任务挂接方法提供物理计划生命周期所需的复制、核算、解释和接口适配。
6. `NormalizeNestedIndexJoinBuildExchange` 是一个独立的 MPP 规范化辅助：当 Index Join 下极小 build 侧面对较大 probe 侧时，将 build Exchange 改成 Broadcast 并清空哈希列，避免两侧同时保留 Hash Exchange。

## 主要符号

- `append_outer_hash_key_pair(...) -> bool`：仅接受一列属于 outer、另一列属于 inner 的跨孩子等值对；任一列带 `InOperand`、方向无法判断或已有同一对时返回 `false`，否则克隆并同步追加两个键向量。
- `NormalizeNestedIndexJoinBuildExchange(sender, build_rows, probe_rows) -> bool`：只有 `build_rows <= 1`、`probe_rows > 512` 且 build 小于 probe 时修改 sender；成功时设为 `tipb::ExchangeType::Broadcast` 并清空 `HashCols`。
- `PhysicalIndexJoin`：核心计划结构。`BasePhysicalJoin.InnerChildIdx` 决定内外侧；`InnerPlan` 是实际索引探测子计划；`Ranges`、`KeyOff2IdxOff`、`IdxColLens` 和 `CompareFilters` 描述查找键与动态范围；`OuterHashKeys`/`InnerHashKeys` 必须成对对齐；`EqualConditions` 保留逻辑连接原始等值条件；`FromDecorrelatedApply` 标记解相关来源。
- `PhysicalIndexJoin::New` / `Init`：前者以空容器构造对象，后者安装类型名 `IndexJoin`、统计信息和两个孩子要求属性。
- `PhysicalIndexJoin::CompleteHashKeys`：重建而不是增量修改哈希键，先复制基本 Join Key，再扫描函数名为 `EQ` 且能由 `IsColOpCol` 识别的列列条件。
- `PhysicalIndexJoin::Clone`：克隆连接基座和 `InnerPlan`，深拷贝范围/向量/表达式，并通过 `cloneForPlanCache` 重建比较函数，错误类型为 `expression::Error`。
- `PhysicalIndexJoin::ExplainInfoInternal`：生成连接类型、inner/left side、连接键、补充等值条件以及 left/right/other 条件；normalized 模式使用稳定排序的归一化表达式文本，Index Merge Join 会抑制哈希等值条件部分。
- `PhysicalIndexJoin::ResolveIndices`：解析所有表达式位置，并对重复输出列使用 `used` 位图保证合并 Schema 中每个出现位置最多匹配一次；Left Outer Semi/Anti Left Outer Semi 的最后一个匹配标志列不参与孩子列解析。
- `ColWithCmpFuncManager`：`TargetCol` 是内表索引目标列，`OpType`、`OpArg`、`TmpConstant` 三个向量按位置对应；`AffectedColSchema` 和私有 `compare_funcs` 按位置对应。
- `ColWithCmpFuncManager::AppendNewExpr`：追加操作符、参数和类型对齐的临时常量，去重合并受影响列后重建比较函数。
- `ColWithCmpFuncManager::BuildRangesByRow`：对当前 outer row 逐个求值 `OpArg`，将结果写入 `TmpConstant`，构造“目标列 op 常量”表达式并调用 `ranger::BuildColumnRange`。
- `IndexJoinInfo`：传递索引选择反馈的轻量结构，含列长度、键映射、范围与比较过滤；本文件只定义数据形状。
- `EMPTY_COL_WITH_CMP_FUNC_MANAGER_SIZE`：用于比较过滤管理器内存核算的空结构大小。

## 执行流程

规划阶段的实际路径如下：

1. `base_physical_plan.rs` 在 RootTask、连接键非空且内侧可构造 TiKV lookup scan 等约束满足时选择内外侧，建立两侧 `PhysicalProperty`，调用 `PhysicalIndexJoin::New(...).Init(...)`。
2. 规划器复制逻辑 Join 的 `EqualConditions`，必要时交换参数使内表键位于预期一侧，然后调用 `CompleteHashKeys`。因此哈希键以普通 Index Join 键为前缀，剩余可用列列等值条件在后。
3. `index_join_probe::best_probe/source_probe` 遍历内表候选访问路径，剔除不适用或非 TiKV 路径，将连接键映射为索引列位置，并通过 ranger 构造带占位值的模板范围。
4. 若等值前缀后的下一索引列可由 outer 表达式上的 `lt/le/gt/ge` 约束，`source_probe` 创建 `ColWithCmpFuncManager`。目标列在比较右边时会反转运算符；引用内表列或不引用任何 outer 列的表达式不会收入管理器。只有附加点范围未触发 range-size fallback 时，管理器与扩展后的模板范围才被保留。
5. 探测结果回填 `PhysicalIndexJoin` 的 `InnerPlan`、`Ranges`、`KeyOff2IdxOff`、`IdxColLens` 和 `CompareFilters`。`attach_canonical_index_join` 也会在缺少 `InnerPlan` 时从孩子任务的索引扫描补建 lookup scan，然后以 `preserve_index_join_variant` 保留普通、Hash 或 Merge 变体并产出 RootTask。
6. 计划最终调用 `ResolveIndices`：先处理基座，再按 `InnerChildIdx` 得出 outer/inner Schema，解析 Join Key、条件、Hash Key、比较参数与受影响列，最后重排输出列下标。
7. 运行期的设计语义是由 `BuildRangesByRow` 对每个 outer row 重新填充末列范围，`CompareRow` 用于 lookup 内容排序/去重。但当前 Rust 仓库未发现这两个方法的有效执行器调用；`pkg/executor/join/index_lookup_join.rs` 和 `index_lookup_merge_join.rs` 中对应代码仍是注释。因而不能把 Rust 动态末列范围执行路径描述为已完整接线。

## 数据与状态

`PhysicalIndexJoin` 的关键不变量是两组键向量成对对齐：`OuterJoinKeys[i]` 对应 `InnerJoinKeys[i]`，`OuterHashKeys[i]` 对应 `InnerHashKeys[i]`。`CompleteHashKeys` 同步推入两侧并去重；`ExplainInfoInternal` 也用 `zip` 消费哈希键，因此扩展代码不能只修改单侧。

`KeyOff2IdxOff` 表示 Join Key 下标到索引列下标的映射，负值代表该索引位置没有可用连接键；`IdxColLens` 保存索引列前缀长度；`Ranges` 是规划期构建、执行期可据 outer row 重填的模板。`InnerPlan: Option<Box<dyn PhysicalPlan>>` 独立于正常两个孩子，表示内表索引查找的具体计划。

`ColWithCmpFuncManager` 内有两组位置对齐关系：`OpType`/`OpArg`/`TmpConstant` 一一对应，`AffectedColSchema.Columns`/`compare_funcs` 一一对应。`AppendNewExpr` 维持这些关系，`cloneForPlanCache` 和 `restore_cache_snapshot` 都在恢复后调用 `rebuild_compare_funcs`。直接改公开向量而不重建派生状态会导致 `CompareRow` 跳过列，或使 `BuildRangesByRow` 按索引访问不一致的向量。

`BuildRangesByRow` 会原地覆盖 `TmpConstant`，所以管理器包含每行求值产生的可变暂存状态；`CompareRow` 本身只读。`MemoryUsage` 统计容器容量及所拥有的列、表达式、常量和比较函数，但它是估算接口，不等同于分配器精确记账。

## 依赖与调用关系

上游直接证据：

- `pkg/planner/core/operator/physicalop/base_physical_plan.rs` 创建、补全、克隆并挂接 `PhysicalIndexJoin`，且调用 `CompleteHashKeys`。
- `pkg/planner/core/operator/physicalop/index_join_probe.rs` 读取 Join Key/OtherConditions，创建 `ColWithCmpFuncManager` 并调用 `AppendNewExpr`，形成内表 `PhysicalIndexScan` 候选。
- `pkg/planner/core/optimizer_runtime.rs` 在 Index Join 子树的 Exchange 规范化中调用 `NormalizeNestedIndexJoinBuildExchange`。
- `physical_index_hash_join.rs` 与 `physical_index_merge_join.rs` 通过组合和 `Deref` 复用本类型；`lib.rs` 的 `index_join_base[_mut]` 统一取得三个变体的基座。
- `cache_snapshot.rs` 捕获/恢复本类型及比较管理器，`plan_clone_generated.rs` 实现计划缓存克隆。

主要下游依赖：

- `base`：`PhysicalPlan`、`Plan`、`Task`、上下文和 Join 类型。
- `expression`：列、常量、Schema、标量函数、求值、克隆和下标解析。
- `ranger`/`rangerctx`：模板范围、逐行范围构造、Row 和列比较函数。
- `property`/`costusage`：统计、孩子要求属性、TaskType 和两版成本接口。
- `tipb`：MPP Exchange 类型；`mysql`：构造表达式返回类型。

crate 边界由 `physicalop/Cargo.toml` 明确声明，上述依赖均为显式依赖；`tipb` 固定到 Git revision `07f0ea6b6bffa9d8ac100d81ee51dbbfe4dda3bf`，其余核心组件主要是 workspace 内路径 crate。本文件没有 feature 或条件编译分支。

## 错误处理与边界

- `append_outer_hash_key_pair` 和 `NormalizeNestedIndexJoinBuildExchange` 用 `false` 表示“不适用/未修改”，不是错误。
- `Clone` 会传播连接基座或 `InnerPlan` 克隆产生的 `expression::Error`。
- `ResolveIndices` 在孩子少于两个时直接 `Ok(())`；对正常二元连接，它会传播任一表达式解析失败。合并 Schema 不可用，或输出列无法在孩子中找到唯一的对应出现位置时，返回带上下文的表达式错误。
- Semi Join 两种带匹配标志的类型会从待解析输出数中减去一，避免把生成标志误当作孩子列。
- `BuildRangesByRow` 明确拒绝缺失 `TargetCol` 或目标列类型；表达式求值、函数构造与 ranger 建范围错误均原样传播。
- `CompareRow` 对缺失的 `compare_funcs[index]` 选择跳过而非 panic。这提高了损坏派生状态时的容错性，但也意味着调用者必须通过构造/克隆/快照恢复入口保持比较函数完整，不能把“跳过”当作有效降级语义。
- `ExplainInfoInternal` 对生成等值表达式失败的项目使用 `filter_map` 忽略；因此解释文本可能少于键对数，不能作为计划正确性的唯一验证来源。
- 成本方法的 Rust 当前实现并不完全等价于 Go：`GetCost` 是本地简化公式，而 V1/V2 委托基座的缓存成本；Go 则委托 `utilfuncp` 中 Index Join 专用成本函数。调整成本时必须在专门成本模块核对，而不能只改本文件。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。计划对象在优化和任务挂接阶段主要通过拥有式字段与显式克隆流转；`Box<dyn PhysicalPlan>` 管理内表计划生命周期，向量和表达式由 Rust 所有权释放。

需要注意的可变状态是 `BuildRangesByRow(&mut self, ...)` 对 `TmpConstant` 的逐行覆盖。因此同一个 `ColWithCmpFuncManager` 不能在没有外层同步或每 worker 独立副本的情况下被多个执行 worker 并发变更。Go 执行器在 worker 初始化时复制 helper；Rust 若补齐执行器接线，也应保持“每执行 worker 拥有独立可变 manager”的生命周期，而不是跨线程共享一个实例。

计划缓存和快照是另一条生命周期边界：`Clone`/`cloneForPlanCache` 克隆可变表达式与常量，`restore_cache_snapshot` 重建不可序列化的比较函数。新增派生字段时必须同步更新这三条路径以及 `MemoryUsage`，否则缓存复用可能携带旧行状态或缺少派生数据。

## 与 Go 版本的对应关系

结构和大部分辅助方法与 `physical_index_join.go` 对齐：字段集合、初始化、克隆、内存统计、EXPLAIN、下标解析、比较管理器、动态范围生成和 `IndexJoinInfo` 均有同名或直接对应实现。Rust 额外保存并深克隆 `EqualConditions`，并通过 `CompleteHashKeys` 实现 Go `completePhysicalIndexJoin` 中从剩余等值条件补齐 Hash Key 的逻辑。

重要差异如下：

- Go 的 `completePhysicalIndexJoin` 在 `pkg/planner/core/exhaust_physical_plans.go` 回填 `IndexJoinInfo`，Rust 的对应工作分散在 `base_physical_plan.rs` 与 `index_join_probe.rs`。
- Go `ResolveIndices` 依靠有序 Schema 双指针处理重复列；Rust 使用“逐输出搜索 + `used` 位图”，并额外支持输出列重排。独立 Rust 测试覆盖重复列及重排输出。
- Go 的 `BuildRangesByRow` 已由 `pkg/executor/builder.go` 调用，`CompareRow` 已由 Index Lookup/Index Merge Join worker 用于排序和去重；当前 Rust 执行器对应代码仍是注释，因此 Rust 只具备规划数据与辅助实现，未验证完整运行期接线。
- Go 比较管理器克隆时可能共享跨 session 安全的列、Schema 和比较函数；Rust 选择克隆列/Schema 并重建比较函数，所有权边界更明确。
- Go 成本方法使用 Index Join 专用 `utilfuncp`；本文件 Rust 的成本路径较简化，属于已识别但非本文档任务修复范围的迁移差异。
- `NormalizeNestedIndexJoinBuildExchange` 是 Rust 当前规划器中的局部规范化辅助，在 Go 同路径文件中没有对应同名函数。

相关独立 Rust 测试 `physical_index_join_test.rs` 验证：跨孩子等值键补齐与去重、比较管理器内存统计、条件/比较 Schema 下标解析、重复输出列和重排输出。相邻的 `index_join_probe_test.rs`、`cache_snapshot_test.rs` 进一步覆盖探测选择和缓存快照，但不是本文件的独立测试主体。Go 同目录 `physical_utils_test.go` 主要覆盖 Index Join 变体识别；执行期比较和建范围行为更多由 executor 测试间接覆盖。

## 扩展指南

- 新增 `PhysicalIndexJoin` 字段时，同步检查 `New`、`Clone`、`MemoryUsage`、`cache_snapshot.rs`、`plan_clone_generated.rs`、Hash/Merge 包装类型以及 Go 对照字段；计划缓存可变字段必须明确深浅拷贝语义。
- 修改 Join Key 或补充等值条件时，保持 outer/inner 向量等长、同序，并扩展 `physical_index_join_test.rs` 中的反向参数、重复键、`InOperand`、非 `EQ` 和非跨孩子条件用例。
- 修改 `ResolveIndices` 时，必须保留 InnerChildIdx 两种方向、重复列每次只消费一个孩子位置、输出重排和 Semi Join 末尾标志列规则；测试仍应放在独立的 `physical_index_join_test.rs`，不要嵌入生产文件。
- 扩展动态比较操作符时，需要同时修改 `index_join_probe.rs` 的候选筛选/反向运算符映射、`AppendNewExpr` 的对齐状态、ranger 支持和执行器接线；尤其要补齐 Rust executor 后才能宣称端到端支持。
- 修改 `ColWithCmpFuncManager` 的公开向量时，应优先提供维护不变量的方法，避免调用者绕过 `rebuild_compare_funcs`。增加字段还要同步 `cloneForPlanCache`、`restore_cache_snapshot` 和 `MemoryUsage`。
- 调整 Exchange 阈值时，修改 `NormalizeNestedIndexJoinBuildExchange` 及 `optimizer_runtime.rs` 调用场景，并补充边界值 `1`、`512`、build/probe 相等和已有 HashCols 清理的独立测试。
- 成本模型扩展不应继续堆入当前简化公式；应先核对 `pkg/planner/core/plan_cost_ver1.rs`、Go `plan_cost_ver2.go` 和 `utilfuncp` 路由，避免三种 Index Join 变体产生不一致排序。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query PhysicalIndexJoin` 定位 Rust 结构于本文件第 76 行及 Go 结构于对照文件第 44 行；`query ColWithCmpFuncManager` 定位本文件结构、`index_join_probe.rs` 的构造入口、`cache_snapshot.rs` 的捕获/恢复和 Go executor 入口；按文件 `node` 完整读取本文件 1–662 行，并报告它被 13 个文件使用。方法级 `callers` 未返回边，故对未覆盖部分使用精确文本检索补证。
- 读取的目标与边界文件：`pkg/planner/core/operator/physicalop/physical_index_join.rs`、`Cargo.toml`、`lib.rs`。
- 读取的直接 Rust 调用证据：`base_physical_plan.rs`（创建、`CompleteHashKeys`、探测结果回填、挂接）、`index_join_probe.rs`（候选索引、模板范围、比较管理器）、`optimizer_runtime.rs`（Exchange 规范化）、`cache_snapshot.rs` 与 `plan_clone_generated.rs`（缓存生命周期）。
- 读取的 Go 对照：`physical_index_join.go`；并以 `pkg/planner/core/exhaust_physical_plans.go`、`pkg/executor/builder.go`、`pkg/executor/join/index_lookup_join.go`、`index_lookup_merge_join.go` 的精确调用点核对规划反馈和执行期语义。
- 测试证据：完整读取 `pkg/planner/core/operator/physicalop/physical_index_join_test.rs`；精确检索 `index_join_probe_test.rs`、`cache_snapshot_test.rs` 和 Go `physical_utils_test.go` 的相关引用。
- 人工复核结论：本文件确实承担 Index Join 公共计划状态、哈希键补齐、下标解析、动态比较范围辅助和生命周期方法；动态比较辅助在当前 Rust executor 中没有有效调用，文档已明确该限制。
- 本任务为纯文档分析，按计划不运行 Cargo；交付只执行任务指定的 11 章节结构检查。
