# `br/pkg/utiltest/suite.rs`

## 文件定位

`suite.rs` 属于 `astersql-br-pkg-utiltest` 测试工具 crate，是 Go `br/pkg/utiltest/suite.go` 的 Rust 对照实现。它不执行备份恢复业务，而是为 restore-schema 一类测试一次性装配 mock TiDB 集群、mock glue 与临时目录上的本地对象存储，并负责回收 mock 集群。crate 入口 `br/pkg/utiltest/lib.rs` 以 `pub mod suite` 纳入本文件，并在 crate 根重新导出 `CreateRestoreSchemaSuite`、`RestoreSchemaSuite` 和 `TestRestoreSchemaSuite`。

`br/pkg/utiltest/Cargo.toml` 将该 crate 标记为 `kind = "library"`、Go 包映射为 `br/pkg/utiltest`，直接依赖精简后的 `astersql-br-pkg-gluetidb-mock`、`astersql-br-pkg-mock` 和 `tempfile`。因此这里是 BR 测试夹具层，而不是服务器进程或恢复执行主链的生产入口。

## 核心职责

本文件有三项紧密相关的职责：

1. `CreateRestoreSchemaSuite` 按固定顺序创建 `MockGlue`、未启动的 `Cluster`、`TempDir` 和 `Storage`，再启动集群并返回完整夹具。
2. `TestRestoreSchemaSuite` 把这些资源聚合到同一个所有权对象中；私有 `_temp_dir` 使 `Storage` 使用的目录至少与套件同寿命。
3. `Stop` 与 `Drop` 将 Go `t.Cleanup(func() { s.Mock.Stop() })` 映射为 Rust 的显式清理加作用域自动清理，并用原子状态避免重复停止。

它有意只清理 `Mock`，不单独关闭 `Storage`。这一点由 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches` 验证：调用 `suite.Stop()` 后仍可经 `suite.Storage` 写入并读回文件。

## 主要符号

- `pub struct TestRestoreSchemaSuite`：套件聚合体。公开字段 `Mock: Cluster`、`MockGlue: MockGlue`、`Storage: Arc<dyn Storage>` 供测试直接使用；私有 `_temp_dir: TempDir` 仅承担目录生命周期所有权；`pub(crate) stopped: AtomicBool` 供 crate 内测试观察清理状态。
- `pub type RestoreSchemaSuite = TestRestoreSchemaSuite`：面向机械移植调用方的兼容别名，不产生新的运行时类型或状态。
- `pub fn CreateRestoreSchemaSuite() -> TestRestoreSchemaSuite`：唯一工厂入口。与 Go 版本不同，它不接收 `testing.T`，而以 panic 表达构造失败、以 `Drop` 表达测试清理。
- `pub fn TestRestoreSchemaSuite::Stop(&mut self)`：显式停止接口。通过 `AtomicBool::compare_exchange(false, true, SeqCst, SeqCst)` 保证只有首次调用进入 `self.Mock.Stop()`。
- `impl Drop for TestRestoreSchemaSuite::drop`：自动调用 `self.Stop()`；RustCodeGraph 给出的直接调用边为 `drop -> Stop`。
- `pub use stubs::Context`：从本模块再次导出测试上下文类型。crate 根也从 `stubs` 导出同一类型；本文件内没有创建或消费 `Context`。

本文件没有模块级常量、trait、条件编译项或异步函数。

## 执行流程

`CreateRestoreSchemaSuite` 的正常流程如下：

1. `MockGlue::default()` 创建无注入 session、`GlobalVars` 为空的 glue；该零值契约由 parity 测试断言。
2. `NewCluster()` 创建尚未启动的 mock 集群。其实现位于 `br/pkg/mock/mock_cluster.rs`，会装配 mock storage、Domain 和 PD 客户端；此时尚未建立 Server/DSN。
3. `TempDir::new()` 创建独占临时目录，并立即用 `NewLocalStorage(_temp_dir.path())` 构造 `Arc<dyn Storage>`。`br/pkg/utiltest/stubs.rs` 中的工厂最终包装 `LocalStorage`。
4. `Mock.Start()` 启动 mock server；`br/pkg/mock/mock_cluster.rs::Cluster::Start` 要求 `Storage` 已存在，创建驱动和 Server，等待就绪后填写 DSN。
5. 返回套件，`stopped` 初始为 `false`。字段声明顺序使 `_temp_dir` 保存在对象内，而不是在工厂返回时销毁。
6. 调用方可显式 `Stop`；若没有显式调用，对象离开作用域时 `Drop::drop` 调用 `Stop`。若两条路径都发生，原子比较交换让后一次成为 no-op。

当前 RustCodeGraph 索引记录的工厂直接调用者是 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches`。大量 Go 恢复、checkpoint 和 task 测试调用的是同名 Go 工厂，不能据此推断它们已经接线到 Rust 工厂。

## 数据与状态

套件的关键状态不变量是：

- 成功返回时，`Mock` 已完成 `NewCluster` 和 `Start` 两阶段初始化；parity 测试检查 `Storage`、`Domain`、`Server` 均为 `Some` 且 `DSN` 非空。
- `MockGlue` 是新建默认值，当前测试以 `GlobalVars.is_empty()` 验证其零值状态。
- `Storage` 使用 `Arc<dyn Storage>`，允许调用方共享动态分派的存储句柄；具体工厂当前返回本地文件系统实现。
- `_temp_dir` 不公开也不参与业务调用，其存在本身保证根目录不会在 `Storage` 之前被 `TempDir` 删除。调用者不能替换或提前释放它。
- `stopped` 只从 `false` 单向变为 `true`，没有重启或复位路径。它防止套件层重复进入 `Cluster::Stop`，但不把 `Cluster` 的各公开字段清空。

类型别名 `RestoreSchemaSuite` 与原类型共享完全相同的数据布局和生命周期；`Context` 再导出不引入额外状态。

## 依赖与调用关系

上游装配关系为 `br/pkg/utiltest/lib.rs -> suite.rs`，crate 根再把三个套件符号暴露给依赖者。RustCodeGraph 对目标文件报告的使用文件包括 crate 的 parity 测试，以及因 `Context` 路径关联到的 syncpoint 实现/测试；其中真正调用工厂的已索引 Rust 入口是 `go_rust_public_contract_matches`。

主要下游关系为：

- `CreateRestoreSchemaSuite -> MockGlue::default`，类型来自 `astersql-br-pkg-gluetidb-mock`。
- `CreateRestoreSchemaSuite -> NewCluster -> Cluster::Start`，来自 `astersql-br-pkg-mock`；后者负责 mock server、Domain、DSN 和相关句柄。
- `CreateRestoreSchemaSuite -> TempDir::new`，来自 `tempfile`。
- `CreateRestoreSchemaSuite -> stubs::NewLocalStorage`，返回 `Arc<dyn stubs::Storage>`。
- `Drop::drop -> TestRestoreSchemaSuite::Stop -> Cluster::Stop`。`Cluster::Stop` 在下游依次尝试关闭 Domain、Storage、Server、HttpServer，并停止 view、清除 online 标志。

`suite.rs` 不依赖真实 objstore、kvproto 或 grpcio；Cargo 注释明确说明该 crate 在 arm64 Darwin 使用本地 `stubs` 存储表面和已精简 mock 依赖。

## 错误处理与边界

工厂对 `NewCluster`、`TempDir::new`、`NewLocalStorage`、`Mock.Start` 的错误均使用 `unwrap_or_else` 转为 panic，并在消息中标出失败阶段。这是在没有 `testing.T` 参数的 Rust API 中对齐 Go `require.NoError` 的快速失败语义；函数不会返回部分初始化的套件或 `Result`。

构造顺序也限定了失败边界：只有本地存储建立后才调用 `Start`；若较晚步骤失败，已创建的局部 Rust 值按栈展开规则释放，但不会构造 `TestRestoreSchemaSuite`，因此其 `Drop` 不会执行。特别是 `Mock.Start()` 已发生部分副作用后再返回错误的回滚完整性由下游 `Cluster::Start` 决定，本文件没有补偿逻辑。

`Stop` 不返回错误，沿用 `Cluster::Stop` 的尽力清理接口。原子标志在调用下游停止前就被置为 `true`；当前下游 `Stop` 无返回值，但如果未来改成可能 panic 的实现，套件不会自动重试，这是扩展时必须保留或明确调整的语义。

本文件不校验存储逻辑名、文件不存在、范围读取等细节；这些属于 `stubs.rs` 的 `Storage` 实现，并由 `parity_test.rs` 的其余断言覆盖。

## 并发与资源生命周期

`TestRestoreSchemaSuite` 的修改接口要求 `&mut self`，因此安全 Rust 调用不能同时对同一套件执行多个 `Stop`。`AtomicBool` 仍为显式调用与 `Drop` 提供幂等门闩，且使用最强的 `SeqCst` 顺序；它并不把整个聚合体声明为可跨线程并发操作。

`Storage` 为 `Arc<dyn Storage>`，而 `Storage: Send + Sync`，可以克隆后跨线程共享。相对地，套件整体能否发送或共享还取决于 `Cluster`、`MockGlue`、`TempDir` 等全部字段的自动 trait，实现未在本文件显式承诺。

资源寿命从工厂成功返回持续到套件被丢弃：`Mock` 在显式 `Stop` 或 `Drop` 时停止；`Storage` 在 `Stop` 后仍保持可用；`_temp_dir` 最终随套件销毁并清理目录。若调用者把 `Storage` 的 `Arc` 克隆到套件之外，目录仍会随套件中的 `_temp_dir` 删除，因此克隆句柄并不延长底层路径寿命，这是调用方必须避免的悬空资源语义。

## 与 Go 版本的对应关系

Go `suite.go` 与 Rust 实现的共同顺序是：创建空 `MockGlue`，调用 `mock.NewCluster`，创建测试临时目录和 local storage，启动集群，注册/提供停止清理。公开的 `Mock`、`MockGlue`、`Storage` 三项也逐一对应。

差异来自语言和测试框架：

- Go 工厂接收 `*testing.T` 并返回指针；Rust 工厂无测试上下文参数并按值返回拥有资源的结构体。
- Go 用 `require.NoError(t, err)` 报告构造失败；Rust用带阶段前缀的 panic。
- Go 的 `t.TempDir()` 由测试框架持有；Rust 必须把 `TempDir` 存入 `_temp_dir` 才能维持目录。
- Go 以 `t.Cleanup` 调用 `s.Mock.Stop()`；Rust 同时提供显式 `Stop` 和 `Drop`，再以 `AtomicBool` 避免重复下游清理。
- Go 的 `Mock` 是指针、`Storage` 是 `storeapi.Storage`；Rust 的 `Mock` 按值持有，`Storage` 是本 crate 本地定义的 `Arc<dyn Storage>` 精简表面。
- `RestoreSchemaSuite` 别名及 `Context` 再导出是 Rust 迁移/调用便利层，在 Go 文件中没有对应声明。

`br/pkg/utiltest/parity_test.rs` 是当前最直接的 Rust 对等测试：它验证成功构造的字段状态、本地存储读写和错误分类、裸集群启动失败，以及显式停止、重复停止和停止后存储可用性。

## 扩展指南

新增套件资源时，应在 `TestRestoreSchemaSuite` 中建立明确所有权，在 `CreateRestoreSchemaSuite` 中按依赖顺序初始化，并在 `Stop`/`Drop` 中确定是否需要清理。资源若依赖临时路径、后台线程、端口、通道或锁，必须把维持其有效性的所有者留在结构体中，并检查字段析构顺序和失败中途的回收行为。

改变构造契约时，应同步核对 `br/pkg/utiltest/suite.go`，避免擅自缩减 Go 初始化步骤；若 Rust 因平台精简必须不同，应在 Cargo 依赖和文档中明确边界。改变公开字段、别名或再导出时，还要同步 `br/pkg/utiltest/lib.rs`。

测试逻辑必须继续放在独立的 `br/pkg/utiltest/parity_test.rs`，不要内嵌到 `suite.rs`。至少应覆盖正常构造、每个新增可触发的失败阶段、显式与自动清理的组合、重复清理，以及资源在停止前后的可用性。若改变 `Cluster::Start/Stop` 或 storage 生命周期，还需同步检查 `br/pkg/mock/mock_cluster.rs` 和 `br/pkg/utiltest/stubs.rs` 的独立测试。兼容风险集中在 Go/Rust 清理时机与公开 API 形态，性能风险主要是每次工厂调用都启动一套 mock cluster 并创建磁盘临时目录；不要在未量化前把该夹具扩展为更重的真实服务。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utiltest` 找到目标、Go 对照、crate 入口和 parity 测试。
- RustCodeGraph `node --file br/pkg/utiltest/suite.rs`：读取目标文件 1–106 行，并报告文件关联；`node br/pkg/utiltest/suite.rs::CreateRestoreSchemaSuite` 给出直接调用者 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches`。
- RustCodeGraph `callees br/pkg/utiltest/suite.rs::CreateRestoreSchemaSuite` 的符号集合确认本文件中的 `drop` 调用 `Stop`；目标源码本身核对了工厂对 `NewCluster`、`NewLocalStorage` 和 `Mock.Start` 的顺序调用。工具未解析出这些跨 crate 调用的完整 callee 边，因此依赖关系同时以源码与 Cargo 为证，不把缺失图边表述为不存在调用。
- 已读源码与配置：`br/pkg/utiltest/suite.rs`、`br/pkg/utiltest/lib.rs`、`br/pkg/utiltest/Cargo.toml`、`br/pkg/utiltest/stubs.rs`、`br/pkg/mock/mock_cluster.rs`、`br/pkg/gluetidb/mock/mock.rs`。
- Go 对照：`br/pkg/utiltest/suite.go`。
- 独立 Rust 测试：`br/pkg/utiltest/parity_test.rs`；通过 `rg` 另行确认 syncpoint 文件只消费再导出的 `Context`，未调用套件工厂。Go 侧同名工厂还被多个 restore/checkpoint/task 测试使用，但这些仅作为 Go 契约使用面证据。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；交付验证仅执行任务指定的 11 章节结构检查，并人工复核符号、调用关系、边界和扩展建议均有上述路径支撑。
