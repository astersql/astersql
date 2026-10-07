# `pkg/expression/expropt/ddlowner.rs`

## 文件定位

本文件属于 `astersql-expression-expropt` crate，是表达式“可选求值属性”机制中 DDL Owner 状态的类型适配层。它不负责选举 DDL Owner，也不保存 Owner 状态；它只把一个查询当前节点是否为 Owner 的闭包包装成 `exprctx::OptionalEvalPropProvider`，并提供按固定属性键读取该闭包的 Reader。模块由 `pkg/expression/expropt/lib.rs` 的 `mod ddlowner; pub use ddlowner::*;` 纳入并公开再导出。

crate 边界由 `pkg/expression/expropt/Cargo.toml` 定义：crate 名为 `astersql-expression-expropt`，库入口是 `lib.rs`，关闭自动测试发现和 doctest；本文件直接使用经 `lib.rs` 再导出的 `exprctx-crate`，错误类型来自 `anyhow`。Cargo 清单没有为本能力设置条件 feature，因此该模块不是条件编译项。

## 核心职责

1. `DDLOwnerInfoProvider` 把 `Fn() -> bool` 封装为可注册的可选属性 Provider，使表达式层不必依赖会话或 DDL Owner 管理器的具体类型。
2. `OptionalEvalPropProvider::Desc` 将 Provider 固定绑定到 `exprctx::OptPropDDLOwnerInfo`；`as_any` 为公共注册表的安全向下转型提供入口。
3. `DDLOwnerPropReader` 通过 `RequireOptionalEvalProps` 声明自己只需要 DDL Owner 这一位属性，并通过 `is_ddl_owner` 完成“按键查找、校验具体类型、调用闭包”的读取过程。

该文件是依赖注入边界，不包含 DDL 调度、Owner 租约、缓存、重试或 SQL 返回值转换。RustCodeGraph 显示生产侧 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 会注册 Provider；但目前 `DDLOwnerPropReader::is_ddl_owner` 的直接调用者是 `pkg/expression/expropt/migration_aster_unit_test.rs`、`pkg/expression/expropt/optional_test.rs` 等测试，未找到它接入 Rust 生产内置函数求值链的静态证据。`pkg/expression/builtin_info.rs::tidb_is_ddl_owner` 当前是接收既成 `bool` 并转换为 `i64` 的独立内核。

## 主要符号

- `pub struct DDLOwnerInfoProvider { provider: Box<dyn Fn() -> bool + Send + Sync> }`：唯一有状态类型，持有堆分配的动态闭包。字段私有，调用者只能通过构造函数和 `call` 使用。
- `DDLOwnerInfoProvider::new<F>(provider: F) -> Self`：公开泛型构造函数。`F` 必须满足 `Fn() -> bool + Send + Sync + 'static`，因此闭包不能借用短生命周期局部值，并可随 Provider 在线程间转移及共享。
- `DDLOwnerInfoProvider::call(&self) -> bool`：公开的无错误调用入口；每次读取都重新执行闭包，不缓存结果。
- `impl exprctx::OptionalEvalPropProvider for DDLOwnerInfoProvider`：`Desc` 返回 `OptPropDDLOwnerInfo.Desc()`；`as_any` 返回 `Some(self)`，供 `Any::downcast_ref` 做类型校验。
- `pub struct DDLOwnerPropReader`：无字段零尺寸类型，不拥有上下文或 Provider。
- `impl RequireOptionalEvalProps for DDLOwnerPropReader`：`required_optional_eval_props` 返回 `OptPropDDLOwnerInfo.AsPropKeySet()`，即仅包含该属性键的位集合。
- `DDLOwnerPropReader::is_ddl_owner<C>(&self, ctx: &C) -> anyhow::Result<bool>`：公开读取入口，接受任何实现 `OptionalEvalPropContext` 的上下文，包括通过 blanket impl 兼容的完整 `exprctx::EvalContext` 和测试用窄上下文。

文件没有模块级常量、枚举、类型别名、宏、异步函数或 `cfg` 条件项。

## 执行流程

生产侧已验证的 Provider 装配流程如下：

1. `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 创建 `OptionalEvalPropProviders` 注册表。
2. 它克隆会话 `Arc` 为 `ddl_session`，构造 `DDLOwnerInfoProvider::new(move || ddl_session.is_ddl_owner())`。
3. `set_optional_prop` 根据 Provider 的 `Desc().Key()` 把它放入固定槽位，并拒绝重复键；完整上下文创建结束时还断言属性集合为满集。
4. 闭包没有在注册时求值。以后调用 `DDLOwnerInfoProvider::call` 才委托会话的 `is_ddl_owner()`，所以结果可反映会话底层状态的后续变化。

Reader 流程如下：

1. 表达式/调用者可先用 `required_optional_eval_props` 声明需要 `OptPropDDLOwnerInfo`。
2. `is_ddl_owner` 调用 `optional.rs::get_prop_provider::<DDLOwnerInfoProvider, _>`。
3. 公共函数先按键从 `OptionalEvalPropContext` 取 Provider，再校验 Provider 自描述键与请求键一致，最后经 `as_any().downcast_ref` 校验具体 Rust 类型。
4. 查找成功后，Reader 调用 `provider.call()` 并以 `Ok(bool)` 返回即时结果；查找或类型校验失败时在调用闭包前返回错误。

Go 的完整表达式流程还包括 `builtinTiDBIsDDLOwnerSig.RequiredOptionalEvalProps` 和 `evalInt` 调用 Reader，再把布尔值转为 SQL 整数。当前 Rust 文件及调用图只证明 Provider 装配与 Reader 本身可用，未证明这段 Reader 已由 Rust 对应 builtin 签名调用。

## 数据与状态

本文件传递的业务数据只有一个 `bool`：`true` 表示当前节点是 DDL Owner，`false` 表示不是。Provider 自身不保存该布尔值，而是保存生成它的闭包；因此状态所有权和一致性策略属于闭包捕获对象。生产闭包捕获 `Arc<C>` 会话上下文，测试则用 `Arc<AtomicBool>` 验证状态改变后 Provider 与 Reader 都能看到新值。

属性身份由 `exprctx::OptPropDDLOwnerInfo` 决定。`required_optional_eval_props` 把它转换为单比特集合，Provider 的 `Desc` 返回同一键的静态描述。注册表 `OptionalEvalPropProviders` 位于相邻 `optional.rs`，内部是按 `OPT_PROPS_CNT` 预分配的 `Vec<Option<Box<dyn OptionalEvalPropProvider>>>`；本文件不直接管理该容器。

Reader 是零尺寸、无状态类型，可按需临时构造。Provider 不实现本文件自定义的 Clone，也不暴露其闭包；若需要共享，通常由拥有它的求值上下文承担生命周期。

## 依赖与调用关系

上游与装配关系：

- `pkg/expression/expropt/lib.rs` 声明并再导出本模块。
- `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 是已找到的生产 Provider 构造者；它把 `SessionContext::is_ddl_owner` 包装为闭包并注册。
- `pkg/expression/sessionexpr/sessionctx.rs::EvalContext` 实现完整表达式上下文，公共 `OptionalEvalPropContext` blanket impl 将读取委托给 `GetOptionalPropProvider`。

下游依赖：

- `pkg/expression/exprctx/optional.rs` 提供 `OptionalEvalPropKey`、`OptionalEvalPropDesc`、`OptionalEvalPropProvider` 以及键 `OptPropDDLOwnerInfo`。
- `pkg/expression/expropt/optional.rs` 提供 `RequireOptionalEvalProps`、`OptionalEvalPropContext` 和 `get_prop_provider`；后者承担缺失、键错配和具体类型错配检查。
- `anyhow::Result` 承载 Reader 的查找/转型错误。
- 闭包 trait 对象依赖标准库 `Fn`、`Send`、`Sync` 和 `Any`。

RustCodeGraph 对目标文件给出的引用文件包括 `pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/expropt/optional_test.rs`、`pkg/expression/expropt/migration_aster_unit_test.rs` 和 sessionexpr 迁移测试。精确探索显示 `is_ddl_owner` Reader 的调用者集中在测试；不要仅凭 Go 的调用链推断 Rust 已完成同样接线。

## 错误处理与边界

`DDLOwnerInfoProvider::call` 返回裸 `bool`，闭包签名不能表达失败，所以本文件不会吞掉或转换 Owner 查询错误。若未来底层查询需要失败语义，必须显式改变 Provider/Reader 的返回类型并同步 Go 兼容契约，而不能用 `false` 暗示错误。

`is_ddl_owner` 的错误全部来自 `get_prop_provider`：

- 未注册该键：错误包含 `not exists in EvalContext`；
- 上下文返回的 Provider 自描述键与请求键不一致：返回键错配错误；
- Provider 不能向下转型为 `DDLOwnerInfoProvider`：返回包含目标类型和键的转换错误。

这些错误通过 `?` 原样向上传播，只有成功取到正确类型后才执行闭包。公共注册表的重复键、越界键等约束在 `optional.rs` 的注册阶段处理，不在本文件重复检查。

本文件不负责把布尔值映射为 SQL 的 `0/1`；该转换目前位于 `pkg/expression/builtin_info.rs::tidb_is_ddl_owner`。也不负责 NULL 语义，因为成功读取总会得到非空布尔值。

## 并发与资源生命周期

闭包被要求同时满足 `Send + Sync + 'static`：Provider 可以安全地随拥有它的上下文跨线程移动或被共享，且不会悬垂借用。这个约束只保证闭包对象满足 Rust 的并发类型规则，不自动保证其捕获状态具有某种业务一致性；捕获可变状态时仍应采用 `Atomic*`、锁或其他同步原语。

生产装配通过 `Arc` 捕获会话，Provider 会延长该会话对象至少到求值上下文/注册表释放为止。注册表持有 `Box<dyn OptionalEvalPropProvider>`，释放上下文时 Box、闭包及其捕获的 `Arc` 引用一并正常析构。本文件不启动任务、线程、计时器或通道，也没有显式关闭流程。

每次 `call` 都同步执行闭包，无内部锁、缓存、重试或 I/O 调度。`pkg/expression/expropt/optional_test.rs::verify_ddl_owner` 使用 `Arc<AtomicBool>` 与 `SeqCst` 顺序验证 Provider/Reader 能观测动态切换；这证明测试夹具的可见性，不代表本文件强制所有调用者采用 `SeqCst`。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/expropt/ddlowner.go`：

- Go `type DDLOwnerInfoProvider func() bool` 对应 Rust 内含 `Box<dyn Fn() -> bool + Send + Sync>` 的结构体。Rust 增加 `new`/`call` 方法以及线程安全和 `'static` 约束，以适配 trait object 与所有权模型。
- Go `Desc()` 与 Rust `OptionalEvalPropProvider::Desc` 都返回 `OptPropDDLOwnerInfo.Desc()`。
- Go 空结构体 `DDLOwnerPropReader{}` 对应 Rust 零尺寸 `DDLOwnerPropReader`。
- Go `RequiredOptionalEvalProps` 与 Rust `required_optional_eval_props` 都返回该键的单比特集合。
- Go `IsDDLOwner` 先取具体 Provider 再执行函数；Rust `is_ddl_owner` 保留此顺序，并用 `Any` 安全 downcast 代替 Go 类型断言。

Go 测试 `pkg/expression/expropt/optional_test.go::TestOptionalEvalPropProviders` 验证缺失时报错、闭包状态切换以及直连 Provider/Reader 结果一致；Rust 的 `pkg/expression/expropt/optional_test.rs::verify_ddl_owner` 对齐这些行为。Go `pkg/expression/sessionexpr/sessionctx.go` 注册 `sctx.IsDDLOwner`，Rust `NewEvalContext` 注册 `ddl_session.is_ddl_owner()`，二者都是延迟调用而非构造时快照。

迁移差异必须保留：Go `pkg/expression/builtin_info.go::builtinTiDBIsDDLOwnerSig` 通过嵌入/调用 Reader 声明并读取属性；现有 Rust `pkg/expression/builtin_info.rs::tidb_is_ddl_owner` 只把传入布尔值转为整数。静态证据不足以声称 Rust Reader 已经接入 SQL builtin 的生产求值路径。

## 扩展指南

- 若只改变 DDL Owner 状态的来源，应优先修改 `pkg/expression/sessionexpr/sessionctx.rs::NewEvalContext` 中构造闭包的位置，保持本文件的键和 Reader 契约不变，并同步 sessionexpr 的独立测试。
- 若要让新的 Rust 表达式读取状态，应组合/实现 `RequireOptionalEvalProps` 并声明 `OptPropDDLOwnerInfo`，在求值时调用 `DDLOwnerPropReader::is_ddl_owner`；同时验证调用链确实把所需 Provider 装入上下文。
- 若要完成 Rust `TIDB_IS_DDL_OWNER()` 的生产接线，应对照 Go `builtinTiDBIsDDLOwnerSig`，在对应独立 Rust builtin 签名/求值层读取 Reader，而不是把 DDL Owner 管理器硬编码进表达式内核。需同步标量、向量、类型推导和集成测试，但不应在本文件内嵌测试。
- 若修改属性键或描述符，必须同步 `pkg/expression/exprctx/optional.rs`、Go 同路径定义、注册表顺序与键集合测试；键是固定槽位协议，随意重排会造成 Provider 类型错配。
- 若把 Provider 改为可失败调用，需同步 `DDLOwnerInfoProvider::call`、Reader 返回语义、Go 对照以及所有构造者和测试，并明确区分“不是 Owner”和“查询失败”。
- 并发扩展不得移除 `Send + Sync`，除非同时证明所有拥有者与调用路径都不再跨线程；高频求值还应评估闭包调用成本，但不要在这里缓存可能变化的 Owner 状态。

应优先扩展的独立测试文件是 `pkg/expression/expropt/optional_test.rs`（注册表、动态真假值）、`pkg/expression/expropt/migration_aster_unit_test.rs`（缺失和 Go 对齐）、`pkg/expression/sessionexpr/sessionctx_test.rs` 与 `migration_aster_unit_test.rs`（真实装配）。生产 builtin 接线变化还需覆盖 `pkg/expression/builtin_info_test.rs`、`builtin_info_vec_test.rs` 及对应集成路径。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边，目标文件已索引。
- RustCodeGraph `node --file pkg/expression/expropt/ddlowner.rs`：核对了文件全部 75 行、两个公开结构体、构造/调用方法、两个 trait impl 和 Reader 错误传播。
- RustCodeGraph `explore "tidb_is_ddl_owner DDLOwnerPropReader is_ddl_owner pkg/expression/builtin_info.rs"`：确认 Provider 的生产装配位置、Reader 的直接 Rust 调用者主要为测试，并区分当前 Rust bool 转换内核与 Go Reader 接线。
- 已读生产文件：`pkg/expression/expropt/lib.rs`、`pkg/expression/expropt/optional.rs`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/expression/builtin_info.rs`、`pkg/expression/exprctx/optional.rs`。
- 已读边界声明：`pkg/expression/expropt/Cargo.toml`。
- 已读 Go 对照：`pkg/expression/expropt/ddlowner.go`、`pkg/expression/expropt/optional_test.go`、`pkg/expression/sessionexpr/sessionctx.go`、`pkg/expression/sessionexpr/sessionctx_test.go`、`pkg/expression/builtin_info.go`。
- 已读独立 Rust 测试：`pkg/expression/expropt/optional_test.rs`、`pkg/expression/expropt/migration_aster_unit_test.rs`、`pkg/expression/sessionexpr/sessionctx_test.rs`、`pkg/expression/sessionexpr/migration_aster_unit_test.rs`；它们覆盖未注册错误、键声明、动态真假值、Provider/Reader 一致性和生产式上下文装配。
- 本任务是只新增说明文档的静态分析，按计划未运行 Cargo。结构验证命令及退出码在任务交付时记录。
