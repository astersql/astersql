# `pkg/sessionctx/sessionstates/session_token.rs`

## 文件定位

本说明对应真实源文件 [`session_token.rs`](./session_token.rs)。该文件属于 `astersql-sessionctx-sessionstates` crate，由同目录的 `lib.rs` 以 `pub mod session_token` 声明并通过 `pub use session_token::*` 再导出。它实现会话迁移令牌的 JSON 形状、签发、验签以及签名证书轮换；同一 crate 中的 `session_states.rs` 提供其对外错误类型 `SessionStateError` 和标准错误码 `ErrCannotMigrateSession`。

令牌解决的是代理迁移会话时无法保存用户密码的问题：源节点签发带用户名和有效期的令牌，目标节点用共享证书验签后允许对应用户恢复会话。Rust 实现目前在 crate 内完整存在并由独立 Rust 测试覆盖；仓库搜索没有发现测试之外的 Rust 生产调用者。完整应用中的实际接线仍见 Go：`pkg/executor/show.go::fetchShowSessionStates` 签发令牌，`pkg/privilege/privileges/privileges.go` 在 `AuthTiDBSessionToken` 认证分支验签，`pkg/domain/domain.go::LoadSigningCertLoop` 配置并周期重载证书。

## 核心职责

- 用 `SessionToken` 固定与 Go 兼容的字段名：`username`、`sign-time`、`expire-time`、可省略的 `signature`；`go_signature_bytes` 将签名字节编码为 Go `encoding/json` 对 `[]byte` 使用的标准 base64 字符串。
- `CreateSessionToken` 先序列化不含签名的令牌，再使用缓存中最新证书的私钥签名；`ValidateSessionToken` 反序列化后取出签名、重建相同的无签名 JSON，并依次校验签名、过期时间、最大生命周期和用户名。
- `SigningCert` 从 PEM 文件加载匹配的 X.509 证书/私钥，保留最新证书以及仍在宽限期内的旧证书，使不同节点证书轮换略有时间差时旧令牌仍可验证。
- `sign_with_key`、`verify_with_key` 支持 Ed25519、ECDSA、RSA PKCS#1 和 RSA-PSS；RSA 摘要根据证书签名算法选择 SHA-256、SHA-384 或 SHA-512。
- 根据 `config::deploymode::IsStarter()` 切换普通模式和 Starter 模式的令牌寿命、证书重载周期与旧证书宽限期。

## 主要符号

- 六个公开时长常量定义安全窗口：普通模式 `tokenLifetime = 1 分钟`、`LoadCertInterval = 10 分钟`、`oldCertValidTime = 15 分钟`；Starter 模式对应 `8 小时`、`24 小时`、`36 小时`。`currentTokenLifetime` 和 `currentOldCertValidTime` 是内部模式选择器，`GetLoadCertInterval` 是公开选择器。
- `SessionToken { Username, SignTime, ExpireTime, Signature }` 是公开可序列化令牌。`Signature` 为空时不写入 JSON，这是签名输入能在签发端和验证端一致重建的关键不变量。
- `CertInfo` 保存一张 `X509`、对应 `PKey<Private>` 和进程内缓存过期时刻；该过期时刻不是证书自身的 X.509 `notAfter`，而是轮换缓存的清理期限。
- `SigningCert { cert_path, key_path, certs }` 是内部证书仓库。`set_cert_path`、`set_key_path`、`check_and_load_cert`、`load_cert` 管理路径和缓存，`sign` 始终用 `certs[0]`，`check_signature` 从新到旧尝试仍有效的证书。
- `GLOBAL_SIGNING_CERT: LazyLock<RwLock<SigningCert>>` 是进程级共享状态；公开入口 `SetCertPath`、`SetKeyPath`、`ReloadSigningCert` 修改它，签发和验签读取它。
- `digest_for_certificate`、`configure_rsa_signer`、`configure_rsa_verifier`、`sign_with_key`、`verify_with_key` 封装 OpenSSL 算法分派。RSA-PSS 额外设置 PSS padding 和 `DIGEST_LENGTH` salt。
- `equal_fold` 实现与 Go `strings.EqualFold` 对齐的 Unicode 简单大小写折叠，用于用户名比较，而不是普通 ASCII 忽略大小写。
- `SetMockNowOffset`、`ResetSigningCertForTest` 和 `MOCK_NOW_OFFSET_MILLIS` 是隐藏的测试支撑；`get_now` 把原子毫秒偏移应用到 `Utc::now()`。

## 执行流程

签发流程从 `CreateSessionToken` 开始：调用 `get_now`；按当前部署模式计算过期时间；构造空 `Signature` 的 `SessionToken`；由 `serde_json::to_vec` 产生无 `signature` 字段的规范签名输入；获取 `GLOBAL_SIGNING_CERT` 读锁；`SigningCert::sign` 选择首张证书；`sign_with_key` 按私钥类型生成签名；最后把签名字节写回令牌并返回。没有已加载证书时，签名阶段返回 `no certificate or key file to sign the data`，再包装成不可迁移错误。

验签流程从 `ValidateSessionToken` 开始：解析输入 JSON；用 `mem::take` 移出 `Signature`；重新序列化无签名令牌；在读锁下调用 `check_signature`。后者按缓存顺序遍历，遇到第一张已超过本地 `expire_time` 的证书即停止，因为 `load_cert` 维持新到旧、过期时刻递减的顺序；任一证书验签成功就返回。密码学验证通过后，再拒绝 `now > ExpireTime`、`SignTime + currentTokenLifetime() < now` 或用户名不能经 `equal_fold` 匹配的令牌。这里不要求 `SignTime + lifetime == ExpireTime`，也不拒绝未来的 `SignTime`，与 Go 为版本差异和节点时钟偏差保留容忍度的规则一致。

证书配置和轮换从 `SetCertPath`、`SetKeyPath` 或 `ReloadSigningCert` 进入。路径只有在值变化时触发加载，且证书与私钥路径都非空才读取文件。`load_cert` 解析 PEM、验证公私钥匹配，按 `now + GetLoadCertInterval() + currentOldCertValidTime()` 计算缓存期限，将新证书插到首位，再仅保留尚未过期的旧证书。路径加载失败或密钥不匹配时不会替换已有 `certs`，所以已加载的旧配对仍可继续签发和验签。

## 数据与状态

令牌的签名覆盖 `Username`、`SignTime` 和 `ExpireTime` 的 JSON 表示，不覆盖 `Signature` 本身；修改任一受签字段后，即使字段在业务校验上看似合理，也应先在密码学验签阶段失败。`Signature` JSON 缺失时因 `default` 反序列化为空向量，随后仍会进入验签，不会被视为未签名令牌而放行。

证书缓存 `certs` 的第一个元素始终是最近成功加载的配对。每次成功加载都会新增一个缓存项，即使文件内容未变化；旧项按原顺序附加，过期项从首次过期处整体丢弃。这一顺序同时支撑“最新证书签名”和“多张旧证书依次验签”两个行为。

全局时钟偏移使用 `AtomicI64` 和 `SeqCst`，单位为毫秒；它影响令牌签发、令牌校验以及证书缓存期限。生产调用不应使用隐藏的测试入口。部署模式来自 `astersql-config-deploymode`，因此同一进程切换模式会立即改变后续签发和校验采用的时间窗口。

## 依赖与调用关系

crate 边界由 `pkg/sessionctx/sessionstates/Cargo.toml` 定义：本文件直接使用 `base64`、带 `serde` 支持的 `chrono`、`openssl`、`serde`、`serde_json`，并通过别名 `config` 依赖 `astersql-config-deploymode`。`SessionStateError` 来自同 crate 的 `session_states` 模块；测试额外使用 `rcgen`、`tempfile`，目标测试文件还直接使用 OpenSSL 生成测试证书。

RustCodeGraph 给出的关键内部边包括：`CreateSessionToken -> get_now/currentTokenLifetime/SigningCert::sign -> sign_with_key`；`ValidateSessionToken -> SigningCert::check_signature -> verify_with_key`，并调用 `get_now/currentTokenLifetime/equal_fold`；`SetCertPath/SetKeyPath -> set_*_path -> check_and_load_cert -> load_cert`；`ReloadSigningCert -> check_and_load_cert`。`load_cert` 再调用 `GetLoadCertInterval`、`currentOldCertValidTime` 和 `get_now`。

RustCodeGraph 的调用者结果和 `rg` 交叉核对表明，Rust 入口当前仅被 `session_token_test.rs` 及 `session_states_1_aster_unit_test.rs` 调用，尚无非测试 Rust 主链接线。Go 的对应生产边为：`Domain::LoadSigningCertLoop -> SetCertPath/SetKeyPath/GetLoadCertInterval/ReloadSigningCert`，`ShowExec::fetchShowSessionStates -> CreateSessionToken`，权限验证的会话令牌分支 `-> ValidateSessionToken`。因此这些 Go 边说明预期应用位置，但不能作为 Rust 生产接线已经完成的证据。

## 错误处理与边界

JSON 编解码错误通过 `?` 转为 `SessionStateError::Json`，其 `code()` 为 `0`。缺证书、文件读取、PEM 解析、公私钥不匹配、不支持的算法、OpenSSL 签名/验签失败，以及令牌过期、生命周期超限、用户名不匹配，都会经 `SessionStateError::cannot_migrate` 转为标准 `ErrCannotMigrateSession`；独立测试验证缺证书错误的 errno 为 `ErrCannotMigrateSession`。

`SetCertPath`、`SetKeyPath` 和 `ReloadSigningCert` 的公开签名不返回错误：内部加载结果被显式丢弃。与 Go 版本会记录加载成功或失败日志相比，Rust 当前没有日志依赖，也不会把热加载失败通知调用者；其可观察保障只是保留此前成功加载的缓存。扩展运维可观测性时应避免改变“配置其中一个路径时允许另一路径尚为空”和“失败后旧证继续可用”的兼容行为。

锁中毒由 `expect("session signing certificate lock poisoned")` 触发 panic，而不是转为 `SessionStateError`。不支持的 RSA 摘要和密钥类型会返回明确字符串；RSA-PSS 摘要通过证书文本中的算法参数识别，只接受 SHA-256/384/512。实现没有单独检查 X.509 有效期，运行时信任的是 PEM 解析、公私钥匹配和本地轮换缓存期限；若要增加证书有效期策略，必须同时评估 Go 兼容性和轮换测试。

## 并发与资源生命周期

`GLOBAL_SIGNING_CERT` 使用 `RwLock`：签发和验签持读锁，路径更新和重载持写锁。加密操作和文件读取均发生在锁保护范围内，因此状态一致但慢磁盘或昂贵密码学操作会延长锁占用；当前独立测试 `certificate_reload_and_reads_are_concurrent_safe` 用一个证书写线程、两个重载线程和三个签发/验签线程覆盖这一设计。

证书 PEM 由 `fs::read` 读入临时字节缓冲，解析后的 `X509`/`PKey` 由 OpenSSL Rust 包装类型按所有权释放。证书缓存没有后台任务；`ReloadSigningCert` 只是同步执行一次，生产环境需要像 Go `Domain::LoadSigningCertLoop` 那样由外层生命周期按 `GetLoadCertInterval` 调度。旧证书只会在成功重载时被清理，若外层长期不调用重载，缓存项也不会按墙钟主动删除；不过 `check_signature` 会跳过已经超过本地期限的条目。

测试通过 `SESSION_TOKEN_TEST_LOCK` 串行化所有会修改全局签名状态的用例，并在结束时调用 `ResetSigningCertForTest`；新增相关测试应沿用该独立测试文件和锁，不能把测试嵌回生产源文件。

## 与 Go 版本的对应关系

Rust 文件逐项镜像 `pkg/sessionctx/sessionstates/session_token.go`：常量和 Starter 分支相同；`SessionToken` JSON 字段与 Go struct tag 相同；创建时签名无签名字段的 JSON、验证时清空签名再序列化的顺序相同；证书缓存的最新项优先、旧证宽限期和“遇到首个过期项即停止”相同；用户名使用 Unicode 简单大小写折叠，测试覆盖希腊 sigma 的普通/终结形式。

密码学覆盖也与 Go 测试意图对齐：RSA 2048/4096 位的 SHA-256/384/512 PKCS#1 与 PSS、ECDSA 三种摘要组合、Ed25519；RSA-PSS 两端使用等于摘要长度的 salt。Rust 的 ECDSA 路径显式使用 DER 编解码，Ed25519 使用无摘要的一次性接口。

需要注意的实现差异是：Go `checkAndLoadCert` 会记录成功/失败日志，Rust 丢弃加载错误；Go 用嵌入式 `sync.RWMutex`，Rust 用全局 `RwLock<SigningCert>`；Go failpoint 注入 `time.Duration`，Rust 用全局原子毫秒偏移；Go 返回 `*SessionToken`，Rust 返回拥有所有权的 `SessionToken`。此外，Go 已接入 Domain、Executor 和 Privilege 生产链，Rust 当前只有 crate 导出和测试调用证据，不能宣称生产迁移链已经切换到 Rust。

## 扩展指南

- 新增或修改令牌字段时，应同时修改 `SessionToken`、签发/验签的无签名序列化协议、Go 对应 struct 和 `session_token_json_matches_go_field_names_and_base64_signature`；任何字段名、时间编码或省略规则变化都可能使跨语言验签失败。
- 新增签名算法或摘要时，成对修改 `sign_with_key` 与 `verify_with_key`，必要时扩展 `digest_for_certificate` 和 RSA 配置函数，并在 `signing_algorithm_matrix_matches_go` 中加入签发到验签的往返矩阵；Go 版本若仍需互操作，也必须同步其算法分派。
- 调整令牌或证书时间窗口时，应同时审查普通/Starter 常量、`current*` 选择器、`starter_token_lifetime_matches_go`、`certificate_grace_period_matches_go`，以及 Go 的同名常量与 `TestStarterSessionTokenLifetime`、`TestCertExpire`。
- 改动证书热加载时必须保留公私钥匹配检查、加载失败不破坏旧缓存、缓存新到旧排序和旧证宽限期；同步扩展 `set_cert_and_key_matches_go`、宽限期与并发测试。
- 若要把这些 API 接入 Rust 生产主链，应在对应 Rust Domain/Executor/Privilege 边界做最小接线并新增各自独立测试，而不是把集成测试写入本文件。还应明确外层重载任务的启动、停止和失败可观测性。
- 修改共享全局状态或锁粒度时，需评估磁盘 I/O、OpenSSL 操作持锁时间、锁中毒策略和测试隔离；性能优化不能让签发观察到未配对的证书与私钥。

## 验证依据

- 源码：`pkg/sessionctx/sessionstates/session_token.rs`，核对全部常量、类型、公开入口、内部算法分派、锁和测试时钟；`pkg/sessionctx/sessionstates/session_states.rs`，核对 `SessionStateError`、`ErrCannotMigrateSession` 与错误码。
- crate/模块：`pkg/sessionctx/sessionstates/Cargo.toml` 和 `pkg/sessionctx/sessionstates/lib.rs`，核对包名、依赖、`nextgen` feature、公开再导出和独立测试模块。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点、1,848,419 条边；`files/node/query/callers/callees` 查询确认目标文件、`SessionToken`、`CreateSessionToken`、`ValidateSessionToken` 及内部调用链。图查询显示调用者集中在 Rust 测试；随后用仓库搜索确认没有非测试 Rust 调用者。
- Rust 测试：`pkg/sessionctx/sessionstates/session_token_test.rs`，覆盖 JSON/base64、缺证书错误码、证书路径与不匹配密钥、算法矩阵、过期/伪造/用户名、Unicode 折叠、Starter 时长、旧证宽限期和并发读写；`pkg/sessionctx/sessionstates/session_states_1_aster_unit_test.rs` 提供额外的 Go 行为对照调用证据。
- Go 对照：`pkg/sessionctx/sessionstates/session_token.go`、`session_token_test.go`；生产入口证据来自 `pkg/domain/domain.go::LoadSigningCertLoop`、`pkg/executor/show.go::fetchShowSessionStates` 和 `pkg/privilege/privileges/privileges.go` 的会话令牌认证分支。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复查：所有当前能力均能回指上述符号或测试，Rust 生产接线缺口已明确标注，未把 Go 接线当成 Rust 已支持事实。
