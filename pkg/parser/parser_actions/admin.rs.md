# `pkg/parser/parser_actions/admin.rs`

## 文件定位

`admin.rs` 是 `astersql-parser` crate 的语义动作分片，位于词法/语法规约与 `parser-ast` AST 类型之间。它不负责识别 token 或驱动 LR 状态机；`pkg/parser/parser_runtime.rs` 在规约时构造 `Rhs` 与 `parser_actions::Context`，再由 `pkg/parser/parser_actions/mod.rs::apply` 按稳定 `RuleId` 分派到本文件的 `owns`/`apply`。本文件覆盖的范围比 SQL 的 `ADMIN` 前缀更广，包含资源组、统计、备份恢复、SET/SHOW/FLUSH、绑定、权限、锁、序列、Plan Replayer、流量回放、Query Watch 等管理类或运维类语法。

crate 边界由 `pkg/parser/Cargo.toml` 确认：当前库名为 `astersql-parser`，AST 来自本地依赖 `parser-ast`，时长解析还使用本地 `parser-duration`；本文件通过 `use super::super::*` 复用解析器内部的 `RuleId`、`yySymType`、辅助类型与函数，因此不是可独立调用的公共 API。

## 核心职责

1. `identify` 将生成器提供的稳定字符串规则 ID 映射为私有 `AdminRule` 枚举，隔离易变化的 LR 数字规约号。
2. `owns` 向统一分派器声明本文件是否拥有某条语义动作；`remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 固定验证本模块拥有 551 条规则且每条需要动作的规约恰有一个 owner。
3. `apply` 在识别成功后调用 `apply_rule`；未知规则返回 `None`，由上层继续选择其他 action 模块。
4. `apply_rule` 从 RHS 语义槽读取标识符、表达式、表名、列表和选项，构造最终 statement AST 或供更高层规约继续消费的 `out.item` 中间值。
5. 在只能结合多个子项判断的地方执行语义校验，例如重复资源组选项、重复 Query Watch 选项、Go 风格 duration、互斥 masking-policy 修饰符以及权限/角色类型错配，并把错误写入 lexer。

该文件只建立 AST 和解析期语义值，不执行 SQL、访问 catalog、修改资源组或启动备份任务。

## 主要符号

- `AdminRule`：私有、可排序的 551 变体枚举；每个 `*AltNN` 对应 `parser.y` 中一个具体产生式备选。排序能力也被少量连续变体分支用于归类，但规则身份的权威来源仍是 `identify` 的显式映射。
- `identify(rule_id: RuleId) -> Option<AdminRule>`：精确匹配稳定 ID。任何新增或变更 grammar action 若没有同步此表，都不会归本模块处理。
- `owns(rule_id: RuleId) -> bool`：仅检查 `identify` 是否成功，不读取或修改解析状态。
- `apply(rule_id, rhs, context) -> Option<Result<bool, isize>>`：模块边界入口。`None` 表示非本模块规则；`Some(Ok(true))` 表示已完成规约；`Some(Ok(false))` 表示规则已识别但所需动态语义值缺失或类型不符；`Some(Err(1))` 表示已向 lexer 追加错误并要求解析器以状态 1 终止。
- `apply_rule(rule, rhs, context) -> Result<bool, isize>`：主实现。它按 `rhs.len()` 计算相对位置，从 `yySymType` 的 `ident`、`expr`、`item` 等字段读取值，并写入 `context.output.statement` 或 `context.output.item`。
- `Context`（定义于 `parser_actions/mod.rs`）：携带输出槽、`Parser` 状态和 `yyLexer`。本文件当前实际使用输出槽和 lexer；虽解构了 `parser_state`，目标文件中没有进一步引用它。

代表性输出包括 `AdminStmt`、`AnalyzeTableStmt`、`BRIEStmt`、`ShowStmt`、`SetStmt`、`GrantStmt`/`RevokeStmt`、`CreateResourceGroupStmt`、`PlanReplayerStmt`、`TrafficStmt`、`AddQueryWatchStmt`、`RefreshStatsStmt`，以及相应的选项、列表、布尔值和枚举中间语义值。

## 执行流程

1. `parser_runtime.rs` 完成一条 grammar reduction，准备默认输出槽，并用当前语义栈切片建立 `Rhs`。
2. `parser_actions/mod.rs::apply` 依次询问各 action 分片；当 `admin::owns(rule_id)` 为真时调用 `admin::apply(...).expect(...)`。`identify` 的同一映射同时保障“拥有”与“可执行”一致。
3. `apply_rule` 解构 `Context`，按 `AdminRule` 进入对应分支。叶子产生式通常把 token 转成枚举、数字、字符串或单个 option；递归产生式克隆已有 `Vec<T>` 并追加当前项；statement 产生式读取前面形成的中间值并组装最终 AST。
4. 例如资源组链先产生 `ResourceGroupOption`/`ResourceGroupRunawayOption`，列表分支检查同类选项重复，再由 create/alter resource-group 分支写入 statement。ADMIN 链直接选择 `AdminStmtType`，并按产生式填入 `job_ids`、表、索引、where、limit 或 BDR/plan-cache 字段。
5. Plan Replayer 的八种 dump 变体由共享分支根据 rule 计算 `Analyze`、RHS 回退位置、statement/file/string-list/slow-query 条件，并携带可选 `AsOfClause`；load/capture/remove 则由后续独立分支构造。
6. `apply_rule` 成功走完分支后统一返回 `Ok(true)`。上层运行时将规约输出压回语义栈；只有 `Ok(false)` 才会进入默认 `$1` 移动逻辑，但 `has_semantic_action` 会把该规约标记为语义不完整，避免把类型缺失误当成完整 AST。

## 数据与状态

数据在 `yySymType` 中逐层传递。终结符文本通常位于 `ident`，表达式位于 `expr`，动态类型中间值位于 `item: Option<Box<dyn Any...>>`，完整 statement 位于 `statement`。`apply_rule` 通过 `downcast_ref::<T>()` 读取并通常克隆值；在可转移所有权的路径使用 `take()`/`downcast::<T>()`，例如扩展已有 BRIE statement，从而避免要求所有 AST 节点可克隆。

列表的次序保持输入次序。资源组选项、动态校准选项、Query Watch 选项在追加时按 option type 去重；runaway 选项允许多个 `Rule` 条件，但 action/watch 等非 Rule 类别不得重复。空 optional 规则通常写入 `None`、空向量、空字符串或显式默认枚举，这些值要与 Go grammar 中的 `nil`、空 slice 或零值语义对应。

本文件没有持久全局状态。唯一可见的解析器侧状态变化是写 `Context.output` 与在错误时调用 `yyLexer::AppendError`；当前未修改 `parser_state`。AST 中的表名、标识符等通过 `NewCIStr` 保留大小写不敏感语义，表达式节点则沿 RHS 共享/克隆其引用型表示。

## 依赖与调用关系

上游调用链为 `parser_runtime.rs` reduction → `parser_actions/mod.rs::apply` → `admin::owns` → `admin::apply` → `apply_rule`。`parser_actions/mod.rs::has_semantic_action` 也调用 `admin::owns`，用于决定未成功构造动作时是否可采用 goyacc 的默认 `$$ = $1`。

下游主要依赖：

- `parser_ast`：所有 statement、option、枚举、表名、表达式和辅助结构的目标类型。
- `Rhs`/`yySymType`：解析栈语义值访问；`Rhs` 的索引实现遇到越界会 `expect`，所以 RHS 回退量必须与 grammar 产生式严格同步。
- `yyLexer`：生成并累计解析期语义错误。
- `validate_go_duration` 与 `parser_duration::ParseDuration`：分别核对 Go 兼容 duration 和动态校准 duration。
- `semantic_value_text`、`getUint64FromNUM`、`RoleOrPrivSemantic` 等解析器内部辅助项：完成动态语义值到 AST 字段的转换。

RustCodeGraph 可检索到 `AdminRule` 与 `apply_rule`，并确认统一分派调用点在 `parser_actions/mod.rs`；针对该 5762 行 action 文件的 `explore` 与按文件 `node` 本次未返回正文，因此具体分支依据以目标源码、模块入口和测试交叉核对。

## 错误处理与边界

显式解析错误均先通过 `yylex.Errorf`/`AppendError` 记录，再返回 `Err(1)`。已确认的边界包括：重复资源组、runaway、background、动态校准或 Query Watch 选项；非法 `EXEC_ELAPSED`、watch/traffic/dynamic-calibrate duration；BRIE 中非固定时间单位；GRANT/REVOKE 位置出现 role；masking policy 同时使用 `OR REPLACE` 与 `IF NOT EXISTS`；未知 masking-policy restrict operation。

若某个 RHS 动态值不存在或 downcast 失败，关键 statement 分支会返回 `Ok(false)`；许多可选字段或集合则采用 `unwrap_or_default()`。因此新增动作时必须判断字段是“grammar 保证必有”还是“合法可空”，不能把必需值悄悄降级成默认值。RHS 索引表达式形如 `rhs[rhs_len - back]`，它依赖产生式长度；grammar 改动而未同步回退量可能导致错误字段、整数下溢或索引 panic。

语法层拒绝的输入由生成的状态机处理，而非全部在本文件检查。例如 Go 与 Rust 测试均确认负数 job count、缺少 DDL job ID、残缺 alter-job option 等不能解析。相反，本文件只负责规约成功后仍需跨子项判断的语义约束。错误文本当前保留 Go 版本中的 `Dupliated` 拼写，这是兼容事实，不应仅在 Rust 侧随意更名。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。每次规约调用只借用当前 parser、lexer、输出槽与语义栈切片，生命周期由 `Context<'_>` 和 `Rhs<'_>` 限制在调用期间；返回前不保存这些借用。

动态 AST/中间值由 `Box`、`Vec`、克隆值或表达式引用型字段拥有。`take()` 会清空被移动的 RHS 槽，符合 `parser_runtime.rs` 明示的“goyacc 默认值通过移动而非克隆”策略。并发安全由“每个 Parser/lexer 实例独占其可变状态”的调用约束提供，本文件自身没有跨解析共享的可变单例。

## 与 Go 版本的对应关系

权威 Go 对照是 `pkg/parser/parser.y`，而不是一个同名 Go 文件。`AdminRule::*AltNN` 基本逐一对应 grammar 中同名非终结符的备选，`apply_rule` 则把 Go action 中的类型断言、slice append 和 AST 字段赋值改写为 Rust downcast、`Vec` 追加和结构体构造。

关键一致性证据包括：

- `AdminStmtLimitOpt` 的三种 LIMIT 写法保持 offset/count 交换规则；`AdminStmt` 的 show/check/cancel/pause/resume/binding/BDR/alter-job 分支保持相同 AST 字段。
- `ResourceGroupOptionList`、runaway/background/Query Watch 列表保持 Go 的重复项检查；priority 数值仍是 LOW=1、MEDIUM=8、HIGH=16；`UNLIMITED` watch duration 仍归一化为空字符串。
- `PlanReplayerStmt` 保持 dump/analyze/slow-query/file/list/load/capture/remove 的字段组合，并保留可选 historical stats 信息。
- `DropQueryWatchStmt` 仍区分数值 ID、静态资源组名和用户变量表达式；`RefreshStatsStmt` 保留 global/database/table scope、FULL/LITE 与 CLUSTER 标志。

Go `parser_test.go::TestAdminStmt` 提供兼容样例与失败样例；Rust 的 `parser_3_aster_unit_test.rs` 和 `yy_parser_4_aster_unit_test.rs` 验证迁移后的公开 parser 确实产生对应 AST。迁移不是“支持全部 Go 行为”的自动证明：新增 Go grammar action 时，Rust 的稳定 ID 映射、RHS 位置、AST 类型以及独立 Rust 回归测试都必须显式同步。

## 扩展指南

新增管理语法时应先修改权威 grammar/生成元数据，再在本文件同步三处：为每个有语义动作的备选增加 `AdminRule` 变体；在 `identify` 添加生成后的稳定 RuleId；在 `apply_rule` 构造与 Go action 等价的中间值或 statement。若新语法属于 DDL、security 等现有分片，应保持唯一 owner，不应仅因“管理用途”就放入本文件。

修改列表规则时必须保持逗号/无逗号两种 RHS 回退位置、输入次序和重复检测不变量；修改 statement 时要逐字段核对 Go 的 nil/zero/default 语义。新增必需动态值应使用失败分支而非无条件默认值。新增 duration、互斥修饰符或类型约束时，应沿用 lexer 错误通道并返回解析失败状态。

测试应继续放在独立文件而非 `admin.rs` 内：

- 在 `pkg/parser/parser_3_aster_unit_test.rs` 增加端到端 AST 字段与错误输入回归；公开 parser/语法接受范围可扩展 `yy_parser_4_aster_unit_test.rs`。
- owner 数量或分片发生变化时同步 `pkg/parser/parser_actions/remaining_aster_unit_test.rs::all_action_rules_have_one_owner` 的精确计数，并保证 `remaining_modules_have_no_numeric_fallback` 仍通过。
- 与 Go 兼容相关的语法同步 `pkg/parser/parser_test.go`/`parser.y` 的既有意图；AST restore/visitor 行为应在 `pkg/parser/ast/*_test.rs` 的相应独立测试中覆盖。

主要风险是稳定 RuleId 漏接导致规约语义不完整、RHS 回退量错位导致字段误填或 panic、默认值掩盖必需类型缺失，以及 Rust/Go 在校验和错误行为上漂移。该层只在解析时按产生式执行，通常不构成独立性能热点；但大列表分支会克隆已有向量，扩展时不应引入与输入规模无关的额外扫描。

## 验证依据

- 目标源码：`pkg/parser/parser_actions/admin.rs`；确认唯一模块级类型 `AdminRule`，以及 `identify`、`owns`、`apply`、`apply_rule` 四个函数；枚举和稳定映射包含 551 个 action 规则。
- 主链：`pkg/parser/parser_runtime.rs` 的 reduction 调用与默认 `$1` 移动；`pkg/parser/parser_actions/mod.rs` 的分片分派、`Context` 和 `has_semantic_action`。
- crate：`pkg/parser/Cargo.toml` 的 `astersql-parser` 库、`parser-ast` 与 `parser-duration` 本地依赖。
- Rust 测试：`pkg/parser/parser_actions/remaining_aster_unit_test.rs`（唯一 owner、admin_count=551、禁止数字回退），`pkg/parser/parser_runtime_aster_unit_test.rs`（稳定 RuleId 分派），`pkg/parser/parser_3_aster_unit_test.rs`（ADMIN 具体字段及资源组、Query Watch、Refresh Stats、Plan Replayer、Traffic 等完整性），`pkg/parser/yy_parser_4_aster_unit_test.rs`（公开 parser 的 TiDB 管理语法接受范围与非法输入）。
- Go 对照：`pkg/parser/parser.y` 的 `ResourceGroupOptionList`、`AdminStmtLimitOpt`/`AdminStmt`、`PlanReplayerStmt`、`AddQueryWatchStmt`/`DropQueryWatchStmt`；`pkg/parser/parser_test.go::TestAdminStmt` 及资源组、Plan Replayer 用例。
- RustCodeGraph：`status` 显示目标在有效本地索引中；`files --filter pkg/parser/parser_actions/admin.rs` 确认 557 个索引符号；`query AdminRule --kind enum` 定位第 21 行，`query apply_rule --kind function` 定位第 1784 行。该文件的 `explore`/`node --file` 没有返回正文，调用关系因此又以源码入口和测试做了直接复核。
- 本任务为纯文档分析，依照计划未运行 Cargo；交付前使用任务指定命令验证目标存在且恰有 11 个固定二级章节，并人工复查只新增本说明文件（任务文件按完成协议删除）。
