# `pkg/ddl/ddl_tiflash_api.rs`

## 文件定位

该文件属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 的 `[lib]` 指向 `lib.rs`，而 `pkg/ddl/lib.rs` 以 `pub mod ddl_tiflash_api` 将其公开，并在 `cfg(test)` 下把同目录的 `ddl_tiflash_api_test.rs` 作为独立测试模块接入。

它提供 DDL 侧 TiFlash 副本轮询的内存模型和纯计算工具，而不是完整的生产轮询服务。文件中没有会话、InfoSchema、PD HTTP client、DDL owner、定时器或持久化依赖，源码仅导入 `std::collections::{BTreeMap, BTreeSet, VecDeque}`。RustCodeGraph 对主要纯函数未找到 Rust 生产调用者；当前直接使用者是 `pkg/ddl/ddl_tiflash_api_test.rs` 和 `pkg/ddl/tests/tiflash/ddl_tiflash_test.rs`。完整运行时链仍位于 Go 的 `pkg/ddl/ddl_tiflash_api.go`：`pkg/ddl/ddl.go` 的 `(*ddl).Start` 启动 `PollTiFlashRoutine`，后者仅在 owner 上调用 `refreshTiFlashTicker`，访问 InfoSchema/PD、更新进度缓存和表元数据，并按周期修复 placement rule。

因此，从 DDL 框架问题看，本文件自身不是新的 job 状态机、schema-state 转换或 reorg/backfill 实现，也不直接改变 schema version；Go 侧规则修复才会提交 `ActionSetTiFlashReplica` job。Rust 文件当前承担可复用算法和迁移契约，而非完整应用接线。

## 核心职责

文件包含两组相关但独立的 API：

1. `PollTiFlashBackoffElement`、`PollTiFlashBackoffContext` 及其 Go 风格方法精确保留 Go 公开退避契约，包括浮点阈值、容量拒绝策略、校验顺序和 `Get` 返回稳定元素地址的测试可观察行为。
2. `PollTiFlashContext` 与一组 snake_case 纯函数构成较类型化的 Rust 模型：展开逻辑表为物理表状态、筛选可写 TiFlash store、校验并更新进度、计算 placement-rule 差异，以及按物理 ID 执行一轮带退避的轮询。

两套退避模型不可混用：Go 风格池在容量满时拒绝 `Put`，按 `Tick(id)` 驱动每个元素自己的计数；Rust 风格池会把容量至少钳制为 1，用全局 `poll_counter` 判断到期，并以近似 LRU 顺序淘汰旧项。扩展时必须先决定是在维持 Go ABI/测试契约，还是在演进尚未接线的纯函数模型。

## 主要符号

- `type TiFlashTick = f64`：Go `TiFlashTick float64` 的对应类型，供第一套退避 API 使用。
- `PollTiFlashBackoffElement { Counter, Threshold, TotalCounter }`：单 ID 的 Go 风格状态。`NeedGrow` 将整数计数与截断为 `i32` 的浮点阈值比较；`MaybeGrow` 在到期时调用私有 `doGrow`，增长后清零 `Counter`。
- `PollTiFlashBackoffError`：构造参数的四类错误；`Display` 文本与 Go 构造器错误保持一致。
- `NewPollTiFlashBackoffElement`：建立 `Counter=0`、`Threshold=1.0`、`TotalCounter=0` 的盒装元素。
- `PollTiFlashBackoffContext { MinThreshold, MaxThreshold, Capacity, Rate, elements }`：第一套退避池。公开的 `Tick`、`Remove`、`Get`、`Put`、`Len` 使用 Go 命名以便逐项对照。
- `NewPollTiFlashBackoffContext`：按 `max < min`、`min < 1`、`capacity < 0`、`rate <= 1` 的固定顺序校验并创建空池；容量 `0` 合法，但不能放入元素。
- `TiFlashTableReplica`：逻辑表 ID、分区物理 ID、期望副本数和位置标签组成的输入模型。
- `TiFlashReplicaStatus`：物理表粒度的状态，保留逻辑/物理 ID、副本数、可用性与 `[0,1]` 进度。
- `PlacementRule`：物理 ID、副本数和位置标签组成的简化期望规则；它不是 PD HTTP rule 的完整结构。
- `TiFlashPollTick`：逻辑表与累计 tick 的公开数据类型；当前文件没有函数构造或消费它，属于尚未接线的模型。
- `PollTiFlashBackoffEntry`：第二套退避模型的阈值及最近轮询时的全局计数快照。
- `PollTiFlashContext`：第二套退避池，维护容量、阈值范围、整数增长倍率、`entries`、近似 LRU 的 `order` 和公开的全局 `poll_counter`。
- `TiFlashError`：纯函数错误域。当前会产生 `InvalidReplicaCount` 和 `InvalidProgress`；`NoWritableStore` 在本文件内未被返回。
- `load_tiflash_replica_status`：普通表生成一条、分区表按 `partition_ids` 各生成一条初始状态。
- `writable_tiflash_stores`：从 `(store_id, is_tiflash, writable)` 中筛出同时为 TiFlash 且可写的 ID，以 `BTreeSet` 去重并排序。
- `update_replica_progress`：校验有限且位于 `[0,1]` 的进度，更新状态并返回数值是否变化；进度达到 `1.0` 时设置 `available`。
- `desired_placement_rules`：在副本数非零时，将逻辑表展开成物理表规则。
- `refresh_tiflash_placement_rules`：比较期望规则和现有规则，返回缺失/不同的 `updates` 与多余物理 ID 的 `deletes`，但不执行 PD 请求或 DDL job。
- `poll_replica_status`：第二套模型的一轮入口；RustCodeGraph 记录其下游为 `tick`、`need_poll`、`maybe_grow`、`update_replica_progress` 和 `remove`。

## 执行流程

Go 风格退避流程如下：调用方先 `Put(id)`；每轮调用 `Tick(id)`。`Tick` 暂时从映射移出盒装元素，以便同时可变访问元素和只读借用 context；随后在增加计数之前调用 `MaybeGrow`。达到阈值时，`doGrow` 先把过低阈值提升到 `MinThreshold`，再乘 `Rate` 并封顶于 `MaxThreshold`，清零计数；`Tick` 最后将 `Counter` 和 `TotalCounter` 各加一并重新插入元素。因此发生增长的那次 tick 结束后 `Counter` 为 1，而不是 0。

纯函数轮询流程从 `load_tiflash_replica_status` 开始：非分区表以 `table_id` 作为 `physical_id`，分区表只展开分区 ID；初始进度为 0 且不可用。`poll_replica_status` 先全局 `tick`，然后逐状态执行：

1. `need_poll(physical_id)` 未到期则跳过。
2. 本轮无观测值时视为无进展，`maybe_grow(..., false)` 扩大间隔。
3. 有观测值时由 `update_replica_progress` 校验和更新。
4. 已可用则累计 `completed` 并从退避池移除；仍不可用则按进度是否变化选择重置到最小阈值或指数增长。

规则刷新流程为 `refresh_tiflash_placement_rules -> desired_placement_rules`。期望规则使用输入表顺序和分区顺序生成；`updates` 保留该顺序。`deletes` 遍历 `BTreeMap` 键，因此按物理 ID 有序。返回值只是差异计划，调用方仍需负责真实 PD 写入、失败重试及与 DDL job 的协调。

## 数据与状态

第一套 `elements` 使用 `BTreeMap<i64, Box<PollTiFlashBackoffElement>>`。盒装使映射节点被暂时移出和重新插入时元素堆地址保持稳定，供 `Get` 返回的裸指针观察；键排序对算法没有要求。`Put` 对已有 ID 幂等返回 `true`，容量判断使用 `Len() < Capacity`，不自动淘汰。

第二套 `PollTiFlashContext` 的构造器会规范化参数：容量和最小阈值至少为 1，最大阈值不低于规范化后的最小值，增长倍率至少为 2。`tick`、阈值相加和阈值相乘均使用饱和算术，避免 `u64` 溢出。`entries` 保存每个物理 ID 的退避状态，`order` 在每次 `maybe_grow` 时把该 ID 移到尾部，并在超容量时从头部淘汰；它是基于更新次序的近似 LRU，不记录只读 `need_poll/get`。

表状态以 `physical_id` 为退避键。`pkg/ddl/ddl_tiflash_api_test.rs::partition_backoff_is_keyed_by_physical_id_like_go` 证明同一逻辑表的两个分区可以独立推进：已完成分区从池中删除，缺少观测的另一分区仍保留。`replica_count` 和 `location_labels` 只参与初始状态或规则计算，不在轮询中变化。

## 依赖与调用关系

Rust 源码的直接依赖仅为标准库集合类型；`pkg/ddl/Cargo.toml` 没有为本文件引入专属外部 crate 或 feature gate。模块由 `pkg/ddl/lib.rs` 无条件公开，测试模块则只在 `cfg(test)` 下编译。

RustCodeGraph 索引包含本文件 46 个符号，并显示：

- `poll_replica_status` 调用本文件的 `PollTiFlashContext::{tick, need_poll, maybe_grow, remove}` 与 `update_replica_progress`；图中还出现一个同名 `remove` 的误匹配候选，源码调用接收者明确是本地 `context`。
- `refresh_tiflash_placement_rules` 调用 `desired_placement_rules`，并查询现有 `BTreeMap`。
- Rust 生产代码中没有找到上述纯函数的上游调用；`rg` 确认直接引用位于两份 Rust 测试中。
- Go 生产主链为 `pkg/ddl/ddl.go` 启动 `PollTiFlashRoutine`，再由 owner 周期调用 `refreshTiFlashTicker`；该流程调用 infosync 计算进度、写进度缓存并经 executor 更新副本元数据。NextGen 模式另由 `refreshTiFlashPlacementRules` 检查缺失规则并提交 `ActionSetTiFlashReplica` job。

这意味着 Rust 函数的确定性返回值不能等同于集群已执行相应动作；真实 I/O、owner 生命周期、session pool 和持久化均在当前 Rust 文件边界之外。

## 错误处理与边界

`NewPollTiFlashBackoffContext` 返回具名参数错误，并刻意保持 Go 的检查顺序。例如 `(min=0.5, max=1, rate=1)` 先报最小阈值错误，而 `(min=10, max=1)` 先报最大值小于最小值。等于边界是允许的：`max == min` 有效，`capacity == 0` 有效但 `Put` 必定失败；`rate` 必须严格大于 1。

`desired_placement_rules` 遇到任一表副本数为 0 会立即失败，不返回已构造的部分规则。空表列表合法，并会使刷新逻辑把全部现有规则列入删除集合。分区列表为空被解释为普通表；函数不检查重复物理 ID、负 ID、逻辑/分区 ID 冲突或跨表冲突。

`update_replica_progress` 拒绝 NaN、正负无穷以及范围外数值，错误时不会修改状态。它允许进度下降，只把任何大于 `f64::EPSILON` 的差异视为变化；`available` 会据新值重新计算，所以从 1.0 降低会恢复为不可用。`poll_replica_status` 按切片顺序修改，若后续元素返回错误，先前元素的状态和 context 改动不会回滚，因此该 API 不是事务性的。

`writable_tiflash_stores` 对空输入返回空集合，并不会产生 `NoWritableStore`；因此该错误变体目前只是预留边界，而非已实现行为。`refresh_tiflash_placement_rules` 只比较简化结构的完全相等性，不能覆盖 PD rule 的其他字段、HTTP 错误或并发更新冲突。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、定时器、网络连接、事务或持久资源。两个 context 的可变操作均要求 `&mut self`，Rust 类型系统阻止同一实例被无同步并发修改；若未来跨线程共享，调用方必须自行提供 `Mutex` 等同步和生命周期管理。

第一套 `Get(&mut self) -> (*mut PollTiFlashBackoffElement, bool)` 是特殊风险点。它返回裸指针以保留 Go 测试对元素身份和原地变化的观察方式；解引用必须由调用方使用 `unsafe`，而 `Remove`、context 析构或其他破坏该元素生命周期的操作会使指针失效。现有 Rust 测试只在元素仍位于 context 且未删除时读取。生产扩展不应长期保存该指针，优先新增安全的快照或闭包式访问接口，同时保留旧方法供兼容测试。

第二套 context 完全拥有映射、队列和状态；`remove` 同时清理两处记录，容量淘汰也从两处移除。它没有后台时钟，只有调用 `tick` 才会推进时间。Go 生产实现中的 goroutine、`time.After`、owner 切换、session pool 获取/归还以及退出 context 都不在 Rust 文件内。

## 与 Go 版本的对应关系

`TiFlashTick`、`PollTiFlashBackoffElement`、`PollTiFlashBackoffContext`、`NewPollTiFlashBackoffElement`、`NewPollTiFlashBackoffContext` 以及 `Tick/NeedGrow/doGrow/MaybeGrow/Remove/Get/Put/Len` 与 `pkg/ddl/ddl_tiflash_api.go` 前半段逐项对应。关键语义包括：初始阈值固定为 1 而非构造器的 `MinThreshold`；增长发生在本次计数递增之前；容量满不淘汰；浮点倍率可为 1.5；错误文本和校验次序保持一致。`pkg/ddl/tests/tiflash/ddl_tiflash_test.go::TestTiFlashBackoffer` 与 Rust 集成测试中的对应断言覆盖增长序列、封顶、容量、删除和非法参数。

Rust 的 `TiFlashReplicaStatus` 并非 Go 同名结构的字段级复刻。Go 结构记录 ID、标签、逻辑表可用性、优先级和分区标记；Rust 简化模型改为显式逻辑/物理 ID 和数值进度。类似地，`PollTiFlashContext`、`TiFlashTableReplica`、`PlacementRule` 及 snake_case 函数是为可测试算法抽取出的 Rust 模型，没有对应 Go 同名 API。

Go 的完整文件还实现 `TiFlashManagementContext`、store 刷新、InfoSchema 扫描、已可用表进度缓存、failpoint、ticker、DDL owner 限制、PD placement rule 查询与 job 修复；这些均未移植到本 Rust 文件。Rust 的 `load_tiflash_replica_status` 也只展开 `partition_ids`，未覆盖 Go `LoadTiFlashReplicaInfo` 的 adding partitions、high priority、logical-table availability 等字段。文档或调用方不得据纯函数测试推断完整 Go 运行时已经由 Rust 替代。

## 扩展指南

若修正 Go 风格退避行为，应修改第一组符号，并同步 `pkg/ddl/tests/tiflash/ddl_tiflash_test.rs` 中 Go 对照的 backoffer 测试；特别注意浮点阈值转整数的时机、增长后本轮计数为 1、零容量和错误优先级。不要把这组测试内嵌回源文件，仓库要求 Rust 源文件和单元测试分离。

若扩展纯函数模型，应按目标选择入口：表/分区展开改 `load_tiflash_replica_status`，store 资格改 `writable_tiflash_stores`，进度约束改 `update_replica_progress`，期望规则改 `desired_placement_rules`，差异算法改 `refresh_tiflash_placement_rules`，轮询退避改 `PollTiFlashContext` 与 `poll_replica_status`。同步更新 `pkg/ddl/ddl_tiflash_api_test.rs` 的物理 ID 隔离回归，以及 `pkg/ddl/tests/tiflash/ddl_tiflash_test.rs` 的规则和进度测试。

若目标是接入真实运行时，则不能只调用这些纯函数后宣称完成。至少需要明确映射 InfoSchema/`model.TableInfo`、PD store/rule、infosync 进度缓存、owner-only 调度、session pool、错误重试和 `ActionSetTiFlashReplica` job；还要决定 Go 中 adding partition、高优先级、逻辑表可用性与 NextGen write-node 过滤如何表示。此类接线会跨出本文件，应按 DDL job 生命周期和兼容性要求另立范围，并增加独立集成测试。

性能方面，表展开和规则比较会克隆分区 ID/位置标签；`maybe_grow` 每次用 `VecDeque::retain` 移动项目，复杂度为池大小线性；规则差异依赖树结构，为对数查询且结果稳定排序。扩大默认容量或高频调用前应评估这些成本。兼容方面，公共 Go 风格名称、错误字符串和裸指针可观察行为已经被测试依赖，不应无迁移方案地改动。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ddl/ddl_tiflash_api.rs` 确认目标文件已索引且含 46 个符号。
- RustCodeGraph：`node --file pkg/ddl/ddl_tiflash_api.rs --offset 1 --limit 1200` 阅读了 528 行完整源码；`query` 唯一定位 `poll_replica_status` 和 `refresh_tiflash_placement_rules`，并区分 Go/Rust 两个 `NewPollTiFlashBackoffContext`。
- RustCodeGraph：`callees poll_replica_status` 与 `callees refresh_tiflash_placement_rules` 核对了内部调用边；`callers poll_replica_status` 无输出，随后以仓库 `rg` 交叉确认 Rust 上游引用仅在测试中。
- 源码与 crate 边界：`pkg/ddl/ddl_tiflash_api.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Go 对照与生产入口：`pkg/ddl/ddl_tiflash_api.go`、`pkg/ddl/ddl.go`；前者包含 `PollTiFlashRoutine`、`refreshTiFlashTicker`、`refreshTiFlashPlacementRules`，后者启动轮询 routine。
- 独立 Rust 测试：`pkg/ddl/ddl_tiflash_api_test.rs`；另读 `pkg/ddl/tests/tiflash/ddl_tiflash_test.rs` 中 backoffer、placement-rule 和 progress/backoff 场景。
- Go 测试：`pkg/ddl/tests/tiflash/ddl_tiflash_test.go::TestTiFlashBackoffer` 及相邻 TiFlash 轮询场景。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认目标文件存在且恰好包含上述 11 个固定二级标题，并人工检查唯一生产物、源码链接、未接线声明和测试位置。
