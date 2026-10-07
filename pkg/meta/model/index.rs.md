# `pkg/meta/model/index.rs`

## 文件定位

本文件定义 AsterSQL Rust 元数据模型中的“索引”部分：持久化的索引描述、列存索引附加信息、全局索引格式版本开关，以及 DDL 和外键校验需要的查找/覆盖判定。它不是物理索引读写实现；其职责是提供跨 DDL、会话、规划等模块共享的结构和纯判定逻辑。

编译归属并非由同目录顶层 `Cargo.toml` 直接逐文件声明。`pkg/meta/model/internal/group1/lib.rs:446-451` 以 `#[path = "../../index.rs"] mod index; pub use index::*;` 把本文件编入 `astersql-meta-model-group1`；`pkg/meta/model/lib.rs` 再通过 `pub use ::group_1::*` 对外暴露同一套类型。顶层 `pkg/meta/model/Cargo.toml` 说明公开 crate 是 `astersql-meta-model`，并以路径依赖聚合 group1–group4；其中 group1 拥有 column/index/table 的正式定义。

## 核心职责

- 表达索引元数据：`IndexInfo` 聚合标识、名称、索引列、schema/backfill 状态、唯一/主键/不可见/全局/MV 属性、partial 条件、全局索引格式版本和 Region 预拆分策略。
- 表达列存索引扩展：`VectorIndexInfo`、`InvertedIndexInfo`、`FullTextIndexInfo` 及 `ColumnarIndexType` 分别承载向量、倒排、全文索引的专有信息和统一分类。
- 在 SQL/存储表示之间做小型映射：距离函数与 `DistanceMetric` 双向表、全文分词器 SQL 名称转换、字段类型到倒排索引写入描述的转换。
- 为 DDL 提供元数据辅助逻辑：生成修改列期间不冲突的临时索引名，识别 changing/removing 名称，判断索引状态和类型，解析 partial index 条件。
- 为索引选择与外键建约束提供保守判定：按左前缀及完整列长度判断覆盖；外键场景还限制 partial predicate 只能是外键列上的 `IS NOT NULL`。
- 保存进程级的 Global Index V1 能力位，供创建新全局索引时决定键格式版本。

## 主要符号

- `DistanceMetric` 与 `DistanceMetricL2`、`DistanceMetricCosine`、`DistanceMetricInnerProduct`：序列化为字符串的距离度量。当前可索引函数映射只包含 cosine 和 L2；inner product 只有命名常量，不能据此认定已支持索引。
- `IndexableFnNameToDistanceMetric()` / `IndexableDistanceMetricToFnName()`：由 `OnceLock<HashMap<...>>` 惰性构造的只读映射。前者已被建表、会话 DDL 和查询表达式识别使用；仓库 Rust 引用搜索未找到后者的生产调用者。
- `VectorIndexKind` / `VectorIndexKindHNSW` 与 `VectorIndexInfo { Kind, Dimension, DistanceMetric }`：保存向量索引算法、维度和度量；`Kind` 是字符串别名，当前约定值为 HNSW，而非封闭枚举。
- `InvertedIndexInfo { ColumnID, IsSigned, TypeSize }` 与 `FieldTypeToInvertedIndexInfo(...) -> Option<_>`：把受支持的 MySQL 字段类型映射为读取所需列 ID、写入所需符号性和字节宽度；不支持的类型返回 `None`。
- `FullTextParserType`、三个 parser 常量、`SQLName()`、`String()`、`GetFullTextParserTypeBySQLName()`：在内部带版本名称（如 `STANDARD_V1`）与面向 SQL 的名称（如 `STANDARD`）之间转换；未知名称落为 `INVALID`。
- `ColumnarIndexType`：数值表示固定为 `NA=0`、`Inverted=1`、`Vector=2`、`Fulltext=3`，自定义 serde 保持 Go `uint8` JSON 编码；未知数值反序列化为错误。
- `RegionSplitPolicy { Lower, Upper, Regions, TimeZone }`：持久化索引 Region 预拆分上下界、目标数量及解释时间值的时区；`Clone()` 返回拥有独立 `Vec`/`String` 的副本。
- `IndexColumn { Name, Offset, Length, UseChangingType }`：索引列在 `TableInfo.Columns` 中的定位和前缀长度；`types::UnspecifiedLength` 表示整列。
- `IndexInfo`：本文件主类型。`Hash64` 和 `Equals` 以 `ID` 表示模型身份；`Clone` 深克隆 Rust 拥有的数据；其余方法提供名称状态、列引用、公开状态、列存类型、partial 条件等查询。
- `GenUniqueChangingIndexName()`：生成 `_Idx$_<原始名>_<序号>`，以 `CIStr.L` 集合执行大小写不敏感冲突检测。
- `FindIndexByColumns()` / `IsIndexPrefixCovered()`：返回候选切片中第一个满足左前缀覆盖的索引，或执行单个索引判定。
- `FindIndexByColumnsForForeignKey()` / `IsIndexPrefixCoveredForForeignKey()` / 私有 `isIndexConditionCoveredByForeignKeyCols()`：在普通覆盖规则上增加外键 partial index 安全条件。
- `FindIndexInfoByID()`、`FindIndexColumnByName()`：线性查找辅助函数；后者未命中时返回 `(-1, None)`。
- `SetGlobalIndexV1Supported()`、`GetGlobalIndexV1Supported()`、`InitGlobalIndexSupport()`：进程级能力位的写、读和 next-gen 初始化入口。

## 执行流程

1. 创建普通/列存索引时，调用方先建立 `IndexColumn` 与 `IndexInfo`。`pkg/ddl/create_table.rs:1950-2190` 使用距离函数映射构造向量信息、解析全文 parser，并通过 `FieldTypeToInvertedIndexInfo` 拒绝不支持的倒排列类型；`pkg/session/runtime/ddl.rs:3380-3455` 在会话 ALTER TABLE 路径执行同类转换。
2. 修改列时，`pkg/ddl/persistent_modify_column.rs:550-578` 克隆相关索引，分配新 ID，再调用 `GenUniqueChangingIndexName` 生成不会与表内索引重名的临时名，并更新被修改列的名称、偏移与 `UseChangingType`。
3. 创建全局索引时，`pkg/ddl/index.rs:set_global_index_version` 先把版本清零；只有能力位为真、索引为 global、表非 clustered 且键中确实需要 handle 时，才设置 `GlobalIndexVersionV1`。本文件只保存能力位和版本常量，不决定完整策略。
4. 查找覆盖索引时，`FindIndexByColumns` 顺序扫描候选并调用 `IsIndexPrefixCovered`。判定先检查索引列数，再逐位比较小写规范名、验证非负且有效的表列偏移，最后要求前缀长度为 `UnspecifiedLength` 或至少达到字段 `flen`。
5. 外键路径 `pkg/ddl/persistent_create_table.rs:332-360` 调用 `FindIndexByColumnsForForeignKey`。若索引没有 partial 条件，普通覆盖即足够；若有条件，`ConditionExpr` 把条件包装成 `select <condition>` 交给 parser，并且只接受 AST 形态“某个外键列 `IS NOT NULL`”。解析失败或其他表达式均按不安全处理。
6. `IndexInfo::GetColumnarIndexType` 以 Vector、Inverted、Fulltext 的固定优先级返回类型；正常构造应只设置一种附加信息，但函数本身不会拒绝多个字段同时为 `Some`。

## 数据与状态

`IndexInfo` 是可 serde 持久化的拥有型结构。重要 JSON 键包括 `idx_name`（反序列化也接受旧别名 `name`）、`idx_cols`、`state`、`backfill_state`、三类列存信息、`condition_expr_string`、`global_index_version` 和 `region_split_policy`。`#[serde(default)]` 使缺失字段按 Rust 默认值补齐；版本 0、`None` 字段和空时区按属性省略序列化。

`IndexInfo::Equals` 只比较 ID，其他字段变化不改变这里定义的对象身份；`Hash64` 同样只写入 ID，二者必须保持一致。`Clone` 依赖 Rust 拥有型字段的派生深克隆，不共享 `Columns`、`AffectColumn`、`RegionSplitPolicy` 内部容器。

全局能力状态存放在 `OnceLock<AtomicBool>`。首次访问以 `kerneltype::IsNextGen()` 初始化，之后可由 setter 覆盖。Acquire/Release 顺序保证线程间读写可见，但它只是单进程能力快照，不负责集群节点探测、持久化或共识。

两个函数映射分别保存在独立的 `OnceLock<HashMap>` 中，初始化后仅通过 `&'static HashMap` 暴露，调用者不能获得可变引用。其余查找/判定只读取传入数据，无缓存和隐式状态。

## 依赖与调用关系

下游依赖来自 `internal/group1/Cargo.toml` 及本文件导入：parser/AST 用于 `CIStr` 和 partial 条件解析，parser-mysql/types 提供字段类型、flag、`flen` 与 `UnspecifiedLength`，planner-base 提供 `Hasher`，kerneltype 提供 next-gen 判断，serde 提供持久化表示；`TableInfo`、`SchemaState`、`BackfillState` 和 removing 前缀来自同一个 group1 模型边界。

已核实的上游生产调用边包括：

- `pkg/ddl/create_table.rs` → 距离函数映射、全文 parser 转换、倒排字段转换；
- `pkg/session/runtime/ddl.rs` → 向量/倒排索引构造，`pkg/session/runtime/query.rs` → 可索引向量函数识别；
- `pkg/ddl/persistent_modify_column.rs` → `GenUniqueChangingIndexName`；
- `pkg/ddl/index.rs` → `GetGlobalIndexV1Supported` 与全局索引版本常量；
- `pkg/ddl/persistent_create_table.rs` → `FindIndexByColumnsForForeignKey`，用于引用表索引校验。

RustCodeGraph 的文件节点报告本文件被 22 个文件使用，并点名 `pkg/ddl/create_table.rs`、`pkg/ddl/index.rs`、`pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/domain/infosync/info.rs` 等。精确 `callers/callees` 命令在本次环境中未返回边，因此上述函数级边由 `rg` 引用和调用点源码补证；不能从无输出推断 API 无调用。对 `IndexableDistanceMetricToFnName`、`FindIndexByColumns`、`FindIndexInfoByID`、`InitGlobalIndexSupport` 的生产 Rust 引用搜索仅命中定义或内部链路，当前接线应视为未使用/预留。

## 错误处理与边界

- `FieldTypeToInvertedIndexInfo` 用 `None` 表示不支持的字段类型；调用方负责转成面向 DDL 的错误。整数宽度和符号性按 MySQL 类型/unsigned flag 计算，Year、Enum、Set 和时间类型有显式固定规则。
- `ColumnarIndexType` 对 0–3 之外的 JSON 数值返回 serde 错误，避免产生未知枚举态。
- `ConditionExpr` 将 parser 错误转为字符串；若语句不是 `SelectStmt`、没有首个字段或字段没有表达式，也返回错误。外键覆盖判定吞并该错误并返回 `false`，采取拒绝不安全索引的保守策略。
- `IsIndexPrefixCovered` 对列数不足、名称/顺序不符、负偏移、越界偏移和不足以覆盖完整字段的 prefix length 均返回 `false`；空 `columns` 会被任意索引真空覆盖，这与 `all` 的语义一致，调用方应避免把空外键列集合当作有效约束。
- `FindIndexByColumns*` 只返回第一个匹配项，不排序也不比较成本、可见性或 schema 状态；调用方若需要这些约束，必须在传入候选前过滤。
- `GenUniqueChangingIndexName` 的后缀循环理论上无界，但对有限索引集合必会找到空位；它只在当前 `TableInfo.Indices` 快照内保证唯一，不提供跨线程事务性预留。
- `GetChangingOriginName` 按最后一个下划线去掉后缀；不符合生成约定的字符串可能被截断，因此只应对 changing 临时名使用。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或外部资源。`OnceLock` 保证两个映射和能力原子的单次线程安全初始化；映射随后存活至进程结束。`AtomicBool` 使用 Release 写/Acquire 读，允许并发 DDL 路径看到已发布的能力变化，但多个测试或调用者并行修改仍是共享全局状态，必须在测试中恢复原值并避免无序并发。

`ConditionExpr` 每次调用都新建 parser 并产生拥有型 AST，没有跨调用缓存。返回的 `&IndexInfo`/`&IndexColumn` 引用受输入切片生命周期约束，不转移所有权；`Clone` 方法用于调用方需要独立修改副本的场景。

## 与 Go 版本的对应关系

主要语义直接对照 `pkg/meta/model/index.go`：常量值、JSON 字段名、字段类型到倒排信息的映射、全文 parser 名称、列存类型编号、临时命名、索引身份、左前缀覆盖和外键 partial predicate 限制均保持一致。`pkg/meta/model/index_test.rs` 复刻 `pkg/meta/model/index_test.go` 的列顺序与 partial index 正反例，并额外验证 Rust JSON 表示和旧元数据兼容。

已确认的语言层差异如下：

- Go 的 `IndexInfo`/`IndexColumn` 使用指针及 nil；Rust 使用拥有值、`Option` 和借用，因此 Rust `Equals(&dyn Any)` 不表达 Go `(*IndexInfo)(nil)` 的比较分支，但非 nil 对象仍按 ID 等价。
- Rust 的 `HasColumnInIndexColumns` 和 `IsIndexPrefixCovered` 显式拒绝负数/越界 offset；Go 对部分异常 offset 可能索引越界。该差异是 Rust 的安全边界，不改变有效元数据语义。
- Go 的 `ColumnarIndexType` 是开放的 `uint8` 别名；Rust 反序列化只接受四个已知编号。
- Go 包 `init()` 在 next-gen 时设置能力位；Rust 的惰性原子首次读取即以 `IsNextGen()` 初始化，并另有未发现生产调用的显式 `InitGlobalIndexSupport()`。因此不要假定后者像 Go `init()` 一样自动执行。
- Rust `VectorIndexInfo` 使用 `#[serde(default)]`，测试证实缺少新 `kind` 字段的旧 JSON 会得到空字符串；Go 当前结构没有等价的显式默认注解，但零值结果相同。
- Rust 对 `IndexInfo.Name` 反序列化额外接受 `name` 别名，这是兼容输入，不改变输出仍使用 `idx_name`。

## 扩展指南

- 新增距离度量时，应同时审查 `DistanceMetric*`、两个方向的映射、DDL/查询函数名识别、下游存储协议命名，以及独立 Rust 测试；仅新增常量不会使其可索引。
- 新增向量算法、全文 parser 或列存类型时，要保持 JSON/数值兼容并同步 `SQLName`、反序列化分支、`IndexInfo::IsColumnarIndex`、`GetColumnarIndexType` 和 Go 对照。不要复用既有枚举编号表达新语义。
- 扩展倒排支持类型时，修改 `FieldTypeToInvertedIndexInfo` 并在独立测试中覆盖 signed/unsigned、宽度及不支持类型；同时核对建表与 ALTER TABLE 两个调用路径的错误行为。
- 修改 prefix coverage 时，先保持左前缀顺序、有效 offset 和完整列长度三个不变量；外键规则还必须符合 MATCH SIMPLE 的语义。同步更新 `pkg/meta/model/index_test.rs`，不要把测试嵌入生产文件，并对照 `pkg/meta/model/index_test.go`。
- 新增 `IndexInfo` 持久化字段时，应给出 serde 名称和旧元数据默认值，核对 Clone、Go JSON tag、模型身份是否受影响，并评估 infoschema/domain/DDL 消费者的兼容性。
- 修改全局索引版本时，本文件只适合承载常量与能力边界；选择策略在 `pkg/ddl/index.rs:set_global_index_version`。需同步评估滚动升级兼容、键格式读写路径和共享原子测试隔离。
- Region split 字段变化需保持旧 JSON 可读，并确认 `TimeZone` 缺省时的历史行为；范围上下界是字符串表示，解释逻辑不在本文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/meta/model` 覆盖 `index.rs`、`index_test.rs`、`index.go`、`index_test.go`；`node --file pkg/meta/model/index.rs --offset 1/421` 读取完整 553 行并报告 22 个使用文件；`query` 定位 `IndexInfo`、`IndexColumn`、`FieldTypeToInvertedIndexInfo`、`GenUniqueChangingIndexName`、`IsIndexPrefixCovered`。`callers/callees` 精确查询超时且无输出，未将其当作否定证据。
- 源与装配：`pkg/meta/model/index.rs`；`pkg/meta/model/internal/group1/lib.rs:446-451`；`pkg/meta/model/internal/group1/Cargo.toml`；`pkg/meta/model/lib.rs`；`pkg/meta/model/Cargo.toml`。
- Rust 调用点：`pkg/ddl/create_table.rs`、`pkg/ddl/index.rs`、`pkg/ddl/persistent_create_table.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/session/runtime/ddl.rs`、`pkg/session/runtime/query.rs`。
- Rust 独立测试：`pkg/meta/model/index_test.rs` 覆盖索引左前缀、外键 partial 条件、全局能力位恢复、列存类型 JSON、HNSW 与旧向量元数据；`pkg/meta/model/bdr_1_aster_unit_test.rs` 和 `pkg/meta/model/dependency_tests.rs` 提供额外模型集成断言。
- Go 对照：`pkg/meta/model/index.go` 与 `pkg/meta/model/index_test.go`。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核未修改 Rust、Go、Cargo 或总计划。
