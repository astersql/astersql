# `pkg/store/copr/coprocessor_cache.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate。crate 由 [`pkg/store/copr/Cargo.toml`](Cargo.toml) 定义，以 [`pkg/store/copr/lib.rs`](lib.rs) 为入口；`lib.rs` 将 `coprocessor_cache` 声明为公开模块并重新导出其公开项。它位于 TiDB/AsterSQL 到 TiKV 的 coprocessor 请求链上：[`Store::new`](store.rs) 创建可选缓存并以 `Arc<CoprocessorCache>` 共享给 [`CopClient`](coprocessor.rs)，[`CopTaskWorker`](coprocessor.rs) 在发送任务前查缓存、在收到可缓存响应后写缓存。

该文件实现的是进程内响应缓存及其键编码、准入和容量策略，不负责 Region 路由、RPC、重试或 TiKV 端缓存版本判定。当前 Rust 调用链与 Go 版本在命中校验上并不等价，具体差异见“与 Go 版本的对应关系”。

## 核心职责

1. 用 `CoprocessorCacheConfig` 把 MB 和毫秒配置转换为缓存内部使用的字节数与 `Duration`，并用 `Option` 表达关闭状态。
2. 用 `coprocessor_cache_build_key` 对请求类型、请求体、KeyRange 列表和“是否分页”进行确定性编码，使相同计算请求落到同一缓存键。
3. 用 `check_request_admission` 和 `check_response_admission` 过滤不值得缓存的请求/响应：range 太多、结果为空或过大、计算时间过短、分页任务序号过高时拒绝。
4. 用 `Mutex<CacheState>` 保护哈希表、FIFO 插入队列、容量记账和驱逐计数，在容量不足时同步驱逐旧条目。
5. 用 `CoprocessorCacheValue` 保存响应数据以及时间戳、Region、Region 数据版本和分页边界等元数据，供上层请求链使用。

## 主要符号

- `CoprocessorCacheConfig`：公开配置。`capacity_mb == 0.0` 表示关闭；`admission_max_ranges == 0` 表示不限制 range 数；另外两个字段分别限制单项结果大小和最短处理时间。
- `CoprocessorCacheRequest`：只包含构键所需的摘要。`request_type` 最终必须装入 1 字节，`data` 长度必须装入 `u32`，每个 range 的起止键长度必须分别装入 `u16`。
- `CoprocessorCacheValue`：公开缓存值。`len()` 以 `size_of::<Self>()` 加四个 `Vec<u8>` 当前长度估算成本；`Display` 输出时间戳、Region、数据版本和载荷长度。`is_empty()` 实际上恒为假，因为结构体本身占用非零空间，它主要是与常见容器 API 对齐。
- `CacheState`：私有可变状态，含 `entries: HashMap`、`insertion_order: VecDeque`、当前 `cost` 和累计 `evictions`。
- `CoprocessorCache`：公开线程安全缓存。配置阈值在构造后不可变，只有 `state` 需要互斥保护。
- `CoprocessorCache::new`：返回 `BatchResult<Option<Self>>`；关闭返回 `Ok(None)`，非法的有效容量或最大结果大小返回 `BatchError::OtherResponse`。
- `get`：锁住状态、从哈希表查找、再次检查值内保存的 `key`，并克隆整个值返回。
- `check_request_admission` / `check_response_admission`：纯读取准入阈值，不获取互斥锁。
- `set`：强制把 `value.key` 设为入参键，计算成本，覆盖同键旧值，然后按 FIFO 驱逐并插入。
- `evictions`：读取累计实际驱逐条目数。
- `coprocessor_cache_build_key`：公开键编码函数，错误类型沿用 `batch_request_sender::BatchError`。
- `coprCache` / `coprCacheValue`：面向 Go 命名迁移的公开类型别名，不增加新行为。

## 执行流程

### 初始化与共享

1. [`Store::new`](store.rs) 把配置交给 `CoprocessorCache::new`。
2. 容量为零时 Store 保存 `None`；启用时构造缓存并包装为 `Arc`，保存在 `Mutex<Option<Arc<_>>>` 中。
3. [`Store::get_client`](store.rs) 克隆该 `Arc` 给 `CopClient`；后者继续传入 `CopIterator` 和 `CopTaskWorker`，因此多个 worker 共享同一个缓存实例。
4. [`Store::close`](store.rs) 从 owner 槽中取走 `Arc`。已被 client/worker 克隆的引用仍按 Rust 引用计数生命周期存活，最后一个 `Arc` 释放时自动销毁缓存。

### 请求查找

1. [`response_cache_key`](coprocessor.rs) 将 `CopRequest` 的类型、data、当前任务 ranges 和分页参数转换为 `CoprocessorCacheRequest`，再调用 `coprocessor_cache_build_key`。
2. 构键按顺序写入：1 字节请求类型、4 字节小端 data 长度、data 原文；每个 range 依次写入 2 字节小端 start 长度、start、2 字节小端 end 长度、end。
3. `paging_size` 或 `paging_size_bytes` 任一非零时只追加一个 `1` 标记。具体分页大小不进入键，所以所有分页粒度共享一个键空间，但与非分页请求隔离。
4. [`CopTaskWorker`](coprocessor.rs) 在缓存存在且 `check_request_admission(task.ranges.len())` 通过时调用 `get`。命中后当前 Rust 实现直接以缓存的 `data` 和 `page_start` 构造响应并跳过后端请求。

### 响应写入与驱逐

1. 后端响应成功后，[`CopTaskWorker`](coprocessor.rs) 要求 `response.can_be_cached` 为真，并用数据长度、RPC/任务耗时和分页任务序号调用 `check_response_admission`。
2. 响应为空、超过最大大小或 `paging_task_index > 50` 时拒绝；首页要求达到完整最短处理时间，后续分页任务的门槛是该时间的三分之一。
3. 上层构造 `CoprocessorCacheValue`，记录响应数据、请求 `start_ts`、Region ID、响应的 `cache_last_version` 以及响应 range 的首尾键，再调用 `set`。
4. `set` 先覆盖值内的 key。若单项估算成本大于总容量，立即返回 `false` 且不改状态。
5. 同键更新时先删除旧映射、扣除旧成本并从插入队列移除所有同键记录。空间不足时从队首逐项删除并增加 `evictions`，直到新项可放入；最后追加新键、写入哈希表并返回 `true`。

## 数据与状态

- 缓存键不包含 `start_ts`、Region ID、Region epoch/data version，也不包含分页大小的具体数值。它只描述计算形状、ranges 及分页/非分页类别。
- `CoprocessorCacheValue.timestamp`、`region_id` 和 `region_data_version` 被写入并公开，但本文件的 `get` 不解释或校验这些字段；是否有效必须由调用链负责。当前 Rust 调用链尚未完成与 Go 相同的 Region、时间戳和 TiKV 版本确认。
- `page_start` 与 `page_end` 保存分页响应覆盖的范围。当前 Rust 命中分支使用 `page_start` 作为 `CopResponse.start_key`，未在该分支恢复 `page_end`；Go 对照会恢复完整 response range。
- `cost` 是估算值，不是分配器的精确堆内存统计：它包含结构体静态大小和四段字节内容，但不计 `Vec` 多余 capacity、哈希表/队列桶、克隆键以及分配器开销。
- 驱逐顺序是插入 FIFO，不是访问 LRU。`get` 不调整 `insertion_order`；更新同键会把它移到队尾。
- `evictions` 只统计容量驱逐成功从 `entries` 删除的条目；同键覆盖和缓存最终销毁不计入。

## 依赖与调用关系

上游调用边（由 RustCodeGraph 文件索引及精确 `rg` 交叉核对）：

- [`Store::new`](store.rs) → `CoprocessorCache::new`：创建可选缓存。
- [`Store::get_client`](store.rs) → `CopClient::new`：把共享缓存传入 coprocessor 客户端。
- [`response_cache_key`](coprocessor.rs) → `coprocessor_cache_build_key`：把业务请求/任务适配为缓存摘要。
- [`CopTaskWorker`](coprocessor.rs) → `check_request_admission` → `get`：请求发送前的本地命中路径。
- [`CopTaskWorker`](coprocessor.rs) → `check_response_admission` → `set`：成功响应后的写入路径。

下游依赖：

- 标准库 `HashMap`/`VecDeque` 实现索引与 FIFO，`Mutex` 实现跨 worker 同步，`Duration` 表示时间阈值。
- [`batch_request_sender::KeyRange`](batch_request_sender.rs) 提供 range 字节边界；`BatchResult`/`BatchError` 统一 crate 内的错误返回。
- `Cargo.toml` 将本文件编入 `astersql-store-copr` 库；缓存实现本身没有直接使用外部缓存 crate，尽管整个 crate 还依赖 TiKV client、kvproto、tokio 等请求链依赖。

## 错误处理与边界

- `capacity_mb == 0.0` 是关闭而非错误。非零值转换成 `usize` 后为零（包括负值或小于 1 字节的正值）返回 `"Capacity must be > 0 to enable the cache"`。
- 启用缓存时，`admission_max_result_mb` 转换后为零会返回 `"AdmissionMaxResultMB must be > 0 to enable the cache"`。浮点到整数使用 Rust 的 `as` 语义；这里没有显式拒绝 NaN、无穷或超过 `usize` 表示范围的配置，扩展配置校验时应补充针对这些输入的明确契约。
- 构键拒绝超过 `u8` 的请求类型、超过 `u32` 的 data 长度，以及超过 `u16` 的单个 range 起止键长度，并返回对应 `OtherResponse`。长度转换发生在已估算 `Vec` 容量之后；极端输入还可能先触及总容量求和/分配限制。
- `set` 对单项大于容量返回 `false`。调用方当前忽略此返回值，因此“后端响应成功”不意味着一定成功缓存。
- 所有状态锁都以 `expect("coprocessor cache lock poisoned")` 获取；持锁线程 panic 会使后续访问也 panic，而不是返回可恢复错误。
- `state.cost + cost` 在极端 `usize` 边界可能溢出；正常配置下由可用内存和容量约束先行限制，但若强化不可信配置处理，应改为 checked/saturating 计算并增加独立测试。
- 本文件不验证缓存值的 Region、时间戳或数据版本；当前 Rust 上层也没有把 `region_data_version` 作为 `cache_if_match_version` 发给 TiKV，因此这是兼容性边界，不应把字段存在视为已实现缓存新鲜度协议。

## 并发与资源生命周期

- `CoprocessorCache` 可通过 `Arc` 在线程间共享；所有 `entries`、顺序队列、成本和驱逐计数的复合更新都在同一把 `std::sync::Mutex` 下完成，因此单次 `get`、`set`、`evictions` 对状态是串行的。
- `check_request_admission` 和 `check_response_admission` 只读构造时固定的阈值，不需要锁。
- `get` 在持锁期间完成查找与值克隆，返回独立的 `CoprocessorCacheValue`；调用者修改返回值不会改变缓存，但大响应的克隆会增加锁持有时间和内存带宽。
- `set` 在持锁前克隆 key 到 value 并计算成本，持锁后执行 `VecDeque::retain`、可能的多次驱逐和哈希插入。同键更新的 `retain` 是线性扫描，容量压力下驱逐也会延长独占区间。
- 实现没有后台任务、通道或显式 `close`。资源由 `HashMap`/`VecDeque` 和 `Arc` 的 RAII 自动释放；与 Go Ristretto 需要 `Close` 的生命周期不同。
- `Store::close` 只释放 Store 持有的缓存引用，不会强制终止仍持有 `Arc` 的 worker，也不会清空底层 KV store。

## 与 Go 版本的对应关系

直接对照文件是 [`coprocessor_cache.go`](coprocessor_cache.go)，行为测试是 [`coprocessor_cache_test.go`](coprocessor_cache_test.go)，Rust 独立测试位于 [`coprocessor_cache_test.rs`](coprocessor_cache_test.rs)。

已对齐部分：

- 配置字段、缓存值元数据、`Display/String` 格式、成本估算的设计意图一致。
- 关闭语义、容量/最大项校验错误文本、range 与响应准入边界、分页后续页三分之一耗时门槛和分页任务上限 50 一致。
- 构键字节布局一致：小端长度前缀，分页仅追加单一 marker，不编码分页具体数值。
- `set` 都会强制值内 key 与调用 key 一致，`get` 都会在取值后复核完整 key，以防只依赖哈希命中。

明确差异：

- Go 使用 Ristretto（带准入/频率策略、异步写入和回调指标）；Rust 使用同步 `HashMap + FIFO`，所以命中时序、淘汰选择、指标和性能特征不等价。
- Go 的方法允许 nil receiver 并返回 false/nil；Rust 用 `Option<Arc<CoprocessorCache>>` 在调用处表达关闭，不能对不存在的缓存调用方法。
- Go 请求链只在 `CmdCop`、请求标记 `Cacheable` 且请求准入通过时构键；命中候选还检查 `RegionID` 与 `TimeStamp <= StartTs`，然后把 `RegionDataVersion` 作为 `CacheIfMatchVersion` 发给 TiKV，由 `IsCacheHit` 响应确认有效。当前 Rust 请求链仅检查缓存存在和 range 数，随后本地命中即直接返回，没有上述命令类型、cacheable、Region、时间戳及服务端版本确认。
- Go 只在 `CacheLastVersion > 0`、有处理耗时详情且响应准入通过时更新缓存；当前 Rust 检查 `can_be_cached` 和响应准入，但不要求版本大于零。
- Go 分页命中恢复 `PageStart` 与 `PageEnd` 到 response range；当前 Rust 命中分支只把 `page_start` 放入 `CopResponse.start_key`。
- Rust 增加了同步 FIFO 驱逐计数接口 `evictions()`，但尚未接入 Go 的 `CoprCacheCounterEvict/Hit/Miss` 指标。

因此，本文件是真实可执行缓存，不是桩；但其 Go 缓存协议移植仍不完整。后续补齐时应限定在相关调用链接线和独立测试中，不能仅修改此文件后宣称协议已对齐。

## 扩展指南

- 调整键格式：修改 `coprocessor_cache_build_key`，同时更新 `CoprocessorCacheRequest` 适配和 `build_cache_key_matches_go_byte_layout`。键格式影响命中隔离，必须说明兼容策略；尤其不能无意把分页与非分页合并，或让 range 边界产生歧义。
- 调整准入策略：修改相应 `check_*_admission`，在 `request_and_response_admission_matches_go_boundaries` 增加等于阈值、阈值前后、首页/后续页及 50/51 的边界用例；若偏离 Go，需明确记录理由。
- 调整容量/驱逐：修改 `CacheState` 与 `set`，增加小容量下多项驱逐、同键覆盖成本回收、超大单项拒绝和 `evictions` 计数测试。不要把测试嵌入生产文件，应继续放在独立的 `coprocessor_cache_test.rs`。
- 补齐 Go 缓存有效性协议：主要接入点在 `coprocessor.rs` 的 `response_cache_key` 和 `CopTaskWorker` 请求/响应处理，而非只在本文件。测试应覆盖 cacheable/命令类型、Region 不匹配、未来时间戳、版本匹配/失配、`cache_last_version == 0` 以及分页完整 range 回填。
- 增加指标：应在命中、未命中和实际驱逐的确定位置接入 `astersql-store-copr-metrics`，并评估可选依赖/feature 边界，避免让核心缓存无条件依赖可选 metrics crate。
- 优化并发：若替换全局互斥锁或 FIFO，需要保持 `entries`、成本和顺序结构的原子不变量，并用并发独立测试或模型测试证明没有重复键、成本漂移和锁顺序问题。
- 修改成本估算：同步更新 `CoprocessorCacheValue::len` 和 `cache_value_len_counts_struct_and_owned_bytes`；明确它是策略成本还是精确内存占用，避免两种含义混用。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/store/copr` 确认目标源、Go 对照和独立测试均已索引；`query CoprocessorCache` 定位 `CoprocessorCacheConfig`、`CoprocessorCacheRequest`、`CoprocessorCacheValue`、`CoprocessorCache`、`new/get/set` 和构键函数；`node --file pkg/store/copr/coprocessor_cache.rs --offset 1 --limit 400` 读取了目标文件全部 276 行。精确 `callers/callees` 对常见方法名出现跨仓库同名歧义且部分查询超时，因此调用边改用以下精确源码搜索核验。
- 生产源码：[`coprocessor_cache.rs`](coprocessor_cache.rs)（全部符号与内部算法）、[`coprocessor.rs`](coprocessor.rs)（`response_cache_key`、`CopTaskWorker` 的查找/写入路径）、[`store.rs`](store.rs)（构造、共享和关闭生命周期）、[`lib.rs`](lib.rs)（模块声明和 re-export）。本目录没有 `doc.go`/`doc.rs` 包契约文件。
- crate 配置：[`Cargo.toml`](Cargo.toml) 确认 crate 名、库入口、Go 包映射和依赖边界。
- Go 对照：[`coprocessor_cache.go`](coprocessor_cache.go)（Ristretto 实现）以及 [`coprocessor.go`](coprocessor.go) 的 `buildCacheKey`/`handleCopCache`（Region、时间戳、TiKV 版本确认和分页回填）。
- 测试证据：[`coprocessor_cache_test.rs`](coprocessor_cache_test.rs) 覆盖键布局、配置、准入边界、长度、显示格式、实际 get/set 和负容量；[`coprocessor_cache_test.go`](coprocessor_cache_test.go) 提供原始 Go 边界和分页回填语义。Rust 测试文件前半还保留 `GO_REFERENCE` 文本，但真正可执行的 Rust 测试从 `config()` 及其后的 `#[test]` 开始。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节、链接/范围和事实可追溯性。
