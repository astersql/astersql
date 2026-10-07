# `br/pkg/gluetidb/mock/mock.rs`

## 文件定位

该文件是 crate `astersql-br-pkg-gluetidb-mock` 的主体实现，由同目录
[`lib.rs`](lib.rs) 通过 `#[path = "mock.rs"] mod mock` 挂载并扁平导出。它实现
[`br/pkg/glue/glue.rs`](../../glue/glue.rs) 定义的 `Glue`、`Session`、
`BatchCreateTableSession` 和 `Progress` 抽象，为 BR 测试提供轻量 TiDB Glue 替身；
它不是 BR 命令或 SQL 请求的生产入口。

同目录 [`Cargo.toml`](Cargo.toml) 将该 crate 标为对应 Go 包
`br/pkg/gluetidb/mock` 的 library，只依赖 `astersql-br-pkg-glue` 与
`astersql-errors`。因此这里没有直接引入真实 `kv`、`domain`、`sessionapi` 或
gRPC 类型，而是用本地接口和占位类型保持测试所需的调用形状。当前可见的外部
Rust 使用点是 [`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs) 中的
`TestRestoreSchemaSuite::MockGlue`。

## 核心职责

1. `MockGlue` 实现 `Glue` 工厂接口，创建 mock session、空 storage、无 I/O 的
   progress，并报告固定版本与客户端类型。
2. `MockSession` 将 SQL 调用转发给可注入的 `SessionAPI`，保持 Go mock 对内部事务
   来源、结果集首次拉取、错误传播和资源关闭的行为。
3. `SessionAPI`、`RecordSet`、`Chunk`、`NilStorage` 与 `NopProgress` 隔离真实 TiDB
   重型依赖，使调用方可以注入精确可观测的测试替身。
4. 对尚未实现的建库、建表、放置策略、表模式和元数据刷新路径显式 panic，防止
   测试误把占位行为当成真实能力。

该文件的目标是测试契约对齐，而不是实现完整 TiDB Glue。尤其是 `GetDomain`、
`Open` 和 `StartProgress` 为适应 Rust trait 返回类型使用了非空占位对象；这与 Go
返回 `nil` 的表示不同，但同目录契约测试只要求“无错误”和对应的可观察行为。

## 主要符号

- `InternalTxnBR: &str = "br"`：本地表示 Go `kv.InternalTxnBR` 的事务来源标签。
- `WithInternalSourceType(Context, &'static str) -> Context`：把最近一次来源写入线程
  局部 `LAST_INTERNAL_SOURCE`，再原样返回 context；
  `take_last_internal_source_for_test` 取出并清空该值，避免测试间残留。
- `Chunk { num_rows }`：只为 `RecordSet::Next` 提供参数形状的最小结果块。
- `RecordSet`：要求实现 `NewChunk`、`Next`、`Close`；`CloseRecordSet` 的 `Drop`
  将 Go 的 `defer rs.Close()` 映射为 Rust RAII。
- `SessionAPI`：可注入底层会话的最小接口，包含带参数的 `ExecuteInternal`、
  `Close` 与 `session_ctx_handle`。
- `MockSession`：持有可选的 `Arc<Mutex<dyn SessionAPI>>` 和私有
  `HashMap<String, String>` 全局变量快照，实现 `Session` 与
  `BatchCreateTableSession`。
- `MockGlue`：持有可选共享 session 和公开 `GlobalVars`，实现 `Glue`；
  `SetSession` 安装共享会话，`clear_session` 将其清空。
- `NilStorage`：`Storage` 占位实现，名称为空串。
- `NopProgress`：用 `AtomicI64` 保存进度，用 `AtomicBool` 保存关闭状态；对外通过
  `Box<dyn Progress>` 返回。

公开 API 由 `lib.rs` 的 `pub use mock::*` 导出。`CloseRecordSet`、`NilStorage`、
`NopProgress` 和 `MockSession::se` 保持文件私有，属于实现细节。

## 执行流程

### 创建与注入

调用方先用 `MockGlue::new/default` 得到空 glue，可通过 `SetSession` 注入
`Arc<Mutex<dyn SessionAPI>>`，并按需填充 `GlobalVars`。`CreateSession` 克隆 session
句柄和变量表，构造独立的 `MockSession`；变量表是创建时快照，之后修改
`MockGlue::GlobalVars` 不会回写已创建的 session。

`UseOneShotSession` 则故意只克隆底层 session，并用空 `HashMap` 创建临时
`MockSession`，随后同步调用传入回调。它不读取 `store`、`closeDomain`，也不自动
调用 `Session::Close`；这些都是当前 Go mock 的既有契约，而不是完整生产生命周期。

### SQL 执行

`MockSession::Execute` 用空参数切片调用 `ExecuteInternal`。后者按顺序：

1. 以 `WithInternalSourceType(ctx, InternalTxnBR)` 标记 BR 内部事务来源。
2. 通过 `MockSession::se` 取得共享 session，锁住 mutex，并调用底层
   `SessionAPI::ExecuteInternal`。底层返回的错误通过 `?` 原样向上传播；锁在调用
   返回后立即释放。
3. 若无结果集，直接成功返回。
4. 若有结果集，将其包进 `CloseRecordSet`，调用一次 `NewChunk(None)`，再调用恰好
   一次 `Next`，用于触发 `ADMIN RECOVER INDEX` 一类惰性副作用。
5. `Next` 返回错误时故意返回 `Ok(())`；正常时也返回成功。无论正常、错误还是
   panic 展开，`CloseRecordSet::drop` 都调用 `RecordSet::Close`。

这里不会排空所有结果行，`Chunk::num_rows` 也不参与判断。若扩展逻辑依赖完整结果
集，不能假定当前方法已经遍历所有行。

### 其他 Glue 行为

- `GetDomain` 返回新的空 `Domain`；`Open` 返回 `NilStorage`。
- `OwnsStorage` 固定为 `true`，`GetVersion` 固定为 `"mock glue"`，`GetClient`
  固定为 `ClientCLP`。
- `StartProgress` 忽略命令名、总数和日志重定向参数，返回从零开始的
  `NopProgress`；`Record` 是空操作。
- `GetGlobalVariable` 优先返回注入值，缺失时返回字符串 `"True"`；
  `GetGlobalSysVar` 固定返回空串。
- `GetSessionCtx` 和 `Close` 分别转发到底层 session 的句柄读取与关闭操作。

## 数据与状态

`MockGlue::se` 和每个 `MockSession::se` 都是同一 `Arc` 的克隆，因此不同 session
可以串行访问同一个注入对象。`MockGlue::GlobalVars` 是公开配置源，但
`CreateSession` 会深拷贝整个 map，`UseOneShotSession` 则明确忽略它。

`LAST_INTERNAL_SOURCE` 是线程局部 `Cell<Option<&'static str>>`：只记录当前线程最近
一次来源，不是跨线程审计记录；读取辅助函数同时清空值。它只验证调用是否打上
来源标签，不改变 `Context` 内容。

`NopProgress.current` 使用 `AtomicI64`，允许共享 trait 对象上的 `Inc`、`IncBy` 与
`GetCurrent`；`closed` 只由 `Close` 写入，目前没有公开读取接口，主要用于实现层
保留关闭状态。`Chunk::num_rows` 默认为零，当前执行路径不修改它。

## 依赖与调用关系

- 上游抽象：[`br/pkg/glue/glue.rs`](../../glue/glue.rs) 的 `Glue`、`Session`、
  `BatchCreateTableSession`、`Progress`、`Storage` 及相关数据类型定义了本文件必须
  满足的接口。
- crate 边界：[`Cargo.toml`](Cargo.toml) 只声明 `astersql-br-pkg-glue` 和
  `astersql-errors`，注释明确说明本地桩用于避开 `kv/domain/kvproto/grpcio` 路径。
- 实际 Rust 上游：[`br/pkg/utiltest/suite.rs`](../../utiltest/suite.rs) 导入
  `astersql_br_pkg_gluetidb_mock::MockGlue`，在 `CreateRestoreSchemaSuite` 中把默认
  glue 与 mock cluster、临时本地存储组合为恢复 schema 测试套件。
- 文件内部调用边：`Execute -> ExecuteInternal -> WithInternalSourceType`；
  `ExecuteInternal -> SessionAPI::ExecuteInternal -> RecordSet::{NewChunk, Next}`；
  `CloseRecordSet::drop -> RecordSet::Close`；`GetSessionCtx -> session_ctx_handle`；
  `NopProgress::Inc -> IncBy`。
- 测试入口：[`parity_test.rs`](parity_test.rs) 由 `lib.rs` 以独立测试模块挂载，直接
  构造 `FakeSession`/`FakeRecordSet` 验证上述调用边和资源行为。

RustCodeGraph 的文件关系将 `mock.rs` 标记为被 `br/pkg/utiltest/parity_test.rs`、
`br/pkg/checkpoint/{log_restore,storage}.rs`、`br/pkg/restore/log_client/import_retry.rs`
等文件关联；其中若只命中通用字段名 `se`，不能据此断言那些文件直接构造
`MockGlue`。文本级 crate 引用确认的直接生产源码使用点目前是 `utiltest/suite.rs`。

## 错误处理与边界

- `SessionAPI::ExecuteInternal` 的错误向上传播；`RecordSet::Next` 的错误被有意吞掉，
  与 Go mock 保持一致。两类错误不可混为一谈。
- `RecordSet::Close` 无返回值，因此关闭失败无法表达或传播。
- 未注入 `SessionAPI` 时，`GetSessionCtx`、`Execute/ExecuteInternal`、`Close` 会在
  `MockSession::se` 的 `expect("nil sessionapi.Session")` 处 panic；变量读取不依赖
  session，仍可成功。
- mutex 污染时，相关方法在 `expect("session lock")` 处 panic。
- `CreateDatabaseOnExistError`、`CreatePlacementPolicy`、`CreateTable`、
  `CreateTables`、`AlterTableMode`、`RefreshMeta` 全部 panic。Rust panic 只模拟 Go
  `log.Fatal` 的“不可继续”意图，并不等价于 Go 的进程退出语义。
- `GetDomain`/`Open` 的占位返回值不能提供真实 domain 或存储能力；调用方只应在
  测试明确覆盖的表面上使用它们。
- `StartProgress` 返回可用对象而 Go 返回 `nil`，因此 Rust 调用方可安全调用
  `Inc/Close`，但不应据此推断存在真实 UI、日志或指标输出。

## 并发与资源生命周期

`SessionAPI` 被 `Arc<Mutex<dyn SessionAPI>>` 包装：`Arc` 允许多个 mock session 共享
所有权，`Mutex` 让底层可变调用串行化。`ExecuteInternal` 只在调用底层 session 时
持锁，随后在处理结果集前释放锁，避免结果集拉取期间长期占用 session mutex。

`CloseRecordSet` 通过 `Drop` 保证结果集在正常返回、`Next` 错误和 panic 展开时关闭；
同目录测试 `result_set_closes_when_next_panics` 专门验证 panic 路径。另一方面，
`MockSession` 自身没有 `Drop` 实现，离开作用域不会自动调用底层 `SessionAPI::Close`；
只有显式 `Session::Close` 才会关闭共享 session。由于多个 `MockSession` 可能共享同一
session，任一实例调用 `Close` 都作用于同一底层对象。

内部事务来源使用线程局部状态，不需要锁，也不会在线程之间传播。`NopProgress`
使用原子量而非 mutex：计数采用 `Relaxed`，只提供原子计数、不提供额外发布顺序；
关闭标志采用 `SeqCst`。当前 API 没有阻止关闭后的继续计数，也没有幂等检查。

## 与 Go 版本的对应关系

直接对照文件是 [`mock.go`](mock.go)：

- Go `mockSession.se sessionapi.Session` 对应 Rust 可选的共享 `SessionAPI` trait 对象；
  Rust 多出的 `Mutex` 满足 trait 对象可变调用与并发共享要求。
- 两端 `Execute` 都委托给 `ExecuteInternal`；都先标记 `InternalTxnBR`，传播底层执行
  错误，对非空结果集只调用一次 `Next`，吞掉 `Next` 错误，并确保关闭结果集。
- Go `GlobalVars` map 与 Rust `MockGlue::GlobalVars`/`MockSession::globalVars` 保持
  “命中返回值、缺失返回 `True`”语义；一次性 session 均不携带该 map。
- Go 未实现方法调用 `log.Fatal` 后写形式上的 `return nil`；Rust 用 panic 表达不可用。
  二者都不应被正常测试流程调用，但故障机制并非逐字等价。
- Go `GetDomain`、`Open`、`StartProgress` 返回 nil；Rust 因 trait 返回非可空对象，
  分别返回空 `Domain`、`NilStorage`、`NopProgress`。
- 两端 `OwnsStorage`、`GetVersion`、`Record`、`GetClient` 的可观察结果一致。

独立 Rust 测试 [`parity_test.rs`](parity_test.rs) 覆盖固定版本/客户端、变量覆盖、SQL
转发、事务来源、底层错误、`Next` 错误、结果集关闭、session 关闭、一次性 session、
未实现路径 panic 与进度计数。Go 侧真实使用样例还可见
`br/pkg/backup/client_test.go`、`br/pkg/restore/data/data_test.go`、
`br/pkg/task/restore_test.go` 和 `br/pkg/utiltest/suite.go`。

## 扩展指南

- 新增 `Glue` 或 `Session` trait 方法时，应先更新 `br/pkg/glue/glue.rs`，再在
  `MockGlue`/`MockSession` 提供与 Go `mock.go` 一致的行为，并在独立
  `parity_test.rs` 增加正常、错误和资源路径；不要把测试模块内嵌回 `mock.rs`。
- 若实现当前 panic 的 DDL/meta 方法，应通过 `SessionAPI` 增加最小必要注入接口，
  同时核对 Go 的参数、错误和副作用顺序。不要仅返回 `Ok(())` 掩盖未实现能力。
- 修改 SQL 结果集逻辑时，必须保留底层执行错误与 `Next` 错误的不同策略，并验证
  所有退出路径都会 `Close`。若改为排空结果集，需要评估额外工作量、潜在阻塞和
  与 Go “只拉一次”的兼容风险。
- 修改 session 所有权时，应明确共享 `Arc` 的关闭语义，避免一个 session 提前关闭
  其他使用者正在共享的底层对象；若需要真正独立 session，应改变注入工厂而非仅
  克隆 `Arc`。
- 扩展 progress 时需决定关闭后计数、溢出、可见性和实际 I/O 语义；当前
  `AtomicI64::fetch_add` 会按 Rust release 模式的整数规则运行，未做上限校验。
- 新增直接消费者时，在其独立测试文件验证契约；若增加 crate 依赖，应同步
  `Cargo.toml`，并评估是否破坏当前避开重型 TiDB/gRPC 依赖的目的。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；目标文件可完整读取，索引将
  `Execute -> ExecuteInternal`、`ExecuteInternal -> WithInternalSourceType/NewChunk/Next`
  和 `CloseRecordSet::drop -> Close` 等边解析出来。
- RustCodeGraph `node --file br/pkg/gluetidb/mock/mock.rs`：核对了 383 行实现中的全部
  常量、线程局部状态、trait、结构体与 impl；`node` 同时列出该文件与 checkpoint、
  restore、utiltest 和 parity 测试区域的关联。
- RustCodeGraph `node --file br/pkg/gluetidb/mock/lib.rs`：确认模块挂载、公开再导出和
  独立测试模块。
- RustCodeGraph `node --file br/pkg/glue/glue.rs`：确认 `Glue`、`Session`、
  `BatchCreateTableSession`、`Progress` 的上游接口契约。
- RustCodeGraph `node --file br/pkg/utiltest/suite.rs`：确认直接 crate 导入和
  `CreateRestoreSchemaSuite` 的装配生命周期。
- 直接读取 [`Cargo.toml`](Cargo.toml)、[`mock.go`](mock.go) 与
  [`parity_test.rs`](parity_test.rs)：分别核对 crate 边界、Go 对照语义和独立测试覆盖。
- 使用 `rg` 复核 Rust crate 名及 Go 包使用点；未运行 Cargo，符合本任务纯文档分析
  的明确范围。
