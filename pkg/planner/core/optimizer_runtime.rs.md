# `pkg/planner/core/optimizer_runtime.rs`

## 文件定位

`optimizer_runtime.rs` 是 `astersql-planner-core` crate 中把逻辑计划变成可执行物理计划的运行时实现。模块由 `pkg/planner/core/lib.rs` 私有声明为 `mod optimizer_runtime`，但 `DoOptimize`、`LogicalOptimizeForMpp`、`PhysicalOptimizeForMpp`、AST 优化入口安装函数和超长列 chunk 复用判断会从 crate 根重新导出。常规 SQL 的直接上游包括 `planbuilder_runtime.rs` 和 `expression_rewriter.rs`；测试夹具也从 crate 根调用这些入口。

该文件属于 Volcano 风格优化主链，而不是只有声明的兼容门面：它执行按位掩码控制的逻辑重写、调用 `physicalop::FindBestTask` 枚举物理任务、计算代价，并做 MPP 边界、投影、索引解析、计划 ID 等后处理。Go 对照主体是同目录 `optimizer.go`。Rust 另有较小的 `optimizer.rs` 紧凑接口；它不是本文件实现的替代入口，二者的参数模型和覆盖范围不同。

## 核心职责

1. 维护优化器的进程级接线和规则目录：`OptimizeAstNode`/`OptimizeAstNodeNoCache`、`AllowCartesianProduct`、`MaxMemoryLimitForOverlongType`，以及与 Go `optRuleList` 顺序一致的 `LOGICAL_RULES` 和 `LOGICAL_RULE_FLAGS`。
2. 由 `logical_optimize_in_place` 按 flag 顺序调度逻辑规则，并在整棵树变换后通过 `refresh_join_order_stats` 刷新派生统计。规则覆盖列裁剪、生成列替换、去相关、聚合消除/下推、谓词与 TopN 下推、外连接改写、Join 重排、静态分区裁剪、全文索引解析、Sequence/Expand 等。
3. 由 `DoOptimize` 组织完整主链：捕获 Plan Replayer 表统计，执行逻辑优化，枚举最优物理任务，再进行 TiFlash/MPP、投影、索引、计划 ID、shuffle 等规范化，最后返回物理计划及 cost model v1 代价。
4. 为 CTE/Mpp producer 提供 `LogicalOptimizeForMpp` 和 `PhysicalOptimizeForMpp`，避免独立 seed 跳过逻辑规则或按普通 RootTask 物理化。
5. 实现 `ShouldSkipReuseChunkForPhysicalPlan` 和 `ShouldSkipReuseChunkForPointGet`，依据字段类型、主机内存、可信直方图、行数上界及单 chunk 最大行数，保守决定超长列是否禁用 chunk 复用。
6. 提供 crate 内测试/特殊路径入口，如 `DoOptimizeForUpdate`、`PhysicalOptimizeForTest`、`PhysicalOptimizeForTestWithWindowConcurrency`，以及仅测试编译时存在的逻辑规则执行轨迹。

## 主要符号

- `OptimizeAstNodeFn`：AST、planner context、解析节点和 `InfoSchema` 到 `(Plan, NameSlice)` 的函数指针类型。`InstallOptimizeAstNode` 将带缓存和无缓存实现各安装一次；两个 `OnceLock` 不允许后续替换。
- `LogicalRule`、`LOGICAL_RULES`、`LOGICAL_RULE_FLAGS`：规则身份、固定执行顺序和稳定 flag 位的一一映射。新增或移动规则必须同步三者及 Go 顺序测试，不能只修改 `match`。
- `logical_optimize_in_place(flag, plan)`：私有逻辑调度器。它先安装 CTE seed 优化回调、初始化规则及 NDV 缩放函数，然后逐项执行开启的规则；末尾递归刷新 Join 顺序相关统计。
- `DoOptimize(ctx, sctx, flag, plan)`：公开根入口。当前 `_ctx` 未被主流程读取，真正状态来自 `sctx` 和逻辑计划；返回 `Result<(PhysicalPlan, f64), expression::Error>`。
- `do_optimize_with_update_projection_policy`：`DoOptimize` 与 `DoOptimizeForUpdate` 的共同实现。更新语句或 `StmtCtx` 已登记逻辑列引用时，保留必要投影/列布局。
- `physical_optimize_without_post`：安装 canonical task/cost routers，清空 canonical task cache，传播 TiFlash 可用性，以空根属性调用 `FindBestTask`，克隆候选、保留 plan ID、解析索引并计算代价。
- `LogicalOptimizeForMpp`、`PhysicalOptimizeForMpp`：面向独立 MPP producer/CTE seed 的逻辑和物理入口。后者要求 `MppTaskType`、允许 enforcer、检查 invalid task，并压缩或提升 producer 局部结构。
- `ShouldSkipReuseChunkForPhysicalPlan`：对真实物理 reader/point-get 使用 schema 与统计判断；`ShouldSkipReuseChunkForPointGet` 是只持有已解析字段类型时的紧凑桥接。
- `with_transient_plan_ids`：用 RAII `Drop` 恢复 plan-ID checkpoint，保证试探性建计划即使提前返回也不永久消耗编号。
- `prepare_tiflash_availability`：显式后序遍历逻辑树和共享 CTE seed，缓存子树是否可用 TiFlash；用 visited 集合避免重复访问共享 CTE。
- `capture_plan_replayer_table_stats`：开关开启时递归查找 `DataSource`，按 table ID 将逻辑统计写入 statement context。

## 执行流程

常规根计划路径如下：

1. `planbuilder_runtime.rs` 或 `expression_rewriter.rs` 构造 `LogicalPlanRef` 并调用 crate 根再导出的 `DoOptimize`。
2. `do_optimize_with_update_projection_policy` 先捕获 Plan Replayer 所需的表统计，并判定更新/锁计划是否必须保持输出 schema。
3. `logical_optimize_in_place` 依据 `flag & rule_flag` 依次运行 `LOGICAL_RULES`。例如列裁剪同时清除失效 Apply/空投影，谓词下推保留根残余谓词，聚合下推先做唯一键消除，Join 重排前提升相关谓词。
4. 若开启 TopN 下推，根部 `Limit + Sort` 可由 `rewrite_root_limit_sort_to_topn` 改写；随后 `physical_optimize_without_post` 通过 `FindBestTask` 选择候选并取得初始代价。
5. 物理树依次经过选择性 TiFlash 等值条件物化、残余 scan filter 保留、常量 Sort 消除、聚合投影注入、TiFlash `count(*)` 改写、广播 Join exchange 规范化、MPP reader 展平/边界折叠、窗口 exchange 保证和根 sender 修复。
6. 根据 update/select-lock 等约束消除冗余投影；必要时恢复 MPP partial aggregate 输入投影、limited index lookup 内外投影或更新 range selection。
7. 运行 fine-grained shuffle，校准多种 Join/MPP/标量子查询 plan ID，填充 index join inner plan，调用 `resolve_indices`，最后按默认根属性重新计算 cost model v1 代价并返回。

MPP CTE 路径先由 `LogicalOptimizeForMpp` 复用同一逻辑规则流水线，再由 `PhysicalOptimizeForMpp` 递归优化尚未处理的嵌套 CTE seed，要求 MPP task，最后处理 producer 专属的聚合、投影与根 TopN 形状。若找不到有效任务，它返回明确错误而不是退回一个占位计划。

## 数据与状态

- 主要可变输入是 `Box<dyn logicalop::LogicalPlan>` 组成的树；逻辑阶段会原位替换节点、children、schema、表达式、统计和缓存。物理阶段以 `Box<dyn base::PhysicalPlan>` 表示拥有所有权的树，改写时通常 clone 后重新安装 children。
- `LogicalRule` 是调度身份；`LOGICAL_RULES` 与 `LOGICAL_RULE_FLAGS` 依索引成对 zip，顺序本身即行为。`NORMALIZE_RULES` 当前只是同一 slice 的别名。
- `LogicalInteractionRules` 与 `DefaultDisabledLogicalRulesList` 是 `RwLock<Vec<_>>` 的进程级配置容器；本文件当前的核心循环并不直接读取禁用列表，不能仅凭名字推断它们已作用于每次调度。
- `OptimizeAstNode*` 是只写一次的全局函数表；`AllowCartesianProduct` 和内存阈值使用原子类型及 `SeqCst` 读取，适合跨线程共享但属于进程全局策略。
- 超长列分类为 `None`、`Unbounded` 或带列集合/总声明长度的 `Bounded`。LongBlob/Blob/JSON/VectorFloat32 一律保守禁用；长 varchar/blob 只有在内存门槛、reader 类型、可信统计和估算的单 chunk 占用都通过时才允许复用。
- 物理候选 cache、statement 的 plan/column ID 分配器和 `StmtCtx` 中的统计/列引用是隐含状态。`ResetCanonicalTaskCache` 每次物理枚举前清空候选缓存，但不能重置 statement allocator；测试轨迹则是 `thread_local RefCell<Vec<LogicalRule>>`，只在 `cfg(test)` 下存在。

## 依赖与调用关系

上游调用边以源码和 RustCodeGraph 为准：`planbuilder_runtime.rs` 在普通查询、insert-select 等路径调用 `crate::DoOptimize`，更新路径调用 `optimizer_runtime::DoOptimizeForUpdate`；`expression_rewriter.rs` 为子查询调用 `DoOptimize`；`logicalop::LogicalCTE` 通过已安装的 CTE seed 回调触发 `LogicalOptimizeForMpp`。`pkg/planner/core/lib.rs` 负责将主要入口暴露为 `astersql_planner_core::*`。

核心下游依赖包括：

- `logicalop-dependency`：逻辑节点 trait、具体节点和谓词下推等树操作；
- `physicalop-dependency`：物理节点、`FindBestTask`、task cache、reader/MPP/Join 后处理和索引解析；
- `rule-dependency`：稳定规则 flag 与初始化；`property-dependency`：根物理属性和统计；
- `expression-dependency`、`aggregation-dependency`：表达式、列身份、聚合描述及统一错误；
- `cardinality-dependency`、`statistics-dependency`、`costusage-dependency`：NDV/row-size 估计、统计加载和代价选项；
- `vardef-dependency`、`variable-dependency`、`memory-dependency`：会话开关、statement 状态、chunk/内存门槛；
- `infoschema-dependency`、`model-dependency`、`table-dependency`、`kv-dependency`：AST 入口、分区/表元数据及存储类型判断。

`pkg/planner/core/Cargo.toml` 将 crate 定义为 `astersql-planner-core`，`autotests = false`，默认 feature 为空，`nextgen` 只转发 deployment/kernel feature。测试由 `lib.rs` 和本文件的 `#[cfg(test)]` 模块显式装配，而非 Cargo 自动发现。

## 错误处理与边界

多数变换统一返回 `expression::Error` 并用 `?` 向 `DoOptimize` 传播；来自 logical operator 的错误会在边界映射为 planner expression error。显式失败包括找不到普通物理计划、找不到 MPP CTE seed 计划、树形不满足必须的 child/schema/column 条件、clone 或 `resolve_indices` 失败。可选模式匹配通常返回原计划，只有语义必需的不变量才报错。

保守边界包括：统计为 pseudo、实时行数/列数为零、reader 类型不受支持、总内存未知/不足时，不放宽超长列 chunk 复用；全文索引 dirty transaction 或残留未解析表达式会拒绝；MPP 入口不会悄悄退回 RootTask。物理树中的多处 `expect` 只用于前面已用类型/长度检查建立的不变量，但未来改变树形时必须同步这些守卫。

`prepare_tiflash_availability` 包含一段受控 `unsafe`：栈中保存裸指针以实现非递归后序遍历。安全前提是遍历期间节点和 CTE seed 的 Box 所有权不被替换，只更新节点内缓存；扩展该循环时不得在指针尚存活时移动或删除节点。

## 并发与资源生命周期

优化过程自身是同步、单调用栈执行，不创建线程、异步任务或 channel。并发安全主要体现在进程级状态：函数入口用 `OnceLock` 一次安装，布尔/阈值用原子读写，规则配置容器用 `RwLock`。测试规则轨迹是 thread-local，因此并行测试之间不会共享轨迹内容。

计划树通过 `Box` 独占，context 及部分共享 CTE 状态通过 `Arc`/`Rc<RefCell<_>>` 或项目自有引用类型共享。`prepare_tiflash_availability` 的 `visited_ctes` 以 `Rc` 地址去重，共享 seed 只处理一次。`with_transient_plan_ids` 的局部 guard 在正常返回和 unwind 时都恢复 checkpoint；物理候选在返回前 clone，避免把 task cache 内部所有权暴露给调用者。

内存方面，超长列决策评估的是一个可复用 chunk 的保留量，而不是整个结果集；非 point-get 行数上限钳制到 `DefMaxChunkSize`。这既是资源策略也是兼容边界，调整阈值可能改变 executor 是否复用 chunk 以及峰值内存。

## 与 Go 版本的对应关系

Go `optimizer.go` 的 `DoOptimize → doOptimize → VolcanoOptimize → logicalOptimize/physicalOptimize → postOptimize` 是主对照。Rust `DoOptimize` 将 Volcano 主链和多项 `postOptimize` 行为集中在本文件；Go 还包含 Cascades 分支和 `adjustOptimizationFlags` 等外围逻辑，因此不能假定两个单文件逐行同构。规则数组的次序与 flag 映射明确要求对齐 Go `optRuleList`/`optRuleFlags`，Rust 测试会观测实际执行次序。

两侧超长列规则都以 120 GiB 主机内存为放宽门槛、以 `MaxBlobWidth * 2` 为单 chunk 阈值，并对无界类型保持保守。Rust 额外提供只接收 `FieldType` 的 point-get 桥接入口；真实物理计划入口则与 Go 一样利用 reader、行数和直方图信息。

Rust 的 `OnceLock`/原子/RwLock 是对 Go 包级变量并发语义的显式实现；`Result`/`?` 对应 Go error 返回。Rust MPP/投影/plan-ID 处理中存在为当前移植树形补齐的局部规范化，不应仅因 Go 函数名不同就删除。当前 crate 的 `package.metadata.porting.go-package = "pkg/planner/core"` 也明确了对照包边界。

## 扩展指南

- 新增逻辑规则时，同时更新 `LogicalRule`、`LOGICAL_RULES`、`LOGICAL_RULE_FLAGS`、`logical_optimize_in_place` 分支和 Go 对照顺序；在独立测试文件（优先 `optimizer_logical_entry_aster_unit_test.rs` 或该规则自己的 `*_test.rs`）验证 flag 关闭/开启、执行顺序及边界，测试不要内嵌到生产文件。
- 新增物理后处理时，先判断应放在 `physical_optimize_without_post` 的候选选择边界，还是 `do_optimize_with_update_projection_policy` 的最终树规范化阶段；必须覆盖普通 RootTask、MPP、CTE seed、update/select-lock 的 schema 与 plan-ID 稳定性。
- 修改 Join 重排、分区裁剪或谓词/聚合下推时，维持列 UniqueID、schema 顺序、表达式 hash/cache、统计刷新和 correlated column 绑定。相关回归应放在已有 `optimizer_logical_entry_aster_unit_test.rs`、`main_test.rs`、`integration_test.rs` 或更聚焦的独立测试文件。
- 修改超长列复用时同步 Go `shouldSkipReuseChunkForPhysicalPlan`，覆盖低内存/未知内存、无界类型、point/batch point get、pseudo/缺失统计、NULL-only 列和最大 chunk 行数；这是 executor 内存风险，不宜只凭声明 `flen` 做乐观放宽。
- 修改 `prepare_tiflash_availability` 时保持共享 CTE 去重和后序语义，并专门检查裸指针安全前提；`optimizer_runtime_tiflash_test.rs` 是最近的独立测试入口。
- 新 API 只有确需跨 crate 使用时才在 `lib.rs` 再导出。测试专用函数保持 `pub(crate)`/`cfg(test)`，避免扩大稳定公共面。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件已索引为 12,592 行。查询确认 `DoOptimize` 位于第 8231 行、`LogicalOptimizeForMpp` 位于第 9312 行、`PhysicalOptimizeForMpp` 位于第 9472 行、`ShouldSkipReuseChunkForPhysicalPlan` 位于第 200 行。
- RustCodeGraph 节点/调用轨迹：`DoOptimize → do_optimize_with_update_projection_policy`；`LogicalOptimizeForMpp → logical_optimize_in_place`，其已索引调用者包括 CTE seed 回调和 `PhysicalOptimizeForMpp` 的嵌套 seed 路径；超长列入口调用 `classify_overlong_schema`、`trusted_histograms`、`has_usable_overlong_type_size_stats`、`physical_reader_row_bound` 与磁盘平均行大小估计。
- 已读生产与装配文件：`pkg/planner/core/optimizer_runtime.rs`、`pkg/planner/core/lib.rs`、`pkg/planner/core/Cargo.toml`、`pkg/planner/core/planbuilder_runtime.rs`、`pkg/planner/core/expression_rewriter.rs`。
- Go 对照：`pkg/planner/core/optimizer.go` 的 `DoOptimize`、Volcano/逻辑/物理/后处理链、`LogicalOptimizeTest` 和 `shouldSkipReuseChunkForPhysicalPlan`。
- Rust 测试证据：`optimizer_logical_entry_aster_unit_test.rs` 验证逻辑规则实际顺序和多类语义屏障；`optimizer_runtime_tiflash_test.rs` 验证共享 CTE seed 的 TiFlash 可用性传播；`plan_replayer_capture_test.rs` 验证开关、表级捕获和重复表；`main_test.rs`/`integration_test.rs` 覆盖完整 `DoOptimize`、正有限代价、非空 schema、MPP/window 等路径。生产文件仅通过 `#[path]` 装配独立测试文件。
- 本任务为纯文档分析，按计划不运行 Cargo；最终仅运行固定 11 章节结构检查，并人工核对本文件未把测试写入生产 Rust、未声称未由源码/测试支持的能力。
