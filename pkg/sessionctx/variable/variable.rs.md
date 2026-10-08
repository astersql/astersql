# [`pkg/sessionctx/variable/variable.rs`](./variable.rs)

## 文件定位

`variable.rs` 是 `astersql-sessionctx-variable` crate 的系统变量核心模型与注册表实现。crate 根 `pkg/sessionctx/variable/lib.rs` 以私有 `mod variable` 挂载它，再通过 `pub use variable::*` 暴露其公开 API；因此调用方通常使用 crate 根路径访问 `SessionVars`、`SysVar`、`GetSysVar` 等符号，而不是直接引用本模块。

它位于 SQL 会话状态与具体内建变量定义之间：`pkg/sessionctx/variable/sysvar_builtins.rs`、`embedding_vars.rs` 构造并注册 `SysVar`，会话或 SQL 执行路径按名称查询、校验、读写变量。例如 `pkg/session/runtime/dispatch.rs` 的 `SHOW VARIABLES` 路径调用 `GetSysVars` 获取快照并过滤、排序、展示。该文件本身不枚举全部内建变量，也不负责持久化全局值；这些职责分别属于注册调用方和 `GlobalVarAccessor` 的实现。

crate 边界见 `pkg/sessionctx/variable/Cargo.toml`：本文件直接使用 `chrono` 处理 TIME、使用 `astersql-kv` 的退避默认值、使用 `astersql-parser-auth` 表示用户/角色，并依赖相邻 `vardef` crate 提供变量名、类型、作用域和默认值常量。Cargo 元数据将该 crate 对应到 Go 包 `pkg/sessionctx/variable`。

## 核心职责

1. 定义系统变量的值、错误和访问边界：`Datum`、`VariableErrorKind`、`VariableError`、`GlobalVarAccessor`。
2. 承载会话侧状态：`StatementContext` 保存校验警告，`SessionKVVars` 保存下发 KV 的参数，`SessionVars` 保存字符串变量表及执行器/规划器消费的会话字段。
3. 用 `SysVar` 描述单个系统变量的作用域、类型、范围、别名、敏感性和读写/校验钩子。
4. 实现严格与宽松两条规范化路径，包括整数、无符号整数、布尔、浮点、枚举、TIME 和 Go `time.Duration` 风格字符串。
5. 维护进程内、大小写不敏感的系统变量注册表 `SYS_VARS`，支持注册、注销、查询、替换默认值、快照和依赖优先排序。

这里的 `Context` 是轻量占位类型，`StatementContext` 也只覆盖系统变量校验所需的警告与少量标志；不能将它们等同于仓库其他模块中的完整执行上下文或完整语句上下文。

## 主要符号

- `Datum::{Int, Uint, String}`：`SysVar::GetNativeValType` 的协议回包值载体；与 `MYSQL_TYPE_LONGLONG`、`MYSQL_TYPE_VAR_STRING`、`UNSIGNED_FLAG`、`BINARY_FLAG` 一起描述近似 MySQL 类型。
- `VariableErrorKind` / `VariableError`：区分作用域错误、未知变量、类型错误、非法值、截断警告等。`unknown`、`wrong_type`、`wrong_value`、`truncated` 统一构造消息，`Display` 与 `Error` 使其可沿普通 Rust 错误链传播。
- `StatementContext`：持有私有 `warnings`；`append_warning`、`warnings`、`WarningCount` 是公开观察面，`take_warnings`/`set_warnings` 专用于宽松校验时隔离临时警告。
- `GlobalVarAccessor`：全局/实例变量及 `mysql.tidb` 表值的抽象。`set_global_sys_var` 与 `set_instance_sys_var` 默认转发给 `set_global_sys_var_only(..., true)`；具体存储、跨节点传播和事务语义由实现者负责。
- `SessionKVVars`：默认取 `kv::DefBackoffLockFast`、`kv::DefBackOffWeight` 及 `vardef` 中 txn-file 配置，其中 `DisableTxnFile` 是 enable 开关的反值。
- `SessionVars`：拥有 `systems: HashMap<String, String>`、`StmtCtx`、`GlobalVarsAccessor`、时区、用户/角色以及多项查询、DML、MPP、TiFlash 和事务参数。`new` 先调用 `crate::register_builtin_sysvars()`，再从 `vardef` 或显式兼容默认值初始化字段。
- `SessionVars::{GetSessionOrGlobalSystemVar, SetSystemVar, SetSystemVarWithRelaxedValidation}`：分别实现按作用域读取、严格设置、宽松设置。`GetMaxKeysRead` 仅在 `StmtCtx.InSelectStmt` 为真时暴露限制。
- `ValidationHook`、`SetSessionHook`、`SetGlobalHook`、`GetSessionHook`、`GetGlobalHook`、`GetStateValueHook`、`PrivilegeHook`：均以 `Arc<dyn Fn + Send + Sync>` 表示可共享钩子；钩子捕获状态时必须自行满足线程安全约束。
- `SysVar`：核心元数据。除名称、默认值、类型、范围、作用域外，还携带 empty/AUTO 规则、只读和内部变量标记、别名、依赖排序标记、缓存/初始化属性、敏感值标记和动态权限计算钩子。
- `SysVar::{Validate, ValidateFromType, ValidateWithRelaxedValidation}`：校验主入口；私有 `validateScope` 和各 `check*SystemVar` 完成具体规则。
- `parse_go_duration` / `format_go_duration`：将 Go 风格复合时长解析为纳秒并以 Go 风格重新格式化，支持 `ns/us/µs/μs/ms/s/m/h`。
- `RegisterSysVar`、`UnregisterSysVar`、`GetSysVar`、`SetSysVar`、`GetSysVars`、`OrderByDependency`：全局注册表 API。注册时还会为内置敏感变量名强制设置 `IsSensitive`。

## 执行流程

会话创建流程如下：

1. `SessionVars::new` 调用 `register_builtin_sysvars`，保证内建定义进入全局注册表。
2. 它创建空的会话 `systems` 映射、默认 `StatementContext` 和 `SessionKVVars`，并保存调用方提供的 `GlobalVarAccessor`。
3. 后续初始化代码按 `SysVar::SkipInit` 决定是否把全局/默认值装入会话；本文件只提供判断，不负责遍历加载。

严格设置流程由 `SessionVars::SetSystemVar` 串联：

1. `GetSysVar` 对名称转小写并从 `SYS_VARS` 查询；缺失返回 `UnknownSystemVariable`。
2. `SysVar::Validate` 先以 `validateScope` 拒绝只读、错误作用域和用户不可见的内部会话变量。
3. `ValidateFromType` 根据 `Type` 规范化输入；越界数值或 duration 被裁剪并通过 `StmtCtx.append_warning` 记录警告，非法格式或枚举值返回错误。
4. 类型校验成功后再执行可选 `Validation` 钩子，钩子同时收到规范化值和原始值。
5. `SetSessionFromHook` 先运行主变量的 `SetSession` 副作用，再写入 `systems`；随后对每个直接别名运行其 setter 并写值，但不递归处理别名的别名，以避免循环。

读取流程由 `GetSessionOrGlobalSystemVar` 按作用域选择：无作用域或 INSTANCE 变量直接走 `GetGlobalFromHook`；SESSION 变量优先走 `GetSessionFromHook`，若会话值尚未加载或钩子失败，则回落到全局读取。全局 getter 存在时，其返回值会经过宽松校验；无作用域变量直接返回 `SysVar::Value`；其余情况委托 `GlobalVarAccessor`。

宽松路径 `ValidateWithRelaxedValidation` 不检查作用域。它暂存整个警告列表，执行类型校验和自定义校验后恢复原列表，确保读取旧版本或外部存储值时不泄漏规范化警告。失败时通常返回原串；TIME 类型按 Go 对照行为返回空串。`SetSystemVarWithRelaxedValidation` 随后仍会执行设置钩子并写会话映射。

注册表流程中，`RegisterSysVar` 以小写键覆盖定义并将值存为 `Arc<SysVar>`；`GetSysVar` 克隆 `Arc`，读锁释放后定义仍有效；`SetSysVar` 先克隆旧定义、替换 `Value`、再重新注册；`GetSysVars` 深拷贝每个 `SysVar` 形成调用方可独立修改的快照。`OrderByDependency` 只保证 `Depended == true` 的已知变量出现在其余名称之前，不保证两组内部的稳定或字典序。

## 数据与状态

状态分为三层：

- 进程级定义状态：`SYS_VARS: LazyLock<RwLock<HashMap<String, Arc<SysVar>>>>`。键统一为小写；定义替换是整项覆盖，不原地修改已被调用方持有的 `Arc`。
- 会话级运行状态：`SessionVars.systems` 保存已经加载或设置的字符串值，其他公开字段供规划、执行、事务、MPP/TiFlash 等路径直接消费。时区 `location` 私有，只能通过 `location`/`set_location` 访问。
- 语句级诊断状态：`StatementContext.warnings` 累积截断等警告；宽松校验必须保持调用前的警告集合不变。

重要不变量包括：注册表查询大小写不敏感；严格设置必须完成“作用域 -> 类型 -> 自定义钩子 -> 设置副作用 -> 存储”的顺序；setter 失败时主值不写入，但别名更新过程中若后续别名失败，先前副作用和写入没有回滚机制；`GetSysVars` 返回深拷贝而不是共享定义；敏感变量的 `IsSensitive` 只影响诊断脱敏约定，不改变 SQL getter 语义。

`parse_go_duration` 使用 `f64` 累加各段再截断为纳秒，并拒绝非有限值及超过 `i64::MAX` 的正向总量。它允许前导正负号和复合单位；`format_go_duration` 按量级选择 ns、µs、ms 或 h/m/s 并裁去小数尾零。该实现服务于与 Go `time.Duration` 字符串兼容的变量校验，不是通用任意精度时长库。

## 依赖与调用关系

上游与接线证据：

- `pkg/sessionctx/variable/lib.rs` 再导出本文件公开符号，并将 `variable_test.rs` 作为独立 `#[cfg(test)]` 模块挂载。
- `pkg/sessionctx/variable/sysvar_builtins.rs` 大量调用 `RegisterSysVar` 构建内建表，并复用 `parse_go_duration`/`format_go_duration`；`register_noop_compatibility_vars` 还以 `GetSysVar` 避免覆盖已有完整实现。
- `pkg/sessionctx/variable/embedding_vars.rs` 为 embedding 配置构造带读写钩子的变量后调用 `RegisterSysVar`。
- `pkg/session/runtime/dispatch.rs` 的 `SHOW VARIABLES` 路径调用 `GetSysVars`，过滤 global/internal 项后生成结果行；连接配置路径调用会话变量的 `SetSystemVar` 语义设置连接排序规则。
- `pkg/extension/lib.rs` 再导出 `ConnectionInfo`、`GetSysVar`、`RegisterSysVar`、`SysVar`、`UnregisterSysVar`，使扩展变量进入同一注册表。

下游依赖：

- `vardef` 决定 `ScopeFlag`、`TypeFlag`、变量名和默认值；作用域判断当前通过 `ScopeFlag::String()` 后匹配 `SESSION/GLOBAL/INSTANCE`。
- `GlobalVarAccessor` 隔离全局系统变量和 `mysql.tidb` 存储；本文件不假设具体数据库实现。
- `chrono` 解析、格式化 TIME，并在短格式输入中附加当前会话固定偏移。
- `kv` 与 `vardef` 提供 `SessionKVVars` 默认值；`parser_auth` 提供 `UserIdentity`、`RoleIdentity`。
- `crate::error::errGlobalVariable` 提供 SESSION 写入 global-only 变量时的兼容错误文本，`crate::TiDBOptOn` 用于布尔值转协议整数。

RustCodeGraph 将该文件标为被 `pkg/session/runtime/dispatch.rs`、`pkg/sessionctx/variable/session.rs`、`pkg/sessionctx/stmtctx/stmtctx.rs` 及测试文件等 9 个文件使用；对 `SetSystemVar` 的 callee 图明确给出 `GetSysVar -> Validate -> SetSessionFromHook`。图对若干 caller 查询未产出边，因此上述具体上游位置又以源码引用核验，未把缺失图边解释为“没有调用者”。

## 错误处理与边界

`VariableErrorKind` 是可程序化判断的粗粒度分类，消息负责兼容可读语义。未知名称、非法类型/枚举、只读或错误作用域都返回 `Result::Err`；数值和 duration 越界不是硬错误，而是裁剪到 `MinValue`/`MaxValue` 并追加 `TruncatedWrongValue` 警告。

边界行为：空串只有在 `AllowEmpty && SESSION` 或 `AllowEmptyAll` 时直接通过；无符号负数只要能解析为 `i64` 就裁剪到最小值；`AllowAutoValue` 仅特许精确字符串 `-1`；布尔接受大小写不敏感的 ON/OFF、0/1，开启 `AutoConvertNegativeBool` 后负整数也转 ON；枚举既接受名字也接受从 0 开始的下标；原生无符号转换失败时 `GetNativeValType` 回退为 0，而不是返回错误。

TIME 的短输入只按 `%H:%M` 解析，输出带当前会话固定时区偏移；长输入要求 `%H:%M %z`。这比 Go 的具名时区和包含秒/纳秒的 `time.ParseInLocation` 能力窄，扩展前必须用 Go 对照用例确认兼容范围。duration 解析也以浮点中间值实现，极端精度和负溢出边界需要额外测试。

锁中毒通过 `expect("sysvar registry poisoned")` 触发 panic，而不是转成 `VariableError`。注册、注销和替换是进程全局副作用；测试或扩展必须成对清理自定义变量。`clear_sys_vars_for_test` 会清空整个注册表，只应在隔离测试环境使用。

## 并发与资源生命周期

`SYS_VARS` 首次访问时惰性创建，`RwLock` 允许并发查询/快照并串行化注册和注销。`GetSysVar` 在读锁内克隆 `Arc<SysVar>`，因此锁释放后引用仍安全；重新注册只影响之后的查询，不会修改旧 `Arc`。`GetSysVars` 在读锁内克隆全部定义，快照与后续注册表变化相互独立。

钩子采用 `Arc` 且要求 `Send + Sync`，可以随 `SysVar` 克隆并跨线程共享；但 `SessionVars` 中的访问器是 `Box<dyn GlobalVarAccessor>`，其可变方法仍要求调用方持有 `&mut SessionVars`。本文件没有异步任务、通道或显式事务生命周期，也不为一组别名副作用提供原子回滚。

敏感变量名单在每次 `RegisterSysVar` 时检查，即使扩展构造时遗漏标志，列出的内建名称也会被强制脱敏。扩展注册的其他密钥变量必须主动设置 `IsSensitive`；注册表无法从名称之外推断秘密内容。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/sessionctx/variable/variable.go`。Rust 保留了 Go 的 `SysVar` 元数据布局、hook 调用次序、作用域规则、类型裁剪、别名非递归更新、注册表大小写归一化、快照复制和依赖优先排序。`pkg/sessionctx/variable/tests/variable_test.rs` 保存大段 Go 测试迁移参考，并在文件后部提供部分可执行 Rust 覆盖；真正由 crate 挂载的聚焦测试是同目录 `variable_test.rs`。

已验证的关键对应关系：

- Go `Validate` 与 Rust `Validate` 都先查作用域，再做类型规范化，最后执行自定义 validation。
- Go `ValidateWithRelaxedValidation` 与 Rust 实现都屏蔽校验错误并恢复旧警告；Rust 对 TIME 错误显式返回空串，对应 Go type validator 的空 normalized value。
- Go 的别名 setter 不递归调用别名的别名；Rust `SetSessionFromHook`/`SetGlobalFromHook` 保持该防循环策略。
- Go 注册表用 `sync.RWMutex + map[string]*SysVar`；Rust 用 `LazyLock<RwLock<HashMap<String, Arc<SysVar>>>>`，并通过克隆实现替换与快照。
- Go `time.ParseDuration`/`Duration.String` 由 Rust 的 `parse_go_duration`/`format_go_duration` 本地复刻；`variable_test.rs::duration_validation_matches_go_string_precision` 覆盖 `0`、纳秒小数和秒小数格式。

当前差异与迁移边界也必须保留：Rust `Context` 是占位；Rust `StatementContext` 和 `SessionVars` 仅是本文件所需模型的一部分；Rust TIME 使用 `FixedOffset` 而非 Go `time.Location`，短格式仅解析到分钟；Rust错误没有 Go terror/MySQL 错误码栈的完整结构。`pkg/sessionctx/variable/tests/variable_test.rs` 前段的 `GO_REFERENCE` 是迁移参考文本而非可执行测试，不能据此声称所有 Go 用例已经落地。

## 扩展指南

新增普通系统变量时，优先在 `sysvar_builtins.rs` 的注册流程中构造 `SysVar`，不要绕过 `RegisterSysVar` 直接操作注册表。明确选择 `Scope`、`Type`、默认值和范围；若值含秘密，设置 `IsSensitive`；若另一个变量的回放依赖它，设置 `Depended`；若存在别名，确保别名关系不会依赖递归传播。

新增类型或改变校验规则时，修改 `ValidateFromType` 和对应 `check*SystemVar`，并同步独立测试 `pkg/sessionctx/variable/variable_test.rs`。需要更广的兼容矩阵时，应把 `pkg/sessionctx/variable/tests/variable_test.rs` 中相关 Go 参考逐条转为真正可执行的独立 Rust 测试，而不是把测试内嵌回生产文件。作用域或错误文本变化还应同步 `pkg/session/test/variable/variable_test.rs` 的注册/作用域回归。

新增 getter/setter 时要维持现有次序和失败语义：validation 不应产生持久副作用；session setter 成功后才写主值；别名 setter 不递归；global setter 若自行处理写入会立即返回，不再自动传播别名。若要求跨别名原子性，需要在本文件现有契约之外设计显式回滚，不能假定当前实现已经保证。

修改注册表并发模型时，要保持大小写不敏感、读后持有对象有效、`GetSysVars` 返回独立快照这三个外部契约，并关注 `SHOW VARIABLES` 和扩展注册路径。修改 duration/TIME 时应与 `variable.go` 逐边界对照，至少覆盖空值、非法单位、正负号、上下界、精度、时区偏移和宽松校验不污染警告。

主要风险：错误/规范化差异会影响 MySQL 兼容性；默认值或作用域差异会改变新会话行为；全局锁粒度和全表深拷贝影响高频查询性能；公开字段或钩子签名变化会影响 crate 内建变量、扩展 API 和执行路径。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/sessionctx/variable` 定位目标、Go 对照和测试；`node --file pkg/sessionctx/variable/variable.rs --offset 1/501/801` 阅读完整 1,081 行；`query` 定位 `SessionVars`、`SysVar`、`GetSysVar`、`RegisterSysVar`、`SetSystemVar`、`parse_go_duration`、`OrderByDependency`；`callees SetSystemVar` 验证严格设置链。
- 生产源码：[`variable.rs`](./variable.rs)；模块入口 [`lib.rs`](./lib.rs)；注册调用方 [`sysvar_builtins.rs`](./sysvar_builtins.rs)、[`embedding_vars.rs`](./embedding_vars.rs)；运行时读取方 [`pkg/session/runtime/dispatch.rs`](../../session/runtime/dispatch.rs)；扩展再导出 [`pkg/extension/lib.rs`](../../extension/lib.rs)。
- crate 配置：[`Cargo.toml`](./Cargo.toml)，用于核对 crate 名、依赖、禁用自动测试发现和 Go 包映射。
- Go 对照：[`variable.go`](./variable.go)，用于核对 hook、校验器、注册表和依赖排序语义。
- Rust 测试：[`variable_test.rs`](./variable_test.rs) 覆盖 TIME 宽松失败、duration 格式精度和全局 duration hook；[`pkg/session/test/variable/variable_test.rs`](../../session/test/variable/variable_test.rs) 覆盖注册/注销及四类作用域；[`tests/variable_test.rs`](./tests/variable_test.rs) 同时包含迁移参考和后部可执行的注册、缓存跳过、宽松校验、依赖排序等覆盖，引用时已区分二者。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证目标文档存在且恰好包含 11 个固定二级标题，并人工复核未把占位上下文、参考测试或缺失调用图边描述为完整实现。
