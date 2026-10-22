# `pkg/store/copr/region_cache.rs`

## 文件定位

为普通 Cop 构建可合批任务，并实现 `BatchTaskSource` 供 batch cop/MPP 生成 TiFlash `RegionInfo`、查询拓扑与 store。

## 核心职责

- 定义 Region 定位结果 `KeyLocation`、bucket 元数据 `Buckets`、定位后范围组 `LocationKeyRanges`，统一半开区间 `[start, end)` 和空 `end = +∞` 的边界语义。
- 校验一组 location 是否有序、无重叠、完整覆盖输入 ranges 且不存在未被使用的多余 location（`check_locations_ordered`、`check_ranges_covered`、`validate_location_coverage`）。
- 先批量定位、再把逻辑 `KeyRanges` 切成每个 Region 的片段；预取 location 不足时以单键定位补齐，并以 `MAX_RELOCATE_ON_OVERFLOW` 防止异常元数据导致无限循环。
- 在 Region 拆分后按 bucket 边界继续细分；bucket 元数据乱序、range 起点越界或切分无进展时，整批回退到仅按 Region 拆分，保证 bucket 优化不改变正确性。
- 为普通 Cop 构建可合批任务，并实现 `BatchTaskSource` 供 batch cop/MPP 生成 TiFlash `RegionInfo`、查询拓扑与 store。
- 实现 `RegionFailureHandler`：仅对明确标记为 TiFlash 且带 Region 元数据的发送失败通知后端失效或重载。

## 主要符号

- `UNSPECIFIED_LIMIT = -1`：表示 Region 拆分数量不设上限。其他负 limit 不是该哨兵值，当前实现会立即返回空结果。
- `MAX_RELOCATE_ON_OVERFLOW = 64`：批量定位结果耗尽后的单键补定位预算，避免 livelock。
- `LOCATION_SUMMARY_MAX_DISPLAY = 5`：`location_coverage_summary` 最多展示的焦点附近 location 数量。
- `Buckets { version, keys }`：Region 内有序 bucket 边界；相邻键界定一个 bucket。`keys` 未在类型层强制排序。
- `KeyLocation { region, start_key, end_key, buckets, store, peer }`：Region 版本、边界和选中副本信息。`contains_start` 判断起点是否位于 `[start_key, end_key)`；`covers_end` 按 range 排他终点判断覆盖；`bucket_version` 在无 bucket 时返回 0。
- `LocationKeyRanges { location, ranges }`：把一个 location 与落入其中的逻辑范围绑定；`split_key_ranges_by_buckets` 返回拆分组及可选回退诊断。
- `BucketSplitFallbackInfo`：记录回退原因、相关 range、bucket 边界和剩余数量。当前原因包括 `bucket_boundaries_not_ordered`、`range_start_outside_location`、`bucket_not_contain_start_no_progress` 和 `bucket_split_no_progress`。
- `compare_key_range_boundary`：比较 start/end 边界；空 end 被提升为 `+∞`，空 start 仍是普通最小字节串。
- `check_locations_ordered`、`check_ranges_covered`、`validate_location_coverage`：覆盖性质检查；最终校验要求所有输入 range 被覆盖且每个 location 至少被一个 range 使用。
- `location_coverage_summary`：统计相邻 location 的 gap、overlap、contiguous 数并输出焦点附近 Region ID，当前只用于诊断证据构造。
- `RegionCacheBackend`：`Send + Sync + 'static` 的注入接口，隔离路由算法与真实网络/缓存实现。
- `RegionCache { backend: Arc<dyn RegionCacheBackend> }`：主要门面；公开 `locate_key`、`locate_end_key`、三种拆分、`build_batch_task`、失效与 bucket 更新。
- `RegionFailureHandler for RegionCache`：承接批量发送失败；`BatchTaskSource for RegionCache`：承接 batch cop/MPP 的 Region 切分和拓扑查询。
- `bucketSplitFallbackInfo`、`UnspecifiedLimit`、`NewRegionCache`：为 Go 移植代码保留的别名和命名入口。

## 执行流程

普通 Cop 任务构建的主链如下：

1. `coprocessor.rs::build_cop_tasks` 调用 `CopBackend::split_key_ranges`；生产适配器 `store.rs::StoreCopBackend` 根据 `skip_buckets` 选择 `RegionCache::split_key_ranges_by_locations` 或 `split_key_ranges_by_buckets`。
2. `split_key_ranges_by_locations` 先调用 `backend.batch_locate_key_ranges`，把 `need_leader` 与 `buckets` 需求传给后端。空 ranges 或 `limit == 0` 直接返回空。
3. 每个原始逻辑 range 都把 location 搜索下标重置为 0。这一点用于支持 `[a,z)` 后再出现被其包含的 `[b,c)` 等非单调剩余队列，保持 Go 行为。
4. 算法跳过结束键不大于当前起点的 location，确认 location 包含当前起点，并取 `min(range.end, location.end_key)` 生成片段。相邻片段属于同一 `RegionVerId` 时合并进同一个 `LocationKeyRanges`。
5. 预取 locations 耗尽而仍有范围时，使用 `backend.locate_key(remaining.start)` 追加 location；超过 64 次返回错误。任何非空片段满足 `start >= end` 也返回错误，避免空转。
6. 若启用 bucket，`split_key_ranges_by_buckets` 先取得带 bucket 的 Region 分组，然后每组由 `LocationKeyRanges::split_key_ranges_by_buckets` 沿首个大于当前起点的 bucket 边界切分，并按 bucket index 聚合片段。
7. 任一组出现 fallback 时，不返回部分 bucket 结果，而是重新调用 `split_key_ranges_by_locations(..., buckets=false)` 对原始整批 ranges 做 Region-only 拆分。覆盖校验和 `range_issues_for_key_ranges` 在该分支生成诊断事实，但当前 Rust 实现没有把结果写日志。
8. `build_cop_tasks` 把定位结果转成 `CopTask`；满足合批条件时经 `StoreCopBackend::build_batch_task` 调用本文件 `build_batch_task`。只有 Leader read、有 RPC context/store 且 `estimated_wait_ms <= request.store_busy_threshold` 时才返回 `BatchedCopTask`。

Batch cop/MPP 链路中，`batch_coprocessor.rs::split_all_ranges` 调用 `BatchTaskSource::split_key_ranges`。本文件先做 Region-only 拆分，再为每个 Region 查询 TiFlash context，填充 `RegionInfo` 的版本、peer、ranges、有效 store ID 与 partition index。拓扑、计算节点、探活、RPC context 和失效操作均委托后端。

发送失败链路中，`batch_request_sender.rs::on_send_fail_for_batch_regions` 在非存算分离模式调用 `RegionFailureHandler`。本文件拒绝空 store 和非 TiFlash store，跳过无 `Meta` 的 Region，再逐项调用 `backend.on_send_fail_tiflash`。

## 数据与状态

`RegionCache` 自身只有一个 `Arc<dyn RegionCacheBackend>`，没有内部 Region map、锁或后台任务。真实缓存、PD 客户端和连接生命周期属于后端。门面可廉价共享，所有后端调用都通过 `&self` 完成；后端必须自行保证并发安全。

键范围采用字节序半开区间。空 `start` 是键空间开头；空 `end` 表示正无穷。`KeyLocation::covers_end` 因 range 终点是排他的，允许 `key == location.end_key`；但空 range end 只有空 location end 才覆盖。修改任一比较时必须整体维持这些约定。

`RegionVerId` 同时携带 region ID、版本和 conf version，是合并片段、失效缓存和更新 bucket 的身份键。`Buckets.version` 传播到 `CopTask.bucket_version`，用于服务端/响应侧识别 stale bucket。`store`、`peer` 均为可选，定位存在不等于已经得到可发送副本。

拆分主要通过拥有的 `Vec<u8>` 和 `KeyRanges` 克隆数据。Region 拆分会为每个片段复制边界；bucket 拆分会克隆 location 和片段。`limit` 限制的是已生成的 `LocationKeyRanges` 组数，不是原始 range 数或字节数。

## 依赖与调用关系

直接下游依赖：

- `batch_request_sender.rs` 提供 `KeyRange`、`KeyRanges`、Region/store/peer/RPC 数据结构、`BatchError`/`BatchResult` 和 `RegionFailureHandler`。
- `coprocessor.rs` 提供 `CopRequest`、`CopTask`、`BatchedCopTask` 与 `ReplicaReadType`。
- `batch_coprocessor.rs` 提供 `BatchTaskSource` 和 `ReplicaReadPolicy`；后者目前仅由 `_assert_replica_policy` 保留编译期引用。
- `range_diagnostics.rs::range_issues_for_key_ranges` 在 bucket fallback 时分析输入范围问题。
- `std::sync::Arc` 共享后端，`Duration` 表达 store busy threshold 与探活 TTL。

直接上游调用：

- `store.rs::StoreCopBackend::{send_tiflash_batch, split_key_ranges, build_batch_task, invalidate_region, update_buckets}` 将该门面接入普通 Cop 与 TiFlash 发送路径。
- `coprocessor.rs::build_cop_tasks` 经 `CopBackend` 消费拆分结果并在满足条件时构建 store 合批任务。
- `batch_coprocessor.rs::split_all_ranges` 经 `BatchTaskSource` 获取 batch cop/MPP 的 `RegionInfo`。
- `batch_request_sender.rs::RegionBatchRequestSender::on_send_fail_for_batch_regions` 经 `RegionFailureHandler` 触发 TiFlash Region 失败处理，然后执行 backoff。
- crate 外的 `pkg/store/driver/kv_adapter.rs` 和 `pkg/server/handler/tikv_handler.rs` 直接使用 `locate_key`/`locate_end_key` 完成扫描或诊断路由；`lib.rs` 的公开重导出使这些调用无需访问私有模块细节。

RustCodeGraph 报告目标文件被 34 个文件使用，并确认 `locate_key` 的直接调用者包含 `kv_adapter.rs::locate_scan_region` 与 TiKV handler，`invalidate_region` 的调用者包含 coprocessor 失败重建路径和 `store.rs`。对索引未能精确解析的 impl 方法调用，本文同时用定向源码调用点核验。

## 错误处理与边界

- 后端定位、RPC context 和拓扑错误原样以 `BatchResult` 向上传播；本文件不吞掉这些错误。
- `split_key_ranges_by_locations` 对空输入和 limit 0 返回空；`UNSPECIFIED_LIMIT` 以外的负 limit 也返回空，这是 Rust 测试明确固定的 Go 对齐行为。
- 预取 location 耗尽时最多补定位 64 次；预算耗尽返回 `BatchError::OtherResponse`。location 不包含剩余起点、片段无法前进也返回明确错误，而不是 panic 或循环。
- bucket 无数据时不细分；边界非严格递增、起点越界或无进展时返回原组与 `BucketSplitFallbackInfo`。上层看到任一 fallback 都丢弃已产生的 bucket 分片并对原始输入做完整 Region-only 重算。
- `build_batch_task` 的“不能合批”均用 `Ok(None)` 表示：非 Leader read、RPC context 缺失、store 缺失或 store 预计等待超过请求阈值。标签缺失或 `estimated_wait_ms` 解析失败按 0 处理。
- 发送失败处理对 `store=None`、非 TiFlash 或 `RegionInfo.Meta=None` 静默跳过；只有 `engine == "tiflash"` 的精确标签才触发后端。
- `split_key_ranges_by_buckets` 当前计算覆盖与 range 诊断但不输出；相比 Go 版本，Rust 没有 panic recover/重新查询 PD/结构化日志链。不能把 Go 的这些诊断能力写成 Rust 已支持。
- `expect("bucket group was inserted")` 和 `expect("filtered metadata")` 依赖同一局部分支刚建立的不变量；正常外部输入错误应在此前回退或过滤。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或定时器。`RegionCacheBackend: Send + Sync + 'static` 与 `Arc` 允许同一门面被多个请求线程共享；缓存一致性、网络连接、PD 重试和内部锁均由具体后端负责。

拆分过程只使用函数局部的 `locations`、`result`、`remaining` 和计数器，没有跨调用可变状态。返回值拥有自己的键和 location 克隆，因此不同任务之间不会通过本文件共享可变 range 缓冲区，代价是拆分路径存在分配与复制。

失败后的缓存状态变化是同步委托：`invalidate_region`、`update_buckets`、`on_send_fail_tiflash` 调用返回即结束本文件职责；是否异步重载由后端的 `schedule_reload` 语义决定。`Duration` 只作为阈值/TTL 参数，本文件不持有计时资源。

## 与 Go 版本的对应关系

当前实现差异必须明确：

- Go `RegionCache` 嵌入 `*tikv.RegionCache`，Rust 只持有 trait object，真实缓存位于后端。
- Go 使用 `KeyRanges` 的 `first/mid/last` 队列式切片；Rust 对每个原始 range 从 locations 起点重新搜索，直接构造平坦 `Vec`，从而处理 Go 测试揭示的非单调剩余队列。
- Go bucket panic 分支会 recover、查询 PD、记录缓存/PD/范围诊断后重新 panic；Rust 没有 failpoint 或 panic 诊断流程，只对可预见的异常元数据执行返回式 fallback。
- Go fallback 输出丰富结构化日志并查询 PD；Rust 当前仅构造 `BucketSplitFallbackInfo`、覆盖布尔值和 range issue 统计，随后静默 Region-only 重算。
- Go 从 client-go store 读取 `EstimatedWaitTime()`；Rust 从 store 标签 `estimated_wait_ms` 解析毫秒。缺失或非法标签被视为零。
- Go `OnSendFailForBatchRegions` 通过 `store.IsTiFlash()`；Rust 依赖精确的 `engine=tiflash` 标签。

Rust 测试顶部的 `GO_REFERENCE` 大段内容只是注释参考，不是已执行测试；文档不能将其中 panic/failpoint 场景算作 Rust 验证结果。

## 扩展指南

- 修改 Region 边界规则时，同时检查 `KeyLocation::{contains_start,covers_end}`、`compare_key_range_boundary`、三项 coverage 校验和两级拆分。回归测试放在独立 `region_cache_test.rs`，并对照 Go 的空 start/end、gap、overlap、unused location 与包含 ranges 矩阵。
- 新增 backend 能力应先扩展 `RegionCacheBackend`，再在 `network_backend.rs` 实现并由 `store.rs` 接线；若属于 batch cop/MPP 公共能力，还需同步 `BatchTaskSource` 及其测试替身。trait 新方法会影响所有 mock 实现。
- 修改 bucket 策略时保持“任何异常整批 Region-only 重算”这一正确性边界，不能返回此前已经生成的部分 bucket 结果。新增 fallback 原因应同步 `BucketSplitFallbackInfo`、Rust 回归测试和 Go 对照说明。
- 调整补定位策略时保留有界重试和无进展检测，并测试批量结果缺失、重复返回不覆盖起点的 location、无界尾 Region 以及非单调 ranges；否则容易产生 livelock。
- 改变合批判定时同时核对 `coprocessor.rs::build_cop_tasks` 的 `may_batch` 条件、store busy threshold 来源、Leader-only 约束和 `append_batched_task` 的按 store 分组行为。
- 扩展 TiFlash 失败处理时同步检查 `batch_request_sender.rs` 的 cancelled/shutdown/disaggregated 分支，明确 store 标签缺失与 `Meta=None` 的策略，避免把 TiKV 错误误送到 TiFlash 回调。
- 若补齐 Go 的诊断能力，优先把日志/PD 查询放在明确的后端或诊断接口中，避免让纯拆分算法直接持有网络资源；同时为 panic 与非 panic fallback 分开测试。
- 性能优化应重点测量 location/bucket 克隆、每个 range 重启搜索的复杂度以及大批键边界复制；任何索引优化都必须保持重叠/包含 ranges 的 Go 语义。
