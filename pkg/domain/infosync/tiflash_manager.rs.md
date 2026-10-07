# `pkg/domain/infosync/tiflash_manager.rs`

## 文件定位

本文件属于 `astersql-domain-infosync` crate，由 `pkg/domain/infosync/lib.rs` 以 `mod tiflash_manager` 装配并全量重导出。它定义 TiFlash 副本管理抽象、两个内存实现、PD placement rule 构造工具以及测试用 TiFlash/PD 替身。上层 `pkg/domain/infosync/info.rs` 把这些能力包装为全局 InfoSyncer API，DDL 和 GC 等路径再通过这些 API 配置或删除 TiFlash 规则。

需特别区分“接口意图”与“当前 Rust 接线”：Go 版 `TiFlashReplicaManagerCtx` 调用真实 PD HTTP 和 TiFlash status HTTP；Rust 版同名类型只维护内存 map/list，而且 `GlobalInfoSyncerInit` 当前始终将 `mockTiFlashReplicaManagerCtx` 注入 `InfoSyncer.tiflashReplicaManager`，即使传入了 `PdHttpClient` 也不会切换到真实 TiFlash 管理器（`info.rs:163-224`）。

## 核心职责

- 用 `TiFlashReplicaManager` 统一规则 CRUD、规则组配置、批量加速调度、Region/Store 查询、副本进度计算与进度缓存。
- 用 `makeBaseRule` / `MakeNewRule` / `MakeRuleID` 生成符合 TiFlash learner 约束的 placement rule，包括表 record key 范围、副本数与 location labels。
- 用 `encodeRule` / `encodeRuleID` 在 keyspace-aware 模式下改写规则键范围和 ID。
- 用 `getTiFlashPeerWithoutLagCount` / `calculateTiFlashProgress` 从传入 Store 的标签数据估算完整副本进度和至少一副本的覆盖进度。
- 提供 `MockTiFlash` 和 `mockTiFlashReplicaManagerCtx`，让 Rust 单元测试可观察规则写入、删除、分区加速和 Store 列表。这部分不是真实网络服务；`setUpMockTiFlashHTTPServer` 当前为空实现。

## 主要符号

- `type TiFlashRule = placement::pd::Rule`：统一使用 `ddl-placement` 定义的 PD 规则数据结构。
- `TiFlashReplicaManager: Send + Sync`：上层只依赖该 trait。`SyncTiFlashTableSchema` 和 `Close` 有成功/空操作默认实现，因此实现者可以不提供真实 schema 同步或资源回收。
- `TiFlashReplicaManagerCtx`：持有 `rules`、`tiflashProgressCache` 和 `stores` 三个 `RwLock` 容器的内存实现。`SetPlacementRule` 以 `Count == 0` 表示删除，否则按 rule ID 插入或覆盖；`GetRegionCountFromPD` 固定返回 1，`SetTiFlashGroupConfig` 和加速调度为空操作。
- `encodeRule(codec, rule)` / `encodeRuleID(codec, ruleID)`：当 `Codec.keyspace_aware_rules` 为真时，在 `StartKey`/`EndKey` 前插入 `keyspace_id` 的 4 字节大端表示，并把 ID 改为 `keyspace-<id>-<ruleID>`。该函数会原地变更传入规则。
- `getTiFlashPeerWithoutLagCount(table_id, stores)`：仅读取状态为 `Up` 或 `Disconnected` 的 Store，将 `table-<id>-regions` 标签解析为逗号分隔的 Region ID；返回标签项总数（peer 数）和去重 Region 数。
- `calculateTiFlashProgress(table_id, replica_count, stores)`：返回 `(full, one)`。副本数为 0 时返回 `(1, 1)`；无可见 Region 时返回 `(0, 0)`；其余情况按 peer/region/期望副本数求比例，两个值都以 1 为上限。
- `makeBaseRule()`：产生 group=`TiFlashRuleGroupID`、index=`RuleIndexTiFlash`、role=`Learner`、count=2，且带 `engine In [tiflash]` 约束的模板。
- `MakeNewRule(id, count, locationLabels)`：用 `table_record_prefix(id)` 作起点，用 `table_prefix(id + 1)` 作终点，生成半开区间 `[table record prefix, next table prefix)`。`table_prefix` 通过翻转 `i64` 符号位并大端编码，对齐 Go `tablecodec` 的 memcomparable 表 ID 编码。
- `MockTiFlash`：保存规则组 index、各表同步/加速状态、Store 元数据、全局 placement rules 及 PD/网络开关。`HandleSetPlacementRule` 会校验 `table-<id>-r` ID，并把表状态的 Region 设为 `[1]`。
- `mockTiFlashReplicaManagerCtx`：持有可选 `Arc<MockTiFlash>` 和独立进度缓存，将 trait 操作转发给 mock；未注入 mock 时多数写操作成功空返回，但 `GetPlacementRule` 和 `GetStoresStat` 会返回错误。
- `isRuleMatch`：只比较键范围、Count、LocationLabels、Learner role 和 TiFlash engine 约束，故意不比较 ID、GroupID 和 Index，与 Go mock 的语义一致。
- `MockTiFlashError`：简单字符串错误，实现 `Display` 和 `std::error::Error`。

## 执行流程

1. DDL/清理路径调用 `info.rs` 中的 `ConfigureTiFlashPDForTable`、`ConfigureTiFlashPDForPartitions` 或 `DeleteTiFlashPlacementRules`。RustCodeGraph 显示这三个函数都会调用本文件的 `MakeNewRule`。
2. `MakeNewRule` 从 `makeBaseRule` 取得 TiFlash learner 模板，设置 `table-<id>-r` ID、表 record 键范围、Count 和 location labels。删除流程也生成规则，但把 Count 设为 0。
3. `info.rs` 从全局 `InfoSyncer` 取得 `Arc<dyn TiFlashReplicaManager>`，调用 `SetPlacementRule` 或 `SetPlacementRuleBatch`。当前全局初始化实际选用 `mockTiFlashReplicaManagerCtx`。
4. mock 上下文如已通过 `SetMockTiFlash` 注入实例，会转发到 `HandleSetPlacementRule[Batch]`；后者以 Count 判断增删规则，解析表 ID，并初始化同步状态。未注入时，写路径静默成功。
5. 分区配置在批量写规则后，如 `accel` 为真则调用 `PostAccelerateScheduleBatch`；mock 逐表设置 `SyncStatus[table_id].Accel = true`。
6. 进度路径由 `info.rs::CalculateTiFlashProgress` 转发到 trait。两个 Rust 实现都调用本地 `calculateTiFlashProgress`，它基于传入 Store 的 `table-<id>-regions` 标签统计，不会访问 PD 或 TiFlash HTTP。
7. 关闭路径 `info.rs::CloseTiFlashManager` 调用 trait 的 `Close`；Rust 实现未覆盖该默认空方法，因此当前无实际资源释放。

## 数据与状态

- placement rule 的核心不变量是：TiFlash group、Learner role、`engine=tiflash` 标签约束，以及不跨越下一表前缀的半开 key range。`MakeRuleID` 是规则 ID 的唯一构造入口，格式为 `table-<id>-r`。
- `TiFlashReplicaManagerCtx.rules` 和 `MockTiFlash.GlobalTiFlashPlacementRules` 都按 rule ID 存储，但二者是独立状态；前者不会自动同步到 mock。
- 两个 manager context 都有各自的 `HashMap<i64, f64>` 进度缓存。缓存只提供更新、读取、单表删除和整体清空，没有 TTL、容量上限或持久化。
- `MockTiFlash.SyncStatus` 的 `Regions` 表示测试中已同步 Region，`Accel` 表示是否收到加速调度。`ResetSyncStatus(table, true)` 更新已有项时只替换 Regions，保留原有 Accel。
- `PdEnabled=false` 使 `HandleSetPlacementRule` 直接成功返回而不修改规则和同步状态。`NetworkError` 在 Rust 中只能设置/读取字段，由于 mock HTTP server 未实现，当前不影响本文件中的任何请求。

## 依赖与调用关系

- crate 边界：`pkg/domain/infosync/Cargo.toml` 定义 `astersql-domain-infosync`；本文件直接使用 crate 重导出的 `Codec`、`Error`、`Result`、`StoreInfo`、`StoreMeta`、`StoresInfo` 和 `placement`。`placement` 来自 path 依赖 `astersql-ddl-placement`。Cargo 中的 tagged `tikv-client` 是该 crate 依赖，但本文件没有直接引用它。
- 上游 Rust 门面：`info.rs` 的 `SetTiFlashPlacementRule`、`DeleteTiFlashPlacementRules`、`GetTiFlashGroupRules`、`GetPlacementRule`、`GetTiFlashStoresStat`、`ConfigureTiFlashPDForTable`、`ConfigureTiFlashPDForPartitions`、`CalculateTiFlashProgress` 和缓存 API 都委托给该 trait。
- 应用主链：RustCodeGraph 显示 `MakeNewRule` 的 Rust 生产调用者是上述三个配置/删除门面；Go 图进一步显示 `ConfigureTiFlashPDForTable/Partitions` 被 create table、set TiFlash replica、truncate table/分区等 DDL 路径使用，而 `DeleteTiFlashPlacementRules` 也被 GC worker 使用。Rust 这些更远的生产接线不应仅根据 Go 调用图推定为已移植。
- 下游只是内存容器与 placement 数据结构：当前 Rust 文件不调用 `PdHttpClient`、不调用 TiFlash status HTTP，也没有异步 runtime 依赖。
- 测试调用者：`tiflash_manager_test.rs` 直接覆盖 key range、Reset 状态、group 查询和规则比较；`info_test.rs::test_tiflash_manager` 通过全局门面验证 CRUD、Store、删除、整表/分区配置和加速状态。

## 错误处理与边界

- 内存 manager 查不到规则时返回 `Error::External("placement rule not found")`；mock context 未找到规则时返回 `Error::External("not implemented")`，未注入 `MockTiFlash` 却查询 Store 时返回 `"MockTiFlash is not accessible"`。错误文本并非结构化稳定 API。
- `HandleSetPlacementRule` 要求 ID 严格符合 `table-<i64>-r`；keyspace 编码后的 ID 不能被该 mock 解析，会返回 `Can't parse rule`。当前全局 mock 路径也没有调用 `encodeRule`。
- 所有 `std::sync::{Mutex,RwLock}` 通过 `unwrap()` 获取；若其他持锁线程 panic 导致锁中毒，后续调用将 panic，而不是返回 `Result` 错误。
- 批量写入是逐条执行，遇到首个错误立即返回，没有事务或回滚；此前规则可能已经落入内存。
- `table_prefix(id + 1)` 在 `id == i64::MAX` 时存在整数溢出边界（debug 构建 panic，release 构建环绕）；现有入口没有显式拒绝该 ID。
- 进度计算的 Rust 语义与 Go 差异较大：Rust 以 Store label 里出现的去重 Region 数作分母，不调用 PD 取总 Region 数，也不传播 HTTP/取消错误。因而其结果只是当前可见标签的比例，不等价于 Go 生产进度。

## 并发与资源生命周期

- manager 状态均用 `RwLock` 实现多读单写；`MockTiFlash.groupIndex` 用 `Mutex`。对 map 的方法通常在单次锁保护内克隆快照或完成更新，返回值不携带锁 guard。
- `mockTiFlashReplicaManagerCtx` 先持有自身 `tiflash` 的读锁，再进入 `MockTiFlash` 内部锁。本文件中没有反向从 `MockTiFlash` 回调 context 的路径，因此当前看不到锁顺序环；扩展回调时必须保持这一点。
- `Arc<MockTiFlash>` 提供共享所有权；`SetMockTiFlash` 直接替换当前实例，旧实例在最后一个 `Arc` 释放后销毁。
- trait 要求 `Send + Sync`，但所有方法均是同步调用，本文件不创建线程、task、channel 或 timer。
- `Close` 是默认空实现，`setUpMockTiFlashHTTPServer` 也是空实现，因此当前 Rust 没有与该管理器绑定的 socket/后台线程需要回收。如后续移植 Go mock HTTP server，必须同步覆盖 `Close`。

## 与 Go 版本的对应关系

- trait 方法集、`makeBaseRule`、`MakeNewRule`、`MakeRuleID`、mock 类型和缓存 API 与 `pkg/domain/infosync/tiflash_manager.go` 同名符号一一对应。Rust 独立测试证明表 record/下一表键前缀编码与 Go `tablecodec` 意图对齐。
- Go `TiFlashReplicaManagerCtx` 持有 `pd.Client` 和 TiKV codec：写规则前确保 rule group，对 keyspace 编码，通过 PD HTTP 增删查规则、加速调度、查 Region/Store，并向各 TiFlash Store 同步 schema。Rust `TiFlashReplicaManagerCtx` 未移植这些网络行为，只是内存容器。
- Go `calculateTiFlashProgress` 以 PD 报告的总 Region 数作分母，逐 TiFlash Store 通过 HTTP 收集无滞后 peer，区分存活和离线 Store 的错误，并保留 context 取消错误。Rust 仅依赖预先填入的 label，对无 Region 返回成功 `(0,0)`，对 `replica_count==0` 返回 `(1,1)`；这是简化语义，不是 Go 生产实现的完整移植。
- Go `MockTiFlash` 启动 `httptest.Server`，模拟 sync-status/config/accelerate/region-stats/stores 等 HTTP 路由，还有 delay、not-available 和 network-error 行为。Rust 保留了部分字段和直接 handler 语义，但 HTTP server 为空占位，不支持延迟/可用性模拟。
- Go mock 的 `GetPlacementRule` 固定返回 `not implemented`；Rust mock context 已能从注入的 `MockTiFlash` 按 ID 读取规则，只在缺失时返回该错误。这是一处可观察的语义差异。
- Go `HandleGetGroupRules` 同样忽略 group 参数；Rust 测试 `mock_group_rule_lookup_matches_go_unfiltered_contract` 显式锁定这一兼容行为。

## 扩展指南

- 若移植真实 PD 实现，应从 `TiFlashReplicaManagerCtx` 入手：注入 PD HTTP client 与 codec，对齐 Go 的 group 预置、keyspace 编码、批量 add/delete op、Region/Store 查询及 schema 同步；同时修改 `info.rs::GlobalInfoSyncerInit` 使有 PD client 时选择真实实现。不要只完善本类型而忘记全局接线。
- 若完善进度计算，应保留 Go 的两个分母概念：PD 总 Region 数与 TiFlash 已覆盖的去重 Region 数；同时实现 Store 状态分支、context/取消传播和超额 peer 截断。应扩展独立 `tiflash_manager_test.rs`，覆盖多 Store、重复 Region、离线 Store、网络错误、零 Region 和多副本。
- 若改变 rule key/ID 编码，必须同时检查 `encodeRule`、`encodeRuleID`、`table_prefix`、`table_record_prefix`、`MakeNewRule` 和 mock ID 解析，并在独立测试中加入 API V1/V2（keyspace-aware）对照。特别注意重复调用 `encodeRule` 会重复加前缀，设计时需明确“只编码一次”的所有权边界。
- 若扩展 mock，保持其与 Go 测试契约的意图，而不要让它反向定义生产语义。新增 HTTP server/后台线程时，应让 `Close` 可幂等地停止资源，并在 `tiflash_manager_test.rs` 而非生产文件内新增测试。
- 若改变分区或删除语义，除本文件外还必须同步检查 `info.rs::{DeleteTiFlashPlacementRules, ConfigureTiFlashPDForTable, ConfigureTiFlashPDForPartitions}` 及 `info_test.rs::test_tiflash_manager`。批量路径应明确部分成功时的处理契约。
- 性能风险主要在大批规则逐条锁定/克隆、`GetGroupRules` 全表扫描以及 Store label 中 Region 列表的字符串拆分。引入真实网络后还要评估超时、批量上限、重试和并发度，不能保留当前“静默成功”的 mock 边界作为生产行为。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点；`files --filter pkg/domain/infosync` 确认目标 Rust/Go/测试文件均已索引。
- 源码全貌：`rustcodegraph node --file pkg/domain/infosync/tiflash_manager.rs --offset 1 --limit 500` 和 `--offset 500 --limit 220`，覆盖该文件 658 行的 trait、两个 manager context、规则/进度工具与 mock。
- 符号/调用图：`query MakeNewRule`、`query calculateTiFlashProgress`、`query mockTiFlashReplicaManagerCtx`以及 `explore "MakeNewRule ConfigureTiFlashPDForTable ConfigureTiFlashPDForPartitions SetTiFlashPlacementRule DeleteTiFlashPlacementRules Rust callers"`。图证据显示 Rust `MakeNewRule` 被 `info.rs` 的整表配置、分区配置和删除入口调用，并列出相关 Rust 测试调用者。精确 `callers/callees` 命令本次未产生额外文本输出，因此没有用它推导更多边。
- Rust 入口与测试：读取 `pkg/domain/infosync/info.rs:1-260,600-879`、`pkg/domain/infosync/tiflash_manager_test.rs:1-62` 和 `pkg/domain/infosync/info_test.rs:1-250`。关键证据包括 `GlobalInfoSyncerInit` 当前固定注入 mock manager，以及规则 CRUD/分区加速的独立测试。
- Go 对照：通过 RustCodeGraph 读取 `pkg/domain/infosync/tiflash_manager.go:1-351,350-479,650-939`，核对真实 PD/TiFlash HTTP 行为、keyspace 编码、进度分母、mock HTTP 服务器和关闭语义。
- crate 与模块边界：读取 `pkg/domain/infosync/Cargo.toml` 和 `pkg/domain/infosync/lib.rs`，确认 crate 名、path 依赖、tagged `tikv-client` 依赖、模块重导出及独立测试文件装配。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令检查文档存在且恰好包含 11 个固定二级标题，并人工复核本文未将 Go 的完整生产能力写成 Rust 当前事实。
