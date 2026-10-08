# `pkg/privilege/privileges/errors.rs`

## 文件定位

[`errors.rs`](errors.rs) 是 `astersql-privilege-privileges` crate 的错误模型文件。crate 入口 [`lib.rs`](lib.rs) 以私有 `mod errors` 装配该模块，再通过 `pub use errors::*` 将其公开项提升到 crate 根，因此同 crate 的 `cache.rs`、`privileges.rs`、`tidb_auth_token.rs` 以及外部依赖者都可以从 crate 根使用 `PrivilegeError` 和标准错误元数据常量。

本文件处在权限数据加载、连接鉴权、动态权限注册和 Auth Token/JWKS 校验的共同错误边界上，但它本身不执行权限判断，也不读写权限状态。其直接依赖只有 `thiserror::Error`；[`Cargo.toml`](Cargo.toml) 声明包名为 `astersql-privilege-privileges`，并把 `lib.rs` 设为库入口。

## 核心职责

文件提供两套相关但当前未自动互转的表示：

1. `PrivilegeError` 是 Rust 业务路径实际返回的强类型错误。各变体保留鉴权主体、失败次数或底层错误文本等运行时上下文，并由 `thiserror` 生成 `Display` 与 `std::error::Error` 实现。
2. `PrivilegeErrorKind` 及七个公开常量保存 Go `dbterror.ClassPrivilege.NewStd` 所对应的稳定名称、MySQL/TiDB 错误码和格式化消息模板。它们用于移植一致性校验；仓库搜索显示这些常量当前只被 [`errors_test.rs`](errors_test.rs) 使用，生产路径尚未把 `PrivilegeError` 映射成这些协议级标准错误。

因此，“Rust 内部错误可读”与“Go/MySQL 协议错误码完全接线”是两种不同能力。当前文件实现了前者并登记了后者的元数据，不能仅凭常量存在就断言连接层已经按这些代码向客户端编码错误。

## 主要符号

- `pub enum PrivilegeError`：可克隆、可比较的权限错误枚举（`Clone + Debug + Error + PartialEq + Eq`）。
  - `InvalidPrivilegeType(String)`：动态权限名为空、过长或重复等注册错误；调用点见 `RegisterDynamicPrivilege`。
  - `NonexistingGrant(String)`、`LoadPrivilege(String)`：分别表达 GRANT 不存在与权限加载失败。当前仓库的 Rust 生产代码未检索到构造点，应视为预留/尚未接线变体，而不是已覆盖路径。
  - `AccessDenied { user, host }`：用户/主机无法匹配、SSL/哈希/密码校验失败或会话令牌无效时的通用拒绝错误。
  - `AccountLocked { user, host }`：显式账户锁定；由 `UserPrivileges::ConnectionVerification` 返回。
  - `PasswordLock { user, host, attempts, remaining }`：连续失败登录触发的自动锁定；由 `GenerateAccountAutoLockErr` 构造。
  - `MustChangePassword`：密码过期且未启用沙箱模式；由 `UserPrivileges::CheckPasswordExpired` 返回。
  - `NoSuchTable(String)`：权限系统表缺失；`cache.rs::noSuchTable` 用模式匹配识别。当前文件不决定该错误何时产生。
  - `Authentication(String)`：认证插件、密码哈希、JWT 格式/签名/claims 等失败的内部分类。
  - `InvalidJson(String)`：用户属性时间字段或 JWKS/claims JSON 解析失败。
  - `Io(String)`：JWKS 文件读取失败。
- `pub struct PrivilegeErrorKind`：由三个私有只读字段 `name: &'static str`、`code: u16`、`message_template: &'static str` 组成的标准错误描述符。`new` 是模块私有的 `const fn`；公开的 `name`、`code`、`message_template` 均按值读取且为 `const fn`。
- 七个 `PrivilegeErrorKind` 常量：`ErrInvalidPrivilegeType`(8050)、`ErrNonexistingGrant`(1141)、`ErrLoadPrivilege`(8049)、`ErrAccessDenied`(1045)、`ErrAccountHasBeenLocked`(3118)、`ErUserAccessDeniedForUserAccountBlockedByPasswordLock`(3955)、`ErrMustChangePasswordLogin`(1862)。名称刻意保持 Go 风格；crate 根允许非 Rust 惯用命名。

文件没有 trait、普通运行时函数、`impl PrivilegeError`、条件编译项或全局可变状态。

## 执行流程

错误传播的典型流程如下：

1. 上游业务函数执行权限相关工作。例如 `cache.rs::MySQLPrivilege::LoadAll` 顺序加载多个 `mysql.*` 权限表；`privileges.rs::UserPrivileges::ConnectionVerification` 校验账户锁定、SSL、哈希、密码和过期状态；`tidb_auth_token.rs::JWKSImpl` 读取密钥并验证 JWT。
2. 失败点直接构造对应的 `PrivilegeError`，或以 `map_err` 把 I/O、JSON、JWT/密钥错误的文本转换到 `Io`、`InvalidJson`、`Authentication`。使用 `?` 的调用链保持同一枚举继续上抛。
3. 调用者可以对变体做结构化分支。例如 `cache.rs::noSuchTable` 只识别 `NoSuchTable`；测试也直接匹配 `InvalidPrivilegeType` 和 `MustChangePassword`。
4. 当错误最终被格式化时，`thiserror` 使用各变体的 `#[error(...)]` 文本。该文本与 `PrivilegeErrorKind::message_template` 是两套独立模板：前者已经填入 Rust 字段，后者仍含 Go/MySQL 的 `%s`、`%d`、宽度限制等占位符。

标准错误元数据本身没有运行时查表流程：七个常量在编译期由私有 `PrivilegeErrorKind::new` 构造，使用者通过三个访问器取得静态值。当前没有从枚举变体选择标准错误常量的匹配函数。

## 数据与状态

`PrivilegeError` 所有载荷均为拥有所有权的 `String` 或标量 `i64`，不借用业务对象，也不保存密码、token、锁或缓存引用。`AccessDenied`、`AccountLocked`、`PasswordLock` 保存用户名与主机；`PasswordLock` 额外保存失败次数和已经格式化为字符串的剩余锁定描述。其可克隆、可比较性质使测试能够精确匹配，但复制会克隆字符串。

`PrivilegeErrorKind` 只持有三个 `'static` 字符串切片和一个 `u16`，并实现 `Copy`。所有实例都是编译期常量，不分配堆内存、不需要初始化顺序，也没有可变入口。字段私有保证 crate 外只能经访问器读取，不能构造或篡改现有描述符。

文件自身不维护错误链或底层 `source`：底层错误均被转换为 `String`，因此保留可读信息但丢失原始具体错误类型及链式 downcast 能力。

## 依赖与调用关系

- 装配与公开：[`lib.rs`](lib.rs) 声明 `mod errors`、在测试配置下声明 `mod errors_test`，并 `pub use errors::*`。这使 `tidb_auth_token.rs` 可用 `crate::PrivilegeError`，外部 crate 也能从库根引用公开符号。
- 下游依赖：本文件仅依赖 `thiserror::Error` 派生宏；[`Cargo.toml`](Cargo.toml) 指定 `thiserror = "2"`。
- 权限缓存：[`cache.rs`](cache.rs) 的 `PrivilegeDataSource` 方法、`MySQLPrivilege::LoadAll`/各表加载函数和 `Handle::Update*` 统一返回 `Result<_, PrivilegeError>`；`noSuchTable` 对 `NoSuchTable` 分类。
- 连接与权限逻辑：[`privileges.rs`](privileges.rs) 的认证、账户锁定、密码过期、动态权限注册、用户属性 JSON 解析等路径构造 `AccessDenied`、`AccountLocked`、`PasswordLock`、`MustChangePassword`、`Authentication`、`InvalidPrivilegeType`、`InvalidJson`。
- Auth Token/JWKS：[`tidb_auth_token.rs`](tidb_auth_token.rs) 的文件读取、JSON 解码、JWT 头/签名/claims 处理构造 `Io`、`InvalidJson`、`Authentication`。
- 跨 crate 接口：`pkg/privilege/privilege.rs` 的权限管理 trait 使用另一个导入别名 `GoError as PrivilegeError`，并非本文件枚举；阅读调用边时不能因同名而混为一谈。
- RustCodeGraph 已索引目标文件及上述相邻模块，但对限定的 `errors.rs::PrivilegeError`/`PrivilegeErrorKind` 未返回静态 callers/callees；这里的调用关系由仓库精确符号搜索和对应源码位置补证。

## 错误处理与边界

- `thiserror` 的展示字符串是内部 Rust 错误语义。例如 `AccessDenied` 不包含 Go 标准模板中的 `using password` 参数，`PasswordLock` 的字段也不等同于标准模板需要的“锁定天数/剩余天数/失败次数”完整参数序列。若协议层需要 Go 兼容错误，必须显式设计映射，不能直接复用当前 `Display` 文本冒充协议结果。
- `Authentication(String)`、`InvalidJson(String)`、`Io(String)` 是宽类别，分类之外只留下文本；扩展时应避免把可被上游可靠处理的新边界全部塞入字符串。
- `PrivilegeErrorKind::new` 私有且字段为静态引用，限制了运行时动态登记标准错误；新增标准错误应在本文件声明常量并以 errno 权威定义核对。
- `NonexistingGrant`、`LoadPrivilege` 在当前 Rust 生产路径没有检索到构造点；`NoSuchTable` 有识别点但未在本 crate 检索到构造点。文档只记录现状，不推断未索引的外部构造或未完成行为。
- 错误中包含 `user`、`host` 和底层错误文本。日志或外部响应层仍需决定脱敏与暴露策略，本文件没有进行脱敏。
- 文件没有 `From` 实现；I/O、JSON、JWT 等错误必须由调用点显式 `map_err`，这维持分类可见性，也意味着新增失败来源不会自动进入 `PrivilegeError`。

## 并发与资源生命周期

本文件不创建线程、任务、通道、锁、事务、文件句柄或网络资源。错误值拥有其字符串数据，可跨普通返回路径移动；是否满足跨线程发送要求由字段和派生 trait 自动决定，本文件没有显式承诺并发 API。

并发资源位于调用者：例如 `tidb_auth_token.rs::JWKSImpl` 在 `RwLock` 中维护密钥集并可能启动刷新线程，`cache.rs::Handle` 通过 `Arc<RwLock<_>>` 和 `AtomicBool` 管理权限快照。错误只从这些操作中返回，不持有锁守卫；相邻代码在构造/返回错误前结束临时借用，因此错误生命周期不会延长锁占用。`PrivilegeErrorKind` 常量为不可变静态数据，可无同步读取。

## 与 Go 版本的对应关系

Go 同路径 [`errors.go`](errors.go) 只声明七个 `dbterror.ClassPrivilege.NewStd(mysql.<errno>)` 错误变量：两个为包内小写（`errInvalidPrivilegeType`、`errLoadPrivilege`），五个为导出或既有命名。Rust 对应常量全部是 `pub`，可见性比 Go 更宽；名称、代码和模板则由 [`errors_test.rs`](errors_test.rs) 逐项锁定。

错误码在 `pkg/errno/errcode.go`/`errcode.rs` 中分别核对为 8050、1141、8049、1045、3118、3955、1862，消息模板在 `pkg/errno/errname.go`/`errname.rs` 中也与本文件一致。Go 生产代码通过 `FastGenByArgs` 或 `GenWithStackByArgs` 从标准错误对象生成带协议码的错误；Rust 生产代码目前主要构造 `PrivilegeError` 变体，尚没有等价生成器或标准码映射。

反过来，Rust 枚举中的 `Authentication`、`InvalidJson`、`Io`、`NoSuchTable` 等泛化类别并不是 [`errors.go`](errors.go) 里的同名标准变量；它们是 Rust 移植为统一 `Result` 接口增加的内部错误边界。`PrivilegeErrorKind` 常量也不会自动覆盖这些变体。

## 扩展指南

- 新增内部失败类别：在 `PrivilegeError` 增加语义明确的变体与 `#[error]` 文本，并在真正产生该失败的 `cache.rs`、`privileges.rs` 或 `tidb_auth_token.rs` 路径构造它；不要仅新增未接线变体。
- 新增或调整 Go 标准权限错误：先核对 `pkg/errno/errcode.go`、`pkg/errno/errname.go` 及 Rust 对应生成文件，再增加/修改 `PrivilegeErrorKind` 常量；同步扩展独立测试 [`errors_test.rs`](errors_test.rs) 的表驱动用例。
- 要实现协议级兼容：新增显式的 `PrivilegeError` → 标准错误码/模板映射，并为每个变体规定参数顺序、是否含 `using password`、堆栈/错误源与未知变体回退策略。不能只比较展示文本。
- 若新增错误可被上游恢复或分类，优先添加结构化字段/变体及匹配测试；若必须保留底层类型或错误链，应评估去掉部分 `Clone/Eq` 约束或引入适合的 source 包装，而不是继续 `to_string()`。
- Rust 单元测试继续放在独立的 [`errors_test.rs`](errors_test.rs)，由 `lib.rs` 的 `#[cfg(test)] mod errors_test` 接入；不要把测试嵌入生产源文件。涉及具体业务分支时，还应同步相邻的 `cache_test.rs`、`privileges_test.rs` 或 `tidb_auth_token_test.rs`。
- 兼容风险主要是客户端可见错误码/模板、大小写和参数顺序；正确性风险是把账户状态、认证失败或解析失败错误分错类；本文件没有热路径运算，单纯增加常量几乎无性能风险，但无界保存底层长错误文本会增加错误路径分配量。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/privilege/privileges` 确认目标及相邻 Go/Rust 文件已索引；`query PrivilegeError --kind enum` 定位到 `errors.rs:22`；`node --file pkg/privilege/privileges/errors.rs --offset 1 --limit 240` 读取完整 134 行源码。自然语言 `explore` 因通用 `Error` 名称产生歧义，限定符号的 `callers/callees` 未给出边，故没有据此虚构图关系。
- 目标与装配：[`errors.rs`](errors.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 直接 Rust 使用点：[`cache.rs`](cache.rs)、[`privileges.rs`](privileges.rs)、[`tidb_auth_token.rs`](tidb_auth_token.rs)；精确搜索覆盖 `PrivilegeError` 各变体和七个 `PrivilegeErrorKind` 常量。
- Go 对照与权威元数据：[`errors.go`](errors.go)、[`privileges.go`](privileges.go)、`pkg/errno/errcode.go`、`pkg/errno/errname.go`，并交叉检查 `pkg/errno/errcode.rs`、`pkg/errno/errname.rs`。
- 独立 Rust 测试：[`errors_test.rs`](errors_test.rs) 验证七个标准描述符的名称、代码和模板；`privileges_test.rs` 对 `InvalidPrivilegeType`、`MustChangePassword` 等真实业务分支有匹配断言。没有运行 Cargo，符合本任务纯文档约束。
- 交付结构检查使用任务文件指定命令，要求目标存在且恰好包含上述 11 个固定二级标题；另以差异审查确认只新增本说明文档，并在成功后删除编号任务文件，保持总计划只读。
