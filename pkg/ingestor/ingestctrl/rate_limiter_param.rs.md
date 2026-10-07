# `pkg/ingestor/ingestctrl/rate_limiter_param.rs`

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate；crate 根在 `pkg/ingestor/ingestctrl/lib.rs`，其中以 `pub mod rate_limiter_param;` 公开本模块。`pkg/ingestor/ingestctrl/Cargo.toml` 将该 crate 定位为 Go 包 `pkg/ingestor/ingestctrl` 的 Rust 移植，并把库入口设为 `lib.rs`。

它位于导入控制面的“配置持久化与运行时限速器之间”：定义四项 ingest/split 参数的默认值和进程级原子快照，要求调用方通过 `RateLimiterParamStore` 提供持久化读写能力，再由 getter 向实际 split/ingest 流程暴露数值。需要特别说明的是，当前 Rust 仓库中只有 `lib.rs` 的模块声明和本文件内部引用；未检索到 `RateLimiterParamStore` 实现、`InitializeRateLimiterParam` 调用者或四个 getter 的 Rust 调用者，因此这是已公开但尚未接入 Rust 应用主链的移植边界，而不是当前 Rust 运行时已经启用的配置链。

## 核心职责

1. 用四个私有常量保存默认值：批量切分上限为 `2048`，每秒切分数、最大在途 ingest 数和 ingest QPS 均为 `0`。
2. 用 `RateLimiterParamStore` 抽象四项参数各自的读取和写入，避免本模块依赖具体 meta transaction 类型。
3. `InitializeRateLimiterParam` 顺序加载四项参数；持久化项缺失时先写默认值，再把有效值发布到进程级原子缓存。
4. 用 `GetMaxBatchSplitRanges`、`GetMaxSplitRangePerSec`、`GetMaxIngestConcurrency`、`GetMaxIngestPerSec` 提供无锁读取入口。

这里不创建或驱动限速器。真正消费“并发/QPS”语义的相邻 Rust 实现在 `pkg/ingestor/ingestctrl/rate_limiter.rs` 的 `newIngestLimiter`/`ingestLimiter`；但截至本次核验，两者之间没有 Rust 调用边。

## 主要符号

- `DEFAULT_MAX_BATCH_SPLIT_RANGES: i32 = 2048`：每批 split-and-scatter 的 range 数默认上限。只有这一项把缓存值 `0` 解释为回退到非零默认值。
- `DEFAULT_SPLIT_RANGES_PER_SECOND: f64 = 0.0`、`DEFAULT_MAX_INGEST_IN_FLIGHT: i32 = 0`、`DEFAULT_MAX_INGEST_PER_SECOND: f64 = 0.0`：三个“零表示不限速”的默认值；该语义也可由相邻 `rate_limiter.rs` 中 `NoLimit`、`Acquire` 的 `> 0` 分支验证。
- `CurrentMaxBatchSplitRanges: AtomicI32` 与 `CurrentMaxIngestInflight: AtomicI32`：直接存储整数配置。
- `CurrentMaxSplitRangesPerSec: AtomicU64` 与 `CurrentMaxIngestPerSec: AtomicU64`：通过 `f64::to_bits`/`f64::from_bits` 无损搬运浮点位模式；原子层不做数值计算。
- `RateLimiterParamStore`：公开、对象安全的同步 trait。每个 getter 返回 `Result<Option<T>>`，其中 `None` 表示持久化键不存在；setter 负责写入默认值。所有方法接收 `&mut self`，使初始化过程独占地顺序操作同一 store。
- `InitializeRateLimiterParam(&mut dyn RateLimiterParamStore) -> crate::Result<()>`：唯一写缓存的公开入口，按 batch、split rate、in-flight、ingest rate 的固定顺序处理。
- 四个 `GetMax*` 函数：公开读取 API。batch getter 对原子值 `0` 二次回退为 `2048`；其余 getter直接返回原子缓存（浮点值先从 bits 还原）。

## 执行流程

`InitializeRateLimiterParam` 对每项参数执行相同的“读—必要时写默认—发布”骨架，但 batch 项多一条零值回退规则：

1. 调用对应的 `GetIngest*` trait 方法。错误通过 `?` 立即返回。
2. 如果结果是 `None`，调用对应 setter 持久化默认值；setter 成功后将局部值改为 `Some(default)`。
3. 将局部值写入对应原子缓存，使用 `Ordering::Release` 发布。
4. 四项都完成后返回 `Ok(())`。

具体分支如下：

- batch：`None` 会写入 `2048`；`Some(0)` 不改持久化数据，但发布时也转换成 `2048`；其他整数原样发布。
- split rate、in-flight、ingest rate：`None` 会写入 `0`，随后发布 `0`；已有值（包括负数、NaN、无穷大或负零的浮点位模式）不经校验直接发布。
- 任一步出错即短路，后续参数不再读取或初始化；先前已完成的持久化写入和原子发布不会回滚。

初始化前四个原子均为零。此时 getter 已给出与默认配置一致的可用结果：batch 返回 `2048`，其余返回 `0`。因此读取不依赖初始化完成才具备定义良好的默认值，但初始化仍承担从持久化层装载非默认值的职责。

## 数据与状态

持久状态由 trait 实现持有，本文件只规定其形状，不规定键名、编码、事务提交或重试方式。进程内状态是四个 `static` 原子，生命周期与进程一致，没有实例级隔离或显式重置 API。

浮点缓存选择 `AtomicU64` 是因为标准库不提供对应的 `AtomicF64`。`to_bits`/`from_bits` 保留完整 IEEE-754 位模式，包括 `-0.0`、NaN payload 和无穷大；这也意味着本模块不会替调用方完成合法性校验。整数同样未限制负值。相邻 `rate_limiter.rs::newIngestLimiter` 会把负的并发/QPS 钳到零，但当前不存在从本模块 getter 到该构造函数的 Rust 接线，不能据此声称所有潜在消费者都会获得钳制。

状态更新不是四项参数的事务性快照：读者可能在初始化过程中观察到部分新值、部分旧值。该设计适合每项参数可独立读取的全局调优值，不提供跨字段一致性不变量。

## 依赖与调用关系

- 直接标准库依赖只有 `std::sync::atomic::{AtomicI32, AtomicU64, Ordering}`；错误类型通过 crate 根的 `crate::Result` 引入，实际别名为 `std::result::Result<T, crate::Error>`。
- RustCodeGraph 将本文件识别为 15 个符号，并确认 `InitializeRateLimiterParam` 向 trait 的八个 getter/setter 方法发出内部调用；同时文件级结果为 `used by 0 files`。
- `pkg/ingestor/ingestctrl/lib.rs` 公开声明本模块，但精确仓库搜索未发现任何其他 Rust 文件引用本模块 API。`Cargo.toml` 也没有为本模块提供具体存储依赖；`astersql-meta` 仅出现在 Windows 条件依赖区，且没有本 trait 的实现。
- Go 主链是当前可验证的运行时对照：`pkg/domain/domain.go` 在 `kv.RunInNewTxn` 中创建 `meta.Mutator` 并调用 Go `InitializeRateLimiterParam`；`pkg/ingestor/ingestctrl/local.go` 在导入 range 时读取 batch/split rate，在创建 ingest limiter 时读取 in-flight/QPS。
- Go 的持久化实现位于 `pkg/meta/meta.go`：整数以十进制字符串读写，浮点数写入时保留两位小数并在读取时解析。Rust trait 没有绑定这种编码细节。

## 错误处理与边界

所有持久化操作都返回 crate 统一的 `Result`。初始化函数使用 `?` 原样传播 store 错误，没有添加“正在读取哪个参数”等上下文，也没有日志。读取失败时该项不发布；缺失项的默认写入失败时同样不发布；后续项均不执行。

由于初始化是逐项进行，错误可能留下部分副作用：前面参数的默认值可能已经写入持久化层，前面原子也可能已经更新。函数没有补偿、回滚或恢复旧快照。若具体 store 需要全有或全无，必须由 trait 实现所在事务或更上层调用者提供事务边界。

输入边界完全信任 store：负整数、负速率、NaN 和无穷大都能进入缓存。batch 的特殊规则只把恰好为 `0` 的值映射为 `2048`，负数不会回退。扩展时若引入校验，应先确定与 Go 的兼容策略，并为持久化值、进程缓存值以及错误后的部分更新分别建立测试。

## 并发与资源生命周期

写入使用 `Ordering::Release`，读取使用 `Ordering::Acquire`。每次单字段 load/store 都是原子的，不会发生整数撕裂或浮点位模式的半写入；Acquire/Release 也为单次发布提供同步关系。它不把四个原子合并成一个版本化配置，因此不保证读取者看到同一轮初始化的四字段组合。

`InitializeRateLimiterParam` 本身没有互斥或 once guard；多个线程可以同时调用，并交错执行 store 读写与原子发布，最终值由最后一次对各字段的 store 决定。trait 参数是 `&mut dyn RateLimiterParamStore`，可防止单个 store 对象在安全 Rust 中被同一调用并发共享，但不能阻止不同 store 实例并行初始化这些全局原子。

模块不分配堆资源、不启动任务、不持有锁或通道，也没有清理阶段。四个静态原子在进程结束前一直存在；持久化事务和连接的生命周期完全由 trait 实现及上层调用者负责。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/ingestor/ingestctrl/rate_limiter_param.go`：四个默认值、四个全局缓存、初始化顺序和四个 getter 的公开语义一致。Go 通过泛型 `initializeVariables` 复用初始化骨架，Rust 则将四段流程展开；Go 用 `atomic.Pointer<T>` 表达“尚未初始化”，Rust 用零初始化原子加 getter 默认语义表达相同的启动期行为。

关键差异如下：

- Go 初始化函数直接接收 `*meta.Mutator` 和 `*zap.Logger`；Rust 用 `RateLimiterParamStore` 解耦具体存储，且不记录加载/默认写入日志。
- Go 的泛型 helper 对任意零值先换成默认值。因为除 batch 外另外三个默认值本身都是零，当前结果与 Rust 一致；Rust 只显式对 batch 做非零回退。
- Go 用指针原子保存整数/浮点值；Rust 的浮点值以 `AtomicU64` 位模式保存。
- Go getter 已被 `local.go` 消费，初始化入口已被 `domain.go` 调用；Rust 对应 API 当前没有仓库内调用者或 store 实现，迁移接线尚未完成。
- Go `meta.Mutator` 为读取错误添加底层错误链，Go 初始化 helper 再添加参数名上下文；Rust 只传播 trait 返回的 `crate::Error`，上下文质量取决于实现者。

## 扩展指南

新增或修改参数时，应同步考虑以下接入点：

1. 在本文件增加默认常量、原子缓存、trait getter/setter、`InitializeRateLimiterParam` 分支和读取函数；决定零值是否表示“不限速”、默认回退还是合法显式值。
2. 在具体 meta 存储层实现 trait 方法，并在 Rust 应用启动事务中调用初始化函数。目前这两个接线点都不存在，不能只增加字段而假设其会被加载。
3. 在实际 split/ingest 路径消费 getter。对应设计可参考 Go `local.go` 的两组调用，但应以 Rust `local.rs` 和 `rate_limiter.rs` 的真实接口为准，不机械复制 Go 并发模型。
4. 浮点参数必须明确是否允许负值、NaN、无穷大及精度损失；若持久化协议需与 Go `meta.go` 互操作，还要覆盖其两位小数字符串格式。
5. 测试必须放在独立测试文件，建议新增同目录 `rate_limiter_param_test.rs` 并在 `lib.rs` 的 `#[cfg(test)]` 区声明，覆盖缺失值写默认、已有值加载、batch 零回退、每一步读写错误短路、部分更新、浮点 bits 往返以及并发读写。不要把测试内嵌进生产源文件。

兼容风险主要是持久化编码/零值语义变化；正确性风险主要是初始化失败后的部分发布和非法数值进入缓存；性能风险较低，运行时读取是单次原子 load，但频繁或并发重新初始化可能造成配置瞬态不一致。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖本文件；`files --filter pkg/ingestor/ingestctrl` 找到 Rust/Go 对照；`node --file pkg/ingestor/ingestctrl/rate_limiter_param.rs` 读取 119 行、15 个符号并报告 `used by 0 files`；`query InitializeRateLimiterParam` 同时定位 Rust 第 64 行和 Go 第 47 行；`explore` 给出初始化函数到八个 trait 方法的调用边。精确 `callers` 查询在本次环境中超时，未将超时结果作为结论依据。
- Rust 源与模块边界：`pkg/ingestor/ingestctrl/rate_limiter_param.rs`、`pkg/ingestor/ingestctrl/lib.rs`、`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/rate_limiter.rs`。
- Go 对照与运行时入口：`pkg/ingestor/ingestctrl/rate_limiter_param.go`、`pkg/ingestor/ingestctrl/local.go`、`pkg/domain/domain.go`、`pkg/meta/meta.go`、`pkg/ingestor/ingestctrl/rate_limiter.go`。
- 测试检索：仓库级精确搜索未找到四个 getter、`InitializeRateLimiterParam` 或 `RateLimiterParamStore` 的 Rust/Go 测试引用；现有 `rate_limiter_test.rs`/`.go` 测试的是限速器本体，不是本参数加载模块。因此本文不声称已有直接回归覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证仅执行固定十一章节结构检查，并人工复核上述符号、调用边和“Rust 当前未接线”限制。
