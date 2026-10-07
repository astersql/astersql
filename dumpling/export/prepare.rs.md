# `dumpling/export/prepare.rs`

## 文件定位

[`prepare.rs`](./prepare.rs) 属于 `astersql-dumpling-export` library crate。crate 根文件 [`lib.rs`](./lib.rs) 通过 `include!("prepare.rs")` 把它直接并入 crate 根作用域，而不是建立独立的 `prepare` 模块；因此这里的公开常量、类型和函数都直接成为 crate 级符号，并可与其他 `include!` 文件共享 `HashMap`、`OnceLock`、`tcontext`、`Conn`、`Config`、`OutputTemplate` 等导入或定义。

它位于 Dumpling 真正创建导出任务之前，提供三组准备期能力：输出文件名模板入口、数据库目标校验，以及表示库表清单的基础数据结构。常规主链为 `dump.rs::prepareTableListToDumpInner` → `prepareDumpingDatabases` → `sql.rs::ShowDatabases`/`block_allow_list.rs::filterDatabases`，随后再由 `sql.rs::ListAllDatabasesTables` 形成 `Config::Tables`。本文件本身不读取表数据、不写导出文件，也不启动 worker。

[`Cargo.toml`](./Cargo.toml) 将该 crate 声明为 Go 包 `dumpling/export` 的 Rust library 移植；此文件直接使用的 Dumpling context 来自 `astersql-dumpling-context`，SQL 连接、错误类型和模板实现目前来自同 crate 的 `stubs.rs`。Cargo 没有为本文件设置条件 feature。

## 核心职责

1. `DefaultOutputFileTemplate` 和 `ParseOutputFileTemplate` 建立文件名模板的默认定义，并保证每次解析都在全局默认值的独立副本上进行。
2. `prepareDumpingDatabases` 从实例发现数据库，先应用 schema filter，再区分“未显式指定数据库”和“显式白名单”两条路径；显式白名单中的不存在项会被一次性汇总为错误。
3. `TableType`、`TableInfo` 和 `DatabaseTables` 是准备期与后续 dump/sql/filter 代码共享的对象清单模型；`DatabaseTablesExt` 提供追加、合并和诊断文本等操作。
4. `DatabaseTablesToMap` 把清单投影成 `库名 → 普通表名集合`，明确排除视图和序列。

这些职责只负责“准备导出什么”和“如何命名输出”；实际表枚举在 `sql.rs`，实际过滤接线和任务生成在 `dump.rs`，模板执行实现则在 `stubs.rs::OutputTemplate`。

## 主要符号

- `outputFileTemplateSchema`、`outputFileTemplateTable`、`outputFileTemplateView`、`outputFileTemplateSequence`、`outputFileTemplateData`、`outputFileTemplatePolicy`：模板定义名的稳定字符串。它们保留 Go 风格命名，供配置、writer 和任务代码按对象类别选择模板。
- `DefaultAnonymousOutputFileTemplateText = "result.{{.Index}}"`：没有库表上下文的自定义 SQL 导出使用的默认 data 模板；`config.rs::Config::ParseFromFlags` 在 SQL 非空且用户未给模板时采用它。
- `DEFAULT_OUTPUT_FILE_TEMPLATE: OnceLock<OutputTemplate>`：进程内惰性初始化的默认模板缓存。缓存内容只构造一次，调用者取得的是 clone。
- `DefaultOutputFileTemplate() -> OutputTemplate`：以 `OutputTemplate::default_dumpling()` 为基础，补入 `event`、`function`、`procedure`、`trigger` 四个 post-schema 定义，然后返回深拷贝。
- `ParseOutputFileTemplate(text: &str) -> Result<OutputTemplate>`：clone 默认模板并调用 `OutputTemplate::Parse`，因此局部自定义不会污染后续调用。
- `prepareDumpingDatabases(tctx, conf, db) -> Result<Vec<String>>`：内部准备函数。返回值在未显式指定时保持过滤后发现顺序，在显式指定时保持 `Config::Databases` 顺序。
- `TableType`：`#[repr(i8)]` 的 `Copy` 枚举，取值为 base table、view、sequence；默认值是 `TableTypeBase`。`String` 返回 information schema/SHOW 结果使用的固定大写文本，`ParseTableType` 执行严格反向解析。
- `TableInfo { Name, AvgRowLength, Type }`：单个数据库对象的准备期元信息。`Equals` 只比较名字与类型，刻意忽略用于估算/分块的 `AvgRowLength`。
- `DatabaseTables = HashMap<String, Vec<TableInfo>>`：按数据库名分组且保持每个库内向量顺序的清单；数据库之间的遍历顺序不稳定。
- `DatabaseTablesExt`：为该类型别名补充 `AppendTable`、`AppendTables`、`AppendViews`、`Merge`、`Literal`。批量追加普通表时按同下标读取 `avg_row_lengths`；视图行长固定为 0；合并只拼接、不去重。
- `DatabaseTablesToMap(&DatabaseTables)`：为每个已有数据库建立内层 map，只插入 `TableTypeBase` 的名字；即使一个库只有 view/sequence，也会保留空的内层 map。

## 执行流程

默认模板流程如下：首次调用 `DefaultOutputFileTemplate` 时，`OnceLock` 执行 `OutputTemplate::default_dumpling`，再补齐四种 post-schema 对象模板；后续调用直接 clone 已缓存对象。`ParseOutputFileTemplate` 又在该 clone 上调用 `Parse(text)`，最终返回调用者私有模板。`Config::ParseFromFlags` 把结果写入 `Config::OutputFileTemplate`；SQL-only 且模板为空时先把文本替换为匿名模板。

数据库准备流程如下：

1. `prepareDumpingDatabases` 调用 `ShowDatabases(db)`；查询、扫描或关闭 rows 的错误通过 `?` 返回。
2. 将发现结果交给 `filterDatabases(tctx, conf, databases)`，只保留 `Config::TableFilter.MatchSchema` 命中的库。
3. 若 `Config::Databases` 为空，立即返回过滤后的发现结果。
4. 若配置了显式数据库，先从过滤后的结果建立 set-like `HashMap<&str, ()>`，再按配置顺序收集所有不在集合中的名字。
5. 缺失列表非空时返回 `Unknown databases [db4,db6]` 形式的单个错误；否则 clone 并返回显式列表。

`dump.rs::prepareTableListToDumpInner` 只在既无自定义 SQL、也未提供显式 tables-list 时调用这条流程。之后它依据 `NoViews`/`NoSequences` 组成允许的 `TableType` 列表，调用 `ListAllDatabasesTables`，再做表级过滤。因此本函数的“存在性”判断也是“经过 schema filter 后是否仍可导出”的判断，而不只是服务器上是否物理存在。

表清单 helper 都是同步的原地操作：`AppendTable` 通过 `entry(...).or_default().push` 建组；`AppendTables`/`AppendViews` 循环追加；`Merge` 消费另一个 map 并逐库 `extend`；`Literal` 生成日志文本；`DatabaseTablesToMap` 生成新的嵌套 map，不修改原清单。

## 数据与状态

唯一的全局可变状态是 `DEFAULT_OUTPUT_FILE_TEMPLATE`，它在初始化后不可替换；对外返回 clone，因而调用方修改 `text` 或 `defines` 不会回写缓存。模板的真实结构是 `stubs.rs::OutputTemplate { text, defines }`，`defines` 是模板名到模式字符串的 `HashMap`。

数据库清单使用拥有所有权的 `String`、`Vec<TableInfo>` 和 `HashMap`。`AppendTable`/批量 append 保证单库内部的插入顺序，但 `HashMap` 不承诺库的遍历顺序，所以 `Literal` 不适合作为稳定序列化格式或 golden 输出。`Merge` 保留左右两边在单库内的相对顺序，但不检查重复名或类型冲突。

`AvgRowLength` 是附加统计值，不属于 `TableInfo::Equals` 定义的对象身份。调用者若需要比较分块估算，必须单独检查它。`DatabaseTablesToMap` 的 `()` 值只表达成员关系，不保留类型或行长。

## 依赖与调用关系

- 上游：RustCodeGraph 显示 `prepareDumpingDatabases` 的生产调用者是 `dump.rs::prepareTableListToDumpInner`，测试调用者是 `prepare_test.rs::test_prepare_dumping_databases`。
- 下游：`prepareDumpingDatabases` 调用 `sql.rs::ShowDatabases` 和 `block_allow_list.rs::filterDatabases`；前者管理查询 rows 生命周期，后者调用 `Config::TableFilter.MatchSchema` 并记录被忽略数据库。
- 模板：`config.rs::Config::ParseFromFlags` 和 CLI 配置路径调用 `ParseOutputFileTemplate`；`DefaultConfig`、该解析函数及 `prepare_test.rs` 调用 `DefaultOutputFileTemplate`。底层 `OutputTemplate::{default_dumpling, Clone, Parse, Execute}` 位于 `stubs.rs`。
- 类型解析：RustCodeGraph 和 `sql.rs` 代码显示 `ListAllDatabasesTables` 在 information schema 与 `SHOW FULL TABLES` 分支调用 `ParseTableType`。当前调用点对解析错误使用 `unwrap_or(TableTypeBase)`，因此未知服务器文本在该下游会降级为普通表，而不是继续传播本函数产生的错误。
- 清单：`filterTablesFunc` 使用 `AppendTable` 组织保留项和忽略项，并使用 `Literal` 输出诊断日志；dump、schema projection 和相关测试广泛构造/读取 `DatabaseTables`。
- `DatabaseTablesToMap`：RustCodeGraph 没有发现除函数自身外的 Rust 生产调用者；Go 版本仍由 `dump.go::renewSelectTableRegionFuncForLowerTiDB` 使用。因此它目前是为 Go API 对齐保留的公开辅助函数，不能据此声称已接入 Rust 主链。

## 错误处理与边界

- `prepareDumpingDatabases` 不吞掉 `ShowDatabases` 的查询、扫描和 rows 关闭错误；独立 Rust/Go 测试都覆盖了查询失败。
- 显式数据库校验基于“发现后且过滤后”的集合；被 filter 排除的显式库会被报告为 unknown。错误按配置顺序汇总所有缺失项，逗号之间无空格。
- 数据库名比较是精确字符串比较，本文件不做大小写折叠、trim 或去重；重复的显式名字会按原样返回，重复缺失名也可能重复出现在错误中。
- `ParseTableType` 只接受 `BASE TABLE`、`VIEW`、`SEQUENCE` 三个精确文本。Rust 枚举不能构造未知判别值，所以 `TableType::String` 无 Go 版 `UNKNOWN` 分支。
- `AppendTables` 假定 `table_names.len() <= avg_row_lengths.len()`；若行长数组更短，Rust 下标访问会 panic，与 Go 对应代码的越界约束一致。调用者应在进入该函数前保证两个数组一一对应。
- `Merge` 和 `Append*` 不做重复检测；`DatabaseTablesToMap` 则因 map key 覆盖而自然去重普通表名。
- 当前 `stubs.rs::OutputTemplate::Parse` 只是 Go `text/template` 的有限子集：它识别 data define、匿名模板和少量占位替换，当前实现始终返回 `Ok(())`，不具备 Go 模板解析器的完整语法校验。这是现状限制；新增模板语法不能只改本文件。

## 并发与资源生命周期

`OnceLock` 保证默认模板在并发首次访问时只初始化一次。初始化完成后所有调用者持有独立 clone，无需围绕模板修改加锁，也不会发生一个请求覆盖另一个请求模板的情况。

其余类型没有内部锁、原子量、线程或 channel；`DatabaseTablesExt` 依赖 `&mut self` 提供独占修改。若需要跨线程共享清单，责任在调用方，应在并发开始前完成准备或使用外部同步，不能并发持有可变引用。

`prepareDumpingDatabases` 同步借用 `&Conn`，自身不持有事务、连接所有权或后台任务。`ShowDatabases` 创建的 rows 在其函数内显式 `Close`，关闭失败同样向上传播；本文件返回后不保留数据库资源。`tcontext::Context` 只被过滤逻辑用于日志上下文。

## 与 Go 版本的对应关系

对应源文件是 [`prepare.go`](./prepare.go)，直接回归文件是 [`prepare_test.go`](./prepare_test.go) 与 [`prepare_test.rs`](./prepare_test.rs)。数据库发现、过滤顺序、显式白名单返回顺序、缺失项汇总错误、三种表类型、`Equals` 忽略行长、append/merge 不去重，以及 base-table-only map 投影均与 Go 结构保持一致。

主要表示差异如下：Go 的 `DefaultOutputFileTemplate` 是初始化时构造的 `*template.Template` 全局变量，Rust 用 `OnceLock<OutputTemplate>` 惰性初始化并通过函数返回 clone；这强化了调用方隔离。Go `DatabaseTables` 保存 `[]*TableInfo`，Rust 保存拥有所有权的 `Vec<TableInfo>`，避免空指针但复制/合并会移动或 clone 值。Go 用接收者方法，Rust 因类型别名不能定义固有方法，改用同作用域可见的 `DatabaseTablesExt` trait。

模板能力尚未完全等价：Go 使用完整 `text/template`、`missingkey=error` 和正则驱动的文件名转义；Rust 当前由 `stubs.rs::OutputTemplate` 实现有限模式解析和替换。默认模板定义的回归测试覆盖 schema、event、function、procedure、sequence、trigger、view、table、data、placement-policy 的输出，但不能证明任意 Go 模板语法均兼容。

测试范围也要区分：`prepare_test.rs` 中数据库清单测试直接覆盖本文件；同文件的 table-list 测试主要覆盖 `sql.rs::ListAllDatabasesTables`，只是复用了这里的数据类型与 helper。文档不把后者误归为本文件实现。

## 扩展指南

- 新增可导出对象类型时，应同步修改 `TableType`、字符串常量、`String`、`ParseTableType`、模板定义和 `dump.rs` 的任务分派；同时检查 `DatabaseTablesToMap` 是否仍应只保留 base table。测试应放在独立的 `prepare_test.rs`，必要时同步 `sql_test.rs`/`dump_test.rs`，不要嵌入生产源文件。
- 扩展输出模板名称或默认路径时，优先修改默认模板所有者 `DefaultOutputFileTemplate` 或 `stubs.rs::OutputTemplate::default_dumpling`，并在 `test_default_output_file_template_matches_go_definitions` 增加精确输出断言。若要求完整 Go template 语法，需扩展/替换 `stubs.rs::OutputTemplate::Parse`，并评估配置解析与 writer 命名兼容性。
- 修改数据库选择语义时，保持“发现 → schema filter → 显式存在性校验”的顺序，或明确评估行为变化；至少补充成功、空显式列表、底层错误、多个缺失项、被 filter 排除和重复输入的测试。
- 调整 `AppendTables` API 时，应显式决定长度不匹配是继续 panic、返回 `Result` 还是截断；静默 `zip` 会改变 Go 行为，不应作为无说明的简化。
- 若要在 Rust 主链使用 `DatabaseTablesToMap`，应先追踪 Go 调用点的锁表/区域选择语义，并补生产调用与独立回归测试，不能只因函数存在就假定功能已迁移。
- 性能方面，默认模板 clone 和多个清单投影都会分配/复制字符串；在超大 schema 数量下若要优化，应以 profiling 证据决定是否引入共享不可变模板或容量预估，同时保持调用者不可污染全局默认值这一不变量。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 7,032 个 Rust 文件；`node --file dumpling/export/prepare.rs` 读取了目标文件 1–238 行并确认 38 个符号。
- RustCodeGraph 调用证据：`explore "prepareDumpingDatabases in dumpling/export/prepare.rs callers and callees"` 确认 Rust 主链调用者 `prepareTableListToDumpInner` 与测试调用者；针对 `ParseOutputFileTemplate`、`DefaultOutputFileTemplate`、`DatabaseTablesToMap` 的 explore 确认配置、测试和 Go/Rust调用差异。
- 已读生产文件：`dumpling/export/prepare.rs`、`dumpling/export/lib.rs`、`dumpling/export/Cargo.toml`、`dumpling/export/dump.rs`、`dumpling/export/config.rs`、`dumpling/export/sql.rs`、`dumpling/export/block_allow_list.rs`、`dumpling/export/stubs.rs`。
- 已读对照与测试：`dumpling/export/prepare.go`、`dumpling/export/prepare_test.go`、`dumpling/export/prepare_test.rs`。
- 关键测试事实：Rust 测试验证默认模板十类定义；数据库测试验证显式顺序、默认发现结果、查询失败和多个缺失项的精确错误。Go 测试提供相同数据库行为基线。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证恰好存在十一个固定二级章节，并人工复核源文件链接、符号名、调用边、边界说明和扩展建议。
