# `pkg/expression/expropt/advisory_lock.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate（见 `pkg/expression/expropt/Cargo.toml`），定义咨询锁（advisory lock）在“会话能力”与“表达式求值上下文”之间的可选属性适配层。`pkg/expression/expropt/lib.rs` 通过 `mod advisory_lock` 加载它，并用 `pub use advisory_lock::*` 再导出全部公开符号。

它不实现锁表、超时策略或 SQL 函数语义：会话层实现 `AdvisoryLockContext`，`AdvisoryLockPropProvider` 把该实现注册为 `OptPropAdvisoryLock`，消费者则用 `AdvisoryLockPropReader` 声明并取得这项依赖。

## 核心职责

- 用 `AdvisoryLockContext: Send + Sync` 规定会话侧的四项最小能力：获取指定名称的锁、查询占用者连接 ID、释放单锁和释放本会话的全部锁。
- 用 `AdvisoryLockPropProvider` 持有 `Arc<dyn AdvisoryLockContext>`，为对象安全的可选求值属性提供稳定生命周期，并将四项操作原样转发给真实会话实现。
- 实现 `exprctx::OptionalEvalPropProvider`：`Desc()` 将 Provider 绑定到 `exprctx::OptPropAdvisoryLock`，`as_any()` 允许通用注册表安全地向下转型。
- 用零大小的 `AdvisoryLockPropReader` 将“需要咨询锁属性”纳入表达式依赖集，并从 `OptionalEvalPropContext` 中取回具体 Provider。

## 主要符号

- `pub trait AdvisoryLockContext: Send + Sync`：边界 trait。`get_advisory_lock(&self, name, timeout) -> anyhow::Result<()>` 保留后端错误；`is_used_advisory_lock(&self, name) -> u64` 以 `0` 表示未占用；`release_advisory_lock(&self, name) -> bool` 表示是否成功释放；`release_all_advisory_locks(&self) -> i32` 返回释放数。
- `pub struct AdvisoryLockPropProvider { context: Arc<dyn AdvisoryLockContext> }`：只读适配器，隐藏具体会话类型。
- `AdvisoryLockPropProvider::new<T>(Arc<T>)`：构造入口；`T: AdvisoryLockContext + 'static` 使 `Arc<T>` 可上转为存放在 Provider 中的 trait object。它本身不执行空值或运行时状态检查。
- `impl AdvisoryLockContext for AdvisoryLockPropProvider`：四个方法均是直接委派，不改写名称、超时、返回值或错误。
- `impl exprctx::OptionalEvalPropProvider for AdvisoryLockPropProvider`：完成属性键自描述与 `Any` 下转型支持。
- `pub struct AdvisoryLockPropReader`：无字段 Reader，可按值嵌入表达式签名类型。
- `RequireOptionalEvalProps::required_optional_eval_props()`：返回仅含 `OptPropAdvisoryLock` 的位集合。
- `AdvisoryLockPropReader::advisory_lock_ctx()`：调用共享的 `get_prop_provider(ctx, OptPropAdvisoryLock)`，返回与输入上下文同生命周期的 `&AdvisoryLockPropProvider`。

## 执行流程

1. 会话实现 `AdvisoryLockContext`。Rust 会话适配边界 `pkg/expression/sessionexpr/sessionctx.rs` 进一步要求其 `SessionContext` 继承该 trait。
2. `sessionexpr::NewEvalContext` 克隆会话 `Arc`，调用 `AdvisoryLockPropProvider::new(Arc::clone(&sctx))`，再把 Provider 放入 `OptionalEvalPropProviders` 的 `OptPropAdvisoryLock` 槽位。
3. 潜在消费表达式通过 `AdvisoryLockPropReader::required_optional_eval_props()` 把第 7 号属性位加入需求集（键值见 `pkg/expression/exprctx/optional.rs`）。
4. 求值时，`advisory_lock_ctx()` 向 `OptionalEvalPropContext` 查询该键；共享的 `get_prop_provider` 检查“已注册”、“Provider 自描述键一致”和“具体类型可下转”。
5. 读取者通过返回的 Provider 调用四项 trait 方法；Provider 再经 `Arc<dyn AdvisoryLockContext>` 委派给会话实现。

当前 Rust 生产接线只能直接确认上述 Provider 注册流程。`pkg/expression/builtin_miscellaneous.rs` 的 `get_lock` / `release_lock` / `is_free_lock` / `is_used_lock` / `release_all_locks` 使用其自己的可变 `AdvisoryLockContext`，没有引用本文件的 Reader；因此不能声称 Rust SQL 内置函数已经由本适配层连通。

## 数据与状态

本文件唯一持久状态是 `AdvisoryLockPropProvider.context`：一个强引用的 `Arc<dyn AdvisoryLockContext>`。Provider 不缓存锁名、占用者、重入次数、超时或释放数；这些均由具体 `AdvisoryLockContext` 实现拥有。

`AdvisoryLockPropReader` 是零大小类型，不保存求值状态。`required_optional_eval_props()` 产生单位 bit set；`OptPropAdvisoryLock` 在当前注册表中的数值为 `7`，而描述对象由 `OPTIONAL_PROPERTY_DESC_LIST` 静态持有。

返回值的语义是边界契约的一部分：`is_used_advisory_lock == 0` 表示空闲，非零值是占用连接 ID；单锁释放用 `bool`；全部释放计数用 `i32`。适配层对这些值不做归一化。

## 依赖与调用关系

- crate 内依赖：`use crate::*` 引入 `RequireOptionalEvalProps`、`OptionalEvalPropContext`、`get_prop_provider` 以及 `exprctx` 别名；具体注册表和类型检查在 `pkg/expression/expropt/optional.rs`。
- 外部依赖：标准库 `Arc` 提供共享所有权；`anyhow` 提供获锁及 Provider 查找的错误通道；`exprctx-crate` 提供键、描述、bit set 和 Provider trait。这些均由 `pkg/expression/expropt/Cargo.toml` 声明或间接再导出。
- 生产上游：`pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 构造并注册 Provider；其 `SessionContext` 是真实后端的实现边界。
- 已索引的 Rust Reader 调用者是独立测试：`pkg/expression/expropt/optional_test.rs::verify_advisory_lock`、`pkg/expression/expropt/migration_aster_unit_test.rs::advisory_lock_reader_returns_the_registered_provider_and_forwards_calls` / `registry_and_missing_reader_paths_match_go`，以及 `pkg/expression/sessionexpr/sessionctx_test.rs::test_session_eval_context_opt_props`。
- Go 生产消费者：`pkg/expression/builtin_miscellaneous.go` 中 GET_LOCK、RELEASE_LOCK、IS_FREE_LOCK、IS_USED_LOCK 和 RELEASE_ALL_LOCKS 的签名类型均嵌入 `AdvisoryLockPropReader`；`pkg/expression/sessionexpr/sessionctx.go` 用 `NewAdvisoryLockPropProvider(sctx)` 注册会话实现。

RustCodeGraph 对 `advisory_lock_ctx` 报告的直接边为本方法及两个 `migration_aster_unit_test.rs` 测试调用；图未将 trait-object 动态委派解析为具体会话方法，因此后端实现关系以 trait 约束和直接源码引用为据。

## 错误处理与边界

- `get_advisory_lock` 的 `anyhow::Result<()>` 由真实后端产生，Provider 不包装、吞掉或重试该错误。其他三项操作没有错误通道，只能按契约返回数值或布尔值。
- `advisory_lock_ctx` 的错误来自 `get_prop_provider`：键未注册、Provider 自描述键与请求键不一致、或 `Any` 具体类型不是 `AdvisoryLockPropProvider` 时分别返回可诊断的 `anyhow` 错误。
- 本文件不验证空锁名、64 字符限制、大小写归一化、超时范围、死锁映射或 NULL 语义。Go 版本在 `builtin_miscellaneous.go` 中处理这些 SQL 边界；Rust 当前对应逻辑在与本 Provider 尚未接线的 `builtin_miscellaneous.rs` 中。
- `new` 要求一个已存在的 `Arc<T>`；Rust 类型系统中没有 Go `nil` interface 的直接对应，因此不需要 Go 构造函数的 `intest.AssertNotNil(ctx)` 分支。

## 并发与资源生命周期

`Send + Sync` 是 `AdvisoryLockContext` 的强制契约，使 Provider 持有的 trait object 能在并发求值环境中共享。该契约不会自动使后端状态安全：实现者仍必须用锁、原子类型或其他内部可变机制保护锁集合。测试实现使用 `Mutex<Vec<String>>` 验证了这种预期用法。

Provider 克隆并持有会话 `Arc` 的强引用，因而只要 EvalContext 中的 Provider 存活，底层会话对象就不会被释放。`advisory_lock_ctx` 返回借用而非新的 `Arc`，借用生命周期被绑定到输入上下文，避免 Reader 越过注册表存活。本层不生成线程、任务或通道，也没有 `Drop` 清理；锁的自动释放和会话结束清理属于会话实现责任。

## 与 Go 版本的对应关系

Rust 文件逐项对应 `pkg/expression/expropt/advisory_lock.go`：Go `AdvisoryLockContext` 的四个方法对应 Rust 同名 snake_case 方法；Go 嵌入 interface 的 `AdvisoryLockPropProvider` 对应 Rust 的 `Arc<dyn AdvisoryLockContext>` 字段与显式委派 impl；`Desc`、`RequiredOptionalEvalProps` 和 `AdvisoryLockCtx` 与 Rust 实现保持相同属性键语义。

主要语言差异如下：

- Go Provider 接受 interface 并用 `intest.AssertNotNil`检查；Rust 接受 `Arc<T>`，用所有权和 `'static` 约束建模生命周期。
- Go Provider 依靠匿名嵌入自动提升四个方法；Rust 显式实现 trait 并转发。
- Go `AdvisoryLockCtx` 返回 `AdvisoryLockContext` interface；Rust 返回具体 Provider 引用，调用者再经其 trait impl 访问后端。
- Go `ReleaseAllAdvisoryLocks() int` 在 Rust 边界映射为 `i32`；这与当前测试和接口一致，但扩展时需留意计数范围。
- Go 的五类 SQL 函数已直接消费 Reader；Rust 当前仅完成 sessionexpr 注册和测试验证，Rust 内置函数仍使用独立 trait。这是可观测的迁移差距，不是本文件已实现的行为。

Go 的 `pkg/expression/expropt/optional_test.go::TestOptionalEvalPropProviders` 验证 Provider 注册与获取；Rust 将对应事实放在独立的 `optional_test.rs` 和 `migration_aster_unit_test.rs` 中。

## 扩展指南

- 增加或改变咨询锁后端操作时，首先修改 `AdvisoryLockContext`，同步 Provider 的委派 impl，再更新真实 `SessionContext` 实现和独立测试。若需保持 Go 对齐，还必须核对 `advisory_lock.go` 的 interface 及五类 SQL 函数消费点。
- 不应在 Provider 中新增 SQL 参数规范化、超时钳制或错误码转换；这些是 builtin 层职责。Provider 应保持无损委派，否则同一后端在不同消费者中会出现不一致语义。
- 若把 Rust 内置函数接入本 Reader，需先解决 `builtin_miscellaneous.rs::AdvisoryLockContext` 与本 trait 在 `&mut self`/`&self`、`AdvisoryLockError`/`anyhow::Result`、`u64`/`i32` 计数上的差异，而不是只做类型别名。
- 新增 Reader 路径或改变属性键时，需同步 `exprctx::OPTIONAL_PROPERTY_DESC_LIST`、`OptionalEvalPropProviders` 的注册逻辑与需求集合验证，并注意键值是全 crate 共享的 ABI 式协议。
- 测试逻辑应继续放在独立文件：优先更新 `pkg/expression/expropt/optional_test.rs` 的通用 Provider/Reader 验证、`migration_aster_unit_test.rs` 的精确转发与缺失属性回归，以及 `pkg/expression/sessionexpr/sessionctx_test.rs` 的生产注册链验证。若完成 builtin 接线，还需扩展对应的独立 `builtin_miscellaneous_*_test.rs` 和 SQL 集成测试。

## 验证依据

- 目标源码：`pkg/expression/expropt/advisory_lock.rs`，核对了 trait、Provider、Reader、所有 impl 和方法签名；文件无条件编译项。
- crate 与公共机制：`pkg/expression/expropt/Cargo.toml`、`lib.rs`、`optional.rs`，核对了 crate 边界、再导出、`get_prop_provider` 的三类错误及属性键描述。
- Rust 生产接线：`pkg/expression/sessionexpr/sessionctx.rs`，核对 `SessionContext` trait 约束和 `NewEvalContext` 的 Provider 注册；`pkg/expression/builtin_miscellaneous.rs` 用于确认当前 builtin 仍使用独立 trait。
- Rust 独立测试：`pkg/expression/expropt/optional_test.rs::verify_advisory_lock`、`pkg/expression/expropt/migration_aster_unit_test.rs::advisory_lock_reader_returns_the_registered_provider_and_forwards_calls` 与 `registry_and_missing_reader_paths_match_go`、`pkg/expression/sessionexpr/sessionctx_test.rs::test_session_eval_context_opt_props`。这些覆盖缺失 Provider、注册/下转、同一 Provider 引用、四项委派和会话状态可见性。
- Go 对照：`pkg/expression/expropt/advisory_lock.go`、`pkg/expression/expropt/optional_test.go`、`pkg/expression/sessionexpr/sessionctx.go`、`pkg/expression/builtin_miscellaneous.go`，核对了接口、注册点、Reader 消费者及 SQL 层边界归属。
- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`query AdvisoryLockPropProvider`、`query AdvisoryLockPropReader`、`query advisory_lock_ctx`、`node`/`explore` 查询确认了符号位置、Rust 直接调用者及 trait object 动态边界。图对泛化/动态调用的噪声以直接源码引用搜索交叉验证。
- 本任务为纯文档分析，按任务约定不运行 Cargo；结构验证确保目标文件存在且恰好包含规定的十一个二级章节。
