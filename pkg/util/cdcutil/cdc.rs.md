# `pkg/util/cdcutil/cdc.rs`

## 文件定位

该文件是 `astersql-util-cdcutil` crate 的核心实现，负责从 etcd 的 TiCDC 元数据中枚举 changefeed，并依据运行状态及安全时间戳判断哪些 changefeed 仍在运行或可能与恢复操作不兼容。crate 入口 `pkg/util/cdcutil/lib.rs` 通过 `pub mod cdc` 和 `pub use cdc::*` 公开本文件的 API；依赖边界由 `pkg/util/cdcutil/Cargo.toml` 定义。

当前 Rust 生产接线可在 `lightning/pkg/importer/stubs.rs` 的 `etcd::Client` 与 `streamhelper::GetCDCPiTRStatus` 找到：前者实现本文件的 `KvClient`，后者调用 `GetRunningChangefeeds` 做 Lightning 的 CDC/PiTR 预检查。仓库内未检索到 Rust 生产代码调用 `GetIncompatibleChangefeedsWithSafeTS`；它当前主要由独立 Rust 测试验证，并与 Go 的 BR 流恢复检查入口保持语义对齐。

## 核心职责

1. 用 `KvClient` 抽象出巡检所需的最小 etcd range 能力，使检查逻辑既能接入真实元数据客户端，也能被内存测试客户端复用。
2. 用 `EtcdHttpClient` 提供一个具体的 etcd v3 JSON gateway 适配器，处理 range 请求、前缀上界和 base64 编解码。
3. 从 `/tidb/cdc/` 前缀扫描中识别 TiCDC ≤ v6.1 的旧路径和 ≥ v6.2 的 cluster/namespace 路径，并忽略元数据、迁移备份键及不合法 cluster 名称。
4. 读取 changefeed info/status，计算 `max(checkpoint-ts, start-ts)` 作为有效进度；finished、已删除或未知状态使用哨兵值排除。
5. 将命中的 changefeed 按 `cluster/namespace` 聚合为 `CDCNameSet`，供上层判空及生成用户提示。

这些职责分别落在 `CheckCDCClient::load_changefeeds`、`checkpoint_ts_for`、`get_incompatible`、`CDCNameSet` 及两个公开查询函数中。

## 主要符号

- `CDCPrefix`、`ChangefeedPath`、`CDCPrefixV61`：TiCDC etcd 键结构常量。当前解析直接使用前两个；`CDCPrefixV61` 保留为公开兼容常量。
- `INVALID_TS = u64::MAX`：finished、扫描后被删除及未知状态的哨兵 checkpoint；也作为 `GetRunningChangefeeds` 的 `safe_ts`，使所有有效非 finished changefeed 满足“小于 safe-ts”。
- `KeyVersion::{Legacy, Namespaced}` 与 `Changefeed`：记录 changefeed id、cluster、namespace 和键版本；`info_key`、`status_key` 根据版本构造精确键。
- `join_path`、`cut`、`prefix_range_end`：分别负责路径规范化、字节分隔和 etcd 前缀 range 上界计算。`prefix_range_end` 从末尾寻找可递增字节并截断；全为 `0xff` 时返回 `[0]`。
- `KvPair`、`GetOptions`、`KvClient::get`：生产适配器和测试替身共享的最小异步 KV 协议。`GetOptions` 只表达前缀扫描与 keys-only 两项需求。
- `CdcError`：区分抽象 KV 错误、HTTP、base64、JSON 错误，以及带 changefeed 上下文的 `CheckChangefeed` 包装错误。
- `EtcdHttpClient::new` 与其 `KvClient` 实现：向 `{endpoint}/v3/kv/range` 发 JSON 请求，并将 gateway 返回的 base64 键值解码成 `KvPair`。
- `CheckCDCClient`：内部巡检器；`load_changefeeds` 枚举，`fetch_checkpoint_ts_from_status` 读 status，`checkpoint_ts_for` 解释状态，`get_incompatible` 过滤并聚合。
- `ChangefeedInfoView`、`ChangefeedStatusView`：只反序列化判断所需字段；缺失 `start-ts` 或 `checkpoint-ts` 时因 `serde(default)` 取 0。
- `CDCNameSet::{Empty, MessageToUser, changefeed_names}`：提供判空、Go 风格用户消息以及排序后的扁平名称。`changefeed_names` 是公开辅助能力，`TESTGetChangefeedNames` 则在 `export_for_test.rs` 中仅为测试追加。
- `GetRunningChangefeeds`：以 `INVALID_TS` 调用统一过滤流程，返回所有有效、非 finished 的 changefeed。
- `GetIncompatibleChangefeedsWithSafeTS`：返回有效 checkpoint 严格小于给定 `safe_ts` 的 changefeed。

## 执行流程

公开入口最终都进入 `CheckCDCClient::get_incompatible`：

1. `load_changefeeds` 以 `prefix=true, keys_only=true` 扫描 `CDCPrefix`。
2. 每个键先从 `CDCPrefix.len() - 1` 位置保留分隔斜杠，再用 `ChangefeedPath` 切分路径前缀和 changefeed id。
3. 切分前缀为空时识别为旧版键；否则必须以 `/` 开头、包含 `cluster/namespace` 两段，且 cluster 满足 `CLUSTER_NAME_RE`。缺 namespace 的 `__backup__`/cluster-only 键以及其他噪声被跳过。
4. 对每个候选调用 `checkpoint_ts_for`。info 键已在扫描后删除时返回 `INVALID_TS`；JSON 中 state 为 `finished` 也返回该哨兵。`failed`、`running`、`warning`、`normal`、`stopped`、`error` 会继续读取 status，并取 status checkpoint 与 info start-ts 的较大值。未知状态记录 warning 后忽略。
5. 有效 checkpoint 仅在 `checkpoint < safe_ts` 时加入 `CDCNameSet`，等于 safe-ts 不算不兼容。
6. `GetRunningChangefeeds` 传入最大 `u64`，因此所有普通有效时间戳都会入选，而哨兵值本身不会入选；`GetIncompatibleChangefeedsWithSafeTS` 则执行实际安全时间边界筛选。

真实 HTTP 适配器的 `get` 另有一条下游流程：将 key 和可选 `range_end` 做 base64 编码，POST 到 `/v3/kv/range`，检查 HTTP 状态并解析 JSON；keys-only 请求无论响应 value 内容如何都输出空 value，否则再做 base64 解码。

## 数据与状态

文件自身不保存跨调用的可变业务状态。`CheckCDCClient` 只借用一个 `KvClient`；每次查询新建 `Vec<Changefeed>` 与 `CDCNameSet`。`CDCNameSet` 内部是 `HashMap<String, Vec<String>>`：新版键以规范化的 `cluster/namespace` 为桶名，旧版统一使用 `<nil>`。

`MessageToUser` 直接迭代 `HashMap`，因此多个 namespace 的展示顺序不保证稳定；每个桶内的 changefeed 顺序来自 KV 客户端返回顺序。用于断言/工具消费的 `changefeed_names` 会展平后排序，结果稳定。解析键和 JSON 时使用拥有所有权的 `String`/`Vec<u8>`；非 UTF-8 路径片段通过 `String::from_utf8_lossy` 替换非法字节，而不是报错。

状态判断的重要不变量是：有效进度不得早于 `start-ts`，所以必须取 `max(status checkpoint, info start)`；status 缺失或 value 为空视为 checkpoint 0，但仍可能由 start-ts 抬高。info 缺失则认为 changefeed 已删除，不再阻塞上层操作。

## 依赖与调用关系

下游依赖由 `pkg/util/cdcutil/Cargo.toml` 直接声明：`async-trait` 支持异步 trait，`reqwest`（rustls + JSON）访问 etcd gateway，`base64` 编解码 gateway 字段，`serde`/`serde_json` 处理请求响应和 TiCDC JSON，`regex` 校验 cluster 名，`path-clean` 对齐 Go `path.Join` 的路径清理，`thiserror` 建立错误链，`log` 输出忽略与命中信息。`tokio` 只在 dev-dependencies 中供独立异步测试使用。

RustCodeGraph 对目标文件列出 54 个符号，并把 `lightning/pkg/importer/stubs.rs` 标为使用者。该文件中 `etcd::Client` 将 `astersql_metaservice::NamespacedEtcdClient::get` 适配成 `KvClient::get`，把底层错误转成 `CdcError::KvRequest`；`streamhelper::GetCDCPiTRStatus` 再同步等待 `GetRunningChangefeeds`，以 `!names.Empty()` 汇报是否存在 CDC 任务。`lightning/pkg/importer/Cargo.toml` 通过路径依赖接入本 crate。

Go 上游调用显示两类用途：`pkg/executor/importer/precheck.go` 和 `lightning/pkg/importer/precheck_impl.go` 使用 `GetRunningChangefeeds` 阻止与 CDC/PiTR 冲突的导入；`br/pkg/task/stream.go::checkIncompatibleChangefeed` 使用 `GetIncompatibleChangefeedsWithSafeTS`，在流恢复前要求移除 checkpoint 落后于 backup TS 的 changefeed。后者是 Rust API 的语义依据，但不是当前已发现的 Rust 生产调用边。

## 错误处理与边界

前缀扫描请求失败会原样返回 `CdcError`，因为此时还没有具体 changefeed。逐个读取 info/status 或解析 JSON 失败，则由 `get_incompatible` 包装成 `CdcError::CheckChangefeed`，错误文本包含 `Changefeed` 的 Debug 信息；`migration_propagates_json_errors_with_changefeed_context` 验证了损坏 JSON 会同时暴露检查失败和 changefeed id。

`EtcdHttpClient` 用 `error_for_status` 把非成功 HTTP 状态变为 `CdcError::Http`；请求/响应 JSON 错误也沿同一路径传播。gateway 字段的非法 base64 变为 `CdcError::Base64`。抽象适配器无法映射到这些具体错误时可使用 `CdcError::KvRequest(String)`，Lightning 适配器即采用此分支。

有意忽略而非报错的边界包括：不包含 `ChangefeedPath` 的键、缺少前导斜杠或 namespace 的新版候选、非法 cluster 名、未知 state、finished state，以及扫描后被删除的 info。缺失/空 status 视为 0。代码在计算切片起点前检查 `kv.key.len() < CDCPrefix.len() - 1`，避免短键越界；但它信任前缀查询不会返回同长度以上却完全不相关的键，后续路径模式仍会过滤大部分噪声。

## 并发与资源生命周期

`KvClient: Send + Sync` 允许实现跨线程共享；所有网络/KV 操作都是异步的。不过 `get_incompatible` 对 changefeed 逐个 `await`，不会并发读取 info/status，因此请求数量约为一次前缀扫描，加每个候选一次 info，并为活跃状态候选再加一次 status。这样保持简单的错误定位和与 Go 顺序流程一致，但大量 changefeed 时延迟线性增长。

`EtcdHttpClient` 持有可克隆的 `reqwest::Client`，连接池随其 clone 共享；本文件不显式关闭连接或启动后台任务。`CheckCDCClient` 只在公开函数调用期间借用客户端。测试 `TestEtcdClient` 使用 `tokio::sync::RwLock<BTreeMap<...>>` 模拟并发安全 KV，但生产逻辑在一次检查中没有共享可变状态、锁或通道，也不创建任务。

调用方负责外部客户端生命周期。Go 的 importer 调用点用 `defer Close`；Rust Lightning 适配器拥有 metaservice 客户端，并在其上层流程中管理关闭。本文件既不接收 cancellation token，也不设置 HTTP timeout；取消和超时能力取决于传入的 `KvClient` 实现或 `reqwest::Client` 的外部配置，而 `EtcdHttpClient::new` 使用默认客户端配置。

## 与 Go 版本的对应关系

`pkg/util/cdcutil/cdc.rs` 基本按 `pkg/util/cdcutil/cdc.go` 的同名结构移植：常量、两种 `keyVersion`、changefeed 路径构造、扫描过滤、状态集合、checkpoint/start-ts 取最大值、`invalidTs` 哨兵、`CDCNameSet` 分组和两个公开入口均一一对应。`pkg/util/cdcutil/cdc_test.rs` 也复现了 `cdc_test.go` 的 safe-ts 42/40/48、finished/failed、缺 status、新旧路径和 `__backup__` 噪声场景。

主要实现差异如下：

- Go 直接依赖 `clientv3.Client` 和 `context.Context`；Rust 抽出通用 `KvClient`，并额外提供 `EtcdHttpClient` JSON-gateway 实现。Rust API 没有显式 context 参数。
- Go 遇到非法 `keyVersion` 会 panic；Rust 的 `KeyVersion` 是私有封闭 enum，`info_key`/`status_key` 的 match 穷尽，无非法变体分支。
- Go 的原始字节转字符串直接保留字节语义；Rust 对路径片段使用 lossy UTF-8 转换。
- Go 测试以 `ElementsMatch` 忽略输出顺序；Rust 的 `changefeed_names` 显式排序。面向用户的 `MessageToUser` 两侧都受 map 迭代顺序影响，格式保持 `found CDC changefeed(s): ...` 约定。
- Rust 增加 `migration_aster_unit_test.rs`，覆盖混合键版本、未知状态、JSON 上下文错误和真实 KV 错误文本；这些是迁移回归证据，不改变 Go 语义。
- Go 的 `GetIncompatibleChangefeedsWithSafeTS` 已接入 BR `checkIncompatibleChangefeed`；当前 Rust 仓库没有找到等价生产调用，应视为尚未完成的上层接线，而非本文件缺少过滤实现。

## 扩展指南

- 新增 TiCDC state 时，应修改 `CheckCDCClient::checkpoint_ts_for` 的状态 match，并在独立的 `pkg/util/cdcutil/cdc_test.rs` 或 `migration_aster_unit_test.rs` 增加该状态的 info/status 与 safe-ts 边界用例；同时核对 Go `checkpointTSFor`，避免两侧状态集合漂移。
- 支持新的 etcd 键布局时，应优先扩展 `KeyVersion`、`Changefeed::{info_key,status_key}` 和 `load_changefeeds`，覆盖旧版、新版、新布局共存及噪声键。不要把测试嵌入生产源文件。
- 修改 checkpoint 判定时必须保持严格比较 `checkpoint < safe_ts`、`max(checkpoint, start-ts)`、finished/删除哨兵三项契约，并同步 Go 对照测试的 40/42/48 边界。
- 扩展真实 KV 传输能力时可增加 `GetOptions` 字段或新的 `KvClient` 实现；需同步 `EtcdHttpClient`、Lightning 的 `etcd::Client` 适配器以及两个内存测试客户端。若引入分页，还必须确保扫描完整性，不能把 etcd 单页响应误当全量。
- 若需要取消、超时、认证或 mTLS，最合适的接入点是 `KvClient` 的调用契约及 `EtcdHttpClient` 构造配置；这属于跨调用方 API 变更，应评估 Lightning 和未来 BR 接线的兼容性。
- 若要并发读取大量 changefeed，可在 `get_incompatible` 引入有界并发，但必须保留单个 changefeed 的错误上下文、避免无界请求冲击 etcd，并决定结果排序/首错语义。当前串行行为是性能风险基线。
- 修改用户消息格式时要同步 Go 的 `CDCNameSet.MessageToUser` 及依赖该文本的上层错误/测试；若要求确定性展示，应对 namespace 与桶内 id 排序，而不能依赖 `HashMap` 顺序。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/cdcutil` 列出本模块的 Go/Rust 源及测试。
- RustCodeGraph 源码/符号证据：`node --file pkg/util/cdcutil/cdc.rs`（完整 470 行、54 个符号）；目标文件的使用者列表包含 `lightning/pkg/importer/stubs.rs`。对同名 Go/Rust 公开函数执行 `query` 得到两种语言各自定义；通用 `callers` 查询未在限定时间内返回可用结果，因此生产调用关系另以精确引用检索和调用点源码核实。
- 已核对 Rust 源与边界：`pkg/util/cdcutil/cdc.rs`、`pkg/util/cdcutil/lib.rs`、`pkg/util/cdcutil/Cargo.toml`、`lightning/pkg/importer/stubs.rs`、`lightning/pkg/importer/Cargo.toml`。
- 已核对独立 Rust 测试：`pkg/util/cdcutil/cdc_test.rs`、`pkg/util/cdcutil/migration_aster_unit_test.rs`、`pkg/util/cdcutil/export_for_test.rs`。
- 已核对 Go 对照及调用点：`pkg/util/cdcutil/cdc.go`、`pkg/util/cdcutil/cdc_test.go`、`pkg/executor/importer/precheck.go`、`lightning/pkg/importer/precheck_impl.go`、`br/pkg/task/stream.go`。
- 精确引用检索确认：Rust 生产引用 `GetRunningChangefeeds` 位于 `lightning/pkg/importer/stubs.rs`；未发现 Rust 生产引用 `GetIncompatibleChangefeedsWithSafeTS`。Go 的两个公开入口分别服务导入预检查与 BR 流恢复兼容性检查。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查文档存在且恰好包含 11 个固定二级标题，并人工复核所有“已接线”表述均有上述源码依据。
