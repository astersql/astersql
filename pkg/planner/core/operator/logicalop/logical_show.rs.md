# `pkg/planner/core/operator/logicalop/logical_show.rs`

## 文件定位

该文件属于 Cargo crate `astersql-planner-core-operator-logicalop`，crate 入口 `pkg/planner/core/operator/logicalop/lib.rs` 以 `mod logical_show` 编译本模块并通过 `pub use logical_show::*` 导出其公开符号。它实现 `SHOW` 语句的叶子逻辑算子及 `SHOW STATS_META` 的专用谓词抽取，而不负责读取系统表或生成最终结果行。

上游 `pkg/planner/core/logical_plan_builder_runtime.rs::build_show_runtime` 把解析器的 `ShowStmt` 转成 `LogicalShow`，设置输出 `Schema`/`OutputNames`，并在存在 `WHERE` 时先包一层 `LogicalSelection`。优化器随后经 `LogicalPlan::PredicatePushDown` 调用本文件的专用逻辑。下游 `pkg/planner/cascades/old/implementation_rules.rs::ImplShow::OnImplement` 把 `ShowContents` 与 `Extractor` 克隆到 `PhysicalShow`；物理节点定义在 `pkg/planner/core/operator/physicalop/physical_show.rs`。

## 核心职责

- 用 `ShowContents` 保存当前 Rust 规划链实际承载的 SHOW 参数，包括类型、库/分区/索引/资源组、标志位以及导入和分布任务标识。
- 用 `LogicalShow` 将 SHOW 参数、输出 schema、计划上下文、统计信息和可选谓词抽取结果组织成实现 `LogicalPlan` 的叶子节点。
- 仅为 `ShowKind::StatsMeta` 识别 `db_name`、`table_name` 上可安全抽取的等值、`IN` 和全可抽取 `OR` 条件；无法完整证明安全时保留原谓词。
- 为 SHOW 结果提供行数为 1、每个输出列 NDV 为 1 的占位统计，避免后续优化阶段缺少统计信息。
- 定义可克隆的 `ShowPredicateExtractor` trait，使抽取结果可以跨逻辑计划到物理计划传递。

本文件不执行 SHOW、不访问元数据、不校验权限，也不估算真实 SHOW 结果规模；这些职责分别位于构建/执行上下文及后续物理执行链。

## 主要符号

- `StringSet = HashSet<String>`：过滤值集合；集合语义同时用于去重和多个可抽取谓词之间的交集。
- `ShowPredicateExtractor`：物理 SHOW 可消费的抽取器接口。`CloneBox` 支持 trait object 深克隆；`Extract`、`ExplainInfo`、`Field`、`FieldPatternLike` 保留与 Go 接口一致的表面契约。
- `ShowStatsMetaPredicateExtractor { DB, Table }`：保存库名和表名过滤集合。当前 `Extract` 固定为 `false`，其余通用描述方法返回空值；有效数据只通过 `StatsMetaDBFilters` 与 `StatsMetaTableFilters` 暴露。这一点由 `logical_show_test.rs::stats_meta_extractor_interface_matches_go_stub_contract` 固定。
- `ShowKind::{Other, StatsMeta}`：Rust 侧只区分是否走统计元数据专用抽取；其他具体 SHOW 类别统一为 `Other`。
- `ShowContents`：SHOW 参数载荷。`MemoryUsage` 返回结构静态大小，加 `DBName`、`Partition.O/L`、`IndexName.O/L` 的当前字节长度；它没有计入所有 `String` 字段，也按长度而非预留容量计数。
- `LogicalShow`：组合 `LogicalSchemaProducer`、`ShowContents` 与可选 `Extractor`。`Init` 用类型名 `"Show"` 和零偏移初始化 `BaseLogicalPlan`。
- `LogicalShow::PredicatePushDown`：StatsMeta 专用优化入口；非 StatsMeta 原样返回谓词。
- `LogicalShow::DeriveStats` / `getFakeStats`：缓存或重建占位统计。
- `extractStatsMetaFilters`：按显示列名定位列 ID，收集所有可抽取谓词值并求交集，再决定是否移除这些谓词。
- `findShowColumnIDs`：将 `OutputNames` 和 `Schema.Columns` 按位置配对，返回指定列名对应的 `UniqueID`。
- `extractStatsMetaFilterValues`、`extractStatsMetaEQValue`、`extractStatsMetaINValues`、`getStringValueFromConstant`：逐层识别表达式形态并取出字符串常量。
- `impl LogicalPlan for LogicalShow`：提供类型擦除、基类访问，以及对固有 `PredicatePushDown`、`DeriveStats` 的动态分派。

此外，`pkg/planner/core/operator/logicalop/hash64_equals_generated.rs` 为 `LogicalShow` 提供语义哈希和相等比较；当前只比较 `LogicalSchemaProducer`，不会比较 `ShowContents` 或 `Extractor`。

## 执行流程

1. `build_show_runtime` 根据 AST 构造输出列和 `NameSlice`，把 `ShowStmtType::StatsMeta` 映射为 `ShowKind::StatsMeta`，其他类型映射为 `Other`，复制当前 Rust 已支持的 `ShowContents` 字段后调用 `Init`。
2. 若 AST 有 `WHERE`，构建器重写表达式、拆为 CNF 项，并用 `LogicalSelection` 包住 `LogicalShow`；有 `WHERE` 或 pattern 时还会增加投影以稳定输出列标识。
3. 谓词下推遍历到 SHOW 时，非 StatsMeta 节点直接返回全部谓词。StatsMeta 节点先对 `db_name` 调用 `extractStatsMetaFilters(..., to_lower=true)`，再对剩余谓词抽取 `table_name`，后者保留原大小写。
4. 单列抽取先由 `findShowColumnIDs` 找到与显示名匹配的列 ID。每个谓词只有在完全符合支持形态时才产生值集合：`eq` 允许列在等号任一侧；`in` 要求第一项是目标列；`or` 要求每个分支都可递归抽取。
5. 同一列上的多个可抽取谓词按集合交集组合。若没有可抽取项，或交集为空，则返回原谓词和空集合；交集为空时保留原谓词是为了避免把矛盾条件错误变成无过滤扫描。
6. 只有成功移除至少一个谓词时，`PredicatePushDown` 才安装包含 DB/Table 集合的 `ShowStatsMetaPredicateExtractor`。未抽取的条件继续留在上层 Selection 求值。
7. 统计推导在 `reload=false` 且已有缓存时复用缓存并返回“未重算”；否则由 `getFakeStats` 写入 `RowCount=1` 和每列 `NDV=1`，返回“已重算”。
8. `ImplShow` 只在所需物理属性不含排序项时匹配，随后把内容与抽取器复制到 `PhysicalShow`，供执行侧使用。

## 数据与状态

`LogicalShow` 的持久状态分三部分：`LogicalSchemaProducer` 持有计划上下文、schema、输出名和统计缓存；`ShowContents` 是拥有所有权的值对象；`Extractor` 是可选的 boxed trait object。谓词抽取会原地更新 `Extractor`，统计推导会原地更新基类统计缓存，其他辅助函数都是纯输入/输出转换。

列匹配依赖两个位置不变量：`NameSlice.0` 与 `Schema.Columns` 同序，且目标列由大小写折叠后的 `FieldName.ColName.L` 精确匹配。函数用 `zip` 截断长度不一致的两侧，不会越界。过滤值使用集合，因此顺序不具有语义；DB 值统一转小写，Table 值不转小写。参数标记不采用 `Constant.Value` 的旧快照，而通过当前 `EvalContext` 的 `GetUserVar` 重新取值。

`EMPTY_SHOW_CONTENTS_SIZE` 是编译期的 `size_of::<ShowContents>()`。`MemoryUsage` 只加若干字符串的已用长度，和 Go 实现一样是近似值；Rust 测试明确要求不按 `String::capacity` 计费。

## 依赖与调用关系

crate 依赖由 `pkg/planner/core/operator/logicalop/Cargo.toml` 声明。本文件通过 `use crate::*` 使用本 crate 汇总导出的 `base`、`expression`、`parser_ast`、schema/计划基类与统计类型；其中直接影响行为的边包括：

- 上游构建：`logical_plan_builder_runtime.rs::build_show_runtime -> LogicalShow::Init`，并设置 schema、输出名和可能的 Selection/Projection。
- 优化分派：`LogicalPlan::PredicatePushDown -> LogicalShow::PredicatePushDown -> extractStatsMetaFilters -> extractStatsMetaFilterValues`；后者继续调用 EQ/IN/常量辅助函数。
- 统计分派：`LogicalPlan::DeriveStats -> LogicalShow::DeriveStats -> getFakeStats`。
- 算子识别：`pkg/planner/cascades/pattern/pattern.rs` 把本类型识别为 `OperandShow`；memo 的 `group_expr.rs` 使用生成的 `Hash64`/`Equals`。
- 物理化：`pkg/planner/cascades/old/implementation_rules.rs::ImplShow::OnImplement -> PhysicalShow`，复制 `ShowContents` 和克隆 `Extractor`。
- 模块装配与测试：`logicalop/lib.rs` 公开重导出实现，并以独立的 `mod logical_show_test` 挂载测试，测试逻辑未嵌入生产文件。

RustCodeGraph 的文件查询确认目标文件已索引（358 行、42 个符号）；`explore "LogicalShow PredicatePushDown extractStatsMetaFilters"` 确认 `PredicatePushDown` 到 `extractStatsMetaFilters` 的边，以及测试对抽取函数的直接调用。由于常见方法名产生大量同名候选，具体构建和物理化边又以相邻源码核验，未把模糊候选当作本类型调用者。

## 错误处理与边界

公开优化入口返回 crate 的 `Result`，但当前本地逻辑不主动生成错误。表达式形态不支持、列名找不到、参数上下文缺失、参数读取失败、datum 转字符串失败、延迟常量存在等情况都通过 `Option::None` 降级为“不抽取”，让原谓词继续在上层求值。这一策略以不改变 SQL 语义为优先。

安全边界包括：EQ 必须恰有两个参数且其中一个是目标列；IN 第一项必须是目标列，剩余每项都必须可转字符串；OR 任一分支不可抽取就拒绝整个 OR；不支持 AND 作为单个递归节点（正常入口已按 CNF 拆分）；矛盾交集为空时不能删除原谓词。`findShowColumnIDs` 找不到匹配列时也原样返回全部谓词。

当前独立 Rust 测试覆盖矛盾交集、参数标记读取当前上下文、抽取器的 Go stub 契约和内存估算，但未直接覆盖成功的 EQ/IN/OR 抽取、大小写规则、延迟常量、缺失 schema/name、统计缓存与完整 `PredicatePushDown` 安装 Extractor 的路径；这些是扩展时应优先补齐的回归面。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或外部 I/O。`LogicalShow` 在规划阶段由可变借用独占修改；`ShowContents` 拥有字符串和 `CIStr`，过滤器拥有 `HashSet<String>`，因此没有借用源 AST 的生命周期耦合。

跨逻辑/物理阶段传递时，`ShowContents::clone` 复制载荷，`Box<dyn ShowPredicateExtractor>::clone` 调用 `CloneBox` 深克隆具体抽取器。`PhysicalShow::Clone` 在更换计划上下文时同样克隆抽取器，避免两个计划节点共享可变过滤状态。上下文本身由 `base::ContextRef` 管理，具体共享语义由基类定义；本文件只在抽取参数标记时临时借用其求值上下文，不保存该借用。

资源风险主要是过滤集合随 IN/OR 常量数量线性增长；多个谓词求交集会分配新集合。当前实现没有显式大小上限，也没有后台资源需要清理。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/planner/core/operator/logicalop/logical_show.go`。核心行为基本逐项对应：StatsMeta 才抽取；先 DB 后 Table；DB 转小写；多个谓词取交集；矛盾交集保留原谓词；EQ/IN/OR 的安全识别；参数标记按当前求值上下文读取；统计固定为 1；抽取器通用接口仍返回 Go 侧的空/false stub 值。

Rust 侧存在需要明确记录的移植差异：

- Go 的 `ShowContents.Tp` 保存完整 `ast.ShowStmtType`；Rust 压缩为 `ShowKind::{Other, StatsMeta}`，因此本结构自身无法区分其他 SHOW 子类型。
- Go 结构还包含 `Table`、`Column`、`User`、`Roles`、`Limit` 等字段；当前 Rust `ShowContents` 未承载这些字段。现有 Rust 构建器也只复制 Rust 已声明字段，不能据 Go 版本推断这些能力已移植。
- Go `MemoryUsage` 计入 `Roles` 容量，但没有计入所有字符串；Rust 没有 Roles，并额外显式计算 `CIStr` 的 O/L 字符串长度。两者都是近似估算而非完整堆占用。
- Go helper 用 `(value, ok)`，Rust 用 `Option`；语义均是失败时保留谓词。Rust 的 `PredicatePushDown` 简化了 Go 接口返回的 self plan，只返回剩余谓词，因为计划对象由 Rust trait 调用者原地持有。
- Go 的生成哈希目前同样只纳入 `LogicalSchemaProducer`；Rust 生成实现保持这一行为。这意味着仅修改 SHOW 内容不会改变 memo 相等性，扩展时不可擅自单边改变。

Go 测试的同目录哈希用例 `logicalop_test/hash64_equals_test.go::TestLogicalShowHash64Equals` 与 Rust 对应测试共同证明 schema 参与哈希/相等性；目标文件自身的细粒度 Rust 行为测试位于独立的 `logical_show_test.rs`。

## 扩展指南

- 新增 SHOW 参数时，同时检查解析器 `ShowStmt`、`ShowContents`、`logical_plan_builder_runtime.rs::build_show_runtime`、`PhysicalShow` 克隆/内存估算以及最终执行消费方；若是 Go 复刻字段，应保持字段语义而非只加占位。测试继续放在独立 `logical_show_test.rs` 或相应物理/构建器测试文件中。
- 新增可下推谓词形态时，优先扩展 `extractStatsMetaFilterValues` 及更窄的 helper。必须保证表达式可以完全由扫描侧过滤器等价表达；部分可识别的 OR 不得删除，矛盾集合也不得删除原谓词。
- 新增 StatsMeta 可过滤列时，在 `PredicatePushDown` 中明确其大小写规范与组合顺序，并确认执行侧能消费对应集合；不要仅凭列名存在就移除谓词。
- 改变假统计时同步检查 `LogicalShowDDLJobs`、物理 SHOW 的固定行数和基于 `ExpectedCnt` 的物理选择，避免逻辑/物理统计不一致。
- 改变 `ShowContents` 或 Extractor 的语义身份前，必须同时审查 `hash64_equals_generated.rs` 的生成器与 Go 生成规则；当前 memo 身份刻意只看 schema，单独手改生成文件会被再生成覆盖。
- 任何错误传播改动都要区分“无法优化”与“查询本身非法”：前者应保留谓词而非报错，参数求值的真实 SQL 错误是否应提前暴露则需与 Go 行为和执行阶段共同验证。
- 性能上关注大 IN/OR 的集合构建及交集复制；如需优化，应保持去重、全分支可抽取和矛盾保留这三个语义不变量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/planner/core/operator/logicalop/logical_show.rs` 定位到目标文件；`node --file ... --offset 1 --limit 1200` 读取全部 358 行；`query LogicalShow`、`query ShowStatsMetaPredicateExtractor`、`query ShowContents` 核对主要符号；`explore "LogicalShow PredicatePushDown extractStatsMetaFilters"` 核对目标内部调用及测试引用。
- Rust 源：`pkg/planner/core/operator/logicalop/logical_show.rs`；模块装配：`pkg/planner/core/operator/logicalop/lib.rs`；crate 边界：`pkg/planner/core/operator/logicalop/Cargo.toml`。
- 上下游源码：`pkg/planner/core/logical_plan_builder_runtime.rs::build_show_runtime`、`pkg/planner/cascades/pattern/pattern.rs`、`pkg/planner/cascades/old/implementation_rules.rs::ImplShow`、`pkg/planner/core/operator/physicalop/physical_show.rs`、`pkg/planner/core/operator/logicalop/hash64_equals_generated.rs`。
- Go 对照：`pkg/planner/core/operator/logicalop/logical_show.go`；Go/Rust 哈希对照测试：`pkg/planner/core/operator/logicalop/logicalop_test/hash64_equals_test.go` 与 `.rs`。
- 独立 Rust 测试：`pkg/planner/core/operator/logicalop/logical_show_test.rs`，覆盖矛盾过滤保留、参数标记当前值、抽取器接口和内存计数；`logical_datasource_aster_unit_test.rs` 另有 `findShowColumnIDs`/`getFakeStats` 的直接验证。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定 11 章结构命令、路径范围检查和人工事实复核作为交付验证。
