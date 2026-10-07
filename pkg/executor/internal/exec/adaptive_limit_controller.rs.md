# `pkg/executor/internal/exec/adaptive_limit_controller.rs`

## 文件定位

本文件实现“有序扫描位于 `LIMIT` 下方”时的语句级自适应准入控制器，目标是在仍能向前推进的前提下，限制 Index Join 外表行和 double-read 回表 handle 的推测性并发工作量。它属于 `astersql-executor-internal-exec` crate；`pkg/executor/internal/exec/lib.rs` 以公开模块 `adaptive_limit_controller` 导出，`Cargo.toml` 通过 `[lib] path = "lib.rs"` 确认 crate 边界。实现只依赖标准库的 `Mutex`、`Condvar`、`Duration` 和 `Instant`，没有本文件专属的 Cargo feature 或外部 crate 依赖。

Rust 当前接线是不完全对称的：`pkg/executor/distsql.rs` 已使用 lookup 侧的 `ReserveLookup`、`CompleteLookup`、`AbortLookup`、`SuggestedBatchSize`、`Stop` 和 `Snapshot`，`pkg/executor/select.rs` 的 `LimitExec` 已管理 `Reset`/`Stop` 生命周期；`pkg/executor/join/index_lookup_join.rs` 会保存控制器和快照。代码搜索未发现生产 Rust 代码构造 `AdaptiveLimitController`，也未发现生产端调用 `ReserveOuter`、`CommitOuter` 或 `ObserveJoinProgress`；这些 outer 路径目前由独立 Rust 测试覆盖，不能描述为已完整接入 Rust Index Join 主链。

## 核心职责

- 用两个相互独立但会互相反馈的预算限制推测工作：outer 预算按 Index Join 外表行计数，lookup 预算按索引 handle 计数；回表返回行数和最终输出行数另行统计，避免把过滤前后的单位混为一谈（`AdmissionStage`、`ControllerState`）。
- 根据累计产出率与最近 4 个样本的局部产出率估算满足剩余 `LIMIT` 需要的输入；取更保守的较大估计，并在语句前段保留 25% 或 12.5% 余量，靠近尾部时取消余量（`AdaptiveYieldWindow`、`recompute_outer_window`、`recompute_lookup_window`、`add_adaptive_window_headroom`）。
- 在连续无输出阶段不用无意义的产出率做除法，而是在当前窗口已经耗尽、在途工作归零后按最多 2 倍增长；lookup 同时扩大逻辑窗口与执行 batch（`grow_outer_window_if_drained`、`grow_lookup_window_if_drained`）。
- 通过阻塞式准入和显式 reservation 生命周期限制并发在途量，并统计同一阶段多个等待者等待区间的并集，而不是把各线程等待时间相加（`reserve`、`AdmissionBlockStats`）。
- 在达到需求、执行器关闭或显式停止时清空预算并唤醒等待者；通过 `AdaptiveLimitSnapshot` 暴露诊断与运行时统计所需的不可变快照（`Stop`、`stop_locked`、`Snapshot`）。

## 主要符号

- `ADAPTIVE_YIELD_WINDOW_SIZE = 4`：最近产出率环形窗口的固定样本数。
- `AdaptiveLimitMode::{IndexJoin, DirectIndexLookup}`：决定是否存在 outer 阶段，以及最终输出由 join 反馈还是由 lookup 完成量直接累计。
- `AdmissionStage::{OuterRows, LookupHandles}`：选择准入窗口、在途量、等待统计和条件变量。
- `AdaptiveYieldWindow::{add, totals}`：覆盖式写入 4 槽环形数组；`totals` 用饱和加法汇总输入/输出。
- `AdmissionBlockStats::{begin, end, finish, elapsed}`：以首个 waiter 开始、最后一个 waiter 结束的方式计算阻塞时间并集；`finish` 用于停止时一次性结清。
- `AdaptiveLimitConfig`：公开不可变边界，包括最终行需求、outer/lookup 初始和最大窗口、lookup 初始和最大 batch。
- `AdaptiveLimitSnapshot`：公开诊断结构，分别暴露最终输出、outer、lookup、停止瞬间在途量、阻塞时长和停止标志。
- `ControllerState`：互斥锁保护的全部状态，包含模式、累计计数、当前/初始/最大窗口、最近产出率、无输出阶段、增长栅栏、等待统计和 `stopped`。
- `AdaptiveLimitController`：公开控制器；`state: Mutex<ControllerState>` 串行化状态转换，两个 `Condvar` 分别唤醒 outer 和 lookup reserver。
- 公开生命周期 API：`NewAdaptiveLimitController`、`NewAdaptiveLimitLookupController`、`Reset`、`ReserveOuter`、`ReserveLookup`、`CommitOuter`、`ObserveJoinProgress`、`CompleteLookup`、`AbortLookup`、`SuggestedBatchSize`、`Stop`、`Snapshot`。
- 内部算法：`recompute_*`、`grow_*_if_drained`、`lookup_physical_window`、`stop_locked`、`normalize_adaptive_window`、`grow_adaptive_window`、`adjust_adaptive_window`、`divide_and_round_up`。

## 执行流程

1. 构造函数先用 `normalize_adaptive_window` 把初始窗口抬到至少 1，并保证最大值不小于初始值；lookup batch 被限制在 `[min(initial_lookup_window, max_batch), max_batch]` 的有效区间。正数 `demand_rows` 会压低初始窗口，避免起步即超过需求；direct lookup 模式再把 outer 窗口永久置零。`demand_rows == 0` 会立即 `stop_locked`。
2. producer 调用 `ReserveOuter` 或 `ReserveLookup`。`reserve` 在互斥锁内比较窗口与在途量：outer 在途量是 `outer_fetched - outer_consumed + outer_reserved`，lookup 是 `lookup_reserved`；lookup 的可见窗口先经 `lookup_physical_window` 向完整 batch 上取整。容量不足时登记阻塞并在对应 `Condvar` 上等待，停止后返回 `(0, false)`。
3. outer reservation 由 `CommitOuter(reserved, fetched)` 结算：只释放仍存在的 reservation，且 committed fetched 不超过实际释放量。join 随后用 `ObserveJoinProgress(consumed_rows, output_rows)` 报告消费和最终输出；先到达的输出会进入 `pending_outer_output`，等有 outer 消费量时再配对为一个产出率样本。
4. lookup reservation 成功执行后由 `CompleteLookup(reserved, handles, rows)` 结算，失败或裁剪由 `AbortLookup(handles)` 释放。只有 reservation 非零、控制器未停止且 `reserved <= lookup_reserved` 时，complete 才更新累计量。在 direct lookup 模式中，`rows` 同时是最终输出，达到 `demand_rows` 会立即停止。
5. 有输出时，累计产出率与最近 4 个样本共同重新计算窗口。outer 估计满足剩余最终行需要的 outer 输入；Index Join lookup 先扣除 lookup/outer 两侧已缓冲行的较大者，再估计剩余 outer 窗口需要的 handle；direct lookup 直接估计剩余最终输出所需 handle。窗口可立即收缩，但增长必须越过相应进度栅栏，单次最多翻倍。
6. 无输出时清空陈旧的最近产出率并累计空耗输入。只有在本阶段在途量归零且空耗达到当前窗口后才翻倍，从而既避免一次空 task 直接放大，也保证低选择率区间能继续推进。
7. `Stop` 或达到需求时，`stop_locked` 记录停止瞬间的 outer/lookup 在途量，结清阻塞统计，清零 reservation、窗口和 batch；锁释放后通知两个条件变量。`Reset` 恢复初始窗口与 batch、清零累计和诊断状态，再通知所有等待者。

## 数据与状态

所有计数均为 `u64`，但单位不同：`demand_rows`/`output_rows` 是最终结果行；outer 字段是外表行；`lookup_reserved`/`lookup_handles`/lookup 窗口与 batch 是索引 handle；`lookup_rows` 是回表结果行。扩展时必须保持这些单位分离，否则 `recompute_lookup_window` 的选择率和缓冲扣减会失真。

累计量主要使用 `saturating_add`、`saturating_sub` 和 `saturating_mul` 的等价组合以防溢出或倒扣。窗口的三个层次是：配置上限、基于选择率的逻辑窗口，以及 `lookup_physical_window` 按 batch 向上取整后的物理窗口；后者最终仍不超过 `max_lookup_window`。`outer_growth_barrier` 与 `lookup_growth_progress` 保证同一进度 epoch 不能反复扩大窗口。`lookup_in_no_output_phase` 阻止零产出阶段继续使用旧的 productive yield 调窗。

`Snapshot` 在持锁状态下一次性复制状态，并把尚未结束的等待区间计算到当前 `Instant`，因此字段彼此属于同一个观测时点。停止瞬间的 outstanding 字段是专门的历史快照；停止后 reservation 已清零，不能用当前 reservation 反推停止时浪费量。

## 依赖与调用关系

- 模块边界：`pkg/executor/internal/exec/lib.rs` 声明 `pub mod adaptive_limit_controller`，并在 `#[cfg(test)]` 下加载独立的 `adaptive_limit_controller_test.rs`。
- 标准库下游：`Mutex` 保护控制状态，`Condvar::wait/notify_all` 实现阻塞与唤醒，`Instant`/`Duration` 统计等待墙钟时间；本文件没有 I/O、网络、异步 runtime 或其他 crate 调用。
- Rust lookup 主链：`pkg/executor/distsql.rs::indexWorker::fetchHandles` 调用 `ReserveLookup` 和 `SuggestedBatchSize`；任务裁剪/错误路径调用 `AbortLookup`；结果被完整消费后调用 `CompleteLookup`；关闭时调用 `Stop` 并可把 `Snapshot` 写入运行时统计。
- Rust LIMIT 生命周期：`pkg/executor/select.rs::LimitExec::open` 调用 `Reset`，达到 LIMIT、子执行器耗尽、关闭或错误路径调用 `Stop`。
- Rust 统计接线：`pkg/executor/join/index_lookup_join.rs` 和 `pkg/executor/distsql.rs` 持有 `AdaptiveLimitSnapshot`；前者关闭时采样。但精确搜索未发现生产 Rust 构造函数调用，也未发现 outer 三段生命周期的生产调用，因此当前可确认的事实是 lookup/生命周期/统计接口存在局部接线，而非完整端到端启用。
- RustCodeGraph 将目标文件列为被 `pkg/executor/distsql.rs`、`pkg/executor/select.rs`、`pkg/executor/join/index_lookup_join.rs` 及测试文件使用；对精确 Rust 节点执行 `callers/callees` 未产生可见文本，故上述调用边又以精确符号引用搜索核验。

## 错误处理与边界

- Rust API 不返回 `Result`，也不接受取消 token/context；阻塞 reservation 只能由容量变化或 `Stop` 唤醒。调用方必须保证所有正常、错误、裁剪路径最终 `CompleteLookup`/`AbortLookup`，或在生命周期结束时 `Stop`，否则可能永久占用预算。
- `Mutex` 或 `Condvar` 遇到 poison 时用 `PoisonError::into_inner` 继续使用内部状态，不把线程 panic 转换成控制器错误；这保留可用性，但调用方不会收到 poison 诊断。
- `max_units == 0` 返回 `(0, true)`；direct lookup 的 `ReserveOuter` 返回 `(0, false)`；已停止的准入返回 `(0, false)`。`SuggestedBatchSize(0)` 仍返回 1，但停止后的内部 batch 为 0，因此该函数也会返回 1；调用方不能把它当作 stopped 判据。
- `CommitOuter` 会把 `reserved` 限制到当前 reservation，并把 `fetched` 再限制到实际释放量；`AbortLookup` 同样最多扣到当前 reservation。相反，`CompleteLookup` 在 `reserved > lookup_reserved` 或已停止时直接忽略整个反馈，以防重复/越界完成污染累计产出率。
- `ObserveJoinProgress` 把 outer 消费量限制到已 fetched 数；达到需求前使用 `demand_rows - output_rows`，由前置停止判断保证不下溢。整数除法由 `divide_and_round_up` 上取整，除数为零时返回 0。
- 配置的零初始/最大窗口会被正规化为至少 1；唯一合法的零窗口来自 direct lookup 的 outer 阶段或停止状态。

## 并发与资源生命周期

控制器设计为一个 executor tree/一次语句执行共享，通常由 `Arc<AdaptiveLimitController>` 跨 worker 持有。`Mutex<ControllerState>` 是唯一状态同步边界；outer 与 lookup 使用不同 `Condvar`，结算对应阶段时只唤醒相关等待者，反馈可能同时改变两个窗口时则两边都唤醒。通知在释放互斥锁后执行，避免被唤醒线程立即再次阻塞在同一把锁上。

`AdmissionBlockStats.waiters` 只在锁内修改。第一个 waiter 记录 `blocked_since`，中间 waiter 的进入/退出不切断区间，最后一个退出才把持续时间累计到 `blocked_time`；因此并发等待只计算一次墙钟时间。`Stop` 的 `finish` 会把仍开放的区间结清并把 waiter 数归零，再由双 `notify_all` 解除阻塞。

生命周期不变量是每个 reservation 最终必须 commit/complete/abort，或被 `Stop` 清除。`Reset` 的前置条件在 Go 对照注释中明确为旧生命周期 producer 已全部退出；Rust 实现本身无法验证这一点。若旧 producer 跨越 Reset 返回，它可能把旧 reservation/反馈记入新执行，因此调用方必须在 join worker 后再重用控制器。

## 与 Go 版本的对应关系

`pkg/executor/internal/exec/adaptive_limit_controller.go` 是直接语义对照：两版都有两种模式、两个准入阶段、4 槽 yield 窗口、双预算、无输出翻倍、productive yield 调窗、batch 物理窗口、阻塞时间并集、停止快照及相同公开方法族。`pkg/executor/internal/exec/adaptive_limit_controller_test.rs` 的 9 个测试对应 Go 测试中的核心场景：当前/最近产出率、每个进度 epoch 只增长一次、延迟配对 output、零输出恢复、Stop/Reset、lookup 上限与 batch 取整、direct lookup、并发阻塞合并。

已确认的差异如下：

- Go 的 `ReserveOuter`/`ReserveLookup` 接收 `context.Context` 并返回 `(int, bool, error)`，能够在已有容量和阻塞等待两种情况下响应取消；Rust 只返回 `(usize, bool)`，没有独立取消错误路径。Go 测试覆盖了预取消与阻塞后取消，Rust 测试没有、实现也不具备这一语义。
- Go 使用 buffered notification channel 加独立 `stopCh`，`Reset` 会重建 channel；Rust 使用持久 `Condvar`，`Reset` 只重置状态并 `notify_all`。两者都要求 Reset 前旧 producer 已退出，但唤醒机制不可机械等同。
- Go 的算术帮助函数显式使用 `saturatingMultiply`/`saturatingAdd`；Rust 大多数对应位置使用 `u64::saturating_*`。两版窗口公式、headroom 分段和 2 倍增长上限一致。
- Go 生产代码的构造和 outer/lookup 两条链路已有调用证据；本次 Rust 精确搜索只确认 lookup 与生命周期的局部调用，未确认生产构造及 outer 链。因此 Go 的完整接线不能作为 Rust 已启用的证据。
- Rust 独立测试比 Go 对照测试更精简：它验证核心数值与并发不变量，但没有完整复刻 Go 的 context 取消、重复 `Stop`、更多配置硬上限与 benchmark 场景。

## 扩展指南

- 改变窗口算法时，从 `recompute_outer_window`、`recompute_lookup_window`、`recompute_direct_lookup_window` 与 `adjust_adaptive_window` 入手；必须同步评估累计 yield、最近 4 样本、增长栅栏、尾部 headroom 和低选择率零输出路径，避免只让单一测试通过。
- 改变 lookup 执行粒度时，同时检查 `lookup_window`、`lookup_batch_size` 和 `lookup_physical_window`；逻辑窗口收缩不应退化为逐 handle RPC，物理余量又必须小于一个 batch 且不突破最大窗口。
- 新增 reservation 退出路径时，确保成功对应 complete、错误/裁剪对应 abort、全局终止对应 stop；重点审计 `pkg/executor/distsql.rs` 的任务发送失败、结果错误、未消费尾部和关闭路径。
- 若补齐 Rust Index Join 端到端接线，应在 builder/执行器创建点构造同一个 `Arc`，把它传给 LIMIT、index worker 和 join，完整连接 `ReserveOuter -> CommitOuter -> ObserveJoinProgress`，并证明 lookup reservation 的每条退出路径平衡；不得仅设置结构体字段。
- 若要对齐 Go 的取消语义，现有同步签名需要显式设计取消机制及错误类型，不能把 `Stop` 当成某一个调用者的 context 取消；还需增加“有容量前已取消”和“阻塞中取消”两类独立 Rust 测试。
- 测试必须继续放在独立文件 `pkg/executor/internal/exec/adaptive_limit_controller_test.rs`，并同步参考 `adaptive_limit_controller_test.go`；不要把 `#[cfg(test)]` 测试内嵌进生产文件。
- 性能风险集中在热路径每次反馈的全局互斥、`notify_all` 惊群和过大的推测窗口；正确性风险集中在单位混用、reservation 泄漏、重复完成、Reset 与旧 worker 竞态以及整数边界。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的文件清单包含本文件、Go 对照及两份独立测试。
- RustCodeGraph `node --file`：完整读取 `pkg/executor/internal/exec/adaptive_limit_controller.rs` 1–724 行、`pkg/executor/internal/exec/lib.rs` 1–29 行、`pkg/executor/internal/exec/adaptive_limit_controller_test.rs` 1–268 行、Go 对照实现 1–882 行及 Go 测试 1–613 行。
- RustCodeGraph `query --json`：确认 Rust/Go 的 `AdaptiveLimitController`、`ReserveOuter`、`ObserveJoinProgress`、`CompleteLookup`、`SuggestedBatchSize` 精确节点，以及 9 个 Go `TestAdaptiveLimitController*` 测试入口。对 Rust 节点执行 `callers/callees` 未返回可见调用记录，随后以精确引用搜索补证。
- 配置与模块：读取 `pkg/executor/internal/exec/Cargo.toml` 和 `lib.rs`，确认 crate 名、库入口、模块导出与独立测试装配；目标目录不存在 `doc.go`。
- 直接调用证据：核对 `pkg/executor/distsql.rs`、`pkg/executor/select.rs`、`pkg/executor/join/index_lookup_join.rs` 中的控制器字段和方法调用，并在 Rust 生产文件范围搜索所有公开 API 引用；未发现生产 Rust 构造和 outer 三段调用，故文档明确标为未完整接线。
- 测试证据：Rust 测试覆盖 productive/zero-output yield、增长 epoch、pending output、Stop/Reset、batch 取整、direct lookup 和重叠等待区间；Go 测试额外证明 context 取消及更多边界，但这些不能外推为 Rust 已支持。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构检查，并人工复核本文只陈述上述源码、调用引用、Cargo 和测试能够支持的事实。
