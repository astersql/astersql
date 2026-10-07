# `br/pkg/restore/split/mock_pd_client.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-br-pkg-restore-split`。该包由同目录的 `Cargo.toml` 定义为 library crate，入口是 `lib.rs`；`lib.rs` 以 `pub mod mock_pd_client` 装入本文件，并通过 `pub use mock_pd_client::*` 将其中的公开符号提升到 crate 根。它不是 BR 恢复的真实 PD 网络客户端，而是一组供 region 分裂、scatter、PD HTTP 配置及重试测试使用的进程内替身。真实业务契约由 `client.rs` 中的 `PdBackend`、`PdHttpBackend`、`SplitClient` 抽象，本文件实现这些抽象来驱动 `client.rs`、`split.rs` 和 `splitter.rs` 的测试路径。

尽管文件名含 `mock`，它不是独立测试文件：它作为生产候选模块参与 crate 编译，并被 `client_test.rs`、`split_test.rs`、`mock_pd_client_test.rs`、`parity_test.rs` 以及相邻 restore 模块引用。模块注释明确说明其对照对象为同目录 `mock_pd_client.go`，且不连接真实 PD gRPC。

## 核心职责

文件提供五类测试替身，职责彼此不同：

1. `RegionTree` 用有序 `Vec<RegionInfo>` 模拟 PD 的 region 区间索引，负责按 ID/起始键覆盖插入，以及按半开区间扫描。
2. `TestClient` 与 `TestClientMut` 实现 `SplitClient`，维护 store、region 和递增 region ID，用于验证编码后的 split key 如何改写两个相邻 region。
3. `MockPDClientForSplit` 实现 `PdBackend`，集中模拟扫描失败/空结果、单 region 与批量 scatter、operator 状态队列、split 劫持和真实的内存 region 分裂。
4. `FakePDHTTPClient` 实现 `PdHttpBackend`，以互斥哈希表模拟 placement rule 的读取、写入与删除；replica 配置固定返回 `max-replicas = 3`。
5. `FakePDClient` 和 `FakeSplitClient` 是更窄的替身：前者覆盖 store/region 查询及可控的 not-leader 扫描，后者主要记录区间并支持简单扫描，其余接口多为成功空结果或显式 `not implemented`。

这些实现的目的都是让上层算法观察到稳定的输入、错误、计数和状态转换；不能据此推断真实 PD 的 RPC、鉴权、超时、leader 切换或持久化行为已经实现。

## 主要符号

- `RegionTree { regions: Vec<RegionInfo> }`：`SetRegion` 以相同 region ID 或相同 `StartKey` 替换旧项，否则插入后按 `StartKey` 排序；`ScanRange(start, end, limit)` 选择与 `[start,end)` 相交的 region，空 `end` 表示无上界，`limit == 0` 表示不限量。
- `NewTestClient` / `TestClient`：外层包含 `TestClientMut`，另有原子错误注入字段；`GetPDClient` 从内部 store 快照构造 `FakePDClient`。其 `SplitClient` 实现多数委托给 `TestClientMut`，但 `ScanRegions` 使用自身的 `InjectErr`/`InjectTimes`。
- `NewTestClientMut` / `TestClientMut` / 私有 `TestClientInner`：用 `Mutex` 保护 stores、以 ID 索引的 `Regions`、扫描树 `RegionsInfo` 和 `nextRegionID`。`SplitWaitAndScatter` 对每个原始 key 先调用 `codec::EncodeBytes`，找到包含该内部键的 region，创建左侧新 region，并将原 region 的 `StartKey` 推进到 split key。
- `SplitHijack`：`Box<dyn FnMut() -> Result<(RegionInfo, Vec<RegionInfo>)> + Send>`，允许测试直接接管一次或持续接管 `SplitRegion` 的返回结果。
- `NewMockPDClientForSplit` / `MockPDClientForSplit`：克隆只克隆 `Arc`，所有克隆共享 `MockPDInner`。公开 setter/getter 配置扫描错误队列、扫描 hook、split hook、scatter 失败次数/完成百分比、operator 响应队列，并暴露计数供断言。
- 私有 `MockPDInner::set_regions`：把相邻 boundary 两两组成 region，递增 `lastRegionID`，固定 leader/peer 的 store ID 为 1，并写入 `RegionTree`。
- 私有 `MockPDInner::split_region_inner`：根据 `is_raw_kv` 决定是否编码 keys，移除被切 region，按 `[原起点, split keys..., 原终点]` 重建连续区间；原 region ID 分配给首段，其余段使用新 ID，返回 `(origin, others)`。
- `NewFakePDHTTPClient` / `FakePDHTTPClient`：实现 placement rule 的内存 CRUD；规则按 `ruleID` 索引，首次读取缺失规则会创建并保存默认规则。
- `NewFakePDClient` / `FakePDClient`：实现 `PdBackend` 的简化查询、扫描和成功 scatter；`notLeader` 配合 `retryTime: AtomicI32` 控制扫描前若干次返回 `not leader`。
- `NewFakeSplitClient` / `FakeSplitClient`：`AppendRegion` 记录区间，`ScanRegions` 以 Go 版本相同的字节比较筛选；其 store/region/operator/placement-rule 方法仍是桩。

## 执行流程

典型 split/scatter 测试从 `NewMockPDClientForSplit` 开始：

1. 测试调用 `SetRegions` 建立有序的 region 区间，必要时用 `SetStores` 填充 store；`client_test.rs` 和 `split_test.rs` 随后把该值装箱交给 `NewClient` 等上层入口。
2. 上层通过 `PdBackend::ScanRegions` 查询区间。实现先从 `scan_errors` 队首消费一个脚本结果：`Some(error)` 立即失败，`None` 返回无错误空集合；没有脚本结果时，暂时从锁内取出 `scan_before_hook`，无锁执行，再恢复 hook 并扫描树。这条路径用于验证重试、空扫描和扫描期间状态变化。
3. 上层请求 `SplitRegion` 时先递增 `split_count`。若安装了 hijack，则在释放互斥锁后调用闭包；持久 hijack 在调用结束后重新放回。否则进入 `split_region_inner`，按 raw/transactional 模式处理 key 编码并重建区间树。
4. scatter 可走批量或逐 region 路径。`ScatterRegions` 可模拟“不支持批量”、返回若干次 failed IDs，或按 `finished_percentage` 累加已完成计数；`ScatterRegion` 对每个 ID 独立计数，在超过 `scatter_each_fail_before` 之前返回“not fully replicated”。
5. 等待 scatter 完成时，上层可反复调用 `GetOperator`；若该 region 配置了响应队列，则逐个消费，否则返回 `SUCCESS`、描述为 `scatter-region` 的默认响应。

`TestClientMut::SplitWaitAndScatter` 是另一条更直接的路径：它不模拟 PD operator，而是在一把锁内逐 key 查找 `ContainsInterior` 命中的 region，创建新 region、推进旧 region 起点，同时更新 ID map 和扫描树，最终返回新建项。

## 数据与状态

核心不变量是 region 区间按 `StartKey` 排序并按半开区间解释。`RegionTree::ScanRange` 排除 `region.StartKey >= end`（当 end 非空）和 `region.EndKey <= start`（当 region end 非空）的项，因此边界相接不算重叠。`SetRegion` 对同 ID 或同起点采取替换，避免扫描树中保留两个代表同一逻辑位置的条目。

`TestClientMut` 同时维护 `HashMap<u64, RegionInfo>` 与 `RegionTree`。split 时两者都被更新：新 region 插入 map/tree，原 region 的新起点也重新写回 tree；若只修改其中之一，按 ID 查询与范围扫描就会漂移。

`MockPDInner` 的状态都位于同一个 `Mutex` 下，包括 regions、stores、ID 分配器、FIFO 错误/响应队列和计数器。`MockPDClientForSplit::clone` 共享这些状态，适合让 hook 闭包或测试线程观察同一实例。`TestClient` 的顶层 `InjectTimes` 和 `FakePDClient::retryTime` 使用 `SeqCst` 原子操作，但前者与内部 `TestClientMut::set_inject` 的字段是两套独立注入状态。

几个值具有特殊含义：扫描 `limit <= 0` 在 `MockPDClientForSplit` 中归一为 0，即不限量；空 end key 表示无上界；`scan_errors` 中的 `None` 表示“成功但没有 region”，不是“没有配置错误”；`scatter_finished_percentage` 默认 100；缺少 operator 脚本时默认认为 scatter 成功。

## 依赖与调用关系

crate 内部依赖来自 `client::{PdBackend, PdHttpBackend, SplitClient}`、`region::RegionInfo` 与 `stubs` 中的 context、codec 和 PD/metapb/pdhttp 数据结构；错误统一通过 `astersql_errors::{New, SharedError}` 构造。`Cargo.toml` 直接声明 `astersql-errors` 的本地路径依赖，本文件没有网络或异步运行时依赖。

上游调用证据主要位于独立测试：`mock_pd_client_test.rs` 直接验证 `NewTestClient` 的分裂状态、`MockPDClientForSplit` 的追加 region/空 key split、`FakePDHTTPClient` 的缺失规则创建和 `FakeSplitClient` 的空 end 比较；`client_test.rs` 将 `NewMockPDClientForSplit` 传给 `NewClient`/`NewCodecAwareClient`，并使用扫描空结果及 split hijack；`split_test.rs` 使用 scatter 配置、operator 响应队列、扫描错误和 fake split client 覆盖批量分裂、重试及兼容分支；`parity_test.rs::go_rust_public_contract_matches` 还对照 Go 公开契约。

RustCodeGraph 将本文件标为被 22 个文件使用，并识别 `NewMockPDClientForSplit` 的 Rust 调用者包括 `client_test.rs`、`split_test.rs`、`mock_pd_client_test.rs` 与 `parity_test.rs`。同目录 `lib.rs` 是模块装配与公开再导出入口。Go 对照侧，`client_test.go` 和 `split_test.go` 大量构造 `MockPDClientForSplit`，说明该 mock 的主要消费者是 split 客户端算法测试，而非运行时恢复入口。

## 错误处理与边界

所有 trait 方法用 crate 的 `Result` 返回可观察错误；常见错误文本包括 `store not found`、`region not found`、`not leader`、`key and endKey are the same`、`unimplemented` 及 `region {id} is not fully replicated`。placement rule 等未覆盖接口在 `TestClient`/`TestClientMut`/`FakeSplitClient` 中明确返回 `not implemented`，这是当前桩边界，不能在文档或测试中描述为已支持。

实现仍包含若干仅适用于受控测试输入的 panic 边界：多处 `Mutex::lock().unwrap()` 会在锁中毒后 panic；`TestClientMut::GetRegion`、`FakePDClient::ScanRegions` 和 `FakeSplitClient::ScanRegions` 对缺失 `Region` 的 `RegionInfo` 使用 `unwrap()`；调用者应只注入结构完整的测试 region。相反，`RegionTree::ScanRange` 会跳过没有 meta 的项。

split keys 不会在本文件内排序或去重，`split_region_inner` 直接按传入顺序组装 boundaries；安全扩展时必须保持上层提供有序、有效内部键的契约，或同时补齐错误语义和测试。`TestClientMut` 对不在任何 region interior 内的 key 选择跳过而非报错。`FakeSplitClient::ScanRegions` 对空 end key 沿用 Go 的普通字节比较，因此示例 `[a,z)` 对 `endKey = []` 返回空，这与把空 end 当正无穷的其他扫描替身不同，测试已锁定该差异。

## 并发与资源生命周期

本文件没有后台任务、channel、文件句柄或网络连接。生命周期完全由普通 Rust 所有权、`Arc`、`Mutex` 和原子计数管理：`MockPDClientForSplit` 可克隆并共享状态；`TestClientMut`、`FakePDHTTPClient`、`FakePDClient` 和 `FakeSplitClient` 各自用互斥锁保护可变集合。

最重要的锁约束在 `MockPDClientForSplit::ScanRegions` 与 `SplitRegion`。扫描 hook 被暂时移出锁后调用，再恢复到状态中，允许 hook 访问同一 mock 而不自死锁；split hijack 也在锁外执行，持续 hijack 仅在回调结束后有条件放回。新增 hook 时应遵循相同模式，避免在持有 `mu` 时执行可能回调本对象的外部闭包。

其余操作大多在单次临界区完成，使计数、响应队列消费和 region 重建对观察者呈原子状态。代价是 `split_region_inner` 的编码、重建和多次线性扫描都持锁执行；这是测试替身的可预测性取舍，不代表生产性能模型。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/restore/split/mock_pd_client.go`。Rust 保留了 Go 的主要类型分工与可观察语义：`TestClient` 的编码 split、`MockPDClientForSplit` 的 scan/split/scatter/operator 脚本、`FakePDHTTPClient` 的 placement rules、`FakePDClient` 和 `FakeSplitClient` 的窄替身都可找到同名实现。

需要明确记录的当前差异包括：

- Go `TestClient` 用一个 `RWMutex`；Rust 将主要状态放入 `TestClientMut::Mutex`，顶层扫描注入另用原子字段，并未复刻 Go 的 `scattered` map。
- Go `MockPDClientForSplit` 还实现 `BatchScanRegions`、`WithCallerComponent` 等完整 PD client 表面；Rust 只实现本 crate 的 `PdBackend` 契约，没有把整个 Go `pd.Client` 接口搬入。
- Go split hijack 返回 `(bool, SplitRegionResponse, error)`；Rust 将其收敛为 `(origin RegionInfo, other regions)` 的 crate 内部结果，并额外提供一次性/持续两种安装方式。
- Go `FakePDHTTPClient` 还模拟 scheduler delay 和通用 config；Rust 当前只实现 `PdHttpBackend` 所需的 replica config 与 placement rule CRUD，`schedule` 字段尚未参与行为。
- Go `FakePDClient` 的扫描会改写 peer store ID，并包含 TSO/not-leader 行为；Rust 版本不模拟 TSO，只在 region 扫描上实现可控 not-leader。

这些差异说明 Rust 是针对当前 crate 抽象的移植，而不是 Go mock 全接口的逐字段复制；新增上层能力时应先判断对应 trait 是否需要扩展，再对照 Go 行为补齐。

## 扩展指南

扩展 PD 查询或 split/scatter 行为时，优先修改对应 trait（`client.rs`）的最小必要表面，再在本文件所有相关实现中同步：完整行为通常落在 `MockPDClientForSplit`，简单成功替身落在 `FakePDClient`，算法级 `SplitClient` 行为则需要同时检查 `TestClient`、`TestClientMut` 与 `FakeSplitClient`。不要用无条件成功桩替代 Go 中可观察的错误、计数或状态转换。

修改 region 分裂时重点维护三个不变量：输入 key 的 raw/encoded 模式、原 region ID 保留给 origin、`RegionTree` 与 ID map 同步。修改扫描时必须统一说明空 end、零/负 limit、空成功结果和 FIFO 错误注入的语义。修改 hook 时必须继续在锁外执行用户闭包。

测试必须放在独立文件，不能内嵌到本源文件。最直接的同步位置是 `mock_pd_client_test.rs`；若影响 `Client` 的重试/兼容策略，应扩展 `client_test.rs`；若影响批量 split、scatter 或等待 operator，应扩展 `split_test.rs`；公开 Go/Rust 契约变化还应更新 `parity_test.rs` 并核对 `client_test.go`、`split_test.go` 的对应场景。兼容风险集中在错误文本/分类、空 end 解释、hook 消费方式和计数时机；性能风险主要是扩大持锁区或在锁内引入回调。

## 验证依据

- Rust 源：`br/pkg/restore/split/mock_pd_client.rs`，核对了 `RegionTree`、`TestClient{,Mut}`、`MockPDClientForSplit`、`MockPDInner`、`FakePDHTTPClient`、`FakePDClient`、`FakeSplitClient` 的完整定义及 trait 实现。
- crate 边界：`br/pkg/restore/split/Cargo.toml` 与 `br/pkg/restore/split/lib.rs`，确认 library 名称、入口、依赖、模块公开方式及独立测试挂载位置；该目录没有 `doc.go`。
- Rust 独立测试：`br/pkg/restore/split/mock_pd_client_test.rs`、`client_test.rs`、`split_test.rs`、`parity_test.rs`，确认直接构造点、错误/空结果注入、split hijack、scatter/operator 脚本及边界断言。
- Go 对照：`br/pkg/restore/split/mock_pd_client.go`、`client_test.go`、`split_test.go`，确认同名 mock 的来源语义与 Rust 当前裁剪范围。
- RustCodeGraph：执行了 `status`、`files --filter br/pkg/restore/split`、目标文件 `explore`、按文件分段 `node`、`query MockPDClientForSplit --json` 以及构造函数 callers/callees 查询；图确认目标文件共 1501 行、由 22 个文件使用，并定位 Rust/Go 同名符号。精确 callers 输出未展开时，以 `rg` 对 Rust 构造函数和配置方法调用点补齐。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核未把 mock 行为描述成生产 PD 能力。
