# `pkg/ddl/ingest/mock.rs`

## 文件定位

`mock.rs` 是 `astersql-ddl-ingest` crate 中的 ingest 测试替身模块，由 [`lib.rs`](lib.rs) 以 `pub mod mock` 无条件导出。它不是 DDL owner、job 状态机或真实本地 ingest 引擎的生产入口；它在单元测试中包装真实 [`BackendContext`](backend.rs)，并用内存行缓冲实现 [`Engine`](engine.rs) / [`Writer`](engine.rs) 接口。

在完整 DDL 链路中，ingest 属于加索引等 reorg/backfill 的写入与导入环节；本文件只为该环节提供可观测、不依赖真实本地引擎的测试对象。当前 Rust 直接使用点只在 [`mock_test.rs`](mock_test.rs)；RustCodeGraph 曾将 `backfilling_import_cloud.rs` 和 `index_presplit.rs` 列为文件级使用者，但源码搜索没有找到它们对本文件公开类型的直接引用，因此不把这两条视为已验证的运行时调用边。

## 核心职责

- `MockBackendContext` 保留真实 `BackendContext` 的检查点和 ingest 语义，同时将每次 `update_chunk(id, count, done)` 的 `(id, count)` 追加到 `written_chunks`，便于测试断言读取端上报过的进度。
- `MockEngineInfo` 实现 `Engine`，为一个 `index_id` 创建可共享的内存写入器，并提供 `rows()` 快照和 `set_hook()` 注入点。
- 私有 `MockWriter` 实现 `Writer`：未设 hook 时保存 key/value 的拥有副本；设置 hook 时由 hook 替代默认保存路径。
- `flush` 、`close` 和 `written_bytes` 保持 Go mock 的轻量语义：前两者不改变数据，写入字节数始终为 `0`。

## 主要符号

- `pub struct MockBackendContext { pub backend: BackendContext, pub written_chunks: Vec<(usize, usize)> }`：可观测的后端包装器。`new` 保留传入后端并创建空历史。
- `MockBackendContext::{ingest,next_start_key,total_key_count,add_chunk,finish_chunk,import_ts,advance_watermark}`：原样转发到 `BackendContext` 的同名方法；其中 `ingest` 和 `advance_watermark` 保留 `Result<(), String>` 错误。
- `MockBackendContext::update_chunk`：先调用 `backend.update_chunk`，再记录 `(id, count)`。`done` 只由真实后端消费，不写入历史项。
- `type WriteHook = Arc<dyn Fn(&[u8], &[u8]) + Send + Sync>`：私有回调类型；`Send + Sync` 允许其被实现了 `Send + Sync` 的引擎在多线程测试中共享。
- `pub struct MockEngineInfo`：持有私有 `index_id`、`Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>` 行缓冲和 `Arc<Mutex<Option<WriteHook>>>` hook 槽。
- `MockEngineInfo::{new,set_hook,rows}`：分别初始化空引擎、覆盖当前 hook、克隆并返回当前全部行。`set_hook` 没有取消 hook 的公开路径。
- `impl Engine for MockEngineInfo`：`flush` 总是成功，`close` 总是 no-op，`create_writer` 克隆共享状态并总是成功，`index_id` 返回构造值。
- `struct MockWriter` 及其 `Writer` 实现：只能通过 `create_writer` 构造；`write_row` 执行 hook 或内存追加，`written_bytes` 返回 `0`。

文件没有模块常量、枚举、宏、异步函数或条件编译项。

## 执行流程

1. 测试构造真实 `BackendContext`（例如配置 `CheckpointManager`、`MemRoot` 和 `DiskRoot`），再传给 `MockBackendContext::new`。
2. 扫描端用 `add_chunk` 登记分片，用 `update_chunk` 上报读取数量与完成标志；包装器先更新真实检查点，再将 `(id, count)` 写入 `written_chunks`。
3. 写入端用 `finish_chunk` 上报完成数量。当读写数量和批次条件满足时，后端的检查点管理器才能连续推进本地水位线。
4. `ingest()` 调用真实 `BackendContext::ingest`：刷新其已注册引擎、增加导入计数，再以 `imported = true` 推进检查点。测试也可直接调用 `advance_watermark(imported)` 分别模拟“仅刷新”或“已导入”。
5. 引擎路径由 `MockEngineInfo::new(index_id)` 开始；`create_writer(worker_id)` 忽略 worker ID，但返回一个与引擎共享 rows/hook 的 writer。
6. `MockWriter::write_row` 锁定 hook 槽。有 hook 时同步调用并立即返回，不写 rows；无 hook 时再锁定 rows，复制 key/value 并按调用顺序追加。
7. `rows()` 返回深拷贝快照；后续写入不会改变既有快照。`flush` 和 `close(cleanup)` 不改变缓冲，包括 `cleanup = true` 的情况。

## 数据与状态

- 检查点的真实状态属于 `MockBackendContext.backend` 内的 `BackendContext` / `CheckpointManager`；mock 不复制水位线、import TS 或分片表。
- `written_chunks` 是追加型观测日志，记录每次 `update_chunk` 的 ID 和本次 count，包括重复 ID 和零 count；它不记录 `done`，也不会在 ingest 后自动清空。
- rows 使用 `Vec<(Vec<u8>, Vec<u8>)>`，因此保留插入顺序和重复 key；这与真实 [`EngineInfo`](engine.rs) 的 `BTreeMap` 有意不同，后者按 key 有序且同 key 覆盖。
- 同一个 `MockEngineInfo` 创建的所有 writer 共享 rows 与 hook。`set_hook` 会替换槽中的回调，已创建的 writer 也能看到新 hook，因为它们共享同一 `Arc<Mutex<Option<WriteHook>>>`。
- mock 没有 closed/flushed 标志、writer 计数、字节计数、内存/磁盘配额状态，也不模拟唯一索引的重复键检查。

## 依赖与调用关系

- 上游：[`mock_test.rs`](mock_test.rs) 直接构造 `MockBackendContext` 和 `MockEngineInfo`，验证检查点委托、hook 替换默认写入、`close(true)` no-op 三类契约。在当前 Rust 源码中未发现生产模块直接构造这些类型。
- 下游：`MockBackendContext` 调用 [`backend.rs`](backend.rs) 的 `BackendContext`；其检查点语义继续下沉到 [`checkpoint.rs`](checkpoint.rs) 的 `CheckpointManager`。
- 接口边界：`MockEngineInfo` 与 `MockWriter` 分别实现 [`engine.rs`](engine.rs) 定义的 `Engine: Send + Sync` 和 `Writer: Send`，因而能在需要 trait object 的测试路径中代替真实引擎。
- crate 边界：[`Cargo.toml`](Cargo.toml) 定义包名 `astersql-ddl-ingest`、`lib.rs` 入口，并以 `[package.metadata.porting].go-package = "pkg/ddl/ingest"` 记录 Go 来源包。本文件本身只用 Rust 标准库与同 crate 模块，不直接使用 Cargo 外部依赖。
- RustCodeGraph 已索引 `MockBackendContext`、`MockEngineInfo` 和 `Writer::write_row`，但对这些 Rust 符号的 `callers/callees` 查询未返回边；上述调用关系由索引的文件源码和精确 `rg` 引用搜索交叉验证。

## 错误处理与边界

- `MockBackendContext::ingest` 和 `advance_watermark` 不捕获或改写错误，直接向上返回 `BackendContext` 的 `String` 错误。其他检查点委托方法没有 `Result` 边界。
- `MockEngineInfo::{flush,create_writer}` 和 `MockWriter::write_row` 的类型签名允许返回错误，但当前 mock 实现总是 `Ok`；它不能注入 flush、writer 创建或行写入失败。
- 所有 `Mutex` 都用 `lock().unwrap()`。若持锁线程 panic 导致锁中毒，后续调用会 panic，而不是转成 `Result::Err`。
- hook 在 hook 槽的互斥锁仍被持有时调用。hook 若同步重入调用需要再取该 hook 锁的方法（如 `set_hook` 或另一次 `write_row`），可能自锁；扩展 hook 行为时必须考虑这一边界。
- `close(cleanup)` 无论参数为何都不会阻止后续写入或清理 rows；不应用此 mock 验证真实引擎的 close/cleanup 生命周期。
- `rows()` 克隆全量数据，大数据测试中有额外内存和拷贝成本；该 API 定位为断言辅助，不是高性能读取面。

## 并发与资源生命周期

- rows 和 hook 各有独立 `Arc<Mutex<...>>`；克隆到多个 writer 后，所有写入的 rows 修改被串行化，hook 的替换和读取也被串行化。
- 无 hook 的 `write_row` 持有 hook 锁后再获取 rows 锁；`rows()` 只获取 rows 锁，`set_hook()` 只获取 hook 锁。当前内部没有反向的 rows→hook 取锁路径，但用户 hook 的重入行为仍受上节限制。
- `MockWriter` 不实现自定义 `Drop`；writer 销毁只减少 `Arc` 引用计数，不释放配额、不从引擎注销，也不刷新数据。
- `MockEngineInfo::close` 不改变任何状态；rows 和 hook 一直存活到引擎及所有 writer 的 `Arc` 副本均被释放。
- `MockBackendContext` 本身不使用锁，所有更新方法都要求 `&mut self`；它不是用来模拟 Go `MockBackendCtx` 中共享 session 互斥锁的对象。

## 与 Go 版本的对应关系

Go 直接对照文件是 [`mock.go`](mock.go)，Cargo porting metadata 也指向 `pkg/ddl/ingest`。当前 Rust 是聚焦测试所需契约的移植，不是 Go mock 的完整 API 复刻。

- 已对齐：检查点查询/更新方法、ingest 后推进已导入水位线的目标语义；`MockEngineInfo::Flush` 成功、`Close` no-op、创建 writer 成功；hook 完全替代默认写入路径；`WrittenBytes` 返回零。
- Rust 扩展：`MockBackendContext` 包装 Rust 真实 `BackendContext` 并增加 `written_chunks`；`MockEngineInfo` 将默认写入保存在内存 `Vec` 以供断言，而 Go 默认路径通过 session transaction 的 `Txn(true).Set(key, idxVal)` 写入存储。
- 尚未对齐：Rust 没有 Go `NewMockBackendCtx(job, sessCtx, cpOp) -> BackendCtx` 构造和 `BackendCtx` 完整接口实现，也没有 `Register`、`FinishAndUnregisterEngines`、`CollectRemoteDuplicateRows`、`IngestIfQuotaExceeded`、`GetLocalBackend`、`Close`、`GetDiskUsage` 等 Go 方法。
- 尚未对齐：Rust writer 没有 Go 的 session 事务、`sync.Mutex` 临界区精确形态、`onMockWriterWriteRow` / `afterMockWriterWriteRow` failpoint、`MockExecAfterWriteRow` 全局回调、key/value 日志以及 `LockForWrite`。Rust trait 本身也不接收 context、handle 或 local-writer config。
- Go [`testutil/testutil.go`](testutil/testutil.go) 通过 `mockNewBackendContext` failpoint 把 Go mock 注入加索引集成测试；当前 Rust [`testutil/testutil.rs`](testutil/testutil.rs) 的同名辅助类型用自己的 guard/后端管理机制，源码搜索未显示它构造本文件的 `MockBackendContext`。因此 Go 集成测试不能当作 Rust mock 已接入完整 DDL 主链的证据。

## 扩展指南

- 新增检查点契约时，先在 [`backend.rs`](backend.rs) / [`checkpoint.rs`](checkpoint.rs) 实现真实语义，再在 `MockBackendContext` 增加简薄委托；若需观测历史，明确记录的是增量、累计值还是完成标志，并在独立 [`mock_test.rs`](mock_test.rs) 添加顺序与重复调用断言。
- 需要模拟失败时，不应破坏默认“总是成功”的 Go mock 契约；宜增加显式、可选的错误注入状态，分别覆盖 flush/create/write，并验证一次性与持久性语义。
- 若需 hook 重入或运行较慢的用例，考虑在锁内克隆 `Arc<dyn Fn...>`、释放 hook 锁后再调用，并添加并发替换 hook/写入的独立测试；修改取锁顺序时要评估死锁。
- 若要测试真实 close、cleanup、内存配额、磁盘占用、有序导入或同 key 覆盖，应使用 [`engine.rs`](engine.rs) 的 `EngineInfo` 及其独立 `engine_test.rs`，不要为了这些生产语义改变 mock 的 Go 对齐 no-op 行为。
- 若要将 Rust mock 接入完整 DDL 测试链，需先明确补齐 Go `BackendCtx` 契约与注入点的范围；这是跨 `mock.rs`、backend manager、testutil 和集成测试的独立任务，不应在本文件的局部扩展中隐式完成。
- 修改 Rust 生产源文件时，测试仍应保持在独立 `mock_test.rs`，不嵌入 `mock.rs`；并同步复核 [`mock.go`](mock.go) 及 Go 集成测试使用的 failpoint 契约。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含 11,467 个文件；`files --filter pkg/ddl/ingest` 列出本 crate 的 52 个 Go/Rust 文件；`query MockBackendCtx`、`query MockEngineInfo` 核对了 Go/Rust 对照类型；`query write_row --kind method` 核对 `engine.rs::Writer::write_row`；`node --file pkg/ddl/ingest/mock.rs --offset 1 --limit 320` 读取并列出目标文件全部 140 行及文件级使用提示。`callers/callees` 对目标 Rust 类型未产生可用边，已用源码引用搜索补齐。
- 已读 Rust 生产路径：[`mock.rs`](mock.rs)、[`backend.rs`](backend.rs)、[`checkpoint.rs`](checkpoint.rs)、[`engine.rs`](engine.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)；仓库约束入口为 [`pkg/ddl/doc.go`](../doc.go) 和 `docs/agents/ddl/README.md`。
- 已读独立 Rust 测试 [`mock_test.rs`](mock_test.rs)：`mock_backend_forwards_the_complete_checkpoint_contract` 证明分片进度、import TS 和水位线委托；`hook_replaces_the_default_write_path_like_go` 证明 hook 替代 rows 追加且字节数为零；`mock_engine_close_is_a_noop_like_go` 证明 `close(true)` 不清除行。
- 已读 Go 对照 [`mock.go`](mock.go) 和注入辅助 [`testutil/testutil.go`](testutil/testutil.go)，并搜索 `integration_test.go` 对 `InjectMockBackendCtx`、`onMockWriterWriteRow` 和 `afterMockWriterWriteRow` 的使用，用于区分已移植契约与尚未接线的 Go 能力。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时用任务给定的 `test -f` + 11 章节 `rg -c` 命令做结构验证，并用 `git diff --check` 与目标范围 diff 做文档质量复核。
