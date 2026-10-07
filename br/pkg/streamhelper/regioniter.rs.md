# `br/pkg/streamhelper/regioniter.rs`

## 文件定位

本文件属于 `astersql-br-pkg-streamhelper` library crate；crate 的清单是 `br/pkg/streamhelper/Cargo.toml`，入口 `br/pkg/streamhelper/lib.rs` 通过 `#[path = "regioniter.rs"] pub mod regioniter` 挂载本模块，并在末尾用 `pub use regioniter::*` 将其公开 API 扁平导出。它位于日志备份 checkpoint 推进链的集群元数据边界：上游用它把任意键范围分页解析为连续的 TiKV Region，下游再依据每个 Region 的 leader 按 Store 收集 flush TS。

直接业务入口是 `CheckpointAdvancer::GetCheckpointInRange`（`br/pkg/streamhelper/advancer.rs`）：该函数构造 `IterateRegion(self.env.as_ref(), start, end)`，循环调用 `Done`/`Next`，并把每个 `RegionWithLeader` 交给 `ClusterCollector::CollectRegion`。同一文件定义的 `Store` 与 `TiKVClusterMeta` 也被 `br/pkg/streamhelper/flush_subscriber.rs`、`advancer_env.rs` 等模块复用，因此它不只是一个迭代器实现，也是 streamhelper 访问 PD/TiKV 拓扑、GC safe point 和 TSO 的公共抽象边界。

## 核心职责

1. 用 `TiKVClusterMeta` 抽象 Region 扫描、Store 枚举、GC 阻塞/解除和当前 TSO 获取，使推进器、订阅器与测试替身共享同一集群能力接口。
2. 用 `RegionIter` 把半开区间 `[startKey, endKey)` 分页扫描；`endKey == []` 表示扫描到键空间正无穷。
3. 在每一页交付给调用方之前，由 `CheckRegionConsistency` 验证首 Region 覆盖请求起点、末 Region 覆盖本页终点，并验证相邻 Region 无空洞或错位。
4. 对 `RegionScan` 或一致性校验产生的瞬时失败执行 `with_retry`：至多 8 次、每次失败后固定休眠 500 ms。
5. 用 `locateKeyOfRegion` 提供单键定位辅助：扫描 `[key, key_next(key))` 且限制为 1 条。

本文件不负责发起 flush TS RPC、合并 checkpoint、管理订阅线程或实现真实 PD 客户端；这些职责分别位于 `collector.rs`、`advancer.rs`、`flush_subscriber.rs` 和环境实现中。

## 主要符号

- `defaultPageSize: i32 = 2048`：默认单页 Region 上限，与 Go `defaultPageSize` 相同。
- `RegionWithLeader { Region, Leader }`：把 `stubs::Region` 与其 leader `stubs::Peer` 绑定。`ClusterCollector::CollectRegion` 读取 `Leader.StoreId` 选择 Store worker；leader ID 为 0 时把该 Region 范围记为失败子范围。
- `Store { ID, BootAt }`：Store 拓扑摘要。`FlushSubscriber::UpdateStoreTopology` 用 `ID` 建索引，用 `BootAt` 变化识别 Store 重启并重建订阅。
- `TiKVClusterMeta: Send + Sync`：公共集群接口。`RegionScan` 返回带 leader 的 Region；`Stores` 返回拓扑；`BlockGCUntil`/`UnblockGC` 操作服务 GC safe point；`FetchCurrentTS` 读取 TSO。`advancer_env::Env` 将该 trait 与日志备份服务、流元数据及锁解析能力组合。
- `RegionIter<'a>`：借用 `&'a dyn TiKVClusterMeta` 的有状态分页器。`startKey`/`endKey` 保存原范围，`currentStartKey` 是下一页游标，`infScanFinished` 记录是否见到无限末端，`PageSize` 可在调用 `Next` 前调整。
- `IterateRegion(cli, startKey, endKey) -> RegionIter`：复制传入边界，令 `currentStartKey = startKey`、`infScanFinished = false`、`PageSize = 2048`。
- `locateKeyOfRegion(cli, key)`：调用 `RegionScan(key, key_next(key), 1)`；空结果是错误，否则克隆首条结果。
- `CheckRegionConsistency(startKey, endKey, regions)`：页内连续性与覆盖性校验，是 `Next` 的关键防线，也作为公开符号被包级 parity 测试使用。
- `with_retry`：模块私有同步重试器，保留最后一次字符串错误；成功立即返回，连续 8 次失败后返回最后错误。
- `RegionIter::Next`：拉取、校验、推进游标的唯一变更状态操作。
- `RegionIter::Done`：无副作用的结束判断；有限区间在游标达到或越过 `endKey` 时结束，无限区间只看 `infScanFinished`。
- `Display for RegionIter`：打印当前范围、无限扫描完成标志和原始起点，供诊断使用；不会触发扫描。

## 执行流程

典型 checkpoint 推进流程如下：

1. `CheckpointAdvancer::tryAdvance` 先折叠重叠任务范围，再为每个范围调用 `GetCheckpointInRange`。
2. `GetCheckpointInRange` 用 `IterateRegion` 初始化游标，并在 `!iter.Done()` 时调用 `Next`。
3. `Next` 快照当前 `PageSize`、`currentStartKey` 和 `endKey`，然后进入 `with_retry`。每次尝试均调用 `cli.RegionScan(cur, end, page)`。
4. 若扫描得到非空页，代码取最后一个 Region 的 `EndKey` 作为本页实际校验上界，并调用 `CheckRegionConsistency(cur, last.EndKey, regions)`。这样允许页大小截断整个请求范围，同时要求这一页自身连续完整。
5. 若扫描结果为空，代码用原请求终点调用一致性检查并取得其错误，使“空页”进入相同重试路径，而不是被当作正常完成。
6. 成功后，`Next` 取末 Region 的 `EndKey`。空终点表示已经遇到键空间末 Region，故设置 `infScanFinished = true`；随后无论是否为空，都把 `currentStartKey` 推进到该终点并返回整页。
7. 调用方把页内每个 `RegionWithLeader` 交给 `ClusterCollector::CollectRegion`；有限范围在游标 `>= endKey` 时结束，无限范围在遇到空 `EndKey` 后结束。

单键定位不复用 `RegionIter`：`locateKeyOfRegion` 使用 `key_next` 构造最小非空半开区间并只取一条 Region，适合需要快速获得某键所属 Region 的路径。

## 数据与状态

所有键都以 `Vec<u8>`/`&[u8]` 按字节字典序比较。范围约定为半开区间 `[start, end)`；终点空字节串具有“+∞”语义，而起点空字节串表示键空间开头。因为两者都编码成空串，`RegionIter` 不能仅靠比较游标和终点判断无限扫描完成，必须维护独立的 `infScanFinished`。

`RegionIter` 拥有三个边界副本：`startKey` 仅用于保留原始输入和展示，`endKey` 在整个生命周期不变，`currentStartKey` 每次成功 `Next` 后才更新。失败尝试不会提前推进游标，因此调用方收到错误后不会丢页。`PageSize` 是公开可变配置；修改只影响下一次 `Next` 捕获的页大小。

`RegionWithLeader`、`Region`、`Peer` 和 `Store` 都是拥有数据的值类型；`locateKeyOfRegion` 会克隆首条结果。迭代器本身只借用 `TiKVClusterMeta`，因此不能比客户端活得更久，也不持有连接、锁或后台任务。

一致性不变量是：结果非空；首 Region 的 `StartKey <=` 请求起点；当末 Region 的 `EndKey` 非空时，其值必须 `>=` 校验终点；每一对相邻 Region 都满足前者 `EndKey ==` 后者 `StartKey`。最后一项同时排斥空洞和重叠/乱序边界。

## 依赖与调用关系

本文件的直接 Rust 依赖很小：标准库的 `Ordering`、`thread`、`Duration`，以及本 crate `stubs` 中的 `Peer`、`Region`、`key_next`。`Cargo.toml` 没有为本文件单独引入网络或异步运行时；真实集群访问完全留在 `TiKVClusterMeta` 实现边界之后。

主要上游关系：

- `advancer.rs::GetCheckpointInRange -> IterateRegion -> RegionIter::{Done, Next}`，是日志备份 checkpoint 扫描主链。
- `advancer.rs::importantTick`、`onTaskEvent` 等通过组合后的 `Env: TiKVClusterMeta` 使用 `BlockGCUntil`、`UnblockGC`、`FetchCurrentTS`。
- `flush_subscriber.rs::UpdateStoreTopology -> TiKVClusterMeta::Stores`，用 `Store` 对齐订阅集合。
- `advancer_env.rs::PDRegionScanner` 将五个集群方法逐一转发给其 `Arc<dyn TiKVClusterMeta>`；`Env` trait 则把本接口纳入推进器完整运行环境。

主要下游关系：

- `Next -> TiKVClusterMeta::RegionScan`，成功页再调用 `CheckRegionConsistency`。
- `locateKeyOfRegion -> key_next + TiKVClusterMeta::RegionScan`。
- `GetCheckpointInRange -> ClusterCollector::CollectRegion`；后者依据 `RegionWithLeader::Leader` 路由到 per-Store 收集线程。

RustCodeGraph 的 `explore "regioniter RegionIter"` 还显示 `IterateRegion` 的直接 Rust 调用包括 `advancer.rs::GetCheckpointInRange`、`regioniter_test.rs` 与 `subscription_test.rs`，`CheckRegionConsistency` 也由 `parity_test.rs` 直接校验；这些调用与源码入口互相印证。

## 错误处理与边界

`locateKeyOfRegion` 原样传播 `RegionScan` 的 `String` 错误；扫描成功但无结果时生成包含 key 调试值的新错误。`CheckRegionConsistency` 对空结果、左侧缺口、有限末端覆盖不足、相邻边界不相等分别返回可区分的文本，但没有结构化错误码。

`Next` 把扫描错误和一致性错误都视为可重试错误，固定执行最多 8 次。当前 `with_retry` 在每次失败后都休眠 500 ms，包括第 8 次最终失败之后，因此彻底失败的返回至少额外阻塞约 4 秒；独立测试只覆盖“首试失败、第二试成功”且确认等待至少约 500 ms。成功之后 `rs` 必为非空，所以随后索引末元素安全；这一安全性由闭包的空页错误分支保证。

有限范围若扫描返回的最后 Region 越过用户 `endKey`，一致性检查仍可通过；这是正确行为，因为 Region 是不可切分的物理覆盖单元，`Done` 会在游标达到或越过终点后停止。无限范围必须最终看到 `EndKey == []`，否则会继续翻页。

与 Go 相比，Rust 版本缺少 `context.Context` 取消/超时、PingCAP 类型化 `ErrPDBatchScanRegion`、日志告警、metrics 计数和 key redact 展示；它使用同步 `thread::sleep` 与普通 `Debug` 键输出。扩展错误或日志时应避免把敏感原始键无条件暴露，并评估是否需要恢复可取消重试语义。

## 并发与资源生命周期

`TiKVClusterMeta` 要求实现者 `Send + Sync`，允许同一环境被推进器、订阅器及收集器跨线程共享；但 `RegionIter` 自身没有内部锁，也未声明或提供并发调用协议。Go 源码同样明确 `PageSize` 可在每次 `Next` 前修改但不提供线程安全；Rust 的 `Next(&mut self)` 在类型层面阻止同一迭代器被普通安全代码同时推进。

`Next` 是同步阻塞调用，重试期间当前线程休眠。迭代器不创建线程、不持有通道、不拥有客户端，也没有显式清理步骤；借用生命周期保证 `cli` 在迭代期间有效。实际 per-Store worker 的创建、Sender 关闭和 join 位于 `ClusterCollector`，不属于本文件生命周期。

成功返回一页时状态更新顺序为“扫描与校验完成 → 判断无限末端 → 推进游标 → 返回页”；任何错误都在状态修改前返回。这个顺序是安全重试与调用方错误恢复的关键约束。

## 与 Go 版本的对应关系

Go 对照文件是 `br/pkg/streamhelper/regioniter.go`，Rust 独立测试对照 `br/pkg/streamhelper/regioniter_test.rs` 与 Go 的 `regioniter_test.go`。

保持一致的行为包括：默认页大小 2048；`RegionWithLeader`/`Store`/`TiKVClusterMeta` 的业务角色；初始化游标；单键扫描 limit=1；四类连续性校验；`Next` 用本页最后终点校验非空页；8 次、固定 500 ms 重试；空末 Region 标记无限扫描结束；有限区间使用 `currentStartKey >= endKey` 判定完成。Rust 测试复刻了局部范围、起点落在首 Region 内、10,000 Region 多页扫描、扫描到 +∞、从键空间开头扫描到有限终点等 Go 用例，并额外用 `next_uses_go_equivalent_retry_backoff` 锁定首次失败后的退避。

实现形态差异包括：Go 的 Region/Peer 是 protobuf 指针，Rust 使用拥有值；Go 方法接收 `context.Context`，Rust trait 无上下文；Go 返回带错误类别的 PingCAP error 并记录日志/指标，Rust 返回 `String`；Go 的 `utils.WithRetry` 接受上下文，Rust 是本地阻塞循环；Go 的 `String` 使用 key stringify/redact，Rust `Display` 使用 `Debug`。这些差异意味着核心分页语义已经移植，但可观测性、取消和错误分类并非完全等价，不能把文档中的“对齐”理解为所有运行环境能力都已等价。

## 扩展指南

- 修改分页或结束条件时，优先改 `RegionIter::Next`/`Done`，并同步独立文件 `br/pkg/streamhelper/regioniter_test.rs`；至少覆盖有限终点、空终点、跨多页、末页越过查询终点和错误后游标不推进。
- 修改连续性规则时，集中在 `CheckRegionConsistency`，补充空结果、首端缺口、末端缺口、相邻空洞、相邻重叠/乱序的定向测试，并核对 Go `regioniter.go` 是否需保持同样规则。
- 新增 `TiKVClusterMeta` 方法会影响所有生产/测试实现及组合 trait `advancer_env::Env`；应先用调用图枚举实现者，不要只更新 `PDRegionScanner`。接口变化还可能波及 `basic_lib_for_test.rs`、`parity_test.rs`、`collector_test.rs`、`advancer_test.rs` 与 utiltest 假集群。
- 调整 `RegionWithLeader` 或 `Store` 字段时，检查 `ClusterCollector::CollectRegion` 的 leader 路由和 `FlushSubscriber::UpdateStoreTopology` 的重启检测，不可只验证迭代器测试。
- 若恢复 Go 的取消、结构化错误、日志或 metrics，避免在本文件中引入一套与 crate 环境重复的客户端生命周期；优先把上下文/错误契约放在 trait 边界，并保留失败时不推进游标的不变量。
- 测试逻辑必须继续放在 `regioniter_test.rs` 等独立测试文件，不能内嵌到生产源文件。性能改动需关注默认每页 2048 条的内存复制，以及固定阻塞退避对推进器线程的延迟。

## 验证依据

本说明依据以下静态证据完成，未运行 Cargo（任务明确为纯文档分析）：

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；随后用 `node --file br/pkg/streamhelper/regioniter.rs` 阅读本文件 1–211 行，并用 `query`/`explore` 核对 `RegionWithLeader`、`TiKVClusterMeta`、`IterateRegion`、`Next`、`CheckRegionConsistency` 的调用关系。
- `br/pkg/streamhelper/regioniter.rs`：类型、常量、构造函数、一致性校验、重试、分页和结束条件的直接实现。
- `br/pkg/streamhelper/lib.rs` 与 `br/pkg/streamhelper/Cargo.toml`：模块挂载、公开再导出、crate 类型、Go package 映射和依赖边界。
- `br/pkg/streamhelper/advancer.rs`：`GetCheckpointInRange` 的迭代消费主链，以及 GC/TS 能力的业务使用。
- `br/pkg/streamhelper/collector.rs`、`flush_subscriber.rs`、`advancer_env.rs`：Region leader 路由、Store 拓扑用途、`Env` 组合及集群方法转发。
- `br/pkg/streamhelper/regioniter.go`：Go 原实现；逐项核对接口、页大小、一致性规则、重试、游标推进与完成条件。
- `br/pkg/streamhelper/regioniter_test.rs` 与 `regioniter_test.go`：局部/无限范围、万级分页和 Rust 退避行为的测试证据；`parity_test.rs` 提供包级公开契约的补充证据。

结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题。人工复核重点是：本文区分了当前 Rust 事实与 Go 完整能力，没有把同步字符串错误层描述成已具备 Go 的取消、日志、指标或类型化错误。
