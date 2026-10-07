# `pkg/planner/core/casetest/instanceplancache/support.rs`

## 文件定位

本文件是 `astersql-planner-core-casetest-instanceplancache` crate 的共享测试支撑层。crate 入口 `pkg/planner/core/casetest/instanceplancache/lib.rs` 仅在 `cfg(test)` 下通过 `#[path = "support.rs"] mod support` 挂载它，因此它不进入正常的 planner 运行时，也不实现实例级计划缓存本身。它服务于同目录的 `builtin_func_test.rs`、`concurrency_test.rs`、`concurrency_tpcc_test.rs`、`dml_test.rs`、`main_test.rs` 和 `others_test.rs`，把测试环境、SQL 调用及随机数据生成的重复接线集中起来。

crate 边界由 `pkg/planner/core/casetest/instanceplancache/Cargo.toml` 确定：该 crate 的库入口为 `lib.rs`，`astersql-testkit` 是 dev-dependency；`astersql-domain`、`astersql-parser-auth`、`astersql-planner-core` 和 `astersql-session-sessmgr` 也是测试依赖。本文件直接使用的外部接口来自 `astersql-testkit`。

## 核心职责

1. `Harness` 为每个顶层用例创建新的 `AnalyzeStatsStore`、`Domain` 和主 `TestKit`，并持有一个进程级互斥锁，使会修改实例计划缓存全局变量的 Rust 用例不因测试运行器并行执行而互相污染。
2. `Harness::session` 从同一个 `Arc<AnalyzeStatsStore>` 创建独立 `TestKit`，用于表达“共享实例存储、隔离会话状态”的多连接场景。
3. `rand::intn` 用一个进程级原子状态提供线程安全、可复现算法的有界伪随机整数，替代 Go 用例中的 `math/rand.Intn`。
4. `exec`、`query`、`exec_many` 和 `rows` 将 Rust `TestKit` 接口收窄为本测试族常用的 Go 形状，减少每个移植用例重复传递空参数或转换结果行。

这些职责只负责测试编排。计划缓存命中、失效、权限、DDL、事务及执行计划行为仍由被测的 planner/session/executor 代码完成。

## 主要符号

- `pub mod rand`：局部随机数命名空间，供调用方以 `rand::intn(...)` 使用，贴近 Go 源码写法。
- `rand::STATE: AtomicU64`：以固定常量 `0x9e3779b97f4a7c15` 为初始状态的全局线性同余生成器状态；不对 crate 外公开。
- `rand::intn(upper: i32) -> i32`：先断言 `upper > 0`，再通过 `compare_exchange_weak` 循环提交 `old * 6364136223846793005 + 1`（溢出回绕），最终返回 `next % upper`。
- `pub struct Harness`：包含公开的 `store: Arc<AnalyzeStatsStore>`、公开的主会话 `tk: TestKit`，以及私有 `_test_guard: MutexGuard<'static, ()>`。私有 guard 的存在把互斥锁生命周期绑定到整个测试环境。
- `TEST_LOCK: OnceLock<Mutex<()>>`：惰性创建且全进程共享的用例级互斥锁。
- `Harness::new() -> Self`：取得全局锁，调用 `CreateMockStoreAndDomain()`，再在返回的 store 上创建主 `TestKit`。返回的 `Domain` 在此函数中不另存字段。
- `Harness::session(&self) -> TestKit`：克隆共享 store 的 `Arc`，构造一个连接 ID、会话状态均独立的新 `TestKit`。当前同目录源码没有直接调用该方法；并发测试多在闭包中直接执行同形的 `TestKit::new(store.clone())`。
- `exec(tk: &mut TestKit, sql: &str)`：调用 `TestKit::MustExec(sql, Vec::new())`，适合无绑定参数且不读取结果的 SQL。
- `query(tk: &TestKit, sql: &str) -> astersql_testkit::result::Result`：调用 `TestKit::MustQuery(sql, Vec::new())`，把结果留给调用者执行 `Sort`、`Check`、`Rows` 或 `Equal`。
- `exec_many(tk: &mut TestKit, statements: &[&str])`：按切片顺序逐条调用 `exec`；当前同目录源码没有直接调用者。
- `rows(values: &[String]) -> Vec<Vec<String>>`：把一维字符串切片克隆成单列二维行集，适配 `Result::Check`。它不负责排序、类型转换或多列拆分。

文件没有 trait、enum、type alias 或条件编译分支；条件编译位于上游 `lib.rs`。

## 执行流程

典型用例先调用 `Harness::new`。该函数通过 `TEST_LOCK.get_or_init` 获得唯一 mutex，并一直把 guard 保存到 `Harness`；随后 `CreateMockStoreAndDomain` 创建统计信息模拟存储及其 Domain，`TestKit::new(store.clone())` 建立主会话。用例用主会话建库、建表、装载数据和设置全局变量，再把 `harness.store` 克隆给多个工作线程，各线程创建自己的 `TestKit`，从同一存储观察实例级缓存。

SQL 路径保持刻意简单：不需要结果的语句进入 `exec -> TestKit::MustExec -> TestKit::Exec -> Database::execute`；查询进入 `query -> TestKit::MustQuery -> TestKit::Query -> Database::query`，成功后包装为可比较的 `result::Result`。批量初始化若采用 `exec_many`，则严格按输入顺序串行执行，首条失败即终止，后续语句不会运行。

随机路径中，每次 `rand::intn` 都加载共享状态、计算下一状态并尝试 CAS；CAS 失败表示另一线程已经推进状态，当前线程用返回的新值重算，直到成功。因此全局状态更新不会丢失。固定种子和固定递推公式使成功提交形成确定的数值序列，但并发调度会影响某个线程拿到序列中的哪一项，不能把它理解为每个线程各自拥有稳定随机流。

## 数据与状态

`Harness::store` 是 `Arc<AnalyzeStatsStore>`，负责让主会话与工作会话共享表、统计信息和实例范围状态；每次 `TestKit::new` 则创建独立的测试会话。`Harness::tk` 是初始化和单会话断言的默认入口。`CreateMockStoreAndDomain` 同时创建并注册 Domain 的自动分析执行器，但本文件不直接操作 Domain。

进程级可变状态有两处：`TEST_LOCK` 管理顶层测试环境的串行化，`rand::STATE` 管理所有调用者共享的伪随机序列。两者都没有显式重置 API，所以随机序列会跨同一测试进程中的用例继续推进；文档或测试不能依赖某个用例总是取得初始种子后的第一个值。

`rows` 创建新的二维 `Vec` 并克隆每个字符串，调用后与原输入没有借用关系。`exec`/`query` 总是传空的 `Vec<DbValue>`；参数化 SQL 在这些用例中通过 SQL `PREPARE`、用户变量与 `EXECUTE` 表达，而不是通过 TestKit 参数数组表达。

## 依赖与调用关系

上游装配边为 `lib.rs -> support`（仅测试构建）。直接导入边可由各测试文件的 `use super::support::{...}` 核对：六个测试模块使用 `Harness`、`exec`、`query`、`rand` 的不同组合，`builtin_func_test.rs`、`concurrency_test.rs` 和 `main_test.rs` 还使用 `rows`。`exec_many` 与 `Harness::session` 暂无同目录调用边，属于预留的共享辅助接口。

下游关键边为：

- `Harness::new -> TEST_LOCK -> CreateMockStoreAndDomain -> TestKit::new`；
- `Harness::session -> Arc::clone(store) -> TestKit::new`；
- `exec -> TestKit::MustExec`，而 `MustExec` 在 `pkg/testkit/testkit.rs` 中把执行错误转成带 SQL/注释上下文的 panic；
- `query -> TestKit::MustQuery`，而 `MustQuery` 查询后将字符串行包装为 `pkg/testkit/result.rs` 的 `Result`；
- `exec_many -> exec`；
- `rows -> Vec<Vec<String>>`，供 `Result::Check` 消费。

本文件不直接调用 planner cache API；缓存路径由这些 SQL 经 TestKit 的数据库/会话执行接口进入完整系统，因此它在应用主链中的位置是“测试入口之前的适配层”。

## 错误处理与边界

`rand::intn` 对零或负上界执行 `assert!` 并 panic，与 Go `rand.Intn` 的非法上界失败语义对齐；调用者必须先保证候选集合非空以及计算出的范围为正。`upper` 转为 `u64` 发生在正值断言之后，不会把负数误当作巨大上界。取模保证返回值位于 `[0, upper)`。

`Harness::new` 获取 mutex 时若发现锁已 poisoned，不继续传播 poisoning，而用 `poisoned.into_inner()` 取得 guard。这样前一个 panic 不会让后续用例仅因锁污染而立即失败，但共享的进程级配置是否已恢复仍由后续新建环境和用例设置负责。`CreateMockStoreAndDomain` 和 `TestKit::new` 在这里没有 `Result` 返回值，因而本层没有可传播的初始化错误。

`exec` 和 `query` 使用 Must 接口：SQL 执行或查询失败会 panic，而不是返回 `Result` 给调用者处理。这适用于“该 SQL 必须成功”的测试路径；要断言预期错误，应直接使用 TestKit 的 `ExecToErr`/`QueryToErr` 等接口，不能扩展 `exec` 后吞掉错误。`exec_many` 不提供事务性回滚，前面已成功的语句在后续语句 panic 时仍可能留下状态。

## 并发与资源生命周期

`TEST_LOCK` 由 `OnceLock` 初始化一次，mutex 存活到进程退出。每个 `Harness` 从 `new` 开始持锁，直到结构体被 drop 时 `_test_guard` 被释放；这覆盖主会话、共享 store 以及由该用例管理的工作线程生命周期。调用方应确保所有借用 `harness.store` 的 scoped 线程在 `Harness` drop 前结束，现有测试通过 `thread::scope` 或显式等待实现这一点。

串行锁只隔离“分别创建 `Harness` 的顶层 Rust 用例”；它不会阻止同一 Harness 内有意创建的并发会话。绕过 `Harness::new` 直接创建环境也不会获得该保护。新增会修改进程级 instance-plan-cache 配置的测试应复用 Harness，不能只复制 store 初始化代码。

`rand::STATE` 使用 `Ordering::Relaxed`。这里需要的是原子读改写不丢失，而不是借助随机状态同步其他内存，因此没有额外 happens-before 保证。`compare_exchange_weak` 允许伪失败，循环负责重试。共享 store 的并发安全由 `AnalyzeStatsStore`/TestKit 下层承担，本文件没有额外锁住 SQL 执行。

## 与 Go 版本的对应关系

Go 同路径不存在单独的 `support.go`；本文件提取的是散布在多个 `*_test.go` 中的重复模式。`builtin_func_test.go` 展示 `testkit.CreateMockStore(t)`、主 `testkit.NewTestKit(t, store)`、goroutine 内再次 `NewTestKit`、`MustExec`、`MustQuery(...).Check(testkit.Rows(...))` 和 `rand.Intn` 的原始组合；`concurrency_test.go` 同样创建十个共享 store 的 TestKit。Rust 的 `Harness`、`exec/query`、`rows` 和 `rand::intn` 分别承担这些角色。

关键差异是 Rust 测试运行器可并行调度独立 `#[test]`，而 Go 文件没有调用 `t.Parallel()`，包内这些顶层测试默认不并行。因此 Rust 增加 `TEST_LOCK` 来保持 Go 的用例级串行隔离。Rust 随机数实现还用固定种子和原子 CAS 取代 Go 包级 `math/rand`，以便并发调用不会产生 Rust 数据竞争且算法可复核；它只要求范围和工作负载形状对齐，不承诺生成与 Go 完全相同的数列。

`CreateMockStoreAndDomain` 比 Go 用例直接调用的 `CreateMockStore(t)` 显式返回 Domain，但 `Harness::new` 只保留 store 和 TestKit。SQL 的 Must 失败语义与 Go TestKit 的“测试立即失败”意图一致，在 Rust 中具体表现为 panic。

## 扩展指南

新增实例计划缓存测试时，优先调用 `Harness::new` 获取隔离环境；需要跨会话验证时克隆 `harness.store` 后创建独立 `TestKit`，或在语义适合时使用 `Harness::session`。不要让多个线程共享可变的同一个 `TestKit`。需要修改全局缓存变量的用例尤其不能绕过 Harness 的 guard。

新增辅助函数应保持本层只做测试接线：普通成功 SQL 可复用 `exec/query`，单列字符串期望值可复用 `rows`；多列、参数数组、预期错误、事务回滚或异步行为应使用 TestKit 的真实接口，不应把这些语义硬塞进现有薄封装。若启用 `exec_many`，需接受它是顺序、非事务、失败即 panic 的事实，或者另建名字明确的新接口。

修改随机算法时必须同步核对 `main_test.rs`、`concurrency_test.rs`、`concurrency_tpcc_test.rs`、`dml_test.rs` 和 `builtin_func_test.rs` 的所有上界均为正，并保持 Go 测试的选择范围；不要依赖并发线程获得固定子序列。修改 `Harness` 的 store/session 生命周期或串行策略时，应同步检查上述六个 Rust 测试文件及对应六个 Go `*_test.go`，特别关注全局变量污染、跨会话缓存共享和线程结束后 guard 才释放的不变量。

该仓库要求 Rust 测试逻辑放在独立测试文件中；因此针对 support 行为的新回归检查应放入同目录独立的 `*_test.rs` 并从 `lib.rs` 在 `cfg(test)` 下挂载，不能内嵌到 `support.rs`。

## 验证依据

- 目标源码：`pkg/planner/core/casetest/instanceplancache/support.rs`，核对了 `rand::STATE/intn`、`Harness`、`TEST_LOCK`、`Harness::{new,session}`、`exec`、`query`、`exec_many` 与 `rows` 的完整实现。
- crate 与模块边界：`pkg/planner/core/casetest/instanceplancache/Cargo.toml` 和 `lib.rs`，确认 dev-dependency、`cfg(test)` 挂载方式及六个测试模块。
- Rust 直接调用证据：同目录六个 `*_test.rs` 的 `use super::support` 与调用点；精确搜索显示 `exec_many` 和 `.session()` 当前无调用点。
- 下游实现：`pkg/testkit/testkit.rs` 的 `TestKit`、`Exec/Query/MustExec/MustQuery`，以及 `pkg/testkit/mockstore.rs` 的 `CreateMockStoreAndDomain`。
- Go 对照：同目录 `builtin_func_test.go`、`concurrency_test.go`、`concurrency_tpcc_test.go`、`dml_test.go`、`main_test.go`、`others_test.go`；这些文件提供共享 store、多 TestKit、Must SQL、Rows 与 `rand.Intn` 的直接语义证据，并且未发现 `t.Parallel()`。
- RustCodeGraph：`status` 确认索引包含目标仓库；`query Harness/intn/exec/query/exec_many/rows` 定位到本文件符号。`explore`、文件 `node` 及调用边命令未返回可用内容，因此调用关系由精确源码引用搜索补证，没有据此推断未验证的运行时边。
- 本任务是纯文档分析，未运行 Cargo 或代码测试；验收使用任务指定的十一章节结构检查，并人工复核文档没有把测试支撑层描述为生产缓存实现。
