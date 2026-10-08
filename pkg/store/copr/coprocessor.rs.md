# `pkg/store/copr/coprocessor.rs`

## 文件定位

本文件是 `astersql-store-copr` crate 的标准 Coprocessor 客户端核心。`pkg/store/copr/lib.rs` 将其作为 `coprocessor` 模块公开并再导出全部公开项；`pkg/store/copr/store.rs::Store::get_client` 和 `pkg/store/driver/tikv_driver.rs::TikvDriver::GetClient` 是生产侧构造 `CopClient` 的直接入口。它位于 SQL 执行产生的 DAG/Analyze/Checksum 请求与 TiKV、TiFlash 或 TiDB 内存协处理器之间，负责把逻辑 key ranges 定位、拆分成任务，调度 RPC，并把重试、分页和批处理结果重新组织成响应流。

crate 边界由 `pkg/store/copr/Cargo.toml` 定义：包名为 `astersql-store-copr`，本文件直接依赖同 crate 的 `batch_coprocessor`、`batch_request_sender`、`coprocessor_cache`、`ema`、`range_diagnostics`，并通过已发布 tag `v0.4.2-aster.10` 的 `tikv-client`、`astersql-kv`、`kvproto` 和 `protobuf` 承载读统计、限流器及锁协议。该文件没有条件编译项；测试由 `lib.rs` 以独立的 `coprocessor_test.rs` 模块接入，符合生产代码与测试分文件的布局。

## 核心职责

1. **请求与协议建模**：`CopRequest` 汇总请求类型、目标引擎、时间戳、分区 ranges、排序/分页/并发、副本读、资源组和锁提示；`CopWireRequest` 是交给 transport 的编码前协议模型；`CopProtocolResponse`/`CopResponse` 分别表示存储协议响应和上层可消费响应。
2. **Region/Store 任务构建**：`build_cop_tasks` 经 `CopBackend::split_key_ranges` 定位 Region（可细到 bucket），按 `RANGES_PER_TASK` 分片，传播行数提示、分页和超时，并在满足条件时把小任务按 store 合并。
3. **单任务执行及恢复**：`CopTaskWorker::handle_task_once` 处理缓存、runaway 检查、RU 计费、两层限流、RPC、bucket 更新、锁解析、Region 错误、越界重建、分页续页和批任务子响应拆解。
4. **并发响应流**：`CopIterator` 提供普通 worker 池、有序通道和单任务 lite worker 三种消费形态，保证取消、查询 kill、可见性检查与线程回收。
5. **策略辅助**：小任务并发估算、store batch 资格、优先级/隔离级别映射、响应内存估算和 `RateLimitAction` 的 runaway 降速状态都集中于此。

## 主要符号

- 配置枚举：`StoreType`（TiKV/TiFlash/TiDB）、`RequestType`（DAG/Analyze/Checksum）、`ReplicaReadType`、`Priority`、`IsolationLevel` 和 `RunawayAction`。它们决定任务构建、wire 字段和调度分支。
- 请求/任务：`CopRequest` 是一次逻辑请求；`PartitionKeyRanges` 保留分区 ranges 与逐 range 行数提示；`LocatedKeyRanges` 是后端定位结果；`CopTask` 是最小执行/重试单元；`BatchedCopTask` 与 `StoreBatchWireTask` 表示 store batch 的子任务。
- 后端边界：`CopBackend` 抽象 range 定位、批任务构造、TiDB 地址枚举、发送、Region/bucket 失效与更新、锁解析、MVCC 可见性，以及 TiFlash batch transport。默认 batch 方法明确返回“未配置/不可用”，不能视为已接线。
- 资源控制：`RunawayChecker` 在执行前、发送前和响应后检查；`CopRUInterceptor`/`ProductionCopRUInterceptor` 按预测读取字节、实际读取字节和 KV CPU 毫秒累计 RU；`CopRequestAttemptLimiter` 在每次实际 RPC 目标 store 上获取请求许可并记录等待时长。
- 执行：`build_cop_tasks`、`CopTaskWorker::handle_task_once`、`CopIterator::{open,open_with_lite_worker,next,close}` 和 `CopClient::{send,build_cop_iterator}` 构成主链。
- 结果：`CopTaskResult` 同时携带父响应、拆出的 batch 子响应及需要重试/续页的 `remains`；`CopResponseStream` 区分标准迭代器、通用 batch 迭代器和 TiFlash 直接 batch 响应。
- 兼容面：末尾的 `copTask`、`copIterator`、`BuildKeyRanges` 等 Go 风格别名用于迁移兼容，不是第二套实现。

## 执行流程

1. `CopClient::send` 对 TiFlash batch 请求直接调用 `CopBackend::send_tiflash_batch`；其余请求进入 `build_cop_iterator` 并立即 `open`。
2. `build_cop_iterator` 先规范化能力：非 TiKV DAG 禁用 byte paging，TiDB 或非 DAG 禁用普通 paging，不满足 `check_store_batch_coprocessor` 时关闭 store batch。随后对每个 `PartitionKeyRanges` 调用 `build_cop_tasks`。
3. `build_cop_tasks` 先用 `ensure_monotonic_key_ranges` 修正乱序 ranges；一旦重排或 hint 数量不匹配即丢弃 hints。TiDB 分支对每个 server 地址建立全 ranges 任务；存储分支调用 `split_key_ranges`，每 25,000 个 range 切一项任务，页大小按位置逐次倍增并受最大值约束。允许 batch 时，`append_batched_task` 按 `(store_id, load_based_replica_retry)` 分组且限制子任务数；降序请求最后反转任务序列。
4. `CopIterator::open_concurrent` 把任务放入共享队列，按常规并发加小任务并发创建线程。无序模式共享有界响应通道；`keep_order` 模式给每个原任务独立通道，用 sequence、`ordered_buffer` 和 `ordered_done` 串行暴露结果。`open_with_lite_worker` 只在恰好一个任务且 CAS 抢到状态时同步执行；若产生续作则触发测试钩子并回退并发模式。
5. `handle_task_once` 计算剩余 key 预算并建立 wire 请求，附加已解析/已提交锁提示、byte-paging EMA 预测及按 store 的 attempt limiter。缓存命中直接返回；否则依次执行 `before_cop_request`、请求 RU 预扣、请求级令牌获取、`backend.send`、许可释放及响应 RU 结算。
6. 成功响应先更新父/子任务 bucket 版本并执行 runaway 阈值检查。Region 错误会 backoff 后重新定位；锁错误先解析锁再把父或子任务放回 `remains`；`Request range exceeds bound` 会失效 Region、跳过 bucket 重建，并受 `MAX_EXCEEDS_BOUND_RETRIES` 限制；其他协议错误直接返回。
7. 正常结果累计扫描 key、按准入规则写缓存、拆解 batch 子响应。分页响应通过 `calculate_remain` 保留未消费 ranges 并扩大下一页大小；`CopIterator::next` 在返回每条结果前调用 `CopBackend::check_visibility(start_ts)`，维持快照可见性约束。

## 数据与状态

- `CopRequest` 在构建 iterator 后被克隆进 `Arc`，成为 worker 共享的只读请求快照；唯一共享可变请求统计 `resource_control_ru` 使用 `Mutex`。
- `CopTask` 持有 Region 版本、bucket 版本、范围、目标地址、分页游标和批子任务映射。重试不是原地循环到成功，而是返回新的 `CopTaskResult::remains`，由当前 worker 的 pending 队列继续处理。
- `keys_read` 和 `paging_task_index` 用原子量分别执行全请求 key 预算与页序号分配；`resolved_locks`、`committed_locks`、`runtime_stats` 和 limiter 等待统计使用共享 `Mutex`。
- `RuEma` 根据 byte paging 的响应读取字节更新时间感知预测；预测值写入下一次 `CopWireRequest::predicted_read_bytes`，用于 RU 预扣和分页控制。
- store batch 的成功/回退计数在所有子 backoff 与重建成功后一次性原子更新，避免失败路径重复计数；`CopResponse::mem_size` 惰性缓存响应占用估值。
- 常量不变量包括：每任务最多 `RANGES_PER_TASK = 25_000` 个 range；小任务阈值为 32 行（batch 父任务为 64）；越界自愈最多 3 次；keep-order 模式的小任务额外并发最多 20。

## 依赖与调用关系

上游生产链为 `pkg/store/driver/tikv_driver.rs::TikvDriver::GetClient` → `pkg/store/copr/store.rs::Store::get_client` → `CopClient::new`；请求侧最终调用 `CopClient::send` 或 `build_cop_iterator`。RustCodeGraph 对 `build_cop_tasks` 的被调用边未完整返回，但源码明确显示 `CopClient::build_cop_iterator` 与 `CopTaskWorker::{rebuild,rebuild_whole_store_batch}` 调用它；测试也直接调用以验证构建规则。

下游主链为 `CopClient::build_cop_iterator` → `build_cop_tasks` → `CopBackend::{split_key_ranges,build_batch_task,tidb_server_addresses}`，以及 worker 的 `handle_task_once` → `CopBackend::{send,invalidate_region,update_buckets,resolve_lock}`。RustCodeGraph 的 callees 结果还确认 `build_cop_tasks` 调用 `ensure_monotonic_key_ranges`、`grow_paging_size`、`row_hint_for_location`、`is_small_task`、`append_batched_task` 和 `range_diagnostics::first_out_of_bound_key_range_in_location`；`handle_task_once` 调用 wire 构造、重建、批响应拆解、缓存和锁处理函数。

`pkg/store/copr/network_backend.rs::NetworkBackend` 是生产网络实现：它复用 Region 元数据定位和标准 Coprocessor transport，在 `send_coprocessor_stream` 中按实际 peer/store 获取 `CopRequestAttemptLimiter` 许可并设置超时。`batch_coprocessor.rs` 提供 TiFlash batch iterator/统计，`coprocessor_cache.rs` 提供响应缓存，`batch_request_sender.rs` 提供 `Backoffer`、`KeyRanges`、错误与 Region/Peer 类型。

## 错误处理与边界

- 所有可恢复运行错误经 `BatchResult<T>`/`BatchError` 传播。取消返回 `Cancelled`，kill 信号返回 `QueryInterrupted`，后端关闭和 Region 缺失由后端错误表示；锁或 poisoned mutex 属于内部不变量破坏，代码使用 `expect` 明确失败。
- Region error：先 backoff，再重新定位父任务，并分别重建失败的 batch children。合并 batch 在发送前遇到 `MissingRegion` 时重建完整 batch ranges，避免丢掉子任务。
- 锁：解析 protobuf `LockInfo`，若存储端仍返回 wire 中已声明 resolved/committed 的锁，只额外 backoff 一次；随后调用后端 resolver 并把事务 ID 加入共享提示集合。父锁和子锁均会触发 fallback。
- 协议一致性：响应包含未知 batch task ID 或任一子响应含 `other_error` 时立即报错；write conflict 保留显式类别文本；标为 `data_merged_into_response` 的子响应只收集 runtime stats，不向上层再发空响应。
- 分页和范围：`calculate_retry`/`calculate_remain` 分别按升序或降序选择 split 两侧；limit 小于初始 page size 会禁用该任务分页；持续越界超过预算后不再无限重试。
- `CopBackend` 的默认 `build_batch_iterator` 和 `send_tiflash_batch` 是错误返回，因此调用方必须以具体后端是否覆盖为准。`CopResponseStream::Batch` 在本文件的 `send` 路径中未构造，不能仅凭枚举成员断言已被此路径使用。

## 并发与资源生命周期

`CopIterator` 拥有 worker `JoinHandle`、有界 `sync_channel`、完成标志与 kill 原子量。`open` 幂等；worker 从共享 `VecDeque` 取原始任务，并在私有 pending 队列处理重试/续页。`send_iterator_message` 使用 `try_send` 与 `yield_now` 实现背压，并在 finish 或接收端断开时退出。

`RateLimit` 用 `Mutex<in_flight> + Condvar` 限制一次 store 发送的并发，RAII `RateLimitPermit::drop` 必定归还令牌；等待期间每 10ms 检查 finish。`CopRequestAttemptLimiter` 则位于每个 RPC attempt，优先使用查询级 per-store limiter，否则使用请求 limiter，其 permit 的 `Drop` 调用外部 `Release`。两者用途不同，扩展时不可合并。

`CopIterator::close` 原子设置 finish、关闭 `RateLimitAction`、释放 lite worker CAS 状态并 join 全部线程；`Drop` 再调用 `close`，因此显式关闭与析构均安全。最后一个并发 worker 析构 `RunawayWorkerCompletion` 时重置累计 processed keys；lite 路径在耗尽时同样重置。需要注意：普通 worker 在 request-level `RateLimit::acquire` 中可观察 finish，而通道接收每 3 秒超时一次以轮询取消/kill。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/store/copr/coprocessor.go`。Rust 的 `CopClient::send`/`build_cop_iterator` 对应 Go `(*CopClient).Send`/`BuildCopIterator`；`CopTask`、`BatchedCopTask`、`CopIterator`、`CopTaskWorker::handle_task_once` 分别对应 `copTask`、`batchedCopTask`、`copIterator`、`copIteratorWorker.handleTaskOnce`。`build_cop_tasks`、`is_small_task`、`small_task_concurrency`、`calculate_retry`、`calculate_remain`、`check_store_batch_coprocessor` 和优先级/隔离级别映射均有同名或直接语义对应。

两版共同保留 Region/bucket 切分、store batch、大小任务并发、lite worker、keep-order、分页续作、缓存、锁解析、限流、runaway/RU 和 store-batch 统计等核心意图。Rust 将 Go 的 context/channel/goroutine 改为 `AtomicBool`、标准线程和 `std::sync::mpsc`，并通过 `CopBackend` 把 Region cache 与 transport 隔离；因此 API 形状不是逐行翻译。Go 文件还包含更丰富的日志、内存 tracker、execution detail 收集与 TiDB 专用错误处理；本 Rust 文件当前没有等价的全部细节，文档不将这些 Go 能力宣称为 Rust 已支持。

`pkg/store/copr/coprocessor_test.rs` 中带 `go_merge_48_` 前缀的回归直接记录迁移对齐点，包括合并协议、per-store limiter、锁 hint、批指标、子响应错误和 paging EMA。Rust 另有 `pkg/store/copr/copr_test/coprocessor_test.rs` 的 crate 外测试面；本任务主要依据任务指定的同目录独立测试。

## 扩展指南

- 新增请求字段：同时修改 `CopRequest` 默认值、`CopWireRequest`、`build_wire_request` 和具体 `CopBackend` transport；若字段影响 cache 语义，还必须更新 `response_cache_key`。对照 Go `kv.Request` 到 protobuf 的同一语义，避免只在一层接线。
- 新增错误恢复：优先在 `handle_task_once` 中明确它属于 fatal、backoff 重建、锁 fallback 还是分页续作；确保 `remains` 不丢 range，batch children 不重复响应/计数，并在 `coprocessor_test.rs` 增加回归测试。
- 调整并发/批处理：同步检查 `small_task_concurrency`、`append_batched_task`、`check_store_batch_coprocessor`、keep-order 通道容量与 `RateLimit` 容量。风险集中在乱序、死锁/背压、过度并发及 batch 指标重复。
- 调整分页：同步 `grow_paging_size`、`calculate_remain`、EMA 预测、maximum keys 预算和缓存 page start/end；必须覆盖升/降序、最终页、byte budget 与小 limit。
- 扩展后端：实现 `CopBackend` 时必须定义 Region 失效、bucket 更新、锁解析和 `check_visibility` 的真实行为；TiFlash batch 能力需显式覆盖默认报错方法。
- 测试必须继续放在独立的 `pkg/store/copr/coprocessor_test.rs`（必要时再覆盖 `copr_test/coprocessor_test.rs`），不要嵌入本生产源文件。兼容性重点是 Go 行为、wire 字段与已发布 `tikv-client` tag；性能重点是 range 复制、线程/通道数、缓存键构造和 limiter 等待。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/store/copr` 确认目标、Go 对照和独立测试均已索引；`node --file pkg/store/copr/coprocessor.rs` 阅读了目标全貌；`query build_cop_tasks` 定位生产符号及相关测试；`callees build_cop_tasks` 与 `callees handle_task_once` 核对了上述关键下游边。调用方查询未产出完整结果，因此上游关系改由实际源码引用核验，没有据此猜测。
- 已读生产/配置路径：`pkg/store/copr/coprocessor.rs`、`pkg/store/copr/Cargo.toml`、`pkg/store/copr/lib.rs`、`pkg/store/copr/store.rs`、`pkg/store/copr/network_backend.rs`、`pkg/store/driver/tikv_driver.rs`。
- 已读对照与测试路径：`pkg/store/copr/coprocessor.go`、`pkg/store/copr/coprocessor_test.rs`；测试名覆盖 range 单调性与 Region/bucket 切分、分页、大小任务并发、store batch、限流、锁、RU/runaway、batch 错误/指标及有界通道。
- 结构验证使用任务指定命令，要求文件存在且恰好包含本文 11 个固定二级标题。本文为静态分析文档，按计划不运行 Cargo，也不声称执行了运行时测试。
