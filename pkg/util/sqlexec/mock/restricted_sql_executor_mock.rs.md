# `pkg/util/sqlexec/mock/restricted_sql_executor_mock.rs`

## 文件定位

本文件属于独立 crate `astersql-util-sqlexec-mock`，其清单是 `pkg/util/sqlexec/mock/Cargo.toml`。crate 只直接依赖父目录的 `astersql-util-sqlexec`，并由 `pkg/util/sqlexec/mock/lib.rs` 将本模块的公开项再导出。目标文件实现的是 `pkg/util/sqlexec/restricted_sql_executor.rs` 中 `sqlexec::RestrictedSQLExecutor` trait 的测试替身，不执行真实 SQL，也不参与生产 SQL 请求主链。

文件头标明它源自 Go MockGen 为 `pkg/util/sqlexec.RestrictedSQLExecutor` 生成的 mock；当前 Rust 文件是该生成逻辑的机械迁移，并已包含 AsterSQL 处理标记。相关 Rust 测试不内嵌在生产文件中，而是由 `lib.rs` 的 `#[cfg(test)]` 项加载 `pkg/util/sqlexec/mock/migration_aster_unit_test.rs`。

## 核心职责

`MockRestrictedSQLExecutor` 为受限 SQL 执行边界提供可编排、可观测的严格 mock。测试先通过 `EXPECT()` 得到 recorder，再为 `ExecRestrictedSQL`、`ExecRestrictedStmt` 或 `ParseWithParams` 注册一次性闭包；实际 trait 调用按各方法各自的 FIFO 队列取出一个闭包，将原始参数交给它，并原样返回闭包结果。

严格性体现在两个阶段：没有已注册期望的调用会在 `take` 中立即 panic；测试结束前调用 `verify` 时，任何尚未消费的期望也会触发断言失败。与此同时，每次实际调用会先写入简化的 `Call` 日志，使处理器本身 panic 时仍能保留“调用曾发生”的观测事实。

## 主要符号

- `AnyValue = Box<dyn Any>`：承载 Go 可变参数 `...any` 的动态值。调用方和期望闭包通过具体类型的 `downcast_ref` 解释值。
- `MockError = sqlexec::GoError`：沿用父 crate 的 `Box<dyn Error + Send + Sync>` 错误边界。
- `ExecOptionFn = sqlexec::OptionFuncAlias`：受限执行选项的一次性闭包；它本身不是 `Clone`，因此调用时按值传递。
- `ExecResult` 及三个 `*Handler` 私有类型别名：统一三个 trait 方法的闭包签名和返回值。所有 handler 都是 `FnOnce`，准确表达“一项期望只消费一次”。
- `Call`：公开的调用摘要枚举。SQL/解析调用记录 SQL 字符串和参数个数，语句调用只记录选项个数；它不保存上下文、AST、选项值或动态参数本体。
- `State`：私有共享状态，包含调用日志以及三个互相独立的 `VecDeque` 期望队列。
- `MockRestrictedSQLExecutor`：实现 `RestrictedSQLExecutor` 的公开 mock，内部仅持有 `Arc<Mutex<State>>`，可廉价克隆并共享期望和日志。
- `MockRestrictedSQLExecutorMockRecorder`：由 `EXPECT()` 返回的 recorder；其三个同名方法分别向对应队列尾部追加 handler。
- `NewMockRestrictedSQLExecutor`：保留 Go 命名的构造函数，返回空状态的值，而不是 Go 版本所需的 controller 指针。
- `lock`：统一获取互斥锁；锁被 poison 后取回内部状态继续运行。
- `take`：从指定队列头部弹出 handler；队列为空时以方法名构造 panic 消息。
- `EXPECT`、`ISGOMOCK`、`calls`、`verify`：分别用于取得 recorder、保留 GoMock 身份标记、读取调用快照、检查所有期望已消费。

## 执行流程

1. 测试调用 `NewMockRestrictedSQLExecutor()` 创建空 mock；也可以用 `Default` 获得等价状态。
2. 测试调用 `executor.EXPECT().ExecRestrictedSQL(...)` 等 recorder 方法。recorder 锁住共享 `State`，把装箱后的 `FnOnce` handler 追加到该方法的队列尾部。
3. 被测代码通过 `RestrictedSQLExecutor` trait 调用 mock。实现先锁住状态，根据方法参数生成一条 `Call` 摘要并追加到 `calls`。
4. 同一个临界区内，`take` 从该方法队列头部移除下一项 handler。若队列为空，调用当场 panic；调用日志已经在 panic 前写入。
5. 锁守卫随局部代码块结束而释放，然后才执行 handler。这样 handler 可以访问 mock 的其他克隆或 recorder，而不会因重入同一互斥锁导致自锁。
6. handler 接收完整的上下文及该方法的业务参数，自行断言、构造成功结果或返回 `MockError`；trait 方法不改写其返回值。
7. 测试可用 `calls()` 获取日志快照，并在结束前显式调用 `verify()`，确认三个队列均为空。

三个方法的队列彼此独立；FIFO 只约束同一方法内部的多项期望，并不建立跨方法的全局调用顺序。例如先注册 parse、后注册 stmt，并不强制实际调用也采用这个跨方法顺序。

## 数据与状态

所有可变状态都位于 `State`。`calls` 只追加调用摘要；三个 `VecDeque` 分别存放 SQL 执行、AST 执行和带参解析的一次性 handler。注册发生在队尾，消费发生在队首，因此同一方法的期望保持 FIFO。

`MockRestrictedSQLExecutor` 和 recorder 的克隆共享同一个 `Arc<Mutex<State>>`，不是状态副本。`calls()` 则克隆 `Vec<Call>` 后返回快照，后续调用不会反向修改已取得的快照。动态参数、AST 和选项闭包均按值移交 handler，不留存在日志中；这一选择避免要求这些值可克隆，但也限制了事后审计粒度。

`Call` 派生 `Eq`/`PartialEq`，方便测试精确比较调用顺序和计数；`State` 及 mock 派生 `Default`，初始调用日志和所有队列均为空。

## 依赖与调用关系

上游接口是 `pkg/util/sqlexec/restricted_sql_executor.rs::RestrictedSQLExecutor`：本文件完整实现其三个方法，并复用其中的 `GoError` 和 `OptionFuncAlias`。行、字段、AST 与上下文类型经 `pkg/util/sqlexec/mock/lib.rs` 从父 crate 再导出为 `crate::{chunk, resolve, ast, context, sqlexec}`。

直接使用证据位于 `pkg/util/sqlexec/mock/migration_aster_unit_test.rs`：该测试从 crate 根导入 `NewMockRestrictedSQLExecutor` 和 `Call`，经 `EXPECT()` 注册 handler，并通过 trait 的完全限定调用触发实现。RustCodeGraph 将目标文件列为被多个文件“使用”，但精确文本检索表明本 mock 名称的直接 Rust 使用集中在该独立测试；大量泛化的 `calls`/`verify` 同名命中不是本类型的真实调用边。

同目录 `mock.rs` 定义 `RestrictedSQLExecutorKey`，用于保持 Go 上下文键字符串 `__MockRestrictedSQLExecutor`；它与本文件由 crate 根共同导出，但本文件本身不读取或写入 context 键。`pkg/statistics/handle/util/util.go` 使用的是 Go 生成 mock，也不是 Rust 实现的调用者。

## 错误处理与边界

handler 返回的成功值或 `MockError` 由三个 trait 方法直接传播，不包装、不分类也不记录错误。专属测试以 `io::Error` 验证 `ExecRestrictedStmt` 和 `ParseWithParams` 的错误文本保持不变。

意外调用属于测试配置错误：`take` 在缺少期望时 panic。未消费期望属于测试收尾错误：`verify` 对任一非空队列执行断言并 panic。`verify` 只看剩余队列，不检查调用日志内容，也不会自动在 `Drop` 时运行，因此调用者必须显式执行它。

互斥锁 poison 被 `lock` 有意忽略，继续使用 `PoisonError::into_inner()` 取得状态。这适合测试替身在捕获 panic 后继续检查状态，但意味着 poison 本身不会作为错误或失败信号传播。handler 在解锁后运行，其 panic 原样向上传播。

参数匹配完全由 handler 内的断言或逻辑负责；recorder 不提供 GoMock matcher、调用次数范围、跨方法顺序、默认动作或类型安全的可变参数匹配。`Call` 日志也只保留有限摘要，不能据此重放请求。

## 并发与资源生命周期

`Arc<Mutex<State>>` 保证 mock、其克隆和 recorder 访问队列及日志时互斥；“记录调用并取出期望”在同一锁持有期内完成，因此并发调用不会消费同一个 handler。实际 handler 在锁外运行，缩短临界区，也允许 handler 间接使用共享 mock。

不过 handler 类型未要求 `Send` 或 `Sync`，`AnyValue` 也未要求 `Send`，所以整个 mock 并未承诺可安全跨线程移动或共享。内部使用同步原语主要用于共享所有权和可重入测试编排，不能据此推断它是通用并发测试框架。

handler 被注册后由队列拥有，实际调用时移出并消费；未消费 handler 在共享状态最后一个 `Arc` 释放时被丢弃。没有后台任务、通道、事务、文件句柄或真实 session 生命周期。克隆体只要仍存活，共享状态就继续存在；不存在自动验证析构器。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/sqlexec/mock/restricted_sql_executor_mock.go`。两边都提供 `MockRestrictedSQLExecutor`、recorder、`NewMockRestrictedSQLExecutor`、`EXPECT`、`ISGOMOCK`，并覆盖 `ExecRestrictedSQL`、`ExecRestrictedStmt`、`ParseWithParams` 三个接口方法。Rust 的 handler 参数顺序与父 trait 对齐，返回的行、字段、AST 和错误语义对应 Go 接口。

Go 版本把调用交给 `gomock.Controller`，由 controller 完成 matcher、反射方法类型、期望次数和生命周期验证；Rust 版本没有引入 GoMock 等价框架，而是用每方法 FIFO `FnOnce` 队列实现聚焦替身。Rust 构造函数因此不接收 controller，recorder 方法接收具体闭包而不是 matcher 参数，验证也必须显式调用 `verify()`。

Go 的 `ExecRestrictedSQL`/`ParseWithParams` 使用可变参数，Rust trait 用 `Vec<Box<dyn Any>>` 表达；Go 的 `ExecRestrictedStmt` 选项也是可变参数，Rust 使用 `Vec<OptionFuncAlias>`。Go 返回 `[]*resolve.ResultField`，Rust 返回拥有所有权的 `Vec<resolve::ResultField>`。这些是语言和父 trait 的既有边界差异，不应在本 mock 内自行转换成其他模型。

## 扩展指南

若 `RestrictedSQLExecutor` 新增方法，需同步完成四处修改：为返回值和 handler 签名增加类型别名；给 `Call` 增加足够但不过度持有参数的摘要分支；给 `State` 增加队列和 recorder 注册方法；在 trait impl 中按“先记录、同锁取 handler、锁外执行”模式实现，并把新队列加入 `verify`。同时应在独立文件 `pkg/util/sqlexec/mock/migration_aster_unit_test.rs` 增加成功透传、错误透传、FIFO/意外调用和未消费验证测试，不能把测试写入本生产文件。

若要增强匹配或顺序能力，应先确认是否要兼容 GoMock 的 controller 语义。直接给现有 handler 增加 `Send + Sync`、改变 `AnyValue` 边界或保存完整参数，可能破坏现有单线程闭包、所有权和动态类型用法；引入全局跨方法顺序也会改变当前“三个独立 FIFO”的契约。性能上当前每次注册、调用记录、取 handler、读日志和验证都需要同一把 mutex；测试用途通常足够，但高并发压测不应依赖此 mock 模拟真实执行吞吐。

修改生成来源接口时还应复核 Go 文件的 MockGen 输出和父 trait `pkg/util/sqlexec/restricted_sql_executor.rs`，确保 Rust 方法集合、参数顺序和结果类型仍保持对齐。`RestrictedSQLExecutorKey` 的上下文键语义属于相邻 `mock.rs`，只有扩展涉及 context 注入时才应同步调整。

## 验证依据

- RustCodeGraph `status`：索引可用，覆盖本仓库 Rust/Go 文件。
- RustCodeGraph `files --filter pkg/util/sqlexec/mock`：确认同目录 Rust/Go 实现、crate 根与独立测试文件。
- RustCodeGraph `node --file pkg/util/sqlexec/mock/restricted_sql_executor_mock.rs`：核对全部 225 行、公开/私有符号、FIFO 队列、日志顺序、锁和 trait 实现。
- RustCodeGraph `query MockRestrictedSQLExecutor` 与 `query RestrictedSQLExecutor --kind trait`：核对 Rust/Go 对应类型、构造函数和父 trait 定义位置。
- RustCodeGraph `node --file pkg/util/sqlexec/restricted_sql_executor.rs`：核对三个 trait 方法以及 `GoError`、`OptionFuncAlias` 的真实签名。
- RustCodeGraph `node --file pkg/util/sqlexec/mock/restricted_sql_executor_mock.go`：核对 MockGen 原型及 GoMock controller/recorder 行为。
- RustCodeGraph `node` 读取 `pkg/util/sqlexec/mock/lib.rs`、`mock.rs`、`migration_aster_unit_test.rs`：核对模块再导出、context 键边界，以及参数透传、返回值、错误、FIFO、调用日志和 `verify` 的测试事实。
- `pkg/util/sqlexec/mock/Cargo.toml` 与 `pkg/util/sqlexec/Cargo.toml`：核对 crate 名称、父 crate 路径依赖、无独立 feature，以及测试由显式模块加载而非 Cargo autotest 自动发现。
- `rg` 精确检索 `MockRestrictedSQLExecutor|NewMockRestrictedSQLExecutor|restricted_sql_executor_mock`：区分本实现的直接 Rust 测试与其他目录自建的同名 mock，避免把常见 `calls`/`verify` 名称误判为调用关系。
- 未运行 Cargo：本任务仅新增分析文档，计划明确禁止 Cargo；结构检查按任务文件给定命令执行。
