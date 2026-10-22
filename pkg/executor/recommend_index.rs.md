# `pkg/executor/recommend_index.rs`

## 文件定位

本文件属于根工作区中的 `astersql-executor` crate。`pkg/executor/Cargo.toml` 以 `lib.rs` 为库入口，并直接依赖 `astersql-util-chunk`；`pkg/executor/lib.rs` 通过 `pub mod recommend_index` 公开本模块。文件实现 `RECOMMEND INDEX` 结果集执行阶段的 Rust 语义：将动作分派给索引顾问，并把顾问结果编码进 `Chunk`。

冻结版本以 `lib.rs` 导出本模块，并由 `recommend_index_test.rs` 实例化执行器及测试顾问；尚无已验证的 Rust SQL builder 到此入口的生产接线。Go 的完整调用链见 `pkg/executor/builder.go::buildRecommendIndex` 与 `recommend_index.go`。

## 核心职责

- `RecommendIndexExec::Next` 保证一次性执行：每次先清空输出 `Chunk`，首次调用根据 `action` 执行 `set`、`show`、`run` 或 `return`，后续调用直接返回成功且不再产生行。
- `set` 把 `options` 交给顾问持久化；`show` 输出选项名称、当前值和说明；`run`/`return` 拆分可选 SQL 文本、请求顾问生成建议，并输出八列推荐信息。
- `IndexAdvisor` 将会话上下文、选项存取、推荐算法和错误构造从执行器中抽象出去，使本文件不依赖某个具体索引顾问实现。
- `RecommendIndexResult` 是顾问到执行器的行级传输对象；执行器只负责列映射和生成 `CREATE INDEX` 文本，不负责候选索引搜索、代价估算或结果持久化。

`advise_id` 当前只保存在执行器状态中，`Next` 和 `showOptions` 均未读取它；这与同路径 Go 实现中的现状一致。`apply`、`ignore` 虽可由 parser AST 表达，但本文件没有对应分支，会走“不支持动作”错误。

## 主要符号

- `RecommendIndexResult`：公开、可克隆且可比较的推荐结果。字段依次承载数据库、表、索引名、索引列、已格式化的索引规模、原因和已序列化的受影响查询 JSON。这里没有 Go `IndexDetail`/`TopImpactedQueries` 的嵌套类型，格式化职责已前移到顾问实现。
- `IndexAdvisor`：公开 trait，关联类型 `Context`、`Option`、`Error` 分别定义会话状态、选项表示和统一错误类型。`Option` 必须实现 `Clone`，但本文件只按切片借用选项。
- `IndexAdvisor::set_options`：为 `set` 动作写入选项。
- `IndexAdvisor::all_options` 与 `get_options`：为 `show` 提供稳定的展示顺序、值映射和说明映射。
- `IndexAdvisor::advise_indexes<C>`：消费请求上下文和 SQL 列表，返回“推荐行 + 独立状态”。这种二元返回允许即使状态为错误也先保留部分结果。
- `IndexAdvisor::unsupported_action`、`empty_sqls`：由具体顾问构造与其错误体系兼容的错误，执行器不绑定错误库或固定文案。
- `RecommendIndexExec<A>`：拥有顾问、顾问上下文、动作、SQL、推荐编号、选项和一次性标志。全部字段公开，当前没有构造函数或不变量封装。
- `RecommendIndexExec::Next<C>`：主入口，保留 Go 命名以配合 `#![allow(non_snake_case)]`。
- `RecommendIndexExec::showOptions`：`show` 的内部流程，但因声明为 `pub`，crate 外也可直接调用。

文件导入 parser AST 的四个动作常量，没有自定义模块级常量、枚举、条件编译项或本地 trait 实现。

## 执行流程

`Next` 的确定顺序如下：

1. 无条件调用 `req.Reset()`，因此调用者传入的旧行会先被清除；即使执行器已经完成也同样如此。
2. 若 `done` 已为 `true`，立即返回 `Ok(())`。首次调用则先把 `done` 设为 `true`，再执行任何可能失败的操作，所以错误之后也不会自动重试。
3. 按动作分派：`set` 调用 `set_options` 后直接返回；`show` 调用 `showOptions` 后直接返回；`run` 继续并设置 `persist = true`；`return` 继续并设置 `persist = false`；其他字符串通过 `unsupported_action` 返回错误。动作匹配区分大小写。
4. `run`/`return` 仅在 `sql` 非空时按 ASCII 分号切分，每段用 `str::trim` 去除首尾空白并丢弃空段。非空原文若最终没有有效片段（例如 `";;;"` 或纯空白）则调用 `empty_sqls`。原文恰为 `""` 时则把空向量交给顾问，表示由顾问决定工作负载来源；这与 Go 行为一致。
5. 调用 `advise_indexes(ctx, &mut context, sqls, &options, persist)`。请求上下文 `ctx` 按值传入，顾问上下文被可变借用，选项只读借用。
6. 遍历所有返回结果，将索引列用逗号连接，依次向列 0..7 追加数据库、表、索引名、列列表、规模、原因、JSON 和 `CREATE INDEX {name} ON {table}({columns});`。
7. 所有推荐行写完后才返回顾问同时给出的 `Result`。因此错误和部分结果可以共存；调用层不能假设 `Err` 意味着 `Chunk` 为空。

`showOptions` 先复制 `all_options()`，再可变调用 `get_options()`，避免同时持有对顾问的不可变借用和可变借用。随后按顾问提供的名称顺序遍历：值映射中缺失的名称被跳过；说明缺失时输出空字符串。

## 数据与状态

`RecommendIndexExec` 拥有 `advisor` 和 `context`，意味着顾问状态与会话/执行状态和执行器生命周期绑定。`action`、`sql`、`options` 是一次执行的输入，`done` 是唯一的流程状态。由于字段公开，调用者可以绕过构造约束修改它们；安全接线应在首次 `Next` 前完成初始化，并避免在 `done` 后复用实例处理另一条命令。

`Chunk` 是列式输出缓冲区。`Next` 假定调用者已按动作准备足够且类型兼容的列：`show` 写三列，`run` 写八列；本文件不检查列数。每个 `AppendString` 按列追加一次，同一推荐的八次追加共同组成一行，前提是所有列在进入循环前行数一致。

`BTreeMap<String, String>` 用于选项值与说明，提供确定性键存储；实际展示顺序不依赖映射排序，而由 `all_options()` 的切片顺序决定。推荐结果则保持 `advise_indexes` 返回的向量顺序。

## 依赖与调用关系

- 模块入口：`pkg/executor/lib.rs` 的 `pub mod recommend_index` 暴露本文件；`pkg/executor/Cargo.toml` 声明 crate 名 `astersql-executor`，无专属 feature 门控。
- 直接外部依赖：`astersql_util_chunk::Chunk`，来自 Cargo 依赖 `astersql-util-chunk = { path = "../util/chunk" }`。本文件使用其 `Reset` 与 `AppendString` 接口。
- 标准库依赖：`std::collections::BTreeMap`，用于选项值与说明。
- 抽象下游：`IndexAdvisor` 的六组方法。由于仓库内尚无 Rust 实现，无法把这些 trait 调用进一步追踪到 `astersql-planner-indexadvisor`；Cargo 虽声明该依赖，但本文件没有直接导入它。
- Rust 上游：当前只验证到模块公开，没有生产调用者。RustCodeGraph 的调用图为空与 `rg` 的仓库搜索结果一致。
- Go 对照主链：`pkg/executor/builder.go::buildRecommendIndex` 从 `plannercore.RecommendIndexPlan` 构造 Go 执行器；执行框架调用 `pkg/executor/recommend_index.go::Next`；后者调用 `pkg/planner/indexadvisor` 的选项或推荐函数并写入 `chunk.Chunk`。

索引实际推荐算法、SQL 解析、优化器代价计算及选项存储属于 `pkg/planner/indexadvisor`，不在本文件职责内。相关 Rust 算法测试证明顾问子系统的一部分能力，但不证明本执行器已接线。

## 错误处理与边界

- `set_options`、`get_options` 的错误立即向上传播；`done` 已提前置位，下一次 `Next` 不会重试。
- 非 `set/show/run` 动作由顾问构造错误；因此错误文本和类型是实现方契约。parser 支持的 `apply`、`ignore` 在当前执行器仍属于此类。
- 仅当原始 `sql` 非空而切分后无有效 SQL 时产生 `empty_sqls`；空字符串本身不是错误，而是以空工作负载调用顾问。
- 本文件按分号机械拆分，不识别字符串字面量或注释内分号；该行为直接复刻 Go `strings.Split`，扩展时不能悄悄改成 SQL 感知解析而不更新兼容测试。
- Rust 结果中的 `top_impacted_queries_json` 已经是字符串，执行器不验证其是否为合法 JSON；与 Go 在 `Next` 中调用 `json.Marshal` 并可能失败不同，序列化错误必须由 Rust 顾问在返回结果前处理。
- `CREATE INDEX` 文本直接插入索引名、表名和列名，没有引用或转义；当前逻辑假设顾问只产生安全、规范的标识符。若未来接受外部构造的 `RecommendIndexResult`，这里是兼容性和安全审查点。
- `showOptions` 对缺失值采用“跳过整行”，对缺失说明采用空字符串。Go 版本的 `desc[opt]` 对缺失键也得到空字符串，语义相同。
- `AppendString` 的列边界与行对齐由 `Chunk`/调用者保证，本函数不恢复已写入的半行，也没有事务性回滚。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、文件、网络连接或显式事务。`Next` 与 `showOptions` 都要求 `&mut self` 和 `&mut Chunk`，Rust 借用规则阻止同一实例在安全代码中并发执行；是否可以在线程间移动或共享则取决于泛型 `A`、`A::Context`、`A::Option`、`A::Error` 和 `Chunk` 的自动 trait，本文件没有添加 `Send`/`Sync` 约束。

请求上下文 `C` 在 `run` 时按值转交顾问并在调用结束后释放；`set/show` 不使用它。顾问与上下文由执行器持有，随执行器销毁。输出 `Chunk` 由调用者持有，执行器只在本次调用中清空和填充。部分结果在顾问错误返回时仍留在 `Chunk` 中，这是刻意保留的生命周期语义，而不是回滚失败。

## 与 Go 版本的对应关系

冻结版本中 `RecommendIndexActionReturn` 对应 `RETURN RESULT`，与 `run` 共享分析、SQL 拆分和写行流程，但传给顾问的 `persist` 为 false；Go 对应 `AdviseIndexesWithoutPersist`，`run` 对应 `AdviseIndexes`。默认动作的选择属于 parser，上述执行器不把空动作默认为 return；持久化是否真正被抑制仍依赖具体顾问遵守此契约。

`pkg/executor/recommend_index.go` 是逐项语义基准：两者都先重置结果、用 `done` 保证只执行一次、支持 `set/show/run/return`、按分号拆分 SQL、过滤空片段、先写顾问结果再返回顾问错误，并生成相同形状的八列表格与 `CREATE INDEX` 文本。`advise_id` 在两个版本中都未被读取。

Rust 为解除具体包耦合做了三处抽象：Go 的 `exec.BaseExecutor`/`sessionctx` 被 `A::Context` 代替；`ast.RecommendIndexOption` 被 `A::Option` 代替；`indexadvisor` 包函数被 `IndexAdvisor` trait 代替。因此 Rust 类型本身尚未实现 Go 的 `exec.Executor` 接口等价物，也没有 Go builder 的 Rust 对应接线。

结果表达存在明确差异。Go 接收顾问的嵌套结果，在执行器内用 `fmt.Sprintf` 格式化 `IndexSize`、读取 `Reason`、用 `json.Marshal` 序列化 `TopImpactedQueries`；Rust 的 `RecommendIndexResult` 已包含三个格式化后的字符串。这减少了执行器依赖，但把格式与 JSON 正确性的责任转移给 trait 实现。

直接边界证据主要来自 Go 测试 `pkg/planner/indexadvisor/indexadvisor_test.go`：它覆盖单条/多条 SQL、空片段、非法 SQL，并在 `TestIndexAdvisorCreateIndexStmt` 验证第八列等于 `CREATE INDEX idx_a ON t(a);`。Rust 的 `pkg/planner/indexadvisor/indexadvisor_sql_test.rs` 与 `options_test.rs` 覆盖顾问算法和选项语义，但没有实例化本文件的执行器，不能替代其独立回归测试。

## 扩展指南

- 接入 Rust 主链时，应在 Rust builder/计划层构造 `RecommendIndexExec`，提供真实 `IndexAdvisor` 实现，并为它建立与 Go `exec.Executor` 相同的生命周期协议；不要仅因模块已 `pub` 就认定功能可由 SQL 到达。
- 新增动作时，先核对 parser AST、planner plan、Go executor 和用户可见结果 schema，再修改 `Next` 的动作分派；`apply/ignore` 尤其不能只在此处添加空分支。
- 修改 SQL 切分策略时，必须保留或有意迁移空字符串、纯分号、混合空段、多语句与字符串内分号的兼容行为。
- 修改结果列时，同时核对 `RecommendIndexResult`、`Next` 的列序、planner schema、Go `recommend_index.go` 和下游客户端；第 6 列 JSON 与第 7 列 DDL 的字符串契约需要专门测试。
- 建议新增独立文件 `pkg/executor/recommend_index_test.rs`，并由 `lib.rs` 使用 `#[cfg(test)] mod recommend_index_test;` 接入，遵守测试与源文件分离规则。用假 `IndexAdvisor` 覆盖：三种动作、未知动作、一次性 `done`、SQL 分段、空输入差异、选项缺失、顾问错误伴随部分结果、八列顺序和 DDL 文本。
- 性能上，本文件会复制 `all_options`、分配 SQL 字符串、连接列名并格式化每条 DDL；通常相对顾问算法成本很小，但大结果集扩展前应评估 `Chunk` 容量及字符串分配。正确性风险集中在公开字段导致的不完整初始化、无转义 DDL、预序列化 JSON，以及错误后不可重试。

## 验证依据

- 本次依据冻结范围 `ad0a6f48804e48373cc79c5f53f0072daf99ceb9..2c944fbcbfb2c904cc8cbddafe17954138bd8867` 的逐文件差异及 `git show 2c944fb:pkg/executor/recommend_index.rs`，不把工作区后续变化作为行为依据。
- `pkg/executor/recommend_index_test.rs::recommend_index_return_result_does_not_persist` 记录 return=false、run=true；Go 对照 `pkg/executor/recommend_index.go`。
- crate/module 边界参考对应 `Cargo.toml` 和 `lib.rs`；executor/expression 均没有 `doc.go`。
- 本次 RustCodeGraph `status` 显示含 Rust 索引，但索引不代表冻结提交；以上行为以冻结 blob 为准，旧说明的图节点统计和历史行号不作为本次验证证据。
- Ready 文档验证：检查本任务七篇文档各有 11 个二级章节及 `git diff --check`，另人工审查增量语义和接线边界。按计划不运行 Cargo；上述测试是源码证据，未声称本轮执行通过。
