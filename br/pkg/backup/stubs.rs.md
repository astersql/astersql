# `br/pkg/backup/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-backup` crate 的本地依赖边界集合，而不是一个独立的备份算法模块。crate 入口 `br/pkg/backup/lib.rs` 通过 `#[path = "stubs.rs"] pub mod stubs` 注册它，并以 `pub use stubs::*` 平铺导出；`client.rs`、`store.rs`、`schema.rs` 都直接从 `crate::stubs` 获取协议结构、PD/KV/元数据 trait、内存实现和测试注入点。

`br/pkg/backup/Cargo.toml` 只依赖 `serde` 与 `serde_json`，并明确说明 ARM64 构建不引入 kv/domain/kvproto/grpcio/distsql 等重依赖。因此，本文件通过本地类型和 trait 模拟 Go BR 跨包边界，使备份客户端、store 收发与 schema 路径可编译、可测试。它参与正常编译，但大量实现仅适用于迁移期契约测试，不能被理解为真实 PD、TiKV、对象存储或 checkpoint 后端。

## 核心职责

该文件承担四类职责：

1. 镜像外部数据契约：`backuppb`、`errorpb`、`metapb`、`kvrpcpb`、`model` 保存备份请求/响应、文件、schema、store、DDL job、表元数据等字段子集。
2. 隔离外部服务：`PdClient`、`Storage`、`ExternalStorage`、`BackupClient`、`LockResolver`、`Glue`、`meta::Reader` 等 trait 把真实基础设施替换为可注入边界。
3. 提供确定性的内存实现和算法子集：`objstore::MemStorage`、`metautil::MemMetaWriter`、`meta::MemMeta`、`gc::MemGCManager`、`rtree::ProgressRangeTree`、`WorkerPool` 等支持独立 Rust 测试。
4. 提供测试控制面：`failpoint`、`infosync::set_label_rules`、`checksum::inject_checksum_response`、`set_skip_round_sleep`、`utils::set_skip_backoff_sleep` 让错误、拓扑变化、checksum 与等待行为可重复观测。

文件注释明确限定“符号名、关键字段与错误文案”是主要 Go 对齐目标；空操作、固定返回和精简字段是刻意边界，不代表完整行为等价。

## 主要符号

- 顶层基础设施：`Error`/`Result<T>` 提供字符串错误及 `Trace`、`Annotate`、`Annotatef`、`Cause` 兼容入口；`Context`、`CancelFunc`、`CancelCauseFunc` 用共享 `Mutex<Option<Error>>` 近似取消；`WorkerPool::ApplyOnErrorGroup` 与 `wait_jobs` 管理有界线程和首个错误；`now_ms` 与 `SKIP_ROUND_SLEEP` 提供时间和测试开关。
- 协议/模型模块：`backuppb::{BackupRequest, BackupResponse, File, Schema, BackupClient, BackupStream}`、`metapb::Store`、`model::{CIStr, TableInfo, DBInfo, Job, PolicyInfo}` 是 `client.rs`、`store.rs`、`schema.rs` 的公共数据面；`oracle` 实现 TSO 的 18 位物理/逻辑编解码。
- 服务 trait：`PdClient`、`Codec`、`Storage`、`ExternalStorage`、`txnlock::LockResolver`、`glue::{Glue, Session, SessionCtx, Progress}`、`statistics::Handle`、`meta::Reader` 定义调用方必须注入的能力。`IdentityCodec`、`MemStorage`、`MemStatsHandle`、`MemMeta` 等是本地实现。
- 元数据/checkpoint：`metautil::{MetaWriter, MemMetaWriter, StatsWriter}` 收集 schema、DDL 和文件；`checkpoint::{LoadCheckpointMetadata, SaveCheckpointMetadata, StartCheckpointRunnerForBackup, AppendForBackup, WalkCheckpointFileForBackup}` 提供 JSON 元信息读写及精简 runner。
- 区间/错误核心：`rtree::{RangeTree, ProgressRangeTree}` 记录原始区间、已完成子区间和 checksum；`utils::{HandleBackupError, WithRetry, SummaryFiles}` 分类备份错误、执行三次重试并聚合文件统计。
- 其他兼容模块：`gc`、`storewatch`、`filter`、`meta`、`distsql`、`ddl`、`conn`、`label`、`infosync`、`checksum`、`berrors` 等分别提供 safepoint、store 拓扑、过滤、表范围、DDL 历史、label rule、checksum 和领域错误的最小子集。

## 执行流程

主调用链由三个生产模块组合本文件，而不是从 `stubs.rs` 自身启动：

1. `client.rs` 通过 `PdClient`/`Storage` 获取集群与版本信息，使用 `gc::CheckGCSafePoint`、`checkpoint`、`meta::Reader` 和 `distsql::BuildTableRanges` 构造备份范围及元数据，并用 `rtree::ProgressRangeTree` 跟踪完成度。
2. `store.rs` 将 `backuppb::BackupRequest` 交给注入的 `BackupClient`，消费 `BackupStream::Recv`，用 `utils::WithRetry`、`HandleBackupError`、`storewatch` 和 failpoint 决定重试、失败或刷新 store；`Context`/取消句柄负责终止接收路径。
3. `schema.rs` 使用 `meta::Reader`、`statistics::Handle`、`checksum::ExecutorBuilder` 与 `metautil::MetaWriter` 备份 schema、统计和 checksum；`WorkerPool` 限制并行任务，`wait_jobs` 汇总错误。

`ProgressRangeTree::GetIncompleteRanges` 是文件内最完整的状态流程：按每个 `Origin` 排序已完成片段，计算洞位；完整区间的文件写入 `MetaWriter`，以 XOR/求和更新 checksum，触发完成回调，最后从待处理集合移除。`utils::HandleBackupError` 则先按 KV/Region 结构化错误重试，再按权限、缺文件、凭证、取消和连接错误文案分类，未知错误按 store ID 计数，超过配额后放弃。

## 数据与状态

状态主要保存在进程内同步容器中：`Context` 共享取消原因；`MemStorage` 用 `Mutex<HashMap<String, Vec<u8>>>` 保存文件；`MemMetaWriter` 分别收集 schemas、ddls、files 及 started/finished 标志；`ProgressRangeTree` 保存未完成区间、physical ID 到 checksum 的映射、回调与可选 writer；`storewatch::Watcher` 保存上一轮 store 快照。

测试注入状态分为全局和线程局部两类。`failpoint`、`infosync::RULES`、`summary` 使用全局 `Mutex`，`SKIP_ROUND_SLEEP` 与 GC 序号使用原子量；`checksum::NEXT_RESP` 是线程局部的一次性响应，避免并行测试跨线程串扰。`RangeTree`、`ErrorContext` 和多个内存后端可克隆或共享，但它们均不持久化，进程退出后状态消失。

重要不变量包括：区间采用半开形式 `[start, end)`；`ProgressRangeTree::Insert` 拒绝重叠原始区间；checksum 的 CRC 使用异或，KV/字节数使用加法；`Context::WithCancel` 只复制父 context 当下的错误，之后父级取消不会自动传播到子级；TSO 的物理部分左移 `18` 位。

## 依赖与调用关系

crate 依赖面由 `br/pkg/backup/Cargo.toml` 限定为 `serde`/`serde_json`；其余均来自 `std`。内部主要依赖关系为：协议模型被 `client.rs`、`store.rs`、`schema.rs` 共同消费；`ProgressRangeTree` 下游调用 `MetaWriter::Send`；checkpoint 下游调用 `ExternalStorage::{ReadFile, WriteFile}`；store watcher 和 `conn::GetAllTiKVStoresWithRetry` 下游调用 `PdClient::GetAllStores`；schema checksum 下游调用注入的 `KvClient` 和进度回调。

RustCodeGraph 的文件查询报告该文件被 158 个文件使用，并列出 `br/cmd/br/cmd.rs`、`br/cmd/br/debug.rs`、`br/cmd/br/stream.rs` 等上层用户；在本 crate 内，直接 import 证据集中于 `client.rs`、`store.rs`、`schema.rs` 及对应独立测试。对 `HandleBackupError` 等同名符号执行带文件限定的 callers 查询没有返回跨模块边，说明当前索引对重导出/同名兼容符号的精确调用边覆盖有限；本文不据此宣称“无调用者”，而采用 import、测试引用和 RustCodeGraph 的文件级 used-by 结果交叉验证。

## 错误处理与边界

`Error` 只有消息字符串，不保存类型码、栈或完整 cause 链；`Trace` 是透传，`Cause` 返回自身。锁中毒处统一 `unwrap`，因此 panic 不会被转换为业务错误。`WorkerPool` 能将子线程 panic 映射为 `worker panicked`，并保留已完成任务的首个错误；`bounded_worker_pool_preserves_completed_errors` 专门覆盖并发槽复用时不能丢失前一任务错误。

已知精简边界必须在扩展时保留警惕：`checkpoint::AppendForBackup` 是成功空操作，`WalkCheckpointFileForBackup` 总返回零耗时，runner 不启动真实刷盘循环；`ddl::GetAllDDLJobs` 返回空；`conn` 忽略过滤行为与重试；`distsql::BuildTableRanges` 只生成一个简化表前缀区间；`objstore::New` 总创建内存存储；`checksum` 默认以 table ID 构造假响应；部分 builder 参数被忽略。这些符号足以验证调用契约，但不能替代线上实现。

`client_test.rs::test_on_backup_response` 证明未知错误先重试、超过配额后失败，权限错误立即放弃，KV lock 返回锁信息；`test_build_progress_range_tree` 覆盖未命中、完整包含、越界和完成回调。`store_test.rs` 覆盖 context 取消、首包/后续包超时及停止清理。这些测试是当前边界行为的直接证据。

## 并发与资源生命周期

`WorkerPool` 以线程数量近似 Go worker pool：未满时直接 spawn；满时取出一个 handle，在新线程中先 join 前一任务再执行当前任务，从而维持槽位上限并保留错误。`wait_jobs` 必须由调用方最终调用以 join 所有 handle。它与 Go channel worker pool 并非完全同构，但独立测试锚定了最关键的错误保留语义。

共享状态主要由 `Mutex` 或原子变量保护；回调在锁持有期间可能被调用，例如 `ProgressRangeTree::GetIncompleteRanges` 持有区间锁时写 meta/触发回调，因此扩展回调时应避免重入同一对象造成死锁。`MemStorage::WalkDir` 先复制条目再释放锁后调用用户函数，避免在回调期间持有文件表锁。

取消句柄写入共享错误即可，没有异步通知原语；轮询者必须主动检查 `Done`/`Err`。全局 failpoint 多数 `take_*` 会消费值，但 `hint_backup_start`、`reset_retryable`、`reset_not_retryable` 与 store tick 是读取/克隆语义，测试必须显式清理，防止跨用例污染。

## 与 Go 版本的对应关系

本文件没有同路径 `stubs.go`；它将多个 Go 包的必要子集聚合到一个 Rust 文件。主要对应来源包括：`br/pkg/utils/error_handling.go` 的 `ErrorContext`/`HandleBackupError`，`br/pkg/rtree/rtree.go` 的 `ProgressRangeTree`，`br/pkg/checkpoint/backup.go` 的 backup checkpoint API，`pkg/util/worker_pool.go` 的 `WorkerPool`，`pkg/distsql/request_builder.go` 的 `BuildTableRanges`，`br/pkg/gc/safepoint.go` 的 safepoint 常量/ID，`br/pkg/utils/misc.go` 的 `SummaryFiles`，以及 `br/pkg/version/version.go` 的表信息版本常量。

需要明确记录的差异：Go checkpoint runner 有后台周期刷写，本桩只有内存标志和 JSON metadata；Go rtree 使用泛型 B-tree 并增量聚合 checksum，本桩用加锁 `Vec` 和简化覆盖规则；Go `WorkerPool` 复用具名 worker/channel，本桩使用 OS 线程与 handle 链；Go context 支持父子传播，本桩仅在派生时复制父错误；Go GC 默认 TTL 当前为 5 分钟，而本桩常量为 `120`。因此修改这些值或行为前，必须先判断目标是保持现有 Rust 测试契约，还是推进真实 Go 等价迁移，不能把桩的现状反向当成 Go 规范。

## 扩展指南

新增 backup 能力时优先在真实拥有者模块实现，仅在需要解除重依赖或支持独立测试时扩展本文件。新增协议字段应同步 `backuppb`/`model` 和序列化需求；新增外部动作应优先扩展 trait，再在测试文件提供 mock；不要让 `client.rs`/`store.rs` 直接依赖测试专用全局状态。

修改关键子系统时至少同步以下独立测试：区间与错误策略改动更新 `client_test.rs` 和 `parity_test.rs`；RPC/取消/超时及 failpoint 改动更新 `store_test.rs`；meta、统计、checksum、label/filter 改动更新 `schema_test.rs` 与 `schema_merge_option_test.rs`。Rust 单元测试继续保存在这些独立文件，不应嵌回 `stubs.rs`。

扩展时特别检查：半开区间与重叠判定、错误重试次数、回调持锁重入、全局测试状态清理、序列化兼容、Go 常量漂移、假实现被误用于生产的风险。若某个边界需要真实 PD/TiKV 能力，应迁移到合适的上游 crate/带 tag 依赖，而不是继续把完整子系统堆入本地桩。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/backup` 定位本 crate 生产与独立测试；`node --file br/pkg/backup/stubs.rs` 分段读取全部 2466 行并确认主要模块；文件级结果显示 158 个使用者。
- RustCodeGraph 调用查询：对 `HandleBackupError`、`NewProgressRangeTree`、`WorkerPool`、`StartCheckpointRunnerForBackup`、`BuildTableRanges` 执行带 `--file br/pkg/backup/stubs.rs` 的 callers/callees；精确 callees 能确认 `HandleBackupError` 构造 `HandleResult`，跨模块 callers 未解析，已以直接 import 和测试引用补证。
- Rust crate 边界：`br/pkg/backup/Cargo.toml`、`br/pkg/backup/lib.rs`；直接消费者：`br/pkg/backup/client.rs`、`br/pkg/backup/store.rs`、`br/pkg/backup/schema.rs`。
- Go 对照：`br/pkg/utils/error_handling.go`、`br/pkg/rtree/rtree.go`、`br/pkg/checkpoint/backup.go`、`pkg/util/worker_pool.go`、`pkg/distsql/request_builder.go`、`br/pkg/gc/safepoint.go`、`br/pkg/utils/misc.go`、`br/pkg/version/version.go`。
- 独立 Rust 测试：`br/pkg/backup/parity_test.rs`、`client_test.rs`、`store_test.rs`、`schema_test.rs`、`schema_merge_option_test.rs`。重点用例为 `go_rust_public_contract_matches`、`bounded_worker_pool_preserves_completed_errors`、`test_on_backup_response`、`test_build_progress_range_tree`、`test_observe_store_changes_async` 与 timeout 系列测试。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务给定命令验证目标文件存在且固定二级章节恰好为 11 个，并人工检查未把空操作描述为生产能力。
