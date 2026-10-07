# `br/pkg/streamhelper/collector.rs`

## 文件定位

`collector.rs` 属于 Cargo 包 `astersql-br-pkg-streamhelper`；该包以 `br/pkg/streamhelper/lib.rs` 为入口，通过 `#[path = "collector.rs"] pub mod collector` 装入模块，再以 `pub use collector::*` 暴露公开接口。`br/pkg/streamhelper/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `br/pkg/streamhelper`，因此本文件是 Go 文件 `br/pkg/streamhelper/collector.go` 的 Rust 移植，而不是一个通用指标收集器。

它位于日志备份检查点推进链中：`CheckpointAdvancer::tryAdvance` 创建 `ClusterCollector`，`CheckpointAdvancer::GetCheckpointInRange` 用 `IterateRegion` 扫描目标 key range 并逐个调用 `CollectRegion`，最后 `Finish` 返回全局最保守的 region flush TS 和需要重扫的子区间（`br/pkg/streamhelper/advancer.rs:454-517`）。该收集器的生命周期短于一次 advancer tick，每轮推进都重新创建。

## 核心职责

- 将 `RegionWithLeader` 按 leader 的 `StoreId` 分组；每个实际出现的 store 懒创建一个 `StoreCollector` 和一个工作线程，避免为未涉及的 store 建立客户端。
- 将同一 store 的 region identity 批量装入 `GetLastFlushTSOfRegionRequest`，批大小上限由 `defaultBatchSize = 1024` 控制。请求可以包含不连续 region，因为 TiKV 服务端接口接受离散 region 列表。
- 把成功响应折叠为最小 checkpoint TS。最小值是安全水位：只有所有成功 region 都至少 flush 到该位置，调用方才可据此推进。
- 将无 leader region、以及 RPC 响应中带 region error 的条目转成 `FailureSubRanges`，让上层后续缩小范围重新扫描；这类 region 级不一致不会让整轮立即失败。
- 将获取客户端或执行 RPC 的 store 级错误上抛，并在 RPC 错误时调用 `Env::ClearCache`，促使下次重建连接。
- 可通过 `SetOnSuccessHook` 在每个成功 region 返回时回写其 `(checkpoint, KeyRange)`；advancer 和订阅补洞路径用该钩子更新区间检查点树。

## 主要符号

- `defaultBatchSize: usize = 1024`：单次 RPC 的 region 数上限，与 Go `defaultBatchSize` 相同。
- `OnSuccessHook = Arc<dyn Fn(u64, KeyRange) + Send + Sync>`：成功响应回调。`Send + Sync` 允许闭包被 store 工作线程调用；设置动作应发生在首次 `CollectRegion` 之前，因为新 worker 只克隆创建时的 hook。
- `StoreCollector`：私有的单 store 状态机。`storeID` 和 `service` 确定 RPC 目标；`rx` 是一次性取出的接收端；`currentRequest`、`checkpoint`、`inconsistent`、`regionMap` 保存本轮聚合状态；`err` 保存首个致命错误；`done` 表示 worker 已退出。
- `StoreCollector::report_err`：只记录第一个错误，后续错误不覆盖根因。
- `StoreCollector::append_region_map`：建立 region id 到原始 `[StartKey, EndKey)` 的映射，供响应回填。
- `StoreCollector::send_pending`：获取 store 客户端、发送当前请求、清空已发送的 `Regions`，处理成功 checkpoint 或失败区间，并维护本 store 最小 checkpoint。
- `StoreCollector::recv_loop`：持续接收 region，满批即发送；发送端关闭后仍发送尾批，然后置 `done`。
- `StoreCheckpoints`：公开结果，包含 `HasCheckpoint`、最小 `Checkpoint` 和 `FailureSubRanges`。`merge` 只在对方确有 checkpoint 时更新最小值，并总是追加失败区间；`Display` 输出 checkpoint 或 `none` 以及剩余区间数。
- `RunningStoreCollector`：私有运行句柄，将 `Arc<StoreCollector>`、`JoinHandle` 和输入 `Sender` 绑在一起，确保 `Finish` 能按顺序关闭和等待。
- `ClusterCollector`：公开的集群级控制器；`collectors` 按 store id 保存 worker，`noLeaders` 保存无法路由的范围，`cancelled` 阻止失败或结束后的继续投递。
- `NewClusterCollector`：创建空控制器。
- `ClusterCollector::SetOnSuccessHook`：保存成功回调，供以后懒创建的 worker 继承。
- `ClusterCollector::CollectRegion`：处理取消、无 leader、懒创建 worker、worker 早退错误和 channel 投递。
- `ClusterCollector::Finish`：消费 `self`，关闭所有输入端、等待全部线程、检查错误并合并结果；调用后不能复用该实例。

## 执行流程

1. `CheckpointAdvancer::tryAdvance` 合并重叠范围，调用 `NewClusterCollector(self.env.clone())`；若需要细粒度更新，则先安装成功 hook。
2. `GetCheckpointInRange` 通过 `IterateRegion` 分页扫描 `[start, end)`，把每个 `RegionWithLeader` 交给 `CollectRegion`。
3. `CollectRegion` 首先检查 `cancelled`。已取消时按 Go 语义静默返回成功；leader store id 为 0 时不发 RPC，只把该 region 的 key range 放入 `noLeaders`。
4. 首次遇到某个有效 store id 时创建无界 `mpsc::channel`、构造 `StoreCollector`，并用 `thread::spawn` 启动 `recv_loop`。后续相同 store 的 region 复用该 worker。
5. `recv_loop` 为每个 region 记录范围，并把 `(region id, epoch version)` 追加到请求。累计到 1024 条时调用 `send_pending`。
6. `send_pending` 通过 `Env::GetLogBackupClient(storeID)` 获得客户端，再调用 `GetLastFlushTSOfRegion`。RPC 成功后先清空请求中的 region 列表，再逐项处理响应：带 `Err` 的项追加原范围；成功项调用 hook，并更新本 store 的最小非占位 checkpoint。
7. `Finish` 先置 `cancelled`，随后逐 store 取走并丢弃 `Sender`。channel 关闭让 `recv_loop` 调用一次 `send_pending` 冲刷尾批并退出；主线程 `join` 后才读取 worker 的错误和聚合字段。
8. 任一 store 保存了致命错误时，`Finish` 返回带 `store {id}:` 上下文的错误；否则用 `StoreCheckpoints::merge` 计算所有 store 的最小 checkpoint，并把 region error 与无 leader 范围合并返回。

一个容易忽略但由 `collector_test.rs::full_batch_still_sends_go_equivalent_empty_tail_request` 固定下来的行为是：恰好满 1024 条时先发送满批，`Finish` 关闭 channel 后仍会发送一次空尾批，因此客户端会被调用两次。这与 Go `recvLoop` 在 input 关闭时无条件调用 `sendPendingRequests` 一致，不能擅自以“空请求优化”为由省略。

## 数据与状态

`regionMap` 保存本轮见过的所有 region 范围，即使某批已经发送也不会清除；响应只携带 region identity，必须借此恢复回调与失败重扫所需的 key range。若响应包含请求中未知的 region id，`HashMap::get(...).cloned().unwrap_or_default()` 产生空起止键的零值 `KeyRange`；`collector_test.rs::unknown_failed_region_preserves_go_zero_value_range` 明确将其视为 Go map 缺失键语义，而不是错误。

`checkpoint == 0` 同时充当“尚无成功 checkpoint”的占位值，因此结果用 `HasCheckpoint = cp != 0` 区分是否有效。代码沿用 Go 的前提：服务端成功 checkpoint 不会为 0。收到多个成功条目时，`send_pending` 取最小值；多个 store 汇总时，`StoreCheckpoints::merge` 再取全局最小值。失败区间不去重、不排序，保持发现和合并顺序。

`currentRequest` 在 RPC 成功后只清空 `Regions`；若获取客户端或 RPC 失败，它保持原内容，但 worker 随即记录错误并退出，不再重用该请求。`noLeaders` 属于集群控制器，不对应任何 store worker，在 `Finish` 初始化结果时一次性并入。

## 依赖与调用关系

上游主调用边为：

- `CheckpointAdvancer::tryAdvance` → `NewClusterCollector` → `SetOnSuccessHook`（可选）→ `GetCheckpointInRange` → `ClusterCollector::CollectRegion` → `ClusterCollector::Finish`（`br/pkg/streamhelper/advancer.rs:477-517`）。
- `CheckpointAdvancer::optionalTick` 调用 `tryAdvance`，成功 hook 将 region checkpoint 合并进 `ValueSortedFull`；后续 `importantTick` 读取其最小值并上传全局 checkpoint（`advancer.rs:519-590`）。
- 订阅测试的补洞流程同样扫描落后区间，通过 `SetOnSuccessHook` 把轮询结果合并回 span 树（`br/pkg/streamhelper/subscription_test.rs:248-295`）。

直接下游依赖均位于本 crate：`advancer_env::Env` 提供 `GetLogBackupClient` 和 `ClearCache`；`regioniter::RegionWithLeader` 提供 region、epoch 与 leader；`stubs` 提供请求、identity 和 `KeyRange` 数据类型。文件自身只使用 Rust 标准库的 `HashMap`、原子变量、`mpsc`、`Arc/Mutex` 和线程；`Cargo.toml` 中列出的 `regex`、`serde`、`serde_json`、`uuid` 以及两个本地 spans/config crate 并非本文件的直接依赖。

RustCodeGraph 将 `collector.rs` 标为由 `advancer.rs`、`collector_test.rs`、`advancer_test.rs`、`parity_test.rs`、`br/pkg/stream/stream_metas.rs` 等文件使用；公开符号经 `lib.rs:140` 再导出，其他 crate 可以从包根调用。

## 错误处理与边界

- `GetLogBackupClient` 失败直接结束该 store worker；`GetLastFlushTSOfRegion` 失败前会 best-effort 调用 `ClearCache`，忽略清缓存自身错误，再保留原 RPC 错误。
- region 响应中的 `checkpoint.Err` 是可恢复的拓扑/epoch 不一致，不作为整个调用的 `Err` 返回，而是转成 `FailureSubRanges`。无 leader 也采用相同的延后重扫策略。
- `CollectRegion` 发现 worker 已退出且首错存在时，将集群控制器标为取消并返回该错误。若发送端已经关闭，则返回字符串 `collector channel closed`。
- `Finish` 等待全部 worker 后读取首错，并增加 `store id` 上下文。它在遇到第一个 store 错误时提前返回，因此不会把多个 store 错误聚合成一条结果；`HashMap` 遍历次序也不保证哪个错误先被观察。
- `Mutex::lock().unwrap()` 和 worker `join` 的返回值处理体现了边界限制：互斥锁中毒会 panic，而 worker panic 的 `join` 错误被忽略。相比 Go 的 `utils.PanicToErr`，Rust 版本不会把 worker panic 转为可返回错误。
- Rust `Finish` 没有 context/timeout 参数，会一直 join 到 worker 退出；Go `spawn` 返回的 waiter 可以被调用方 context 取消。当前同步 stub RPC 没有取消参数，因此真实慢/挂 RPC 的可取消性并未与 Go 完全对齐。
- 对已 `cancelled` 的控制器继续调用 `CollectRegion` 会静默成功但不处理输入，与 Go 的 `masterCtx.Err() != nil` 分支一致。

## 并发与资源生命周期

每个 store 恰有一个 `recv_loop` 工作线程，store 之间并行，同一 store 内串行批处理。`ClusterCollector` 本身的方法需要 `&mut self`，所以正常调用侧串行操作控制器；跨线程共享的是 `Arc<StoreCollector>` 和成功 hook。

生命周期的关键顺序是“取走并 drop sender → receiver 得到关闭 → 冲刷尾批 → worker 置 `done` → 主线程 join → 读取结果”。这建立了读取聚合状态前的线程完成边界。虽然字段用 `Mutex` 包装，设计不变量仍是 `recv_loop` 在运行期独占写入 `currentRequest`、`checkpoint`、`inconsistent` 和 `regionMap`，主线程只在 join 后读取最终值。

`done` 与 `cancelled` 使用 `SeqCst`，优先保证清晰的跨线程可见性。`err` 的“首次错误胜出”由互斥锁保证。`onSuccess` 在 worker 线程中执行，耗时或阻塞的 hook 会延迟该 store 的后续批次和 `Finish`；hook 若 panic，worker 会异常退出，而当前 `Finish` 忽略 join panic，这是扩展时的重要风险。

Rust 使用无界 `std::sync::mpsc::channel`，而 Go `newStoreCollector` 创建容量为 1024 的输入 channel。Rust 生产者不会因 worker 落后而在 channel 容量处背压，可能积累更多内存；这属于当前移植差异，不能从 Go 的有界 channel 推断 Rust 已具备相同背压。

## 与 Go 版本的对应关系

符号关系基本一一对应：`storeCollector`/`StoreCollector`、`clusterCollector`/`ClusterCollector`、`sendPendingRequests`/`send_pending`、`recvLoop`/`recv_loop`、`StoreCheckpoints.merge`/`StoreCheckpoints::merge`。批大小、按 store 懒创建、关闭输入时冲刷尾批、checkpoint 取最小值、无 leader/region error 转失败范围、未知 region id 得到零值范围，以及 RPC 失败后清连接缓存等核心语义保持一致。

已验证的对齐证据包括：Go `advancer_test.go::TestBasic` 与 Rust `advancer_test.rs::test_basic` 都扫描全范围并断言结果等于跨 store 最小 checkpoint；Go `TestCollectorFailure` 断言永久客户端失败时 `Finish` 必须报错且不能卡死；Rust `collector_test.rs` 额外固定满批后的空尾请求和未知 region 的零值范围；Rust `parity_test.rs::go_rust_public_contract_matches` 验证 `merge` 取较小 TS、累计失败范围，以及 direct collector `Finish` 得到 checkpoint 88。

当前差异也必须保留在认知中：

- Go 输入 channel 容量为 1024，Rust channel 无界。
- Go 用 `context.Context` 取消 worker/RPC，并允许 `Finish(ctx)` 等待超时；Rust 用原子取消标志，仅阻止新投递，不能打断正在执行的同步 RPC。
- Go `clusterCollector` 有 mutex 保护并可从共享指针调用；Rust 用 `&mut self` 在类型层限制控制器的并发修改。
- Go `utils.PanicToErr` 可把 recv loop panic 变成错误；Rust 忽略 `JoinHandle::join` 的 panic 结果。
- Go 记录 checkpoint 请求指标和结构化日志，并检查 region leader 是否仍等于目标 store；Rust 当前没有这些指标/日志与告警，但 RPC 路由仍按收集时的 leader store id。

## 扩展指南

- 调整批处理策略时修改 `defaultBatchSize`、`recv_loop` 和 `send_pending`，并同步 `collector_test.rs`。必须保留 channel 关闭时的尾批语义，除非同时明确改变并更新 Go 对齐契约；还应增加小于、等于、大于批大小及多 store 的独立测试。
- 新增响应分类或重试策略时，接入点是 `send_pending`。应区分 transport/store 级致命错误与 region 级可恢复错误，保持失败范围可定位，并验证 `ClearCache` 的调用条件。若要识别 EpochNotMatch/NotLeader 指标，应在不丢失原 `KeyRange` 的前提下扩展。
- 修改全局聚合规则时集中调整 `StoreCheckpoints::merge`，并同步 `parity_test.rs` 中的公开契约测试；安全水位必须仍是成功 region 的最小值，不能误改为最大值或平均值。
- 增加成功 hook 行为时，应在第一次 `CollectRegion` 前完成配置，或明确实现对已启动 worker 的传播；hook 必须短小、无阻塞、避免 panic。相关回归应放在独立文件 `collector_test.rs` 或 `subscription_test.rs`，不要把测试嵌入生产文件。
- 若接入真实异步客户端，优先补齐 Go 的有界背压、context 取消、RPC 超时和 panic/任务失败传播，再评估是否从 OS thread 改为异步任务；这些变化会影响内存、停止延迟和兼容性，不能仅以编译通过替代行为验证。
- 如需优化 `regionMap` 内存，应证明服务端响应与批次严格对应后再按批清理；当前跨响应查找和未知 id 的 Go 零值兼容行为都必须保留。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件（其中 Rust 7,032 个）；`files --filter br/pkg/streamhelper` 确认目标、Go 对照、模块入口和独立测试均已索引。
- RustCodeGraph：`node --file br/pkg/streamhelper/collector.rs --offset 1 --limit 400` 读取完整 318 行，并确认该文件被 `advancer.rs`、`collector_test.rs`、`advancer_test.rs`、`parity_test.rs`、`stream_metas.rs` 等使用。
- RustCodeGraph：读取 `br/pkg/streamhelper/advancer.rs:454-590`，确认 `GetCheckpointInRange`、`tryAdvance`、`optionalTick` 和全局检查点树之间的调用关系。
- RustCodeGraph：读取 `br/pkg/streamhelper/collector.go:1-321`，核对 Go 的批处理、context、错误、缓存清理、尾批与合并语义。
- RustCodeGraph：读取 `br/pkg/streamhelper/collector_test.rs:1-146`、`advancer_test.rs:124-139`、`parity_test.rs:400-543`、`subscription_test.rs:248-295`，核对 Rust 边界与主链行为；读取 `advancer_test.go:42-61,399-426` 和 `subscription_test.go:239-268`，核对 Go 基础、失败和 hook 使用意图。
- 文件读取：`br/pkg/streamhelper/Cargo.toml` 核对 crate 名、`lib.rs` 入口和 Go package 元数据；RustCodeGraph 读取 `br/pkg/streamhelper/lib.rs:20-145` 核对模块装配、独立测试挂载与公开再导出。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，并人工检查文档只描述已有符号、调用边、测试与明确列出的移植差异。
