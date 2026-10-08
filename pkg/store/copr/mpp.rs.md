# `pkg/store/copr/mpp.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate。`pkg/store/copr/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/store/copr/lib.rs` 声明 `pub mod mpp` 并公开再导出其符号。它位于 MPP 计划/协调层与 TiFlash RPC 后端之间：接收查询身份、计划片段和 key range，把 range 转为按 Region/store 聚合的 batch cop 任务，再负责下发、取消、建立结果流、检查快照可见性及统计可用 MPP store。

真实依赖通过两个 trait 注入：`BatchTaskSource` 提供 Region 切分和拓扑信息，`MppTransport` 提供网络操作及缓存失效。`pkg/store/copr/store.rs::Store::get_mpp_client` 用 `RegionCache` 和 `StoreMppTransport` 组装具体 `MppClient`。但当前 `MppClient` 没有实现 `pkg/kv/mpp.rs::MPPClient`，`pkg/store/driver/kv_adapter.rs` 暴露给统一 KV 接口的仍是 `UnsupportedMppClient`；因此本文件是可组装的 copr 内部实现，不能据此宣称 Rust 上层 MPP 主链已经完整接通。

## 核心职责

1. 用 `MppBuildTasksRequest` 区分分区表与非分区表请求，并以 MPP 专用参数调用相邻 `batch_coprocessor.rs` 的任务构建器。
2. 把 `DispatchMppTaskRequest` 转为线格式 `MppDispatchWireRequest`，选择普通 Region 或分区 `TableRegions` 表示，并给出重试/错误结果。
3. 向参与查询的所有 store 并行发送取消请求；在特定存算分离拓扑下，传输失败会使 compute store 缓存失效。
4. 建立 sender 到 coordinator/root receiver 的 MPP 数据通道，并以超长读取超时承载查询结果。
5. 用原子变量缓存 MPP store 数量，避免每次规划都重新列举 store，同时过滤 `engine_role=write` 节点。

本文件不实现计划切分算法、Region 路由或 RPC 编解码；任务切分位于 `batch_coprocessor.rs`，真实后端调用由 `MppTransport` 的实现承担。

## 主要符号

- 超时和缓存常量：`READ_TIMEOUT_MEDIUM` 为下发使用的 60 秒读超时，`READ_TIMEOUT_SHORT` 为取消使用的 5 秒超时，`MPP_STORE_COUNT_TTL_MICROS` 为 120 秒 store 数缓存 TTL；建连复用 `TI_FLASH_READ_TIMEOUT_ULTRA_LONG`。
- `MppQueryId`：由 `query_ts`、`local_query_id`、`server_id` 组成查询身份；`PartitionRanges` 保存物理分区 ID 与 range；`MppBuildTasksRequest` 保存 `start_ts` 以及互斥使用的普通/分区 range。
- `MppTaskMeta`：线协议共用任务元数据，包含查询、任务、gather、地址、版本、资源组、连接和 digest 信息。`DispatchMppTaskRequest::address` 优先使用 `meta: batchCopTask` 的地址，否则回退到请求自身地址。
- `DispatchMppTaskRequest`、`MppDispatchWireRequest`、`MppDispatchResponse`：分别代表高层下发参数、传输边界请求和响应。响应的 `retry_regions` 用于通知本地 RegionCache 失效。
- `MppCancelRequest` 与 `MppConnectionRequest`：分别承载取消元数据，以及 sender/receiver 两端元数据。
- `MppStream`：当前是 `closed: Arc<AtomicBool>` 加内存 `packets` 的轻量结果对象；`close`/`is_closed` 只管理本地关闭标志，本文件没有实现持续拉取网络包的迭代器。
- `MppTransport`：下发、取消、建连、可见性、store 枚举、Region 失效和 compute store 失效的依赖反转边界。`pkg/store/copr/store.rs::StoreMppTransport` 将这些操作委托给 `StoreBackend`/`RegionCache`。
- `MppStoreCount`：用 `count`、`last_update`、`initialized` 三个原子字段维护带 TTL 的计数缓存；`get` 接受一次性抓取闭包，便于替换实际 store 来源。
- `MppClient`：持有 `BatchTaskSource`、`MppTransport`、存算分离/自动扩缩容开关及 store 数缓存。主要方法为 `construct_mpp_tasks`、`dispatch_mpp_task`、`cancel_mpp_tasks`、`establish_mpp_connection`、`check_visibility`、`get_mpp_store_count`。
- `MPPClient`、`mppStoreCnt`：文件尾为迁移代码保留的 Go 风格类型别名，不是 `pkg/kv/mpp.rs::MPPClient` trait 的实现。

## 执行流程

任务构造从 `construct_mpp_tasks` 开始。它建立 `BatchBuildOptions`，固定 `is_mpp=true`、连续性均衡开启、连续 Region 块大小为 20，并带入拓扑开关、TTL、dispatch policy 和 replica-read policy。若 `partition_id_and_ranges` 存在，则分别收集分区 ID 和 `KeyRanges`，调用 `build_batch_cop_tasks_for_partitioned_table`；否则要求 `key_ranges` 非空并调用非分区入口。`start_ts` 当前保存在请求中但没有在本方法内继续传给构建器，这是与 Go context 注入路径的可见差异。

下发由 `dispatch_mpp_task` 完成。`task_meta` 复制查询和会话字段；有 `batchCopTask` 时把其 `regionInfos` 转为 `CoprocessorRegionInfo`，同时复制 `PartitionTableRegions`。分区表示非空时清空普通 `regions`，避免同一批 Region 重复编码。随后用 60 秒超时调用 `transport.dispatch`。传输成功后逐个失效响应中的 `retry_regions`，返回 `(Some(response), false, None)`。

传输失败时，存算分离且未启用 autoscaler 会先使 compute store 缓存失效。有 `meta` 的 Region-bearing task 不做本地重试，因为需要重新切割计划与调度；取消错误也不重试。仅无 Region 元数据且非取消的错误交给 `Backoffer`，退避成功才把 `retry` 置为 `true`。

`cancel_mpp_tasks` 对空请求或空地址集直接返回；否则仅从第一项请求构造专用 cancel meta，不复用 dispatch meta。它对每个 store 启动一个作用域线程并行发送 5 秒超时的取消请求，等待全部线程结束。取消是尽力而为：错误不向调用者返回，只在上述特定拓扑下触发 compute store 缓存失效。

`establish_mpp_connection` 将调用方给出的 `sender_meta` 与从 dispatch 请求生成的 receiver meta 组合，强制 receiver `task_id=-1`，再以超长超时调用 `transport.establish`。取消错误不重试；其他错误经过 `Backoffer` 决定重试，并同样按拓扑条件失效 compute store 缓存。`check_visibility` 是直接委托；`get_mpp_store_count` 则以固定 120 秒 TTL 调用计数缓存。

## 数据与状态

`DispatchMppTaskRequest.meta` 同时承担路由来源和地址优先级：存在时使用 `batchCopTask::get_address`，不存在时允许高层无表任务直接给出 `address`。普通任务把 Region 放入 `regions`；分区任务把它们按表放入 `table_regions`，下发时两种表示不得重复承载同一批 Region。

`MppStoreCount` 的 fast path 仅在 `initialized=true` 且尚未过期时返回缓存值。过期后线程用 `last_update.compare_exchange` 竞争刷新权；已经初始化的失败竞争者立即读取旧计数，未初始化的失败竞争者仍会抓取数据。抓取失败会清除 `initialized`，迫使下次再次刷新；成功后排除 label `engine_role=write` 的 store。返回值是本次抓取得到的计数，即使并发刷新导致它不一定最终写入共享缓存。

计数以 `AtomicI32` 保存并在读取时用 `max(0)` 防止负数转换；实际 `Vec<Store>::len()` 转为 `i32` 没有显式溢出检查。`MppStream.closed` 可由克隆共享，Acquire/Release 保证关闭状态可见；`packets` 本身不是共享同步容器。

## 依赖与调用关系

直接下游是 `batch_coprocessor.rs` 的 `BatchBuildOptions`、`BatchTaskSource`、`batchCopTask` 及两个 `build_batch_cop_tasks_for_*` 入口，以及 `batch_request_sender.rs` 的 `Backoffer`、`BatchError`、Region/store/range 类型。`Cargo.toml` 表明 crate 还依赖固定 tag `tikv-client v0.4.2-aster.10`、`kvproto`、`grpcio`、`protobuf` 和 Tokio，但本文件自身的并发使用标准库线程与原子变量。

直接组装者是 `pkg/store/copr/store.rs::Store::get_mpp_client`：`RegionCache` 作为 `BatchTaskSource`，`StoreMppTransport` 把操作转发到 `StoreBackend`，store 列表来自 `RegionCache::all_tiflash_stores`。`pkg/store/copr/lib.rs` 对外再导出本文件符号。

应用层预期接口定义在 `pkg/kv/mpp.rs::MPPClient`；`pkg/executor/internal/mpp/local_mpp_coordinator.rs::MppClientCoordinatorTransport` 通过该 trait 执行 Dispatch、Establish、Cancel 和 CheckVisibility，planner 的 `fragment.rs` 负责生成 fragment/task 拓扑。但源码搜索确认本文件的 `MppClient` 尚未实现该 trait，上述上游无法直接调用这里的小写方法；当前 client-rust adapter 使用 `UnsupportedMppClient`。因此调用图应分为“内部已实现并可由 Store 组装”与“统一上层接口尚未接线”两层理解。

RustCodeGraph 对目标文件的索引列出 49 个符号；精确查询定位所有主要方法。图的 blast-radius 输出也把 Rust 小写方法主要映射为本文件自调用/测试调用，而把上层协调器映射到 `pkg/kv/mpp.rs` 的大写 trait 方法，这与源码接线核验一致。

## 错误处理与边界

- `construct_mpp_tasks` 在非分区请求缺少 `key_ranges` 时返回 `BatchError::OtherResponse("KeyRanges in MPPBuildTasksRequest is nil")`；若分区字段存在，即使为空也优先走分区路径。
- 下发 Region-bearing task 遇到任何传输错误都返回 `retry=false`；无 Region task 只有在退避成功后才允许外层重试。`BatchError::Cancelled` 永远不重试。
- 成功下发并不等于远端业务成功：`MppDispatchResponse.error` 原样留给调用者判断；本文件只处理 `retry_regions` 的缓存失效。
- 取消没有返回错误通道，且不会重试。调用方只能依赖 TiFlash 最终回收任务；空请求/地址集为无操作。
- 建连错误路径没有可供本方法关闭的半开 stream，因为 `MppTransport::establish` 的错误分支不携带 stream；这不同于 Go 可在 `rpcResp` 非空时显式关闭的路径。
- `MppStoreCount::now_micros` 在系统时间早于 Unix epoch 时回退为 0，并对超过 `i64::MAX` 的微秒值截顶；`saturating_sub` 避免时钟倒退产生算术溢出。
- trait/backend、互斥 range 表示和上层接口尚未接线都是扩展时必须保持或补齐的边界，不能用成功编译或默认空返回代表 MPP 可用。

## 并发与资源生命周期

取消广播使用 `thread::scope`，每个目标地址一个 OS 线程；作用域返回前会等待全部请求结束，因此借用的地址集合安全，但目标 store 数量直接决定瞬时线程数。共享 `got_error` 是 `Arc<AtomicBool>`，线程以 Release 写入，汇总处以 Acquire 读取。

store 数缓存采用无锁原子协调。`last_update` 既是刷新时间又充当竞争标记；已初始化时允许竞争失败者继续消费旧值，优先降低规划请求的等待。首次初始化时多个线程仍可能同时 fetch，这是源码有意保留的 Go 语义，而不是严格 single-flight。fetch 失败把缓存标成未初始化，但不回滚 `last_update`；下次调用因 `initialized=false` 仍会尝试抓取。

`MppClient` 自身没有显式 `close`/`Drop`。`Arc<dyn BatchTaskSource>` 和 `Arc<dyn MppTransport>` 随最后一个客户端引用释放；本文件不拥有独立运行时。`MppStream::close` 幂等设置本地标志，不清空 `packets`，也不在 Drop 时自动关闭远端连接，具体远端资源生命周期取决于 `StoreBackend` 实现和上层消费者。

## 与 Go 版本的对应关系

直接对照为 `pkg/store/copr/mpp.go`。Rust 保留了 Go 的主要阶段和参数：分区/非分区任务构造，60 秒下发超时，Region task 不本地重试，无 Region task 退避，stale Region 失效，取消向所有 store 并发广播，receiver task ID 为 -1，超长连接读，以及 120 秒 store 计数缓存和 write node 过滤。`mpp_test.rs::repeated_cancel_broadcasts_and_wire_meta_matches_go` 进一步确认专用 cancel meta 只填充 Go 对应字段，并证明对同一输入重复调用 Rust 方法会重复广播；Go 注释所说“只允许第一次生效”依赖更上层的独占调用约束，不由 `mpp.go::CancelMPPTasks` 自身保存状态。

Rust 通过 `BatchTaskSource`/`MppTransport` 解耦了 Go 对 `kvStore`、PD client、Region sender 和 tikvrpc 的直接依赖。Rust 用 `(Option<T>, bool, Option<BatchError>)` 表达响应、重试和错误三元组，而 Go 使用多返回值。Rust 请求允许在没有 `meta` 时回退到显式地址；Go `DispatchMPPTask` 仍调用 `req.Meta.GetAddress()`，其高层类型约束不同。

尚未对齐或未在本文件体现的部分包括：Go 将 `StartTS` 注入 context、Region sender 的 execution-info 收集和 RPC error 日志、protobuf/tikvrpc 包装、gRPC status 的取消判定、failpoint、stale Region 日志原因、建连失败时关闭已得到的 stream，以及生产 `kv.MPPClient` 接线。Rust 的 `MppStream` 还是内存包集合，不等同于 Go 的 server-streaming response。上述差异均应视为当前迁移边界，不应从 Go 行为推断 Rust 已支持。

## 扩展指南

- 完成生产接线时，应为 concrete adapter 实现 `pkg/kv/mpp.rs::MPPClient`，明确 `kv` 与 copr 两套请求、响应、错误、stream 和 backoff 的转换；同时替换 `UnsupportedMppClient`，补独立集成测试证明 planner/coordinator 能到达这里，不能只保留空任务或默认成功桩。
- 修改任务构造时同步检查 `BatchBuildOptions` 的 MPP 固定值和 `batch_coprocessor.rs` 两个构建入口；若要使用 `start_ts`，应先核对 Go context 语义和 RegionCache/事务可见性契约。
- 修改下发元数据时同时维护 `task_meta`、cancel 专用 meta、connection receiver meta、`MppDispatchWireRequest` 和独立测试；特别测试普通/分区 Region 互斥、meta 缺失地址、远端业务错误与 retry regions。
- 修改重试策略时保持 Region-bearing task 必须重新切片调度的约束，分别覆盖 Cancelled、退避成功/耗尽、存算分离 autoscaler 开关及缓存失效次数。
- 修改取消并发时测试空输入、多地址、部分失败、重复调用和大量地址的资源上限；若改为线程池/异步任务，必须保留返回前是否等待全部取消完成的契约。
- 修改 store 数缓存时测试 TTL 命中、首次并发初始化、刷新竞争、fetch 失败后重试、write node 过滤及时间倒退；测试逻辑继续放在独立 `mpp_test.rs`，不要嵌入生产文件。
- 将 `MppStream` 升级为真实流时，需要定义 Recv/Close/Drop、半开连接错误回收、背压、取消和 packet 所有权，并同步协调器适配层，避免仅在内存 `Vec` 上模拟成功。

## 验证依据

- 目标源码：`pkg/store/copr/mpp.rs`，通过 RustCodeGraph `files --filter` 确认已索引，`node --file ... --offset 280 --limit 340` 阅读客户端、重试、取消、建连和缓存尾部，并人工核对文件前半的数据结构、trait 与常量。
- RustCodeGraph：`status` 报告索引含 7032 个 Rust 文件、目标文件 49 个符号；`explore` 查询 `MppClient` 及六个主入口，`query --json` 精确定位 `construct_mpp_tasks`、`get_mpp_store_count` 和 `MppStoreCount`。调用图对重载符号存在歧义且按内部小写/上层大写接口分流，因此又用源码搜索核对真实接线，没有把模糊 blast-radius 当作唯一证据。
- crate 与组装：`pkg/store/copr/Cargo.toml`、`pkg/store/copr/lib.rs`、`pkg/store/copr/store.rs`，确认 crate 入口、依赖、再导出、`StoreMppTransport` 委托和 `Store::get_mpp_client` 的构造关系。
- 上层边界：`pkg/kv/mpp.rs`、`pkg/executor/internal/mpp/local_mpp_coordinator.rs`、`pkg/planner/core/operator/physicalop/fragment.rs`、`pkg/store/driver/kv_adapter.rs`；确认统一 trait、协调器消费方式、planner fragment 生成，以及当前 `UnsupportedMppClient`。`rg` 未发现 `MppClient` 对 `kv::MPPClient` 的实现。
- Go 对照：`pkg/store/copr/mpp.go`，核对任务构造、下发/取消/建连、重试与 PD/Region 缓存失效、store 数缓存；仓库中不存在 `pkg/store/copr/mpp_test.go`，因此没有声称 Go 独立测试覆盖。
- Rust 独立测试：`pkg/store/copr/mpp_test.rs`，当前唯一 `#[test]` 验证重复取消广播及专用 cancel meta 字段；任务构造、下发、建连、store 数缓存和生产接线尚无该文件内的直接测试证据。
- 本任务按总计划仅做文档分析，不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰好包含十一个固定二级标题，并人工复查只新增本说明文档、未修改 `plan.md` 或运行时代码。
