# `pkg/ddl/ingest/engine.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate（入口为 `pkg/ddl/ingest/lib.rs`，清单为 `pkg/ddl/ingest/Cargo.toml`），定义 DDL ingest 子系统的引擎/写入器接口及一个进程内实现。它服务于加索引回填时“按索引暂存编码后的键值对”这一局部职责，不负责 DDL job 持久化、schema 状态迁移、真正的 SST 生成或向 TiKV 导入。

直接装配点是 `BackendContext::register`（`pkg/ddl/ingest/backend.rs`）：每个 index ID 创建一个 `Arc<EngineInfo>`，保存在 `BackendContext.engines`。`BackendContext::flush_engines`、`ingest`、`finish_and_unregister` 和 `close` 再通过 `Engine` trait 驱动其生命周期；`pkg/ddl/ingest/engine_mgr.rs` 提供批量注册/注销门面。crate 的 `lib.rs` 将 `engine` 声明为公开模块，并把独立测试 `engine_test.rs` 仅在 `cfg(test)` 下接入。

## 核心职责

- `Writer` 定义写入一对编码后字节 key/value 以及查询累计写入字节数的最小协议。
- `Engine` 定义 flush、close、按 worker 创建 writer、查询 index ID 的生命周期协议；trait 要求 `Send + Sync`，writer 要求 `Send`。
- `EngineInfo` 保存索引元数据、共享状态、内存记账对象和单 writer 固定预留额，是当前 Rust 侧的内存实现。
- `EngineState` 用 `BTreeMap<Vec<u8>, Vec<u8>>` 按 key 有序保存数据，并记录 writer 数、关闭状态和 flush 状态。
- `WriterContext` 将多个 writer 连接到同一个 `EngineState`，创建/销毁时通过 `MemRoot` 预检、消费和释放固定配额。
- `ResourceTracker for EngineInfo` 以当前 map 中 key/value 的长度和估算空间占用，供 `DiskRoot` 一类配额组件查询。

这里的 `flush` 只把 `EngineState.flushed` 置为 `true`，没有落盘或导入副作用；行数据仍在内存 map 中。因而本文件是可工作的内存语义实现，但不能等同于 Go 版本基于 Lightning `OpenedEngine`/`EngineWriter` 的真实本地引擎。

## 主要符号

- `pub trait Writer: Send`：`write_row(&mut self, key, value) -> Result<(), String>` 写一行；`written_bytes()` 返回该 writer 自创建以来按输入长度累计的逻辑字节数。
- `pub trait Engine: Send + Sync`：公开 `flush`、`close(cleanup)`、`create_writer(worker_id)` 和 `index_id`。错误目前以字符串表达。
- `EngineState`（私有）：`rows`、`writers`、`closed`、`flushed` 均在同一把 `Mutex` 下维护。`rows` 的同 key 插入采用 map 覆盖语义。
- `pub struct EngineInfo`：`index_id` 和 `unique` 描述所属索引；`tag` 用于内存记账；`state` 由所有 writer 共享；`writer_memory` 在 `new` 中被截断到不小于零。
- `EngineInfo::new`：构造空、未关闭、未 flush 的引擎。它本身不向 `MemRoot` 消费内存。
- `EngineInfo::rows`：锁住状态并克隆整张 map，供 `BackendContext::collect_remote_duplicate_rows` 及测试读取；调用成本与全部缓冲数据规模成正比。
- `EngineInfo::unique`：暴露唯一索引标志，供后端决定是否检查重复值。
- `EngineInfo` 的 `Engine` 实现：完成状态检查、writer 配额登记、关闭和 flush 标记。
- `EngineInfo` 的 `ResourceTracker` 实现：遍历 `rows`，求所有当前 key/value 长度之和；不计 map、向量容量等额外开销。
- `WriterContext`（私有）：持有共享状态、`MemRoot`、writer 标签和局部字节计数；其 `Drop` 负责减少 writer 计数并释放标签记账。
- `WriterContext::lock_for_write`：返回共享状态锁守卫；由于具体类型私有且该方法不在 `Writer` trait 中，当前模块外通过 `Box<dyn Writer>` 无法调用，仓库查询也未发现生产调用。

本文件没有模块级常量、条件编译分支、异步函数或后台任务。

## 执行流程

1. `BackendContext::register` 校验后，为每个 index ID 调用 `EngineInfo::new`，tag 形如 `job-{job_id}-index-{index_id}`，并把 `Arc<EngineInfo>` 注册到 map。
2. 回填方调用 `Engine::create_writer(worker_id)`。该方法在持有状态锁时先拒绝已关闭引擎，再用 `MemRoot::check_consume(writer_memory)` 做配额预检；成功后以 `{engine_tag}-writer-{worker_id}` 消费配额、递增 `writers`，返回 `Box<dyn Writer>`。关闭检查与 writer 登记位于同一临界区，避免创建与 `close` 交错后把新 writer 挂到已关闭引擎。
3. `Writer::write_row` 获取同一状态锁；若已关闭返回 `"engine closed"`，否则复制 key/value 到 map。已有 key 会被新 value 替换，但 `bytes` 仍按每次调用的输入长度累加，并使用 `saturating_add` 避免 `i64` 溢出。
4. `Engine::flush` 在引擎未关闭时仅设置 `flushed = true`。`BackendContext::flush_engines` 串行调用所有引擎的该方法，`ingest` 随后只更新导入次数和 checkpoint 水位；本文件没有真正的存储导入。
5. `Engine::close(cleanup)` 设置 `closed = true`；`cleanup=true` 时清空 map，否则保留行供检查。随后释放 engine 自身 tag 的记账。已存在 writer 没有被强制销毁，但后续写入会因 `closed` 被拒绝。
6. writer 离开作用域时，`Drop` 尝试取得状态锁并饱和递减 `writers`，无论锁是否成功都会调用 `release_with_tag` 释放 writer tag。

## 数据与状态

`EngineState` 是唯一的可变共享状态。`Arc<Mutex<EngineState>>` 使 `EngineInfo` 与所有 `WriterContext` 指向同一份 map 和生命周期标志；`BTreeMap` 保证迭代按 key 排序，但不保留重复 key，也不保存写入历史。`rows()` 返回快照而不是借用，因此调用方不会在遍历时长期占有引擎锁。

状态转换是单向的：初始 `closed=false, flushed=false`；`flush` 可重复执行并保持 `flushed=true`；`close` 可重复调用并保持 `closed=true`，没有 reopen/reset。当前没有读取 `flushed` 或 `writers` 的公开接口，这两个字段只记录内部事实，不会触发导入或阻止关闭。

内存记账是“每个 writer 的固定预留额”，不是行数据的实际容量：`write_row` 不随数据增长调用 `consume_with_tag`。`disk_usage` 则按 map 当前内容动态计算逻辑字节数。`writer_memory < 0` 在构造时变为 0。若相同 engine 上重复使用同一个 `worker_id`，writer tag 会相同；`MemRootImpl` 会把消费累加到同一 tag，而任一 writer drop 的 `release_with_tag` 会一次释放该 tag 的全部记账，因此调用方应把 worker ID 当作同时存活 writer 的唯一标识。

## 依赖与调用关系

上游直接关系：

- `pkg/ddl/ingest/backend.rs::BackendContext::register` 构造 `EngineInfo`；`collect_remote_duplicate_rows` 读取 `unique()` 与 `rows()`；`flush_engines` 调用 `flush()`；结束与关闭路径调用 `close()`。
- `pkg/ddl/ingest/engine_mgr.rs::register_engines` 转发到 backend 注册；`finish_and_unregister_engines` 关闭引擎，并可在关闭后检查重复值、清空注册表。
- RustCodeGraph 对 `EngineInfo` 的定义追踪列出 `engine_mgr.rs` 和 `engine_test.rs` 的导入；对 `create_writer`/`write_row` 的调用追踪主要落在 `engine_test.rs`、`engine_mgr_test.rs` 和 mock 测试。生产 `pkg/ddl/backfilling_operators.rs::execute_add_index_pipeline` 会汇总 writer 的 `written_bytes` 接口结果，但其管线通过泛型 `IndexWriter` 接线，不构成对 `EngineInfo::create_writer` 的直接调用证据。

下游依赖仅为标准库 `BTreeMap`、`Arc`、`Mutex`，以及同 crate 的 `MemRoot` 与 `ResourceTracker`。本文件自身不直接使用 `Cargo.toml` 中的 `fs2`、`fail` 或 `astersql-util-dbterror`；crate 清单的 Windows 条件依赖也不是本文件当前实现的直接依赖。

## 错误处理与边界

- 关闭后 `flush`、`create_writer`、已有 writer 的 `write_row` 都返回精确字符串 `"engine closed"`；`engine_test.rs::closed_engine_rejects_flush_writes_and_new_writers` 验证三条路径，并验证 `close(false)` 保留旧数据。
- writer 配额预检失败返回 `"memory used up"`。预检与消费并非 `MemRoot` 的单个原子操作；当前 `create_writer` 通过 engine 状态锁串行化同一引擎的创建，但共享同一个 `MemRoot` 的不同引擎仍可能同时通过预检后再消费。
- 所有正常锁获取都使用 `lock().unwrap()`；若持锁线程 panic 导致 mutex poisoned，`new` 以外的大多数 API 会继续 panic，而不是返回 `Result` 错误。只有 `WriterContext::drop` 使用 `if let Ok` 忽略 poisoned 状态。
- `close` 返回 `()`，无法向调用方报告清理或记账异常；当前 `MemRoot` 操作本身也无返回错误。
- map 插入覆盖相同 key，故该层不会保留同 key 的两条记录。后端的唯一性检查当前统计的是不同 map entry 的相同 value，这与 Go `WriteRow` 携带 handle/RowID 的重复检测信息并不等价。
- `rows()` 克隆全部数据，`disk_usage()` 全表扫描；大数据量或频繁调用会有明显时间与内存成本。
- 关闭时 engine tag 的释放不等同于释放仍存活 writer 的独立 tag；writer 配额直到各自 drop 才释放。`cleanup` 仅控制 map 是否清空，不控制 writer tag 生命周期。

## 并发与资源生命周期

`EngineInfo` 可跨线程共享，所有行、标志和 writer 计数由一把 mutex 串行保护，因此写入正确性简单但吞吐会受单锁限制。`write_row` 会在锁内分配并复制 key/value；`rows` 与 `disk_usage` 也在锁内遍历/克隆，调用期间阻塞所有 writer。

创建 writer 时，关闭检查、配额消费和 writer 计数递增处在同一个 engine 临界区；这一设计明确避免 `close` 与创建之间的生命周期竞态。`close` 不等待 `writers == 0`，而是先标记关闭，使已经取得的 writer 在下一次写入时失败。writer 的 Rust `Drop` 是配额归还的最终保障；调用方无需显式 close writer，但必须让对象及时析构。

本文件不创建线程、任务或通道，也没有取消上下文。相比 Go 版本的 `flushLock: RWMutex`，Rust 当前实现没有让 writer 暴露可用的 flush 读锁协议；`lock_for_write` 只是同一状态 mutex 的守卫，并未进入公开 trait。若未来引入真实并行 flush，必须先明确写入与 flush 的锁顺序、等待策略和关闭时的 writer 排空规则。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/ingest/engine.go`。接口外形大致对应：Rust `Engine::{flush, close, create_writer}` 对应 Go `Engine::{Flush, Close, CreateWriter}`，Rust `Writer::{write_row, written_bytes}` 对应 Go `Writer::{WriteRow, WrittenBytes}`，`EngineInfo`/`WriterContext` 对应 Go 的 `engineInfo`/`writerContext`。

关键差异如下：

- Go `engineInfo` 持有 context、job ID、UUID、Lightning `OpenedEngine`、backend、按 worker ID 缓存的 `EngineWriter` 和 flush `RWMutex`；Rust 只持有进程内 map、index 元数据与配额对象。
- Go `Flush` 调用 `OpenedEngine.Flush`，会产生真实后端副作用；Rust 仅设置布尔标志。
- Go `Close` 先关闭缓存 writers，再关闭 opened engine，并可调用 `CleanupEngine` 删除中间文件；Rust 仅标记关闭、可选清 map 和释放标签。
- Go `CreateWriter` 接收 `LocalWriterConfig`，刷新消费统计，并按 worker ID 复用本地 writer；Rust 接收预先配置在 engine 上的固定 `writer_memory`，每次都新建上下文，不缓存。
- Go `WriteRow` 接收 context 与 handle，将 handle 编入 `KvPair.RowID` 后调用 `AppendRows`；Rust 不接收 context/handle，只把 key/value 复制进 map。
- Go `LockForWrite` 是公开 `Writer` 接口的一部分，并用共享 `RWMutex` 与 flush 协调；Rust 的同名小写方法不在 trait 中，当前外部不可用。
- Go 错误保留底层错误和日志上下文；Rust 使用少量固定 `String`。

因此，Rust 已覆盖基本接口、关闭门禁、固定配额、字节计数和可测试的内存生命周期，但尚未覆盖 Go 的真实本地引擎、writer 缓存、handle 唯一性信息、上下文取消、日志和 flush/cleanup I/O 语义。扩展时应以这些差异为迁移清单，不能把当前内存行为当作完整 Go 等价实现。

## 扩展指南

- 若增加引擎能力，先修改 `Engine`/`Writer` trait，再同步 `EngineInfo`、`WriterContext`、`pkg/ddl/ingest/mock.rs` 及所有泛型/trait-object 调用点；不要只给具体类型加方法而绕过抽象。
- 若接入真实本地后端，主要落点是 `EngineInfo::{flush, close, create_writer}` 与 `WriterContext::write_row`。需要同时补齐 worker writer 缓存、配置传递、handle/RowID、I/O 错误类型、cleanup 失败处理和 context/cancel 语义。
- 若改变关闭/flush 并发协议，应保留“关闭检查与 writer 注册原子化”的不变量，并设计 flush 与活跃 writer 的同步；避免在持有 engine mutex 时调用可能阻塞或回调本模块的外部 I/O。
- 若改内存记账，应区分 writer 固定开销、writer cache 和行缓冲实际增长；检查共享 `MemRoot` 上预检与消费的原子性，并为重复 worker ID 定义明确策略。
- 若改变重复键保存策略，不能直接用 `BTreeMap::insert` 覆盖；还需同步 `BackendContext::collect_remote_duplicate_rows`，并依据 Go 的 key/value/handle 协议设计数据形状。
- 单元测试必须继续放在独立文件。优先扩展 `pkg/ddl/ingest/engine_test.rs` 覆盖配额失败、cleanup true/false、同 key 覆盖、字节饱和/计数、writer drop 记账和并发 close/create；跨注册/注销行为放在 `engine_mgr_test.rs`，后端聚合行为放在 `backend_test.rs`。若追求 Go 语义对齐，还应核对同目录 Go 测试以及上层 backfilling 集成测试。
- 性能风险集中在单 mutex、锁内复制、`rows()` 全量克隆与 `disk_usage()` 全量扫描；兼容风险集中在错误字符串、trait 方法签名和现有 close(false) 保留数据的契约。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；随后用 `files --filter pkg/ddl/ingest` 确认目标、Go 对照与独立测试均已索引。
- RustCodeGraph `node --file pkg/ddl/ingest/engine.rs`：完整读取 214 行目标文件，核对 `Writer`、`Engine`、`EngineState`、`EngineInfo`、`WriterContext` 及全部 impl。
- RustCodeGraph `node EngineInfo` 与针对 `create_writer`、`write_row`、`written_bytes` 的 explore/call 跟踪：确认定义、导入者及测试调用；查询结果显示 `execute_add_index_pipeline` 汇总 `written_bytes`。
- RustCodeGraph 读取 `pkg/ddl/ingest/backend.rs`、`engine_mgr.rs`、`lib.rs`：确认构造、注册、flush、重复检查、关闭/注销与模块装配关系。
- RustCodeGraph 读取 `pkg/ddl/ingest/engine.go`：逐项核对 Go 接口、真实 Lightning engine/writer、缓存、锁、handle 和 cleanup 行为。
- RustCodeGraph 读取 `pkg/ddl/ingest/engine_test.rs` 与 `engine_mgr_test.rs`：确认关闭后拒绝操作、保留/清理数据、index/unique 顺序、字节初值和重复错误后的关闭状态。当前同目录没有单独的 Go `engine_test.go`；Go 行为证据来自 `engine.go` 及同包上层测试/调用关系。
- 读取 `pkg/ddl/ingest/Cargo.toml`：确认 crate 名、`lib.rs` 入口、Go package 元数据与依赖边界；读取 `mem_root.rs`、`disk_root.rs` 验证标签记账和 `ResourceTracker` 协议。
- 文档只描述当前代码可证实的行为；本任务按计划不运行 Cargo。交付前另以任务指定命令验证目标文件存在且恰有 11 个固定二级标题。
