# `pkg/executor/typed_hash_join.rs`

## 文件定位

本文件属于 `astersql-executor` crate（见 `pkg/executor/Cargo.toml` 的 `[package]` 与 `[lib] path = "lib.rs"`），由 crate 根模块 `pkg/executor/lib.rs` 通过 `pub mod typed_hash_join` 暴露。它实现 typed execution 路径中的内存 Hash Join：上游 `pkg/executor/builder.rs::build_typed_physical_plan` 在识别到 `PhysicalHashJoin` 后递归构造两个 typed 子执行器，再调用 `TypedHashJoin::new`；下游则通过 `ExecExecutor` 接口被会话侧当作普通执行器拉取。

它不是 `pkg/executor/join/hash_join_v1.rs` 或 `hash_join_v2.rs` 的并发/可落盘实现，也没有同名 Go 文件。当前实现固定以右孩子为 build side、左孩子为 probe side，把右侧全部物化到内存，主要为 typed physical-plan adapter 提供一条可执行的 Hash Join 路径。

## 核心职责

- `TypedHashJoin::new` 校验左右等值键数量和 `IsNullEQ` 数量，按 `JoinType` 计算输出 schema，并初始化分页状态。
- `build` 同步读完右孩子；每一行都保存到 `build_rows`，满足右侧过滤且能产生 hash key 的行才进入 `buckets`。保留未入桶行是 Full/Right Outer Join 尾扫输出未匹配右行所必需的。
- `next_inner` 分块读取左孩子，按编码后的连接键查桶，逐候选执行 `other_conditions`，并把未完成的候选列表保存在 `pending_*` 字段中，使一对多结果能跨输出页续跑。
- `finish_probe` 集中实现左外连接补 NULL、Semi/Anti Semi 去重输出，以及 Left Outer Semi 两种三值布尔结果。
- probe 完成后，Right/Full Outer Join 扫描 `build_rows[].matched`，输出所有未匹配右行。
- `ExecExecutor` 实现负责打开/关闭两个孩子、取消检查、schema/chunk 配置、外键级联委托和扫描行数汇总。

## 主要符号

- `StoredRow { values, matched }`：内部 build-row 表示。`values` 是右行的拥有型 `Vec<Datum>`；`matched` 记录是否至少通过一次残余条件，用于右外/全外连接的尾扫。
- `TypedHashJoin`：唯一公开类型。其字段可分为五组：两个子执行器；键、过滤条件和 `JoinType`；`build_rows`/`buckets` 构建态；`probe_chunk`、`pending_*`、`unmatched_build_index` 分页游标；`opened`/`closed` 生命周期标志。
- `TypedHashJoin::new(...) -> Result<Self, String>`：唯一公开固有方法。键数量不一致，或非空 `null_equal` 长度与键数量不一致时立即失败。
- `key`：逐个执行 `Column::Eval`；普通等值键遇 NULL 返回 `None`，对应位置启用 `null_equal` 时则继续编码。最终调用 `astersql_util_codec::EncodeKey`，因此复合键比较以统一编码字节为桶键。
- `side_conditions_match`：通过 `CNFExprs`/`EvalBool` 计算单侧过滤，只取 `matched`；空条件直接为真。
- `conditions_match`：在左右值拼成的一行上计算残余条件，保留 `EvalBool` 的 `(matched, is_null)` 两个分量，供 Anti/Semi 的三值逻辑使用。
- `build`、`next_inner`、`finish_probe`：分别对应 build 阶段、probe/分页状态机和单个 probe 行收尾。
- `impl ExecExecutor for TypedHashJoin`：公开运行边界。核心方法是 `Open`、`Next`/`NextWithContext`、`Close`、`ChunkConfig` 与 `Schema`。

本文件没有模块级常量、trait 定义、泛型、宏或条件编译项。

## 执行流程

1. `pkg/executor/builder.rs::build_typed_physical_plan` 对 `PhysicalHashJoin` 做接线约束：拒绝 `SelectLock` 和 null-aware anti join keys，要求恰有两个孩子。非 Full Outer Join 的单侧条件先包装成 `TypedSelection`；Full Outer Join 的左右条件则原样交给本文件，以免提前过滤掉应作为未匹配行输出的数据。
2. `TypedHashJoin::new` 保存计划表达式。Semi/Anti Semi 只输出左 schema；Left/Anti Left Outer Semi 在左 schema 后附加一个 `LONG_LONG` 标记列；其余连接输出左列后接右列。
3. `Open` 先打开左孩子，再打开右孩子。右侧打开失败时立即关闭已经打开的左侧。状态复位后调用 `build(None)` 同步读完右侧；构建失败则调用自身 `Close` 清理两个孩子。
4. `build` 每批调用右孩子 `Next`，检查取消（有上下文时）、计算右键和右侧条件，并将所有右行复制进 `build_rows`。只有过滤通过且 key 非 `None` 的行索引会追加到 `buckets[key]`。
5. `Next` 或 `NextWithContext` 进入 `next_inner`，先清空输出。若有上一页遗留的 `pending_probe`，就从 `pending_index` 继续遍历桶内候选；候选必须通过 `other_conditions` 才设置 probe/build matched 状态并按连接类型输出。
6. Semi 系列一旦找到首个真匹配便跳到候选尾，保证每个左行最多输出一次。若残余表达式结果为 NULL，`pending_null` 被置位；它会抑制 Anti Semi 输出，或使 Left Outer Semi 标记列输出 NULL。
7. 候选耗尽且输出页尚未满时，`finish_probe` 处理当前左行的未匹配/半连接输出并重置 `pending_*`。输出恰好满页时状态保留到下次调用，避免丢失一对多结果。
8. 当前左 chunk 消耗完后再向左孩子取一批。左侧结束后，Right/Full Outer Join 从 `unmatched_build_index` 继续尾扫未匹配右行；其他连接直接结束。当一次调用无法填满 chunk 且所有阶段结束时返回空或部分结果。

## 数据与状态

- `buckets: HashMap<Vec<u8>, Vec<usize>>` 只持有编码键到 `build_rows` 下标的映射；重复键保留下标顺序，所以一个 probe 行可产生多行。零个连接键时编码空键，语义上形成单桶笛卡尔候选集。
- `build_rows` 拥有全部右行，而不只是入桶行。`matched` 只在等值键命中且残余条件为真后设置；右侧过滤失败、普通 NULL key 或残余条件不匹配的右行仍可在 Right/Full Outer Join 尾部输出。
- `probe_chunk` 复用左孩子产生的 chunk；`probe_index` 指向下一行。`pending_probe` 拥有当前左行，`pending_matches` 是候选下标快照，`pending_index` 是跨页游标。
- `pending_matched` 表示当前左行已有真匹配；`pending_null` 表示至少一个残余条件产生 SQL NULL。两者共同实现 Semi/Anti Semi 的三值语义。
- `unmatched_build_index` 是右侧尾扫游标，`probe_done` 切换 probe 与尾扫阶段。`schema` 在构造后不变。
- `page_keys` 是 `TakeLockKeys` 的返回缓存，但当前 `build` 和 `next_inner` 都调用孩子的 `TakeLockKeys()` 后丢弃结果，且本文件从未向 `page_keys` 写入；因此当前实现返回空锁键。测试 `typed_hash_join_covers_join_types_duplicates_outer_rows_and_paging` 明确断言该现状。

## 依赖与调用关系

上游生产调用边为 `pkg/executor/builder.rs::build_typed_physical_plan -> TypedHashJoin::new`；`pkg/executor/lib.rs` 提供模块注册。测试入口 `pkg/executor/typed_hash_join_test.rs::execute` 则通过 `BuildTypedPhysicalPlanWithBindings` 间接构建并调用 `Open -> Next* -> Close`。

主要下游依赖如下：

- `crate::adapter::ExecExecutor` 定义执行器生命周期、chunk/schema、锁键和外键级联接口；`ExecutionContext::sql_killer` 提供取消信号。
- `astersql_planner_core_base::{ContextRef, JoinType}` 提供表达式上下文和八种被测试的连接类型。
- `astersql_expression::{Column, ExprBox, CNFExprs, EvalBool}` 负责键表达式、单侧过滤及残余条件的 SQL 三值计算。
- `astersql_util_codec::EncodeKey` 按会话表达式上下文中的时区把 `Datum` 序列编码为 hash key。
- `astersql_util_chunk` 提供孩子输入、输出分页和临时 joined row；`astersql_types::datum::Datum` 承载拥有型行值。
- `pkg/executor/Cargo.toml` 声明了上述 `astersql-expression`、`astersql-util-codec`、`astersql-util-chunk`、`astersql-types`、planner base/parser types/errors 等工作区依赖；本模块不受该 crate 唯一的 `nextgen` feature 条件控制。

## 错误处理与边界

- 构造阶段只显式拒绝键数量不等和 `null_equal` 非空但长度不等；空 `null_equal` 表示所有键默认不允许 NULL 相等。
- builder 在进入本类型之前拒绝锁定式 Hash Join、NA join keys 和非两个孩子，故这些能力不是本文件已支持行为。
- `key`、`side_conditions_match`、`conditions_match` 把表达式或编码错误转换为 `astersql_errors::New(error.to_string())` 并向上传播；孩子 `Open`/`Next`/`Close` 错误也不吞掉。
- `next_inner` 在未打开或已关闭时返回 `hash join executor is not open`。`Close` 幂等；两个孩子都尝试关闭，`left.and(right)` 在左侧失败时以左侧错误为返回值，否则返回右侧结果。
- 普通等值比较的 NULL key 不入桶；`IsNullEQ` 对应位为真时 NULL 会参与 key 编码。残余条件的 NULL 不算真匹配，但对 Anti Semi 与 Outer Semi 的结果有额外影响。
- 外连接缺失侧使用 `Datum::default()` 表示 SQL NULL。当前实现固定右侧构建，不根据估算、`InnerChildIdx` 或 `UseOuterToBuild` 换边，也没有默认值行、自适应选择、溢写、内存配额或运行统计。
- `pending_probe.as_ref().expect("pending probe")` 依赖状态机不变量：只有 `pending_probe.is_some()` 分支才访问；若以后拆分状态更新，必须保持该不变量。

## 并发与资源生命周期

实现本身是单线程、同步、拉取式状态机，没有线程、异步任务、锁或通道。`Open` 阶段会完整物化右侧，因此首行延迟和峰值内存与右输入大小相关；`Next` 仅分块持有左输入，但 `pending_matches` 会复制一个桶的全部下标。`build_rows` 和 `buckets` 在 `Open` 前复位、在 `Close` 清空；输出页游标在多次 `Next` 间保留。

取消只在 `NextWithContext` 路径通过 `check_cancel` 生效：probe 循环每轮检查，且 `build(context)` 具备检查能力；但公开 `Open` 调用的是 `build(None)`，所以当前同步构建阶段无法利用随后传给 `NextWithContext` 的取消上下文。`Close` 总会尝试关闭左右孩子，右侧打开失败和构建失败也各有清理路径。

外键检查、级联批次、级联上下文与锁等待时长同时委托给两个孩子；`ScannedRows` 汇总两侧。`Detach` 固定返回 `None`。这些委托不引入本地并发资源。

## 与 Go 版本的对应关系

仓库没有 `pkg/executor/typed_hash_join.go`，因此不能声称逐函数移植。最近的 Go 生产语义证据是：

- `pkg/executor/builder.go::buildHashJoinFromChildExecs` 同样从 `PhysicalHashJoin` 读取 `IsNullEQ`、左右 keys/conditions、`OtherConditions` 和 `JoinType`，并区分 build/probe side；Full Outer Join 的单侧过滤保留在 join 内部。
- `pkg/executor/join/hash_join_v1.go::HashJoinV1Exec` 与 `hash_join_v2.go::HashJoinV2Exec` 都遵循“先构建 hash table，再 probe”的两阶段协议，且通过专门状态扫描外侧未匹配行。
- Rust `TypedHashJoin` 保留这些 SQL 结果语义和 chunk 拉取接口，但属于更窄的 typed adapter 实现：固定右 build/左 probe，单线程同步，无 worker/channel、分区、spill、内存/磁盘 tracker、运行统计、NA join 或动态换边。Go V1/V2 的并发与资源管理细节不能套用为本文件现状。

`pkg/executor/typed_hash_join_test.rs` 是 Rust 的直接语义基准：它覆盖 Inner、Left/Right/Full Outer、Semi、Anti Semi、Left/Anti Left Outer Semi，验证重复 build key、外侧补 NULL、分页续跑、取消、NULL 普通/NULL-safe equality 以及 false residual condition。Go 的 `pkg/executor/join/hash_join_v1.go`、`hash_join_v2.go` 及其测试是算法级对照，而非同文件逐行对应。

## 扩展指南

- 新增连接语义时，先在 `TypedHashJoin::new` 调整 schema，再同步审查 `next_inner` 的真匹配输出、`finish_probe` 的 probe 收尾和 build 尾扫三处；在独立文件 `pkg/executor/typed_hash_join_test.rs` 增加回归，不要把测试嵌入生产源文件。
- 支持 NA join、换 build side 或 planner 默认值时，应同时修改 `pkg/executor/builder.rs` 的前置拒绝/参数接线，不能只改本文件。特别要重新证明输出列顺序、单侧过滤归属与 Full Outer Join 未匹配行语义。
- 优化内存时可从避免 `pending_matches` 克隆、压缩 `StoredRow` 或引入受控 spill 入手，但必须保留跨页游标稳定性和 `matched` 生命周期。若引入并发，`StoredRow::matched`、桶与关闭/取消协议都需要明确同步方案。
- 若要传播锁键，应将两侧 `TakeLockKeys` 的结果汇入 `page_keys`，并新增分页与错误路径测试；当前“读取后丢弃”是已验证现状，不应在无测试时悄然改变。
- 若希望构建阶段可取消，需要让 `Open` 接收/取得执行上下文或延迟 build 到 `NextWithContext`；这会改变首调用时机和错误清理路径，应对齐 adapter 生命周期契约。
- 任何表达式或 key 编码改动都应补充复合键、不同类型/排序规则、NULL-safe equality、残余条件为 NULL 与重复 key 测试；性能上重点关注右侧全量内存、热点桶候选数和临时 joined chunk 分配。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`node --file pkg/executor/typed_hash_join.rs --offset 1 --limit 460` 读取完整核心实现；`query TypedHashJoin` 定位结构体于第 21 行；`query BuildTypedPhysicalPlanWithBindings` 定位 typed 计划入口于 `pkg/executor/builder.rs:162`。对 `Open`/`Next` 等通用名的全库查询存在大量同名歧义，因此生产调用边再由精确文本检索核实。
- Rust 源与接线：`pkg/executor/typed_hash_join.rs`、`pkg/executor/builder.rs`、`pkg/executor/lib.rs`。
- crate 边界：`pkg/executor/Cargo.toml`。
- Rust 独立测试：`pkg/executor/typed_hash_join_test.rs`，包含 `typed_hash_join_covers_join_types_duplicates_outer_rows_and_paging`、`typed_hash_join_honors_cancellation_and_closes_both_children`、`typed_hash_join_applies_null_equality_and_residual_conditions`。
- Go 对照：`pkg/executor/builder.go::buildHashJoinFromChildExecs`、`pkg/executor/join/hash_join_v1.go::{HashJoinV1Exec, Open, Next, Close}`、`pkg/executor/join/hash_join_v2.go::{HashJoinCtxV2, HashJoinV2Exec, Open, Next, Close}`。已确认同目录不存在 `typed_hash_join.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前执行任务规定的 11 章节结构检查，并人工复核文档只陈述上述源码与测试可支持的事实。
