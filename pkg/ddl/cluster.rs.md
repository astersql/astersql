# `pkg/ddl/cluster.rs`

## 文件定位

`cluster.rs` 位于 `astersql-ddl` crate，模块由 `pkg/ddl/lib.rs` 的 `pub mod cluster` 公开。它描述 `FLASHBACK CLUSTER` 的核心数据、设置快照和一个简化的内存状态机，用来表达“关闭集群写相关机制、准备、回写、恢复”的顺序约束。

当前文件不是完整的集群闪回执行器：它只使用标准库容器和错误 trait，不访问 DDL job 表、TSO、GC safe point、PD 或 TiKV，也不发送 Region RPC。仓库搜索仅发现 `pkg/ddl/cluster_test.rs` 直接调用这些 Rust API，未发现 Rust 生产调用者；完整线上链路仍在 Go 的 `pkg/ddl/cluster.go`、`pkg/ddl/job_worker.go` 和 `pkg/ddl/executor.go` 中。因此应把本文件理解为已公开但尚未接入 DDL worker 的语义模型，而不是可独立完成闪回的实现。

`pkg/ddl/Cargo.toml` 声明 crate 名为 `astersql-ddl`、库入口为 `lib.rs`，并以 `go-package = "pkg/ddl"` 标注移植来源。本文件自身没有使用 manifest 中的任何外部依赖。

## 核心职责

- `validate_flashback_ts` 对调用方已经取得的当前 TSO、GC safe point 和最早活跃事务时间进行纯值校验。
- `table_key_range` 构造简化的表前缀区间；`merge_continuous_key_ranges` 按排除项切分并合并待闪回区间。
- `close_cluster_for_flashback` 保存 `ClusterSettings` 并切换到闪回安全组合；`restore_cluster_after_flashback` 按成功或取消路径恢复。
- `FlashbackJob::apply_action` 用同步状态转换表达 prepare、flashback、finish 三阶段；`FlashbackJob::cancel` 表达开始回写后不可取消的约束。
- `is_flashback_action_supported` 接受当前 `FlashbackAction` 枚举的全部三个变体。由于入参类型没有其他变体，它在现有类型定义下恒为 `true`；`ClusterError::UnsupportedAction` 当前也没有产生点。

这些职责只验证顺序和数据不变量。真实系统中的并发闪回互斥、schema version 推进、故障恢复、幂等重试和外部副作用不在本文件内。

## 主要符号

- `FlashbackState`：任务状态枚举。合法主路径是 `Pending -> Preparing -> Prepared -> FlashingBack -> Done`，另有终态 `Cancelled`。
- `FlashbackAction`：状态机输入，仅含 `Prepare`、`Flashback`、`Finish`。
- `KeyRange { start, end }`：字节序左闭右开区间 `[start, end)`；类型本身不阻止空区间或反向区间。
- `ClusterSettings`：GC、超级只读、TTL、自动分析以及 PD 调度器列表的内存快照。它没有连接全局变量或 PD 客户端。
- `FlashbackJob`：保存 `flashback_ts`、当前状态、可选设置快照、全部区间和完成区间数。
- `ClusterError`：包括时间戳越界、活跃事务、非法区间和非法状态等错误。`Display` 直接输出 Debug 变体名，没有上下文值或错误源。
- `validate_flashback_ts(flashback_ts, current_ts, gc_safe_point, min_active_start_ts)`：要求目标小于当前 TSO、不早于 GC safe point，并拒绝位于目标之后且不晚于当前 TSO 的最早活跃事务。
- `table_key_range(table_id)`：用字节 `t` 加 i64 大端编码构造起点，以 `saturating_add(1)` 构造终点。
- `merge_continuous_key_ranges(ranges, excluded)`：先校验所有区间 `start < end`，再把普通与排除区间按起点排序；每个排除项都会结束当前普通区间组，组内结果从首个起点延伸到最大的终点。
- `close_cluster_for_flashback` / `restore_cluster_after_flashback`：保存、关闭及恢复设置。
- `FlashbackJob::new`、`apply_action`、`cancel`：构造并推进任务。

文件没有模块级常量、trait、异步函数或条件编译项；所有主要数据类型和函数均为 `pub`，只有 trait 实现细节是内部实现。

## 执行流程

1. 调用方先自行取得时间与集群事实，再调用 `validate_flashback_ts`。本函数不读取 TSO、GC 或事务管理器。
2. 调用方准备 `KeyRange` 列表。可用 `table_key_range` 生成单表简化区间，或用 `merge_continuous_key_ranges` 让排除项成为区间组边界。
3. `FlashbackJob::new` 只记录目标时间和区间，状态为 `Pending`，设置快照为空，完成数为零。
4. 第一次 `Prepare` 调用 `close_cluster_for_flashback`，保存原设置，并将当前设置改为 GC/TTL/自动分析关闭、超级只读开启、PD 调度器列表清空；状态进入 `Preparing`。
5. 第二次 `Prepare` 只把状态改为 `Prepared`。代码没有锁 Region，这个动作仅代表调用方声称准备完成。
6. 第一次 `Flashback` 把状态改为 `FlashingBack`；第二次 `Flashback` 把 `completed_ranges` 一次性设为 `ranges.len()`。代码没有逐区间进度更新或 TiKV RPC。
7. `Finish` 仅在状态为 `FlashingBack` 且完成数等于区间总数时成功。若有保存快照，则按成功路径恢复大部分设置，然后状态进入 `Done`。
8. `cancel` 在 `FlashingBack` 状态拒绝；其他任意状态都会取出快照（若有）、按取消路径恢复全部设置并进入 `Cancelled`。这也意味着当前实现允许从 `Done` 或 `Cancelled` 再次取消，调用方若需要终态幂等约束必须额外限制。

## 数据与状态

`FlashbackJob` 独占自己的内存状态，没有持久化检查点。`saved_settings: Option<ClusterSettings>` 是设置恢复权的标志：第一次准备写入快照，完成或早期取消通过 `take()` 消费快照，避免同一快照被重复恢复。

`completed_ranges` 不是逐区间计数器。它初始化为 0，在第二次 `Flashback` 时直接变为 `ranges.len()`；空区间任务在第一次进入 `FlashingBack` 后便满足完成数条件，所以可直接 `Finish`。`flashback_ts` 在构造后不再由状态机读取，时间合法性必须由调用方在构造或执行前单独保证。

`ClusterSettings` 使用值语义快照。成功恢复 GC、超级只读、自动分析和 PD 调度器，但故意不恢复 TTL；取消恢复包括 TTL 在内的全部设置。这一不对称由 `restore_cluster_after_flashback(cancelled)` 的布尔参数控制。

`merge_continuous_key_ranges` 会消耗普通区间并克隆排除区间。它假设排除项按语义位于普通区间组之间；实现只把排除项当作切分标记，不从重叠普通区间中做集合减法。普通区间之间即使字节上有空隙也会合并，因为 Go 调用方约定这些空隙没有数据。

## 依赖与调用关系

上游装配关系为 `pkg/ddl/Cargo.toml -> pkg/ddl/lib.rs -> pub mod cluster`。RustCodeGraph 能定位 `validate_flashback_ts` 和 `FlashbackJob`，并完整读取 `pkg/ddl/cluster.rs`；调用边查询没有给出生产调用者。仓库文本搜索确认直接引用集中在 `pkg/ddl/cluster_test.rs`。

文件内部调用边如下：

- `FlashbackJob::apply_action` -> `close_cluster_for_flashback`（首次准备）。
- `FlashbackJob::apply_action` -> `restore_cluster_after_flashback`（成功收尾）。
- `FlashbackJob::cancel` -> `restore_cluster_after_flashback`（早期取消且存在快照）。
- `ClusterError` -> `std::fmt::Display` 与 `std::error::Error` trait 实现。

Go 生产链路则是 `pkg/ddl/executor.go` 创建 `ActionFlashbackCluster` job，`pkg/ddl/job_worker.go` 分派到 `worker.onFlashbackCluster`，`pkg/ddl/cluster.go` 完成四阶段推进，并由 `finishFlashbackCluster` 恢复外部设置。这些 Go 调用边不能视为已连接到 Rust 类型。

## 错误处理与边界

`validate_flashback_ts` 按固定顺序返回错误：目标 `>= current_ts` 为 `TimestampInFuture`；目标 `< gc_safe_point` 为 `BeforeGcSafePoint`；活跃事务起点满足 `flashback_ts < start_ts <= current_ts` 时为 `ActiveTransaction`。边界上，目标等于 GC safe point 被接受；活跃事务恰好等于目标不被拒绝。该模型没有 Go 版 min-resolved-ts 等待、上下文取消、读取 safe point 失败等错误。

`merge_continuous_key_ranges` 对普通和排除区间都要求 `start < end`，否则原子地返回 `InvalidRange`。它不检查区间是否重叠、排除项是否真正位于普通区间之间，也不报告排序前置条件；排序在函数内部完成。

`table_key_range(i64::MAX)` 因饱和加法使起止 ID 相同，产生空区间；函数本身不返回 `Result`，因此调用者若把结果交给合并函数会得到 `InvalidRange`。此外它只是 `b"t" + i64::to_be_bytes` 的本地编码，不等同于 Go `tablecodec.EncodeTablePrefix` 的已验证兼容实现。

`apply_action` 对顺序错误、提前完成、终态继续推进统一返回 `InvalidState`，不改变状态。唯一例外是它不验证快照一定存在：若外部直接构造公开字段或移除快照，满足完成条件时仍会进入 `Done` 而不恢复设置。所有字段公开意味着类型不变量依赖调用纪律。

## 并发与资源生命周期

本文件没有锁、原子量、线程、异步任务、通道、事务或网络资源；所有修改都要求调用方持有 `&mut FlashbackJob` 或 `&mut ClusterSettings`，Rust 借用规则只保证单进程内同一可变引用不并发使用。

真正需要跨节点协调的资源——DDL job 持久化、Owner 故障转移、schema version 同步、GC/全局变量、PD scheduler、Region 锁定、start/commit TSO 和 TiKV RPC 重试——均不在这里。Go `onFlashbackCluster` 把阶段和参数写回 `model.Job` 以支持恢复，并通过 worker/session pool、事务、rangetask 和 RPC 管理生命周期；Rust 模型重启即丢失全部进度。

快照生命周期由 `Option::take` 控制：首次准备取得所有权，完成或取消消费它。成功完成后 TTL 保持关闭，依赖外部后续流程决定何时重新开启；取消则立即恢复原 TTL 值。

## 与 Go 版本的对应关系

`pkg/ddl/cluster.go` 是直接对照文件，但 Rust 只复刻了其中一部分意图：

- Rust `validate_flashback_ts` 对应 Go `ValidateFlashbackTS` 的当前时间和 GC safe point 边界意图，但 Go 还读取 `CurrentVersion`、等待 min-resolved-ts、处理 context timeout/cancel，并调用 `gcutil.ValidateSnapshotWithGCSafePoint`；Rust 的 `min_active_start_ts` 检查不是该 Go 函数的同形实现。
- Rust `merge_continuous_key_ranges` 对应 Go `mergeContinuousKeyRanges` 的“排除项切断连续组”约定。Go 输入由 `getFlashbackKeyRanges` 预先排序且不重叠；Rust 自行排序并增加非法区间校验。
- Rust `ClusterSettings` 和关闭/恢复函数抽象 Go 的 `savePDSchedule`、`checkAndSetFlashbackClusterInfo` 与 `finishFlashbackCluster`。Go 操作真实 PD 配置、GC 和全局变量，并把原值保存在 `model.FlashbackClusterArgs`；Rust 只改内存结构。
- Rust 状态 `Pending/Preparing/Prepared/FlashingBack/Done` 是对 Go `StateNone/DeleteOnly/WriteOnly/WriteReorganization/Public` 的语义重命名，不是一一复用 schema state。Go 每阶段更新 job 参数和必要的 schema version，并实际执行 Region split、prepare RPC、flashback RPC 与 notifier 事件。
- 两侧都保留“进入实际回写阶段后不可取消”和“成功后 TTL 保持关闭、取消时恢复 TTL”的行为意图。Go 由 job cancellation 状态及 `finishFlashbackCluster` 实现，Rust 由 `cancel` 与 `restore_cluster_after_flashback` 实现。
- Go `isFlashbackSupportedDDLAction` 判断的是闪回期间其他 DDL action 是否允许，大多数动作返回 true、部分 placement/TiFlash 动作返回 false；Rust `is_flashback_action_supported` 判断自身三种阶段动作且恒为 true，二者名称相近但语义并不等价。

Go 测试 `pkg/ddl/cluster_test.go` 覆盖真实 job 阶段、闪回期间拒绝新 DDL、全局变量和取消规则；Rust 独立测试 `pkg/ddl/cluster_test.rs` 覆盖生产函数的区间合并、晚期取消和设置恢复，并另有测试内简化模型复述 Go 场景。Rust 测试未证明真实 PD/TiKV/持久化链路可用。

## 扩展指南

- 若要把模型接入真实 DDL 执行，入口应连接 job 创建/分派层，而不是让 SQL executor 直接调用内存状态机；同时需要持久化 `flashback_ts`、区间、外部设置原值和阶段 TSO，并接入 schema version 同步与 Owner 恢复。
- 扩展 `FlashbackAction` 或状态图时，应集中修改 `FlashbackJob::apply_action`、`cancel`、`is_flashback_action_supported` 和 `ClusterError`，并在独立的 `pkg/ddl/cluster_test.rs` 增加每个合法边与非法边测试，不把测试放入源文件。
- 修改区间算法前要维持或显式改变 Go 的前置条件：输入区间不重叠，排除项用于切组，组间空隙保证无数据。若要支持真正的区间差集，应新增长度、包含和重叠测试，并核对 `tablecodec` 编码，不能仅依赖当前简化前缀。
- 修改恢复策略时必须分别验证成功、准备期取消、回写期拒绝取消和缺失快照；TTL 的成功路径特例属于兼容行为，不能与其他变量一起无条件恢复。
- 若增加外部 I/O，应定义每阶段的幂等性、失败后的可重试点、外部设置部分修改时的补偿次序，以及 crash 后从持久化 job 恢复的规则。性能风险主要来自 Region 数量、区间切分/排序和 RPC 重试，而当前纯内存函数只有 `merge_continuous_key_ranges` 的排序开销 `O((n+m) log(n+m))`。
- 所有 Rust 行为变更应同步 `pkg/ddl/cluster_test.rs`；与线上兼容相关的语义还应对照 `pkg/ddl/cluster.go`、`pkg/ddl/cluster_test.go`、`pkg/ddl/job_worker.go` 和 `pkg/ddl/executor.go`。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 Rust/Go；`query validate_flashback_ts --kind function` 定位到 `pkg/ddl/cluster.rs:141`；`query FlashbackJob --kind struct` 定位到 `pkg/ddl/cluster.rs:96`；`node --file pkg/ddl/cluster.rs --offset 1 --limit 380` 读取了完整 324 行。`callers`/`callees` 查询未产生可用调用边，因此又以仓库搜索核对引用。
- Rust 源与装配：`pkg/ddl/cluster.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Rust 独立测试：`pkg/ddl/cluster_test.rs`，重点为 `production_merge_matches_go_exclusion_contract`、`production_rejects_cancel_after_flashback_writes_begin`、`production_finish_and_early_cancel_restore_go_settings`，以及测试内 Go 场景模型。
- Go 对照：`pkg/ddl/cluster.go` 的 `ValidateFlashbackTS`、`mergeContinuousKeyRanges`、`getFlashbackKeyRanges`、`worker.onFlashbackCluster`、`finishFlashbackCluster`；`pkg/ddl/job_worker.go` 的 action 分派；`pkg/ddl/executor.go` 的 job 创建。
- Go 测试：`pkg/ddl/cluster_test.go` 的闪回期间 DDL、全局变量和取消场景。
- 人工复核结论：本文件存在于公开 DDL crate 中，用纯内存类型保存设置快照并约束闪回阶段顺序；安全扩展的关键是不要把该模型误当成已经连接的集群执行链，并为状态、恢复和区间边界同步增加独立测试。
