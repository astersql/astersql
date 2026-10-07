# `br/pkg/restore/split/client.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-split`（见 `br/pkg/restore/split/Cargo.toml`），由 `lib.rs` 以 `pub mod client` 装配并通过 `pub use client::*` 暴露。它位于 BR 恢复链路的 Region 准备层：上层 `RegionSplitter` 在导入数据前调用 `SplitClient`，本文件把“按 key 查找 Region、请求分裂、等待新 Region 可用、发起 scatter、等待 scatter 完成”组织成一个接口和默认实现。它不负责 SST 导入，也不直接定义 Region 分组与分页扫描算法；后两者来自同 crate 的 `split.rs`。

RustCodeGraph 将该文件识别为 116 个符号，并显示它被 `br/pkg/restore/split/split_test.rs`、恢复日志/快照测试及 BR 测试桩等文件使用。生产侧最直接的入口证据是 `br/pkg/restore/split/split.rs`：`RegionSplitter::splitKeys` 调用 `SplitKeysAndScatter`/`SplitKeys`，`WaitForScatterRegionsTimeout` 调用 `WaitRegionsScattered`。

## 核心职责

- `SplitClient` 定义 RegionSplitter 所需的最小能力面：PD 元数据查询、Region 扫描与分裂、scatter 状态查询、placement rule 和 store label 管理。
- `PdBackend`、`PdHttpBackend` 把实际 PD gRPC/HTTP 行为抽象为可注入后端；`PdClient` 组合二者，并实现 `SplitClient`。当前 Rust 文件使用本地 trait/stub，而不是像 Go 文件那样直接建立 TiKV gRPC 连接。
- `splitKeys` 计算扫描范围、分页扫描 Region、把有序 split keys 映射到各 Region，并对失败 key 重新扫描后重试。
- `splitWaitAndMaybeScatter` 按 key 数量和总字节数切批，执行分裂、等待健康、可选 scatter，并触发 `WithOnSplit` 回调。
- `scatterRegions` 优先走批量 API；遇到旧 PD 不支持或只能返回完成百分比时降级为逐 Region scatter；`WaitRegionsScattered` 再轮询 operator 并对失败 operator 重新 scatter。
- 文件还承担 store 元数据缓存、普通/RawKV/codec-aware key 编解码分流、错误分类和指数退避策略适配。

## 主要符号

- `splitRegionMaxRetryTime = 4`：仅约束单批 `SplitRegion` 遇到 `ErrKVNotLeader` 后刷新 Region 的次数。
- `ThreadLocalBatchSize` / `maxBatchSplitSize`：线程局部的批次字节阈值，默认 6 MiB；`load/store` 接受但忽略原子 `Ordering`，测试可临时调小。线程局部意味着不同线程修改的值不互相可见。
- `SplitClient: Send + Sync`：上层契约。默认 `WaitRegionsScattered` 直接返回完成，`WaitRegionsScatteredCount` 将空错误转换为 `None`，`GetCodecPDClient` 默认返回 `None`；替代实现若依赖真实等待必须显式覆盖。
- `PdBackend`：gRPC/Region 数据面的抽象，包括 `GetStore`、Region 查询/扫描、`SplitRegion`、批量和单个 scatter、operator 查询及 store 枚举。
- `PdHttpBackend`：PD HTTP 管理面的抽象，包括副本配置、placement rule 与 store label；`SetStoreLabels` 的默认实现是无操作成功，真实实现必须覆盖才会产生标签变更。
- `ClientOptionalParameter`、`WithRawKV`、`WithOnSplit`：构造期 option。前者打开 RawKV 编码路径，后者安装每批分裂完成后的回调。
- `PdClient`：核心状态对象。`backend`/`http` 是后端，`storeCache` 缓存 store，`needScatterInit` 与 `needScatterVal` 缓存一次 scatter 必要性判断，`isRawKv`/`isCodecPDClient` 选择 key 语义，`splitBatchKeyCnt` 控制单批 key 数；`splitConcurrency` 当前只被保存而未被 Rust 执行路径消费。
- `NewClient` / `NewCodecAwareClient`：创建普通或 codec-aware 客户端。后者设置 `isCodecPDClient` 并保存 `CodecPDClient` 标记，使 `getEncodedKeys` 改走 codec 的 `DecodeRange`。
- `isScatterRegionFinished`：把 PD operator 响应归约为 `(已完成, 需重新 scatter)`；`REGION_NOT_FOUND` 或非 `scatter-region` 描述视为完成，`SUCCESS` 完成，`RUNNING` 继续等待，其余状态要求重新 scatter。
- `BackoffRetryAllExcept` / `ExponentialBackoffer` / `PdErrorCanRetry`：分别负责“除无效范围外均重试”、简单指数退避以及逐 Region scatter 的可重试错误文本分类。

## 执行流程

1. `RegionSplitter::executeSplitByKeys` 选择 `SplitKeysAndScatter` 或 `SplitKeys`，两者都进入 `PdClient::splitKeys(sortedSplitKeys, scatter)`；空输入立即返回空集合。
2. `splitKeys` 用首 key 和末 key 的 `KeyNext` 构造半开扫描范围；末 key 为空时保持开放上界。普通客户端调用 `codec::EncodeBytesExt(..., isRawKv)`，codec-aware 客户端调用 `DecodeRange`。
3. `PaginateScanRegion` 分页获取范围内 Region；codec-aware 返回的 Region 边界会通过 `encodeRegionKeys` 恢复为后续分组期待的编码形式。`getSplitKeysOfRegions` 把 key 分派给对应 Region。
4. 对每个有 key 的 Region，`splitWaitAndMaybeScatter` 按 `maxBatchSplitSize` 或 `splitBatchKeyCnt` 切批，并调用 `batchSplitRegionsWithOrigin`。当前 Rust 实现顺序遍历 Region；一次失败会把该 Region 的 keys 放入 `retrySplitKeys`，下一轮重新扫描以适应 epoch/边界变化。
5. `batchSplitRegionsWithOrigin` 调用后端 `SplitRegion`。仅 `ErrKVNotLeader` 会最多重试 4 次：通过 `GetRegionByID` 刷新元数据，并由 `validateRegionAfterNotLeader` 验证 Region epoch 未变且存在 leader；其他错误原样返回。
6. 成功后 `waitRegionsSplit` 逐个查询新 Region，直到无 pending peers、尝试次数耗尽或 context 取消。等待失败只记日志并继续；context 已取消则终止。随后按 `scatter` 标志调用 `scatterRegions`，scatter 失败同样记录后继续，最后执行 `onSplit`。
7. 为继续处理下一批，代码比较原 Region 与最后一个新 Region 的 `StartKey`，选 start key 更大的一个作为后续分裂对象，以兼容 TiKV 的 left/right derive 配置。
8. `scatterRegions` 先以 `GetAllStores` 数量和 HTTP `max-replicas` 判断是否需要 scatter；判断仅初始化一次，探测失败采用“仍然 scatter”的宽松策略。批量 scatter 只重试失败 ID；API 不支持或仅报告未完全 scatter 时降级到 `scatterRegionsSequentially`。
9. 上层随后通过 `WaitRegionsScattered` 轮询 `GetOperator`。已完成项被移除，异常终态加入重新 scatter 集合；本轮有进展时使用 `ErrBackoffAndDontCount`，全部无进展时使用 `ErrBackoff`。返回值始终包含剩余 Region 数，错误作为同一 `Result` 中的第二元素返回。

## 数据与状态

- split key 输入契约是已排序的未编码 key；代码本身不排序初始输入，也不去重。顺序是 `getSplitKeysOfRegions` 正确映射和批处理延续的前提，调用者必须维护。
- `storeCache: Mutex<HashMap<u64, Store>>` 按 store ID 永久缓存查询结果，没有主动失效机制；这与短生命周期恢复客户端的假设绑定。
- `needScatterInit: Once` 保证 store/replica 探测只运行一次，`needScatterVal: Mutex<bool>` 保存结果。`ForceNeedScatter` 是测试控制口，也会消耗 `Once`，阻止后续自动探测。PD 配置或 store 数在客户端生命周期内变化不会刷新缓存结果。
- `retrySplitKeys`、`ret`、`lastSplitErr` 在 `splitKeys` 内由 `Mutex` 包装；这延续 Go 并发实现的数据保护形状，但当前 Rust 循环是串行的。每次重试清空输出，只返回本轮 scatter 产生的新 Region。
- Region 的关键字段为 `RegionInfo.Region`、`Leader`、`PendingPeers`。多处在缺失 `Region` 时把 ID/StartKey 当作 0/空值；`validateRegionAfterNotLeader` 则对刷新结果执行严格检查。
- `onSplit` 在每个成功提交的批次完成等待/scatter 尝试后同步执行，参数只覆盖当前批次 key；回调运行在调用线程，阻塞或 panic 会直接影响调用链。

## 依赖与调用关系

上游主链为 `RegionSplitter::executeSplitByKeys` → `RegionSplitter::splitKeys` → `SplitClient::{SplitKeysAndScatter, SplitKeys}`；散射等待链为 `RegionSplitter::WaitForScatterRegionsTimeout` → `SplitClient::WaitRegionsScattered`。RustCodeGraph 对 `SplitClient` 方法的查询定位到 `client.rs` trait 定义，并由 `split.rs` 的直接调用核实生产接线。

内部依赖主要来自同 crate：`region::RegionInfo`；`split.rs` 的 `PaginateScanRegion`、`getSplitKeysOfRegions`、`encodeRegionKeys`、重试常量及 `CheckRegionEpoch`；`stubs.rs` 的 `Context`、PD protobuf/HTTP 类型、codec、重试器和日志适配。跨 crate 依赖仅显式使用 `astersql-br-pkg-errors` 的分类错误与 `astersql-errors` 的 `SharedError`/注解工具；Cargo 还声明了 `astersql-br-pkg-restore-utils` 和 `hex`，但本文件没有直接引用它们。

下游外部交互被封装在 `PdBackend` 与 `PdHttpBackend`：Region 元数据和 split/scatter/operator 走前者，副本配置、placement rule 和标签走后者。`GetStore`、`GetRegion`、`ScanRegions` 等 `SplitClient` 方法多为薄转发，`ScanRegions` 会把 options 中任一 `allow_follower` 汇总为一个布尔值。

## 错误处理与边界

- 空 split key 列表成功返回空；包含空末 key 时扫描上界保持为空，`client_test.rs::test_split_scatter_empty_end_key` 覆盖开放范围以及仅空 key 的情况。
- `PaginateScanRegion` 返回空集合会由 `split.rs::checkRegionConsistency` 产生“scan region return empty result”，`test_scan_region_empty_result` 验证重试耗尽后保留该错误。
- 外层 split 重试除 `ErrInvalidRange` 外都会退避重试；context 错误文本包含 `canceled` 时立即返回。单次后端 split 内层仅对 `ErrKVNotLeader` 刷新并重试，epoch 不一致返回 `ErrKVEpochNotMatch`，缺 leader 返回 `ErrPDLeaderNotFound`。
- `waitRegionsSplit` 把后端查询错误当作暂时不健康；达到尝试上限后当前实现仍返回 `Ok(())`。其错误在调用方也仅告警，scatter 错误同样不使已经完成的 split 回滚，因此调用者应依靠后续扫描/等待判断部分成功。
- 批量 scatter 的 header 非 OK 返回 `ErrPDInvalidResponse`；失败 ID 仅重试失败 Region。最终 `ErrPDNotFullyScatter` 被转为成功并留下未散射告警语义。旧 API 的 `unimplemented` 或 `region 0 not found` 文本触发逐 Region降级。
- `PdErrorCanRetry` 采用错误字符串匹配，只识别未完全复制、无 leader、operator queue 满和创建 scatter operator 失败；协议错误文本变化可能改变分类。
- `SetPlacementRule` 等 HTTP 方法在缺少 `http` 时返回 `http missing`；`SetStoresLabel` 顺序更新多个 store，前面成功、后面失败时不会回滚。
- 多处通过 `Mutex::lock().unwrap()` 访问状态；互斥锁中毒会 panic。批分裂成功路径还假设后端返回至少一个新 Region，否则索引 `newRegionsOfBatch[len - 1]` 会 panic，这一前置条件由后端协议承担。

## 并发与资源生命周期

`PdClient` 通过 `Send + Sync` trait 对象可被共享；共享可变状态使用 `Mutex`，scatter 决策使用 `Once`。store cache 查询在持锁期间调用 `backend.GetStore`，因此同一客户端的其他 store 查询也会被串行化。HTTP 标签写入和 Region 操作不持有全局锁。

当前 Rust `splitKeys` 顺序处理各 Region，`splitConcurrency` 没有实际调度作用；这与 Go 版通过 worker pool/errgroup 并行处理 Region 不同，是明确的迁移差异和潜在性能风险。`maxBatchSplitSize` 又是线程局部而非 Go 的进程级变量，测试或未来并行执行时必须在实际执行 split 的线程设置。

所有重试都受传入 `Context` 约束；`waitRegionsSplit` 每轮检查 `Done`，通用 `WithRetryReturnLastErr` 也接收 context。文件不创建后台任务、线程、通道或持久连接，后端对象随 `PdClient` 所有权释放；回调与 trait object 要求 `Send + Sync + 'static`，其资源同客户端一起释放。

## 与 Go 版本的对应关系

Rust 的公开概念和主流程直接对应 `br/pkg/restore/split/client.go`：`SplitClient`、`pdClient`、两个构造器、option、split 扫描/切批、批量 scatter 降级、operator 状态解释及错误字符串分类均保留了同名结构。Rust 独立测试 `client_test.rs` 对照 Go 的 `client_test.go`，覆盖 codec client、批大小、Txn/RawKV 编码、空上界、空扫描、失败重试和 PD 错误分类。

已核实的差异如下：

- Go 的 `pdClient` 持有真实 `pd.Client`、HTTP client、TLS 配置，并在 `sendSplitRegionRequest` 中选择 peer、连接 TiKV、发 RPC；Rust 把这些职责压入 `PdBackend::SplitRegion`，本文件自身没有 TLS/连接生命周期。
- Go `splitKeys` 使用 `splitConcurrency` 大小的 worker pool 并在重试前排序失败 keys；Rust 顺序遍历 Region，且重试 keys 按遍历追加，没有显式排序。因此 Rust 字段当前只是兼容占位，不能声称已具备 Go 的并发吞吐和重试排序保证。
- Go `waitRegionsSplit` 用“有进展不计数”的 backoffer；Rust 是每 Region 固定次数的紧循环，且没有显式 sleep。Go 记录 down peers 但只以 pending peers 判健康；Rust同样只检查 `PendingPeers`，但省略诊断日志。
- Go 对任意批量 scatter RPC `Unimplemented` 错误在 `isUnsupportedError` 中判断；Rust `tryScatterRegions` 对后端错误做同样文本识别。两者对旧 PD 完成百分比不足都降级逐个 scatter。
- Rust 新增 `WaitRegionsScatteredCount` 便利适配和 `ForceNeedScatter` 测试口；Go 接口没有这两个同名成员。

这些差异是当前代码事实，不应在文档任务中解释为等价实现。若继续迁移，应以 Go 文件的并发、重试、真实 RPC 和错误判定为基准，不通过删减语义来换取测试通过。

## 扩展指南

- 新增 RegionSplitter 所需能力时，先扩展 `SplitClient`，再同步 `PdClient`、所有 mock/测试实现及 `lib.rs` 的可见性；对应测试应继续放在独立的 `client_test.rs`、`split_test.rs` 或 mock 测试文件，不内嵌到生产源文件。
- 改 key 编码必须同时审查 `getEncodedKeys`、codec-aware 的 `encodeRegionKeys`、`getSplitKeysOfRegions` 以及 TxnKV/RawKV/空 end key 三组测试，避免重复编码或丢失 keyspace 语义。
- 改批策略应同时维护 key 数上限和字节上限，保留 6 MiB 小于 TiKV 默认 8 MiB raft entry 的安全余量，并新增跨批连续性、空批及超大单 key 测试。
- 若启用 `splitConcurrency`，需确保失败 keys 在下一轮重新排序、回调线程语义明确，并重新评估线程局部 `maxBatchSplitSize`；共享 `ret`/错误集合虽已有 Mutex，仍需验证输出顺序和取消传播。
- 改 scatter 兼容性时，保持批量失败 ID 的缩小重试、旧 PD 降级、非重试错误跳过和部分成功可观察性；优先增加结构化错误判断，避免扩大字符串匹配。
- 改 store/副本配置缓存时，明确失效时机；当前 `Once` 与永久 `storeCache` 假设客户端生命周期内拓扑基本稳定。
- `PdBackend` 接入真实实现时必须补齐 leader/peer 选择、TLS、连接关闭、RegionError 分类和 failpoint 等 Go 版仍在本文件承担的行为，并以独立集成测试证明，不能把 stub 成功当成生产可用。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/restore/split` 确认本 crate 的 Rust/Go 源与测试；`node --file br/pkg/restore/split/client.rs` 分段读取 1–1182 行；`query` 核对 `PdClient`、`SplitKeysAndScatter`、`WaitRegionsScattered`、`splitKeys`、`isScatterRegionFinished` 的位置与签名。
- Rust 生产代码：`br/pkg/restore/split/client.rs`；直接上游与算法依赖：`br/pkg/restore/split/split.rs`；数据类型：`br/pkg/restore/split/region.rs`；后端/重试/codec 替身：`br/pkg/restore/split/stubs.rs`；模块入口：`br/pkg/restore/split/lib.rs`。
- crate 边界：`br/pkg/restore/split/Cargo.toml`，确认包名、library 入口、Go package 映射和直接依赖。
- Go 对照：`br/pkg/restore/split/client.go`，重点核对接口、构造、split 扫描/切批、真实 TiKV RPC、worker pool、scatter 降级与 operator 状态机。
- 独立测试：Rust `br/pkg/restore/split/client_test.rs` 及 `br/pkg/restore/split/split_test.rs`；Go `br/pkg/restore/split/client_test.go`。Rust 直接覆盖的测试包括 `test_get_codec_pd_client`、`test_batch_split`、`test_split_scatter`、`test_split_scatter_raw_kv`、`test_split_scatter_empty_end_key`、`test_scan_region_empty_result`、`test_split_meet_error_and_retry`、`test_pd_error_can_retry` 及 batch scatter 降级判断。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构命令，并人工检查关键陈述均能回到以上符号或文件。
