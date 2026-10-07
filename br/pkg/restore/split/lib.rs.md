# `br/pkg/restore/split/lib.rs`

## 文件定位

`lib.rs` 是 Cargo 包 `astersql-br-pkg-restore-split` 的 crate root。`br/pkg/restore/split/Cargo.toml` 通过 `[lib] path = "lib.rs"` 指向本文件，并以 `[package.metadata.porting] go-package = "br/pkg/restore/split"` 标明它对应 Go 的同路径包。

本文件不是 region 分裂算法或 PD RPC 的实现入口，而是编译期模块装配和公开 API 门面。它没有 `main`、函数、类型、运行期初始化或可变业务状态；实际行为分别位于 `region.rs`、`sum_sorted.rs`、`client.rs`、`split.rs`、`splitter.rs`，测试替身位于 `mock_pd_client.rs` 和 `stubs.rs`。

可验证的直接 Rust 上游是 `br/pkg/restore/log_client`：其 `Cargo.toml` 以路径 `../split` 依赖本 crate，`log_split_strategy.rs` 与 `compacted_file_strategy.rs` 使用 `splitter::{BaseSplitStrategy, NewBaseSplitStrategy}` 和 `sum_sorted::{NewSplitHelper, Span, Value, Valued}`。这证明该 crate 已接入日志恢复的分裂键累计策略；不能仅由门面声明推断真实 PD/TiKV 网络链路已完整接线。

## 核心职责

本文件承担四项边界职责：

1. 通过 `#[path = "..."] pub mod ...` 将 7 个生产构建模块纳入 crate：`stubs`、`region`、`sum_sorted`、`client`、`split`、`splitter`、`mock_pd_client`。
2. 通过 6 条 `pub use ...::*` 将除 `stubs` 外各模块的公开符号提升到 crate 根，模拟 Go 同包文件共享的扁平命名空间。
3. 让 `stubs` 保持公开命名空间但不通配重导出；调用者必须显式使用 `...::stubs::Context` 等路径。`mock_pd_client` 虽包含测试替身，却是无条件公开模块并被根级重导出，当前并非仅在 `cfg(test)` 下编译。
4. 只在测试构建中挂载 `parity_test.rs` 及 6 个独立 `*_test.rs`，保证 Rust 测试逻辑不嵌入生产源文件。

文件顶部的 `#![allow(...)]` 对整个 crate 放宽死代码、Go 风格命名、未使用项及全部 Clippy lint。这是迁移期兼容策略；它会降低“未使用”告警的可见性，因此不能把可编译或无告警当作所有公开 API 都有真实上游的证据。

## 主要符号

本文件不自行定义业务符号，主要符号是模块边界和通配重导出。各模块的代表性公开 API 如下：

- `region`：`RegionInfo` 包装 region、leader 和 pending/down peers；`beforeEnd` 实现“空结束键代表正无穷”的范围比较。
- `sum_sorted`：`Value`、`Span`、`Valued`、`SplitHelper` 和 `NewSplitHelper` 维护重叠键区间的大小/键数累计，用于选择分裂点。
- `client`：`SplitClient` 抽象扫描、分裂、散射和等待能力；`PdClient`、`NewClient`、`NewCodecAwareClient` 封装 PD/PD HTTP 后端；`ExponentialBackoffer` 和 `PdErrorCanRetry` 处理重试分类。
- `split`：`RegionSplitter`、`NewRegionSplitter`、`PaginateScanRegion`、`ScanRegionsWithRetry` 承担按 region 分组、分页扫描、连续性检查、分裂及 scatter 等待；`WaitRegionOnlineBackoffer` 与 `BackoffMayNotCountBackoffer` 保存不同重试预算语义。
- `splitter`：`Splitter`、`SplitStrategy<T>`、`BaseSplitStrategy`、`PipelineRegionsSplitter`、`SplitPoint` 把区间累计、表 ID 重写、阈值判断和底层 region 分裂组合成策略层。
- `mock_pd_client`：`TestClient`、`MockPDClientForSplit`、`FakePDHTTPClient`、`FakePDClient`、`FakeSplitClient` 提供内存替身。它们用于验证和上层 fixture，不应被描述成真实集群客户端。
- `stubs`：为移植实现提供 `Context`、错误/重试、kvproto 风格结构、日志和编码等兼容边界；该模块不被 `pub use stubs::*` 提升到根路径。

通配重导出意味着外部既可写 `astersql_br_pkg_restore_split::splitter::BaseSplitStrategy`，也可从 crate 根访问该类型。未来子模块新增 `pub` 项会自动扩大根 API；若不同模块导出同名符号，可能在 crate root 产生冲突。

## 执行流程

`lib.rs` 只有编译期装配流程，没有运行期业务控制流：

1. Cargo 读取 `Cargo.toml`，以本文件作为库入口并解析三个直接 workspace 依赖和 `hex`。
2. 编译器应用 crate 级 lint 允许列表，再按显式 `#[path]` 加载 7 个模块。声明顺序不代表运行期调用顺序。
3. `pub use` 建立扁平公开面；上游可以选择根路径或子模块路径引用公开符号。
4. 当前已验证的上游 `restore/log_client` 构造 `BaseSplitStrategy`，以 `SplitHelper` 累计日志文件或 compacted SST 的键区间、大小和数量。达到策略阈值后，累积结果可转换成按重写后表前缀排序的 `SplitHelperIterator`。
5. 若调用方继续执行 region 分裂，实际链路由 `splitter::SplitPoint`/`PipelineRegionsSplitterImpl` 调用 `split::RegionSplitter`，后者经 `client::SplitClient` 扫描 region、提交 split 并等待 scatter；这些调用都发生在子模块，不由 `lib.rs` 自动启动。
6. 测试构建额外装入 7 个私有测试模块；普通库构建完全排除这些 `#[cfg(test)]` 模块，但仍包含无条件声明的 `mock_pd_client`。

因此，导入本 crate 不会自动连接 PD、创建线程、发起 region split 或等待 scatter。调用方必须显式构造客户端/策略并驱动相关方法。

## 数据与状态

本文件自身不持有业务数据。它只定义静态编译配置：模块路径、可见性、重导出集合、测试条件和 lint 策略。

经门面暴露的主要状态所有者位于子模块：

- `RegionInfo` 持有 region 元数据、leader、pending peers 和 down peers；范围判断遵循左闭右开及空结束键无穷上界语义。
- `SplitHelper` 保存有序、可合并的 `Valued` 区间；`Value` 同时累计字节数与键数。
- `PdClient` 保存 PD/HTTP 后端、TLS/RawKV 选项、并发和批大小、split 回调等连接与执行配置。
- `RegionSplitter` 保存 `SplitClient`、region 索引步长及 coarse-scatter 选择。
- `BaseSplitStrategy` 以旧 table ID 为键保存 `SplitHelper` 与 `RewriteRules`，并用 `AccumulateCount` 记录本批累计项数。
- `PipelineRegionsSplitterImpl` 用 `Mutex<Vec<RegionInfo>>` 缓存本轮产生、随后要等待 scatter 的 region。
- mock 模块保存 region tree、store/placement 配置、重试计数和注入回调；这些状态是测试模型，不代表生产 PD 的持久状态。

`pub use` 只改变名称解析路径，不复制上述对象，也不产生缓存、单例或额外所有权。

## 依赖与调用关系

向下依赖有两层：

- 模块层：`client` 使用 `split` 的 region 键编码、epoch 检查和等待上限；`splitter` 使用 `client::SplitClient`、`split::RegionSplitter`/`ScanRegionsWithRetry`、`region::RegionInfo` 与 `sum_sorted` 的累计结构；各实现共同依赖 `stubs` 提供的兼容类型。
- Cargo 层：`Cargo.toml` 声明 `astersql-br-pkg-errors`、`astersql-br-pkg-restore-utils`、`astersql-errors` 和 `hex`。其中 restore-utils 提供 `RewriteRules` 与 table key 重写能力，两个 errors crate 提供 BR/通用错误表示。

向上关系由全仓 Cargo/源码搜索确认：`br/pkg/restore/log_client/Cargo.toml` 是唯一对 package 名的依赖声明；`log_split_strategy.rs` 和 `compacted_file_strategy.rs` 是唯二直接使用 Rust crate 路径的生产文件。RustCodeGraph 进一步显示 `NewLogSplitStrategy` 被 `log_client::PreSplitRegions` 和两个 wrapper 使用，`NewCompactedFileSplitStrategy` 被 compacted-file wrapper 使用；这些策略以本 crate 的累计结构准备分裂信息。

测试关系由本文件直接声明：`parity_test.rs` 做跨模块 Go/Rust 公开契约验证，`client_test.rs` 验证 PD client 与 split/scatter 行为，`mock_pd_client_test.rs` 验证替身，`split_test.rs` 覆盖扫描/一致性/重试/取消/分裂，`region_test.rs` 覆盖范围边界，`splitter_test.rs` 覆盖策略分裂边界，`sum_sorted_test.rs` 覆盖有序区间累计。

## 错误处理与边界

本文件没有 `Result`、错误分支或运行期恢复逻辑。模块文件缺失、循环/未满足依赖、重复根导出名称以及测试模块无法编译，都会表现为编译期错误。

运行期错误由重导出的实现保持原样传播：region 扫描会检查空结果、首尾覆盖和相邻 region 连续性；split client 区分可重试 PD/grpc 错误和应立即返回的错误；context 取消会终止重试；scatter 等待以未完成 region 数表达超时结果。`lib.rs` 不捕获、包装或降级这些错误，所以从根路径与从原模块路径调用的语义相同。

需要特别保持以下边界：

- 空 `EndKey` 在 region 范围语义中表示正无穷，不等同于普通空字节串。
- `WaitRegionOnlineBackoffer` 与 `BackoffMayNotCountBackoffer` 的计数规则不同；后者可对特定退避不消耗尝试次数。
- `mock_pd_client` 和 `stubs` 的简化行为只说明测试/移植契约，不能证明真实 PD 的错误身份、网络失败或持久化行为。
- crate 级 `allow` 会隐藏未使用和风格问题，但不会改变任何错误传播或使未接线能力变为可用。

## 并发与资源生命周期

`lib.rs` 自身不创建线程、异步任务、通道、锁、网络连接或事务，也没有 `Drop`、`Close` 或关闭顺序。模块装配和重导出均在编译期生效。

门面暴露的实现包含并发与资源约束：`SplitClient` 要求 `Send + Sync`；`PipelineRegionsSplitterImpl` 以 `Mutex` 保护待 scatter region 缓冲；客户端与 mock 广泛使用 `Arc`、`Mutex` 和原子计数共享状态；重试器保存尝试预算和退避时长。`Context` 负责取消/截止时间传播，测试明确验证首次调用前已取消时不得执行操作闭包。

实际 PD/HTTP 连接、split 请求和 scatter operator 的生命周期属于注入的后端及 `client.rs`，不是 crate root 所有。扩展时不应在 `lib.rs` 新增隐藏的全局锁、后台任务或客户端单例；资源所有权、取消和关闭应留在对应实现类型中，并由独立测试覆盖。

## 与 Go 版本的对应关系

Go 没有单一的 `lib.go`：同目录、同 `package split` 的生产文件天然组成一个扁平包。Rust 必须显式声明 crate root，因此本文件用 `pub mod + pub use` 模拟 Go 包级名称空间。主要一一对应关系为：

- `region.rs` ↔ `region.go`
- `sum_sorted.rs` ↔ `sum_sorted.go`
- `client.rs` ↔ `client.go`
- `split.rs` ↔ `split.go`
- `splitter.rs` ↔ `splitter.go`
- `mock_pd_client.rs` ↔ `mock_pd_client.go`

`stubs.rs` 是 Rust 移植所需的本地兼容层，没有同名 Go 生产文件；Go 直接依赖真实 TiDB/PD/TiKV 类型。`BUILD.bazel` 列出的 Go 生产源正是上述 6 个 `.go` 文件，并把 `client_test.go`、`split_test.go`、`sum_sorted_test.go` 组成 Go 测试目标。

Rust 测试面更细：除对应 Go 三个测试文件的 `client_test.rs`、`split_test.rs`、`sum_sorted_test.rs` 外，还拆出 `region_test.rs`、`splitter_test.rs`、`mock_pd_client_test.rs` 和综合 `parity_test.rs`。`parity_test.rs::go_rust_public_contract_matches` 对照常量、区间累计、region 边界/连续性、重写 split point、退避和 epoch 等契约。Rust manifest 的依赖集合明显小于 Go Bazel 目标，且实现使用本地 stubs，因此不能把 Go 的真实 PD/TiKV 依赖能力直接视为 Rust 已具备。

## 扩展指南

按职责选择修改位置：region 元数据与范围判断进入 `region.rs`；区间累计算法进入 `sum_sorted.rs`；PD/HTTP RPC 与错误分类进入 `client.rs`；扫描、连续性、重试和基础 split/scatter 进入 `split.rs`；表重写、阈值和流水线策略进入 `splitter.rs`；仅测试替身行为进入 `mock_pd_client.rs` 或 `stubs.rs`。不要把业务算法放进 crate root。

新增生产模块时应：

1. 创建独立 `.rs` 实现与独立 `*_test.rs`，遵守生产代码和测试分文件要求。
2. 在本文件增加显式 `#[path] pub mod`；只有确需保持 Go 包式根 API 时才增加通配重导出，并先检查同名冲突和兼容性扩张。
3. 若新增外部依赖，更新 `Cargo.toml`；若要进入应用主链，还必须在真实上游 manifest 和入口中接线，不能用“模块已公开”替代调用证据。
4. 同步核对对应 Go 文件和测试；公开契约变化应更新 `parity_test.rs`，具体算法变化更新同目录相应独立测试。
5. 涉及 region 边界、table key 重写、重试计数、取消、scatter 等待或锁范围时，分别评估兼容性、正确性和性能风险，避免仅靠 mock 成功路径证明真实集群行为。

删除或收窄 `pub use` 会破坏现有根路径 API；新增通配导出可能造成名字冲突。批大小、重试间隔、scatter 超时、region 索引步长和 split 阈值均属于子模块行为，修改后应运行对应 Rust 独立测试及 Go 对照测试，而不是只验证门面结构。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：当前索引包含 7032 个 Rust 文件；`files --filter br/pkg/restore/split` 列出本目录 24 个 Go/Rust 生产与测试文件。
- RustCodeGraph `node --file br/pkg/restore/split/lib.rs --offset 1 --limit 240`：确认文件共 78 行，包含 7 个公开模块、6 条通配重导出和 7 个 `cfg(test)` 测试模块，且没有业务函数或类型。
- RustCodeGraph 对 `astersql_br_pkg_restore_split`、`BaseSplitStrategy`、`NewLogSplitStrategy`、`NewCompactedFileSplitStrategy` 的查询：确认直接 Rust 消费者及其在日志恢复预分裂策略中的位置；crate root 本身没有可用的函数 callers/callees 节点。
- `br/pkg/restore/split/Cargo.toml` 与 `br/pkg/restore/log_client/Cargo.toml`：确认 crate 边界、Go 包元数据、直接依赖和唯一反向 Cargo 依赖。
- 全仓搜索 `astersql-br-pkg-restore-split|astersql_br_pkg_restore_split`：确认生产引用仅位于 `restore/log_client` 的两个策略文件，没有把未发现的上游写成既成事实。
- 读过的 Rust 实现证据：`region.rs`、`sum_sorted.rs`、`client.rs`、`split.rs`、`splitter.rs`、`mock_pd_client.rs` 的公开符号与相互引用；`stubs.rs` 的角色由 crate 文档、使用路径及测试验证。
- 读过的 Go/Cargo/Bazel 对照证据：同目录 `region.go`、`sum_sorted.go`、`client.go`、`split.go`、`splitter.go`、`mock_pd_client.go` 的符号清单，以及 `BUILD.bazel` 的生产/测试源集合。
- Rust 独立测试 `parity_test.rs`、`client_test.rs`、`mock_pd_client_test.rs`、`split_test.rs`、`region_test.rs`、`splitter_test.rs`、`sum_sorted_test.rs`：覆盖公开契约、PD client、替身、扫描/重试/取消、范围边界、策略及区间累计；Go 对照测试为 `client_test.go`、`split_test.go`、`sum_sorted_test.go`。

本任务是纯文档分析，按计划未运行 Cargo，也未验证真实 PD/TiKV 网络、故障和性能行为。交付验证限定为固定 11 章节结构、路径/事实人工复核、Markdown diff 检查及只暂存目标文档的提交检查。
