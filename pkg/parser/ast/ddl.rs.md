# `pkg/parser/ast/ddl.rs`

## 文件定位

本文件属于 `astersql-parser-ast` crate 的公开 `ddl` 子模块，由 [`lib.rs`](./lib.rs) 中的 `pub mod ddl` 暴露。它提供一组轻量 DDL 数据结构以及把这些结构还原成 SQL 文本的函数，覆盖数据库选项、索引选项、放置策略、资源组、表名/外键、部分 DROP/TRUNCATE/ALTER/FLASHBACK/SEQUENCE 语句和少量表选项。

它不是该 crate 中完整 DDL AST 的唯一实现。面向解析器、`Node`/`Visitor` 和执行层的 Go 风格主类型大多直接定义在 [`lib.rs`](./lib.rs)；例如根模块也有 `IndexOption`、`PlacementOption` 和更完整的 `TableOption`。已确认的生产接线是根 `IndexOption::restore_with_special_comments`（`lib.rs`）先把根类型字段转换成本文件的 `ddl::IndexOption`，再调用本文件的同名方法完成 SQL 格式化。其他本文件语句包装类型目前未在非测试 Rust 代码中发现直接使用点，不能据此视为解析器主 AST 的替代品。

[`Cargo.toml`](./Cargo.toml) 将 crate 入口设为 `lib.rs`，并声明 `parser-auth`、`parser-charset`、`parser-mysql`、`parser-types`、`serde`、`serde_json`、`url` 等 crate 级依赖；本文件自身只直接使用标准库字符串/集合操作及根模块的 `SplitOption`、`Node`。

## 核心职责

1. 规范化 SQL 标识符和字符串字面量：内部 `quote_name` 对反引号加倍，`quote_string` 对反斜杠及单引号转义。
2. 用枚举和载荷结构表达有限的 DDL 选项，并按稳定顺序输出 SQL：`DatabaseOption`、`IndexOption`、`PlacementOption`、`ResourceGroupOption`、`SequenceOption`、`TableOption`。
3. 为若干可独立格式化的 DDL 片段或语句提供 `restore`：表名、索引部件、外键引用、重命名对、DROP/TRUNCATE/ALTER DATABASE、放置策略、FLASHBACK DATABASE、资源组和序列。
4. 处理 TiDB/AsterSQL 扩展格式：放置策略、TTL、预分裂 Region、自动预分裂、TiFlash 副本，以及用 `/*T![feature] ... */` 包装的特殊注释。

本文件只做结构保存与文本还原，不负责 SQL 词法/语法分析、名称解析、语义校验、DDL job 构造或实际模式变更。

## 主要符号

- `quote_name`、`quote_string`：本文件私有的两种引用辅助函数。前者总是生成反引号标识符，后者总是生成单引号字符串。
- `CharsetOpt`、`NullString`：兼容性值对象；在本文件中没有行为方法，也未发现本文件内使用点。
- `DatabaseOptionType`、`TiFlashReplicaSpec`、`DatabaseOption`：区分字符集、排序规则、加密、TiFlash 副本和放置策略。`try_restore` 是可失败入口；`restore` 用 `expect` 把合法性当作调用方不变量。
- `ReferOptionType`：外键 `ON DELETE`/`ON UPDATE` 动作到关键字的映射，`NoOption` 输出空串。
- `IndexType`、`IndexVisibility`、`PrimaryKeyType`、`IndexOption`：描述索引算法、可见性、聚簇属性及附加选项。`is_empty` 判定是否需要输出；`restore_with_special_comments` 决定顺序和 TiDB 特殊注释。
- `PlacementOptionType`、`PlacementSchedule`、`PlacementOption`、`restore_placement_options`：表达副本数、Region、约束、调度和策略引用；集合函数按输入顺序以空格连接。
- `BurstableType`、`ResourceGroupPriority`、`ResourceUnitType`、`ResourceGroupOption`、`restore_resource_group_options`：表达 RU、优先级、CPU/IO 配额和突发模式；集合函数按输入顺序以逗号加空格连接。
- `TableName`、`TableToTable`：分别恢复 ``schema.table`` 限定名和 `old TO new` 重命名对。
- `IndexPartSpecification`：在“列名 + 可选前缀长度”和“原始表达式文本”之间二选一，并可追加 `DESC`。
- `ReferenceDef`：组合引用表、引用列、可选 `MATCH`、`ON DELETE` 和 `ON UPDATE`。
- `DropTableStatement`、`TruncateTableStatement`、`AlterDatabaseStatement`、`PlacementPolicyStatement`、`DropPlacementPolicyStatement`、`FlashBackDatabaseStatement`、`ResourceGroupStatement`、`SequenceStatement`：轻量语句包装及 SQL 还原器。
- `ColumnPosition`：优先输出 `FIRST`，否则输出 `AFTER name`，均未设置时为空。
- `SequenceOption`：覆盖增量、起始值、最小/最大值、缓存和循环开关。
- `TableOption`、`TableRestoreFlags`：本文件只覆盖引擎属性、存储类别、事务起点、放置策略、TTL/TTL 开关和预分裂；flags 控制特殊注释、跳过放置策略及强制关闭 TTL。

## 执行流程

典型生产流程是：解析器在根 AST 中保存 Go 风格字段；需要恢复索引选项时，`lib.rs::IndexOption::restore_with_special_comments` 将根枚举、`CIStr`、`ExprNode` 和 `SplitOption` 映射为本文件的 `ddl::IndexOption`，其中条件表达式经 `Node::Text` 转为文本；随后本文件按固定顺序收集非默认选项并以空格拼接。

`IndexOption::restore_with_special_comments` 的输出顺序为：列存副本提示、聚簇属性、块大小、索引算法、解析器、注释、全局索引、可见性、预分裂、辅助引擎属性、条件。若同时有 `split_opt` 和 `auto_pre_split`，代码先处理 `split_opt`，所以手工指定分裂规则优先。分裂规则又分三路：仅 `Num`、`BETWEEN lower AND upper REGIONS n`、`BY (values...)`；表达式通过根 `Node::Text` 取得原始文本。

其余还原器遵循相同模式：构造器把类型和值放入结构；单项 `restore` 根据枚举选择关键字、引用方式和可选分支；集合/语句 `restore` 按保存顺序拼接。例如 `ReferenceDef::restore` 先输出表和列，再依次追加 `MATCH`、`ON DELETE`、`ON UPDATE`；`SequenceStatement::restore` 先依据 `create` 选择 `CREATE`/`ALTER` 及相应存在性子句，再顺序输出所有 `SequenceOption`。

特殊注释不是统一的全语句 RestoreCtx，而是局部字符串包装：索引预分裂使用 `pre_split`/`auto_presplit`，表放置使用 `placement`，TTL 使用 `ttl`。`skip_placement` 只让 `TableOption::PlacementPolicy` 返回空串；调用方负责避免由空片段产生多余分隔符。

## 数据与状态

所有类型都是拥有数据的普通 Rust 值，没有内部可变性或全局状态。名称、表达式和选项值主要存为 `String`，列表存为 `Vec`，缺省载荷用 `Option`、布尔值、零值或 `Default` 表示。多数结构派生 `Clone`、`Debug`、`Eq`、`PartialEq`，适合在 AST 转换和测试中按值比较。

重要不变量包括：

- `DatabaseOption.tp == SetTiFlashReplica` 时 `tiflash_replica` 必须为 `Some`；`tp == None` 不是可恢复选项。
- `IndexPartSpecification` 设计上应恰有 `column` 或 `expression` 之一；代码在无列时用空表达式兜底，因此类型本身没有强制这一约束。
- `PlacementOption` 的 `str_value`/`uint_value` 由 `tp` 决定哪一个有效；公开的 `string`、`number` 和专用构造器帮助维持配对，但字段仍可由调用方直接构造。
- `ResourceGroupOption::priority` 把 Low/Medium/High 编为 1/8/16；恢复时只有 16 和 8 分别映射 HIGH/MEDIUM，其他数值一律输出 LOW。
- `TableRestoreFlags` 是一次恢复调用的值参数，不会回写 AST；`force_ttl_enable_off` 只影响 `TtlEnable` 的输出。

## 依赖与调用关系

上游方面，`lib.rs` 通过 `pub mod ddl` 暴露本文件，并在根 `IndexOption::restore_with_special_comments` 中调用 `ddl::IndexOption::restore_with_special_comments`。`pkg/parser/ast/ddl_test.rs` 直接覆盖本文件的公开 API；`ddl_2_aster_unit_test.rs` 同时对照根类型和本文件类型，验证自动预分裂与 Go 移植语义。仓库文本搜索未发现本文件其他轻量语句类型的非测试直接调用者。

下游方面，本文件仅调用私有引用函数、同文件的各项 `restore`，以及根模块的 `SplitOption` 和 `Node::Text`。`IndexOption` 因 `split_opt: Option<crate::SplitOption>` 与根 AST 相连；其余逻辑不直接依赖 Cargo.toml 中的外部 crate。

应用主链应区分两层：`pkg/parser/parser_actions/ddl.rs` 构造的是 `parser_ast` 根模块的 Go 风格 AST，`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/dispatch.rs` 等消费的也主要是根类型；本文件为其中部分格式化行为提供辅助，而不是执行 DDL。完整访问者子节点关系还定义在 [`walk.rs`](./walk.rs)，但本文件自己的轻量类型没有实现根 `Node`/`Visitor` 协议。

RustCodeGraph `files --filter pkg/parser/ast/ddl.rs` 报告该文件含 161 个索引符号，并给出 75 个“used by”文件的文件级关联；精确 `query` 能定位 `ddl.rs::restore_placement_options` 和 `ddl.rs::SequenceStatement`。但本次 `callers`/`callees` 对这些节点持续超时，因此具体直接边以 `rg` 对限定符和符号的搜索补证，未把文件级关联误当成符号级调用。

## 错误处理与边界

`DatabaseOption::try_restore` 是本文件唯一返回 `Result` 的还原接口：`None` 返回 `invalid DatabaseOptionType: 0`，TiFlash 类型缺失规格返回明确错误。`DatabaseOption::restore` 对该结果调用 `expect("valid database option")`，所以面对外部或未经校验的数据应优先使用 `try_restore`，否则会 panic。

其他枚举恢复大多假定值由受控构造器或解析器产生。`PlacementOption::restore` 对无法归入字符串、数值或策略的分支使用 `unreachable!`；当前枚举的所有变体均被前置分支或后置 `match` 覆盖，但新增变体时必须同步更新该方法。`IndexPartSpecification::restore` 在 `column == None` 且 `expression == None` 时输出 `()`，这是一种容错格式而非有效 SQL 保证。

本文件不做语义验证：不会验证空表列表、空名称、非法 `MATCH` 文本、负数序列边界、相互冲突的序列选项、TTL 表达式、放置约束格式或资源配额单位。它也不使用完整 RestoreCtx，因此无法覆盖 Go 版所有 flag、错误注解和特性包装。调用者必须在更上层保证结构合法，并认识到这里的字符串字段可能包含已格式化表达式。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络资源。所有 `restore` 都只借用 `&self`，在局部 `String`/`Vec<String>` 中构建结果，因此同一不可变值可由多个线程并发读取；是否可跨线程传递由字段类型自动决定，本文件没有额外同步协议。

临时字符串在调用结束后释放，返回的 `String` 由调用方拥有。时间和空间开销主要与选项数和输出长度线性相关；多处 `collect::<Vec<_>>().join(...)` 会分配中间向量。当前用途是 DDL 文本恢复，未发现缓存或长期资源生命周期。

## 与 Go 版本的对应关系

主要对照是 [`ddl.go`](./ddl.go)。Rust 保留了 Go 中 `DatabaseOption`、引用动作、`IndexOption`、`PlacementOption`、`ResourceGroupOption`、`SequenceOption`、`ColumnPosition` 及多种 DDL 语句的关键关键字、字段顺序和引用规则；[`ddl_test.rs`](./ddl_test.rs) 也明确以 Go 用例为来源验证常见输出。

两者不是一一等量移植：

- Go `ddl.go` 超过六千行，完整类型实现 `Node`、`Accept` 和基于 `format.RestoreCtx` 的错误传播/flag 行为；本文件约一千行，是轻量、拥有字符串的格式化子集，不实现访问者。
- Go `TableOption` 覆盖大量 MySQL/TiDB 表选项，本文件仅有七个变体；完整 Rust 根模块仍保留更广的 Go 风格 `TableOption` 供解析器和执行层使用。
- Go `ResourceGroupOption` 还包含 runaway/background 选项，本文件只覆盖 RU、优先级、CPU/IO 和 burstable。
- Go `SequenceOption` 支持 `RESTART`/`RESTART WITH`，本文件没有对应变体。
- Go 放置恢复可按 RestoreCtx 跳过规则、包装整条语句并对未知枚举返回错误；本文件的 `PlacementOption::restore` 始终输出局部片段，特殊注释仅由部分外层类型自行处理。
- Go 的 `AlterDatabaseStmt` 会避免在跳过全部放置选项后产生非法空语句，并可把全放置语句整体包进特殊注释；本文件 `AlterDatabaseStatement` 不接收相应 flags。

因此扩展时应先判断修改目标是“解析/遍历/执行所需的根 AST”，还是“本文件的轻量 SQL 格式化层”；若 Go 行为属于完整主链，不能只在本文件增加同名字段便宣称移植完成。

## 扩展指南

新增数据库、放置、资源组、序列或表选项时，应同时更新对应枚举、载荷结构、构造器（如有）、`restore` 分支和独立测试。若值存在缺失或非法组合，优先提供可返回错误的接口，不要把外部输入直接送入会 panic 的便捷 `restore`。

修改索引选项时需同步检查两层：本文件 `ddl::IndexOption` 的字段、`is_empty` 与输出顺序，以及 `lib.rs::IndexOption::restore_with_special_comments` 的根类型到格式化类型映射。涉及表达式时还要确认 `Node::Text` 是否足以保留所需 SQL；涉及预分裂时保持 `SplitOpt` 优先于 `AutoPreSplit`，并在 [`ddl_2_aster_unit_test.rs`](./ddl_2_aster_unit_test.rs) 覆盖数字、范围、值列表和特殊注释。

新增轻量语句类型前，应先复用根模块现有类型，避免形成第三套 DDL 表示。若确实需要接入解析主链，必须在 `pkg/parser/parser_actions/ddl.rs` 验证构造路径，并在根 `Node`/`Visitor`/`walk.rs` 体系中补齐遍历；只增加本文件结构不会自动被解析器或执行器消费。

测试应继续放在独立文件而非 `ddl.rs` 内：格式化层用 [`ddl_test.rs`](./ddl_test.rs)，涉及根类型桥接或 Go 合并语义用 [`ddl_2_aster_unit_test.rs`](./ddl_2_aster_unit_test.rs)；还应对照 [`ddl_test.go`](./ddl_test.go) 的相应用例。兼容风险集中在关键字顺序、空格/逗号、标识符与字符串转义、特殊注释 feature 名、flag 组合和默认值；性能风险主要是为高频恢复增加不必要克隆或中间集合。

## 验证依据

- 源码与模块边界：[`ddl.rs`](./ddl.rs) 全部 1050 行、[`lib.rs`](./lib.rs) 的 `pub mod ddl` 及根 `IndexOption` 桥接、[`Cargo.toml`](./Cargo.toml) 的 crate 入口/依赖/Go 包元数据。
- Rust 独立测试：[`ddl_test.rs`](./ddl_test.rs) 覆盖名称转义、索引部件、引用动作、索引选项、外键、DROP/TRUNCATE、列位置、序列、ALTER DATABASE、放置策略、FLASHBACK、TTL、预分裂和资源组；[`ddl_2_aster_unit_test.rs`](./ddl_2_aster_unit_test.rs) 覆盖自动预分裂优先级、数据库选项错误和 Go 顺序。
- Go 对照：[`ddl.go`](./ddl.go) 中同名/对应类型及 Restore 实现；[`ddl_test.go`](./ddl_test.go) 中 DDL restore、特殊注释、跳过放置、TTL、序列与资源组用例。`ddl_partition_visitor_test.go` 关注完整根 AST 的分区访问顺序，不是本文件轻量类型的直接测试。
- 上下游搜索：`pkg/parser/parser_actions/ddl.rs` 构造根 AST；`pkg/session/runtime/ddl.rs` 和 `pkg/session/runtime/dispatch.rs` 消费根选项；限定搜索只确认 `lib.rs::IndexOption::restore_with_special_comments -> ddl::IndexOption::restore_with_special_comments` 这一生产直接桥接。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被索引为 161 个符号。精确查询定位了 `ddl.rs::restore_placement_options`（第 477 行）和 `ddl.rs::SequenceStatement`（第 956 行），但符号级 callers/callees 查询超时，故直接调用证据由源码搜索补齐。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行任务规定的 11 章节结构检查，并人工检查只新增本文档、没有把未接线子集描述成完整 DDL 主链。
