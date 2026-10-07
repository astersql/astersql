# `pkg/domain/serverinfo/status_endpoint_claim.rs`

## 文件定位

本文件属于 `astersql-domain-serverinfo` crate；`pkg/domain/serverinfo/lib.rs` 将模块设为私有后通过 `pub use status_endpoint_claim::*` 重新导出其公开符号。它位于 server-info 注册流程与 etcd 原子操作之间：`pkg/domain/serverinfo/syncer.rs` 在构造 `Syncer` 时生成声明键，在建立 server-info lease 后尝试占有该键，并在注销或注册失败时用相同的服务器 ID 与 lease 条件清理。文件自身不创建 lease，也不管理后台循环。

该声明是“尽力而为”的冲突探测：相同的 advertised status endpoint 只能有一个活跃声明，但冲突或检查失败只形成诊断结果和警告，不阻止各实例继续写入各自的 server-info。直接依据是模块注释、`StatusEndpointClaim::acquire` 以及 `Syncer::NewSessionAndStoreServerInfo` 对返回值不作失败传播。

## 核心职责

- `build_status_endpoint_claim` 把 `ServerInfo.StaticInfo` 中的 advertised IP 与 status port 规范化成 `host:port`，再生成 `/tidb/server/status_addr/<RawURLBase64(endpoint)>` 键。
- `StatusEndpointClaim::acquire` 协调一次“原子创建—同 ID 重挂 lease—竞态后重试创建”的状态机，避免覆盖其他实例的声明，也避免依据过期观察重挂 lease。
- `StatusEndpointClaimResult` 将结果归类为跳过、获得、冲突或检查失败，并为冲突/失败生成结构化告警字段。
- `StatusEndpointClaim::from_key`、`cleanup_fields` 为 `Syncer` 的清理路径恢复 endpoint 和生成一致的日志上下文。

本文件只编排 `EtcdClient::TryCreateClaim` 与 `EtcdClient::ReattachClaim`；真实 etcd 事务在 `pkg/domain/serverinfo/real_etcd.rs`，内存实现及 trait 默认错误路径在 `pkg/domain/serverinfo/syncer.rs`。

## 主要符号

- `EndpointClaimState::{Skipped, Acquired, Conflict, CheckFailed}`：一次尝试的四种终态。空键为 `Skipped`；新建或同 ID 成功换绑 lease 为 `Acquired`；已有不同 ID 为 `Conflict`；客户端错误或无法收敛的 revision 竞态为 `CheckFailed`。
- `ObservedStatusEndpointClaim { id, lease, mod_revision }`：原子创建失败时读取到的现有声明快照。`mod_revision` 是后续重挂接的 CAS 条件，`lease` 用于冲突诊断。
- `StatusEndpointClaimResult`：携带状态、endpoint、claim key、本地 ID、已有 ID/lease 和可选 `SyncError`。私有 `apply_create` 统一解释 `TryCreateClaim` 的结果。
- `StatusEndpointClaimResult::{diagnostic, report_result, warning}`：仅为 `Conflict` 和 `CheckFailed` 产生诊断；`report_result` 写 warning 日志，`warning` 生成供测试断言的单行文本。
- `build_status_endpoint_claim(info, enabled)`：公开的纯计算入口。禁用、`StaticInfo.IsAssumed()`、空白 IP 或规范化后空 host 均返回两个空字符串。
- `StatusEndpointClaim<'a> { client, endpoint, key, local_id }`：借用 `dyn EtcdClient` 的单次声明操作对象，不拥有客户端或 lease。
- `StatusEndpointClaim::{from_key, cleanup_fields, acquire, try_acquire_and_report}`：分别负责从键恢复对象、清理日志、执行状态机，以及在父 context 未取消时回调报告。

## 执行流程

1. `syncer.rs::newSyncer` 结合全局 `status.report_status`、`SyncerOption::WithoutStatusEndpointClaim` 和 `StaticInfo.IsAssumed()` 计算开关，调用 `build_status_endpoint_claim`，只在非空时保存 `statusEndpointClaimKey`。
2. 构键时先 trim IP。合法 IP 使用标准字符串形式；带非空 zone 的 IPv6 先规范化地址再保留 `%zone`；其他主机名转小写并去掉一个尾随点。包含冒号的 host 加方括号。`StatusPort == 0` 时使用 `astersql_config::DEF_STATUS_PORT`。最终 endpoint 使用 URL-safe、无 padding 的 Base64 作为键段。
3. `Syncer::NewSessionAndStoreServerInfo` 获得 server-info lease 后调用 `tryClaimStatusEndpoint`；后者以保存的 key 构造 `StatusEndpointClaim`，把当前 session lease 传给 `try_acquire_and_report`。
4. `acquire` 先创建默认 `Skipped` 结果；空 key 立即返回。非空 key 从父 `Context` 派生 `KeyOpDefaultTimeout` 超时 context。
5. 第一次 `TryCreateClaim` 原子地“键不存在则写入本地 ID 并绑定 lease，否则读取现有声明”。错误变为 `CheckFailed`，新建成功变为 `Acquired`，不同 ID 变为 `Conflict`；只有相同 ID 的观察结果继续执行。
6. 对相同 ID 调用 `ReattachClaim`，要求值和 `mod_revision` 都仍与观察一致，才用新 lease 重写。错误为 `CheckFailed`，CAS 成功为 `Acquired`。
7. CAS 失败表示观察后键发生变化。代码再次 `TryCreateClaim`：键消失则可重新获得；不同 ID 则报告冲突；客户端错误则检查失败。如果仍观察到相同 ID，说明竞态未能安全收敛，返回带固定错误文本的 `CheckFailed`，而不是覆盖新值。
8. `try_acquire_and_report` 在 `acquire` 后检查父 context；父 context 已取消则返回 `None` 且不执行报告回调，否则报告并返回 `Some(result)`。`Syncer` 的回调调用 `report_result`，但不把冲突或检查失败升级为 server-info 注册错误。

## 数据与状态

声明键的值是 `ServerInfo.StaticInfo.ID`，lease 是当前 server-info session lease，因此正常 lease 撤销会一起删除声明。endpoint 不直接放在路径中，而以 Raw URL-safe Base64 编码，避免 `/` 等字符改变 etcd 路径层级；测试同时验证生成键固定为四个 `/`。

`StatusEndpointClaimResult` 是一次尝试的值对象。`existing_id` 与 `existing_lease` 只在已观察到现有声明时填充；`error` 只在 `CheckFailed` 路径设置。`ObservedStatusEndpointClaim.mod_revision` 不展示给用户，而是防止 ABA 式的过期观察覆盖后续更新。

`StatusEndpointClaim` 仅持有不可变字符串和客户端借用；可并发创建多个对象。共享状态和原子性由 `EtcdClient` 实现负责。`Syncer` 保存的是可选 claim key，而不是长寿命的 `StatusEndpointClaim`。

## 依赖与调用关系

上游主链为 `syncer.rs::newSyncer -> build_status_endpoint_claim`，以及 `Syncer::NewSessionAndStoreServerInfo -> Syncer::tryClaimStatusEndpoint -> StatusEndpointClaim::try_acquire_and_report -> acquire`。`Syncer::cleanupFailedRegistration` 与 `Syncer::RemoveServerInfo` 也调用 `from_key`/`cleanup_fields`，但实际删除通过 `EtcdClient::CompareAndDelete` 完成。

下游依赖包括：crate 内的 `Context`、`EtcdClient`、`KeyOpDefaultTimeout`、`ServerInfo`、`SyncError`；`astersql-config` 的默认 status port；`base64` 的 URL-safe 无 padding 编解码；`astersql-util-logutil` 的后台 logger、级别与字段。`pkg/domain/serverinfo/Cargo.toml` 明确声明这些 crate，并把 Go 对照包记为 `pkg/domain/serverinfo`。

代码图还记录目标文件被 `pkg/domain/canonical_domain.rs`、`pkg/domain/serverinfo/syncer.rs`、`pkg/domain/serverinfo/syncer_test.rs`、`pkg/executor/internal/mpp/local_mpp_coordinator.rs` 和 `pkg/session/tidb_test.rs` 使用；其中直接行为接线和细粒度测试集中在 `syncer.rs`/`syncer_test.rs`，`pkg/session/tidb_test.rs` 直接调用构键函数准备相关键。

## 错误处理与边界

禁用、assumed server、空 host 与空 key 是正常跳过，不是错误。`diagnostic` 对 `Skipped` 和 `Acquired` 返回 `None`，因此不会制造无意义告警。冲突告警包含本地/现有 server ID、现有 lease（16 位十六进制）、claim key、endpoint、可选 keyspace 和排查建议；检查失败告警包含 `SyncError` 与 etcd 连通性建议。

所有键操作共享一个从调用 context 派生的 `KeyOpDefaultTimeout`，避免声明探测无限阻塞。父 context 在操作完成前后被取消时，底层可能产生 `CheckFailed`，但 `try_acquire_and_report` 明确抑制取消后的报告。该接口仍返回 `None`，调用方不能把它解释为成功获得声明。

相同 ID 不等于可直接覆盖：必须同时匹配观察到的 `mod_revision`。重挂接失败后的二次创建只在键已消失时获得所有权；若仍是相同 ID，则选择 `CheckFailed`。这种保守行为是避免覆盖并发更新的关键不变量。

IPv6 zone 仅在 `%` 左侧可解析为 IPv6 且 zone 非空时特殊处理；其他无法解析为 IP 的输入按主机名规则小写并去尾点。该函数不做 DNS、端口范围或一般主机名合法性校验，保持与配置层职责分离。

## 并发与资源生命周期

文件不创建线程、异步任务、锁或通道。并发安全依赖 etcd 事务语义：`TryCreateClaim` 使用 create-revision 条件创建，`ReattachClaim` 使用值加 mod-revision 的双重比较。`syncer_test.rs::status_endpoint_atomic_competition_restart_and_deletion` 验证两个线程竞争时只有一个 `Acquired`，失败方为 `Conflict`；旧 lease 不能删除已重挂到新 lease 的声明。

lease 生命周期由 `Syncer` 管理。启动时声明绑定当前 server-info lease；重启时同 ID 可安全换绑新 lease；失败注册及显式移除使用 ID+lease 比较删除；`RevokeSession` 撤销 lease 后声明随之消失。声明冲突不会删除赢家，也不会阻止输家保留独立 server-info，这由并发注册测试覆盖。

`StatusEndpointClaim<'a>` 的生命周期参数保证客户端借用不越界；结果和观察值均拥有自己的字符串。日志字段通过 clone 构造，不持有 `Syncer` 锁或借用。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/domain/serverinfo/status_endpoint_claim.go`。Rust 的四态枚举、结果/观察结构、构键、首次创建、同 ID 重挂接、竞态后二次创建、冲突/失败告警以及 cleanup 字段，与 Go 的 `endpointClaimState`、`statusEndpointClaimResult`、`observedStatusEndpointClaim`、`buildStatusEndpointClaim`、`acquire`、`applyCreateResult`、`reportResult` 和 `cleanupFields` 一一对应。

实现分层有所不同：Go 文件直接持有 `*clientv3.Client` 并在本文件实现 `tryCreate`、`reattach`、`remove`；Rust 把这些原子操作抽象到 `EtcdClient`，真实 etcd 实现在 `real_etcd.rs`，删除由 `Syncer` 调用 `CompareAndDelete`。Go claim 对象保存 `keyspace` 和 `report` 回调；Rust 将 keyspace 作为 `diagnostic/report_result/warning` 参数，并把报告闭包传入 `try_acquire_and_report`。这些是接线差异，不改变状态机意图。

Rust 额外显式处理带 zone 的 IPv6 文本，并通过 `from_key` 解码 endpoint 供清理日志使用。Go 使用 `net.JoinHostPort`，Rust 以“host 含冒号则加方括号”实现同类 endpoint 格式。对应 Go 测试集中在 `pkg/domain/serverinfo/syncer_test.go::TestBuildStatusEndpointClaim` 与 `TestStatusEndpointClaim`；Rust 没有把测试内嵌在本文件，而是在 `pkg/domain/serverinfo/syncer_test.rs` 中保持独立。

## 扩展指南

新增构键规范时修改 `build_status_endpoint_claim`，并同步扩展 `syncer_test.rs::status_endpoint_normalization_matches_go_cases` 与 Go 的 `TestBuildStatusEndpointClaim`；必须确认历史 endpoint 规范化是否影响滚动升级期间的键兼容性。新增状态或诊断字段时同时修改 `EndpointClaimState`、`apply_create`、`diagnostic`/`warning`，并更新竞争和失败测试，避免日志与返回状态分叉。

调整声明原子语义时，接入点不是只改本文件：还要同步 `EtcdClient::{TryCreateClaim, ReattachClaim}`、`real_etcd.rs` 的真实事务、`syncer.rs` 的内存实现与 `ClaimFaultClient`。必须保留“不同 ID 不覆盖”“相同 ID 需匹配 revision”“冲突不阻断 server-info 注册”三个行为，并覆盖真实 etcd namespace/lease 清理测试。

清理逻辑位于 `Syncer::cleanupFailedRegistration`、`RemoveServerInfo` 和 `RevokeSession`。若改变 claim 所属 lease 或值格式，必须一起审查这些路径及 `CompareAndDelete` 的比较条件。Rust 测试应继续放在独立的 `syncer_test.rs`，不要写回生产源文件。

性能上每次首次声明通常是一笔事务；同 ID 重启可能是创建观察加重挂接两笔事务，竞态时最多再进行一次创建事务。扩展时不应在同步注册主链引入无界重试或无超时网络调用。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标 `status_endpoint_claim.rs` 被解析为 22 个符号，文件查询列出 5 个使用文件。精确 callers/callees 查询在 30 秒时限内未返回，调用边随后用源码引用搜索核验。
- 生产源码：`pkg/domain/serverinfo/status_endpoint_claim.rs`（全部 260 行）、`pkg/domain/serverinfo/syncer.rs`（trait 原子接口、构造、声明、清理和 session 生命周期）、`pkg/domain/serverinfo/real_etcd.rs`（真实事务实现）、`pkg/domain/serverinfo/lib.rs`（模块导出）。目标包没有 `doc.go`。
- crate 边界：`pkg/domain/serverinfo/Cargo.toml`，包名为 `astersql-domain-serverinfo`，依赖 `astersql-util-logutil`、`base64`、`astersql-config`、`etcd-client` 和 `tokio`，porting 元数据指向 Go 包 `pkg/domain/serverinfo`。
- Go 对照：`pkg/domain/serverinfo/status_endpoint_claim.go`；Go 测试引用位于 `pkg/domain/serverinfo/syncer_test.go`。
- Rust 独立测试：`pkg/domain/serverinfo/syncer_test.rs`，重点包括 `status_endpoint_normalization_matches_go_cases`、`status_endpoint_atomic_competition_restart_and_deletion`、`status_endpoint_revision_races_retry_without_overwrite`、`status_endpoint_failures_and_parent_cancellation_report_correctly`、`status_endpoint_registration_disabled_assumed_default_and_shutdown`、忽略的真实 etcd 测试及并发注册测试。
- 本任务为纯文档分析，按计划不运行 Cargo；结构校验要求文档存在且恰有 11 个固定二级标题。
