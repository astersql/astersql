# `br/pkg/conn/conn.rs`

## 文件定位

`conn.rs` 是 Cargo 包 `astersql-br-pkg-conn` 的主要实现文件，由同目录的 [`lib.rs`](./lib.rs) 以 `#[path = "conn.rs"] pub mod conn` 装配并整体重导出。包元数据在 [`Cargo.toml`](./Cargo.toml) 中把它归入 Go 包 `br/pkg/conn` 的 Rust 移植库；当前依赖只有 BR errors/glue/version、公共 errors、`fail`、`serde` 和 `serde_json`，没有直接依赖真实 TiKV client、kvproto、grpcio 或 HTTP runtime。

该文件位于 BR 与集群控制面的连接边界：向上提供 `Mgr`、store 枚举/过滤、PD 时间戳、备份客户端句柄、TiKV 配置读取等 API；向下把 PD、HTTP、Storage、Domain、StoreManager、GC 和关闭动作抽象成 trait。源码顶部说明这是 Darwin arm64 下的精简边界，因此这里的“连接”多数是依赖注入接口，不应误解为文件自身建立了真实 gRPC/HTTP 客户端。

RustCodeGraph 的文件节点显示本文件被 58 个文件引用，示例包括 `br/pkg/restore/import_mode_switcher.rs`、`br/pkg/restore/internal/import_client/import_client.rs` 和 `br/pkg/restore/log_client/client.rs`。但精确查询 `conn.rs::NewMgr`、自由函数 `conn.rs::GetConfigFromTiKV` 和 `conn.rs::ProcessTiKVConfigs` 的 callers/callees 均返回空边；结合源码搜索可确认当前 Rust 生产树中还存在 `br/pkg/task/common.rs::NewMgr` 等独立抽象，不能据“同名”推断它们必然调用本文件的 `NewMgr`。本文件的构造主链目前最直接的可执行证据来自 [`parity_test.rs`](./parity_test.rs) 的 `go_rust_public_contract_matches`。

## 核心职责

1. 用 `StoreMeta` 取得 PD store 列表，并由 `GetAllTiKVStores` 按 `StoreBehavior` 实现“跳过 TiFlash、遇 TiFlash 报错、仅保留 TiFlash”三种策略；`GetAllTiKVStoresWithRetry` 再叠加取消、failpoint 和有限指数退避。
2. 用 `NewMgr` 依次创建 PD controller、执行可选集群版本检查、检查 store 列举、打开 `tikv://...` storage、验证存储类型、按需创建 Domain 并做 DDL 版本检查，最后组装 StoreManager、GC manager 与生命周期句柄。
3. 由 `Mgr` 集中持有 PD、Domain、Storage、StoreManager、GC 和生命周期资源，提供访问器、PD 时间戳组合、备份/日志备份客户端委派以及幂等关闭。
4. 从所有 Up 状态的 TiKV status 服务读取 `/config`，处理 TLS 前缀、容器/多网卡下 host 改写、IPv6、取消和网络重试；在此基础上聚合恢复并发、region split 参数和 log-backup 开关。
5. 在缺少完整平台依赖时，以 `NewMgrDeps` 及多个 trait 保留 Go API 的可观测调用顺序和错误契约，支持独立测试注入，而不伪造真实网络客户端。

## 主要符号

- 配置常量：`DefaultMergeRegionSizeBytes = 96 MiB`、`DefaultMergeRegionKeyCount = 960000`、`DefaultImportNumGoroutines = 36`、内部余量 `minRestoreConcurrencyOverImportThreads = 4`，以及 GC 空 keyspace 哨兵 `NullspaceID`。`units::{MiB, GiB}` 仅向测试暴露二进制单位。
- 枚举：`VersionCheckerType` 选择普通 BR、PiTR 或跳过版本检查；`StoreBehavior` 选择 TiFlash 过滤策略；`StoreState` 表达 Up/Offline/Tombstone；`GrpcCode` 是精简的 Ok/Canceled/Unknown 状态码。
- 数据类型：`Store`/`StoreLabel` 是 PD store 元数据投影；`StatusUrl` 负责 scheme、host、端口与路径；`ConfigTerm<T>`/`KVConfig` 保存值和用户是否显式修改；`HttpResponse` 保存状态码、body 和请求 URL；`BackupClient`/`LogBackupClient` 是平台中立句柄。
- 边界 trait：`CancelContext`、`HttpClient`、`StoreMeta`、`StoreManagerHandle`、`PdControllerHandle`、`GcManagerHandle` 和 `MgrLifecycleHandle` 分别隔离取消、HTTP、PD、RPC 客户端池、PD controller、GC 与关闭副作用。
- `Mgr`：核心聚合对象。`new_with_pd` 是测试辅助构造；`GetStorage`/`GetDomain`/`GetGCManager`/`SetGcManager` 暴露资源；`GetBackupClient`/`ResetBackupClient`/`GetLogBackupClient` 委派 StoreManager；`GetCurrentTsFromPD` 组合 TSO；`Close` 管理释放顺序。
- 构造入口：`NewMgrDeps` 保存五个可注入工厂或判定函数，`NewMgr` 使用它们构造 `Mgr`。`VersionPdAdapter` 把本地 `StoreMeta` 转成 version crate 所需的 `PdClient`。
- store 与错误入口：`GetAllTiKVStores`、`GetAllTiKVStoresWithRetry`、`CheckStoresAlive`、`status_code`；内部 `with_aggressive_retry`、`grpc_status` 和 `checkStoresAlive` 支撑重试与 Go 私有函数名对照。
- 配置入口：自由函数及成员方法 `GetConfigFromTiKV`、`Mgr::GetConfigBytesFromTiKV`、`ProcessTiKVConfigs`、`IsLogBackupEnabled`，以及 `HandleTiKVAddress`/`handleTiKVAddress`。
- 解析辅助：`parse_import_threads_from_config`、`parse_merge_region_size_from_config`、`parse_log_backup_enable_from_config` 和 `ram_in_bytes`。

## 执行流程

`NewMgr` 的顺序是不应随意调整的：

1. `deps.new_pd(pdAddrs, securityOption)` 创建 controller，并取得 `StoreMeta`。
2. `checkRequirements` 为真时，经 `VersionPdAdapter` 调用 `CheckClusterVersion`；普通模式使用 `CheckVersionForBR`，流恢复使用 `CheckVersionForBRPiTR`，`NoVersionChecker` 跳过。失败会补充 `--check-requirements=false` 提示。
3. `checkStoresAlive` 调用 `GetAllTiKVStores`。当前实现只统计 Up store 数量，没有对零存活节点报错。
4. 组装 `tikv://<pd-list>?disableGC=true&keyspaceName=<name>`，调用 `Glue::Open`；随后由 `deps.is_tikv_storage` 验证类型，否则返回 `ErrKVNotTiKV`。
5. `needDomain` 为真时调用 `Glue::GetDomain`，并在 Domain 创建后使用 `CheckVersionForDDL` 再做一次兼容检查。
6. 从 storage 的 `keyspace_id()` 得到 GC keyspace，创建 lifecycle、StoreManager 和 GC manager，记录 `Glue::OwnsStorage()`，返回 `Mgr`。

TiKV 配置读取链为：成员 `Mgr::GetConfigFromTiKV` 根据 StoreManager 的 `HasTLS` 选择 `http://` 或 `https://` → 自由函数 `GetConfigFromTiKV` 用 `SkipTiFlash` 重试取得 store → 仅处理 `StoreState::Up` → `HandleTiKVAddress` 解析并在 status host 与 node host 不同时保留 status 端口、改用 node host → 每个节点最多 8 次 GET `/config` → 成功响应交给调用者回调。网络 GET 失败会等待 5 ms 后重试；回调错误不重试并立即向上传播。

`ProcessTiKVConfigs` 在三项均为 `Modified` 时直接返回，否则逐节点解析 JSON。region size/key 以 key count 为判据选择更保守的一组；import concurrency 取当前值与 `num-threads + 4` 的较大值。读取或解析失败被有意吞掉，使调用者保留已经写入的值或默认值。`IsLogBackupEnabled` 则对所有被访问节点的 `log-backup.enable` 做逻辑 AND，读取/解析错误直接返回。

## 数据与状态

`Mgr` 中 `pd` 始终存在；`dom`、`storage`、`storeManager`、`gcManager`、`lifecycle` 均允许缺省，以支持精简平台和测试构造。`ownsStorage` 决定关闭时是否执行 Domain/owner/storage 链；`gc_keyspace_id` 是构造时从 storage 投影出的快照。`closed: Mutex<bool>` 只保护关闭幂等性，并让整个关闭序列串行执行。

`STORE_IS_TIKV: AtomicBool` 是进程级标记，`store_is_tikv()` 以 Relaxed 读取。当前 `NewMgr` 在 `Glue::Open` 和 `is_tikv_storage` 检查之前就把它置为 true，且没有回滚路径；因此它更接近“已进入 TiKV 构造路径”的粘性标记，而不是对当前成功构造对象的严格证明。新增依赖该标记的逻辑前必须先确认这一失败路径语义。

`CancelledContext` 用 `Arc<AtomicBool>` 在克隆之间共享取消状态，读写采用 `SeqCst`；`BackgroundContext` 永不取消。`ConfigTerm.Modified` 是配置优先级不变量：用户已修改的字段不应被远端 TiKV 值覆盖。`GetCurrentTsFromPD` 要求 physical/logical 均非负，然后按 TiDB oracle 的 18 位逻辑区组合为 `(physical << 18) | logical`。

## 依赖与调用关系

- `astersql-br-pkg-glue` 提供 `Glue`、`Storage`、`Domain` 和 `SecurityOption`；`NewMgr` 的 storage/domain 建立以及 GC keyspace 来源都依赖它。
- `astersql-br-pkg-version` 提供三个版本谓词和 `CheckClusterVersion`；`VersionPdAdapter` 是两个 crate 之间的元数据桥。
- `astersql-br-pkg-errors` 与 `astersql-errors` 提供取消识别、哨兵错误、注解、Trace、Cause 和多错误 Join；`status_code` 会检查顶层、multierr 子项和 Cause 链。
- `fail` 只服务 `GetAllTiKVStoresWithRetry` 的三条注入路径；`serde`/`serde_json` 只解析 TiKV `/config` 的局部字段。
- `Mgr::GetConfigBytesFromTiKV`、`ProcessTiKVConfigs` 和 `IsLogBackupEnabled` 调用成员 `GetConfigFromTiKV`；成员方法再调用同文件自由函数。自由函数调用 `GetAllTiKVStoresWithRetry` 和 `HandleTiKVAddress`。
- `br/pkg/backup/client.rs` 直接调用同名连接接口 `GetAllTiKVStoresWithRetry`，而仓库中 restore/snap/log-client 等子系统也有自己的适配或桩；扩展时必须依据实际 import 路径辨别 canonical 类型，不能只按符号名替换。
- RustCodeGraph 能定位 `conn.rs::NewMgr` 等精确节点，却没有产出其 callers/callees 边；本说明对上游接线的判断因此以 `lib.rs`、源码 import/引用和独立测试为限，不声称索引未证明的生产调用链。

## 错误处理与边界

PD/store 枚举错误通过 `Trace` 返回；`ErrorOnTiFlash` 以 `ErrPDInvalidResponse` 为因并补充 store id/address。激进重试最多执行 32 轮，初始等待 1 ms、指数增长且封顶 50 ms；上下文取消或错误链被识别为 context canceled 时停止，显式 gRPC Canceled/Unknown 可重试，其余错误停止，最终用 `Join` 聚合已见错误。

`StatusUrl::parse` 只验证存在 `scheme://` 且 host 非空，不是通用 URL 解析器；路径、查询串、用户信息等输入没有完整规范化。`HandleTiKVAddress` 拒绝空 status address，支持方括号 IPv6；当 status 与 node hostname 不同且 status 缺端口时，当前仍会生成带空端口的 `host:`，调用者不应把它当作全面的 URL 校验器。

`GetConfigFromTiKV` 只重试 `HttpClient::Get` 错误，不检查 HTTP 状态码；状态检查由 `GetConfigBytesFromTiKV` 显式执行，而 `ProcessTiKVConfigs` 和 `IsLogBackupEnabled` 直接解析 body。单节点用尽 8 次网络重试会中止整个遍历。无可访问 TiKV 时，`IsLogBackupEnabled` 的初值保持 true，这是全称判断的空集合语义，Rust/Go 测试都覆盖了仅有 TiFlash 的情况。

配置 JSON 缺字段时 serde 默认产生 0/false；格式错误、未知尺寸后缀或非法数字会返回错误。`ram_in_bytes` 使用 `f64` 后转换为 `u64`，可接受小数和负数字面量；负数/极大值转换的 Rust 饱和行为不等价于严格输入拒绝，因此若这里接收不可信配置，应增加与 Go `go-units` 一致的边界测试后再收紧。

`NewMgr` 的 Rust 枚举参数排除了 Go `switch default` 的未知整数分支；与此同时，构造中途失败后没有在本文件内显式关闭已经创建的 controller/storage。真实资源实现是否通过自身析构释放未由此文件证明，应视为接入实现的生命周期责任。

## 并发与资源生命周期

所有可跨线程注入的边界 trait 都要求 `Send + Sync`，共享句柄统一放入 `Arc`。取消通过原子布尔值传播；进程级 TiKV 标记也是原子变量。配置聚合函数接收 `&mut KVConfig` 和 `FnMut`，在一次调用内串行遍历 store，不并发修改配置。

`with_aggressive_retry` 与每节点 HTTP 重试都使用 `thread::sleep`，会阻塞当前 OS 线程，不是 async backoff。节点也逐个处理，因此总耗时可按 store 数量线性累积；扩展并发抓取前需保留确定性的错误选择、取消响应和“保守值”聚合规则。

`Mgr::Close` 先锁住 `closed`，按 StoreManager →（仅 `ownsStorage` 时）Domain → DDL owner manager → TiKV shutting-down → Storage → PD controller 的顺序执行，最后才写入 `closed = true`。`parity_test.rs::go_rust_public_contract_matches` 断言生命周期事件严格为 `domain, owner, shutting-down, storage`，并确认 StoreManager、PD 都关闭。各 trait 的 `Close` 不返回错误，所以当前 API 无法报告清理失败；锁在外部关闭钩子运行期间一直持有，钩子不得回调同一 `Mgr::Close`，否则可能死锁。

## 与 Go 版本的对应关系

Rust 文件直接对照 [`conn.go`](./conn.go)，核心对应为：Go `Mgr` ↔ Rust `Mgr`，Go `VersionCheckerType` ↔ Rust 同名枚举，Go `GetAllTiKVStoresWithRetry`/`checkStoresAlive`/`NewMgr` ↔ Rust 同名流程，Go `ProcessTiKVConfigs`/`IsLogBackupEnabled`/`GetConfigFromTiKV`/`handleTiKVAddress` ↔ Rust 配置链。

主要语义保持包括：三类版本检查选择、TiFlash 策略、`tikv://...?disableGC=true` 打开路径、Domain 后置 DDL 版本检查、keyspace GC 选择、TLS 决定 HTTP scheme、配置读取失败时保留默认值、所有 TiKV 的 log-backup 开关取 AND、以及 Domain 必须早于 storage 关闭。

已验证的差异与移植边界如下：

- Go 直接持有 `pdutil.PdController`、`kv.Storage`、`tikv.Storage`、真实 gRPC client 和 `utils.StoreManager`；Rust 用 trait 与轻量 `BackupClient`/`LogBackupClient` 句柄注入，且没有 Go 的 `GetStore`、`GetLockResolver` 实体能力。
- Go 利用 `util.GetConfigFromTiKVStores` 和 `kvconfig` 解析器；Rust 在本文件内实现地址、HTTP 重试和 JSON/字节单位解析，后续 Go 行为变化不会自动同步。
- Go `NewMgr` 接受 context、TLS config、keepalive 并创建 tracing span；Rust 构造签名改为 `tls_present` 加 `NewMgrDeps`，没有 tracing 和 keepalive 参数。
- Go 会设置全局 store type；Rust 使用 `STORE_IS_TIKV`，且置位时机早于实际类型验证。
- 源码与 Go 当前常量均声明 `DefaultImportNumGoroutines = 36`，但 [`parity_test.rs`](./parity_test.rs) 的 `go_rust_public_contract_matches` 断言 128。由于本任务不运行 Cargo、不修改测试，此处记录为现有测试漂移，不能把 128 写成生产事实。
- `CheckStoresAlive` 在两端当前都只统计/记录存活数量而不要求非零；Rust 没有日志边界，因此计数结果直接丢弃。

## 扩展指南

- 增加真实连接能力时，优先扩展 `StoreManagerHandle` 或增加独立适配实现，不要在 `Mgr` 内复制连接池；同步更新 `BackupClient`/`LogBackupClient` 契约及 [`conn_test.rs`](./conn_test.rs) 的委派、取消测试。真实外部依赖仍须遵守仓库关于独立上游移植和带 tag Git 依赖的规则。
- 改 `NewMgr` 时保持版本检查、Domain 创建、GC keyspace 和关闭顺序；新增依赖应进入 `NewMgrDeps` 以维持可测试性，并在 [`parity_test.rs`](./parity_test.rs) 添加成功、阶段性失败和清理断言。特别要评估 controller/storage 在构造失败时的释放以及 `STORE_IS_TIKV` 置位时机。
- 改 store 过滤或重试时同步维护 `GetAllTiKVStores`、`status_code`、`with_aggressive_retry` 和三个 failpoint；测试应继续独立放在 [`conn_test.rs`](./conn_test.rs)，覆盖 TiFlash 标签顺序、Tombstone、取消错误链和重试上限。
- 改配置聚合时分别确认 `Modified` 优先级、跨节点最小/最大规则、空 store 语义和部分成功后失败的状态。不要把 `ProcessTiKVConfigs` 的容错吞错复制到 `IsLogBackupEnabled`，后者的契约是返回错误。
- 改地址解析时至少覆盖 IPv4、方括号 IPv6、通配 status host、缺失 status address、缺失端口和非法 scheme；若换用通用 URL crate，需要验证字符串输出仍与 Go 测试一致。
- 性能优化若把串行 HTTP 改成并行，必须为并发上限、取消、回调线程安全、确定性错误和聚合顺序增加独立测试；当前 `FnMut` 与 `&mut KVConfig` 明确假设串行执行。
- 修复现有常量测试漂移时，应先以 Go `conn.go` 和用户可见配置默认值为依据统一 `DefaultImportNumGoroutines`，而不是为通过单个 parity 断言修改生产值。

## 验证依据

- RustCodeGraph：运行 `status` 确认索引可用；`files --filter br/pkg/conn` 确认目标文件被索引；用 `node --file br/pkg/conn/conn.rs --offset 1 --limit 500` 和 `--offset 501 --limit 700` 读取完整 1135 行及 58 个引用文件摘要；`query NewMgr/GetConfigFromTiKV/ProcessTiKVConfigs --kind function --json` 精确定位 Rust/Go 同名节点；对 Rust 节点执行 `callers`/`callees` 得到空数组，作为索引边界而非“无人调用”的证明。
- 生产源码与装配：完整核对 [`conn.rs`](./conn.rs)、[`lib.rs`](./lib.rs) 和 [`Cargo.toml`](./Cargo.toml)；通过源码搜索核对本文件主要符号及 BR 树中的直接引用。
- Go 对照：完整核对 [`conn.go`](./conn.go) 的 `Mgr`、`NewMgr`、重试、关闭、配置读取与地址委派；核对 [`conn_test.go`](./conn_test.go) 的 Go 行为用例。
- Rust 独立测试：核对 [`conn_test.rs`](./conn_test.rs) 的重试取消/Unknown、三种 store 行为、取消客户端、PD 时间戳、StoreManager 委派、HTTP 200、配置聚合、log-backup AND 和 IPv4/IPv6 地址用例；核对 [`parity_test.rs`](./parity_test.rs) 的 `NewMgr`、非 TiKV 拒绝、关闭顺序及常量断言；核对 [`main_test.rs`](./main_test.rs) 的公开导出可达性。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务指定的 11 章节结构检查，并人工复核：所有限制均以当前源码或测试为依据，没有把精简 trait 当成真实网络实现，也没有把 RustCodeGraph 的空调用边扩张成生产接线结论。
