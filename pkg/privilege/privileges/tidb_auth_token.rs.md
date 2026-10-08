# `pkg/privilege/privileges/tidb_auth_token.rs`

## 文件定位

本文件属于 `astersql-privilege-privileges` crate。crate 入口 `pkg/privilege/privileges/lib.rs` 以私有模块 `mod tidb_auth_token` 纳入该文件，再用 `pub use tidb_auth_token::*` 对外导出其公开项。`pkg/privilege/privileges/Cargo.toml` 的 `[package.metadata.porting]` 将整个 crate 对应到 Go 包 `pkg/privilege/privileges`；本文件的直接 Go 对照是同目录的 `tidb_auth_token.go`。

它实现 TiDB Auth Token 的底层 JWT/JWKS 能力：从本地 JSON Web Key Set 文件加载公钥、按 JWT header 选择公钥并验签、在可能发生密钥轮换时重新加载并重试，以及可选的后台定时刷新。JWT 中 `sub`、`email`、`iat`、`exp`、`iss` 等业务 claims 的语义校验不在本文件中，而在 `pkg/privilege/privileges/privileges.rs::checkAuthTokenClaims`。

当前接线状态必须与 Go 区分：RustCodeGraph 与精确源码搜索均未找到这些公开 API 在生产 Rust 文件中的调用者；`JWKSImpl`、`LoadJWKS4AuthToken` 和 `checkSigWithRetry` 当前只被 `tidb_auth_token_test.rs` 直接使用。Rust 服务端 `pkg/server/conn.rs` 能识别字符串插件名 `tidb_auth_token`，但尚无证据表明握手路径已调用本文件完成验签。Go 版本则已由 `privileges.go` 的连接验证逻辑调用全局 JWKS。因此，本文件是已实现、已测试但尚未在 Rust 生产鉴权主链中接线的底层组件。

## 核心职责

- `JWKSImpl` 保存 JWKS 文件路径和当前内存密钥集，并为并发读取提供快照式访问。
- `JWKSImpl::load` 完成“读取整个文件 -> 解析 `JwkSet` -> 成功后原子式替换内存值”的同步刷新；读取或解析失败时不会覆盖已有密钥集。
- `JWKSImpl::verify` 解码 JWT header，根据 `kid` 选钥，将 JWK 转为 `DecodingKey` 并验证签名，成功后把 claims 重新序列化成 JSON 字节。
- `JWKSImpl::LoadJWKS4AuthToken` 设置文件路径、可选地启动周期刷新线程，并执行首次同步加载。
- `JWKSImpl::checkSigWithRetry` 先检查 JWT 是否恰为三段，再执行验签；验签失败时从文件重载 JWKS 后重试，用于覆盖密钥刚轮换的窗口。
- `CancellationToken` 为刷新线程提供轻量协作取消；`GlobalJWKS` 提供进程级惰性全局实例。

本文件只证明签名与载荷可解码，不负责把 claims 与用户记录比对。尤其是 `verify` 主动关闭 `jsonwebtoken` 的 `exp`、`nbf` 和 required-claims 校验；调用链必须在随后调用 `checkAuthTokenClaims`（或等价的上层策略）才能完成账户身份和时效验证。

## 主要符号

- `pub struct CancellationToken(Arc<AtomicBool>)`：可克隆的取消句柄。所有克隆共享同一个原子布尔值。
  - `cancel(&self)` 以 `Ordering::Release` 写入 `true`。
  - `is_cancelled(&self) -> bool` 以 `Ordering::Acquire` 读取取消状态。
- `pub struct JWKSImpl`：JWKS 状态容器。
  - `set: Arc<RwLock<Option<JwkSet>>>` 是内部共享密钥集；`None` 表示尚无一次成功加载。
  - `pub(crate) filepath: String` 是后续同步加载和失败重试所使用的本地路径。
  - `new() -> Self` 返回空路径、空密钥集的默认实例。
  - `load(&self) -> Result<(), PrivilegeError>` 读取并解析当前 `filepath`。
  - `verify(&self, token_bytes: &[u8]) -> Result<Vec<u8>, PrivilegeError>` 只做编码、选钥、签名和载荷解码层面的验证。
  - `LoadJWKS4AuthToken(&mut self, cancellation: Option<CancellationToken>, jwks_path: impl Into<String>, interval: Duration) -> Result<Option<JoinHandle<()>>, PrivilegeError>` 初始化并可选启动刷新线程。
  - `checkSigWithRetry(&self, token_string: &str, retry_time: i32) -> Result<HashMap<String, Value>, PrivilegeError>` 返回未经业务 claims 校验的键值表。
- `GlobalJWKS() -> &'static Mutex<JWKSImpl>`：通过 `OnceLock` 惰性创建全局实例；外层 `Mutex` 串行化需要可变访问的初始化操作。

文件内没有 trait、enum、条件编译项或模块级常量。命名保留了 Go 风格的 `LoadJWKS4AuthToken`、`checkSigWithRetry` 和 `GlobalJWKS`；crate 根的 lint allowance 允许这些名称存在。

## 执行流程

初始化流程如下：

1. 调用者创建 `JWKSImpl::new`，或取得并锁定 `GlobalJWKS`。
2. 调用 `LoadJWKS4AuthToken`，方法先把 `jwks_path` 保存到 `filepath`。
3. 若传入 `Some(CancellationToken)`，方法先克隆 `set` 和路径并启动 OS 线程。线程循环使用 `thread::park_timeout(interval)` 等待；被取消则退出，否则尝试读取、解析并替换密钥集。
4. 不论是否启动线程，当前线程随后调用 `load` 做首次同步加载。成功时返回可选 `JoinHandle`；失败时直接返回错误。由于线程先于首次加载启动，即使首次加载失败，调用者仍可在文件后来出现后由后台刷新恢复，这一点由 `test_jwks_refresh_recovers_after_initial_load_failure` 证明。

验签与重试流程如下：

1. `checkSigWithRetry` 要求 `token_string.split('.').count() == 3`，否则立即返回 `Invalid JWT`，不访问文件。
2. 每轮调用 `verify`。`verify` 先做 UTF-8 转换和 JWT header 解码，再取得 `set` 的读锁。
3. 有 `kid` 时调用 `JwkSet::find(kid)`；无 `kid` 时只有单钥集合才允许隐式选第一把钥，多钥且无 `kid` 会失败。
4. 由所选 JWK 构造 `DecodingKey`，并以 header 声明的算法建立 `Validation`。这里关闭时间与 required-claims 检查，但保留库的签名验证行为。
5. 验签成功后，claims 先序列化为 JSON 字节，再由 `checkSigWithRetry` 解析成 `HashMap<String, Value>` 返回。
6. 验签失败时保存该错误并调用 `load`。重载失败立即传播 I/O/JSON 错误；重载成功则进入下一轮。循环条件是 `retry_time >= 0`，所以非负参数实际最多尝试 `retry_time + 1` 次验签。
7. 尝试耗尽时返回带最后一次验签错误文本的 `Authentication("Retry time has been spent out: ...")`。

## 数据与状态

`JWKSImpl` 有两类可变状态：`filepath` 只在持有 `&mut self` 的 `LoadJWKS4AuthToken` 中设置，之后被同步 `load` 和刷新线程读取；`set` 则位于 `Arc<RwLock<Option<JwkSet>>>` 中，允许主实例和后台线程共享。

密钥更新采用“先在锁外完成文件读取和 JSON 解析，成功后再取得写锁整体替换”的方式。因此，读者不会看见部分解析的集合；刷新失败时旧集合仍可继续服务。`load` 的同步路径同样只在成功解析后写入。`verify` 在选钥和构造解码密钥期间持有读锁，写入刷新会等待其结束。

`CancellationToken` 的取消状态是单向的：默认值为未取消，`cancel` 后没有复位 API。后台线程只在一次 `park_timeout` 返回后检查标志，因此取消延迟最长大致受 `interval` 限制；`cancel` 本身不会主动唤醒 parked 线程。

`GlobalJWKS` 的 `OnceLock<Mutex<JWKSImpl>>` 与 Go 的包级全局变量用途相同，但 Rust 调用者必须处理互斥锁获取和潜在 poison。当前函数签名不会把 poison 转换为 `PrivilegeError`，因为锁的取得发生在调用者一侧。

## 依赖与调用关系

crate 边界与第三方依赖由 `pkg/privilege/privileges/Cargo.toml` 确认：生产依赖直接使用 `jsonwebtoken = "9"`、`serde_json = "1"` 和 crate 内的 `PrivilegeError`；RSA 测试材料所需的 `openssl`、`base64`、`tempfile` 仅是 dev-dependencies。

已验证的文件内调用边为：

- `LoadJWKS4AuthToken -> CancellationToken::is_cancelled`，并在首次同步阶段调用 `JWKSImpl::load`。
- `checkSigWithRetry -> JWKSImpl::verify`；验签失败分支再调用 `JWKSImpl::load`。
- `load -> fs::read -> serde_json::from_slice::<JwkSet> -> RwLock::write`。
- `verify -> decode_header -> JwkSet::find/keys.first -> DecodingKey::from_jwk -> decode::<Value> -> serde_json::to_vec`。

上游方面，`lib.rs` 将所有公开符号再导出；当前 Rust 上游仅见 `tidb_auth_token_test.rs`。邻近的 `privileges.rs::checkAuthTokenClaims` 是验签后应进入的业务校验阶段，但当前生产 Rust 搜索没有发现从本文件到它的接线。Go 的实际主链则是 `privileges.go::ConnectionVerification -> GlobalJWKS.checkSigWithRetry -> checkAuthTokenClaims`，而服务端 Go 握手在 `pkg/server/conn.go` 选择 `mysql.AuthTiDBAuthToken`。

## 错误处理与边界

- 文件读取失败映射为 `PrivilegeError::Io`，JSON/JWKS 解析以及 claims 再序列化或反序列化失败映射为 `PrivilegeError::InvalidJson`。
- 非 UTF-8 token、header 解码失败、未加载 JWKS、找不到匹配 key、JWK 无法转为解码密钥、签名失败均映射为 `PrivilegeError::Authentication`。
- `load` 和后台刷新只有在“读取且解析都成功”时替换密钥，因此坏文件不会清空旧密钥；后台线程会静默忽略读取/解析失败，不产生日志或可观察错误。
- `LoadJWKS4AuthToken(None, ...)` 只同步加载一次并返回 `Ok(None)`；`Some(token)` 返回线程句柄。若首次加载失败，函数返回 `Err`，已创建的句柄随局部变量丢弃但线程仍运行，直到共享取消令牌被置位。
- `checkSigWithRetry` 对负数 `retry_time` 不做验签，直接返回“次数耗尽”；对 `0` 则仍尝试一次。
- 当前 `RwLock::read/write` 与全局 `Mutex` 相关实现使用或暴露 `unwrap` 语义：若写锁持有者 panic 导致 poison，后续加载或验签可能 panic，而不是返回 `PrivilegeError`。
- `interval == Duration::ZERO` 会造成刷新线程紧密循环；代码没有最小间隔校验。线程创建失败会由标准库 `thread::spawn` 的 panic 语义体现，而非结果类型。
- header 中算法直接用于 `Validation::new(header.alg)`，实际密钥兼容性和签名由 `jsonwebtoken` 检查；本文件没有额外的算法 allowlist。扩展接线时应把允许算法视为安全策略的一部分。
- 关闭 `exp`/`nbf` 检查意味着单独调用 `verify` 或 `checkSigWithRetry` 不足以完成认证；必须继续进行上层 claims 校验。

## 并发与资源生命周期

每次传入 `Some(CancellationToken)` 调用 `LoadJWKS4AuthToken` 都会新建一个独立 OS 线程，没有防重复启动机制。线程持有 `Arc<RwLock<Option<JwkSet>>>` 和路径副本，因此即使原 `JWKSImpl` 被释放，线程仍可能存活并继续访问文件。调用方负责保存返回的 `JoinHandle`、在关闭时调用同源 token 的 `cancel`，并视需要 `join`；本文件没有 `Drop` 自动取消或回收线程。

刷新线程的等待使用 `park_timeout`，而不是 channel、异步 runtime 或条件变量。取消只改变原子标志，不唤醒线程，所以资源回收不是即时的。`test_jwks_refresh_recovers_after_initial_load_failure` 在成功或超时路径都调用 `cancel`，但没有 join；测试依赖很短的刷新间隔使线程很快自行退出。

正常并发验签通过 `RwLock` 共享同一 `JwkSet`，多个读者可以并发；加载成功时短暂获取写锁替换整个集合。`GlobalJWKS` 外层的 `Mutex` 如果在每次验签期间都被调用方持有，会额外把这些读操作串行化；未来生产接线应缩小全局锁持有范围，或重新评估是否需要外层可变访问模型。

## 与 Go 版本的对应关系

主要语义保持一致：两端都有 `JWKSImpl`、文件路径、进程级全局 JWKS、初始同步加载、可选后台刷新、三段 JWT 检查、验签失败后 reload-and-retry，以及“加载失败不替换旧集合”的行为。Rust 测试 `test_auth_token_claims` 和 `test_jwksimpl` 分别移植 Go 的 `TestAuthTokenClaims`、`TestJWKSImpl`。

关键实现差异如下：

- Go 用 `unsafe.Pointer` 配合 atomic load/store 发布 `jwk.Set`；Rust 用 `Arc<RwLock<Option<JwkSet>>>`。
- Go 用 `context.Context`、`sync.WaitGroup` 和 ticker 管理 goroutine；Rust 用自定义 `CancellationToken` 与 `JoinHandle` 管理 OS 线程。Rust 的 handle 由调用者显式接管，取消不会主动唤醒等待。
- Go 后台刷新在失败时写日志；Rust 后台线程静默保留旧集合。
- Go `jws.Verify(WithKeySet)` 负责从 key set 选钥；Rust 明确实现 `kid` 选择，并允许“无 kid 且恰好单钥”的回退。
- Go 的 jwx 把标准时间 claims 解码成 `time.Time`；Rust 返回普通 `serde_json::Value` 数字。因此 Go 测试中的类型错误文案在 Rust 没有等价表面，Rust 的上层校验会把非数字 `iat/exp` 归入缺失类错误。
- Rust 的 `verify` 显式关闭 `jsonwebtoken` 的内置 `exp`、`nbf` 和 required-claims 校验，让 `privileges.rs::checkAuthTokenClaims` 承担时效与身份策略；Go 同样在之后执行该业务校验，但底层库的 payload 表示不同。
- Rust 新增了“首次同步加载失败后，已启动线程仍可恢复”的专门回归测试；该行为源自两端都先启动刷新任务、再首次加载的顺序。
- 最重要的迁移状态差异是接线：Go 已在 `ConnectionVerification` 使用 `GlobalJWKS`；Rust 当前尚无生产调用者。

## 扩展指南

若要把 Rust Auth Token 真正接入登录链，优先在权限连接验证层新增最小接线：取得 `GlobalJWKS`、调用 `checkSigWithRetry`、随后调用 `checkAuthTokenClaims`，并把 `PrivilegeError` 按现有连接鉴权错误约定向上转换。不能只以验签成功作为登录成功。接线测试应放在独立测试文件中，优先扩展 `pkg/privilege/privileges/tidb_auth_token_test.rs`；涉及握手插件选择时再同步扩展 `pkg/server/conn_test.rs`，不要把测试嵌入生产源文件。

修改密钥选择或算法策略时，应集中在 `verify`，并补充：缺失 `kid` 的单钥/多钥场景、未知 `kid`、算法与 JWK 不兼容、禁止算法等用例。安全风险是算法降级、错误选钥或把仅解码的 claims 当作已认证身份。

修改刷新策略时，应集中在 `LoadJWKS4AuthToken` 和 `CancellationToken`，并覆盖重复启动、零间隔、取消延迟、显式 join、读取失败后保留旧 key、初始失败后恢复。当前 API 在首次加载失败时无法把已启动的 handle 返回给调用者；若调整该契约，需要同时设计可靠的线程回收语义并核对 Go 兼容要求。

修改重试语义时，应注意 `retry_time` 当前表示“失败后的额外重试次数”，所以总验签次数为 `retry_time + 1`。应保持“验签错误触发 reload、reload 错误立即返回”的顺序，除非 Go 行为也同步变化，并补齐精确次数和最后错误保留测试。

若要提高可观测性，可为后台刷新失败增加日志或指标，但不能在失败时清空旧 key；同时避免记录 JWT、claims 或密钥内容。若要改善并发，应先量化全局 `Mutex` 与内部 `RwLock` 的锁范围，不要绕过整体替换不变量。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 Rust、Go、测试与邻近权限文件均可由索引读取。
- 目标源码：`pkg/privilege/privileges/tidb_auth_token.rs`，核对了 `CancellationToken`、`JWKSImpl`、`load`、`verify`、`LoadJWKS4AuthToken`、`checkSigWithRetry`、`GlobalJWKS` 的完整实现。
- 图查询：`query tidb_auth_token` 定位 Rust/Go 文件和全部主要符号；`callees LoadJWKS4AuthToken` 确认 Rust 边到 `is_cancelled`、`load`；`callees checkSigWithRetry` 确认 Rust 边到 `verify`、`load` 及错误枚举。`callers` 没有返回生产 Rust 调用者，随后以精确 `rg` 搜索复核，调用仅位于独立 Rust 测试。
- crate 证据：`pkg/privilege/privileges/Cargo.toml` 与 `pkg/privilege/privileges/lib.rs`，确认 crate 名、依赖、Go 包映射、模块声明、公开再导出和独立测试模块。
- Go 对照：`pkg/privilege/privileges/tidb_auth_token.go`、`pkg/privilege/privileges/privileges.go`、`pkg/server/conn.go`，确认 Go 的 JWKS 实现和已接线认证主链。
- Rust 邻接证据：`pkg/privilege/privileges/privileges.rs::checkAuthTokenClaims`、`pkg/privilege/privileges/errors.rs::PrivilegeError`、`pkg/server/conn.rs::handshake`，确认 claims 校验、错误映射与当前服务端插件识别边界。
- 测试证据：`pkg/privilege/privileges/tidb_auth_token_test.rs` 覆盖有效 claims、缺失/错误 claims、错误路径、非法 JWT 段数、篡改签名、reload 失败、未知 `kid`、密钥集轮换和首次加载失败后的后台恢复；`tidb_auth_token_test.go` 用于核对原始测试意图和库差异。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务给定命令验证本文档存在且恰好包含 11 个固定二级标题，并人工复核没有把未接线能力描述成生产现状。
