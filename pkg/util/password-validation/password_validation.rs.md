# `pkg/util/password-validation/password_validation.rs`

## 文件定位

本文件是 `astersql-util-password-validation` crate 的密码策略实现文件。crate 入口 [`lib.rs`](lib.rs) 将它声明为私有模块后完整再导出其公开 API；工作区根 [`pkg/lib.rs`](../../lib.rs) 又通过 `util::password_validation` 门面再导出该 crate。它读取会话所连接的全局系统变量，按照 LOW、MEDIUM、STRONG 三层策略校验明文密码，但不负责保存、散列或验证密码哈希。

目前 Rust 侧的接线仍是分层的：[`pkg/executor/simple.rs`](../../executor/simple.rs) 在 `CREATE USER`、`ALTER USER` 和 `SET PASSWORD` 路径调用 `SimpleBackend::validate_password`，但仓库内生产代码没有发现该 trait 方法到本文件 `ValidatePassword` 的直接实现或调用；本文件可确认的 Rust 调用者是同 crate 的两个独立测试模块。因此它是已经实现并由测试覆盖、也已被 executor 声明为 Cargo 依赖的策略库，而不是已经证实贯通 Rust SQL 执行主链的实现。

## 核心职责

- `ValidatePassword` 组织总流程：先取得策略值，再依次执行用户名、LOW、MEDIUM 和（非 LOW/MEDIUM 时）字典检查，并把警告文本转成稳定的 `ErrNotValidPassword` 错误。
- `ValidateUserNameInPassword` 防止密码包含当前用户的认证用户名或登录用户名，以及它们的逐字节反序形式。
- `ValidatePasswordLowPolicy` 按 Unicode 标量数量执行最小长度检查。
- `ValidatePasswordMediumPolicy` 统计 Unicode 大写、小写、十进制数字和其余字符，并按固定优先级检查阈值。
- `ValidateDictionaryPassword` 对分号分隔的全局字典执行大小写不敏感的子串检查，只采用字节长度在 4 到 100（含）之间的字典项。
- `PasswordValidationSession` 和 `PasswordValidationContext` 将策略逻辑与完整会话所有权解耦，同时为真实 `variable::SessionVars` 提供适配。

本文件只做同步、只读的策略判断，不修改全局变量、用户对象或会话状态。

## 主要符号

- `maxPwdValidationLength: usize = 100`、`minPwdValidationLength: usize = 4`：字典项参与匹配的字节长度闭区间。名称沿用 Go 版本。
- `PasswordValidationError`：公开错误枚举。
  - `SystemVariable(VariableError)` 透传系统变量访问失败；
  - `InvalidSystemVariableValue { name, value, source }` 保留变量名、原始值和整数解析源错误；
  - `InvalidPassword { code, message }` 表示策略不满足，并保留 TiDB 密码错误码和格式化消息。
- `PasswordValidationResult<T>`：上述错误类型的 `Result` 别名。
- `PasswordValidationSession`：只要求 `global_vars_accessor()` 和 `user()` 两个借用接口；本文件分别为 `PasswordValidationContext<'_>` 与 `variable::SessionVars` 实现它。
- `PasswordValidationContext::new`：从 `&dyn GlobalVarAccessor` 和可选 `&UserIdentity` 构造轻量借用上下文，主要用于不持有完整会话的调用方和测试。
- `parse_global_count`：内部整数系统变量读取器，统一将 `parse::<i64>()` 失败包装为 `InvalidSystemVariableValue`。
- `invalid_password`：内部错误构造器，从 `variable::error::ErrNotValidPassword` 取得稳定错误码并格式化原因；测试确认当前错误码为 1819。
- `ValidateDictionaryPassword`、`ValidateUserNameInPassword`、`ValidatePasswordLowPolicy`、`ValidatePasswordMediumPolicy`：可单独调用的策略步骤，成功时分别返回布尔值或空/非空警告字符串。
- `ValidatePassword<S: PasswordValidationSession + ?Sized>`：公开总入口，成功返回 `()`，任何访问、解析或策略失败均提前返回错误。

## 执行流程

`ValidatePassword` 的实际顺序如下，顺序本身影响错误优先级和访问哪些系统变量：

1. 从 `session_vars.global_vars_accessor()` 取得访问器，首先读取 `ValidatePasswordPolicy`；读取失败立即返回。
2. 调用 `ValidateUserNameInPassword`。该函数先读取 `ValidatePasswordCheckUserName`；开关不是 `TiDBOptOn`、当前用户不存在或两个用户名均为空时放行。否则依次检查 `auth_username`、`username` 的原字节串和逐字节反序字节串，首次命中返回对应警告。
3. 调用 `ValidatePasswordLowPolicy`，读取并解析 `ValidatePasswordLength`，以 `pwd.chars().count()` 比较 Unicode 标量数。长度不足时生成 `Require Password Length: N`。
4. 若策略字符串严格等于 `"LOW"`，到此成功返回。
5. 调用 `ValidatePasswordMediumPolicy`。它先完整扫描密码，然后读取 `ValidatePasswordMixedCaseCount`，依次判断小写和大写；再读取 `ValidatePasswordNumberCount` 判断十进制数字；最后读取 `ValidatePasswordSpecialCharCount` 判断剩余字符。第一项不足即返回警告。
6. 若策略字符串严格等于 `"MEDIUM"`，到此成功返回。
7. 其余策略值均进入 STRONG 路径：读取 `ValidatePasswordDictionary`，按分号切分；每个合法字节长度的字典项和密码都先转小写，再做子串匹配。命中时 `ValidateDictionaryPassword` 返回 `false`，总入口把它转换成字典原因的无效密码错误。

用户名、LOW、MEDIUM 返回的非空警告都由 `invalid_password` 转成 `InvalidPassword`，所以调用者不会从总入口得到裸警告字符串。

## 数据与状态

输入密码和用户身份全程只借用。`PasswordValidationContext` 是两个引用组成的 `Copy` 值，不拥有访问器或用户；`SessionVars` 适配直接借用其 `GlobalVarsAccessor` 与 `User` 字段。每次调用都现场读取全局变量，因此一次校验看到的是各次读取时访问器返回的值，本文件不缓存策略快照。

LOW 的长度单位是 Rust `char`（Unicode 标量），与 Go `[]rune` 对齐，但不是用户感知的字素簇。字典项长度和用户名匹配均采用 UTF-8 字节；字典比较会进行 Unicode 小写转换，用户名比较保持大小写敏感。MEDIUM 的大/小写判定使用 Rust Unicode case property，数字仅接受 `GeneralCategory::DecimalNumber`（Nd），其余所有标量——包括标点、符号以及不属于这些类别的字符——计入特殊字符。

函数内的临时状态包括字符类别计数器、分割后的字典引用数组、密码小写副本、每个字典项的小写副本，以及用户名反序 `Vec<u8>`；调用结束即释放，不写回会话。

## 依赖与调用关系

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义：`parser_auth` 提供 `UserIdentity`，`astersql_sessionctx_vardef` 提供 `ValidatePassword*` 变量名，`astersql_sessionctx_variable` 提供 `SessionVars`、`GlobalVarAccessor`、`TiDBOptOn`、`VariableError` 和密码错误描述符，`unicode-general-category` 用于精确识别 Nd 数字。`lib.rs` 通过局部 `parser`/`sessionctx` 再导出兼容路径供本实现引用。

内部调用边为：`ValidatePassword` → `ValidateUserNameInPassword`、`ValidatePasswordLowPolicy`、`ValidatePasswordMediumPolicy`、`ValidateDictionaryPassword`、`invalid_password`；LOW 和 MEDIUM 检查 → `parse_global_count`；所有步骤通过 `GlobalVarAccessor::get_global_sys_var` 向下读取配置。RustCodeGraph 能确认这些内部边，但其 `callers` 查询没有返回上游调用边；仓库文本检索只发现两个独立 Rust 测试文件调用这些 API。

Go 主链的直接证据位于 [`pkg/executor/simple.go`](../../executor/simple.go)：创建用户、修改用户和设置密码会在密码校验启用时调用同目录 Go 包的 `ValidatePassword`。Rust [`pkg/executor/simple.rs`](../../executor/simple.rs) 的对应路径调用的是 `SimpleBackend::validate_password` 抽象，尚不能据此断言本 crate 已接入生产后端。

## 错误处理与边界

- 任意 `get_global_sys_var` 失败都通过 `From<VariableError>` 变为 `SystemVariable`，保留源错误；总入口按执行顺序短路，因此后续变量不会被读取。
- 计数变量使用有符号 `i64`。非法语法和溢出分别格式化为 Go 风格的 `invalid syntax` 与 `value out of range`；错误对象仍保留实际 `ParseIntError` 和变量名。
- 策略不满足返回 `InvalidPassword`。`Display` 只输出 TiDB 格式化消息，`code` 需通过枚举字段读取；该变体没有下层 source。
- 用户名检查关闭、用户不存在、用户名为空时放行；匹配大小写敏感。逐字节反序对非 ASCII 用户名不等价于 Unicode 字符反序，这是对 Go 字节实现的刻意对齐。
- 字典检查大小写不敏感、采用子串而非整词匹配。短于 4 或长于 100 字节的项被忽略；空字典切分会得到空项，但因长度门槛自然放行。
- LOW 使用 Unicode 标量数而非字节数；组合字符和 emoji 序列可能与用户视觉长度不同。
- MEDIUM 的判断优先级固定为小写、大写、数字、特殊字符，测试依赖这一顺序。负数阈值会自然全部满足，本文件不额外做范围校验，变量合法范围应由系统变量层保证。
- 只有精确的 `"LOW"`、`"MEDIUM"` 会提前返回；包括 `"STRONG"` 在内的其他字符串均执行字典步骤。本文件不自行验证策略枚举值。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、事务或 I/O 资源句柄。所有 API 都是同步借用调用；并发安全性取决于调用方提供的 `GlobalVarAccessor`。trait 方法只要求共享引用 `&self`，本文件不会在读取之间加锁，也不保证一次校验的多个配置读取来自同一原子快照。

主要分配点是字典分割形成的 `Vec<&str>`、密码和字典项的小写字符串、用户名反序字节向量，以及警告/错误消息。字典扫描复杂度随密码长度和合法字典项总长度增长；每个合法项都会单独小写化并执行子串搜索。所有临时值都限制在单次函数调用生命周期内。

## 与 Go 版本的对应关系

直接对照文件是 [`password_validation.go`](password_validation.go)，Rust 保留了 Go 的公开函数命名、常量值、策略顺序、系统变量读取顺序、警告文本、用户名检查顺序和字典布尔语义（`true` 表示未命中、可以通过）。`PasswordValidationContext`、`PasswordValidationSession` 和显式错误枚举是 Rust 为借用与类型化错误增加的适配层。

关键语义对应如下：Go `len([]rune(pwd))` 对应 Rust `pwd.chars().count()`；Go `unicode.IsUpper/IsLower` 对应 `char::is_uppercase/is_lowercase`；Go `unicode.IsDigit` 在本移植中由 Unicode General Category 的 `DecimalNumber` 表达；Go 的 `bytes.Contains` 与逐字节反序对应 Rust 字节窗口和 `Vec<u8>`；Go `ErrNotValidPassword.GenWithStackByArgs` 对应 `invalid_password` 生成的稳定 code/message，但 Rust 不构造 Go 风格堆栈。

[`password_validation_test.go`](password_validation_test.go) 提供原始字典、用户名、LOW、MEDIUM 和三层总流程用例。Rust [`password_validation_test.rs`](password_validation_test.rs) 复刻这些用例并验证真实 `SessionVars` 适配；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 另覆盖字节长度窗口、Unicode 标量与类别、变量读取顺序、错误码 1819 和未知变量错误形态。

迁移差异仍需注意：Go 生产执行器直接调用 Go 实现；Rust 执行器当前只暴露后端校验接口，尚未找到调用本实现的生产后端。另一个细节是 Go `strconv.ParseInt(..., 10, 64)` 的错误文本由 Rust 手工映射为相同风格，而不是相同错误类型。

## 扩展指南

新增或修改策略时，应优先在对应的独立策略函数中实现，再在 `ValidatePassword` 中按期望错误优先级接线；不要把测试嵌入本源文件。若新增系统变量，需要同步 `sessionctx/vardef` 的定义和系统变量合法性约束，并评估一次校验中变量读取顺序是否改变。新增错误原因应继续通过 `invalid_password` 使用 `ErrNotValidPassword`，以保持客户端可见错误码和消息模板兼容。

涉及字符规则时必须先明确单位：用户名当前是大小写敏感 UTF-8 字节子串，字典长度是字节、匹配前做 Unicode 小写，LOW 是 Unicode 标量，MEDIUM 是 Unicode 属性。改变任何一项都可能与 Go、MySQL/TiDB 行为及已有密码策略配置不兼容。字典算法若改为预编译或缓存，还需定义配置失效、并发同步和内存上限；当前实现没有这些生命周期问题。

测试应同步放在 [`password_validation_test.rs`](password_validation_test.rs)；若是迁移边界、错误形态或 Go 差异，也应更新 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 以及 Go 对照测试。若目标是让 Rust SQL 主链实际使用本实现，还必须为 `SimpleBackend::validate_password` 增加明确的生产适配，并分别验证 `CREATE USER`、`ALTER USER`、`SET PASSWORD` 的启用开关、认证插件限制和错误传播；仅保留 executor 的 Cargo 依赖不能证明接线完成。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/util/password-validation` 找到本 crate 的 `lib.rs`、本实现、两个 Rust 测试及 Go 对照文件。
- RustCodeGraph 源码与符号：`node --file pkg/util/password-validation/password_validation.rs --offset 1 --limit 400` 覆盖本文件 342 行；`query` 定位 `ValidatePassword`、四个策略函数、`parse_global_count` 和 `invalid_password`；`callees ValidatePassword` 确认总入口到各策略函数及错误构造器的调用边，LOW/MEDIUM 的查询确认到 `parse_global_count` 的边。
- crate 与门面：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、工作区根 [`Cargo.toml`](../../../Cargo.toml) 和 [`pkg/lib.rs`](../../lib.rs)。
- Rust 上游与接线核验：[`pkg/executor/Cargo.toml`](../../executor/Cargo.toml) 声明本 crate 依赖；[`pkg/executor/simple.rs`](../../executor/simple.rs) 的 `SimpleBackend::validate_password` 及三个账户操作调用点；全仓 Rust 检索未发现生产代码直接调用本文件 `ValidatePassword`。
- Go 对照与主链：[`password_validation.go`](password_validation.go)、[`password_validation_test.go`](password_validation_test.go) 和 [`pkg/executor/simple.go`](../../executor/simple.go) 的三个直接调用点。
- Rust 独立测试：[`password_validation_test.rs`](password_validation_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。本任务依照计划只做文档分析，未运行 Cargo 或测试二进制；测试文件用于行为证据读取。
