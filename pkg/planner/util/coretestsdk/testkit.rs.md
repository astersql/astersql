# `pkg/planner/util/coretestsdk/testkit.rs`

## 文件定位

本文件属于 workspace crate `astersql-planner-util-coretestsdk`，由同目录 `lib.rs` 以公开模块 `testkit` 暴露。它是规划器相关测试使用的夹具层，而不是 SQL 请求的生产执行路径：一部分 API 解析 EXPLAIN 文本字段，另一部分 API 将 `mock.rs` 中的表元数据、会话上下文和规划上下文组装为 `PlannerSuite`。

`Cargo.toml` 通过 `package.metadata.porting.go-package = "pkg/planner/util/coretestsdk"` 指明 Go 对照包。真实的 domain、infoschema、parser、sessionctx 等 crate 目前只出现在 `cfg(windows)` 条件依赖中；本文件实际编译的实现使用 `crate::mock` 的本地简化类型。因此它当前验证的是移植夹具的数据和初始化约定，不能视为已经接入完整 Rust planner、parser 或 domain。

## 核心职责

1. `get_field_value` 复现 Go `GetFieldValue` 对 EXPLAIN 行的窄格式解析规则。
2. `ParserConfig` 和 `Parser` 保存测试关注的两个解析器开关，不执行 SQL 解析。
3. `PlannerSuite` 聚合 parser 桩、`InfoSchema`、`MockContext` 和创建时取得的 `PlanContext` 快照，并提供与 Go getter 对齐的访问入口。
4. `create_planner_suite` 接受调用者提供的上下文和 schema，建立不启用特殊 parser 开关的套件。
5. `create_planner_suite_elements` 建立默认的九张 mock 表，重新分配表/分区 ID，配置默认上下文和 parser 开关；`create_planner_suite_elems` 是兼容 Go 命名语义的别名。

这些职责由 `coretestsdk_aster_unit_test.rs` 的字段解析、默认套件和自定义套件测试直接覆盖。

## 主要符号

- `pub fn get_field_value(prefix: &str, row: &str) -> String`：只接受前缀出现在非零位置、且值后存在空格的行；截取前缀与首个后续空格之间的片段，再从两端移除逗号。条件不满足时返回空字符串。
- `pub struct ParserConfig`：公开字段 `window_functions`、`strict_double_type_check` 表示窗口函数和严格 DOUBLE 类型检查开关。
- `pub struct Parser`：仅持有公开的 `config`，是配置容器而非真实 parser。
- `pub struct PlannerSuite`：私有保存 `parser`、`info_schema`、`session_context`、`plan_context` 和 `closed`。字段私有保证调用者通过访问器观察状态。
- `parser`/`info_schema`/`session_context`/`plan_context`：Rust 风格只读访问器。`info_schema` 特意返回表切片而不是容器。
- `get_parser`/`get_is`/`get_sctx`/`get_ctx`：与 Go 导出方法对应的别名或容器访问器；图索引显示 `get_parser`、`get_sctx`、`get_ctx` 分别下调 Rust 风格访问器。
- `close`/`is_closed`：把 `closed` 置真并将上下文的 `stats_handle_created` 置假；后者用于断言收尾状态。
- `create_planner_suite`：通用构造器，接受 `impl Into<InfoSchema>`；文件末尾的 `From<Vec<TableInfo>> for InfoSchema` 允许直接传表向量。
- `create_planner_suite_elements`：默认夹具构造器，下调九个 `mock_*_table` 构造器、`mock_context` 和 `create_planner_suite`。
- `create_planner_suite_elems`：只转发到 `create_planner_suite_elements`。

文件没有 trait、枚举、模块级常量或条件编译项。

## 执行流程

`get_field_value` 先用 `row.find(prefix)` 找首个匹配，并以 `index > 0` 排除行首匹配；随后从前缀末端生成 `tail`，要求 `tail.split_once(' ')` 成功且空格前的值非空，最后执行 `trim_matches(',')`。这意味着“缺前缀”“前缀在第 0 位”“前缀后立刻为空格”“值位于行尾且没有空格”都会得到空字符串，和 Go 实现的 `strings.Index` 分支一致。

`create_planner_suite` 将输入转换成 `InfoSchema`，立即通过 `MockContext::get_plan_context` 复制当前库名与 schema，创建两个开关均为 `false` 的 `Parser`，并以 `closed = false` 返回套件。`PlanContext` 是创建时快照，不是对 `MockContext` 的动态视图。

`create_planner_suite_elements` 的顺序是：

1. 依次构造 signed、unsigned、view、no-PK、range/hash/list 分区、StateNone 列和全局索引 hash 分区九张表。
2. 从 `id = 1` 开始，对每张表先赋 ID；若有分区，再按 definitions 顺序继续赋 ID。ID 在全部表和分区之间共享同一递增序列。
3. 用表向量建立 `InfoSchema`；创建 `mock_context`，将 schema 克隆进上下文，并启用上下文的窗口函数标志。
4. 调用 `create_planner_suite` 取得套件，再把 parser 的窗口函数和严格 DOUBLE 检查开关设为 `true`。

`create_planner_suite_elems` 不增加状态或分支。`close` 也不销毁对象，只更新两个可观察标志。

## 数据与状态

默认套件持有九张 `TableInfo`。重新编号会覆盖 `mock.rs` 构造器原先的表和分区 ID；测试确认首表 ID 为 1、range 表 ID 为 5 且首分区 ID 为 6、最后一张表 ID 为 15 且第二分区 ID 为 17。表、列、索引和分区的具体形状由 `mock.rs` 负责，本文件只决定集合顺序与跨表 ID 不变量。

`InfoSchema` 在默认构造中至少发生两次克隆/复制所有权：一份放入 `MockContext.info_schema`，一份作为 `PlannerSuite.info_schema`；`PlanContext` 又从上下文复制当前数据库和 schema。后续若能修改上下文，这些副本不会自动同步。

默认 `mock_context` 已把 `current_database` 设为 `test`，并标记 store、domain、stats handle 已建立。本文件默认路径额外开启 `window_functions_enabled`。通用构造器不改输入上下文，只给 parser 使用关闭的默认开关。`closed` 初始为 `false`；调用 `close` 后永久为 `true`，再次调用仍保持同一状态。

## 依赖与调用关系

直接下游全部来自 `crate::mock`：`InfoSchema`、`MockContext`、`PlanContext`、`TableInfo`、`mock_context`，以及九个表构造器。内部调用边为 `get_parser -> parser`、`get_sctx -> session_context`、`get_ctx -> plan_context`、`create_planner_suite_elements -> create_planner_suite`、`create_planner_suite_elems -> create_planner_suite_elements`。RustCodeGraph 对文件的索引列出 19 个符号，并确认这些内部边。

Rust 直接上游目前是同 crate 的 `coretestsdk_aster_unit_test.rs`：它导入并调用 `get_field_value`、`create_planner_suite`、`create_planner_suite_elements`。仓库中多个 Cargo manifest 依赖该 crate，但 Rust 源码搜索未发现它们调用本文件的 `PlannerSuite`/构造器；已发现的跨 crate 使用（如 `pkg/planner/core/casetest/dag/dag_test.rs`）只导入 `mock` 子模块。因此不能仅凭 Cargo 依赖宣称本文件已服务所有 Rust planner 测试。

Go 上游范围更广：例如 `partition_pruner_test.go` 调用 `coretestsdk.GetFieldValue`，`logical_plans_test.go`、`lateral_join_test.go` 等大量测试调用 `CreatePlannerSuiteElems`，`rule_generate_column_substitute_test.go` 调用 `CreatePlannerSuite`。这些是移植目标与使用语义的证据，不是 Rust 调用边。

## 错误处理与边界

本文件没有 `Result`、显式错误类型或 panic 分支。文本格式不符合预期时，`get_field_value` 用空字符串同时表示“未找到”和“找到但无有效值”，调用者不能区分原因；它只找首个前缀，且 `trim_matches(',')` 会移除值两端的所有逗号，而不是只移除一个尾逗号。

默认构造器直接按固定索引使用九张已知表，并只遍历可选分区；无分区表自然跳过。它不验证表名重复、ID 溢出、空 schema 或输入 `MockContext.info_schema` 是否与传入 schema 一致。通用构造器以显式 `info_schema` 作为套件值，但其 `PlanContext` 取自输入上下文；若调用者传入不一致的两者，文件不会协调它们。

Go 默认构造器创建真实 mock domain/statistics handle，失败时 panic，并在 `Close` 中关闭 handle；Rust 版本没有对应失败路径，只操作布尔状态。因此 Rust 的成功不证明真实资源初始化和关闭正确。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或共享引用计数。构造器返回独占的 `PlannerSuite`；访问器只借用，`close` 需要 `&mut self`，Rust 借用规则阻止同一时刻通过安全代码并发修改该套件。

`close` 不是 `Drop`，离开作用域不会自动调用它；同时当前 `MockContext` 只用 `stats_handle_created` 布尔值模拟资源，所以遗漏 `close` 不会在本实现中泄漏真实 handle。若未来接入 Go 对应的 domain/statistics 资源，应明确所有权、失败回滚、幂等关闭和 `Drop` 策略，并补充独立测试，不能继续把布尔翻转当作资源释放证据。

## 与 Go 版本的对应关系

主要映射为：Go `GetFieldValue` 对应 Rust `get_field_value`；Go `PlannerSuite` 与四个 getter 对应 Rust 同名结构及两组访问器；Go `CreatePlannerSuite` 对应 `create_planner_suite`；Go 私有 `createPlannerSuite` 的默认装配对应 `create_planner_suite_elements`；Go `CreatePlannerSuiteElems` 对应 `create_planner_suite_elems`。默认表顺序、全局递增 ID、窗口函数开关和严格 DOUBLE 检查均保留。

差异必须视为当前迁移限制：Rust `Parser` 是配置桩；`InfoSchema` 是 `Vec<TableInfo>` 容器；`MockContext`/`PlanContext` 是值对象；Rust 没有创建 store client、真实 domain、schema validator 或 info cache，也不会执行真实 stats handle 的创建/关闭。Go 的 `PlannerSuite.ctx` 在默认路径指向 mock context，而 Rust 保存的是由它生成的 `PlanContext` 快照。Rust 额外提供 `is_closed` 和 `From<Vec<TableInfo>>`，用于本地测试和便利构造，不是 Go API 原样翻译。

`coretestsdk_aster_unit_test.rs` 明确检查字段解析边界、自定义构造器不开启 parser 选项、默认 ID 顺序、数据库名以及 close 状态；这些测试是当前 Rust 对齐程度的权威边界。

## 扩展指南

- 新增默认 mock 表时，修改 `create_planner_suite_elements` 的表向量，并同步检查全局 ID 顺序；在独立的 `coretestsdk_aster_unit_test.rs` 中增加表数量、关键 ID 和必要元数据断言，不要把测试写回本文件。
- 扩展 EXPLAIN 字段格式时，先确认 Go `GetFieldValue` 或其调用者是否同步变化，再修改 `get_field_value`；必须覆盖行首前缀、空值、无尾随空格、多逗号和多次前缀等兼容边界。
- 增加 parser 配置时，同时更新 `ParserConfig`、通用构造器默认值、默认套件显式配置与两条构造路径的测试，避免“默认关闭/默认夹具开启”的差别被抹平。
- 接入真实 parser/infoschema/sessionctx/domain 时，应替换而不是旁路现有桩，逐项恢复 Go 的 store、domain、统计句柄和 schema cache 行为；真实外部资源的依赖必须有可传播的错误和确定的清理协议。
- 若允许构造后更新 session schema，应先决定 `PlannerSuite.info_schema`、`MockContext.info_schema` 和 `PlanContext.info_schema` 的一致性模型；当前克隆快照语义容易产生陈旧状态。

兼容风险主要来自 Go/Rust 夹具语义漂移和 ID 顺序变化；性能风险当前限于克隆完整 schema，测试规模较小，但扩展到大表集或真实 infoschema 后需重新评估；并发风险要在引入共享真实上下文时重新设计。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/planner/util/coretestsdk` 确认 crate 的六个已索引文件；`node --file pkg/planner/util/coretestsdk/testkit.rs` 读取完整 265 行及 19 个符号；`query` 定位 `get_field_value`、`PlannerSuite` 和三个构造器；`callers`/`callees` 查询确认图中可见的内部转发边，但调用者结果为空，因此上游使用另以源码搜索核验。
- Rust 实现：`pkg/planner/util/coretestsdk/testkit.rs`；直接模型与构造器：`pkg/planner/util/coretestsdk/mock.rs`；模块入口：`pkg/planner/util/coretestsdk/lib.rs`。
- crate 边界：`pkg/planner/util/coretestsdk/Cargo.toml`；workspace 与若干测试 crate 的 Cargo manifest 证明 crate 被纳入并声明为依赖，但不等价于本文件 API 被调用。
- Go 对照：`pkg/planner/util/coretestsdk/testkit.go`；Go 使用样例包括 `pkg/planner/core/casetest/partition/partition_pruner_test.go`、`pkg/planner/core/logical_plans_test.go`、`pkg/planner/core/lateral_join_test.go` 和 `pkg/planner/core/rule_generate_column_substitute_test.go`。
- Rust 独立回归测试：`pkg/planner/util/coretestsdk/coretestsdk_aster_unit_test.rs`。源码搜索确认它是当前直接调用本文件 API 的 Rust 测试；按任务约束未运行 Cargo。
- 人工复核结论：该文件存在是为 Rust 规划器测试提供 Go coretestsdk 的局部可执行替身；当前运行路径只组装内存值和状态标志，安全扩展必须同步同目录独立测试，并区分已实现桩语义与尚未接通的真实 TiDB 资源语义。
