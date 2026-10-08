# `pkg/types/explain_format.rs`

## 文件定位

`pkg/types/explain_format.rs` 是 EXPLAIN 输出格式名称的共享协议词汇表。它不负责解析 SQL、生成执行计划或渲染结果，只把各层共享的格式标识定义成公开的 `&'static str` 常量，并用 `ExplainFormats` 给出合法格式的有序全集。源码有两条装配路径：`pkg/types/lib.rs` 以 `mod explain_format; pub use explain_format::*;` 从 `astersql-types` crate 根公开；`pkg/types/internal/metadata/lib.rs` 又在私有 `explain_format_defs` 模块中以 `include!("../../explain_format.rs")` 编译同一文件并公开再导出。因此调用方既可能从 `astersql_types` 根取得这些常量，也可能经 metadata 子 crate 取得同源定义。

主 crate 的边界由 `pkg/types/Cargo.toml` 定义：包名为 `astersql-types`，库入口是 `lib.rs`，且 `[package.metadata.porting].go-package = "pkg/types"` 明确记录了它与 Go `pkg/types` 的移植对应关系；同一 manifest 通过 `types-group-4` 依赖 metadata 子 crate。目标文件自身没有外部 crate 导入、feature 开关或条件编译项，能够被两条路径原样编译。

## 核心职责

文件承担两项职责：

1. 为 14 种 EXPLAIN 格式提供稳定、无分配、可跨 crate 比较的字符串常量：`brief`、`dot`、`hint`、`json`、`row`、`verbose`、`traditional`、`true_card_cost`、`binary`、`tidb_json`、`cost_trace`、`plan_cache`、`plan_tree` 和 `ru`。
2. 通过 `ExplainFormats: &[&str]` 保存同一组值的规范顺序，供合法值校验、枚举或兼容性检查使用。

它是“格式名称协议”而非“格式实现”。例如 Rust 侧 `pkg/planner/optimize.rs::shouldWarnPlanCacheBypass` 只用 `ExplainFormatPlanCache` 决定是否追加非预处理计划缓存的旁路警告；`pkg/planner/core/operator/physicalop/physical_projection.rs::ExplainInfo` 只用 `ExplainFormatPlanTree` 决定是否从表达式展示中移除列编号。真正的格式分派和行结构仍在规划器/执行器一侧，不能从常量存在推断某格式在所有 Rust 执行路径中都已完整实现。

## 主要符号

- `ExplainFormatBrief: &str = "brief"`：简表格式；Go 注释说明它与 row 类似但忽略 explain ID 后缀。
- `ExplainFormatDOT: &str = "dot"`：Graphviz DOT 输出标识。
- `ExplainFormatHint: &str = "hint"`：优化器 hint 输出标识。
- `ExplainFormatJSON: &str = "json"`：JSON 格式标识。
- `ExplainFormatROW: &str = "row"`：行式表格输出标识，也是 Go 执行路径中的常用默认格式。
- `ExplainFormatVerbose: &str = "verbose"`：包含更多计划信息的详细格式。
- `ExplainFormatTraditional: &str = "traditional"`：兼容名称；Go `Explain.prepareSchema` 会把它转换为 row 处理。
- `ExplainFormatTrueCardCost: &str = "true_card_cost"`：要求用真实基数计算/展示代价的格式标识。
- `ExplainFormatBinary: &str = "binary"`：二进制计划 proto 的输出标识。
- `ExplainFormatTiDBJSON: &str = "tidb_json"`：TiDB 扩展 JSON 输出标识。
- `ExplainFormatCostTrace: &str = "cost_trace"`：算子代价及公式跟踪格式。
- `ExplainFormatPlanCache: &str = "plan_cache"`：非预处理计划缓存原因/警告相关格式；Rust 已在 `shouldWarnPlanCacheBypass` 中直接使用。
- `ExplainFormatPlanTree: &str = "plan_tree"`：树形计划格式；Rust 投影算子的 `ExplainInfo` 对该值执行忽略大小写的比较。
- `ExplainFormatRU: &str = "ru"`：为 `EXPLAIN ANALYZE` 的 RU 代价输出保留的格式；独立测试要求它是合法格式列表最后一项。
- `ExplainFormats: &[&str]`：依上述顺序引用全部 14 个常量的静态切片。它是固定长度、只读视图，不拥有字符串，也没有运行时初始化。

以上全部符号均为 `pub const`；文件中没有 struct、enum、trait、函数、impl、宏或条件编译项。命名保留 Go 风格，crate 根的 `#![allow(non_snake_case, non_upper_case_globals, dead_code)]` 允许这些移植名称继续作为公开 API。

## 执行流程

该文件本身没有可调用流程；它通过读取常量参与上层流程：

1. SQL 解析后形成带 `Format` 的 EXPLAIN AST。
2. Go 主路径 `pkg/planner/core/preprocess.go` 将输入格式转为小写，遍历 `types.ExplainFormats`；非空且不在列表中的值产生 `ErrUnknownExplainFormat`。这证明列表是合法值协议，不是渲染器注册表。
3. 规划/执行阶段按单个格式常量选择行为。Go `pkg/planner/core/common_plans.go` 会先把 `traditional` 归一化为 `row`，再为 row/brief/verbose/DOT/hint/binary/TiDB JSON/RU 等格式准备不同 schema，并在渲染阶段分派到对应实现。
4. 已接线的 Rust 路径按需读取单个常量：`pkg/planner/optimize.rs` 从 `astersql-types` 根路径比较 `plan_cache` 以控制旁路警告；`physical_projection.rs` 从 statement context 读取格式、去空白并对 `plan_tree` 做 ASCII 大小写无关比较，后者经 `expression::types` 和 metadata 子 crate 的 `include!` 装配路径取得同一源码定义，随后控制表达式列编号是否保留。

因此，增加常量只是第一步；若希望新格式实际可用，还必须同步验证入口、会话上下文、schema 选择、渲染分派和测试，不能只扩充 `ExplainFormats`。

## 数据与状态

所有单值均为编译期 `&'static str`，其字节存放在程序静态区。`ExplainFormats` 是静态切片，元素仍指向上述静态字符串；读取不分配、不复制底层字符串，也不存在惰性初始化。

列表顺序是受测试保护的兼容状态，而不是任意集合。`pkg/types/enum_4_aster_unit_test.rs::eval_types_and_explain_formats_preserve_aliases_values_and_order` 对 14 个字面值及其完整顺序逐项断言；`pkg/types/explain_format_test.rs::go_merge_10_ru_is_the_last_valid_explain_format` 额外断言 `ExplainFormatRU == "ru"` 且是末项。消费者若按顺序展示或追加格式，重排会造成可观察差异。

该模块不保存当前语句的格式。运行时状态位于 statement/session context；例如 Rust `physical_projection.rs::ExplainInfo` 通过 `StmtCtx.ExplainFormatValue()` 读取当前值，Go `common_plans.go::prepareSchema` 设置 `InExplainStmt` 并维护 `ExplainFormat`。常量文件既不修改这些状态，也不持有 AST、计划树或运行统计。

## 依赖与调用关系

下游依赖方面，目标文件只依赖 Rust 内建的字符串和切片类型；各常量之间唯一关系是 `ExplainFormats` 引用 14 个单值常量。`pkg/types/Cargo.toml` 没有为该文件引入专属第三方依赖。

上游关系由两条模块装配路径和引用点组成：

- `pkg/types/lib.rs` 私有声明 `explain_format`，随后通配公开再导出全部符号，并在 `#[cfg(test)]` 下挂载独立的 `explain_format_test.rs`。
- `pkg/types/internal/metadata/lib.rs` 以 `include!("../../explain_format.rs")` 把同一物理源码编译到 metadata crate 的 `explain_format_defs` 中并公开再导出；其独立 `Cargo.toml` 定义包名 `astersql-types-metadata`，主 `astersql-types` 再以依赖别名 `types-group-4` 引入它。
- `pkg/planner/Cargo.toml` 以路径依赖 `astersql-types`；`pkg/planner/optimize.rs::shouldWarnPlanCacheBypass` 使用 `ExplainFormatPlanCache`。
- `pkg/expression/Cargo.toml` 以别名 `types-dependency` 依赖 `astersql-types`；`pkg/expression/lib.rs::types` 从 `types_dependency::metadata` 再导出同源的 `ExplainFormatPlanTree`，而 `pkg/planner/core/operator/physicalop/physical_projection.rs::ExplainInfo` 经 `expression::types::ExplainFormatPlanTree` 比较树形格式。
- RustCodeGraph 对目标文件报告的直接使用文件还包括 `pkg/planner/optimize_aster_unit_test.rs`、`pkg/types/enum_4_aster_unit_test.rs` 和 `pkg/types/explain_format_test.rs`；前者验证 plan-cache/RU 上下文分支，后两者验证常量和值序。
- Go 对照消费者 `pkg/planner/core/preprocess.go` 使用完整 `ExplainFormats` 校验输入；`pkg/planner/core/common_plans.go` 和 `pkg/planner/core/planbuilder.go` 使用单值常量完成 schema、渲染和格式约束分派。

RustCodeGraph 能定位这些文件级使用关系和常量定义，但本次索引对 `pub const` 的 `callers/callees` 精确边解析返回“无定义/无边”；因此具体引用点使用 `rg` 补证，未把缺失图边解释为“没有调用者”。

## 错误处理与边界

目标文件没有 `Result`、panic、日志或错误构造；常量访问本身不会失败。错误边界由消费者负责：

- Go `preprocess.go` 只在 `x.Format` 非空且不属于 `ExplainFormats` 时返回 `ErrUnknownExplainFormat`，比较前对用户输入做 `strings.ToLower`，所以合法格式匹配不区分大小写。
- `ExplainFormats` 中保存的值全部为小写；Rust 投影路径额外执行 `trim()` 和 `eq_ignore_ascii_case`，但其他消费者未必执行同样归一化。调用方不能假设本模块会自动清理输入。
- “在合法列表中”不保证任意执行模式都受支持。Go `common_plans.go::prepareSchema` 对不支持的 format/analyze 组合可返回错误，对部分不支持情况会追加 warning；`EXPLAIN FOR CONNECTION` 也只允许 brief、row 和 verbose。
- `traditional` 是别名语义而非独立渲染实现；Go 路径会改写为 row。修改其字面值或移除它会影响兼容输入。
- RU 当前有明确边界：Go 代码把它描述为 `EXPLAIN ANALYZE` 的 RU 输出，并在 schema 分支留有“待每算子 RU 归因可用后填充”的 TODO；文档不把该 TODO 描述成已完成能力。

## 并发与资源生命周期

模块没有锁、原子变量、线程、异步任务、通道、文件句柄、网络连接、事务或堆资源。每个编译该文件的 crate 都得到自己的不可变常量定义；所有数据都是静态引用，可被任意线程并发读取，不需要同步，也没有释放顺序。

Go 对照文件使用包级 `var` 和可变 `[]string`，理论上可被进程内代码改写；Rust 移植改为 `pub const` 和共享只读切片，收窄了可变性并消除了运行时初始化/并发写入风险。这个差异不改变当前字面值和顺序，但意味着 Rust 调用方不能通过修改全局列表动态注册格式；扩展应修改源码并重新编译。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/types/explain_format.go`。Rust 版本逐项保留 Go 的 14 个名称、字面值和列表顺序；`pkg/types/Cargo.toml` 的 porting 元数据也把整个 crate 指向 Go `pkg/types`。两边关键语义一致：brief 与 row 的兼容关系、traditional 作为 row 的别名、true-card-cost/cost-trace/plan-cache/plan-tree/RU 的用途，以及 `ExplainFormats` 作为合法值集合。

主要语言层差异如下：

- Go 单值和列表是包级 `var`；Rust 是 `pub const` 静态借用，Rust 侧不可在运行时替换或追加。
- Go `ExplainFormats` 类型为拥有字符串头的 `[]string`；Rust 类型为 `&[&str]`，列表和元素都借用静态数据。
- Go 包内符号天然可见；Rust 先在私有 `explain_format` 模块定义，再由 `pkg/types/lib.rs` 从 crate 根公开再导出。
- Go 当前拥有完整的 SQL 合法性校验与 Explain 分派证据；本次检索到的 Rust 生产引用只覆盖 `plan_cache` 与 `plan_tree` 的局部行为。其余 Rust 常量已定义并受顺序测试保护，但不能仅据此声称对应渲染链全部移植完成。

Go 测试证据包括 `pkg/planner/core/preprocess_test.go` 对未知格式 `xx` 返回 `ErrUnknownExplainFormat` 的断言，以及 `pkg/executor/explain_test.go::TestExplainFormatInCtx` 对多种格式写入 statement context、plan-cache 特例行为的验证。

## 扩展指南

新增或调整 EXPLAIN 格式时，应按以下边界同步：

1. 在 `pkg/types/explain_format.rs` 增加/修改单值常量，并在 `ExplainFormats` 的兼容位置登记；同时核对 `pkg/types/explain_format.go`，避免 Go/Rust 名称、值和顺序漂移。
2. 更新独立 Rust 测试 `pkg/types/explain_format_test.rs` 或 `pkg/types/enum_4_aster_unit_test.rs`。不要把测试写回生产源文件；若末项约束改变，必须显式评估并更新 `go_merge_10_ru_is_the_last_valid_explain_format`。
3. 为真正可用的格式接入合法性校验、会话 statement context、planner/executor schema 和渲染分派。Go 侧重点是 `preprocess.go`、`planbuilder.go`、`common_plans.go`；Rust 侧应从实际使用该常量的 planner/executor 路径继续追踪，而不是假设通配再导出即代表功能完成。
4. 若格式改变表达式展示，检查 `physical_projection.rs::ExplainInfo` 等算子级 ExplainInfo；若影响计划缓存诊断，检查 `optimize.rs::shouldWarnPlanCacheBypass` 及其独立测试。
5. 增加拒绝未知格式、大小写归一化、支持/不支持的 analyze 组合、输出 schema 和实际渲染结果测试。格式字符串属于 SQL 可见兼容协议，重命名、删除、重排或改变别名行为都有兼容风险。

性能风险很低：常量读取为静态引用，线性验证列表目前仅 14 项。若未来格式数量显著增长才需要评估查找结构；在此之前，把列表替换为更复杂结构可能破坏顺序契约而收益有限。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`node --file pkg/types/explain_format.rs` 读取目标文件并报告 5 个使用文件；对 15 个常量执行 `query --kind constant`，确认定义位置和唯一性。对关键常量执行 `callers/callees` 时索引未解析常量引用边，已用文件级使用关系和 `rg` 补证。
- Rust 源与装配：`pkg/types/explain_format.rs`（15 个公开常量、无可执行函数）；`pkg/types/lib.rs`（主 crate 模块挂载、公开再导出、独立测试挂载）；`pkg/types/internal/metadata/lib.rs`（同一源码的 `include!` 装配与再导出）；`pkg/planner/optimize.rs`（plan-cache 警告门控）；`pkg/planner/core/operator/physicalop/physical_projection.rs`（plan-tree 表达式展示分支）；`pkg/expression/lib.rs`（metadata 路径的跨 crate 再导出）。
- Cargo 边界：`pkg/types/Cargo.toml`、`pkg/types/internal/metadata/Cargo.toml`、`pkg/planner/Cargo.toml`、`pkg/expression/Cargo.toml`，分别证明主/metadata crate 身份、planner 路径依赖和 expression 的别名路径依赖。
- Rust 独立测试：`pkg/types/explain_format_test.rs` 验证 RU 值及末项不变量；`pkg/types/enum_4_aster_unit_test.rs` 验证完整值序；`pkg/planner/optimize_aster_unit_test.rs` 验证仅 plan-cache 格式触发旁路警告。
- Go 对照与行为：`pkg/types/explain_format.go`（完整同名集合）；`pkg/planner/core/preprocess.go` 与 `preprocess_test.go`（合法列表和未知格式错误）；`pkg/planner/core/common_plans.go`（schema/渲染分派及不支持边界）；`pkg/executor/explain_test.go::TestExplainFormatInCtx`（格式进入 statement context）。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核文档只描述有上述路径支持的事实。
