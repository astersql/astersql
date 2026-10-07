# `br/pkg/utiltest/syncpoint/lib.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-utiltest-syncpoint` 的 crate root；`br/pkg/utiltest/syncpoint/Cargo.toml` 通过 `[lib] path = "lib.rs"` 明确了这一边界，根 `Cargo.toml` 又将 `br/pkg/utiltest/syncpoint` 列为 workspace member。它对应 Go 包 `br/pkg/utiltest/syncpoint`，但自身不实现排序算法，而是用 `#[path]` 装配 `stubs.rs` 与 `syncpoint.rs`，再把面向调用者的项目从 crate 根导出。

该 crate 属于 BR 测试基础设施：它让测试把多个 failpoint 命中排成显式顺序，不是备份、恢复或 SQL 请求的生产执行路径。仓库引用搜索未发现其他 Rust `Cargo.toml` 依赖此包，也未发现 crate 外通过 Rust 包名导入它；当前可确认的 Rust 使用面是本 crate 的 `parity_test.rs` 和 `syncpoint_test.rs`。这与 Go 同路径包被大量 BR 文件使用的成熟接线范围不同。

## 核心职责

- `pub mod stubs` 将最小化的可取消 `Context`、`CancelHandle`、`StopWatch` 和 `after_func` 纳入 crate；这些类型只覆盖同步脚本所需的 Go `context` 语义。
- `pub mod syncpoint` 纳入顺序编排实现，并通过 `pub use syncpoint::*` 暴露 `StepFn`、`StepDecl`、`Script`、`Step`、`New` 等公开项。
- `pub use stubs::{CancelHandle, Context, StopWatch, after_func}` 提供扁平的 crate-root API，测试无需知道两个内部文件的划分。
- 测试配置下声明全局 `TEST_LOCK` 并挂载两份独立测试模块，使依赖 failpoint 全局注册表的用例串行运行。
- crate 级 `allow` 放宽迁移代码中的 Go 风格命名（如 `New`、`BeginSeq`、`EndSeq`）和暂未使用项检查，服务于接口对齐；它不改变运行时行为。

## 主要符号

- `pub mod stubs`：由 `stubs.rs` 提供本地 context/定时取消桩。其核心公开类型为 `Context`、`CancelHandle`、`StopWatch`，核心函数为 `after_func`。
- `pub mod syncpoint`：由 `syncpoint.rs` 提供实际编排器。`Step(name, callback) -> StepDecl` 声明一步；`New() -> Script` 创建脚本；`Script::BeginSeq` 激活有序步骤；`Script::EndSeq` 校验并重置序列。
- `pub use ...`：这是本文件的主要公开契约。`syncpoint::*` 是通配再导出，因此以后在实现模块新增 `pub` 项会自动扩大 crate-root API。
- `TEST_LOCK: std::sync::Mutex<()>`：仅在 `cfg(test)` 下存在，访问级别是 `pub(crate)`。`parity_test.rs` 和 `syncpoint_test.rs` 在每个相关测试开头持锁，防止进程级 failpoint 注册互相污染。
- `mod parity_test`、`mod syncpoint_test`：仅测试构建时编译，且通过 `#[path]` 保持测试逻辑在独立文件中；生产库构建不包含它们。

## 执行流程

1. Cargo 以本文件为库入口，先按显式路径编译 `stubs.rs` 和 `syncpoint.rs`，随后建立 crate-root 再导出。
2. 测试调用 `New()` 获得 `Script`，用 `Step` 构造完整 failpoint 路径及零参数回调，再把 `Context` 和步骤传给 `BeginSeq`。
3. `BeginSeq` 在 `syncpoint.rs` 中校验上下文、非空步骤和单一活跃序列，注册尚未注册的 failpoint，并安装取消监听。
4. failpoint 命中后，注册包装器调用私有 `advance`。若名称不是当前期望步骤，线程在 `Condvar` 上等待；匹配时先推进索引并唤醒等待者，再把用户回调交回锁外执行。
5. 调用者观察完步骤副作用后调用 `EndSeq`；该方法不会等待尚未发生的步骤，而是立即检查取消错误与完成度，成功后清空活跃序列以便复用 `Script`。
6. `Script` 离开作用域时，`registered` 中持有的 `FailGuard` 被释放，已注册 failpoint 随之禁用。测试模块通过 `TEST_LOCK` 把整段注册、命中和释放生命周期串行化。

## 数据与状态

本文件自身唯一持久状态是测试专用的零数据互斥量 `TEST_LOCK`；其锁卫兵的词法作用域界定一次测试对全局 failpoint 表的独占期。业务状态全部位于相邻实现文件：`Script` 持有共享 `State` 与按名称缓存的 `RegisteredStep`，`StateInner` 保存当前步骤数组、下一步骤下标、取消错误和可停止的监听句柄。

`Context` 及其克隆共享一个 `Arc<ContextInner>`；`CancelHandle` 也指向同一对象。取消标志用 `AtomicBool`，首个错误和等待条件分别由 `Mutex`/`Condvar` 管理。`StepFn` 是 `Arc<dyn Fn() + Send + Sync + 'static>`，所以步骤回调可跨 failpoint 线程共享，但 Rust 版本只接受零参数、无返回值回调。

## 依赖与调用关系

直接依赖由 `br/pkg/utiltest/syncpoint/Cargo.toml` 给出：`astersql-testkit-testfailpoint` 提供 RAII `FailGuard`、注册与注入桥接，`fail`（启用 `failpoints` feature）供测试查询全局注册表。标准库提供 `Arc`、`Mutex`、`Condvar`、原子量、线程、时长与哈希表。

本文件到下游的静态关系是 `lib.rs -> stubs.rs` 和 `lib.rs -> syncpoint.rs`；测试配置再增加 `lib.rs -> parity_test.rs`、`lib.rs -> syncpoint_test.rs`。实现中的关键调用边为 `Script::BeginSeq -> prepare_step -> register -> astersql_testkit_testfailpoint::enable_call`，failpoint 回调再进入 `advance`；取消路径为 `BeginSeq -> stubs::after_func -> State.err/Condvar::notify_all`。

RustCodeGraph 的文件视图确认 `syncpoint.rs` 被本目录测试及若干测试文件关联，但精确 `callers/callees` 未为 Rust 的 `BeginSeq`/`EndSeq` 建出可靠方法节点；仓库文本引用与 Cargo manifest 搜索进一步确认，当前可验证的 API 调用只在本 crate 两份测试中。Go 图索引则显示 `syncpoint.go` 被大量 BR 文件使用，不能据此推导 Rust 侧也已完成同等接线。

## 错误处理与边界

crate root 不自行产生错误；它决定哪些实现与测试会被编译、哪些符号对外可见。实际错误契约由再导出的实现承担：空上下文、空步骤、重复活跃序列、空步骤名、序列下溢、取消以及未完成序列均通过 `panic!` 模拟 Go `testing.TB.Fatalf`。`Mutex` 或 `Condvar` 中毒也因 `unwrap`/`expect` 继续 panic。

无活跃序列、序列已完成或已取消时的迟到 failpoint 命中不会执行步骤回调。`EndSeq` 名称虽沿用 Go 注释中的 “waits”，当前 Go 与 Rust 实现都不在这里等待条件变量；调用者必须先通过事件、join 或其他副作用确认步骤完成。Rust 类型系统把回调限定为 `Fn()`，因此没有 Go 版本通过反射支持参数签名、检查重复注册签名一致性的运行时分支。

测试边界也值得注意：`TEST_LOCK` 只协调本 crate 内主动获取它的测试，不能自动串行化进程中所有使用 `fail` 全局注册表的外部测试。完整 failpoint 名必须在 `Step` 注册和注入两端一致，Rust 不会像 Go 代码生成那样替调用者展开短名。

## 并发与资源生命周期

`TEST_LOCK` 是测试级粗粒度互斥；两份测试文件在创建 `Script` 前获取它，并让锁卫兵覆盖全部 worker、`EndSeq` 与资源清理。具体排序由 `State.mu` 和 `State.cond` 完成：错序命中的线程睡眠，正确步骤推进 `next` 后广播唤醒。回调在状态锁外执行，所以后一回调可以在前一回调结束前开始；`syncpoint_test.rs::test_callbacks_can_overlap_after_ordered_release` 专门验证该性质。

取消监听由 `after_func` 启动后台线程。完成最后一步或进入 `EndSeq` 时调用 `StopWatch::stop`，与取消回调通过原子交换竞争，确保回调至多执行一次并避免迟到取消覆盖成功序列。注册得到的 `FailGuard` 保存在 `Script.registered` 的整个生命周期中，同名步骤跨多次序列复用；只有 `Script` drop 才解除注册，`parity_test.rs` 用 `fail::list()` 验证清理结果。

## 与 Go 版本的对应关系

Rust `StepDecl`、`Script`、`New`、`BeginSeq`、`EndSeq`、`prepare_step`、`register`、`advance` 与 `br/pkg/utiltest/syncpoint/syncpoint.go` 中同名或同职责符号逐项对应；顺序等待、取消广播、序列复用、序列外静默忽略及回调锁外执行的语义保持一致。`syncpoint_test.rs` 复刻 Go `syncpoint_test.go` 的乱序 a/b/c 和无活跃序列忽略场景，`parity_test.rs` 额外覆盖取消、非法输入、重用和 Guard 清理。

差异来自语言与当前移植边界：Go `New(t testing.TB)` 借助 `t.Helper`、`Fatalf` 和测试 cleanup，Rust `New()` 不保存测试对象，以 panic 报错并用 `FailGuard` 的 RAII drop 清理。Go 的 `Step.fn` 是 `any`，通过反射接受带入参但无返回值的函数，并检查同名注册的类型一致性；Rust `StepFn` 固定为 `Fn()`，编译期限制替代这部分反射验证。Go 使用真实 `context.Context`/`context.AfterFunc`，Rust 为避开重型依赖使用 `stubs.rs` 的最小实现。Go 包已有广泛调用者，而 Rust crate 目前仍是独立 workspace 成员，尚无可验证的 crate 外依赖接线。

## 扩展指南

- 扩充同步脚本行为时，应修改 `syncpoint.rs` 中对应符号，并同步更新独立的 `syncpoint_test.rs` 和/或 `parity_test.rs`；不要把测试逻辑写回 `lib.rs`。
- 扩充取消、超时或监听语义时，应修改 `stubs.rs`，重点保持首次错误、唤醒顺序、至多一次回调和停止竞争不变量，并新增独立测试覆盖竞态。
- 新增 crate-root API 时优先显式评估可见性。由于当前 `pub use syncpoint::*` 会自动导出实现模块的新公开符号，新增 `pub` 项可能无意扩大兼容面。
- 若要支持带参数回调，不能只放宽 `Step` 签名；需要同时设计 `StepFn`、failpoint 注册包装器、重复名称的签名一致性以及 Go parity 用例，兼容和类型擦除风险较高。
- 若将该 crate 接入其他 Rust 包，应在消费方 `Cargo.toml` 增加 workspace 可复现依赖，并确认 failpoint feature、全局注册隔离和测试序列化策略；目前不能假定 `TEST_LOCK` 能跨 crate 协调。
- 性能通常不是此测试工具的主要约束，但不要在持有 `State.mu` 时执行用户回调或注册外部 failpoint，否则可能引入死锁并破坏已验证的回调重叠行为。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 7032 个 Rust 文件；`files --filter br/pkg/utiltest/syncpoint` 确认七个源/测试文件；`node --file` 阅读了 `lib.rs`、`syncpoint.rs`、`stubs.rs`、`parity_test.rs`、`syncpoint_test.rs`、Go 实现和 Go 测试。文件关系确认 `lib.rs` 是装配入口；精确 Rust 方法调用图未返回可靠节点，本文没有把该缺失解释为无调用。
- crate/构建边界：读取 `br/pkg/utiltest/syncpoint/Cargo.toml`、根 `Cargo.toml`、`br/pkg/utiltest/syncpoint/BUILD.bazel` 和相邻 `br/pkg/utiltest/Cargo.toml`；前者声明独立 Rust library，Bazel 文件目前只描述 Go library/test。
- Rust 实现与测试：`br/pkg/utiltest/syncpoint/lib.rs`、`syncpoint.rs`、`stubs.rs`、`syncpoint_test.rs`、`parity_test.rs`。这些文件支持公开面、排序、取消、回调并发、复用和清理结论。
- Go 对照：`br/pkg/utiltest/syncpoint/syncpoint.go` 与 `syncpoint_test.go`。二者支持符号映射、反射签名差异、Fatal/cleanup 语义及基础测试场景。
- 仓库引用检查：对 Rust 源搜索 crate 名、模块导入与 `BeginSeq`/`Step` 调用，并对所有 Cargo manifest 搜索包名；结果只发现 workspace member 和本 crate 内测试调用，因此将“crate 外 Rust 接线”明确记为当前未发现，而非已支持。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前另以任务给定命令验证目标文档存在且固定二级标题恰好为 11 个。
