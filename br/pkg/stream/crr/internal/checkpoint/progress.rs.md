# `br/pkg/stream/crr/internal/checkpoint/progress.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-stream-crr-internal-checkpoint`，包入口 `br/pkg/stream/crr/internal/checkpoint/lib.rs` 以 `progress` 模块挂载它。它不是独立的服务入口，而是为 `calculator.rs` 中的 `Calculator::ComputeNextCheckpoint` 提供一轮 CRR（跨区域复制）安全检查点计算所需的内部步骤。

完整生产调用链是 `br/pkg/stream/crr/service/service.rs` 的 `Service::run_once` 持有计算器锁并调用 `Calculator::ComputeNextCheckpoint`；后者依次调用本文件的 `poll_upstream_checkpoint`、`load_alive_stores`、`plan_round`、`observe_round_planned`、`wait_object_sync`、`advance_synced_state` 和 `observe_checkpoint_advanced`。发生错误时，`ComputeNextCheckpoint` 再调用 `observe_calculation_failed`。因此本文件处于 CRR 服务循环和上游元数据/下游对象同步检查之间。

`Cargo.toml` 将该目录声明为 library crate，并通过路径依赖连接 `stream-backupmetas` 与 `streamhelper`；本文件直接使用的 `Calculator`、事件、上下文和存储抽象则来自同 crate 的 `calculator.rs`、`storage.rs`。

## 核心职责

本文件把“上游 checkpoint 已推进”转换为“下游可以安全使用的 checkpoint”，主要负责五件事：

1. 从 `PDMetaReader` 获取任务的全局 checkpoint，并区分“推进”与“未推进”。
2. 获取 PD 当前存活 store，扫描尚未被各 store 已同步水位覆盖的 meta 文件，以受限并发读取、解析并形成一轮计划。
3. 对计划中的 meta 文件和 data/log 文件路径去重，持续通过 `ObjectSyncChecker::FileSynced` 等待全部对象同步。
4. 在确认本轮所有对象均同步后，更新 `synced_by_store`，剪除离线 store，并在所有存活 store 都已有观测水位时单调推进全局 `synced_ts`。
5. 通过 `CheckpointEvent` 发布等待上游、上游推进、轮次规划、等待下游、检查点推进和计算失败等生命周期事件及文件统计。

安全边界来自 `doc.go` 的包级契约：不能仅凭 `flush_ts <= upstream checkpoint` 判断可恢复性，因为某个 flush 批次可能仍包含恢复到更小 checkpoint 所需的数据。本文件必须扫描并等待该轮发现的所有对象，再允许主循环提交 `last_checkpoint`。

## 主要符号

- `roundPlan`：单轮内部计划，包含去重后的 `pending_paths`、每个 store 本轮最大的 `max_flush_ts_by_store` 和 `FileStatistic`。类型及字段均为 crate 内可见，不构成外部 API。
- `new_round_plan()`：创建空计划，并显式初始化 `PlannedFileSuffixCounts`，保持与 Go 空 map 的可写语义一致。
- `roundPlan::record_loaded_meta()`：把一份 `loadedMetaFile` 合入计划。非 empty meta 自身进入待同步集合；其 data 文件也进入集合；同一路径只计一次；store 水位取最大 `flush_ts`。
- `roundPlan::record_pending_path()`：完成路径去重，并通过 `path_suffix` 更新计划阶段的后缀分桶。
- `ConcurrencyLimiter` / `ConcurrencyGuard`：基于 `Mutex<usize> + Condvar` 的同步信号量；guard 的 `Drop` 归还许可，限制 `plan_round` 中正在执行的 meta 读取数。
- `Calculator::poll_upstream_checkpoint()`：调用 `PD.GetGlobalCheckpointForTask`。只有返回值严格大于 `state.last_checkpoint` 才报告 `EventUpstreamAdvanced`；否则报告 `EventWaitingUpstream` 并令主循环返回旧 checkpoint。
- `Calculator::load_alive_stores()`：调用 `PD.Stores`，将非零 store ID 转成集合；ID 0 被视为占位值并过滤。
- `Calculator::plan_round()`：通过 `new_meta_file_iter` 枚举候选 meta，跳过 `flush_ts <= synced_by_store[store]` 的旧 meta，再以 `MetaReadConcurrency` 为上限调用 `storage::load_meta_file`。迭代或读取失败会取消本轮。
- `Calculator::wait_object_sync()`：反复检查所有 `pending_paths`；已同步路径立即移除，未清空时报告 `EventWaitingDownstream` 并按 `PollInterval` 可取消睡眠。
- `Calculator::advance_synced_state()`：合并各 store 最大 flush 水位、剪除离线 store、检查存活 store 是否缺水位，并单调更新全局 `synced_ts`。
- `Calculator::check_missing_store()`：只要任一 alive store 不在 `synced_by_store` 中就返回 `false`，从而阻止全局水位前进。
- `observe_round_planned()`、`observe_checkpoint_advanced()`、`observe_calculation_failed()`：构造对应事件；统计使用快照，避免后续等待阶段的修改污染已发送事件。
- `sleep_with_context()`：以 10ms 粒度睡眠并检查 `Context`，使长轮询间隔仍可响应取消。
- `path_suffix()`：从 basename 提取扩展名；无扩展名归入 `<none>`，包含点号后长度超过 5 的扩展名归入 `<other>`，控制指标标签基数。

## 执行流程

`Calculator::ComputeNextCheckpoint` 对本文件的编排顺序如下：

1. `poll_upstream_checkpoint` 读取上游全局 checkpoint。若没有严格推进，立即返回 `last_checkpoint`，服务层随后通过 watcher 等待，避免计算器忙轮询。
2. `load_alive_stores` 获取当前拓扑，仅把非零 ID 纳入安全检查。
3. `plan_round` 调用 `new_meta_file_iter` 扫描 meta。对已有 per-store 水位覆盖的 meta 增加 `SkippedStoreSyncedMetaFileCount` 后跳过；其余 meta 进入待加载列表。
4. `plan_round` 为各读取任务取得许可并在 scoped thread 中调用 `load_meta_file`。未被 storage 层忽略的结果通过互斥锁合并进 `roundPlan`。任何首个加载错误写入共享错误槽并取消本轮上下文，使兄弟读取尽快退出；scope 结束保证所有线程已回收。
5. `observe_round_planned` 发布计划快照，其中包含 alive store 数、待同步文件数和读取/跳过/后缀统计。
6. `wait_object_sync` 对去重后的 meta 与 data/log 路径逐轮调用 `FileSynced`。每次检查都会更新计数与后缀统计；只有集合完全清空才成功返回。尚未清空时发布等待事件并休眠。
7. `advance_synced_state` 先保留本轮每个 store 的最大 flush 水位，再剪除非 alive store。若 alive store 全都有水位，就以“剪枝前水位集合”的最小值作为候选，并只在候选更大时更新 `synced_ts`。
8. `ComputeNextCheckpoint` 随后提交 `last_checkpoint = upstream_checkpoint`，并由 `observe_checkpoint_advanced` 发布新的水位及统计。任一步失败都会保留错误结果并发布 `EventCalculationFailed`，不会提交 `last_checkpoint`。

第 7 步特意用剪枝前的快照求本轮最小值：已经离线的 store 在本轮发现的对象仍必须先同步，而且它的旧水位仍可约束当前成功轮；完成本轮后，它才不再约束未来轮次。

## 数据与状态

`roundPlan` 是单次调用的临时状态。`pending_paths` 同时容纳非 empty meta 自身和 meta 引用的数据文件，`HashMap<String, ()>` 模拟 Go 的集合；重复引用不会重复增加 `EstimatedSyncLogFileCount` 或后缀统计。empty meta 不要求读取/等待 meta 内容，但其 `store_id` 与 `flush_ts` 仍会贡献 per-store 进度。

持久状态位于 `Calculator::state`：

- `last_checkpoint` 表示最近一次成功返回给服务层的上游 checkpoint；上游未推进或本轮失败时不改变。
- `synced_by_store` 保存已确认对象同步后的每 store 最大 flush 水位。更新只取最大值，保证单调。
- `synced_ts` 是全局复制完成水位，只能单调增加。它与 `last_checkpoint` 含义不同：新增 alive store 尚无任何 flush 观测时，`last_checkpoint` 仍可前进，但 `synced_ts` 必须停留。

`FileStatistic` 在规划和等待期间累积：读取 meta 数、按 store 水位跳过的 meta 数、估算的日志文件数、下游检查次数，以及计划/检查路径的后缀分布。事件发送前用 `snapshot()` 深拷贝内部 map，事件观察者不会看到后续原地修改。

## 依赖与调用关系

上游调用者：

- `br/pkg/stream/crr/service/service.rs::Service::run_once` 是生产入口，调用 `Calculator::ComputeNextCheckpoint`，成功后排队保存 `PersistentState`；若返回值等于旧 checkpoint，再调用 watcher 等待上游推进。
- `br/pkg/stream/crr/internal/checkpoint/calculator.rs::Calculator::ComputeNextCheckpoint` 是本文件所有阶段函数的直接编排者，并统一处理失败事件。

下游依赖：

- `CalculatorDeps::PD`：`GetGlobalCheckpointForTask` 提供任务 checkpoint，`Stores` 提供 alive store 集合。
- `Calculator::new_meta_file_iter`：根据已有同步水位构造上游 meta 枚举；实现位于 `storage.rs`。
- `storage::load_meta_file`：读取并解析单个 meta，返回路径、store、flush 水位、数据文件列表以及是否忽略。
- `CalculatorDeps::Sync::FileSynced`：只判断对象是否已同步，不读取下游对象内容；具体实现可用下游存在性或等价的复制元数据证明安全。
- `Calculator::observe`：把本文件构造的 `CheckpointEvent` 交给 observer；服务层的 `StatusObserver` 将其转为状态和指标。
- 标准库 `thread::scope`、`Mutex`、`Condvar`：实现不依赖异步 runtime 的有界并发和取消轮询。

Cargo 边界显示该 crate 还依赖 `astersql-br-pkg-stream-backupmetas`、`astersql-br-pkg-streamhelper`、`serde` 和 `serde_json`；它们主要由相邻的 calculator/storage 数据模型使用，本文件没有绕过 crate 内抽象直接访问外部存储 SDK。

## 错误处理与边界

- PD checkpoint 查询和 store 查询会增加操作上下文后返回 `Error`；上游 checkpoint 相等或回退不是错误，只产生等待事件。
- meta 枚举失败立即取消本轮并返回迭代错误。并发读取采用“首错胜出”：首个错误写入错误槽并触发 round-local cancellation，后续错误不覆盖它。
- `MetaReadConcurrency` 在本文件中以 `max(1)` 钳制，避免零许可死锁；正常构造时 `NewCalculator` 也会把非正配置替换为默认值。
- `Mutex`/`Condvar` 的 poisoned lock 使用 `unwrap()`，意味着工作线程 panic 会进一步 panic，而不是转换为业务 `Error`；当前契约假定内部同步原语不被 panic 污染。
- `FileSynced` 的任一错误会附带具体路径并立即终止整轮；未同步不是错误，而是继续等待。上下文取消或超时由 `sleep_with_context` 返回。
- `pending_paths` 为空时等待函数立即成功；没有任何 store 水位时 `advance_synced_state` 不更新 `synced_ts`。
- alive store ID 0 被过滤；任一真实 alive store 缺少水位都会阻止全局水位提升，但不阻止该轮 upstream checkpoint 成功返回。
- `path_suffix` 只按 `/` 分割 basename，适用于对象存储风格路径；超过 5 字节的后缀统一归桶，避免指标高基数。

## 并发与资源生命周期

`plan_round` 先顺序枚举 meta，再为待加载项创建 scoped thread。`ConcurrencyLimiter::acquire` 在 spawn 前取得许可，guard 移入线程并持有到读取结束；其 `Drop` 增加可用许可并 `notify_one`。因此同时执行的读取不超过 `MetaReadConcurrency`，且 `thread::scope` 返回前所有借用 `Upstream`、计划和错误槽的线程都已结束，不会产生脱离计算轮次的后台任务。

`Context::WithCancel` 创建本轮专用取消域。首个读取失败会取消兄弟任务；`progress_test.rs::plan_round_cancels_sibling_meta_reads_after_first_failure` 用一个立即失败读取和一个阻塞读取证明兄弟任务能观察到该取消。

共享 `roundPlan` 和首错槽分别由 `Mutex` 保护。下游同步检查刻意单线程进行：每轮克隆 key 列表以便安全删除原集合，避免要求 `ObjectSyncChecker` 承担额外并发契约。轮询间的睡眠不持有计划锁，且每 10ms 检查取消。服务层在 `run_once` 中以 calculator mutex 串行调用计算器，因此 `Calculator::state` 不需要内部并发保护。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `progress.go`：`roundPlan`、规划/等待/推进函数、统计快照、事件构造和 `pathSuffix` 的主语义一致。关键一致点包括路径去重、每 store 取最大 flushTS、alive store 缺观测时阻塞 `syncedTS`、剪枝前水位约束当前轮、下游全部同步后才推进，以及后缀 `<none>`/`<other>` 分桶。

实现机制存在几处明确差异：

- Go 用 `errgroup.WithContext`、`SetLimit` 和 goroutine；Rust 用 scoped OS threads、`ConcurrencyLimiter` 与共享首错槽实现等价的有界并发和首错取消。
- Go 用 `time.Timer + select` 等待取消；Rust 用 10ms 分段 `thread::sleep` 轮询 `Context`，取消响应有最多约一个 step 的调度延迟。
- Go 在 `planRound` 的读取前后有 `before-read-meta`、`flush-meta` failpoint；Rust 本文件未提供这些 failpoint。Rust 的直接回归测试改用可注入 storage 与 context 验证取消和交错。
- Go 的 `checkMissingStore` 会输出包含缺失 store ID 的 warning；Rust 当前只排序后丢弃该列表并返回 `false`，尚未输出对应日志。安全推进语义保持一致，但可观测性较弱。
- Go 的错误支持 `%w`/`errors.Is` 链；Rust 使用 crate 自有 `Error` 和格式化消息，保留操作与路径上下文，但并非 Go 错误链模型。

同目录 Rust 测试没有内嵌在生产文件中：`progress_test.rs` 专测并发首错取消；`checkpoint_calculator_test.rs` 与 `integration_test.rs` 覆盖推进安全性；`parity_test.rs` 核对 Go/Rust 公开常量和后缀规则。

## 扩展指南

- 新增轮次过滤条件时，应接入 `Calculator::plan_round` 的枚举阶段，并确保被跳过项有独立统计；不得仅按全局 checkpoint 或全局 `synced_ts` 过滤，因为 meta 的 `flush_ts` 只保证在单个 store 内单调。
- 新增 meta 引用对象类型时，应在 `roundPlan::record_loaded_meta` 或 `storage::load_meta_file` 的输出模型中接入，并统一经 `record_pending_path` 去重与统计，不能绕过 `wait_object_sync`。
- 修改水位聚合时必须保持三项不变量：per-store 水位只增不减；alive store 缺观测会阻止全局推进；removed store 的本轮文件先确认同步、其剪枝前水位约束当前轮，随后才从未来轮移除。
- 增加并发度或并行化下游检查前，应重新定义 `UpstreamStorageReader`/`ObjectSyncChecker` 的线程安全要求，并保留首错取消、线程 join 和确定的资源回收。不要让 guard 在 spawn 后立即于父线程释放。
- 新增事件或统计字段时，应在 `observe_round_planned`、`observe_waiting_downstream`、`observe_checkpoint_advanced`、`observe_calculation_failed` 中选择正确生命周期，并在 `FileStatistic::snapshot` 深拷贝可变容器。
- 修改后缀分桶时同步更新 `path_suffix`、Go `pathSuffix` 及 `parity_test.rs::go_rust_public_contract_matches`，并评估指标标签基数兼容性。
- 回归测试应继续放在独立文件：并发/取消优先扩展 `progress_test.rs`；store 水位、剪枝、错误传播扩展 `checkpoint_calculator_test.rs`；完整复制等待和事件序列扩展 `integration_test.rs`；Go/Rust 契约差异扩展 `parity_test.rs` 及对应 Go 测试。

## 验证依据

本说明基于以下直接证据：

- 生产实现：`br/pkg/stream/crr/internal/checkpoint/progress.rs` 的 `roundPlan`、`ConcurrencyLimiter`、`Calculator` 方法、事件辅助函数、`sleep_with_context` 和 `path_suffix`。
- 主循环与状态提交：`br/pkg/stream/crr/internal/checkpoint/calculator.rs::Calculator::ComputeNextCheckpoint`。
- 生产服务入口：`br/pkg/stream/crr/service/service.rs::Service::run_once`。
- 包级安全不变量：`br/pkg/stream/crr/internal/checkpoint/doc.go`；模块挂载：同目录 `lib.rs`；crate 边界：同目录 `Cargo.toml`。
- Go 对照：同目录 `progress.go`，逐项核对 `roundPlan`、`planRound`、`waitObjectSync`、`advanceSyncedState`、事件、睡眠和后缀分桶。
- Rust 直接测试：`progress_test.rs::plan_round_cancels_sibling_meta_reads_after_first_failure`。
- Rust 行为测试：`integration_test.rs::test_checkpoint_calculator_waits_until_round_fully_synced`、`test_checkpoint_calculator_observer_sees_success_lifecycle`；`checkpoint_calculator_test.rs::test_checkpoint_calculator_does_not_advance_synced_ts_when_new_alive_store_has_no_flush`、`test_checkpoint_calculator_fails_on_object_sync_error`、`test_checkpoint_calculator_prunes_removed_store_after_files_synced`、`test_checkpoint_calculator_skips_meta_synced_by_store_progress`；`parity_test.rs::go_rust_public_contract_matches`。
- RustCodeGraph 证据：索引状态为 7032 个 Rust 文件；文件查询确认 `progress.rs` 由 calculator 主循环间接驱动；精确调用关系显示事件辅助函数由 `ComputeNextCheckpoint` 调用，生产侧 `Service::run_once` 调用 `ComputeNextCheckpoint`，而 `plan_round` 下钻到 meta 枚举与 `load_meta_file`。

本任务是纯文档分析，未运行 Cargo 或代码测试；只执行任务规定的章节结构检查，并人工复核上述源码、Go 对照和测试证据。
