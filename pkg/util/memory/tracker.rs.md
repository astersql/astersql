# `pkg/util/memory/tracker.rs`

源码：[tracker.rs](./tracker.rs)；Go 对照：[tracker.go](./tracker.go)；独立 Rust 测试：[tracker_test.rs](./tracker_test.rs)、[tracker_4_aster_unit_test.rs](./tracker_4_aster_unit_test.rs)。

## 文件定位

本文件属于 `astersql-util-memory` crate。crate 根 `pkg/util/memory/lib.rs` 以 `pub mod tracker` 暴露它；`pkg/util/memory/Cargo.toml` 声明默认无 feature，并用可选的 `mem-arbitrator` feature 编译全局内存仲裁路径。

它是查询执行期内存记账的基础实现：`Tracker` 把执行器、语句和 session 的局部用量组织成父子树，向祖先累计消费，维护软/硬配额和峰值，并在超限时执行动作链。上层真实入口包括 `pkg/session/runtime/dispatch.rs` 创建 session/statement tracker，`pkg/session/runtime/planning.rs` 为 SQL 文本创建 tracker，`pkg/sessionctx/stmtctx/stmtctx.rs` 初始化 statement context 的 `MemTracker`；执行期消费者如 `pkg/statistics/runtime_stats_builder.rs`、`pkg/util/chunk/list.rs` 和 `pkg/util/chunk/row_container.rs` 调用 `Consume` 更新用量。

## 核心职责

1. `NewTracker`、`InitTracker` 和 `NewGlobalTracker` 构造或重置追踪器；硬限为非正数时表示无限制，软限由 `limits_for` 固定为硬限的 `0.8`。
2. `AttachTo`、`Detach`、`ReplaceChild` 和全局 tracker 的专用挂接 API 维护树关系，并把子节点已有消费量同步到父链。
3. `Consume` 沿父链原子加减用量、更新峰值、识别最后遇到的软/硬超限节点、检查 session kill 信号并触发对应动作链。
4. `ActionOnExceed`、`reArrangeFallback` 和动作锁维护按优先级排列的 fallback 链；已完成动作会在触发前被跳过。
5. `BufferedConsume`/`BufferedRelease` 以 `TrackMemWhenExceeds`（100 MiB）为阈值批量记账；`Release` 提供 GC-aware 释放接口。
6. feature `mem-arbitrator` 下，tracker 还管理 session 的 small/big budget、root pool、等待配额统计、反向修正量和退出清理。
7. 查询、格式化和观测 API 提供当前值、峰值、子树扫描、稳定文本输出、字节单位格式化以及进程级内存限制全局量。

## 主要符号

- `Tracker`：核心状态对象。`parent: AtomicPtr<Tracker>` 保存父指针；`bytesConsumed`、`bytesReleased` 和 `maxConsumed` 保存当前、GC-aware 暂存释放量和峰值；`bytesLimit` 一次性保护 hard/soft 配额对；`mu.children` 按 label 保存普通子节点；两个 `actionMu` 分别保护硬限和软限动作链。
- `ActionOnExceed` / `LogOnExceed`：超限动作协议及默认实现。协议要求 `Send`，提供动作、fallback 所有权转移、优先级和完成状态；`LogOnExceed` 第一次只标记 acted，之后转入 fallback。
- `InitTracker`、`NewTracker`、`NewGlobalTracker`：公开构造入口。普通 tracker 默认安装 `LogOnExceed`；global tracker 设置 `isGlobal`，其设计是不维护 children 以减小锁竞争。
- `Tracker::Consume`：主记账入口，`bs > 0` 为申请，负数为释放；零值直接返回。
- `Tracker::{AttachTo, Detach, ReplaceChild}`：普通树结构变更；`AttachToGlobalTracker` 和 `DetachFromGlobalTracker` 是不登记 children 的 global 专用版本。
- `Tracker::{Release, BufferedConsume, BufferedRelease}`：释放和阈值缓冲入口。缓冲值由调用者持有，所以注释明确这些 API 不保证线程安全。
- `Tracker::{BytesConsumed, BytesReleased, MaxConsumed, ResetMaxConsumed}`：观测与 statement 周期重置接口；`ResetMaxConsumed` 重置为当前用量而非零。
- `Tracker::{SearchTrackerWithoutLock, SearchTrackerConsumedMoreThanNBytes, CountAllChildrenMemUse, String}`：树查询和诊断输出。`String` 对 label 排序后递归输出，使结果稳定。
- `BytesToString` / `FormatBytes`：按 Bytes/KB/MB/GB 输出；`FormatBytes` 根据是否整除和数值范围选 0、1 或 2 位小数。
- `LabelFor*`：从 `LabelForSQLText` 到 `LabelForHashTableInHashJoinV2` 的负数标签集合，用于区分内存归属；`MetricsTypes` 当前只映射全局 Analyze 内存。
- `atomicutil::{Int64, Uint64, Bool, String, Time}`：本文件用于迁移 Go atomic API 形状的包装；数值原子使用 `SeqCst`，`Time` 使用 `Mutex<SystemTime>`。
- `ServerMemoryLimit*`、`QueryForceDisk`、`TriggerMemoryLimitGC`、`MemoryLimitGC*`、`MemUsageTop1Tracker`：进程级限制、GC 和 Top1 session 观测状态。
- `memArbitrator`、`TrackerArbitrateHelper`、`ReversalRes`（仅 `mem-arbitrator`）：仲裁状态机、向仲裁核心汇报的 helper，以及用显式 `Release` 或 `Drop` 自动撤销的 reversal 守卫。

## 执行流程

普通调用链如下：上层用 `NewTracker(label, limit)` 创建节点，按需要设置 `IsRootTrackerOfSess`、`SessionID`、`Killer` 和超限动作，再用 `AttachTo(parent)` 建树。挂接时先从旧父移除，加入新父 `children[label]`，发布父指针，并把子节点当前 `BytesConsumed` 补记到新父链；`Detach` 做相反操作。

每次 `Consume(bs)` 从当前节点沿 `parent` 向根遍历。对每个节点，它先在启用仲裁时更新 small/big budget，再原子增加 `bytesConsumed`；随后用同一份 `bytesLimits` 快照判断 `bytesConsumed + bytesReleased` 是否达到软/硬限，并用 CAS 提高 `maxConsumed`。遍历同时记录 session root。遍历结束后，正向消费依次处理 Top1 session 候选、session killer、硬限动作和软限动作；负向消费只记账，不触发 kill 或动作。

动作触发由 `tryAction` 在对应 `actionMu` 锁内完成：先反复移除 `IsFinished` 的链头，再调用当前动作。`FallbackOldAndSetNewAction*` 通过 `reArrangeFallback` 递归合并新旧链，保证优先级较高者靠前。

树结构变更以差额守恒：`remove` 找到目标子节点后清父指针并向父链 `Consume(-child.BytesConsumed())`；相同 label 的 `ReplaceChild` 原位替换指针并只消费新旧差额，不同 label 则退化为 remove + attach。global tracker 不保存子指针，只在挂接和摘除时增加或扣减当前消费量。

`Release(bytes)` 未启用 GC-aware 路径时等价于 `Consume(-bytes)`。启用且祖先链遇到 `LabelForGlobalAnalyzeMemory` 时，当前 Rust 实现先 `recordRelease`，再扣减消费，并立刻调用 `release` 清除 released 暂存值；这与 Go finalizer 异步清除 released 值不同。

`mem-arbitrator` 下，`InitMemArbitratorWithSharedKiller` 构造 helper/context 和 small budget；显式预留或历史峰值超过 small limit 时直接尝试进入 big budget。`Consume` 超过 small limit 时调用 `intoBigBudget` 建 root pool、申请初始配额并迁移用量；big budget 用量越过阈值后 `growBigBudget` 申请扩容。`DetachMemArbitrator` 令状态进入 down、清 small budget、停止 big budget、归还 root pool，并在非异常退出时更新 digest profile cache。

## 数据与状态

`Tracker` 的核心不变量是：已挂接普通子节点的消费量包含在每个祖先的 `bytesConsumed` 中；挂接、摘除或替换必须按已有消费量补记或扣回。`bytesHardLimit <= 0` 与 `bytesSoftLimit <= 0` 均表示无限制；正配额的 soft limit 是 `floor(hard * 0.8)`。峰值只增不减，直到 `ResetMaxConsumed` 把它设置为当前消费量。

普通父节点的 `children` 是 `HashMap<label, Vec<*mut Tracker>>`，允许同 label 多个节点；global tracker 刻意不登记 children。`SetLabel` 通过 detach/reattach 保证 map key 和节点 label 一致。树查询返回裸指针，所有权仍在调用方。

`bytesReleased` 只服务 GC-aware Analyze 统计，超限判断使用 `bytesConsumed + bytesReleased`，避免对象虽从逻辑用量移除但尚未实际回收时过早释放配额。当前 Rust `Release` 同步完成该暂存过程，因此方法返回后 released 值回到零；`tracker_test.rs::TestRelease` 明确断言了这一迁移差异。

仲裁状态为 small、into-big、big、down 四态。`small_used` 对应 await-free pool；`big_used`、`big_budget` 和 `root` 对应共享 root pool；`reversal` 从 `RootPoolUsed` 中扣除临时反向修正。`ReversalRes` 保证显式释放和析构只执行一次撤销。

## 依赖与调用关系

crate 内下游依赖包括：`crate::global_arbitrator::UsingGlobalMemArbitration` 决定是否维护旧式 Top1 session；`crate::sqlkiller::SQLKiller` 处理查询 kill；feature 开启时调用 `arbitrator.rs` 的 `MemArbitrator`、`ConcurrentBudget`、`ArbitrationContext`、`RootPoolHandle` 等，以及 `global_arbitrator.rs::GlobalMemArbitrator`。标准库依赖主要是 `Atomic*`、`Mutex`、`Arc`、`HashMap`、裸指针和时间类型。

已核实的上游 Rust 调用边包括：

- `pkg/session/runtime/dispatch.rs`：创建 session tracker 和带查询配额的 statement tracker，并对 statement 内存收费。
- `pkg/session/runtime/session.rs`：创建 session 级 tracker；`pkg/session/runtime/control.rs` 对 session tracker 消费并读写 server memory limit。
- `pkg/session/runtime/planning.rs`：以 `LabelForSQLText` 创建 SQL 文本 tracker，并按 chunk 内存与已记账值的差额调用 `Consume`。
- `pkg/sessionctx/stmtctx/stmtctx.rs`：`InitMemTracker` 保存 `NewTracker` 的结果，形成 statement context 的公共入口。
- `pkg/statistics/runtime_stats_builder.rs`：把运行时统计差额记入 tracker。
- `pkg/util/chunk/list.rs`、`row_container.rs`：为 chunk/list/row container 创建 tracker，并在追加、删除、落盘转换时消费或归还内存。
- `pkg/util/memoryusagealarm/memoryusagealarm.rs`：读取进程内存限制，并使用本文件的 `FormatBytes` 展示峰值。

RustCodeGraph 能索引目标文件和函数节点，但本次对 impl 方法执行 `callers/callees` 未返回 Rust 调用边，因此上述跨文件边由精确 Rust 源码搜索复核；这不影响目标文件内部流程证据。

## 错误处理与边界

大多数 API 不返回 `Result`：内存超限通过动作链或 session killer 表达；`Consume`/`HandleKillSignal` 收到 killer 错误时直接 `panic!`。`AttachToGlobalTracker` 目标不是 global tracker、或 `DetachFromGlobalTracker` 的父不是 global tracker时也会 panic，相关边界由 `TestGlobalTracker` 覆盖。

锁获取统一使用 `unwrap()`，锁中毒会 panic。仲裁配额申请失败通常返回 `false` 或保留原状态：例如 `intoBigBudget` 无法创建/重启 root 或申请初始配额时回退到 small 状态；`growBigBudget` 申请失败不扩容，但仍清理等待统计。`InitMemArbitratorWithSharedKiller(None, ...)` 返回 `false`。

调用者必须遵守裸指针生命周期：父节点和已登记子节点在 detach/replace 之前必须保持有效。`unsafe impl Send/Sync` 依赖这些外部不变量；文件只为 `bytesConsumed`、parent 和 children 等指定路径提供同步，不承诺任意树操作并发安全。`BufferedConsume`、`BufferedRelease` 的外部缓冲变量尤其要求单线程使用。

字节格式化以严格“大于单位”选择单位，所以恰好 1024 字节先由 `FormatBytes` 的分支正确转为 `1 KB`，而 `BytesToString` 单独调用时其边界行为不同。`MaxConsumed` 不得复用 `-1` 表示特殊值，因为二进制计划已用 `-1` 区分 0 与 N/A。

## 并发与资源生命周期

`bytesConsumed`、`bytesReleased`、parent 和多数兼容原子包装使用 `SeqCst`；仲裁热路径按用途使用 Acquire/Release/AcqRel。`maxConsumed` 以 CAS 循环更新。children map、hard/soft 动作链、配额对、仲裁状态转换和 big budget 扩容分别有独立锁，避免把每次记账串行化在一个全局锁上。

线程安全边界沿用源码注释：`BytesConsumed`、`Consume` 和 `AttachTo` 是明确支持并发的主路径，其他树操作不能据此推导为任意并发安全。测试 `TestConsume` 与 `concurrent_consume_matches_go_atomic_accounting` 通过多线程正负消费验证净额与峰值区间；feature 测试还覆盖 consume 与 detach 竞争、状态转换期间 detach 等路径。

普通 tracker 的生命周期由上层 `Box`/`Arc` 持有，但树内部只保存裸指针，文件没有 `Drop for Tracker` 自动 detach，因此所有者应在销毁前完成结构清理。`Reset` 会 detach、清零当前用量并清 children，但保留 label 和 limit。global tracker 不持有子节点，避免形成所有权关系。

仲裁资源由 `DetachMemArbitrator`/`memArbitrator::reset` 显式关闭；down 状态阻止迟到的 grow 重新打开预算。`ReversalRes` 是 RAII 例外：离开作用域自动撤销 reversal。`Done` 返回与 killer 事件相连的取消接收器，`Finish` 只标记 helper 完成。

## 与 Go 版本的对应关系

结构和命名有意贴近 `pkg/util/memory/tracker.go`：Go 的 `Tracker`、`InitTracker`、`NewTracker`、树操作、`Consume`、动作链、格式化、label、进程级全局量和 `memArbitrator` 在 Rust 中均有直接对应。`tracker_test.rs` 也按 Go 测试名称覆盖 SetLabel、Consume/Release、OOM action、Attach/Detach/Replace、ToString、峰值、global tracker 和 FormatBytes；`tracker_4_aster_unit_test.rs` 追加树统计、并发记账与仲裁清理回归。

需要特别注意的迁移差异：

- Go 依赖 `runtime.SetFinalizer` 延后清除 `bytesReleased`；Rust 没有等价运行时 finalizer，本实现于 `Release` 所代表的所有权边界同步调用 `release`。
- Go 的 atomic pointer/atomic limit 快照在 Rust 中分别落为 `AtomicPtr` 与 `Mutex<bytesLimits>`；语义上保证 hard/soft 配额成对发布，但实现机制不同。
- Go 的 tracker/children 是受 GC 管理的指针；Rust 使用 `Box`/`Arc` 持有对象、树内保存裸指针，因此生命周期义务更显式。
- 本文件内 `metrics::Gauge` 的 `Set` 当前为空实现，只保留调用形状；不能据此声称 Rust 已真实导出对应内存 gauge。
- 源码注释指出 Go 通过字段顺序追求 64 字节对齐，Rust 当前只保留原子访问意图，没有显式 cache-line 对齐布局。
- `MetricsTypes` 当前仅包含 `LabelForGlobalAnalyzeMemory`，应按实际表项描述，不能假定所有 label 都会输出 metrics。

## 扩展指南

新增内存归属时，在 label 常量区增加稳定且不冲突的负值，并在真实使用点通过 `NewTracker`/`AttachTo` 接入；如需 metrics，再同步扩展 `MetricsTypes` 及真实 metrics 后端。测试应放在独立文件 `tracker_test.rs` 或 `tracker_4_aster_unit_test.rs`，不要嵌入生产源文件，并覆盖树传播、detach 后回滚和稳定输出。

新增超限策略应实现 `ActionOnExceed`，明确优先级、一次性/重复触发语义和 fallback 所有权；通过 `FallbackOldAndSetNewAction` 或 soft-limit 版本安装。必须测试相同/不同优先级顺序、finished 链头跳过、链中解绑，以及 hard/soft 同时达到时的行为。

改变树结构 API 时，应维护“祖先消费等于挂接子树贡献”的差额不变量，并同步审查所有裸指针生命周期。任何并发扩展都应说明由哪个原子或锁保护，重点覆盖 attach/detach/consume 竞争；不能仅凭 `Send/Sync` 假设所有方法安全。

修改 `Release` 时必须同时对照 Go finalizer 语义和 Rust 所有权边界，验证 `bytesConsumed + bytesReleased` 的配额判断不会漏算实际存活内存。若引入真正异步回收通知，需要独立测试等待与竞态，而不是把测试逻辑放入 `tracker.rs`。

扩展 `mem-arbitrator` 时应从 `InitMemArbitratorWithSharedKiller`、`intoBigBudget`、`growBigBudget` 和 `reset` 四个状态边界接入，并验证申请失败、已有 kill、detach 竞争、digest cache 更新和所有配额归还。feature 相关测试必须带 `#[cfg(feature = "mem-arbitrator")]`，同时保留默认 feature 下的基础 tracker 行为。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/memory` 显示 `tracker.rs` 有 209 个符号。
- RustCodeGraph `node --file pkg/util/memory/tracker.rs`：分段读取完整 1,829 行，核对构造、动作链、树操作、Consume/Release、格式化、标签和 feature 条件代码。
- RustCodeGraph `query`：确认 `NewTracker`、`NewGlobalTracker`、`InitTracker`、`reArrangeFallback`、`BytesToString`、`FormatBytes`、`MetricsTypes`、`InitMemArbitratorWithSharedKiller` 的真实节点与签名；`callers/callees` 对目标 impl 方法未产出可用 Rust 边，随后用精确 `rg` 补证上游调用点。
- crate 与模块证据：`pkg/util/memory/Cargo.toml`、`pkg/util/memory/lib.rs`；无 `doc.go` 可读。
- Go 对照证据：`pkg/util/memory/tracker.go`、`pkg/util/memory/tracker_test.go`，重点核对树累计、软硬限、GC-aware finalizer、global tracker 和仲裁状态。
- Rust 测试证据：`pkg/util/memory/tracker_test.rs`、`pkg/util/memory/tracker_4_aster_unit_test.rs`；覆盖并发消费、动作链、结构变更、格式化及 feature 仲裁生命周期。本任务按计划是纯文档分析，未运行 Cargo 或代码测试。
