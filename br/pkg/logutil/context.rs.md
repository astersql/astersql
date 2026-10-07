# `br/pkg/logutil/context.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-logutil`，由 [`br/pkg/logutil/lib.rs`](./lib.rs) 以 `pub mod context` 装配，并把 `CL`、`Context`、`ContextWithField`、`LoggerFromContext`、`ResetGlobalLogger` 重新导出为 crate 公共 API。它移植自 [`br/pkg/logutil/context.go`](./context.go)，职责是把日志器附着到轻量上下文，并为没有上下文日志器的调用提供包级回退。

这里的 `Context` 不是仓库其他子系统中兼具取消、超时和值传递能力的通用上下文；它只保存 `Option<Logger>`。RustCodeGraph 对当前 Rust 源码的检索未发现本文件公开函数在生产文件中的直接调用点；`br/pkg/restore/log_client/*.rs` 中可见的 `logutil::CL` 实际导入自该 crate 自己的 `stubs::logutil`，不能当成本文件已经接入恢复主链的证据。

## 核心职责

- 用 `Context` 保存一个可选的、已绑定字段的 `Logger`。
- 由 `LoggerFromContext` 实现固定的三级选择顺序：上下文日志器、`GLOBAL_LOGGER`、`default_logger()`。
- 由 `ContextWithField` 从当前有效日志器派生新日志器并返回新上下文，使字段随新上下文传播，而不修改输入上下文或原日志器。
- 由 `ResetGlobalLogger` 提供主要面向测试的全局回退替换入口；`CL` 只是 `LoggerFromContext` 的短名称。

该层不负责日志级别过滤、字段编码或实际输出；这些行为由 [`br/pkg/logutil/logging.rs`](./logging.rs) 中的 `Logger::With`、`Logger::log` 和 `default_logger` 承担。

## 主要符号

- `GLOBAL_LOGGER: LazyLock<RwLock<Option<Logger>>>`：进程内延迟初始化的包级可选日志器。`None` 表示继续使用默认日志器。
- `ResetGlobalLogger(l: Option<Logger>)`：在写锁下整体替换全局日志器；传入 `None` 可恢复默认回退路径。
- `global_logger() -> Option<Logger>`：内部读辅助函数；取得读锁后克隆当前 `Option<Logger>`，避免把锁守卫带出函数。
- `Context { logger: Option<Logger> }`：可克隆、可默认构造的轻量值对象。字段私有，外部只能通过本文件的 API 建立带日志器的上下文。
- `Context::Background() -> Context`：返回 `Default` 上下文，即 `logger == None`。
- `ContextWithField(c: Context, fields: impl IntoIterator<Item = Field>) -> Context`：消费输入上下文，解析其有效日志器，调用 `Logger::With` 合并字段，再将派生日志器存入新上下文。
- `LoggerFromContext(c: &Context) -> Logger`：返回克隆后的上下文日志器；若不存在，则克隆全局日志器；若全局也未设置，则新建默认 tracing 日志器。
- `CL(c: &Context) -> Logger`：无额外行为的简写入口。

## 执行流程

1. 调用者通常从 `Context::Background()` 得到不含日志器的上下文。
2. 首次调用 `LoggerFromContext(&ctx)` 或 `CL(&ctx)` 时，函数先检查 `ctx.logger`；背景上下文会进入全局回退。
3. 若 `ResetGlobalLogger(Some(logger))` 已设置全局日志器，`global_logger()` 在读锁下克隆它；否则 `unwrap_or_else(default_logger)` 创建无预置字段、使用 tracing 后端的日志器。
4. `ContextWithField(ctx, fields)` 先走同一解析路径取得当时有效的日志器，再由 `Logger::With` 克隆已有字段并追加新字段，最后返回 `logger: Some(derived)` 的新上下文。
5. 后续从新上下文取日志器时直接命中第一层，不再读取全局值。因此全局日志器稍后被替换或清空，不影响已经包装的上下文；继续包装该上下文也会沿用旧日志器后端和已有字段。

RustCodeGraph 的直接调用边印证了上述结构：`ContextWithField → LoggerFromContext → global_logger/default_logger`，以及 `CL → LoggerFromContext`。

## 数据与状态

持久的共享状态只有 `GLOBAL_LOGGER`。它保存一个 `Logger` 克隆句柄，而不是日志记录本身；真实后端及字段位于 [`logging.rs`](./logging.rs) 的 `Logger` 中。`Logger::With` 创建新值，先复制原字段，再按迭代顺序追加字段，所以上下文字段先于单次日志调用传入的字段出现；`logging_test.rs::test_contextual` 对此顺序和内容有断言。

`Context::Background()` 没有显式生命周期资源，也不捕获创建时的全局日志器。只有执行 `ContextWithField` 后，解析到的日志器才被快照式地存入 `Context.logger`。因此“背景上下文观察最新全局值”和“已包装上下文保持旧值”是两个需要共同维持的不变量。

## 依赖与调用关系

本文件只直接依赖标准库的 `LazyLock`、`RwLock`，以及同 crate `logging` 模块的 `Field`、`Logger`、`default_logger`。[`br/pkg/logutil/Cargo.toml`](./Cargo.toml) 把 crate 定义为 library，未为 `context.rs` 声明条件 feature；`tracing` 是默认日志器最终输出所用的 crate 级依赖，但本文件不直接调用它。

模块入口 [`lib.rs`](./lib.rs) 对外重导出全部上下文 API。仓库中 `summary`、`rtree`、`metautil`、`conn/util`、`utils` 等 Cargo manifest 依赖 `astersql-br-pkg-logutil`，但依赖 crate 并不等价于调用本文件；按当前 `rg` 与 RustCodeGraph 结果，本文件真实的 Rust 调用证据集中在同 crate 的独立测试中。Go 侧则由 BR 各调用方通过 Go 包 `br/pkg/logutil` 使用相应 API。

## 错误处理与边界

所有公开函数都返回普通值而非 `Result`，没有可恢复错误通道。唯一显式失败边界是 `GLOBAL_LOGGER` 的 `RwLock` 中毒：读写路径都用 `expect("global logger lock poisoned")`，此前若有线程持锁 panic，后续访问会继续 panic，而不会静默退回默认日志器。

`LoggerFromContext` 的参数是非空引用，所以 Rust API 不存在 Go 中传入 nil `context.Context` 后调用 `Value` 的同类运行时边界。`ContextWithField` 接受空迭代器，此时仍会生成保存当前有效日志器的新上下文；这会把全局日志器的当前值固定下来。字段重复、日志级别与编码冲突不在本文件处理，而由 `Logger` 的追加和输出语义决定。

## 并发与资源生命周期

全局替换和读取通过 `RwLock` 串行化写操作并允许并发读取，消除了 Go 原实现中裸全局指针的 Rust 数据竞争风险。`global_logger()` 在锁内只做克隆，返回前释放锁；实际写日志、字段合并或 tracing 输出均不持有该全局锁，因此临界区很短。

`Context` 和 `Logger` 都按值克隆；已派生上下文拥有自己的日志器句柄和字段快照。`ResetGlobalLogger` 不追踪也不更新现存上下文。测试设置全局 capture 日志器后必须调用 `ResetGlobalLogger(None)` 清理，否则共享进程状态可能污染并行或后续测试；当前两个 Rust 测试路径都显式清理，但该全局状态本身不提供测试级自动恢复守卫。

## 与 Go 版本的对应关系

[`context.go`](./context.go) 的 `globalLogger`、`ResetGlobalLogger`、`ContextWithField`、`LoggerFromContext`、`CL` 均有一一对应的 Rust 符号。选择顺序和关键语义一致：上下文值优先，其次测试可替换的全局日志器，最后使用包默认日志器；已经由 `ContextWithField` 包装的上下文不会因后续全局重置而变化。

主要表示差异如下：Go 使用标准 `context.Context` 和私有 `loggingContextKey`/`context.WithValue`，可同时保留取消、截止时间及其他值；Rust 使用仅含日志器的专用 `Context`，没有这些通用上下文能力。Go `ContextWithField` 接受可变参数 `...zap.Field` 且不消费父上下文；Rust 接受 `IntoIterator<Item = Field>` 并消费 `Context`，复用父上下文时需先 `clone()`。Go 返回 `*zap.Logger`，Rust 返回可克隆的 `Logger` 值。Go 全局变量是裸指针，Rust 用 `LazyLock<RwLock<Option<Logger>>>` 保证并发访问安全，并新增锁中毒 panic 边界。

Go 的 `logging_test.go::TestContextual` 与 Rust 的 `logging_test.rs::test_contextual` 都验证全局 capture 回退、上下文字段继承和字段顺序；`parity_test.rs::go_rust_public_contract_matches` 还验证 `CL` 简写及相同的两条日志契约。

## 扩展指南

- 若要新增上下文属性，先判断它是否属于日志职责。扩展 `Context` 时应保持 `LoggerFromContext` 的三级优先级，并在独立测试文件中覆盖背景上下文、已包装上下文及全局重置后的行为。
- 若要改变字段合并规则，应修改 [`logging.rs`](./logging.rs) 的 `Logger::With`，并同步 `logging_test.rs::test_contextual`、`parity_test.rs::go_rust_public_contract_matches` 及 Go 对照测试；不要在 `context.rs` 复制字段处理逻辑。
- 若要把真实 BR 生产路径接到此 API，必须先确认调用方使用的是 `astersql_br_pkg_logutil`，而非局部 `stubs::logutil`，并处理专用 `Context` 与调用方通用上下文类型之间的适配。不能仅凭同名 `CL` 判断已经接线。
- 若要提高测试隔离性，可设计作用域式全局日志器守卫，但需保持现有 `ResetGlobalLogger` 公共契约，并验证并发测试不会互相覆盖。全局单槽位意味着同时替换仍是最后写入者生效。
- Rust 单元测试应继续放在 `logging_test.rs` 或新增的独立测试文件中，不要内嵌到 `context.rs`；同时保持 Go `TestContextual` 的行为意图，不以简化实现代替移植语义。

## 验证依据

- 源码与模块边界：`br/pkg/logutil/context.rs`、`br/pkg/logutil/lib.rs`、`br/pkg/logutil/logging.rs`、`br/pkg/logutil/Cargo.toml`。
- Go 对照：`br/pkg/logutil/context.go`、`br/pkg/logutil/logging_test.go::TestContextual`。
- Rust 独立测试：`br/pkg/logutil/logging_test.rs::test_contextual`、`br/pkg/logutil/parity_test.rs::go_rust_public_contract_matches`；两者验证全局回退、字段继承，后者额外覆盖 `CL`。
- RustCodeGraph：索引状态为 11,467 个文件（其中 Rust 7,032 个）；`files --filter br/pkg/logutil` 确认目标及相邻实现/测试；`node --file br/pkg/logutil/context.rs` 核对 80 行源码；`query`/`explore` 核对 `LoggerFromContext`、`ContextWithField`、`CL`、`default_logger` 及直接调用边。
- 普通文本检索仅用于索引外/跨语言与精确接线核验：确认真实 Rust API 的测试引用、Go 对照引用、依赖该 crate 的 Cargo manifests，并辨别 `restore/log_client` 的同名局部桩。
- 本任务是纯文档分析，按计划不运行 Cargo。最终结构检查要求本文恰有十一个规定的二级标题。
