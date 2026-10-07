# `pkg/ddl/ingest/testutil/testutil.rs`

源码：[`testutil.rs`](./testutil.rs)

## 文件定位

本文件是 `astersql-ddl-ingest-testutil` crate 的核心实现，crate 入口 `pkg/ddl/ingest/testutil/lib.rs` 通过 `pub mod testutil` 加载它，并用 `pub use testutil::*` 重新导出全部公开接口。它不参与 DDL 作业调度或索引回填本身，而是为 ingest 测试提供两个进程级辅助能力：临时注入 mock backend 环境，以及在测试进程退出前检查 ingest 资源是否泄漏。

`pkg/ddl/ingest/testutil/Cargo.toml` 将该 crate 映射到 Go 包 `pkg/ddl/ingest/testutil`，并声明 ingest、KV、元数据模型、指标与 testkit 等相邻依赖。不过当前 `testutil.rs` 没有直接引用这些 crate；真实系统状态由调用者实现的 `IngestTestRuntime` trait 间接接入。这意味着当前 Rust 文件是可测试的抽象边界，而不是已经连接到真实 ingest 全局变量、failpoint 或进程退出设施的适配器。

## 核心职责

1. 用 `BackendRoots` 表示测试需要替换和恢复的磁盘根路径、内存上限。
2. 用 `IngestTestRuntime` 抽象所有外部副作用，包括 roots 快照与替换、mock backend 开关、初始化标志、泄漏计数、报告以及进程退出。
3. `InjectMockBackendCtx` 按固定顺序建立 mock 环境，并在中途失败时尽力回滚已完成的步骤。
4. `MockBackendGuard` 保存旧 roots，通过显式 `restore` 或 RAII `Drop` 恢复进程级状态。
5. `CheckIngestLeakageForTest` 在原退出码为零时依次检查 tracker、backend context 和已注册作业；发现泄漏时报告并改为退出码 1。

该文件的职责严格限于测试隔离和收尾。它没有创建 DDL job、执行 backfill、管理 checkpoint，也没有直接持有真实 backend context。

## 主要符号

- `IngestTestError(pub String)`：公开的字符串错误包装，实现 `Display` 和 `std::error::Error`，作为所有可失败运行时操作的统一错误类型。其内容由 `IngestTestRuntime` 实现者提供，本文件不增加上下文或错误分类。
- `BackendRoots { disk_path: String, memory_limit: u64 }`：可克隆、可比较的 roots 快照。注入时磁盘路径换成调用者给定的临时目录，内存上限换成 `i64::MAX as u64`；恢复时整体写回旧值。
- `IngestTestRuntime: Send + Sync`：公开运行时边界。状态变更方法返回 `Result`，计数与作业查询直接返回值；`exit(code) -> !` 表示调用后不返回。`Send + Sync` 允许实现被放入 `Arc<dyn IngestTestRuntime>` 并跨线程共享，但 trait 文档同时要求实现者自行串行化全局 roots 和 failpoint 的修改。
- `MockBackendGuard`：持有 `Arc<dyn IngestTestRuntime>` 和 `Option<BackendRoots>`。`old_roots` 同时充当恢复数据和一次性状态标志。
- `MockBackendGuard::restore(&mut self)`：先 `take` 旧 roots，随后依次取消 initialized、恢复 roots、关闭 mock backend；重复调用因 `old_roots` 已为 `None` 而返回 `Ok(())`。
- `Drop for MockBackendGuard`：离开作用域时调用 `restore`，但故意丢弃返回错误，保证析构不会传播失败或引发二次 panic。
- `InjectMockBackendCtx(runtime, store_id, temporary_directory)`：公开注入入口，返回 guard。保留 Go 风格名称，并以 `#[allow(non_snake_case)]` 局部允许。
- `CheckIngestLeakageForTest(runtime, exit_code) -> !`：公开进程收尾入口，同样保留 Go 风格名称。它的控制流最终必定调用 runtime 的 `exit`。

## 执行流程

`InjectMockBackendCtx` 的成功路径如下：

1. 调用 `snapshot_roots`，在任何修改发生前取得完整旧 roots；失败则立即返回。
2. 调用 `enable_mock_backend(store_id)`；失败则返回，此时本函数不会主动清理，契约要求实现者不要在失败返回前留下不可见的半开副作用。
3. 调用 `set_initialized(true)`；失败时尽力调用 `disable_mock_backend`，然后返回原始初始化错误。
4. 构造新的 `BackendRoots`，将 `disk_path` 设为 `temporary_directory.into()`，将 `memory_limit` 设为 `i64::MAX as u64`，再调用 `replace_roots`。
5. roots 替换失败时，依次尽力取消 initialized 并关闭 mock backend，返回原始 roots 错误。
6. 成功后返回保存 runtime 和旧 roots 的 `MockBackendGuard`。

恢复路径由 `MockBackendGuard::restore` 执行：它先取走 `old_roots` 以保证幂等，然后按注入的逆向意图执行 `set_initialized(false)`、`replace_roots(old_roots)`、`disable_mock_backend()`。三个调用都会执行；`Result::and` 最终返回按该组合顺序出现的第一个错误。作用域退出会由 `Drop` 自动触发同一路径。

`CheckIngestLeakageForTest` 的流程是：仅当 `exit_code == 0` 时执行泄漏检查；先看 `tracker_count`，再看 `backend_count`，二者同时非零时 tracker 具有报告优先级。发现任一对象泄漏后报告 `add index leakage check failed: ... leak` 并立即 `exit(1)`。前两项均为零才读取 `registered_jobs`；非空时以空格连接作业标识，报告 `add index metrics leakage: [...]` 并 `exit(1)`。没有泄漏或原退出码非零时，直接 `exit(exit_code)`。

## 数据与状态

本文件本身没有静态变量、锁或全局容器。全部可变进程状态位于 `IngestTestRuntime` 的实现背后；guard 只保存一份旧 `BackendRoots` 和共享 runtime 句柄。

`old_roots: Option<BackendRoots>` 是关键不变量：`Some` 表示尚可执行一次恢复，`take()` 后变为 `None`，从而让显式恢复、重复恢复和随后发生的 `Drop` 不会重复修改全局状态。需要注意，旧状态只包含 roots，不包含注入前的 initialized 值或 mock backend 开关值；恢复逻辑无条件将 initialized 设为 `false` 并禁用 mock，因此调用方必须在未注入的干净测试环境中使用它，不能依赖嵌套注入恢复外层状态。

新内存上限采用 `i64::MAX as u64`，与 Go 版本的 `math.MaxInt64` 数值一致，而不是 `u64::MAX`。`registered_jobs` 返回拥有所有权的 `Vec<String>`，泄漏检查只读取并格式化它，不清理指标注册。

## 依赖与调用关系

RustCodeGraph 对 `InjectMockBackendCtx` 给出的直接下游边是 `snapshot_roots`、`enable_mock_backend`、`set_initialized`、`replace_roots`、`disable_mock_backend`；对 `CheckIngestLeakageForTest` 给出的下游边是 `tracker_count`、`backend_count`、`registered_jobs`、`report_leak`、`exit`。`MockBackendGuard::restore` 也调用后三个状态变更方法中的 `set_initialized`、`replace_roots`、`disable_mock_backend`。

当前 Rust 上游仅在同 crate 的独立测试 `pkg/ddl/ingest/testutil/testutil_aster_unit_test.rs` 中可见：测试实现 `MockRuntime`，调用两个公开函数并验证状态与退出码。全仓库直接引用搜索没有发现其他 Rust 文件调用本实现；根 workspace、`pkg/ddl/Cargo.toml`、`pkg/ddl/ingest/Cargo.toml` 和 metadata-lock 测试 manifest 虽声明了该 crate，但依赖声明不等于当前接口已经接线。根 `pkg/lib.rs` 还通过 facade 重新导出此 crate。

Go 上游则已广泛接线：`pkg/ddl/ingest/integration_test.go` 的多项测试、`pkg/ddl/backfilling_dist_scheduler_test.go` 以及 `pkg/ddl/tests/metadatalock/mdl_test.go` 都以 `defer ingesttestutil.InjectMockBackendCtx(t, store)()` 建立和恢复环境。Go 的泄漏检查通常作为测试主进程退出路径的一部分使用；Rust 当前仓库没有对本文件中同名函数的真实 harness 调用。

## 错误处理与边界

注入函数保留导致失败的原始 `IngestTestError`。失败回滚属于 best effort：`set_initialized(true)` 失败时忽略禁用 mock 的错误；roots 替换失败时忽略取消初始化与禁用 mock 的错误。因此调用者能看到首要失败原因，但不能仅凭返回错误确认全局状态已完整恢复。

`restore` 与失败回滚的语义不同：它尝试三个清理步骤，并用 `initialized.and(roots).and(failpoint)` 返回第一个错误。因为调用在组合结果前已经分别求值，即使前一步失败，后续清理仍会执行。可是 `old_roots` 在清理前已被取走，所以一次恢复部分失败后不能靠再次调用 guard 重试；需要 runtime 实现或测试 harness 另行兜底。

`Drop` 丢弃恢复错误，适合析构安全，但可能隐藏测试污染。对清理结果有要求的测试应在 guard 离开作用域前显式调用 `restore()` 并处理错误。`CheckIngestLeakageForTest` 的 `exit -> !` 使泄漏分支立即终止，后续项目不会再检查；这保留 tracker 优先于 backend、对象泄漏优先于 job 指标泄漏的顺序。

边界输入没有在本层验证：空 `store_id`、空临时路径、重复作业名和任意错误字符串都会转交 runtime 或原样格式化。线程安全也不由函数内部锁保证，而由 `IngestTestRuntime` 实现承担。

## 并发与资源生命周期

`Arc<dyn IngestTestRuntime>` 让 guard 与调用者共享同一 runtime；trait 的 `Send + Sync` 只保证类型可以跨线程使用，不自动保证一组多步骤状态变更是原子的。`InjectMockBackendCtx` 的“快照—启用—初始化—换 roots”和 `restore` 的三步清理之间都没有本文件级锁，因此实现者必须用互斥锁或等价机制覆盖完整全局状态协议，并避免并行测试互相覆盖 roots。

资源生命周期以 guard 为界：创建成功后，guard 持有恢复责任；显式 `restore` 会消费旧 roots，之后 drop 是空操作；未显式恢复时 drop 自动清理。若 guard 被长期持有，mock backend 与临时 roots 也会长期有效。若进程通过 `CheckIngestLeakageForTest` 直接退出，则普通 Rust 析构是否执行取决于 runtime 的 `exit` 实现，调用者不能把进程退出路径当作 guard 清理替代品。

独立测试中的 `MockRuntime` 用 `Mutex<RuntimeState>` 展示了符合 trait 串行化要求的最小实现；其 `exit` 用 panic 模拟不返回行为，以便 `catch_unwind` 断言退出码，而不终止测试进程。

## 与 Go 版本的对应关系

Go 文件 `pkg/ddl/ingest/testutil/testutil.go` 是直接语义来源。两版都保存旧磁盘/内存 roots、开启 mock backend、标记 ingest 已初始化、把磁盘根切到临时目录、把内存上限设为最大有符号 64 位整数，并提供退出前泄漏检查。两版泄漏检查都只在原退出码为零时运行，检查顺序均为 tracker、backend、registered jobs，泄漏时退出 1，否则保留原退出码。

实现形态存在重要差异：

- Go 版接收 `testing.T` 与 `kv.Storage`，直接创建 `testkit.TestKit`，通过 failpoint 回调把真实 `BackendCtx` 替换成 `ingest.NewMockBackendCtx`；Rust 版只把 `store_id` 传给抽象 runtime，并不直接构造 session、checkpoint operator 或 backend context。
- Go 版直接读写 `ingest.LitInitialized`、`LitDiskRoot`、`LitMemRoot` 以及真实指标；Rust 版全部通过 trait 访问，仓库中目前只有测试替身实现。
- Go 恢复闭包只恢复 initialized 与 roots，没有显式禁用 failpoint，因为 `EnableCall(t, ...)` 的生命周期由 Go 测试框架管理；Rust guard 明确调用 `disable_mock_backend`。
- Rust 版增加了中途失败的 `Result`、best-effort 回滚、RAII guard 和幂等显式恢复；Go helper 的操作没有返回错误路径。

因此 Rust 版本目前保留了核心状态转换和泄漏判定语义，但尚不能视为 Go 测试工具的完整运行时替代：真实 ingest/testkit/failpoint 适配和测试 harness 接线仍未在本文件或其相邻实现中出现。

## 扩展指南

若要接入真实 Rust ingest 测试，优先新增独立适配器类型实现 `IngestTestRuntime`，把每个 trait 方法映射到真实 roots、backend/failpoint、指标和退出设施；不要把平台状态访问重新硬编码进 `InjectMockBackendCtx`。适配器必须对整个注入/恢复协议提供串行化，并明确是否允许嵌套或并发 guard。新增实现应放在合适的独立源文件，测试继续放在独立 `*_test.rs` 文件，不能嵌回本源文件。

若增加或调整状态字段，需要同步修改 `BackendRoots`、`snapshot_roots`、`replace_roots`、guard 保存内容以及独立测试中的 `RuntimeState`。如果新状态也需要恢复，应保存注入前值，而不是像 initialized/mock 开关那样假定初始为关闭；否则嵌套和已有环境会被破坏。

若改变错误策略，应重点决定三件事：失败回滚是否聚合清理错误、部分恢复后是否允许重试、`Drop` 错误如何可观测。相关回归测试应加入 `pkg/ddl/ingest/testutil/testutil_aster_unit_test.rs`，至少覆盖 `enable_mock_backend`、`set_initialized`、`replace_roots` 和恢复各阶段失败，以及只依赖 `Drop` 的清理路径。

若改变泄漏检查顺序、消息格式或退出码，必须同步核对 Go 文件和测试 harness 对字符串/退出行为的依赖，并扩展现有泄漏测试。性能风险很低，主要风险是进程级共享状态导致的测试串扰、清理失败被隐藏，以及真实 `exit` 提前终止其他清理。

## 验证依据

- 源码事实：`pkg/ddl/ingest/testutil/testutil.rs`，包含 `IngestTestError`、`BackendRoots`、`IngestTestRuntime`、`MockBackendGuard`、`InjectMockBackendCtx`、`CheckIngestLeakageForTest` 的完整定义。
- crate 边界：`pkg/ddl/ingest/testutil/lib.rs` 的模块加载、公开重导出与独立测试声明；`pkg/ddl/ingest/testutil/Cargo.toml` 的 crate 名、Go 包映射和依赖声明。
- Rust 测试：`pkg/ddl/ingest/testutil/testutil_aster_unit_test.rs` 验证成功注入、显式恢复、重复恢复、泄漏优先级、报告文本、零/非零退出码；未覆盖注入失败回滚、恢复失败和仅靠 `Drop` 清理。
- Go 对照：`pkg/ddl/ingest/testutil/testutil.go`；Go 调用证据见 `pkg/ddl/ingest/integration_test.go`、`pkg/ddl/backfilling_dist_scheduler_test.go`、`pkg/ddl/tests/metadatalock/mdl_test.go`。
- RustCodeGraph：索引状态显示目标文件已收录；`files --filter pkg/ddl/ingest/testutil` 列出 Rust/Go 实现与独立测试；`query` 定位两版同名函数；`callees InjectMockBackendCtx` 和 `callees CheckIngestLeakageForTest` 核对了上述 runtime 调用边。图查询未找到 Rust 上游调用者，随后以全仓库直接引用搜索确认除同 crate 单测外没有本实现调用点。
- 人工边界复核：本文件没有条件编译项、静态状态或内部测试模块；所有生产符号均为公开接口或公开 trait 方法，内部状态仅为 `MockBackendGuard` 的两个私有字段。
