# `pkg/store/copr/batch_coprocessor.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate；`pkg/store/copr/Cargo.toml` 将该 crate 的入口设为 `lib.rs`，而 `lib.rs` 声明并公开再导出 `batch_coprocessor`。它位于 SQL 层生成的 key range 与 TiFlash Batch Cop RPC 之间：把 range 按 Region 切分，选择可用 TiFlash/Compute 节点，生成按地址聚合的任务，并把并行执行结果包装成上层可迭代消费的响应。

直接上游之一是 `pkg/store/copr/mpp.rs`：其任务构建流程调用 `build_batch_cop_tasks_for_partitioned_table` 或 `build_batch_cop_tasks_for_non_partitioned_table`。`pkg/store/copr/store.rs` 则把 RegionCache 作为 `BatchTaskSource` 交给 MPP 客户端。RPC、Region、range、backoff 等线类型来自相邻的 `batch_request_sender.rs`；本文件不直接实现网络传输。

## 核心职责

1. 用 `batchCopTask`/`batchCopResponse` 表达按 store 聚合的请求和返回结果，并记录运行时统计及近似内存占用。
2. 在存算一体模式中发现 Region 副本、过滤存活/同 zone 的 TiFlash store，并以连续性优先或加权贪心算法重新均衡 Region。
3. 在存算分离模式中获取 Compute 拓扑，以一致性哈希或全局轮询把 Region 分派给 Compute 地址。
4. 为分区表把扁平 `RegionInfo` 转为按物理表 ID 分组的 `TableRegions`，并为失败重试重新合并、排序 range。
5. 用 `batchCopIterator` 并发执行初始任务和动态重试任务，通过有界通道向消费者返回响应，支持查询中断和显式关闭。

该文件不负责真实 RPC 编解码或 RegionCache 的具体实现：任务构建所需能力由 `BatchTaskSource` 注入，单任务执行由 `BatchTaskRunner` 注入。

## 主要符号

- 常量：`FETCH_TOPO_MAX_BACKOFF`、`MAX_BALANCE_SCORE`、`BALANCE_SCORE_THRESHOLD`、`MAX_REMOTE_READ_COUNT_PER_NODE_FOR_CLOSEST_REPLICAS` 和 `TI_FLASH_READ_TIMEOUT_ULTRA_LONG` 分别定义拓扑退避、均衡评分、跨 zone 读取上限和超长读超时；文件尾保留 Go 风格别名以兼容移植调用。
- `batchCopTask`（公开别名 `BatchCopTask`）：保存目标地址、`CommandType`、`RpcContext`、普通 Region 和分区表 Region。`region_count` 在两种表示之间取其一计数，正常构建流程保持二者不会同时承载同一批 Region。
- `batchCopResponse`（公开别名 `BatchCopResponse`）与 `CopRuntimeStats`：包装 protobuf 风格 `BatchResponse`、起始键、错误、耗时和退避统计；`mem_size` 首次调用后缓存估算值，因此响应内容一旦被改变，缓存不会自动失效。
- 均衡函数：`balance_batch_cop_task_with_continuity` 以固定大小连续块轮流分配；`balance_batch_cop_task` 先固定只有一个有效副本的 Region，再用连续性方案或候选数/当前负载加权的贪心方案处理多副本 Region。`check_batch_cop_task_balance`、`balance_score`、`prefer_contiguous_tasks` 决定是否接受或回退。
- 调度与副本策略：`DispatchPolicy` 选择 `RoundRobin` 或 `ConsistentHash`；`ReplicaReadPolicy` 选择全部副本、严格就近或自适应就近。`AliveStoresBundle` 同时保存全 zone 与本 zone 的 store 列表和 ID 集合。
- 存活与路由辅助：`get_alive_stores_and_store_ids`、`check_alive_store`、`filter_accessible_stores_and_build_region_info` 执行存活、同 zone 和远程读取上限判断；`get_tiflash_compute_rpc_context_by_*` 与 `build_compute_tasks` 负责 Compute 节点分派。
- `BatchTaskSource`：任务构建的依赖反转边界，提供 range 切分、拓扑、存活探测、Region RPC context、副本列表、全量 TiFlash store 和缓存失效。
- `BatchBuildOptions`：选择存算模式、autoscaler、MPP、TTL、均衡方式、调度策略、副本读取策略、zone、重试次数及测试模式。
- `build_batch_cop_tasks_for_non_partitioned_table`/`build_batch_cop_tasks_for_partitioned_table`：对外任务构建入口。RustCodeGraph 将非分区入口定位在第 1196 行，并确认其分别调用 `build_disaggregated_tasks` 与 `build_integrated_tasks`。
- `BatchTaskRunner`、`TaskRunResult` 与 `batchCopIterator`（别名 `BatchCopIterator`）：把实际发送器与并发响应迭代器解耦；`TaskRunResult.retry_tasks` 允许 worker 在本地队列继续处理重试任务。
- `handle_batch_cop_response`/`handle_streamed_batch_cop_response`：过滤 retry-region 响应、转换远端错误，并把取消或流错误映射成服务端超时语义。

## 执行流程

非分区表入口把单组 range 直接交给对应模式；分区表入口先校验 range 组数和 `partition_ids` 数量一致，构建任务后再调用 `convert_region_infos_to_partition_table_regions`。

存算分离路径 `build_disaggregated_tasks` 的流程是：`split_all_ranges` 按物理表切分 Region；从 autoscaler 的地址拓扑或静态 Compute store 列表获取候选；并发探测存活节点；空拓扑时按 `max_retries` 和 `Backoffer` 重试；最后由 `build_compute_tasks` 按策略生成 `RpcContext` 并按地址合并任务。`Invalid` 策略和空节点集直接返回错误。

存算一体路径 `build_integrated_tasks` 的流程是：切分 Region；逐 Region 获取主 RPC context 与所有有效 TiFlash 副本；缺少 context 时失效对应 Region 并退避重试；探测相关 store 的存活及 zone 分布；`check_alive_store` 判定是否需要失效并重取 Region；为每个 Region 过滤可访问副本，必要时生成跨 zone 警告或触发远程读取上限错误；先按原 RPC 地址聚合任务，再调用 `balance_batch_cop_task` 在实际相关的存活 store 间重新分配。

均衡时，只有一个有效 store 的 Region 被固定放置；多副本 Region进入候选集。候选达到 500 个且启用连续性时，`balance_batch_cop_task_with_continuity` 按 `balance_continuous_region_count` 大小成块选择，确保总数守恒后评分。达到 85 分即直接采用；否则执行加权贪心。贪心结果仍不平衡而连续性结果存在时，`prefer_contiguous_tasks` 选择连续性结果。

执行阶段由 `batchCopIterator::run` 为每个初始任务启动一个线程。线程调用 `BatchTaskRunner::run_task`，把 `retry_tasks` 追加到自己的 FIFO 队列，把不含 `retry_regions` 的响应写入容量 2048 的同步通道。`next` 每三秒醒来检查关闭和 killed 标记；通道断开代表正常结束，响应内错误或 killed 标记则返回错误。

## 数据与状态

- `RegionInfo.Region` 是版本化 Region ID，`Ranges` 是该 Region 内的 key range，`AllStores` 是可承载该 Region 的候选 store ID；均衡算法以 `RegionVerId` 去重。
- 普通任务使用 `regionInfos`；分区表转换后清空它并写入 `PartitionTableRegions`。`merge_task_ranges_for_retry` 能处理两种表示并保持 range 排序。
- 连续性均衡维护 `store_id -> Region 下标队列` 和 `selected: Vec<bool>`，保证一个候选 Region 只分配一次；最终再次比较分配总数以防遗漏。
- `ROUND_ROBIN_SEED` 是进程内全局 `AtomicUsize`，每批调度只取一次起点；一致性哈希则以 `"address-region_id"` 的 Murmur3 32 位值选择最大得分地址，节点输入顺序不影响结果。
- `batchCopIterator` 的 `started` 只允许启动一次；`finish` 控制所有 worker 停止；`killed` 暴露给外部设置查询中断；`workers` 保存 join handle，保证关闭时回收线程。
- `CopRuntimeStats` 随一个 `TaskRunResult` 的所有响应克隆，包含退避聚合和被调用地址；这里不负责填充这些统计，只负责携带。

## 依赖与调用关系

crate 内依赖集中在 `batch_request_sender.rs` 的 `Backoffer`、`BatchError`、`BatchResponse`、`RegionInfo`、`RpcContext`、`Store` 和 `TableRegions`。`KeyRanges::new(...).into_sorted()` 用于重试前恢复单调 range。`pool_task_details::PoolTaskDetails` 是响应统计中的可选读池明细。

上游方面，`pkg/store/copr/mpp.rs` 直接调用两个 `build_batch_cop_tasks_for_*` 入口；`pkg/store/copr/lib.rs` 公开再导出所有符号。`pkg/store/copr/store.rs` 的 `get_mpp_client` 将 RegionCache 转为 `Arc<dyn BatchTaskSource>`，形成真实元数据来源。普通 Cop 后端在 `pkg/store/copr/coprocessor.rs` 暴露 `build_batch_iterator` 扩展点并以 `BatchCopIterator` 为返回类型。

下游方面，构建逻辑只依赖 trait，不直接绑定网络实现；真实 transport 位于相邻 `network_backend.rs`/`batch_request_sender.rs`。`Cargo.toml` 表明本 crate 依赖带固定 tag 的 `tikv-client v0.4.2-aster.10`、`kvproto`、`grpcio`、`protobuf`、Tokio，以及多个工作区 crate；本文件自身使用的是标准线程和 `std::sync::mpsc`，不是 Tokio task。

RustCodeGraph 的路径过滤没有匹配该文件，但精确符号查询成功定位 `build_batch_cop_tasks_for_non_partitioned_table`、`balance_batch_cop_task` 和 Rust/Go 两份 `batchCopIterator`，并从 node 调用轨迹确认非分区入口到两种内部构建器的边；其余调用关系用上述源码入口交叉核验。

## 错误处理与边界

- 均衡算法把异常或无法证明安全的输入回退为 `original_tasks`：例如某 Region 没有存活候选、同一 store 出现重复 Region、候选无法继续分配或总数不守恒。这是保守退化，不返回错误。
- 空 Compute 节点集返回 `BatchError::NoAliveStore`；无效调度策略返回 `InvalidDispatchPolicy`；Region/context 数量不一致和分区索引非法返回 `OtherResponse`。
- `build_integrated_tasks` 在 Region 无 TiFlash peer 或存活判定要求刷新时调用 `invalidate_region`，随后退避；超过 `max_retries` 才返回 `NoAliveStore`。未配置 `tidb_zone` 会强制退化为 `AllReplicas`。
- `ClosestReplicas` 没有本 zone 副本时会记录跨 zone Region；数量超过“本 zone 存活节点数 × 3”即 `RemoteReadLimit`。`ClosestAdaptive` 可直接选择其他 zone 的存活副本而不会使用严格上限分支。
- 分区入口严格要求 range 组数等于物理表 ID 数；`PartitionIndex` 必须非负且在数组范围内。
- `handle_batch_cop_response` 优先传播 `other_error`，含 `retry_regions` 的响应不交给消费者；流中的取消和其他传输错误均以 `ServerTimeout` 结束，后者会先尝试一次 backoff。
- `send_iterator_response` 在有界通道满时 yield 并重试，关闭或接收端断开后停止；调用方应避免一个长期不消费、又不关闭的迭代器占用 worker。

## 并发与资源生命周期

存活探测通过 `thread::scope` 为每个地址启动作用域线程，并以无界 channel 汇总存活下标；结束后排序下标，从而保持原输入顺序。闭包要求 `Sync`，作用域保证地址借用在线程退出前有效。

迭代器使用容量 2048 的 `sync_channel` 提供背压。`finish` 使用 Acquire/AcqRel，确保关闭信号在线程间可见；`killed` 用 Acquire 读取，外部通过 `killed_signal` 共享原子标记。`run` 取走初始任务并为每项创建 OS 线程；每个 worker 串行处理自身及其返回的重试队列，不会跨 worker 迁移重试任务。

`close` 幂等设置 `finish`、丢弃持有的 sender，并 join 所有 worker；`Drop` 无条件调用 `close`，避免遗留线程。需要注意：worker 在 `BatchTaskRunner::run_task` 内阻塞时，`finish` 不能强制取消该调用，关闭会等它返回；真正的网络取消需由 runner/transport 自己配合。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/batch_coprocessor.go`。Rust 保留了 Go 的核心名字和阶段：`batchCopTask`/`batchCopResponse`、连续性与贪心均衡、存算分离的一致性哈希/轮询、存活与 zone 策略、分区 Region 转换，以及 `batchCopIterator` 生命周期；文件尾的驼峰函数/常量进一步为移植代码保留 Go 风格入口。

Rust 的 `BatchTaskSource` 和 `BatchTaskRunner` 把 Go 中直接依赖 `kvStore`、`RegionCache`、RPC client 和上下文的部分抽象为 trait，便于独立测试。所有权也替代了 Go 的指针/切片别名：例如 `deep_copy_store_task_map` 明确克隆 Region，Rust 独立测试验证副本不会反向修改原任务。

两者并非逐行等价。Go 文件包含 context 取消、failpoint、日志/metrics、详细 RPC 重试、锁/Region 错误处理和 execution info 收集；Rust 本文件只提供构建算法、响应转换与通用 runner 迭代框架，实际发送和部分统计位于其他模块或 trait 实现中。Rust 的流错误映射和 worker 数量也应以当前 Rust 源码为准，不能从 Go 实现推断未接线能力。

独立 Rust 测试 `batch_coprocessor_test.rs` 前半保存 Go 对照材料，实际 `#[test]` 位于后半，验证了大规模连续性均衡及小集合退化、空任务、贪心回退、深拷贝、一致性哈希与输入顺序无关、轮询均衡、退避耗尽、store ID 过滤及副本存活判定。它没有直接覆盖任务构建两个主入口、分区转换、迭代器关闭/中断和流式错误映射；这些不能声称已由该测试证明。

## 扩展指南

- 新增任务构建所需元数据时，优先扩展 `BatchTaskSource` 及其独立实现，同时补 `batch_coprocessor_test.rs` 中的 mock/单元测试；不要把具体 RegionCache 或网络 client 塞回本文件。
- 新增调度策略时，同时修改 `DispatchPolicy`、`build_compute_tasks` 的穷举分支、Go 对照策略映射及哈希/负载分布测试；必须定义空拓扑、节点顺序变化和节点增减时的行为。
- 修改均衡算法时维持三项不变量：每个 Region 恰好一次、只分配到 `AllStores` 允许且存活的 store、总 Region 数守恒。同步扩展连续性、单副本、多副本、重复候选和无法分配的独立测试，并评估 10 万 Region 级别的时间和内存。
- 修改副本读取策略时同步审查 `filter_all_stores_according_to_tiflash_replica_read`、`check_alive_store` 和 `filter_accessible_stores_and_build_region_info`，特别关注无本 zone 节点、部分副本死亡和远程读取上限的兼容行为。
- 修改分区表示时同步维护 `convert_region_infos_to_partition_table_regions` 与 `merge_task_ranges_for_retry`，测试负索引、越界、空分区及 range 顺序；Rust 测试必须继续放在独立的 `batch_coprocessor_test.rs`。
- 修改迭代器时应补并发测试：有界通道满、runner 阻塞、动态重试、错误传播、killed、重复 `run`/`close` 和 Drop。若引入真正取消，必须定义 runner 的取消契约，避免 `close` 永久等待。
- 对 Go 新变更做逐提交对齐时，只移植该提交增量和不可缺少的局部接线；日志、metrics、failpoint 或锁处理等现有差距应单列范围，不以简化桩冒充行为一致。

## 验证依据

- 目标源码：`pkg/store/copr/batch_coprocessor.rs`，人工枚举并检查其常量、类型、trait、公开入口、内部构建器、均衡算法和迭代器生命周期。
- crate 边界：`pkg/store/copr/Cargo.toml` 与 `pkg/store/copr/lib.rs`，确认 crate 名、入口、依赖、模块声明、公开再导出及独立测试装配。
- 上下游源码：`pkg/store/copr/mpp.rs`、`pkg/store/copr/store.rs`、`pkg/store/copr/coprocessor.rs`、`pkg/store/copr/batch_request_sender.rs` 和 `pkg/store/copr/network_backend.rs`，确认 MPP 调用入口、`BatchTaskSource` 实例来源、迭代器扩展点与传输边界。
- Go 对照：`pkg/store/copr/batch_coprocessor.go`，核对同名数据结构、均衡/路由/副本策略、任务构建和迭代执行阶段；差异按当前 Rust 实现保守描述。
- 独立测试：`pkg/store/copr/batch_coprocessor_test.rs`，人工检查实际 Rust `#[test]` 的覆盖范围，并明确记录尚未覆盖的主入口和并发生命周期。
- RustCodeGraph：`status` 显示本仓库索引包含 7032 个 Rust 文件；`query build_batch_cop_tasks_for_non_partitioned_table --kind function` 定位到目标文件第 1196 行；`node` 展示其源码及到 `build_disaggregated_tasks`、`build_integrated_tasks` 的调用边；`query balance_batch_cop_task` 同时定位两个生产函数和对应 Rust 测试；`query batchCopIterator` 同时定位 Rust 与 Go 实现。路径 `files --filter` 未命中，因此未把路径检索当作证据。
- 本任务按计划只做文档事实与结构验证，不运行 Cargo；结构命令要求目标文件存在且恰有十一个固定二级标题。
