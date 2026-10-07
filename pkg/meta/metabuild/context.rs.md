# `pkg/meta/metabuild/context.rs`

## 文件定位

本文件实现 `astersql-meta-metabuild` crate 的元数据构建上下文。crate 根模块 `pkg/meta/metabuild/lib.rs` 将私有的 `context` 模块整体 `pub use`，所以这里的 `Context`、`NewContext`、`NewNonStrictContext` 和全部 `With*` 构造器构成该 crate 的公开 API。它位于解析完成的 `CREATE TABLE` AST 与 `TableInfo`/`IndexInfo` 等元数据对象之间：调用方先把会话或工具侧配置装入 `Context`，DDL 构建代码再通过只读访问器决定字符集、聚簇主键、主键约束和预分裂参数。

Rust 生产接线的直接证据包括 `pkg/session/runtime/ddl.rs`：普通 `CREATE TABLE` 路径用 `NewContext` 注入 `WithClusteredIndexDefMode`、`WithShardRowIDBits`、`WithPreSplitRegions`，随后调用 `astersql_ddl::BuildTableInfoFromAST`。`pkg/ddl/create_table.rs` 接收 `&metabuild::Context<C, E>`，并读取其中多个配置。其他直接创建默认上下文的生产位置包括 `pkg/session/fts_runtime.rs`、`pkg/session/runtime/mview_ddl.rs`、`pkg/session/runtime/session.rs`、`pkg/session/ddl_tables.rs` 和 `pkg/importsdk/file_scanner.rs`。

## 核心职责

1. 用 `Context<C, E>` 汇集元数据构建所需的表达式环境、会话开关、表布局默认值以及可选的只读 InfoSchema。
2. 用 `Option<C, E>` 与 `FuncCtxOption` 实现和 Go functional-option 模式一致的按序覆盖；`NewContext` 从左到右应用选项，因此同一字段以后出现的选项为准。
3. 在调用方未传表达式上下文时构造静态默认 `ExprContext`，保证通过 `NewContext` 得到的对象可安全执行表达式相关访问。
4. 将 SQL mode、默认 utf8mb4 排序规则、warning/note 写入等行为转发到表达式求值上下文，而不在本文件重复实现这些规则。
5. 提供 `NewNonStrictContext`，显式以 `mysql::ModeNone` 构造用于宽松元数据解析/比较的上下文。

本文件只保存构建策略，不直接解析 SQL，也不创建 `TableInfo`。实际元数据构建与校验位于 `pkg/ddl/create_table.rs` 等下游模块。

## 主要符号

- `ExprContextRef = Arc<dyn exprctx::ExprContext>`：共享表达式上下文 trait object。`Arc` 允许选项和最终上下文共享对象，并使 `WithExprCtx` 的参数在类型层面不可为 `nil`。
- `InfoSchemaRef<C, E> = Arc<dyn infoschemactx::MetaOnlyInfoSchema<Context = C, Error = E>>`：只读元数据目录 trait object。泛型 `C`、`E`保留 `MetaOnlyInfoSchema` 的关联上下文和错误类型；本文件不绑定具体 InfoSchema 实现。
- `Option<C, E>`：公开选项 trait，唯一方法 `apply_ctx(&self, &mut Context<C, E>)`。自定义实现可以修改上下文，但通常应使用本文件的 `With*` 构造器。
- `FuncCtxOption<C, E>`、`func_opt`：私有闭包适配层，把 `Fn(&mut Context<C, E>)` 装箱为 `Box<dyn Option<C, E>>`。
- 八个选项构造器：`WithExprCtx`、`WithEnableAutoIncrementInGenerated`、`WithPrimaryKeyRequired`、`WithClusteredIndexDefMode`、`WithShardRowIDBits`、`WithPreSplitRegions`、`WithSuppressTooLongIndexErr`、`WithInfoSchema`。每个只覆盖一个字段；`WithInfoSchema` 接受 `Option<InfoSchemaRef<C, E>>`，可显式设置或清空目录引用。
- `Context<C = (), E = Infallible>`：包含 `expr_ctx`、`enable_auto_increment_in_generated`、`primary_key_required`、`clustered_index_def_mode`、`shard_row_id_bits`、`pre_split_regions`、`suppress_too_long_index_err`、`info_schema` 八项状态。字段均私有，只能经选项构造及访问器使用。
- `NewContext`：主构造入口，建立默认值、按序应用选项、补齐表达式上下文。
- `NewNonStrictContext`：以 `mysql::ModeNone` 构造 `EvalContext` 和 `ExprContext`，再委托 `NewContext`。
- `Context` 访问器：`GetExprCtx`、`GetDefaultCollationForUTF8MB4`、`GetSQLMode`、`AppendWarning`、`AppendNote`、`EnableAutoIncrementInGenerated`、`PrimaryKeyRequired`、`GetClusteredIndexDefMode`、`GetShardRowIDBits`、`GetPreSplitRegions`、`SuppressTooLongIndexErr`、`GetInfoSchema`。

## 执行流程

`NewContext(opts)` 的顺序是本文件最重要的不变量：

1. 先令 `expr_ctx` 和 `info_schema` 为空；布尔开关采用会话兼容默认值；聚簇索引模式取 `vardef::DefTiDBEnableClusteredIndex`；行 ID 分片位数和预分裂数由相应 `vardef` 常量转换为 `u64`。
2. 按 `Vec` 顺序对每个 `Option` 调用 `apply_ctx`。选项没有合并逻辑，所以同一字段被多次写入时最后一次生效；`context_options_override_in_declaration_order` 和 `options_override_in_order_and_preserve_shared_interfaces` 对此有直接断言。
3. 仅当选项应用后 `expr_ctx` 仍为空时，调用 `exprstatic::NewExprContext(Vec::new())` 安装静态默认上下文。因而显式 `WithExprCtx` 不会被默认值覆盖。
4. 返回按值持有状态的 `Context`。调用方通常以共享引用传给 DDL 构建函数。

`NewNonStrictContext()` 先通过 `exprstatic::NewEvalContext([WithSQLMode(ModeNone)])` 建立非严格求值环境，再用 `WithEvalCtx` 嵌入新的表达式上下文，最后以 `WithExprCtx` 进入上述通用构造流程。`pkg/util/schemacmp/table_test.rs` 的 Rust 测试辅助代码直接使用该入口；Go 生产调用证据见 `pkg/infoschema/perfschema/init.go`。

下游 DDL 流程在 `pkg/ddl/create_table.rs` 中表现为：`BuildTableInfoFromAST` 进入带校验的构建流程；`GetClusteredIndexDefMode` 决定未显式指定时是否使用聚簇主键；`GetDefaultCollationForUTF8MB4` 补齐 utf8mb4 默认排序规则；`PrimaryKeyRequired` 执行主键必需性检查；`GetShardRowIDBits` 与 `GetPreSplitRegions` 填入表布局并把预分裂数限制到可用分片位数以内；`GetExprCtx` 传给分区定义规范化逻辑。

## 数据与状态

- `expr_ctx` 在结构体内部使用 `Option` 只是为了表达构造过程中的“尚未初始化”；公开构造完成后必须为 `Some`。`GetExprCtx` 对违反该不变量的内部状态执行 `expect`。由于字段私有，crate 外调用方无法构造缺少表达式上下文的 `Context`。
- `enable_auto_increment_in_generated` 默认取 `vardef::DefTiDBEnableAutoIncrementInGenerated`；它表达生成列能否使用 `AUTO_INCREMENT`。当前目标文件与测试证明其存取语义，但精确 Rust 生产消费者未在直接引用搜索中发现，不能据此宣称对应 DDL 校验已经接线。
- `primary_key_required` 默认 `false`；`pkg/ddl/create_table.rs` 在构建和最终校验中读取它，缺少主键时返回错误。
- `clustered_index_def_mode` 使用 `vardef` 默认模式；下游根据显式主键类型、该模式以及主键是否为单整数列选择聚簇行为。
- `shard_row_id_bits`、`pre_split_regions` 默认值来自有符号的 `vardef` 常量，构造时经 `u64::try_from` 校验非负。下游只在表选项未覆盖等条件成立时使用默认值，并保证 `PreSplitRegions` 不超过可用分片位数。
- `suppress_too_long_index_err` 默认 `false`。Rust 直接引用证据目前只覆盖本文件和独立测试；Go 的 schema tracker 会显式设置它，但不可把 Go 接线当作 Rust 生产接线。
- `info_schema` 可为空；`GetInfoSchema` 同时返回克隆后的 `Option<Arc<...>>` 和 `is_some()`，维持 Go `(is, ok)` 的判定形状。`Arc::clone` 不复制目录内容，独立测试以 `Arc::ptr_eq` 验证同一实例被保留。

## 依赖与调用关系

crate 边界由 `pkg/meta/metabuild/Cargo.toml` 明确：本文件只依赖工作区内的 `astersql-util-context`、`astersql-expression-exprctx`、`astersql-expression-exprstatic`、`astersql-infoschema-context`、`astersql-parser-mysql` 和 `astersql-sessionctx-vardef`。`pkg/meta/metabuild/lib.rs` 用局部模块名再导出这些 crate，使本文件通过 `crate::{contextutil, exprctx, exprstatic, infoschemactx, mysql, vardef}` 访问它们；没有 feature 条件分支。

主要下游边如下：

- `NewContext -> Option::apply_ctx -> FuncCtxOption::f`：执行字段覆盖。
- `NewContext -> exprstatic::NewExprContext`：缺少显式表达式上下文时提供默认值。
- `NewNonStrictContext -> exprstatic::NewEvalContext/WithSQLMode -> exprstatic::NewExprContext/WithEvalCtx -> NewContext`：建立 SQL mode 为零的完整上下文。
- `GetDefaultCollationForUTF8MB4/GetSQLMode/AppendWarning/AppendNote -> GetExprCtx -> ExprContext::GetEvalCtx` 或表达式上下文对应方法：查询和诊断状态均由下游上下文拥有。
- `pkg/session/runtime/ddl.rs -> NewContext -> pkg/ddl/create_table.rs::BuildTableInfoFromAST`：当前 Rust CREATE TABLE 主接线之一。
- `pkg/ddl/create_table.rs -> Context` 的访问器：把上下文策略落实到表元数据字段和校验分支。

RustCodeGraph 已索引 `pkg/meta/metabuild/context.rs` 的 29 个符号，并能定位 Rust/Go 的同名 `NewContext`、`NewNonStrictContext`、`WithExprCtx`；但对这些精确 Rust 符号执行 `callers/callees` 未返回调用边。因此这里的跨文件接线结论使用精确 `rg` 引用和相邻源码复核，而没有把索引缺边解释为“无调用者”。

## 错误处理与边界

- `NewContext` 本身不返回 `Result`。唯一显式构造失败点是把 `vardef::DefShardRowIDBits`、`vardef::DefPreSplitRegions` 转成 `u64` 时的 `expect`；负默认常量会触发 panic，错误信息分别说明哪个默认值必须非负。这是编译进程序的配置不变量，不是用户 SQL 错误路径。
- `GetExprCtx` 在内部 `expr_ctx` 为空时 panic。正常公开构造流程会补齐它；风险主要来自未来在同一模块新增绕过 `NewContext` 的构造代码。
- `WithExprCtx` 与 Go 不同：Go 用 `intest.AssertNotNil` 做运行期非空断言，Rust 参数是非可空的 `Arc<dyn ExprContext>`，把该约束前移到类型系统。
- `WithInfoSchema(None)` 是合法操作，不是错误；`GetInfoSchema` 的布尔值必须与返回 `Option` 是否为 `Some` 一致。
- warning/note 不在本文件转换或吞掉；`AppendWarning`、`AppendNote` 原样把 `SharedError` 交给 EvalContext。独立测试确认写入后的级别分别为 `WarnLevelWarning` 和 `WarnLevelNote`。
- 本文件不验证 `shard_row_id_bits`、`pre_split_regions` 的业务上限，也不判断聚簇模式枚举值是否合法；这些约束由调用方、常量定义或下游 DDL 逻辑承担。

## 并发与资源生命周期

`Context` 自身没有锁、异步任务、通道或事务，也不进行 I/O。它是构建期间的同步配置对象；`NewContext` 完成后，下游主要通过 `&Context` 只读访问。

表达式上下文和 InfoSchema 使用 `Arc` 共享生命周期。`WithExprCtx` 在闭包被应用时克隆 `Arc`；`WithInfoSchema` 在每次应用时克隆 `Option<Arc<_>>`；`GetInfoSchema` 返回时再次克隆 `Arc`。这些操作只增加引用计数，不深拷贝状态。文件本身没有声明 `Send`/`Sync` 边界，是否可跨线程取决于 trait object 的具体约束与实现，本文没有证据把它描述为线程安全容器。warning/note 的内部同步语义也由具体 EvalContext 实现负责。

选项对象被 `NewContext` 按值消费，而其 `apply_ctx` 接受共享的 `&self`；闭包捕获的数据至少活到该选项应用完毕。构造结束后 `Vec<Box<dyn Option<...>>>` 被释放，已写入 `Context` 的标量或克隆后的 `Arc` 独立存续。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/metabuild/context.go`，公开概念与字段保持一一对应：Go `Option.applyCtx` 对应 Rust `Option::apply_ctx`；Go `funcCtxOption`/`funcOpt` 对应 Rust 私有闭包适配器；八个 `With*`、`Context` 八个状态以及全部访问器名称和主要语义均被保留。

关键语言映射如下：

- Go 返回 `*Context`，Rust 返回按值的 `Context<C, E>`，调用方在 DDL 边界借用它。
- Go interface 引用由 Rust `Arc<dyn Trait>` 表达；InfoSchema 的 Go `nil` 由 Rust `Option<Arc<...>>` 表达。
- Go `opts ...Option` 由 Rust `Vec<Box<dyn Option<C, E>>>` 表达，二者都保证声明顺序应用。
- Go 默认为空的 `exprCtx` 在选项应用后用 `exprstatic.NewExprContext()` 补齐；Rust采取相同顺序，以 `Option` 表达构造期空值。
- Go 默认整数可直接赋给 `uint64`；Rust 对工作区常量执行 `u64::try_from`，额外显式守卫非负不变量。
- Go `NewNonStrictContext` 的注释说明 `ModeNone` 用于接受诸如零日期的特殊值；Rust保持相同构造链和最终 SQL mode。

会话注入的 Go 依据位于 `pkg/ddl/metabuild.go::NewMetaBuildContextWithSctx`：它从 session context 注入表达式上下文、生成列自增开关、主键要求、聚簇模式、分片位、预分裂数和最新 InfoSchema，再把额外选项追加在末尾，使调用方覆盖基础值。当前 Rust `pkg/session/runtime/ddl.rs` 的 CREATE TABLE 路径只直接注入聚簇模式、分片位和预分裂数；因此“类型/选项已移植”与“所有 Go 会话接线均已移植”必须区分，后者不能由本文件证明。

## 扩展指南

新增元数据构建选项时，应保持以下同步面：

1. 在 `Context` 增加私有字段，并在 `NewContext` 写明与 Go/会话默认值一致的初始化；若源常量有符号而字段无符号，继续显式处理转换失败。
2. 增加单字段 `With*` 构造器和只读访问器；保持选项无隐式副作用，确保重复选项仍是“后者覆盖前者”。
3. 在 `pkg/meta/metabuild/context_test.rs` 的独立测试中补默认值、至少两个代表性覆盖值和顺序覆盖；若涉及 trait object，再验证共享身份与 `None` 语义。不要把 Rust 测试内嵌进生产源文件。
4. 在 `pkg/meta/metabuild/migration_aster_unit_test.rs` 补齐 Go 语义迁移覆盖，并同步检查 `pkg/meta/metabuild/context_test.go` 和 `context.go`，避免 Rust 与 Go 字段集合或默认值漂移。
5. 把值接入实际消费者，例如 `pkg/ddl/create_table.rs`；若来自真实 session，还需检查 Rust session 构造路径是否像 Go `pkg/ddl/metabuild.go` 一样注入。只增加字段和测试不代表生产功能已接线。
6. 若新增 crate 依赖，更新 `pkg/meta/metabuild/Cargo.toml` 和根工作区依赖接线；当前文件没有 feature 门控，除非确有多实现需求，不应仅为选项增加条件编译。

兼容性风险主要是默认值变化导致建表元数据漂移、选项顺序变化破坏调用方覆盖、Go/Rust 会话注入不对称，以及将 `Arc` 改为深拷贝后丢失共享身份。性能上本文件仅有线性应用选项和少量 `Arc` 引用计数操作；新增昂贵计算或 I/O 应留在下游构建阶段，而不是 getter 中。

## 验证依据

- 源实现：`pkg/meta/metabuild/context.rs`，核对了全部类型别名、trait、私有适配器、八个选项、`Context` 八个字段、两个构造器和十二个访问/转发方法；文件无条件编译项。
- crate 边界：`pkg/meta/metabuild/Cargo.toml`、`pkg/meta/metabuild/lib.rs`，确认 crate 名称、六个工作区内部依赖、公开再导出和两个独立 Rust 测试模块。
- Go 对照：`pkg/meta/metabuild/context.go`、`pkg/meta/metabuild/context_test.go`、`pkg/ddl/metabuild.go`，核对 functional options、默认值、nil/ok 语义、字段覆盖表和会话注入顺序。
- Rust 测试：`pkg/meta/metabuild/context_test.rs` 的 `context_defaults_match_session_defaults`、`context_options_override_in_declaration_order`；`pkg/meta/metabuild/migration_aster_unit_test.rs` 的 `defaults_and_forwarded_expression_behavior_match_go`、`options_override_in_order_and_preserve_shared_interfaces`、`warnings_notes_and_non_strict_context_match_go`。这些用例覆盖默认表达式行为、全部选项、覆盖顺序、`Arc` 身份、InfoSchema 清空、warning/note 和非严格 SQL mode。
- 生产调用与消费：`pkg/session/runtime/ddl.rs`、`pkg/session/fts_runtime.rs`、`pkg/session/runtime/mview_ddl.rs`、`pkg/session/runtime/session.rs`、`pkg/session/ddl_tables.rs`、`pkg/importsdk/file_scanner.rs`、`pkg/ddl/create_table.rs`；Cargo 使用方还包括 `pkg/session/Cargo.toml`、`pkg/ddl/Cargo.toml`、`pkg/importsdk/Cargo.toml` 等。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/meta/metabuild` 确认同目录 Rust/Go 源与测试；`node --file pkg/meta/metabuild/context.rs` 返回完整 273 行和 29 个符号；`query` 定位了 Rust/Go 的 `NewContext`、`NewNonStrictContext`、`WithExprCtx`。精确 `callers/callees` 无输出，跨文件关系据精确引用搜索与源码读取复核。
- 本任务只新增说明文档，按计划不运行 Cargo。交付前执行固定十一章节结构检查，并人工复核本文没有把 Go 接线或测试覆盖误写成 Rust 生产支持。
