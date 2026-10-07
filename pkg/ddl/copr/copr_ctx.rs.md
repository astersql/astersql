# `pkg/ddl/copr/copr_ctx.rs`

## 文件定位

`copr_ctx.rs` 是 `astersql-ddl-copr` crate 的核心实现文件。它由
`pkg/ddl/copr/lib.rs` 声明为私有模块，再通过 `pub use copr_ctx::*` 从 crate
根导出公共类型和函数。该文件不执行 DDL job，也不直接发起存储请求；它负责在
索引回填扫描前，把表、索引、条件和行 handle 元数据整理成统一的
`CopContext`。下游可据此确定要读取的列、输出列位置以及可下推的索引条件。

这属于 DDL reorg/backfill 的数据扫描准备层，而不是 owner、job 状态机、schema
version 或 checkpoint 持久化层。Go 生产链中，
`pkg/ddl/backfilling_txn_executor.go::NewReorgCopContext` 创建表达式上下文和
push-down flags 后调用同路径 Go 实现 `copr.NewCopContext`；其上游包括
`NewAddIndexIngestPipeline` 和 `NewWriteIndexToExternalStoragePipeline`。当前 Rust
仓库中没有检索到这些生产入口对本文件 `NewCopContext` 的直接调用，故本文只把
这条链描述为 Go 对照和预期接线位置，不声称 Rust 已进入该生产主链。

## 核心职责

1. 用 `CopContextBase` 保存一次索引回填扫描共享的表信息、表达式设置、请求来源、
   实际读取列、字段类型、handle 输出位置及虚拟列信息。
2. 用 `CopContextSingleIndex` 和 `CopContextMultiIndex` 统一单索引与一次扫描回填
   多索引的查询接口（`CopContext` trait）。
3. 计算最小读取列闭包：索引列、条件引用列、生成列的传递依赖，以及主键或隐藏
   `_tidb_rowid` handle。
4. 为条件索引提取列引用并做轻量语法校验；遇到虚拟生成列时禁止条件下推。
5. 建立“表列 ID—扫描输出位置”的映射，供行解码、handle 构造和索引键构造使用。

本文件只构造不可变的值对象；它不访问网络、磁盘、事务、系统表或 DDL checkpoint。

## 主要符号

- `ExtraHandleID = -1`、`ExtraHandleName = "_tidb_rowid"`：无聚簇索引表的隐藏
  handle 标识。
- `CopError(String)`：构造和条件解析阶段的错误载体，实现 `Display` 与
  `std::error::Error`。
- `FieldType`、`ColumnInfo`、`IndexColumn`、`IndexInfo`、`TableInfo`：本 crate
  自包含的轻量元数据模型。`ColumnInfo::Dependences` 表示生成列依赖，
  `VirtualExpr` 标记虚拟生成列，`PrimaryKey` 用于定位 PK handle 列。
- `BuildContext`：当前为空的迁移占位类型；`Expression` 只保留原条件文本和引用
  列 ID，并不是完整可执行表达式树。
- `ExprColumn`、`FieldName`、`Schema`：扫描输出 schema 的轻量表示。
- `CopContextBase`：共享扫描上下文。重要字段间的不变量是
  `ColumnInfos`、`FieldTypes`、`ExprColumnInfos` 按同一列集合构造；handle 与虚拟列
  偏移都指向该输出集合。
- `CopContext`：提供 `GetBase`、`IndexColumnOutputOffsets`、`IndexInfo` 和
  `GetCondition` 四个查询接口。
- `NewCopContext`：公共工厂；索引数恰为 1 时创建单索引实现，否则创建多索引实现。
  因而空索引列表也进入多索引分支。
- `NewCopContextBase`：核心归一化函数，负责读取列闭包、handle、输出列和虚拟列信息。
- `NewCopContextSingleIndex` / `NewCopContextMultiIndex`：合并索引列与条件引用列，
  创建 base，并保存每个索引的输出偏移。
- `condition_columns` / `parse_condition`：前者扫描 SQL 条件文本并解析列引用，后者
  转成轻量 `Expression`，同时拒绝向存储层下推引用虚拟列的条件。
- `fillUsedColumns`：展开索引列及生成列依赖的传递闭包。
- `resolveIndicesForIndex`、`resolveIndicesForHandle`、
  `collectVirtualColumnOffsetsAndTypes`：输出位置与虚拟列辅助计算。
- `CopContextBase::GetSchemaAndNames`：克隆表达式列、把 `Index` 重排为实际输出顺序，
  并生成表名/列名；隐藏 handle 使用固定名称。

## 执行流程

典型入口是 `NewCopContext(expr, flags, table, indexes, source, collate)`：

1. 工厂按 `indexes.len() == 1` 选择单索引或多索引实现。
2. 对每个索引复制其 `Columns`；若 `IndexInfo::HasCondition` 为真，
   `condition_columns` 从 `ConditionExprString` 提取额外列。
3. `dedup` 按列偏移去重并保留首次出现顺序，然后调用 `NewCopContextBase`。
4. `fillUsedColumns` 先校验每个索引列偏移，再以显式栈展开 `Dependences`；已见列 ID
   会阻止重复处理，因此依赖环不会无限循环。
5. `NewCopContextBase` 按表的 handle 模式补列：
   - `PKIsHandle`：加入 `GetPkColInfo` 找到的主键列；
   - `IsCommonHandle`：要求 `PrimaryIndex` 存在，加入其全部列及生成列依赖；
   - 两者皆否：在已用表列末尾追加 `_tidb_rowid`。
6. 已用普通列始终按 `TableInfo::Columns` 的表定义顺序输出，而不是按索引列输入顺序；
   隐藏 handle 最后追加。
7. 基于输出列构造字段类型、表达式列、handle 偏移和虚拟列偏移/类型。
8. 单索引上下文保存一组索引列输出偏移；多索引上下文按 `allIndexInfos` 顺序保存
   多组偏移，并按索引 ID 查询。
9. 调用 `GetCondition` 时再次解析条件。单索引返回一个条件；多索引要求每个索引
   都有可下推条件，再用带括号的文本以 `OR` 连接，并对引用列 ID 排序去重。任一
   索引无条件或引用虚拟列时返回 `None`，含义是不能用组合条件过滤，必须扫描全部数据。

## 数据与状态

`CopContextBase` 构造后没有可变接口，调用者通过共享引用读取它。其主要数据关系为：

- `ColumnInfos` 是索引回填所需的最小列闭包，包含索引列、条件列、生成列依赖和
  handle；非聚簇表的 `_tidb_rowid` 不存在于原 `TableInfo::Columns` 中。
- `ExprColumnInfos` 里的初始 `Index` 使用原表列 `Offset`；
  `GetSchemaAndNames` 返回克隆并把 `Index` 改成紧凑输出位置 `0..n`。
- `HandleOutputOffsets` 按 handle ID 的请求顺序生成。common handle 因此保留主键索引
  列顺序，不会被表物理顺序取代。
- `VirtualColumnsOutputOffsets` 与 `VirtualColumnsFieldTypes` 一一对应，只收集
  `VirtualExpr == true` 的输出列。
- `PrimaryKeyInfo` 只在 common-handle 分支保存；`PKIsHandle` 和隐藏 row ID 分支为
  `None`。
- `PushDownFlags`、`RequestSource`、`UseNewCollate` 仅被存入上下文；本文件不解释
  flags、不消费 request source，也没有用 `UseNewCollate` 改变轻量条件解析行为。

## 依赖与调用关系

Rust 文件的直接标准库依赖只有 `std::collections::HashSet` 和 `std::fmt`。内部调用边
经 RustCodeGraph 核对如下：

- `NewCopContext` → `NewCopContextSingleIndex` 或 `NewCopContextMultiIndex`；
- 两个具体构造器 → `condition_columns`、`dedup`、`NewCopContextBase`、
  `resolveIndicesForIndex`；
- `NewCopContextBase` → `fillUsedColumns`、`TableInfo::GetPkColInfo`、
  `TableInfo::HasClusteredIndex`、`resolveIndicesForHandle`、
  `collectVirtualColumnOffsetsAndTypes`；
- 两个 `GetCondition` 实现 → `parse_condition` → `condition_columns`。

`pkg/ddl/copr/Cargo.toml` 定义 crate 名 `astersql-ddl-copr`，入口为 `lib.rs`，并记录
Go 包映射 `pkg/ddl/copr`。其中 expression、meta、parser、table、types 等迁移依赖
位于 `target.'cfg(any())'.dependencies`，永不启用；这与本文件当前使用自包含轻量类型
一致。根 workspace 收录该 crate，并由根 facade 重新导出；`pkg/ddl/Cargo.toml` 对它的
依赖位于 Windows 条件区。仓库 Rust 搜索只发现 crate 导出、manifest 依赖和独立测试，
未发现生产 Rust 代码直接调用这些构造器。

Go 侧真实调用链是
`NewAddIndexIngestPipeline` / `NewWriteIndexToExternalStoragePipeline` →
`NewReorgCopContext` → `copr.NewCopContext`，位于
`pkg/ddl/backfilling_operators.go`、`pkg/ddl/backfilling_txn_executor.go` 和
`pkg/ddl/copr/copr_ctx.go`。

## 错误处理与边界

所有可预期构造错误通过 `Result<_, CopError>` 返回，不发生 I/O 重试。主要错误边界包括：

- 空条件、结尾缺操作数、括号错误、未知字符、分号或孤立点号；
- 未闭合的字符串、反引号标识符或块注释；
- 未知列、大小写不敏感匹配出多个同名列；
- 索引列偏移越界、生成列依赖不存在；
- PK handle 找不到主键列、common handle 缺主键索引或主键索引偏移越界。

条件扫描器会跳过字符串和注释；限定名只以最后一段作为列名；函数名和已列出的 SQL
关键字不按列处理；列按首次出现顺序去重。它只是最小语法扫描器，不等价于完整 TiDB
SQL parser：关键字集合、数字扫描和操作数状态机均是有限实现，扩展 SQL 语法时必须增加
针对性测试。

`resolveIndicesForIndex` 和 `resolveIndicesForHandle` 对找不到的列采用跳过而非报错；
`CopContextMultiIndex::IndexColumnOutputOffsets` 对未知 ID 返回空向量，`IndexInfo` 返回
`None`。单索引实现则按 Go 接口约定忽略传入 ID，始终返回唯一索引的数据。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部资源。所有集合都在构造栈帧内创建，
结果拥有 `TableInfo`、`IndexInfo`、字符串和向量；trait 查询只借用或克隆这些数据。
因此并发安全取决于调用者如何共享 `Box<dyn CopContext>`：trait 本身未声明
`Send + Sync`，本文件也不提供内部同步。

构造阶段完成后上下文逻辑上只读，这与 Go 文件注释中的“初始化后不变”契约一致。
返回的偏移向量会被克隆，调用者修改结果不会改变上下文本体。表达式解析没有缓存，
每次 `GetCondition` 都重新扫描条件文本；多索引场景的成本与索引数和条件文本总长度成正比。

## 与 Go 版本的对应关系

Rust 基本保留 `pkg/ddl/copr/copr_ctx.go` 的公开形状和核心语义：单/多索引分派、最小
读取列集合、三种 handle 模式、生成列依赖展开、索引与 handle 偏移、多条件 `OR`
组合，以及虚拟生成列条件不下推。Rust 测试中的 handle 模式用例直接对应 Go
`TestNewCopContextSingleIndex`，handle 顺序和虚拟列测试也复刻 Go 用例。

需要明确的差异和迁移边界：

- Go 使用真实 `model.TableInfo`、`expression.Column`、`exprctx.BuildContext` 和
  `types.FieldType`；Rust 当前是同文件内的简化类型。
- Go 通过 `tables.ExtractColumnsFromCondition` 和 `expression.ParseSimpleExpr` 使用完整
  parser/rewriter，返回可执行 `expression.Expression`；Rust 用字符扫描器，只返回文本
  与列 ID。
- Go 的新排序规则设置由 `exprCtx` 携带，并有固定 collation 回归测试；Rust 使用空
  `BuildContext` 和单独 `UseNewCollate` 字段，当前解析逻辑不消费该字段，因此不能据此
  宣称完整 collation 等价。
- Go `fillUsedColumns` 直接索引偏移，Rust 增加了偏移越界错误；Rust 还显式报告缺失
  primary index、未知/歧义条件列等错误。
- Go 的生产回填链已经调用该组件；Rust 当前只确认 crate 导出和独立单元测试，未确认
  等价生产接线。

## 扩展指南

- 新增输出元数据时，优先扩展 `CopContextBase`，并在 `NewCopContextBase` 中一次性维持
  `ColumnInfos`、`FieldTypes`、`ExprColumnInfos` 及偏移之间的不变量。
- 调整 handle 规则时，应同时修改 `TableInfo::{GetPkColInfo, HasClusteredIndex}`、
  `NewCopContextBase` 和 `resolveIndicesForHandle`，并在独立文件
  `pkg/ddl/copr/copr_ctx_test.rs` 增加 row ID、PK handle、common handle 回归用例。
- 调整条件索引语义时，应同步审查 `condition_columns`、`parse_condition` 以及单/多索引
  `GetCondition`。必须覆盖引号、注释、限定名、大小写、未知/歧义列、非法语法、函数、
  虚拟列、无条件索引和多条件 `OR`；不要把测试内嵌回生产源文件。
- 若要完成真实 expression crate 接线，应先替换简化模型并解除 Cargo 依赖的
  `cfg(any())` 保护，再逐项对齐 Go parser、collation、类型与错误语义；不能只扩充当前
  字符扫描器后就视为完整移植。
- 若新增生产调用点，最接近的对照位置是 Go `NewReorgCopContext`。接线时还需验证
  request source、push-down flags、表达式上下文和多索引扫描生命周期，而不能在本文件
  中引入 DDL job/checkpoint 逻辑。
- 性能上应避免重复复制大型元数据或在热路径反复解析条件；若引入缓存，需要明确上下文
  构造后不可变的约束以及跨线程共享要求。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；
  `files --filter pkg/ddl/copr/copr_ctx.rs` 确认目标文件已索引且含 57 个符号。
- RustCodeGraph `node --file pkg/ddl/copr/copr_ctx.rs`：核对完整 911 行源码、类型、工厂、
  trait 实现、条件解析和辅助函数。
- RustCodeGraph `query` / `explore`：核对 `NewCopContext`、三个构造器、
  `fillUsedColumns`、`GetCondition`、偏移函数的内部调用边；精确 `callers` 查询在本地长时间
  无输出后中止，因此又以仓库 Rust 搜索确认没有生产调用点。
- RustCodeGraph `node NewReorgCopContext`：确认 Go 调用
  `pkg/ddl/copr/copr_ctx.go::NewCopContext`，以及来自两个回填 pipeline 的上游边。
- 已读源码与配置：`pkg/ddl/copr/copr_ctx.rs`、`pkg/ddl/copr/lib.rs`、
  `pkg/ddl/copr/Cargo.toml`、根 `Cargo.toml`、`pkg/ddl/Cargo.toml`。
- Go 对照：`pkg/ddl/copr/copr_ctx.go`、
  `pkg/ddl/backfilling_txn_executor.go::NewReorgCopContext`，并参考 RustCodeGraph 给出的
  `pkg/ddl/backfilling_operators.go` 上游边。
- 独立测试：`pkg/ddl/copr/copr_ctx_test.rs` 覆盖 handle 三模式、handle 顺序、虚拟列、
  条件合法性/引用列/虚拟列拒绝和多索引查询；`pkg/ddl/copr/copr_ctx_test.go` 覆盖对应
  Go 行为及固定 collation。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务规定的 11 章节结构检查，
  并人工复核本文没有把 Go 生产链或迁移目标误写成 Rust 当前已接线能力。
