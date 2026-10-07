# `br/pkg/pdutil/pd.rs`

## 文件定位

`br/pkg/pdutil/pd.rs` 属于 Cargo crate `astersql-br-pkg-pdutil`。crate 入口 `br/pkg/pdutil/lib.rs` 以 `pub mod pd` 挂载本文件，并通过 `pub use pd::*` 将其公开符号平铺到 crate 根；同一 crate 的 `utils.rs` 提供键编码和 `UndoFunc`。`br/pkg/pdutil/Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/pdutil` 的 library 移植，直接依赖 BR 错误类型、通用错误封装、`semver`、Serde/JSON、hex 和 UUID。

本文件位于 BR 与 PD 控制面的边界：它不实现 PD 服务，而是通过 `PdHttpClient`、`PdClient` 两个 trait 抽象 PD HTTP/客户端操作，围绕备份、恢复期间的调度器暂停、临时调度配置、按 key range 禁止调度、版本能力判断和恢复清理提供 `PdController`。RustCodeGraph 显示 Rust 侧直接使用者包括 `br/pkg/task/backup_raw.rs`、`br/pkg/task/backup_txn.rs`、`br/pkg/restore/import_mode_switcher.rs`、`br/pkg/task/operator/prepare_snap.rs`、`br/pkg/restore/snap_client/client.rs`，而 `br/pkg/conn/conn.rs` 通过控制器接口暴露 PD client 生命周期。

## 核心职责

- 以 `PdController` 统一持有可选 `PdClient`、必需 `PdHttpClient`、已解析 PD 版本、暂停刷新通道和关闭状态。
- 用 `Schedulers()` 白名单筛选会影响 BR 性能的 PD 调度器；用 `DefaultExpectPDCfgGenerators()` 计算暂停期临时配置，并保存可恢复的原值。
- 执行“首次同步暂停成功后才启动后台续租”的生命周期：调度器 delay 和临时配置均按 TTL 设置，后台每 `TTL / 3` 刷新，`ResumeSchedulers`、上下文取消或 `Close` 终止刷新。
- 生成 `UndoFunc`，恢复调度器、region label rule 和原调度配置；恢复配置失败保留 `ErrPDUpdateFailed` 分类。
- 在 PD 版本达到 6.1.0 时支持用 `schedule=deny` 的 key-range region label rule 暂停指定范围，并周期刷新 TTL、取消后删除规则。
- 提供 PD 辅助透传：集群版本、region 数、store 信息、最小 resolved TS、base alloc ID、ResetTS、recovering mark 和 follower handle。

## 主要符号

- `Context`：由 `Arc<AtomicBool>` 实现的轻量协作式取消对象。`new`/`Background` 创建未取消上下文，`cancel` 写入标志，后台循环用 `is_cancelled` 轮询。它是 Go `context.Context` 的局部替身，不携带 deadline、value 或超时错误。
- 常量 `maxMsgSize`、`pauseTimeout`、`PDRequestRetryTime`、`maxPendingPeerUnlimited`：分别保留 Go 的消息大小、默认 5 分钟暂停 TTL、请求重试时间和暂停期 pending peer 上限语义；本文件当前只直接使用后两类调度相关值，连接构造参数仍由外部接线负责。
- `PauseConfigGenerator` 及 `zeroPauseConfig`、`pauseConfigMulStores`、`pauseConfigFalse`、`constConfigGeneratorBuilder`：把 store 数和当前 PD 配置映射成暂停值。`pauseConfigMulStores` 将数值乘以 store 数并封顶 40；非数值 JSON 通过 `as_f64().unwrap_or(0.0)` 回落为 0。
- `ClusterConfig`：暂停/恢复快照，保存 `Schedulers`、`ScheduleCfg` 和可选 `RuleID`。`RegionLabel`、`LabelRule`、`KeyRangeRule`、`LabelRulePatch` 则对应 PD label API 的序列化结构。
- `PdHttpClient`：版本、scheduler delay、schedule config、region label、resolved TS、ResetTS、recovering mark 等 HTTP 能力的最小 trait；`PdClient`：store 枚举、follower handle 和关闭的最小 trait。二者使生产逻辑能用 mock 做契约测试。
- `PdController::NewPdControllerWithPDClient`：当前 Rust 文件的主要构造入口，注入 client、HTTP client 和版本；`NewPdControllerWithClients` 是兼容别名。Rust 文件没有 Go `NewPdController` 中创建真实 PD gRPC/HTTP client、TLS、backoff 和版本探测的实现。
- `RemoveSchedulersWithConfigGenerator`：暂停主流程的核心编排函数；`RemoveSchedulers`、`RemoveSchedulersWithConfig`、`RemoveAllPDSchedulers` 和 `RemoveSchedulersWithCfg` 是不同策略的公开包装。
- `GenRestoreSchedulerFunc`、`MakeUndoFunctionByConfig`、`MakeFineGrainedUndoFunction`：捕获配置和 client，生成延迟执行的恢复闭包；实际恢复由私有 `restore_schedulers` 完成。
- `pause_scheduler_by_key_range_with_ttl`、`PauseSchedulersByKeyRange`、`RemoveSchedulersOnRegion`：创建、续租、取消和清理 key-range label rule 的入口。
- `parseVersion`、`FetchPDVersion`、`isPauseConfigEnabled`、`CanPauseSchedulerByKeyRange`：解析版本并实施 4.0.8/6.1.0 两级能力门控。

## 执行流程

常规全局暂停从 `RemoveSchedulers` 或 `RemoveSchedulersWithConfig` 开始：

1. `RemoveSchedulersWithConfigGenerator` 先通过 `PdClient::GetAllStores` 得到 store 数；未注入 client 时 Rust 当前实现使用空列表。随后读取 `GetPDScheduleConfig`。
2. 对 generator 关注且实际存在的配置项，分别记录原值并计算暂停值；不存在的配置项被跳过。
3. `ListSchedulers` 返回现有调度器，流程只保留 `Schedulers()` 白名单中的名称，自定义调度器不会被暂停。
4. `doRemoveSchedulersWith` 要求 PD 版本至少为 4.0.8，然后调用 `pauseSchedulersAndConfigWith`。后者先逐个设置 scheduler delay，再以 `schedule.` 前缀和 TTL 写配置；任一首次调用失败立即返回，不启动续租线程。
5. 首次暂停成功后创建本轮 channel 并启动线程。线程以 `TTL / 3` 为周期重新设置 delay 和配置；刷新错误被忽略，等待下一周期。上下文取消、`ResumeSchedulers` 发信号或 sender 被丢弃时线程退出。
6. 调用方得到的 `UndoFunc` 最终进入 `restore_schedulers`：调度器 delay 置 0，若有 `RuleID` 则请求删除 label rule，从快照筛出需恢复配置，并在 PD 4.0.8 及以上以 TTL=0 写回；配置恢复错误被标注为 `ErrPDUpdateFailed` 和 `fail to update PD merge config`。

按范围暂停从 `pause_scheduler_by_key_range_with_ttl` 开始：空范围或首个 start key 为空时直接返回空 rule；否则每个边界转为 hex，创建 UUID rule ID 和 `schedule=deny`、`rule_type=key-range` 的 label rule。首次 `SetRegionLabelRule` 成功后，后台线程每 `TTL / 3` 刷新。取消后线程使用新的 `Context::Background()` PATCH 删除 rule，再通过 done channel 通知清理结束。`RemoveSchedulersOnRegion` 会额外等待 20ms 让 PD 异步规则生效，并返回一个执行“cancel + 等待 done”的闭包。

## 数据与状态

`PdController` 的共享状态由 `Arc`、`Mutex` 和原子量组合：`pd_client` 支持运行时替换，`pd_http` 以 `Arc<dyn PdHttpClient>` 被控制器、恢复闭包和后台线程共享；`scheduler_pause_ch` 保存当前暂停轮次的 sender；`closed` 保证 `Close` 幂等；`SchedulerPauseTTL` 为零时由 `ttlOfPausing` 回落到 `pauseTimeout`。`pause_loop_alive` 会在启动、恢复和关闭时更新，但后台线程实际捕获的是另一个局部 `Arc<AtomicBool>`，因此它目前不是可靠的 join/存活观测接口，也没有公开读取者。

`ClusterConfig` 是恢复所需的最小快照：只保存 generator 关注且 PD 当前存在的配置，不是完整 PD schedule config；`removed_cfg` 保存对应的暂停值。两个快照在成功路径共享同一组已暂停调度器。配置 JSON 使用 `serde_json::Value`，数值、布尔值和字符串按各 PD 配置项的约定保留。

`Context` 的取消状态可被克隆后的后台线程观察；`RemoveSchedulersOnRegion` 返回的 wait 闭包用 `Mutex<Option<Receiver>>` 保证只取走一次 receiver。普通 scheduler 暂停线程没有 join handle，恢复依赖 channel 信号和 TTL 的最终兜底。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 的文件/调用查询确认：

- `br/pkg/task/backup_raw.rs::RunBackupRaw` 和 `br/pkg/task/backup_txn.rs::RunBackupTxn` 调用调度器移除能力，为备份阶段降低 PD 调度干扰。
- `br/pkg/restore/import_mode_switcher.rs::RestorePreWork` 使用 `RemoveSchedulersWithConfig`，`FineGrainedRestorePreWork` 使用 `RemoveSchedulersOnRegion`，分别覆盖全局和细粒度恢复准备。
- `br/pkg/task/operator/prepare_snap.rs::pauseSchedulerKeeper` 使用 `RemoveAllPDSchedulers`，`pauseGCKeeper` 使用 `GetMinResolvedTS`；这表明本文件也是 snapshot 恢复环境适配的一部分。
- `br/pkg/restore/snap_client/client.rs::ResetTS` 向本控制器的时间戳重置语义靠拢；`br/pkg/conn/conn.rs` 暴露 `GetPDClient`、`SetPDClient` 和 `Close` 等连接管理边界。
- Go 主链另有 `br/pkg/task/backup.go::RunBackup`、`br/pkg/task/restore.go::runSnapshotRestore` 等调用；它们用于核对原始设计，但不能据此断言同名 Rust 路径均已完成接线。

下游依赖集中在 trait：scheduler/config/label/恢复标记请求进入 `PdHttpClient`，store 枚举和 follower 选项进入 `PdClient`；`crate::utils::encode_bytes` 将 region 查询键转换为 memcomparable 格式，`UndoFunc` 定义恢复闭包签名；`astersql_br_pkg_errors::ErrPDUpdateFailed` 和 `astersql_errors::Annotate` 保持错误分类；`semver` 执行版本比较；Serde、hex、UUID 构造 PD label 请求。

## 错误处理与边界

- 首次暂停 scheduler 失败会原样上抛；已经成功设置 delay 的前序 scheduler 不会立即回滚，这与 Go 顺序循环一致。首次配置写入失败会包装为 `ErrPDUpdateFailed`，且不会启动后台刷新。
- 后台刷新 scheduler/config 的错误均被忽略，让下一次 tick 重试；恢复 scheduler delay、删除 label rule 的错误也被忽略，因为 TTL 是最终恢复兜底。相反，恢复原配置失败会返回带分类的错误。
- `RemoveSchedulersWithConfigGenerator` 在读取 store、配置或 scheduler 列表失败时返回当时已构造的快照和错误；调用方必须检查第三个返回值，不能仅使用快照。
- PD 低于 4.0.8 时 `doRemoveSchedulersWith` 明确拒绝 pause config；低于 6.1.0 时 `CanPauseSchedulerByKeyRange` 返回 false，但底层 helper 本身不重复做版本校验，调用方须先门控。
- `parseVersion` 会去空白、外层引号和 `v` 前缀；非法版本回落到 0.0.0，从而走保守能力路径。`ResetTS` 将错误文本包含 `Forbidden` 视为旧 PD 不支持该 API 并返回成功，其他错误继续上抛。
- `GetRegionCount` 对起止键做 memcomparable 编码，空 end key 保持空值表示开放右边界。key-range label helper 只用“范围为空或首个 start 为空”判定无操作，其余范围不会单独拒绝空 start/end。
- `SetFollowerHandle` 在未注入 `PdClient` 时返回 `pd client not set`；`Close` 对缺失 client 安全且幂等。`pauseConfigMulStores` 对非数值 JSON 静默按 0 处理，这是 Rust 当前边界，与 Go 的类型断言 panic 不完全相同。

## 并发与资源生命周期

全局暂停线程在首次写 PD 成功后创建。正常恢复由 `ResumeSchedulers` 向当前 channel 发送信号并把 scheduler delay 置 0；`Close` 通过 `closed.swap` 防止重复释放，依次关闭可选 PD client、HTTP client并丢弃 sender，使等待中的线程收到 disconnect。上下文取消也会终止循环，但不会在该线程内主动恢复 delay/config；安全性依赖 PD TTL 过期或调用方执行 undo。

key-range 暂停线程持有 `Arc<PdHttpClient>` 和克隆的 `Context`。取消后先退出刷新循环，再用不带取消标记的新 context 删除 rule，最后发送 done；调用方必须执行 `RemoveSchedulersOnRegion` 返回的 wait 闭包，才能确保清理完成。当前恢复 context 没有 Go 版本的 5 秒 deadline，清理失败仍被忽略并依赖 label TTL 到期。

使用扩展入口时必须保证 TTL 大于零且足以产生合理的 `TTL / 3` 周期；本文件没有显式拒绝零 TTL。普通暂停线程没有保存 `JoinHandle`，因此 `Close` 和 `ResumeSchedulers` 只负责唤醒，不同步等待线程真正退出。不要把 `pause_loop_alive` 当作完成屏障。

## 与 Go 版本的对应关系

直接对照文件是 `br/pkg/pdutil/pd.go`，行为测试对照 `br/pkg/pdutil/pd_serial_test.go`。Rust 保留了 Go 的 scheduler 白名单、配置 generator、4.0.8/6.1.0 门控、首次同步暂停与 TTL/3 续租、`schedule.` 前缀、Undo 恢复顺序、ResetTS Forbidden 兼容以及 label rule 的 create/refresh/delete 契约。

当前可见差异必须在扩展时保留意识：

- Go `NewPdController` 负责按地址/TLS 创建真实 PD client、配置 gRPC 消息大小/超时、创建 HTTP client/backoff 并探测版本；Rust `pd.rs` 只有注入式构造器，trait 的生产实现和拨号接线不在本文件中。
- Go 使用完整 `context.Context`、ticker 和 buffered channel；Rust 使用原子取消标志、`std::thread` 和 mpsc channel，没有 deadline/value 传播。Go label 清理 context 有 5 秒超时，Rust 使用 `Context::Background()`。
- Go `RemoveSchedulersWithConfigGenerator` 要求非空 `pdClient`；Rust 未注入时以 0 个 store 继续计算。Go `pauseConfigMulStores` 对错误类型执行强制断言，Rust 回落为 0。
- Go 构造失败路径会关闭已经创建的 client；Rust 构造只接收现成对象。Rust `Close` 额外提供显式幂等保护并允许缺失 PD client。
- Go `parseVersion` 包含 failpoint 覆盖和告警日志；Rust 没有该 failpoint与日志。Rust 测试以 trait mock 替代 Go 的真实 pdhttp/httptest 边界。

因此，本文件是 Go 控制逻辑的可测试移植，不应被描述为已经独立实现了完整 PD 网络客户端。

## 扩展指南

- 新增会影响 BR 性能的 scheduler 时，更新 `Schedulers()`，并在独立测试 `br/pkg/pdutil/parity_test.rs` 中同时验证目标 scheduler 被暂停、自定义 scheduler 不受影响；还应核对 Go `Schedulers` map。
- 新增临时 PD 配置时，优先扩展 `expect_pd_cfg_generators` 或由调用方传入 `PauseConfigGenerator`，同时确认 PD 配置值的 JSON 类型、TTL 支持版本和恢复原值逻辑。若配置属于全暂停策略，也检查 `RemoveAllPDSchedulers`。
- 新增 PD HTTP/client 能力时，在相应 trait 增加最小方法，由 `PdController` 做策略编排，并同步更新 `parity_test.rs`、`pd_serial_test.rs` 中的 mock 实现；不要把测试内嵌进 `pd.rs`。
- 修改暂停生命周期时，必须同时覆盖首次失败、后台刷新、Resume、context cancel、Close 幂等和 TTL 兜底，特别注意不要让恢复信号发送阻塞，也不要在已取消 context 上做 label 清理。
- 修改 key-range rule 时，保持 `Data` 是 `KeyRangeRule` 列表、键使用 hex、label 固定为 `schedule=deny`，并同步核对 PD 6.1.0 TTL 能力门槛以及 `pd_serial_test.rs::test_pause_schedulers_by_key_range`。
- 若补齐真实 `NewPdController`，应在连接层实现并复用现有 trait，而不是把 TLS/gRPC 细节混入暂停算法；需要逐项对照 Go 的 API context、消息大小、60 秒自定义超时、HTTP backoff、版本探测和失败清理。
- 性能风险主要来自后台线程数量和过短 TTL；兼容风险主要来自 PD 配置键/值类型、版本门控和错误文本匹配；正确性风险主要来自部分暂停后失败以及恢复清理未被等待。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7032 个 Rust 文件；`files --filter br/pkg/pdutil` 确认 `pd.rs`、crate 入口、三个相关 Rust 测试和 Go 对照文件均已索引；`node --file br/pkg/pdutil/pd.rs` 阅读了 1-1139 行完整实现。
- RustCodeGraph：查询 `PdController`、`PdClient`、`NewPdController` 并执行包含 `RemoveSchedulers`、`RemoveSchedulersOnRegion`、`FetchPDVersion` 的 `explore`，核对内部调用链和 `backup_raw.rs`、`backup_txn.rs`、`import_mode_switcher.rs`、`prepare_snap.rs`、`snap_client/client.rs` 等上游。
- crate/入口：读取 `br/pkg/pdutil/Cargo.toml` 和 `br/pkg/pdutil/lib.rs`，确认 crate 名、依赖、模块挂载、平铺导出及测试文件均通过 `#[cfg(test)]` 独立接入。
- Go 对照：读取 `br/pkg/pdutil/pd.go` 全部公开/私有入口及 `br/pkg/pdutil/pd_serial_test.go` 的用例清单，核对调度器暂停、配置 TTL、Undo、版本解析、ResetTS 和 region label 生命周期。
- Rust 测试：读取 `br/pkg/pdutil/pd_test.rs`、`br/pkg/pdutil/pd_serial_test.rs`、`br/pkg/pdutil/parity_test.rs`。覆盖证据包括 duration 精度、首次 pause/config 失败、错误分类、非法版本回落、Forbidden 兼容、空配置仍发送、白名单筛选、store 数生成配置、label TTL 刷新/取消清理、follower handle 和 Close 幂等。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令确认本文恰有 11 个固定二级标题，并人工核对未把 Go 调用关系误写为 Rust 已接线事实。
