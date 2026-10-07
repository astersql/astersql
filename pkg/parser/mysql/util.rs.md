# `pkg/parser/mysql/util.rs` 逻辑说明

## 文件定位

`pkg/parser/mysql/util.rs` 属于独立 crate `astersql-parser-mysql`，由 [`pkg/parser/mysql/lib.rs`](./lib.rs) 以 `pub mod util` 暴露。该 crate 的清单 [`pkg/parser/mysql/Cargo.toml`](./Cargo.toml) 指定 `lib.rs` 为库入口；本文件自身只依赖标准库的 `HashMap`、`LazyLock`，以及同 crate 的 `const.rs` 认证插件名和 `type.rs` MySQL 协议类型编号，不直接使用清单中的外部依赖。

它位于“协议类型常量”与上层元数据/解析逻辑之间：把 MySQL 类型编号映射为默认显示长度（flen）和小数位数（decimal），并提供整数类型、明文口令类认证插件的分类谓词。调用方包括 DDL 列构造、CAST 语义动作、字段类型兼容判断、表达式推导、行编码和服务端列元数据转换；它不是 SQL 解析入口，也不保存会话或存储状态。

## 核心职责

文件有三组职责，均为进程内、确定性的查询：

1. `IsIntegerType` 判定协议类型是否属于五种整数类型：`TypeTiny`、`TypeShort`、`TypeInt24`、`TypeLong`、`TypeLonglong`。
2. `GetDefaultFieldLengthAndDecimal` 和 `GetDefaultFieldLengthAndDecimalForCast` 分别查询普通字段/表达式与 CAST 场景的默认 `(flen, decimal)`。两张表不可合并：例如 `TypeString` 在普通表中是 `(1, 0)`、在 CAST 表中是 `(0, -1)`；`TypeJSON` 分别是 `(4_294_967_295, 0)` 与 `(4_194_304, 0)`；`TypeLonglong` 分别是 `(20, 0)` 与 `(22, 0)`。
3. `IsAuthPluginClearText` 把 `mysql_native_password`、`tidb_sm3_password`、`caching_sha2_password` 对应的三个常量归为需要明文口令交换路径的插件集合。该函数只比较插件名，不验证密码，也不代表名为 `mysql_clear_password` 的插件会返回 `true`。

## 主要符号

- `LengthAndDecimal { length: i64, decimal: isize }`：私有、可复制的表项。`length` 保持 Go `int64` 表项的表示，API 返回时转换为 `isize`；`decimal` 直接对应 Go `int` 的平台字长表示。
- `defaultLengthAndDecimal: LazyLock<HashMap<u8, LengthAndDecimal>>`：私有普通默认表，共登记 25 个类型。除了数值、日期时间和字符串类型，还包含 blob、JSON、NULL、SET、ENUM；SET/ENUM 的 flen 为 `-1`，表示不能由类型编号单独确定。
- `pub fn IsIntegerType(tp: u8) -> bool`：公开纯谓词，只接受协议类型编号，不把 `TypeYear`、`TypeBit` 或浮点/定点类型算作整数。
- `pub fn GetDefaultFieldLengthAndDecimal(tp: u8) -> (isize, isize)`：公开普通默认值查询。表内返回记录值，未登记类型返回 `(-1, -1)`。
- `defaultLengthAndDecimalForCast: LazyLock<HashMap<u8, LengthAndDecimal>>`：私有 CAST 默认表，共登记 9 个类型，覆盖 string、date/datetime、decimal、duration、bigint、float/double、JSON。
- `pub fn GetDefaultFieldLengthAndDecimalForCast(tp: u8) -> (isize, isize)`：公开 CAST 默认值查询，未登记类型同样返回 `(-1, -1)`。
- `pub fn IsAuthPluginClearText(authPlugin: &str) -> bool`：公开认证插件分类谓词，精确比较三个常量，不做大小写折叠、别名解析或默认插件解析。

文件没有 trait、enum、公开 struct、`impl` 或条件编译项。`LengthAndDecimal` 与两张表均为模块私有；四个函数构成全部公开 API。

## 执行流程

普通字段默认值的典型主链如下：

1. DDL 在 [`pkg/ddl/create_table.rs`](../../ddl/create_table.rs) 的 `build_column` 取得列的协议类型，调用 `GetDefaultFieldLengthAndDecimal`。
2. 只有原字段的 flen/decimal 为 `UnspecifiedLength` 时，DDL 才写入查询结果；无符号且不是 `BIGINT` 的整数会在调用方把默认 flen 再减一。
3. 同一个 API 也被 [`pkg/types/field_type.rs`](../../types/field_type.rs) 的 `minFlenAndDecimalForType` 和 `needReorgToChange` 使用，分别补齐整数/YEAR 最小元数据、比较整数类型变更是否需要数据重组。

CAST 的主链与普通表分离：

1. [`pkg/parser/parser_actions/expression.rs`](../parser_actions/expression.rs) 在归约 `CAST`/`CONVERT` 相关语法时构造 `FieldType`。
2. 语义动作以目标类型编号调用 `GetDefaultFieldLengthAndDecimalForCast`。
3. 部分分支仅在 flen/decimal 未指定时补默认值；直接构造 CAST 类型的分支则立即设置返回值，再设置 binary flag、字符集和排序规则。后续 AST 节点因此携带 CAST 专用的类型元数据。

`IsIntegerType` 没有复杂流程，只用一次模式匹配返回布尔值。调用方据此选择整数专用逻辑，例如 [`pkg/session/runtime/row_codec.rs`](../../session/runtime/row_codec.rs) 的行编码、[`pkg/expression/util.rs`](../../expression/util.rs) 的整数转换长度比较，以及 [`pkg/util/schemacmp/lattice.rs`](../../util/schemacmp/lattice.rs) 的 schema lattice 比较。

`IsAuthPluginClearText` 顺序比较三个插件常量，任一相等即返回 `true`。仓库搜索未发现 Rust 生产代码调用；当前 Rust 证据是 crate 独立测试。Go 对照函数仍由 `pkg/executor/simple.go` 的密码校验与双密码能力判定调用，不能据此宣称 Rust 账户管理路径已经接线。

## 数据与状态

两张 `HashMap` 是该文件唯一的长期状态。它们由 `LazyLock` 在进程内第一次访问对应查询函数时初始化，随后只通过共享引用读取；API 不暴露 map 或表项，因此调用方不能修改表内容。两个 `LazyLock` 独立初始化，调用普通查询不会强制构造 CAST 表，反之亦然。

键是 `u8`，与 `type.rs` 的 MySQL 协议类型编号一致。返回值采用 `(isize, isize)`：正数是具体显示长度/小数位，`decimal == -1` 表示未指定小数位，整体 `(-1, -1)` 表示该表没有此类型。SET/ENUM 在普通表返回 `(-1, 0)`，与“完全未知”不同，调用方不可只检查 flen 就把两者混同。

函数不缓存每次查询结果之外的数据，不读取环境变量、配置、网络、磁盘、系统表、密码或会话上下文。

## 依赖与调用关系

下游依赖：

- `std::collections::HashMap` 保存稀疏的类型编号到默认值映射；`std::sync::LazyLock` 保证一次初始化。
- [`pkg/parser/mysql/type.rs`](./type.rs) 提供全部 `Type*` 编号和列 flag 常量；本文件通过 `use super::r#type::*` 引入。
- [`pkg/parser/mysql/const.rs`](./const.rs) 提供 `AuthNativePassword`、`AuthTiDBSM3Password`、`AuthCachingSha2Password`。

上游调用关系由 RustCodeGraph 文件节点和 `rg` 交叉核对。RustCodeGraph 将该文件标为被 16 个文件使用；精确搜索确认主要调用面包括：

- 默认字段长度：`pkg/ddl/create_table.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/types/field_type.rs`、`pkg/parser/types/field_type.rs`、`pkg/expression/aggregation/base_func.rs`、`pkg/session/runtime/system_query.rs`、`pkg/server/internal/column/convert.rs`、`pkg/meta/model/column.rs`、`br/pkg/utils/misc.rs`。
- CAST 默认长度：`pkg/parser/parser_actions/expression.rs` 的多个 CAST/CONVERT 归约分支。
- 整数分类：`pkg/types/field_type.rs`、`pkg/expression/util.rs`、`pkg/session/runtime/{dml,load_data,row_codec,ttl_metadata}.rs`、`pkg/ddl/create_table.rs`、`pkg/util/ranger/{ranger,points}.rs`、`pkg/util/schemacmp/lattice.rs`。
- 认证分类：只在 `pkg/parser/mysql/error_3_aster_unit_test.rs` 与 `pkg/parser/mysql/unit_test.rs` 找到 Rust 调用；未找到 Rust 生产调用。

若调用方写成 `model::mysql::*` 或 `mysql::*`，其来源可能是再导出而非另一份实现。例如 [`pkg/meta/model/internal/group1/lib.rs`](../../meta/model/internal/group1/lib.rs) 通过 `pub use parser_mysql::util::*` 转发本文件 API；[`pkg/parser/types/lib.rs`](../types/lib.rs) 也再导出 `util::*`。

## 错误处理与边界

本文件不返回 `Result`、不产生业务错误，也没有显式 panic。未知 `u8` 类型是正常边界，两个查询函数稳定返回 `(-1, -1)`；分类谓词对未知值或未知插件名返回 `false`。

重要边界包括：

- `TypeYear` 在普通默认表中有 `(4, 0)`，但 `IsIntegerType(TypeYear)` 为 `false`；需要 YEAR 默认值的调用方必须显式包含它，`minFlenAndDecimalForType` 正是如此。
- 普通表与 CAST 表的覆盖集合和值不同。把调用点换到另一函数会改变 CHAR、JSON、BIGINT 等类型的元数据语义。
- `IsAuthPluginClearText` 是区分大小写的精确字符串比较，不解析空插件名或系统默认插件；`AuthMySQLClearPassword`/`"mysql_clear_password"` 明确不在集合内。
- `length as isize` 沿用 Go `int(val.length)` 的平台相关转换形状。在仓库正常 64 位目标上，最大值 `4_294_967_295` 可表示；若扩展到 32 位目标，应先评估截断风险，当前函数不会报告溢出。
- `LazyLock` 初始化闭包只构造常量条目，当前没有可观察的失败路径；若未来在闭包中加入会 panic 的逻辑，初始化中毒/重复访问行为将成为新风险。

## 并发与资源生命周期

两个静态表由标准库 `LazyLock` 保证并发安全的一次初始化。初始化完成后只读，查询通过共享借用取得 `Copy` 字段，不需要额外锁、原子计数或克隆 map。初始化成本最多各发生一次，map 生命周期等同进程，不存在显式释放、连接关闭、任务取消或 channel 排空步骤。

公开函数本身无可变全局状态、无异步任务、无阻塞 IO，因而可被多个线程并发调用。性能成本主要是首次构造小型 `HashMap` 和后续哈希查找；若将来显著扩大表或把它放入热路径，可评估 `match`/定长表，但必须先用基准证明收益并保持未知值及两张表差异不变。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/parser/mysql/util.go`](./util.go)。Rust 当前逐项保留其结构与语义：

- Go 私有 `lengthAndDecimal` 对应 Rust 私有 `LengthAndDecimal`。
- 两个 Go 包级 map 分别对应两个 `LazyLock<HashMap<...>>`，登记的类型和值一致。
- Go `IsIntegerType` 的五分支 switch 对应 Rust `matches!`。
- 两个 Go 查询函数在 map miss 时返回 `(-1, -1)`；Rust `match Option` 保持同样结果。
- Go 认证函数的三个 `||` 比较在 Rust 中原样保留，参数由拥有的 `string` 变为借用 `&str`，避免无必要分配而不改变比较语义。

表示层差异是 Go API 返回 `int`，Rust 返回 `isize`；map 的 length 在两边都先以 64 位有符号整数保存。Rust 使用 `LazyLock` 是因为静态 `HashMap` 不能直接用普通常量初始化，不意味着增加运行时配置或可变性。

Go 测试中 `pkg/server/driver_tidb_test.go`、`pkg/executor/show_test.go`、`pkg/expression/expression_test.go`、`pkg/planner/core/optimizer_test.go` 间接验证普通默认长度的上层行为。Rust 直接回归位于 [`pkg/parser/mysql/error_3_aster_unit_test.rs`](./error_3_aster_unit_test.rs) 的 `sql_states_flags_and_type_defaults_match_go` 和 [`pkg/parser/mysql/unit_test.rs`](./unit_test.rs) 的 `parser_mysql_util_exposes_type_defaults_and_auth_helpers`；前者覆盖 JSON/CHAR 差异、未知类型和三个认证插件，后者提供较小的 crate 级 API 抽样。

## 扩展指南

新增或调整 MySQL 类型默认值时，应先判定语义属于普通字段、CAST，还是两者：分别修改 `defaultLengthAndDecimal`、`defaultLengthAndDecimalForCast`，不要为了消除重复而合并两表。同步核对 `pkg/parser/mysql/util.go` 的对照语义，并扩展独立 Rust 测试；至少覆盖新类型的命中值、未知类型不回归，以及普通/CAST 值不同的场景。

扩展整数分类时，最可能修改 `IsIntegerType`。必须审查所有依赖此谓词的调用者，尤其是 DDL 无符号 flen 调整、字段类型 reorg 判断、row codec 和 ranger；把 `TypeYear` 或 `TypeBit` 纳入会造成跨模块行为变化，不能只改本文件测试。

扩展认证插件集合时，修改 `IsAuthPluginClearText` 并同步 `const.rs` 的插件常量与两个 Rust 独立测试。还应核对 Go `pkg/executor/simple.go` 对该谓词的安全含义：它不只是协议描述，也用于密码策略/双密码资格；将 LDAP、socket、token 或仅名称含 “clear” 的插件纳入可能错误允许密码操作。Rust 生产认证链当前未发现调用，新增接线应在所属 crate 的独立测试中验证，不应把测试写回本源文件。

兼容风险主要是生成元数据变化导致客户端列描述、DDL 默认值、CAST 结果类型或 schema 兼容判断变化；性能风险较低，集中在热路径查询和静态表膨胀；32 位兼容风险集中在 `i64 -> isize` 转换。所有修改都应保持源文件与测试文件分离。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/parser/mysql/util.rs` 确认目标已索引且含 6 个符号。
- RustCodeGraph `node --file pkg/parser/mysql/util.rs --offset 1 --limit 240` 与 `--offset 217 --limit 180`：读取完整 323 行实现，并报告该文件被 16 个文件使用。
- RustCodeGraph `query`：定位 Rust/Go 的 `IsIntegerType`、`GetDefaultFieldLengthAndDecimal` 和 CAST 版本。精确 `callers/callees` 命令在本地 30 秒内无输出，因此具体函数调用点改用 `rg` 核对；这里不把文件级引用数当作精确调用次数。
- 源与边界：`pkg/parser/mysql/{util.rs,util.go,Cargo.toml,lib.rs,type.rs,const.rs}`。
- Rust 调用与语义：`pkg/ddl/create_table.rs`、`pkg/parser/parser_actions/expression.rs`、`pkg/types/field_type.rs`，以及“依赖与调用关系”列出的调用文件。
- 独立测试：`pkg/parser/mysql/error_3_aster_unit_test.rs`、`pkg/parser/mysql/unit_test.rs`；Go 上层回归点为 `pkg/server/driver_tidb_test.go`、`pkg/executor/show_test.go`、`pkg/expression/expression_test.go`、`pkg/planner/core/optimizer_test.go`。

本任务是只读代码分析和文档新增，未运行 Cargo 或代码测试；按任务约束，以源/调用/对照/测试静态证据和固定章节结构检查作为验证。
