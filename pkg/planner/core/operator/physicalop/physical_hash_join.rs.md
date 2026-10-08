# `pkg/planner/core/operator/physicalop/physical_hash_join.rs`

## 文件定位

本文件定义规划器中的物理哈希连接节点 `PhysicalHashJoin`。它描述优化器已经选择哈希连接之后需要保留的连接键、过滤条件、build/probe 侧、执行存储、MPP shuffle 和 Runtime Filter 元数据；它本身不建立哈希表，也不逐行执行连接。实际执行前，节点通过 `ToPB` 编码为 `tipb::Executor(TypeJoin)`，交给 TiKV/TiFlash 一侧的执行器。

该文件属于 Cargo crate `astersql-planner-core-operator-physicalop`（`pkg/planner/core/operator/physicalop/Cargo.toml`），由 `lib.rs` 的 `mod physical_hash_join` 纳入并经 `pub use physical_hash_join::*` 导出。`lib.rs` 为它单独实现 `ConcretePhysicalOperator`，将 EXPLAIN、下标解析、内存统计、相关列提取、代价查询和 protobuf 编码接入通用 `PhysicalPlan` 接口；`PhysicalApply` 也复用其中的哈希连接骨架。

Rust 候选生成的直接入口位于 `base_physical_plan.rs`：逻辑 Join/Apply 被转换为 `BasePhysicalJoin` 后调用 `NewPhysicalHashJoin`，再填入等值条件、NA 条件、存储类型、MPP/hint 信息并调用 `Init`。计划附着阶段由 `Attach2Task` 转到通用 `PhysicalPlan::attach_to_task`，下游的 `pkg/planner/core/task.rs` 按 `PlanKind::HashJoin` 选择 root 或 MPP 附着流程。

## 核心职责

- `IsGAForHashJoinV2`、`CanUseHashJoinV2` 和 `CanTiFlashUseHashJoinV2` 对连接形态、键、NULL-safe equality、版本与 spill 配置做能力门控，防止把未支持的形态送到 HashJoin V2。
- `PhysicalHashJoin` 保存连接公共基座及本算子专属状态，并通过 `Init`、`Clone`、`Attach2Task` 参与物理候选生命周期。
- `ExplainInfo`/`ExplainNormalizedInfo` 生成可读或归一化的计划描述，覆盖连接类型、等值/NA 等值条件、左右/其它条件、shuffle stream 数和 Runtime Filter。
- `ResolveIndices`/`ResolveIndicesItself` 把逻辑列引用解析为两个孩子 Schema 中的稳定槽位，同时修正连接键、条件和输出 Schema。
- `ToPB` 把节点、条件、两个孩子和 Runtime Filter 编码为 TiPB Join executor。
- `ExtractCorrelatedCols`、`GetCost`、`GetPlanCostVer1/2`、`MemoryUsage`、`RightIsBuildSide` 分别服务相关列分析、代价模型、内存估算与 build/probe 侧判定。
- `RuntimeFilterBuildNode` 实现把本节点暴露给 Runtime Filter 基础设施；当前 `register_runtime_filter` 只检查重复 ID，没有把过滤器加入 `RuntimeFilterList`，不能把它理解成已完成的注册写入。

## 主要符号

- `IsGAForHashJoinV2(join_type, left_keys, null_eq, na_keys) -> bool`：GA 白名单仅含 Left/Right Outer、Inner、Semi、AntiSemi；还要求普通左键非空、NA 键为空且 `null_eq` 全为 false。
- `CanUseHashJoinV2(...) -> bool`：调用内部 `can_use_hash_join_v2_with_non_ga(..., true)`；在 GA 集合之外额外允许 LeftOuterSemi 与 AntiLeftOuterSemi，但仍拒绝 cross join、NA 键与 NULL-EQ。
- `can_use_hash_join_v2_with_non_ga(..., allow_non_ga)`：独立测试可控制非 GA 开关；`allow_non_ga == false` 时先要求 `IsGAForHashJoinV2`。
- `can_tiflash_use_hash_join_v2(...)`：只接受版本字符串 `optimized`、未启用两种 spill 条件、InnerJoin、有普通键、无 NA 键且无 NULL-EQ。
- `PhysicalHashJoin`：包含 `BasePhysicalJoin`、`Concurrency`、`EqualConditions`、`NAEqualConditions`、`UseOuterToBuild`、`StoreTp`、`MppShuffleJoin`、hint/别名标志，以及 Runtime Filter 列表和类型。
- `NewPhysicalHashJoin(base, concurrency, use_outer_to_build)`：建立默认 TiKV、非 shuffle、空条件与空 Runtime Filter 的节点；逻辑条件由上游候选生成器随后填充。
- `DeduplicateOutputColumns`/`ResolveOutputColumns`：先按 `UniqueID` 去掉待解析区间内的重复输出列，再以“一次只能消费一个孩子槽位”的 `marked` 映射解析输出下标；两者为 `pub(super)`，主要服务本文件解析流程和同 crate 测试。
- `Init`：设置上下文、类型名 `HashJoin`、查询块 offset、统计信息和两个孩子的所需物理属性。
- `Clone`：深拷贝基座、ScalarFunction 和 Runtime Filter，并显式保留 `IsNullEQ`、存储/hint/shuffle 等标志。
- `explain`：统一实现普通与归一化 EXPLAIN；LeftOuterJoin 会按左右键顺序重建等值函数参数后再渲染。
- `ResolveIndicesItself`：本文件最主要的可失败变换，详见“执行流程”。
- `ToPB`：本文件的序列化边界，生成 `tipb::Join` 及外层 `tipb::Executor`。
- `RuntimeFilterBuildNode for PhysicalHashJoin`：提供 plan ID、build 侧和支持类型；注册函数目前没有状态写入。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

1. 候选生成：`base_physical_plan.rs` 从逻辑 Join 构造 `BasePhysicalJoin`，读取会话并发度后调用 `NewPhysicalHashJoin(...).Init(...)`，再复制普通/NA 等值条件，设置 `StoreTp`、MPP shuffle、hint 和别名状态，并依据 join 类型/提示枚举不同 build 侧候选。
2. 能力判定：优化器用 `CanUseHashJoinV2` 检查 root HashJoin V2 形态；TiFlash 路径用 `CanTiFlashUseHashJoinV2` 同时读取 `TiFlashHashJoinVersion`、external join 阈值、单节点内存配额和 spill ratio。
3. Task 附着：`Attach2Task` 进入通用附着接口；`pkg/planner/core/task.rs` 的 HashJoin 分派处理 root/MPP 任务、exchange 与分区键，使节点进入可执行计划树。
4. 下标解析：`ResolveIndices` 先让 `PhysicalSchemaProducer` 解析通用部分，再调用 `ResolveIndicesItself`。
   - 普通等值条件必须恰有两个参数；先按左参数/左 Schema、右参数/右 Schema 解析，失败时允许交换两个参数再试，并同步更新 `LeftJoinKeys`/`RightJoinKeys`。
   - NA 等值条件固定按左、右 Schema 解析，不尝试反转；两类条件都要求解析结果为 `Column`、键数组长度足够，并在改写后清理函数 hash code。
   - 已保存的普通左右键再次解析；若 `UniqueID` 已因重排或投影变化失配，仅在列字符串在目标 Schema 中唯一时按名字补救，否则保留原始错误。
   - 左/右条件分别对各自 Schema 解析；其它条件对合并 Schema 解析。其它条件也有唯一列名替换的补救路径，以覆盖 Join reorder/聚合键投影产生的新 `UniqueID`。
   - 输出 Schema 对 Outer Semi 两种类型跳过最后的匹配标志列；其余待解析列先按 `UniqueID` 去重，再映射到合并孩子 Schema。若未全部解析，整个操作返回错误；成功后替换节点 Schema。
5. 计划展示与估算：通用接口调用 `Explain*`、相关列提取、内存和代价方法。`GetCost` 的本地简化值是非负左右行数之和除以至少为 1 的并发度；V1/V2 计划代价方法当前委托 `BasePhysicalPlan` 的缓存/计算接口。
6. 下推编码：`ToPB` 拒绝普通键与 NA 键并存，选定一套键与等值条件，编码左右键及三类过滤条件；Anti/Semi 类把来源于 `IN` 的等值“其它条件”拆到 `other_eq_conditions_from_in`；随后编码字段类型、Runtime Filter、join 类型、inner index、两个孩子、NULL-aware/NULL-EQ 标志和 fine-grained shuffle 参数，最终返回 `TypeJoin` executor。

## 数据与状态

`BasePhysicalJoin` 是权威的公共状态，包含 `JoinType`、左右普通/NA 键、`IsNullEQ`、左右/其它条件、`InnerChildIdx`，并经 `PhysicalSchemaProducer` 持有上下文、统计、Schema、孩子和 shuffle 信息。本文件中的 `EqualConditions`/`NAEqualConditions` 与基座的键数组按下标对应；`ResolveIndicesItself` 明确检查这种长度不变量，防止条件和键错位。

`UseOuterToBuild` 与 `InnerChildIdx` 共同决定 build 侧：使用 outer build 时，`InnerChildIdx == 0` 表示右侧 build；常规 inner build 时，`InnerChildIdx != 0` 表示右侧 build。调用方不应只看其中一个字段。`Concurrency` 只进入本文件的旧式 `GetCost` 分母；零值会被 `max(1)` 保护。

`StoreTp` 默认 TiKV，MPP 候选通常由上游改为 TiFlash；`MppShuffleJoin` 记录是否采用 shuffle，而实际 exchange/partition 接线在 task 层。`FromHashJoinHint` 与 `HasTableAlias` 是候选生成阶段的决策元数据，本文件不解释或修改它们。

`RuntimeFilterList` 存放已可编码/展示的过滤器，`RuntimeFilterTypes` 表示允许生成的类型。`RuntimeFilter` 自身保存 build/target 节点 ID、源/目标列和模式；本节点的 trait 注册钩子当前不写列表，所以仅调用 `RuntimeFilter::Assign` 不足以证明 `RuntimeFilterList` 已更新。

## 依赖与调用关系

上游主要关系如下：

- `base_physical_plan.rs` 调用 `NewPhysicalHashJoin` 与 `Init` 生成 Join/Apply 候选，并设置条件、MPP、存储和 hint 状态。
- `lib.rs` 的 `ConcretePhysicalOperator for PhysicalHashJoin` 把本文件方法接入 trait object 计划树；宏 `impl_concrete_physical_plan!` 提供统一的 `PhysicalPlan` 外壳。
- `pkg/planner/core/task.rs` 根据 HashJoin kind 完成任务附着；`plan_cost_ver1.rs`/`plan_cost_ver2.rs` 读取 `RightIsBuildSide` 等状态估价。
- `optimizer_runtime.rs` 等规划流程按 `PhysicalHashJoin` 向下转型，读取 build/probe 键、生成/应用 Runtime Filter 或做计划改写。
- `PhysicalApply` 嵌入 `PhysicalHashJoin`，复用 EXPLAIN、条件、内存和 protobuf 编码语义。

下游依赖包括：`base` 的计划上下文/trait/JoinType/Task，`expression` 的列、Schema、ScalarFunction、解析与 PB 转换，`property` 的统计和物理属性，`costusage` 的代价类型，`kv::StoreType`，会话变量常量 `vardef`，以及固定 git revision 的 `tipb` protobuf 类型。Cargo 文件把 Go 对照包标为 `pkg/planner/core/operator/physicalop`，测试专用上下文来自 `exprstatic` dev-dependency。

RustCodeGraph 将该源文件标为被 29 个文件使用，并在 `base_physical_plan.rs` 找到多个 `NewPhysicalHashJoin` 构造点；精确调用边对 trait 分派和同名 Go/Rust 方法存在歧义，因此调用关系同时以 `lib.rs`、`task.rs` 和候选生成源码作直接证据，而没有把模糊的同名图结果当成唯一依据。

## 错误处理与边界

- V2 判定函数返回 `bool`，不产生诊断；cross join（无普通键）、NA 键、任一 NULL-EQ 或不支持的 JoinType 会被拒绝。TiFlash V2 还拒绝非 optimized 版本和已启用的 spill 配置。
- `ResolveIndicesItself` 在孩子少于两个时直接成功返回；正常构造期应有两个孩子，但该分支让未完整接线的节点不在这里报错。
- 普通/NA 等值函数参数数目不是 2、解析后不是 `Column`、键数组与条件数不一致、左右/其它条件无法解析，或输出列未全部映射，都会返回带上下文的 `expression::Error`。
- 普通等值条件允许左右参数反转恢复；NA 条件不允许。按名字恢复只接受唯一匹配，避免同名列歧义造成静默错绑。
- Outer Semi 输出最后一列是匹配标志，不属于孩子列，因此特意不解析；修改输出 Schema 时必须保留这个例外。
- `ToPB` 要求 `BuildPBContext` 中有 client，并拒绝普通 join key 与 NA join key 同时存在。表达式、字段类型、Runtime Filter 或孩子编码失败都会向上传播错误。
- 未显式匹配的 JoinType 在 PB 映射中回落为 InnerJoin；新增 JoinType 时必须同步更新映射，不能依赖该默认值。
- `ExtractCorrelatedCols` 先使用 `BasePhysicalJoin` 的结果，再补充普通/NA 等值函数，避免漏掉基座条件中的相关列。

## 并发与资源生命周期

本文件没有线程、锁、channel、异步任务或显式资源释放。`Concurrency` 是计划元数据：它描述 probe worker 数并影响 `GetCost`，不在这里创建 worker。两个孩子、表达式和上下文均由计划树/共享上下文所有；`Clone` 创建独立的条件、基座和 Runtime Filter 副本，避免候选改写相互污染。

`ToPB` 只构造 protobuf 对象，不发送 KV 请求；独立测试用会在 `Send` 时 panic 的假 client 验证这一点。真正的网络请求、哈希表内存和 spill 生命周期位于执行层，不应归因于本文件。Runtime Filter 的等待、传播与目标扫描生命周期也在 `physical_plan_misc.rs` 及生成器/执行器侧，本文件只持有规划期描述。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/operator/physicalop/physical_hash_join.go`。Rust 保留了 Go 的主要语义：V2/GA 门槛、TiFlash optimized + spill 限制、同名核心字段、EXPLAIN 分类、相关列提取、build 侧公式、左右/NA 键解析、Outer Semi 匹配列例外、`IN` 条件拆分、JoinType/NULL-EQ/Runtime Filter/孩子的 TiPB 编码。

已核实的实现差异：

- Go 构造器直接接收 `LogicalJoin` 并完成基座和条件复制；Rust 构造器只接收已建好的 `BasePhysicalJoin`，逻辑到物理的组装迁到 `base_physical_plan.rs`。
- Go 的非 GA V2 开关来自 `joinversion.UseHashJoinV2ForNonGAJoin`；Rust 的公开 `CanUseHashJoinV2` 固定允许非 GA 白名单，内部 helper 才暴露 `allow_non_ga` 给测试/局部调用。
- Rust 增加 `FromHashJoinHint`、`HasTableAlias`、公开的 `RuntimeFilterTypes`，并用 `StoreTp` 默认 TiKV；这些服务 Rust 候选路由与运行时过滤接线。
- Rust 的下标解析比当前 Go 文件多出反向解析等值参数、按唯一列名补救 `UniqueID` 漂移、输出列预去重和更细的错误信息；这些是保持 Go 语义结果的兼容边界，而不是放宽歧义绑定。
- Go `GetCost` 和 V1/V2 方法通过 `utilfuncp` 进入 core 专用代价实现；Rust 的 `GetCost` 是本地简式，V1/V2 方法委托 `BasePhysicalPlan`。阅读代价行为时必须继续查看 Rust `plan_cost_ver1.rs`/`plan_cost_ver2.rs` 的外部调用路径，不能假定方法体逐句等价。
- Go `MemoryUsage` 计入结构体固定开销且容忍 nil receiver；Rust 类型没有 nil receiver，只累加 `BasePhysicalJoin` 和两类 ScalarFunction 的内存估算。
- Go 的 `runtimeFilterList` 是私有字段；Rust 列表公开，但 `RuntimeFilterBuildNode::register_runtime_filter` 仍是无写入占位行为。文档只声明当前代码事实，不宣称 Runtime Filter 注册链已经完整等价。
- Rust `ToPB` 显式要求 `BuildPBContext` client 存在，并循环编码任意数量孩子；Go 直接索引两个孩子。正常 HashJoin 仍应恰有两个孩子。

## 扩展指南

- 新增 HashJoin V2 支持形态时，同时修改 `IsGAForHashJoinV2`、`can_use_hash_join_v2_with_non_ga` 和（若涉及 TiFlash）`can_tiflash_use_hash_join_v2`；同步扩展 `physical_hash_join_test.rs` 与 `pkg/planner/core/optimizer_test.rs` 的矩阵，并核对 Go 的 joinversion 开关语义。
- 新增字段时更新 `NewPhysicalHashJoin` 默认值、`Clone`、`MemoryUsage`、计划缓存快照/克隆逻辑及必要的 EXPLAIN/PB 编码；不要只改结构体，否则候选克隆可能丢状态。
- 新增 JoinType 或条件类别时同步更新 `explain`、`ResolveIndicesItself`、`ToPB` 的 JoinType 映射和 `IN` 条件拆分，并在独立测试中覆盖错误与 PB 结果。特别避免让新类型静默回落到 InnerJoin。
- 修改列解析时保留三项不变量：普通条件与键数组下标对齐；同一个孩子槽位不能被重复消费；Outer Semi 最后一列不对孩子解析。对 `UniqueID` 补救只能接受唯一名字匹配。
- 完成 Runtime Filter 注册链时，优先修改 `RuntimeFilterBuildNode::register_runtime_filter` 与生成/分配流程，并验证不会重复注册；同步测试 EXPLAIN、PB 列表和 clone 行为。
- 调整 build 侧选择时同时审查 `RightIsBuildSide`、`base_physical_plan.rs` 的候选枚举、`task.rs` 的 MPP 分区/exchange、代价模型和 Runtime Filter 源/目标列方向。
- Rust 测试必须继续放在独立的 `physical_hash_join_test.rs` 或上层独立测试文件，不能嵌回本源文件；若是 Go 对齐修改，应保留 Go 的真实行为而不是用简化桩通过测试。
- 性能风险集中在条件/Schema 克隆、按列线性查找与 PB 编码；兼容风险集中在 JoinType、NULL/NA 语义、左右键方向和 protobuf 字段。任何变更都应先用定向单元测试证明边界，再走仓库要求的 Ready 验证流程。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 文件、307,296 节点和 1,848,419 条边；`files --filter pkg/planner/core/operator/physicalop` 确认目标源、Go 对照与独立测试均已索引。
- RustCodeGraph 源码读取：`node --file pkg/planner/core/operator/physicalop/physical_hash_join.rs --offset 1 --limit 500` 与 `--offset 501 --limit 500`，覆盖全部 867 行；`query PhysicalHashJoin`/`query NewPhysicalHashJoin` 确认 Rust/Go 同名符号和构造入口。
- RustCodeGraph 测试读取：`node --file pkg/planner/core/operator/physicalop/physical_hash_join_test.rs --offset 1 --limit 500`，确认四类边界：FullOuter + `IsNullEQ` 克隆/PB 保真、重复输出列去重、非 GA feature gate、TiFlash 版本/spill/连接形态矩阵。
- 直接读取 `pkg/planner/core/operator/physicalop/Cargo.toml` 与 `lib.rs`，确认 crate 名称、path 依赖、`tipb` revision、Go package 元数据、模块再导出和 `ConcretePhysicalOperator` 接线。
- 直接读取 Go 对照 `physical_hash_join.go`，逐项核对能力门槛、字段、构造、EXPLAIN、解析、build 侧和 TiPB 编码；读取 Go `optimizer_test.go` 的 TiFlash V2 矩阵作为移植语义证据。
- 直接读取 Rust `base_physical_plan.rs`、`task.rs`、`plan_cost_ver1.rs`、`optimizer_runtime.rs` 和 `physical_plan_misc.rs` 的直接引用，确认候选生成、任务附着、代价使用、Runtime Filter 与下游编码位置。
- 直接读取 Rust `optimizer_test.rs` 与 `integration_test.rs`，确认 V2 判定、MPP NULL-EQ HashJoin 选择和 FullOuter shuffle/左右条件保留的应用级证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的结构验证，要求目标文件存在且恰好包含上述 11 个固定二级标题；另人工检查本文未把执行器行为、模糊调用图或 Runtime Filter 占位接线写成已验证事实。
