# `pkg/ingestor/ingestctrl/localhelper.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 入口 `pkg/ingestor/ingestctrl/lib.rs` 以 `pub mod localhelper` 暴露它。它承载 local ingest 后端的四组辅助能力：批量 Region split/scatter、半开键范围判断、按 Store 写入限速、SST compaction 阈值估算。包级契约 `pkg/ingestor/doc.go` 将 Region 预切分和 scatter 定义为写入/ingest 前的环境准备，本文件就是该契约在 Rust 侧的一组基础构件。

当前接线状态必须与设计角色区分：RustCodeGraph 显示 `localhelper.rs` 的直接 Rust 使用者是 `pkg/ingestor/ingestctrl/localhelper_test.rs` 和 `pkg/executor/benchmark_test.rs`，仓库搜索未发现这些公开辅助函数被 Rust 生产路径调用。Go 对照 `localhelper.go` 则已由 `local.go`、`region_job.go`、`pkg/executor/importer/table_import.go` 等生产路径使用。因此本文件是已实现并有独立测试的移植模块，但不能据此宣称 Rust 主链已完成对应接线。

## 核心职责

1. `splitAndScatterRegionInBatches` 把 split keys 分批交给 `BatchSplitClient`；键数超过 `coarseGrainedSplitKeysThreshold`（64）时，先抽取稀疏键做粗切分，再对全量键做细切分，以提前摊开大 Region。
2. `beforeEnd`、`keyInsideRegion`、`insideRegion` 和 `largerStartKey` 提供字节字典序上的 Region 边界运算，统一采用 `[start, end)`，空 `end` 表示正无穷。
3. `storeWriteLimiter` 为每个 Store ID 惰性维护独立令牌桶，支持读取和热更新全局写速率；不同 Store 不共享令牌预算。
4. `EstimateCompactionThreshold2` 根据原始文件总量估算单次 compaction 阈值，目标约为把文件数控制在 512 内，同时把结果限制在 512 MiB 到 32 GiB。

本文件不负责真实 PD/TiKV RPC、Region 元数据扫描、SST 写入或 compaction 执行；这些动作分别由 `BatchSplitClient` 实现者和更上层导入流程承担。

## 主要符号

- `coarseGrainedSplitKeysThreshold: usize = 64`：启用两阶段 split/scatter 的严格阈值；只有键数大于 64 才执行粗切分。
- `BatchSplitClient`：`Send + Sync` 的最小客户端 trait，唯一方法 `SplitAndScatter(&CancellationToken, &[Vec<u8>]) -> Result<()>` 把实际切分/打散及其错误留给调用方实现。
- `splitAndScatterRegionInBatches(...) -> Result<()>`：公开编排入口。`batchCnt == 0` 时返回 `Error::InvalidArgument`；正速率时创建一个由粗、细两阶段共享的 `TokenBucket`。
- `getCoarseGrainedSplitKeys(&[Vec<u8>]) -> Vec<Vec<u8>>`：以 `floor(sqrt(n))`（至少 1）为步长克隆抽样，并保证最后一个 key 恰好出现一次。
- `splitAndScatterRegionInBatchesWithLimiter(...)`：私有批处理循环；每批先等待足够令牌，再调用客户端。
- `beforeEnd`、`keyInsideRegion`、`insideRegion`、`largerStartKey`：无状态的键范围工具。`insideRegion` 检查每个输入范围的起止键都满足 `keyInsideRegion`，它不是一般意义上的“允许子范围终点等于 Region 终点”的包含判断。
- `StoreWriteLimiter`：公开接口，包含 `WaitN`、`Limit`、`UpdateLimit`。
- `storeWriteLimiter`：具体实现；`limiters` 是 `storeID -> Arc<Mutex<TokenBucket>>`，`limit` 和 `burst` 是原子值。
- `newStoreWriteLimiter`、`calculateLimitAndBurst`：构造和参数归一化。非正限制转成 `(0, 0)`；正限制的 burst 为 `limit + limit/5`，用饱和加法防溢出。
- `getLimiterWithBeforeWrite`：双重检查式惰性建桶核心；注入闭包仅供并发竞态测试确定时序。`getLimiterForTest` 与 `limiterCount` 仅在 `cfg(test)` 下存在。
- `CompactionLowerThreshold`、`CompactionUpperThreshold`：分别为 512 MiB、32 GiB。
- `EstimateCompactionThreshold2(i64) -> i64`：计算 `max(totalRawFileSize / 512, 1)` 的向上二次幂，再钳制到上下界。

## 执行流程

Region 切分流程如下：

1. `splitAndScatterRegionInBatches` 先拒绝零批大小。
2. 若 `maxCntPerSec > 0`，批大小被压到 `getRateBurst(maxCntPerSec)` 以内，并以 `maxCntPerSec * ratePerSecMultiplier` 为速率、`getRateBurst(...) * ratePerSecMultiplier` 为容量创建满桶；`ratePerSecMultiplier` 在 `rate_limiter.rs` 中为 1000。
3. 当 key 数量大于 64，`getCoarseGrainedSplitKeys` 产生约 `sqrt(n)` 个稀疏键并补齐尾键，先走一轮 `splitAndScatterRegionInBatchesWithLimiter`。任何取消、锁错误或客户端错误都会立即返回，不进入细粒度阶段。
4. 随后全量 key 按 `batchCnt` 分片。有限速器时，每次调用先检查取消，预留 `batch.len() * 1000` 个令牌；令牌不足则最多睡 10 ms 后重试。成功后调用 `BatchSplitClient::SplitAndScatter`。

写限速流程如下：

1. `newStoreWriteLimiter` 保存规范化后的速率和 1.2 倍 burst，初始 Store 表为空。
2. `WaitN` 经 `getLimiter` 查找或创建对应 Store 的桶；全局 limit 为 0 时直接成功，甚至不会检查已经取消的 token。
3. `n > 0` 时，每轮最多申请当前 burst 个令牌，等待循环每次检查取消并以不超过 10 ms 的短睡轮询，直至取得该块令牌，再处理剩余数量。
4. `UpdateLimit` 在写锁下再次确认值确实变化，同时发布新的 `limit`/`burst`。新限制为 0 时清空所有 Store 桶；否则逐个调用 `TokenBucket::update` 更新已有桶。

阈值估算没有共享状态：先按 512 等分总原始大小，取不小于该值的 2 的幂，然后应用 512 MiB/32 GiB 边界。

## 数据与状态

split keys、Region 起止键均是未经解码的字节序列，比较规则是 Rust slice 的字典序。`KeyRange.end.is_empty()` 仅在 `beforeEnd` 中被解释为正无穷；范围起点没有特殊空值分支。粗抽样返回 key 的克隆，因而不借用输入，也会产生与抽样数量成比例的分配和复制。

`storeWriteLimiter` 有三层状态：全局 `AtomicIsize` 速率、全局 `AtomicIsize` burst、以及 `RwLock<HashMap<u64, Arc<Mutex<TokenBucket>>>>` 的逐 Store 桶。桶本身记录速率、容量、当前令牌和上次补充时间（见 `rate_limiter.rs::TokenBucket`）。同一 Store 的等待者竞争同一个 `Mutex<TokenBucket>`，不同 Store 只在首次建桶和更新配置时竞争映射锁。

原子读取使用 `Acquire`，更新使用 `Release`；映射的创建、清空和批量更新由写锁串行化。`getLimiterWithBeforeWrite` 在无锁快查与取得写锁之后重新读取 limit，防止关闭限速期间发布过时的新桶。

## 依赖与调用关系

直接内部依赖为：

- `crate::rate_limiter::{TokenBucket, getRateBurst, ratePerSecMultiplier}`：两类限速逻辑的令牌桶、burst 和换算尺度。
- `crate::{CancellationToken, Error, KeyRange, Result}`：取消传播、统一错误、键范围数据和结果类型，定义于 `lib.rs`。
- 标准库 `HashMap`、`Arc`、`Mutex`、`RwLock`、原子类型、`Duration` 和线程睡眠。

`Cargo.toml` 将 crate 命名为 `astersql-ingestor-ingestctrl`，库入口是 `lib.rs`，并在 `package.metadata.porting.go-package` 指向 `pkg/ingestor/ingestctrl`。本文件自身不直接引用 Cargo 中的外部 crate；所需令牌桶是本 crate 实现，而不是 Go 使用的 `golang.org/x/time/rate`。

RustCodeGraph 对主要符号的调用关系为：`splitAndScatterRegionInBatches`、`getCoarseGrainedSplitKeys`、`newStoreWriteLimiter`、`WaitN`、`UpdateLimit` 和 `EstimateCompactionThreshold2` 均由 `localhelper_test.rs` 覆盖；没有确认到 Rust 生产调用边。作为对照，图索引确认 Go 的 `splitAndScatterRegionInBatches` 由 `local.go::prepareAndSendJob` 使用，写限速接口由 `region_job.go::doWrite`、`GetWriteSpeedLimit`、`UpdateWriteSpeedLimit` 使用，Go 的阈值估算由 `pkg/executor/importer/table_import.go::OpenIndexEngine` 使用。

## 错误处理与边界

- 零 `batchCnt` 在 Rust 中显式返回 `Error::InvalidArgument`，避免 `slice::chunks(0)` 触发 panic；这是比 Go 对照更明确的输入保护。
- split/scatter 限速等待和 `WaitN` 都传播 `CancellationToken::check()` 的 `Error::Cancelled`；客户端的任意错误原样经 `?` 向上传播，粗阶段错误会阻止细阶段执行。
- `Mutex`/`RwLock` 中毒通常映射为 `Error::Poisoned`。例外是 `UpdateLimit`：trait 签名没有返回值，写锁中毒会静默放弃更新，单个桶锁中毒会跳过该桶；调用者无法从返回值发现部分更新。
- `WaitN` 的 `n <= 0` 不进入扣令牌循环并返回成功。若限速关闭，函数在获取 token 或检查取消之前返回成功，这是测试明确固定的行为。
- `insideRegion` 对每个 meta 的 `end` 也调用右端严格排除的 `keyInsideRegion`，所以 `meta.end == region.end` 返回 false；扩展调用时不能把它误当成普通半开区间包含运算。
- `calculateLimitAndBurst` 用 `saturating_add` 处理最大整数；`EstimateCompactionThreshold2` 对零数或负数最终返回下界，对极大正数返回上界。
- 当前 Rust 生产主链未接线是功能边界，不应仅凭公开 API 或 Go 调用关系推断已生效。

## 并发与资源生命周期

本文件不启动后台任务，也不持有网络连接。等待是调用线程内的同步短睡；取消检查间隔通常不超过一次 10 ms 睡眠加锁竞争和客户端调用时间。`BatchSplitClient::SplitAndScatter` 是否可取消、是否阻塞以及 RPC 资源如何释放由 trait 实现负责。

Store 桶按首次访问惰性创建，并以 `Arc` 返回，使等待者能在释放映射锁后独立持有桶。`UpdateLimit(0)` 清空映射只阻止后续获取；已经克隆出的 `Arc` 可活到当前 `WaitN` 结束。关闭与建桶竞态由写锁后的第二次 limit 检查解决，`disabling_store_write_limiter_while_creating_bucket_does_not_publish_stale_bucket` 用两个 `Barrier` 验证不会把旧配置桶重新插入映射。

更新为正限制时先持有映射写锁，再逐一锁住桶并更新，因此首次建桶与其他配置更新被阻塞；已有 `WaitN` 只短暂持有桶锁计算等待，不在睡眠期间持锁。不同 Store 的令牌消耗隔离，测试用 Store 1 耗尽预算后 Store 2 仍可使用自己的初始 burst 证明这一点。

## 与 Go 版本的对应关系

Rust 的模块划分和算法主体对应 `pkg/ingestor/ingestctrl/localhelper.go`：64 键阈值、平方根稀疏抽样、粗后细两阶段、`[start,end)` 边界、逐 Store 令牌桶、1.2 倍 burst、热更新与 compaction 上下界均保持一致。`localhelper_test.rs` 将 Go 测试意图拆为独立 Rust 测试文件，符合源码与测试分离要求；Go 原始测试位于 `localhelper_test.go`。

重要差异如下：

- Go 的切分方法绑定 `Backend` 并直接调用真实 split client；Rust 通过 `BatchSplitClient` 解耦，但当前没有生产实现/调用边，所以只证明算法和接口存在。
- Go 用 `context.Context` 和 `rate.Limiter::WaitN`；Rust 使用 crate 自有 `CancellationToken`、`TokenBucket` 和 10 ms 轮询。Rust 构造 split 限速器时直接使用浮点 `maxCntPerSec * 1000`，而 Go 先截断并将内部 event limit 至少设为 1；极小正速率下等待时长可能不同，接入生产前应做对照验证。
- Go 的 `getLimiter` 用 failpoint 固定“读锁未命中、等待写锁”竞态；Rust 用仅测试可见的 `beforeWrite` 闭包和 Barrier 验证同一不变量。
- Rust 对 `batchCnt == 0`、锁中毒和整数溢出给出显式防护；Go 依赖调用约束、不可中毒锁和手工溢出判断。
- Go `insideRegion` 接受 protobuf Region/SSTMeta；Rust 使用简化的 `KeyRange`，尚未携带 Region epoch、peer 或 SST protobuf 元数据。

## 扩展指南

- 接入真实 Rust ingest 主链时，应为实际 split/scatter 客户端实现 `BatchSplitClient`，在 local backend 准备作业的位置调用 `splitAndScatterRegionInBatches`，并补充独立集成测试证明 RPC 错误、scatter 等待和取消语义；不能把测试 mock 当作生产实现。
- 修改两阶段策略时，优先改 `coarseGrainedSplitKeysThreshold`、`getCoarseGrainedSplitKeys` 或顶层编排，并同步 `localhelper_test.rs` 中大/小输入、末键唯一、粗阶段失败和取消用例。必须确认 key 仍有序；本函数不排序也不去重。
- 修改写限速时，应保持“每 Store 独立”“关闭期间不得发布旧桶”“大请求按 burst 分块”三个不变量，并同步竞态与计时测试。若要让 `UpdateLimit` 报告锁错误，需要先变更 trait 签名及所有调用者，不能只改具体实现。
- 修改边界工具时，应增加空 end、等于 start、等于 end、多 meta 的独立测试，并核对 Go 的 protobuf 语义；若需要标准半开子区间包含，应新增语义清楚的函数，避免悄然改变 `insideRegion` 的既有严格终点判断。
- 修改 compaction 公式时，应同步上下界、2 的幂取整和极值测试，并评估文件数量、迭代成本以及大 SST compaction 时长。Go 注释给出的取舍是尽量把 SST 数控制在约 512，同时避免压缩 32 GiB 以上数据带来的长延迟。
- 所有 Rust 测试继续放在 `pkg/ingestor/ingestctrl/localhelper_test.rs`，不要内嵌回生产文件；若生产接线跨到 `local.rs`、`region_job.rs` 或 executor，还应同步各自的独立测试。

## 验证依据

- RustCodeGraph 索引状态：项目含 11,467 个文件、7,032 个 Rust 文件；`pkg/ingestor/ingestctrl/localhelper.rs` 已索引，共 301 行。
- RustCodeGraph 源码与符号查询：`node --file pkg/ingestor/ingestctrl/localhelper.rs`；`query splitAndScatterRegionInBatches`、`query EstimateCompactionThreshold2`、`query newStoreWriteLimiter`、`query WaitN`；`explore` 返回了上述 Go/Rust 调用边及测试调用者。
- 读取的 Rust 直接证据：`pkg/ingestor/ingestctrl/localhelper.rs`、`lib.rs`、`rate_limiter.rs`、`region_job.rs`、`localhelper_test.rs`。
- 读取的 crate/包证据：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/doc.go`。
- 读取的 Go 对照：`pkg/ingestor/ingestctrl/localhelper.go`、`localhelper_test.go`；调用边还指向 `local.go`、`region_job.go` 和 `pkg/executor/importer/table_import.go`。
- 测试事实：Rust 独立测试覆盖逐 Store 隔离与 burst 分块、限速热更新/关闭、关闭与建桶竞态、粗后细批次、阈值边界、粗阶段错误、取消、末键唯一、半开区间以及 compaction 极值。本任务按计划仅做文档分析，未运行 Cargo。
