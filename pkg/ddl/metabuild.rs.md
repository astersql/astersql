# `pkg/ddl/metabuild.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；`pkg/ddl/Cargo.toml` 将库入口设为 `lib.rs`，`pkg/ddl/lib.rs` 通过 `pub mod metabuild` 公开该模块，并仅在 `#[cfg(test)]` 下挂载独立测试 `metabuild_test.rs`。它的设计位置是 DDL 语句进入表、索引等元数据构造逻辑之前的适配层：把会话侧默认值与调用方显式选项合并成一次元数据构建所需的上下文。

当前 Rust 事实必须与这一设计位置区分开：全仓精确引用搜索和 RustCodeGraph 的调用轨迹只发现 `pkg/ddl/metabuild_test.rs` 调用 `new_meta_build_context_with_session`，没有 Rust 生产调用者。Rust 文件还定义了自己的字符串化 `SessionBuildContext`、`MetaBuildContext` 和 `BuildOption`，没有调用同 workspace 中更完整的 `astersql-meta-metabuild` 实现。因此它目前是可独立验证的会话默认值合并模型，不是已经接入 Rust DDL 生产主链的适配器。

从 DDL 框架角度看，本文件既不创建持久 DDL job，也不推进 schema state，不做 reorg/backfill，不更新 schema version；它只是构建进程内参数快照。对应 Go 入口会在创建/修改表、视图、索引和 schema tracker 等路径中被调用，但这些 Go 调用边不能作为 Rust 已接线的证据。

## 核心职责

1. 用 `SessionBuildContext` 表达构建上下文所需的最小会话输入：表达式上下文标识、最新 InfoSchema 标识和一组会话变量快照。
2. 在 `new_meta_build_context_with_session` 中把会话默认值复制到新的 `MetaBuildContext`，并落实“受限 SQL 不强制主键”的特殊规则。
3. 按迭代顺序应用 `BuildOption`；同一字段出现多次时后一个值覆盖前一个值，也允许显式选项覆盖会话默认值。
4. 为每次构建初始化独立的空 `warnings`，并把 `suppress_too_long_index_error` 的无选项默认值设为 `false`。
5. 用断言拒绝空的表达式上下文标识，避免构造缺少必要表达式环境的上下文。

该文件不校验 `shard_row_id_bits`、`pre_split_regions` 的业务上限，也不消费上下文来构造 `TableInfo`/`IndexInfo`；这些职责应由真实会话适配层、变量校验层和下游 metabuild 实现承担。

## 主要符号

- `ClusteredIndexMode::{IntOnly, Off, On}`：聚簇索引模式的三值枚举；派生 `Default`，默认分支为 `IntOnly`。它是本文件的简化类型，并非直接复用 `sessionctx/vardef` 的 `ClusteredIndexDefMode`。
- `SessionVariables`：六个会话字段的拥有型快照，包括生成列自增开关、主键要求、受限 SQL 标志、聚簇索引模式、row ID 分片位数和预分裂 Region 数。
- `SessionBuildContext`：函数的会话输入。`expression_context: String` 和 `latest_info_schema: String` 只是标识文本，不具备 Go 接口或 Rust trait object 的行为。
- `BuildOption`：七种覆盖项。除了六类会话派生设置，还提供 `InfoSchema(String)` 和 `SuppressTooLongIndexError(bool)`；枚举不包含 warnings 覆盖项。
- `MetaBuildContext`：最终拥有型结果，包含八项配置及 `warnings: Vec<String>`。所有字段公开，调用者可以在构造后直接修改，因此类型自身不维护封装不变量。
- `new_meta_build_context_with_session(&SessionBuildContext, impl IntoIterator<Item = BuildOption>) -> MetaBuildContext`：唯一函数和公开构造入口。泛型迭代器允许数组、向量等按顺序提供选项；函数无 `Result` 返回。

文件没有 trait、`impl`、模块级可变状态、条件编译项、异步函数或私有辅助函数。

## 执行流程

`new_meta_build_context_with_session` 的执行顺序如下：

1. 断言 `session.expression_context` 非空；失败时以固定消息 `session expression context must not be empty` panic，且尚未创建结果或产生外部副作用。
2. 借用 `session.variables`，随后克隆表达式上下文和最新 InfoSchema 字符串，复制所有标量会话值。
3. 计算 `primary_key_required = !in_restricted_sql && primary_key_required`。因此普通 SQL 继承会话设置，受限 SQL 无条件得到 `false`。
4. 将 `suppress_too_long_index_error` 初始化为 `false`，将 `warnings` 初始化为新的空向量；二者都不从 `SessionVariables` 读取。
5. 按 `other_options` 的迭代顺序逐项匹配七个 `BuildOption` 变体，并原地覆盖对应字段。函数不去重、不排序，也不因值与默认值相同而跳过。
6. 返回完整拥有的 `MetaBuildContext`。

独立测试 `caller_options_follow_session_defaults_and_later_options_win` 以两次 `ShardRowIdBits` 覆盖固定了“后者胜出”；`restricted_sql_disables_primary_key_requirement` 固定了受限 SQL 规则；`copies_every_session_derived_field` 固定了初始复制、空 warnings 和 suppress 默认值。

## 数据与状态

所有输入与结果都在单次同步调用内处理，没有全局状态或持久状态。`SessionBuildContext` 被共享借用，函数不会修改会话；输出通过克隆两个字符串与复制标量取得独立所有权。选项中的 `InfoSchema(String)` 被移动到结果，其余选项是可复制标量。

覆盖不变量是“先会话默认值，后调用方选项，且选项从左到右生效”。但调用方可以用 `BuildOption::PrimaryKeyRequired(true)` 覆盖受限 SQL 计算出的 `false`；这与 Go 把 `otherOpts` 追加到 session-derived options 后面的顺序一致，也意味着“受限 SQL 不强制主键”只是一项默认策略，不是最终不可突破的安全约束。

`warnings` 在这里是普通 `Vec<String>`，构造时永远为空，文件内没有追加接口；它与 Go `metabuild.Context` 通过表达式求值上下文共享 statement warnings 的机制并不等价。类似地，字符串形式的 `expression_context` 和 `info_schema` 不能提供 SQL mode、collation、warning sink 或跨表约束查询能力。

数值字段使用 `u64`，所以没有负数状态，但没有检查最大 shard bits、预分裂数或二者之间的关系。`ClusteredIndexMode` 的类型系统限制结果只能是三个已知模式。

## 依赖与调用关系

crate 与模块关系为 `pkg/ddl/Cargo.toml` → `[lib] path = "lib.rs"` → `pkg/ddl/lib.rs::pub mod metabuild` → 本文件。`pkg/ddl/lib.rs` 在测试配置下另外声明 `mod metabuild_test`，保持生产逻辑与测试分文件。

RustCodeGraph 能定位本文件的 `ClusteredIndexMode`、`BuildOption`、`MetaBuildContext` 和 `new_meta_build_context_with_session`。函数节点的调用轨迹只有 `pkg/ddl/metabuild_test.rs` 中四个普通测试和一个 `should_panic` 测试；`callees` 查询返回无下游调用。仓库精确搜索也只找到该测试文件引用本文件 API，故当前没有可确认的 Rust 生产上游。

函数内部只依赖 Rust 标准语言能力（`String`、`Vec`、派生 trait、`IntoIterator`、`assert!` 和模式匹配），没有直接使用 `pkg/ddl/Cargo.toml` 声明的外部 crate。值得注意的是 manifest 已声明路径依赖 `astersql-meta-metabuild = ../meta/metabuild`，而本文件没有使用它；后者的 `pkg/meta/metabuild/context.rs::Context` 已实现真实 `ExprContextRef`、可选 `InfoSchemaRef`、option trait 和 warning/note 转发，说明当前 DDL 文件与 workspace metabuild 类型仍是两套接口。

Go 生产上游由 `pkg/ddl/metabuild.go::NewMetaBuildContextWithSctx` 承接；精确搜索显示其被 `executor.go`、`add_column.go`、`modify_column.go`、`materialized_view.go`、`mock.go` 和 `schematracker/dm_tracker.go` 等调用。这些边仅用于说明期望位置和迁移差距，不代表 Rust 本函数已被上述路径调用。

## 错误处理与边界

本函数没有可恢复错误类型。唯一显式失败是空 `expression_context` 导致 panic；`pkg/ddl/metabuild_test.rs::rejects_missing_expression_context` 验证了准确消息。由于参数是 `&SessionBuildContext` 而不是 `Option`，整个 session 本身不能为 null，但其两个字符串都可以为空；代码只拒绝表达式字符串为空，不拒绝空 InfoSchema 标识。

所有 `BuildOption` 都无条件接受输入。函数不会验证数值范围、InfoSchema 是否真实存在、表达式上下文标识是否可解析，也不会限制重复选项。后出现的 `PrimaryKeyRequired(true)` 可以重新打开受限 SQL 默认关闭的要求；调用方必须把覆盖顺序视作 API 契约。

构造过程没有 I/O 或可失败的下游调用，因此除内存分配 panic 外不存在部分成功状态。`warnings` 不承载结构化错误级别，`suppress_too_long_index_error` 也只记录布尔策略；本文件本身不会捕获索引过长错误或把它转成 warning。

与 Go 边界相比，Go `NewMetaBuildContextWithSctx` 使用 `intest.AssertNotNil` 检查 session 与 session vars，`WithExprCtx` 也要求非空真实接口；Rust 简化函数只检查非空字符串。不能把这个断言视为已覆盖 Go 的接口有效性、SQL mode 或 InfoSchema 能力检查。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务、owner lease 或持久资源。函数只对输入做不可变借用，并返回拥有型结果，因此同一 `SessionBuildContext` 可以被多个调用者并发读取；是否安全共享取决于外层如何提供该值，本类型自身没有内部可变性。

两个字符串在每次构造时克隆，warnings 向量每次新建；返回值之间不共享这些缓冲区。函数结束后对 session 的借用释放，结果由调用者独立管理，drop 时由 Rust 自动释放。选项迭代器在调用期间被同步消费，不会被保存。

真实 Go/Rust `pkg/meta/metabuild` 上下文使用共享表达式上下文和 InfoSchema 接口，可能间接关联 warning sink 或元数据快照生命周期；本文件的字符串模型没有这些共享资源语义。将其接入生产链时不能仅替换类型名，必须明确 `Arc` 所有权、InfoSchema 快照有效期以及 warnings 应写入哪个 statement context。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/ddl/metabuild.go`。命名对应为 Rust `new_meta_build_context_with_session` ↔ Go `NewMetaBuildContextWithSctx`，Rust `SessionVariables` 字段 ↔ `sctx.GetSessionVars()` 中的同名设置，Rust `BuildOption` ↔ `pkg/meta/metabuild.Option`。两边已对齐的局部规则包括：从 session 获取表达式环境和最新 InfoSchema；复制生成列自增、聚簇索引、shard bits、pre-split regions；以 `!InRestrictedSQL && PrimaryKeyRequired` 计算默认主键要求；把额外选项追加在 session-derived 默认选项之后，使后面的设置覆盖前面的设置；suppress-too-long-index-error 无额外选项时为 false。

`pkg/ddl/metabuild_test.go::TestNewMetaBuildContextWithSctx` 还验证了真实 Go 上下文保持表达式对象身份、SQL mode、UTF8MB4 默认 collation、warning/note sink 和 InfoSchema 对象身份。Rust `pkg/ddl/metabuild_test.rs` 只验证字符串值、标量复制、三种聚簇模式、选项顺序和空字符串 panic；没有等价验证真实表达式或 InfoSchema 行为。

主要迁移差异如下：

- Go 返回 `*pkg/meta/metabuild.Context`；Rust 返回本文件自定义 `MetaBuildContext`，没有复用已存在的 `astersql_meta_metabuild::Context`。
- Go 表达式上下文与 InfoSchema 是可调用接口对象；Rust 此处是 `String`。Go warnings/notes 写入表达式 eval context；Rust 此处只有未接线的 `Vec<String>`。
- Go option 是对完整 context 应用的可扩展行为对象；Rust option 是封闭枚举，新增选项必须同时修改枚举与 match。
- Go 入口已有广泛生产调用；Rust 入口目前只有独立单元测试调用。

所以本文件证明的是默认值合并规则的局部移植，不证明 DDL metabuild 适配层已经完整生产化。

## 扩展指南

若目标是接入真实 Rust DDL 主链，优先考虑让会话适配器直接构造 `astersql_meta_metabuild::Context`，复用其 `WithExprCtx`、`WithInfoSchema` 和其他 options；不要继续扩张字符串占位类型形成第三套上下文。接线点应位于 Rust DDL 语句转换成 TableInfo/IndexInfo 之前，并以精确生产调用搜索证明入口真实可达。

若仍扩展当前模型：新增会话派生字段时必须同步 `SessionVariables`、`MetaBuildContext`、初始化结构体、相应 `BuildOption` 与 match 分支；仅加字段而漏掉任一处会造成默认值或覆盖语义漂移。新增不可由调用方突破的约束时，应在应用完 options 后重新校验，而不是只在 session 默认值阶段计算。

测试必须继续放在独立的 `pkg/ddl/metabuild_test.rs`。至少覆盖：新字段从 session 复制、显式 option 覆盖、重复 option 后者胜出、受限 SQL 与显式覆盖的预期关系、无效输入的准确失败行为。若切换到真实 `astersql-meta-metabuild` 类型，还应增加对象身份、SQL mode/collation、warning/note 转发和可选 InfoSchema 的测试，并参考 `pkg/meta/metabuild/context_test.rs`。

兼容风险集中在公开结构体字段和 `BuildOption` 枚举变体的变化；正确性风险集中在 option 顺序、受限 SQL 主键策略以及将字符串占位误当成真实上下文；性能风险较小，主要是每次构造克隆字符串和分配 warnings 向量。接入共享 trait object 后则需额外评估 `Arc` 克隆和快照生命周期，而不是复制 Go 的指针假设。

## 验证依据

- 目标源码：`pkg/ddl/metabuild.rs`，核对了全部 151 行、三个公开结构体、两个公开枚举、唯一函数、断言和全部 option 分支。
- crate 与模块：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`、根门面 `pkg/lib.rs`；确认 crate 为 `astersql-ddl`、模块公开、测试独立挂载，以及 manifest 已声明但本文件未使用 `astersql-meta-metabuild`。
- Rust 独立测试：`pkg/ddl/metabuild_test.rs`；覆盖所有 session-derived 字段、受限 SQL、全部 option、重复 option、三种聚簇模式和空表达式上下文 panic。
- Rust 真实 metabuild 对照：`pkg/meta/metabuild/context.rs` 与 `pkg/meta/metabuild/context_test.rs`；用于确认 workspace 已有真实表达式/InfoSchema 接口和左到右 option 语义。
- Go 对照：`pkg/ddl/metabuild.go`、`pkg/ddl/metabuild_test.go`、`pkg/meta/metabuild/context.go`；并通过精确引用搜索核对 Go 生产调用分布。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/ddl/metabuild.rs` 读取完整文件；`query` 唯一定位 `new_meta_build_context_with_session`、`MetaBuildContext` 和 `ClusteredIndexMode`；函数节点列出的调用者均在 `metabuild_test.rs`，`callees` 返回无调用。
- 仓库搜索交叉验证：Rust 非测试源码没有引用本文件公开 API；Go 非测试源码中同名入口被多个 DDL 构造路径调用。因此人工结论是：该文件存在于会话配置到元数据构建上下文的适配位置，但当前 Rust 仅实现并测试了简化合并规则；安全扩展应优先复用真实 metabuild context 并补齐生产接线证据。
