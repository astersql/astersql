# `br/pkg/utils/schema.rs`

## 文件定位

[`schema.rs`](schema.rs) 是 `astersql-br-pkg-utils` crate 的 schema/name 辅助模块，由 [`lib.rs`](lib.rs) 以 `#[path = "schema.rs"] pub mod schema` 挂载，并把本文件的常量之外全部公开函数再导出到 crate 根。其 Rust 语义基准是同目录的 [`schema.go`](schema.go)：一组函数负责判断表是否需要备份 AutoID，另一组负责 SQL 标识符引用，以及 BR 备份/恢复期间系统库临时名称的生成、识别和还原。

这个模块不是 schema 元数据的所有者，也不执行 DDL。它只对调用方已有的 `TableInfo`、`CIStr` 或字符串做同步、确定性的判定/转换。当前仓库中可确认的本 crate 生产调用是 [`filter.rs`](filter.rs) 调用 `StripTempDBPrefixIfNeeded` 与 `IsSysDB`；crate 根还把所有函数公开给依赖 `astersql-br-pkg-utils` 的下游。多个 BR 子 crate 同时保留了自己的 `stubs.rs` 同名实现，因此那些局部 stub 的调用不能算作本文件的实际调用边。

## 核心职责

- `NeedAutoID` 根据表是否使用整数主键 handle、common handle，以及是否存在 `AUTO_INCREMENT` 列，判断备份是否必须保存 AutoID 水位。
- `EncloseName`、`UnquoteName` 和 `EncloseDBAndTable` 在 SQL 文本边界处理反引号引用，避免名称中的反引号破坏生成的语句。
- `IsSysDB` 与 `IsTemplateSysDB` 区分真实系统库和恢复过程中的临时系统库；前者包含 `mysql`、`sys`、`workload_schema`，后者按 Go 版本只识别临时 `mysql` 与临时 `sys`。
- `TemporaryDBName`、`StripTempDBPrefixIfNeeded`、`StripTempDBPrefix` 和 `GetSysDBCIStrName` 构成临时名称的生成/还原协议，并在需要时报告是否实际去掉了前缀。
- `IsSysOrTempSysDB` 把“先还原临时前缀、再判断系统库”组合为过滤谓词，使临时 `workload_schema` 也能被识别。

## 主要符号

- `pub const temporaryDBNamePrefix: &str = "__TiDB_BR_Temporary_"`：BR 私有临时库名前缀。常量本身没有在 [`lib.rs`](lib.rs) 的 `pub use schema::{...}` 中再导出，外部调用需走 `schema::temporaryDBNamePrefix`。
- `pub fn NeedAutoID(tbl_info: &TableInfo) -> bool`：当 `!PKIsHandle && !IsCommonHandle`（表需要隐式 `_tidb_rowid`）或 `GetAutoIncrementColInfo()` 找到自增列时返回 `true`。
- `pub fn EncloseName(name: &str) -> String`：先把名称内每个反引号替换成两个反引号，再在两端补一对反引号。
- `pub fn UnquoteName(name: &str) -> String`：若首尾都是反引号则只剥掉最外层一对，随后把所有双反引号还原为单反引号；没有成对外壳时仍会执行内部还原。
- `pub fn EncloseDBAndTable(database: &str, table: &str) -> String`：分别调用 `EncloseName`，再用点号拼成 `` `db`.`table` ``，不会把整个限定名作为一个标识符引用。
- `pub fn IsTemplateSysDB(dbname: &CIStr) -> bool`：读取 `CIStr.O`，精确匹配临时 `mysql` 或临时 `sys`；不匹配临时 `workload_schema`。
- `pub fn IsSysDB(db_lower_name: &str) -> bool`：精确匹配 parser/mysql crate 提供的 `SystemDB`、`SysDB`、`WorkloadSchema` 常量。参数契约是小写名称，函数本身不规范化大小写。
- `pub fn TemporaryDBName(db: &str) -> CIStr`：拼接前缀后交给 `NewCIStr`，同时产生原始形式 `O` 和小写形式 `L`。
- `pub fn StripTempDBPrefixIfNeeded(temp_db: &str) -> String`：有精确前缀则返回后缀，否则复制原字符串；不返回命中标志。
- `pub fn StripTempDBPrefix(temp_db: &str) -> (String, bool)`：执行同一剥离规则，并用布尔值区分“已剥离”和“原样返回”。
- `pub fn IsSysOrTempSysDB(db: &str) -> bool`：组合 `StripTempDBPrefixIfNeeded` 与 `IsSysDB`。
- `pub fn GetSysDBCIStrName(mut temp_db: CIStr) -> (CIStr, bool)`：按值取得 `CIStr`；仅当 `O` 有精确前缀时，才以相同字节长度裁掉 `O`、`L` 的前缀并返回 `true`。

## 执行流程

1. 备份方需要决定是否读取 AutoID 时，把 `TableInfo` 传给 `NeedAutoID`。函数先计算是否没有显式 handle，再查询列集合中的自增列；任一条件成立即需要 AutoID。整数主键 handle 或 common handle 且没有自增列时返回 `false`。
2. 生成 SQL 片段时，单个名称进入 `EncloseName`：内部反引号先加倍，随后整体加反引号。库表限定名经 `EncloseDBAndTable` 分别引用，保证点号仍是 SQL 限定符。解析已引用名称时，`UnquoteName` 先处理可选外壳，再反转双反引号转义。
3. 备份系统库时，`TemporaryDBName` 将真实库名改为 BR 临时名；`IsTemplateSysDB` 可识别其中用于模板处理的临时 `mysql`/`sys`。
4. 过滤或恢复时，字符串路径使用 `StripTempDBPrefixIfNeeded`/`StripTempDBPrefix` 还原名称；`IsSysOrTempSysDB` 再把还原结果传给 `IsSysDB`，所以真实与临时的三个系统库都得到相同判定。
5. 元数据对象路径使用 `GetSysDBCIStrName`。命中时同时裁剪 `CIStr.O` 和 `CIStr.L`，保持原始名与大小写不敏感名指向同一后缀；未命中时原对象按值返回且标志为 `false`。

## 数据与状态

本文件没有全局可变状态、缓存、锁或 I/O。唯一模块级数据是不可变字符串常量 `temporaryDBNamePrefix`；所有函数均只读取借用参数，或消费/构造拥有所有权的 `String`、`CIStr`，因此可重入。

`TableInfo` 来自 `astersql-meta-model`。`NeedAutoID` 依赖的关键状态是 `PKIsHandle`、`IsCommonHandle` 和 `Columns` 中的 AUTO_INCREMENT 标志；`GetAutoIncrementColInfo` 线性扫描列并返回第一个匹配项，所以该判定最坏为列数线性复杂度，其余条件为常数时间。

`CIStr` 来自 `astersql-parser-ast`，用 `O` 保存原始字符串、用 `L` 保存小写字符串。`TemporaryDBName` 通过 `NewCIStr` 正常建立两者；`GetSysDBCIStrName` 则保留既有对象的剩余内容，只按固定 ASCII 前缀长度同步裁剪两字段。字符串函数都分配新结果，不修改调用方传入的 `&str`。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：包名是 `astersql-br-pkg-utils`，库入口为 [`lib.rs`](lib.rs)。本文件直接依赖：

- `astersql-meta-model::TableInfo`：提供 handle 标志、列集合和 `GetAutoIncrementColInfo`。
- `astersql-parser-ast::{CIStr, NewCIStr}`：承载原始/小写两种库名表示并构造一致对象。
- `astersql-parser-mysql::const::{SystemDB, SysDB, WorkloadSchema}`：提供系统库名称常量，避免在判定函数中复制字面量。
- Rust 标准库 `String`、`format!`、`str::replace` 与 `starts_with`：完成纯字符串变换。

可确认的内部调用边为 `EncloseDBAndTable` → `EncloseName`（两次），`IsSysOrTempSysDB` → `StripTempDBPrefixIfNeeded` → `IsSysDB`。上游方面，[`filter.rs`](filter.rs) 的 `MatchSchema` 和 `MatchTable` 先调用 `StripTempDBPrefixIfNeeded`，再调用 `IsSysDB`，以便 table filter 对真实库名工作并尊重 `with_sys` 开关；[`schema_test.rs`](schema_test.rs) 直接验证 `IsSysOrTempSysDB`，[`parity_test.rs`](parity_test.rs) 验证引号、去前缀和系统库基本语义。

RustCodeGraph 能定位本文件全部同名函数及 Go 对照，但对目标函数的 `callers`/`callees` 查询没有返回跨文件边。全仓库精确检索进一步确认：依赖该 crate 的生产模块目前主要使用其它 utils 子模块；`backup`、`task`、`restore/snap_client`、`stream` 等目录看到的同名调用多数解析到各自 `stubs.rs` 或局部函数，而非本文件。扩展时不能假定修改本文件会自动改变这些副本的行为。

## 错误处理与边界

所有 API 都是无 `Result` 的纯函数，不产生显式错误。边界由返回值表达：前缀不存在时返回原名和/或 `false`；非系统库返回 `false`；没有 AutoID 需求返回 `false`。

- `NeedAutoID` 只接受有效的 `&TableInfo`，不存在 Go 指针版本的 `nil` 输入分支。它判断“是否需要备份 AutoID”，不读取或校验实际 AutoID 水位。
- `EncloseName` 对空串返回 `` `` ``，对任意数量的内部反引号逐个加倍。它只引用标识符，不验证名称长度、字符集或 SQL 保留字。
- `UnquoteName` 只在首尾同时为反引号时剥一层；不平衡的单边反引号保留。随后无条件压缩 ` `` `，因此它不是严格 SQL parser，也不报告格式错误。
- 临时前缀匹配和系统库匹配均区分大小写。`IsSysDB("MYSQL")` 为 `false`；调用者应传入 `CIStr.L` 或其它已小写输入。`TemporaryDBName` 生成的 `O` 使用固定大小写前缀，符合后续精确匹配契约。
- `IsTemplateSysDB` 有意不包含 `workload_schema`，而 `IsSysDB`/`IsSysOrTempSysDB` 包含它；两者职责不可互换。
- `StripTempDBPrefix*` 对任何带该前缀的名称都执行剥离，不要求后缀真是系统库，甚至允许空后缀。是否为系统库必须另行调用 `IsSysDB` 或使用组合函数。
- `GetSysDBCIStrName` 以 `O` 是否命中作为唯一门槛，然后按相同 ASCII 字节数裁剪 `L`。它假定传入 `CIStr` 的 `O`/`L` 来自一致名称；手工构造的不一致对象可能产生语义不一致，函数不会修复或验证。

## 并发与资源生命周期

模块不创建线程、异步任务、通道、锁、事务、文件句柄或网络连接，没有跨调用生命周期。所有借用仅持续到函数返回；`GetSysDBCIStrName` 消费传入值后在本地修改并交还所有权。

主要资源成本来自字符串分配：`EncloseName` 的替换和格式化、`UnquoteName` 的初始复制/可能切片复制/替换、临时名称拼接与前缀剥离都会创建新字符串。名称通常很短，因此当前实现以清晰和 Go 语义一致为主；若用于大批量热路径，优化时必须保持反引号转义顺序以及 `CIStr.O`/`L` 同步不变量。

## 与 Go 版本的对应关系

[`schema.go`](schema.go) 是逐函数语义基准，Rust 保留了相同的临时前缀、AutoID 布尔公式、反引号转义/还原、系统库集合、前缀剥离返回值以及 `CIStr` 双字段裁剪。[`schema_test.go`](schema_test.go) 的八组 `IsSysOrTempSysDB` 表驱动用例已移植到 [`schema_test.rs`](schema_test.rs)：三个真实系统库、三个临时系统库返回 `true`，普通库与带前缀的普通库返回 `false`。

语言适配差异主要是所有权和类型安全：

- Go `NeedAutoID(*model.TableInfo)` 理论上可收到 `nil` 并 panic；Rust 使用不可空引用，空值在类型层面不可表示。
- Go 字符串切片和 Rust 字符串切片都按字节边界工作。本实现只在成功匹配固定 ASCII 前缀后用其长度裁剪，因此对合法 UTF-8 后缀仍位于字符边界。
- Go `TemporaryDBName` 返回 `ast.CIStr`，Rust 返回对应的 `astersql_parser_ast::CIStr`；两者都同时维护原始名与小写名。
- Go 包函数天然通过 `utils.X` 使用；Rust 既可走 `astersql_br_pkg_utils::schema::X`，也可对 [`lib.rs`](lib.rs) 已再导出的函数走 crate 根。前缀常量没有 crate 根再导出。
- 当前 Rust 独立测试只直接覆盖 `IsSysOrTempSysDB`；其它函数主要由 [`parity_test.rs`](parity_test.rs) 部分覆盖或依赖 Go 测试作为对照，覆盖面尚未达到 Go 文件全部 API 的逐函数单测。

## 扩展指南

- 增加或调整系统库集合时，应同步检查 `IsSysDB`、`IsTemplateSysDB` 与 `IsSysOrTempSysDB` 的职责差异，并更新独立的 [`schema_test.rs`](schema_test.rs) 和 Go 对照测试；还要验证 [`filter.rs`](filter.rs) 在 `with_sys` 两种模式下的结果。
- 修改临时前缀协议时，必须成组调整 `TemporaryDBName`、两个 `StripTempDBPrefix*`、`IsTemplateSysDB`、`IsSysOrTempSysDB`、`GetSysDBCIStrName` 及相关测试，确保生成、识别和还原保持可逆；还需盘点各子 crate 的同名 stub，防止迁移副本漂移。
- 扩充 AutoID 规则时，从 `NeedAutoID` 接入，并在 [`schema_test.rs`](schema_test.rs) 增加独立 `TableInfo` 场景：无 handle、整数主键 handle、common handle、有/无 AUTO_INCREMENT，以及组合情况。不要把 Rust 测试内嵌回生产文件。
- 调整引用规则时，对 `EncloseName`/`UnquoteName` 增加空名、多个反引号、成对/不成对外壳和往返测试；限定名规则应继续让库名和表名分别转义。
- 若要让新调用方使用这些函数，优先依赖 canonical crate API，而不是再复制到 `stubs.rs`。若移除已有 stub 或改接 canonical 实现，应作为单独的接线任务验证依赖层次和行为，不属于本说明任务。
- 性能修改应关注 `GetAutoIncrementColInfo` 的列扫描和字符串重复分配，但不能用大小写折叠或宽松前缀匹配改变当前兼容语义。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、7,032 个 Rust 文件。`query` 分别定位了 `br/pkg/utils/schema.rs` 中的 `NeedAutoID`、`EncloseName`、`IsSysOrTempSysDB`、`GetSysDBCIStrName` 及其 Go 同名符号；对目标符号执行 `callers`/`callees`，未得到可用跨文件边，因此使用精确源码检索补充图覆盖缺口。
- Rust 实现与类型证据：[`schema.rs`](schema.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`filter.rs`](filter.rs)、`pkg/meta/model/table.rs`、`pkg/parser/ast/lib.rs`。
- Go 对照证据：[`schema.go`](schema.go)、[`schema_test.go`](schema_test.go)。
- Rust 测试证据：[`schema_test.rs`](schema_test.rs)、[`parity_test.rs`](parity_test.rs)。前者覆盖系统库/临时系统库组合判定；后者覆盖名称引用与反引用、临时前缀剥离和基础系统库判定。
- 接线核对：检索了 `astersql-br-pkg-utils` 的 Cargo 依赖、`astersql_br_pkg_utils` 使用点和全部主要符号，确认了 [`filter.rs`](filter.rs) 的直接生产边，并区分了其它 BR crate 的局部 stub/重复实现。
- 本任务只新增文档，按计划未运行 Cargo。交付验证使用任务规定的 11 个固定二级标题结构检查，并人工复核文档能够说明文件存在原因、运行流程、边界与安全扩展位置。
