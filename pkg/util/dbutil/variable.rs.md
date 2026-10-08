# `pkg/util/dbutil/variable.rs`

## 文件定位

本文件是 `astersql-util-dbutil` crate 的 MySQL/TiDB 变量与授权查询辅助模块，由 [`pkg/util/dbutil/lib.rs`](./lib.rs) 以公开的 `variable` 模块挂载。它位于数据库驱动抽象之上：调用者提供实现了 `QueryExecutor` 的对象，模块只负责拼接 `SHOW GLOBAL VARIABLES` / `SHOW GRANTS`、解释返回值和规范化授权文本，不建立连接，也不管理事务。

当前 Rust 工作区中的直接接线仍很窄。RustCodeGraph 对文件的反向关系只找到 [`pkg/util/dbutil/variable_test.rs`](./variable_test.rs)，仓库搜索也未找到这些公开函数的生产调用点；因此这里是可复用 API 与已测试的移植实现，不能描述为已经进入服务请求主链。

## 核心职责

- `ShowVersion`、`ShowLogBin`、`ShowBinlogFormat`、`ShowBinlogRowImage` 把固定变量名交给 `ShowMySQLVariable`，统一读取 MySQL 全局变量。
- `ShowServerID` 在同一路径取回 `server_id` 后将十进制文本解析为 `u64`，并把解析失败转换为 `DbError`。
- `ShowMySQLVariable` 生成 `SHOW GLOBAL VARIABLES LIKE '<variable>';`，通过 `QueryExecutor::QueryRowContext` 取首行，并把第二列转换成字符串。
- `ShowGrants` 读取当前用户或指定 `user@host` 的授权；若首轮结果包含 MySQL 8.0 角色授权，则追加 `USING <roles>` 再查一次，使结果包含角色带来的权限。
- `normalize_grant` 修补 TiDB parser 难以接受的三种 `IDENTIFIED BY PASSWORD` 表达。虽然当前 Rust 实现不再调用 parser，这一规范化仍保持 Go 返回语义和后续消费者可解析性。

## 主要符号

- `string_value(Option<&Value>) -> Option<String>`：只接受 `Value::String` 与 `Value::Bytes`。字节使用 `String::from_utf8_lossy`，非法 UTF-8 会以替换字符保留，而 `NULL`、数值和布尔值均返回 `None`。
- `ShowVersion` / `ShowLogBin` / `ShowBinlogFormat` / `ShowBinlogRowImage`：公开、无状态的固定变量名包装器，返回 `Result<String, DbError>`。
- `ShowServerID(&dyn QueryExecutor) -> Result<u64, DbError>`：公开的数值适配器；查询错误原样传播，解析错误生成 `code = 0`、无 SQLSTATE 且带原值的错误。
- `ShowMySQLVariable(&dyn QueryExecutor, &str) -> Result<String, DbError>`：变量查询的公开公共入口。它要求结果首行至少有两列，第二列必须是字符串或字节。
- `normalize_grant(String) -> String`：私有、按顺序且每种模式至多替换一次的授权文本规范化函数。
- `read_grants(&dyn QueryExecutor, &str) -> Result<Vec<String>, DbError>`：私有查询循环；读取每行第一列，规范化后保持原顺序收集。
- `keyword_outside_quotes(&str, &str) -> Option<usize>`：私有字节扫描器，仅在单引号、双引号或反引号之外匹配不区分大小写的关键字；单/双引号内识别反斜杠跳过，三种引号均识别成对分隔符。
- `split_roles(&str) -> Vec<String>`：私有角色列表切分器，只按引号外逗号分隔、去除两侧空白并丢弃空项。
- `granted_roles(&[String]) -> Vec<String>`：私有角色提取器，只接受以 `GRANT ` 开头、存在引号外 ` TO ` 且此前没有引号外 ` ON ` 的语句。
- `ShowGrants(&dyn QueryExecutor, user, host)`：公开编排入口；空 `host` 默认 `%`，空 `user` 使用 `CURRENT_USER`。

本文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项。

## 执行流程

变量查询路径如下：包装器选择变量名；`ShowMySQLVariable` 将变量名原样插入 SQL；`QueryRowContext` 执行查询并返回首行；`string_value(row.get(1))` 读取值列。`ShowServerID` 还会执行一次 `str::parse::<u64>`。空结果由 `QueryRowContext` 产生 `sql: no rows in result set`，缺列或值类型不兼容则由本文件产生“变量值为 NULL”的 `DbError`。

授权查询先确定基础 SQL：空用户名为 `SHOW GRANTS FOR CURRENT_USER`，否则为 `SHOW GRANTS FOR '<user>'@'<host>'`。`read_grants` 取得完整结果集，逐行读取首列并执行密码片段规范化。随后 `granted_roles` 用引号感知扫描识别 `GRANT role... TO user...`，排除含引号外 ` ON ` 的普通对象权限。若没有角色，直接返回首轮结果；若有角色，则以原基础 SQL 加 ` USING ` 和逗号连接的角色列表执行第二次 `read_grants`，最终只返回展开后的第二轮结果。

该角色解析不是通用 SQL parser：它只为 Go 实现所识别的 `GrantRoleStmt` 形状提供局部判别。测试 `quoted_role_containing_on_is_expanded` 证明角色标识符内部出现 ` ON ` 时不会被误当作对象权限。

## 数据与状态

输入数据库能力以借用的 `&dyn QueryExecutor` 表示；查询参数数组始终为空，所有筛选值都已进入 SQL 文本。查询结果来自 [`pkg/util/dbutil/interface.rs`](./interface.rs) 的 `QueryResult { columns, rows }`，本文件不使用列名，只依赖列位置：变量值在第二列，授权文本在第一列。

临时状态全部为函数局部所有权：变量查询持有一行 `Vec<Value>`；授权查询持有 `Vec<String>`；引号扫描器持有当前分隔符、字节索引和切片起点。角色顺序、授权行顺序均保持服务端返回顺序。模块没有缓存、全局可变状态或跨调用状态。

## 依赖与调用关系

直接代码依赖只有同 crate 的 `interface::{DbError, QueryExecutor, Value}`。`ShowMySQLVariable` 下调 `QueryExecutor::QueryRowContext`；其默认实现再调用 `QueryContext` 并取第一行。`read_grants` 直接调用 `QueryContext`，之后调用 `string_value` 与 `normalize_grant`。`ShowGrants` 调用 `read_grants`、`granted_roles`，后者再调用 `keyword_outside_quotes` 与 `split_roles`。

[`pkg/util/dbutil/Cargo.toml`](./Cargo.toml) 声明 crate 名为 `astersql-util-dbutil`，库入口为 `lib.rs`；本文件不直接引用任何外部 crate。manifest 的常规依赖和 Windows 条件依赖属于整个 crate 边界，不能据此推断本模块直接使用 parser 或数据库驱动。

上游方面，`lib.rs` 公开 `pub mod variable`，但不在 crate 根重新导出这些函数。RustCodeGraph 的文件关系与仓库级符号搜索均只确认独立测试调用，尚无生产调用证据。下游真正的数据库 I/O 由调用者提供的 `QueryExecutor` 实现决定。

## 错误处理与边界

所有执行器错误通过 `?` 原样返回。变量首行不存在时，错误来自 `QueryRowContext`；第二列缺失、为 `NULL` 或非字符串类型时，统一生成 `variable <name> has NULL value`，因此文案不能区分缺列、NULL 和类型不匹配。`Bytes` 会宽松解码而不报 UTF-8 错误。`server_id` 必须是 Rust `u64` 可接受的十进制文本，负数、溢出或非数字都会得到带上下文的 `DbError`。

授权结果的任一行缺首列、为 `NULL` 或非字符串都会使整个调用失败，不会静默跳过；[`variable_test.rs`](./variable_test.rs) 的 `show_grants_does_not_silently_drop_null_rows` 固化了这一点。空授权结果合法返回空向量。第二轮角色查询失败时不会退回首轮结果。

变量名、用户名和主机名均按 Go 行为原样插入 SQL，没有参数绑定或转义；测试 `show_queries_preserve_go_identifier_interpolation` 明确固定了这一现状。调用者必须只传入可信、已验证的标识内容，否则存在语法破坏或 SQL 注入风险。`keyword_outside_quotes` / `split_roles` 是面向预期 `SHOW GRANTS` 输出的字节级扫描器，不验证未闭合引号，也不覆盖完整 MySQL 词法规则。

## 并发与资源生命周期

模块内部不创建线程、异步任务、锁、通道或事务，也不持有结果集句柄。所有公开入口只借用执行器到同步调用返回为止，临时 `String`、行与向量随后按 Rust 所有权自动释放。`QueryExecutor: Send + Sync` 允许实现者被并发共享，但线程安全、连接池复用、超时和取消均由实现者负责。

与 Go 版本不同，Rust API 没有 `context.Context` 参数，不能在此层直接传播取消或截止时间；`QueryResult` 是急切加载的完整结果集，也没有 Go `rows.Close()` / `rows.Err()` 对应的流式资源阶段。若以后接入真实驱动，这些生命周期语义必须在 `QueryExecutor` 实现或接口演进中明确处理。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/dbutil/variable.go`](./variable.go)，Go 回归测试是 [`pkg/util/dbutil/variable_test.go`](./variable_test.go)，Rust 独立测试是 [`pkg/util/dbutil/variable_test.rs`](./variable_test.rs)。六个公开函数的名称、固定变量名、SQL 形状、空 host 的 `%` 默认值、空 user 的 `CURRENT_USER`、密码占位修补以及有角色时的二次查询意图均保持一致。

关键迁移差异有三项。第一，Go 将 `context.Context` 传给 `database/sql`，Rust 签名没有上下文。第二，Go 通过 `Scan`、`rows.Next`、`rows.Err` 和延迟 `Close` 消费驱动结果；Rust 使用已物化的 `QueryResult` 和位置型 `Value`。第三，Go 用完整 TiDB parser 将授权语句解析为 `ast.GrantRoleStmt` 并使用 `auth.RoleIdentity::String`；Rust 用 `granted_roles` 的轻量引号扫描近似识别，不具备完整语法验证或规范化能力。

Rust 测试覆盖固定变量包装器、`server_id = 42` 解析、当前/显式用户 SQL、三类密码占位、NULL 授权错误、原样插值，以及引号内含 ` ON ` 的角色。内嵌 `GO_REFERENCE` 只是历史对照文本，实际运行断言位于同一独立测试文件后半部。

## 扩展指南

- 新增固定系统变量辅助函数时，复用 `ShowMySQLVariable`，并在独立的 `variable_test.rs` 增加准确 SQL 与返回值断言；不要把测试嵌入生产源文件。
- 改变结果列或值转换时，应优先调整 `string_value` / `ShowMySQLVariable`，同时覆盖空行、缺列、`NULL`、`Bytes`、非字符串和非法 UTF-8。注意现有宽松字节解码与错误文案可能是兼容契约。
- 扩展授权规范化时，在 `normalize_grant` 添加最小规则，并同步 Go/Rust 密码掩码用例；替换顺序和“只替换首次匹配”会影响结果。
- 支持更多角色语法时，修改 `keyword_outside_quotes`、`split_roles` 或 `granted_roles` 前，应先从 Go parser 行为提取独立回归用例。轻量扫描与完整 AST 的差异是主要正确性风险；复杂转义、注释、字符集前缀和畸形引号都需要明确决策。
- 若要接入生产主链，应在真实 `QueryExecutor` 适配层补足取消、超时、流式结果关闭和错误映射，并评估将原样 SQL 插值改为安全标识符处理的兼容影响。不能仅在本文件宣称这些能力已存在。
- 角色很多或授权结果很大时，当前实现会物化两轮完整结果并进行线性扫描；通常成本很小，但性能敏感场景应测量结果规模与二次查询开销。

## 验证依据

- RustCodeGraph：`status` 确认本仓库索引包含 11,467 个文件且可用；`files --filter pkg/util/dbutil` 确认 Rust/Go 源与测试集合；`node --file pkg/util/dbutil/variable.rs --offset 1 --limit 260` 读取全部 199 行并显示该文件只被 `variable_test.rs` 使用；对 `ShowGrants`、`normalize_grant`、`read_grants`、`granted_roles`、`keyword_outside_quotes`、`split_roles` 的查询确认符号位置。
- 源码与边界：[`variable.rs`](./variable.rs)（公开 API、查询和轻量角色解析）、[`interface.rs`](./interface.rs)（`Value`、`DbError`、`QueryResult`、`QueryExecutor` 契约）、[`lib.rs`](./lib.rs)（模块公开与测试挂载）、[`Cargo.toml`](./Cargo.toml)（crate 名、入口和依赖边界）。目标目录不存在 `doc.go`，因此无更近的包级 Go 契约可读。
- 对照与测试：[`variable.go`](./variable.go)、[`variable_test.go`](./variable_test.go)、[`variable_test.rs`](./variable_test.rs)。仓库级 `rg` 搜索未发现排除本文件和其测试之外的 Rust 调用点。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务规定的命令确认文档存在且固定二级标题恰好为 11 个，并人工复核唯一生产物、相对链接、无运行能力臆测及扩展风险说明。
