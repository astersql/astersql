# `pkg/errctx/context.rs`

源码：[context.rs](./context.rs)

## 文件定位

`context.rs` 是 `astersql-errctx` crate 的核心实现文件，由同目录的 `lib.rs` 以 `pub mod errctx` 暴露。它位于 SQL 执行链的错误策略层：调用方把带 MySQL/TiDB 错误码的 `SharedError` 交给 `Context`，本文件再依据错误所属的 `ErrGroup` 决定返回错误、追加 warning，或忽略错误。它不生成 SQL 错误，也不保存完整的语句诊断区；错误类型和因果链来自 `contextutil_crate::errors`，warning/note 的实际存储由注入的 `contextutil::WarnAppender` 完成。

`pkg/errctx/Cargo.toml` 将该 crate 定义为 `astersql-errctx`，入口为 `lib.rs`，直接依赖 `astersql-errno` 和 `astersql-util-context`。生产调用可经本 crate 直接访问，也会通过 expression、statement context、DistSQL context、table context 和 ranger context 等门面继续向上暴露。RustCodeGraph 的文件查询显示本文件被 59 个文件引用；范围化源码检索确认典型接线位于 `pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/distsql/context/context.rs` 和 `pkg/table/column.rs`。

## 核心职责

1. 用 `Level` 表示某一类 SQL 错误的三种处置结果：`LevelError`、`LevelWarn`、`LevelIgnore`。
2. 用 `ErrGroup` 和定长 `LevelMap` 把多个 errno 汇聚为七类策略槽，避免每个调用点重复判断具体错误码。
3. 通过 `Context::{HandleError, HandleErrorWithAlias}` 统一执行“解开因果错误、识别 errno、查组、查级别、返回/告警/忽略”的流程。
4. 提供不修改原对象的策略派生接口，以及严格且丢弃 warning 的全局 `StrictNoWarningContext`。
5. 复刻 Go `pkg/errctx/context.go` 的错误码分组、组合错误顺序、别名语义和 `ResolveErrLevel` 优先级。

本文件只处理已登记的 SQL errno。普通错误、无法转换为 `u16` 的错误码以及未登记 errno 均不会被降级，而是返回调用方提供的 `err` 别名。

## 主要符号

- `Level`：`#[repr(u8)]` 的公开枚举。默认值是 `LevelError = 0`；另外两项是 `LevelWarn = 1` 和 `LevelIgnore = 2`。默认值与 Go 零值严格模式一致。
- `ErrGroup`：`#[repr(usize)]` 的公开枚举，枚举值直接充当 `LevelMap` 下标。七项依次是截断、重复键、非法 NULL、缺少默认值、除零、自增读取失败、无匹配分区。
- `errGroupCount` / `LevelMap`：分组数常量为 7；`LevelMap` 是 `[Level; 7]`。添加分组时必须同步枚举、计数、errno 映射和测试。
- `WarnAppenderRef`：`Arc<dyn contextutil::WarnAppender + Send + Sync>`，允许克隆 `Context` 时共享同一 warning 接收端，并允许跨线程持有。
- `Context`：私有字段 `levelMap` 与 `warnHandler` 组成的可克隆策略对象。级别数组按值复制，handler 通过 `Arc` 共享。
- `LevelMap`、`LevelForGroup`：分别返回整个数组副本和单组级别；调用者不能借此原地修改 `Context`。
- `WithStrictErrGroupLevel`、`WithErrGroupLevel`、`WithErrGroupLevels`：创建新 `Context`，保留同一 handler；原上下文不变。
- `AppendWarning` / `AppendNote`：分别把 `SharedError` 委派给 handler 的 warning/note 接口。
- `HandleError`：接受可空错误；识别 `errors::Errors` 展开的组合错误并按顺序递归处理，遇到首个需要返回的错误即短路。
- `HandleErrorWithAlias`：核心处置入口。`internalErr` 用于分类，`err` 是 Error 级别时的对外返回值，`warnErr` 是 Warn 级别时记录的对外值。
- `ErrGroupForCode`：以穷举 `match` 将 Go `init` 表中的 18 个 errno 映射到七个分组，未知码返回 `None`。
- `NewContext` / `NewContextWithLevels`：前者构造全 Error 策略，后者接受完整映射；两者都要求调用者提供非空的 Rust trait object。
- `StrictNoWarningContext`：`LazyLock<Context>`，首次访问时用 `contextutil::ignoreWarn` 创建全 Error 上下文。
- `ResolveErrLevel`：从 `(ignore, warn)` 标志解析级别；`ignore` 优先，其次 `warn`，否则 Error。

## 执行流程

典型主链如下：会话或求值上下文创建 `Context`，表达式、表编码、统计或 ranger 代码得到 `SharedError` 后调用 `HandleError`，然后根据返回的 `Option` 决定继续还是向上抛错。例如 `pkg/sessionctx/stmtctx/stmtctx.rs` 的同名方法直接转发给字段 `errCtx`；`pkg/expression/errors.rs`、`pkg/table/column.rs` 和 `pkg/util/ranger/ranger.rs` 则直接消费结果。

`HandleError` 的具体步骤是：

1. `None` 立即返回 `None`。
2. 调用 `errors::Errors(&err)` 取得子错误。若子项数量不是 1，或唯一子项并非与原错误共享同一底层指针，则把输入视为错误组。
3. 对错误组按原顺序递归调用 `HandleError`。已降级为 warning/ignore 的子项返回 `None`，继续处理下一项；首个返回 `Some` 的子项立即结束整个流程，后续子项不会被处理。
4. 对单一错误调用 `HandleErrorWithAlias(Some(&err), err.clone(), err.clone())`，即默认用同一错误完成分类、返回和告警。

`HandleErrorWithAlias` 先以 `errors::Cause` 找到 `internalErr` 的根因；根因若不是共享错误库的 `errors::Error`、错误码不能转成 `u16`，或 `ErrGroupForCode` 未登记该码，则原样返回 `err`。成功归组后读取对应槽：Error 返回 `err`；Warn 调用 `AppendWarning(warnErr)` 后返回 `None`；Ignore 直接返回 `None`。

级别创建链也有直接证据：`pkg/session/runtime/system_session.rs` 按 SQL mode 调用 `ResolveErrLevel`；`pkg/sessionctx/stmtctx/stmtctx.rs::newErrCtx` 把截断标志和已有级别合并后调用 `NewContextWithLevels`；表达式静态上下文和 DistSQL 上下文也分别在自己的构造过程中注入 warning appender。

## 数据与状态

`LevelMap` 是小型定长数组，读取与派生都是按值复制；不存在哈希表、运行时注册或可变全局 errno 映射。`ErrGroup as usize` 是数组索引，因此枚举判别值、声明顺序和 `errGroupCount` 共同构成内部不变量。

`Context` 自身没有内部锁，也不会修改 `levelMap`。`Clone` 和所有 `With*` 方法只克隆 handler 的 `Arc`；因此多个派生上下文拥有独立策略数组，却把 warning/note 写入同一接收端。接收端如何计数、加锁、截断或保存警告不属于本文件，由具体 `WarnAppender` 实现决定。

组合错误处理中使用 `SharedError::ptr_eq` 区分“普通错误经 `errors::Errors` 返回自身”与“真正的错误组”。这避免把普通错误无限递归地再次当作组处理。

## 依赖与调用关系

下游依赖均经 `lib.rs` 重导出：

- `crate::errno`：来自 `astersql-errno`，提供 MySQL/TiDB errno 常量；`ErrGroupForCode` 直接使用。
- `crate::errors`：来自 `astersql-util-context`，提供 `SharedError`、`Error`、`Cause` 和 `Errors`，承担共享所有权、根因解析、SQL 错误 downcast 与组合错误展开。
- `crate::contextutil`：同样来自 `astersql-util-context`，提供 `WarnAppender`、`ignoreWarn` 以及 warning/note 委派接口。
- 标准库 `Arc`：共享 warning handler；`LazyLock`：延迟初始化静态严格上下文。

已核对的直接上游包括：

- `pkg/sessionctx/stmtctx/stmtctx.rs`：保存 `errctx::Context`，转发 `HandleError`、`HandleErrorWithAlias`、`LevelMap`、`LevelForGroup`，并按类型标志重建错误上下文。
- `pkg/session/runtime/system_session.rs`：根据 strict mode 和 division-by-zero mode 用 `ResolveErrLevel` 填充级别映射。
- `pkg/expression/exprstatic/evalctx.rs`、`pkg/expression/exprctx/context.rs`：构造或派生表达式求值用错误上下文。
- `pkg/expression/errors.rs`、`pkg/expression/expr_to_pb.rs`：将表达式错误交给 `HandleError`，以返回值判断是否继续传播。
- `pkg/table/column.rs`、`pkg/table/tables/partition_expr.rs`、`pkg/util/ranger/ranger.rs`、`pkg/statistics/{cmsketch,fmsketch}.rs`：在列转换、分区表达式、范围构建和统计处理中应用策略。
- `pkg/distsql/context/context.rs`、`pkg/ddl/backfilling_txn_executor.rs`：创建带各自 warning handler 的上下文。

RustCodeGraph 的精确 `callers/callees` 子命令在本次索引上超时且没有返回边；以上调用关系由 RustCodeGraph 的文件/符号节点及范围化源码命中逐项核对，不把“文件引用”数量等同于直接调用数量。

## 错误处理与边界

- `None` 表示无错误；`HandleError(None)` 和 `HandleErrorWithAlias(None, ...)` 都返回 `None`。
- 只有能追溯到 `errors::Error` 且 code 能转为 `u16` 的错误才参与分组。普通错误、负数或超范围错误码、未知 errno 都按 Error 路径返回 `err`，不会静默吞掉。
- Error、Warn、Ignore 三个分支分别返回传入的返回别名、记录 warning 别名、无副作用忽略。分类始终依据 `internalErr`，这允许稳定对外消息而不丢失内部分类信息。
- warning handler 在 Rust 类型上不是 `Option`，构造函数直接接收 `Arc<dyn ...>`；因此与 Go 版的运行时 nil 防御不同，本文件依赖类型系统保证 handler 存在。handler 内部若 panic，本文件不捕获。
- 组合错误按顺序短路。这是刻意对齐 Go 的兼容行为：首个需返回错误之后，即使还有本应变成 warning 的子错误，也不会继续处理。
- `StrictNoWarningContext` 的名称中“无 warning”指 sink 丢弃输入；其级别仍全是 Error，因此正常经 `HandleError` 进入的已分类错误会返回，而不是被忽略。

## 并发与资源生命周期

`Context` 没有后台任务、通道、文件、网络连接或显式析构逻辑。策略数组完全由值拥有，随 `Context` 自动释放。handler 通过 `Arc` 引用计数管理：构造时转入一个强引用，克隆或派生上下文时增加强引用，最后一个持有者释放时才销毁实际接收端。

`WarnAppenderRef` 的 `Send + Sync` 约束允许 `Context` 被放入跨线程的上层状态，但本文件不为 handler 的内部可变状态加锁；线程安全责任由 trait 实现承担。测试中的 `Arc<Mutex<Option<SharedError>>>` 和静态 warning handler 展示了可用实现。`StrictNoWarningContext` 由 `LazyLock` 保证一次初始化，初始化后只共享不可变策略和线程安全 handler。

## 与 Go 版本的对应关系

Rust 的 `Level`、`LevelMap`、`Context`、`ErrGroup`、三个 `With*` 方法、两个处理入口、两个构造函数、严格静态上下文以及 `ResolveErrLevel` 均对应 `pkg/errctx/context.go` 的同名概念。`ErrGroupForCode` 的 `match` 是 Go `init` 中 `group2Errors`/`errGroupMap` 的静态等价物；测试逐码覆盖了 Go 表中的全部分类。

关键语义保持一致：默认零值语义是 Error；派生上下文不修改原上下文；错误组顺序处理并在首个返回错误处停止；内部错误负责分类，返回/告警别名负责用户可见内容；`ignore` 在 `ResolveErrLevel` 中优先于 `warn`。

实现差异主要来自语言模型：Rust 用 `[Level; 7]` 和枚举转下标代替 Go 数组索引；用 `Option<SharedError>` 表示 nil；用 `Arc<dyn WarnAppender + Send + Sync>` 排除 nil handler；用 `LazyLock` 建立静态上下文；用 `errors::Errors` 加指针同一性判断来识别错误组，而 Go 直接断言 `errors.ErrorGroup`。另外，Go 的 `AppendWarning`/`AppendNote` 在非测试构建下仍对 nil handler 做保护，Rust 无对应 nil 分支。

`pkg/errctx/context_test.rs` 复刻 Go `context_test.go` 的核心断言；`pkg/errctx/migration_aster_unit_test.rs` 进一步覆盖全部 errno 映射、嵌套组合错误短路、warning/note 级别、别名及 `ResolveErrLevel` 四种输入组合。

## 扩展指南

新增错误分组时，应同时修改 `ErrGroup`、末尾的 `errGroupCount`、`ErrGroupForCode` 和所有需要显式构造 `LevelMap` 的上层策略，并在 `pkg/errctx/migration_aster_unit_test.rs` 增加逐码分类断言。因为枚举值是数组下标，宜在末尾追加；插入或重排会改变判别值，需先审计任何序列化、FFI 或整数转换使用。还应同步 Go `pkg/errctx/context.go`，除非迁移计划明确允许语义分叉。

新增某个现有组的 errno 时，只修改 `ErrGroupForCode` 仍不足以交付：应核对 errno 常量存在，更新迁移测试的 `classifications`，并检查 Go 映射是否同样包含该码。遗漏映射的安全表现是返回错误，但会造成 SQL mode 下本应告警/忽略的语句行为不兼容。

改变组合错误处理时，应优先扩展独立测试文件，而不要在 `context.rs` 内嵌测试。重点覆盖嵌套组、首个 fatal 后不再产生 warning、单子项组以及根因包装。改变 alias 规则时，应分别断言内部分类错误、返回别名和 warning 别名的指针身份。

性能上，热路径目前是定长数组索引和常量 `match`；不要无必要地引入分配或动态映射。兼容性上，最敏感的是默认 Error 语义、短路顺序、errno 清单和 `ignore > warn > error` 的优先级。并发扩展应保持 handler 的 `Send + Sync` 约束，不应在 `Context` 内引入无界增长的共享可变状态。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出 `context.rs`、`context_test.rs`、`migration_aster_unit_test.rs` 及 Go 对照文件。
- RustCodeGraph 源码节点：`pkg/errctx/context.rs` 全部 254 行；主要符号包括 `Level`、`ErrGroup`、`Context`、`HandleError`、`HandleErrorWithAlias`、`ErrGroupForCode`、`NewContextWithLevels`、`StrictNoWarningContext`、`ResolveErrLevel`。
- crate 与模块证据：`pkg/errctx/Cargo.toml`、`pkg/errctx/lib.rs`。
- Rust 独立测试：`pkg/errctx/context_test.rs`、`pkg/errctx/migration_aster_unit_test.rs`。
- Go 对照：`pkg/errctx/context.go`、`pkg/errctx/context_test.go`。
- 上游桥接与直接使用证据：`pkg/sessionctx/stmtctx/stmtctx.rs`、`pkg/session/runtime/system_session.rs`、`pkg/expression/errors.rs`、`pkg/expression/exprstatic/evalctx.rs`、`pkg/distsql/context/context.rs`、`pkg/table/column.rs`、`pkg/util/ranger/ranger.rs`。
- RustCodeGraph 的精确 `callers/callees` 查询连续超时且未输出；调用边由已索引源码节点与范围化 `rg` 交叉核验。任务是纯文档分析，按计划未运行 Cargo 或代码测试。
