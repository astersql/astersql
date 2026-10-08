# `pkg/util/misc.rs`

## 文件定位

`pkg/util/misc.rs` 是 `astersql-util` crate 的杂项基础设施模块，由 [`pkg/util/lib.rs`](lib.rs) 通过 `pub mod misc` 公开。它不是一条单独的业务主链，而是把多个需要跨子系统复用、又不适合形成独立 crate 的边界能力集中起来：有限重试与 panic 隔离、SQL 语法错误归一化、证书名称与 SAN 解析、列元数据序列化、序列对象解耦接口、服务端 TLS、集群内部 HTTP、网卡地址选择、自签证书以及 INSERT/IMPORT 类型转换标志。

crate 边界由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-util`，入口是 `lib.rs`。本文件直接使用 `anyhow`、`thiserror`、`openssl`、`reqwest`、`get_if_addrs`、`hostname`、`tokio-util`，并通过工作区内部依赖 `task_config`、`task_mysql`、`task_types_group` 读取全局配置、SQL mode 和标志位。该文件没有条件编译的 API；唯一的条件编译块位于 `CreateCertificates` 中，仅在 Unix 上把私钥文件权限设置为 `0600`（`misc.rs:753-757`）。

## 核心职责

- `RunWithRetry`、`WithRecovery`、`Recover`、`HasCancelled` 提供同步重试、panic 捕获和取消状态检查（`misc.rs:49-131`）。重试采用第 `attempt` 次休眠 `backoff * attempt` 毫秒的线性退避；panic 帮助函数会吞掉已捕获 panic，记录日志，并可执行清理回调。
- `SyntaxError`、`SyntaxWarn` 与 `SqlSyntaxError` 把普通错误规范为带 TiDB 语法错误前缀的错误，并用 `warning` 区分警告和硬错误（`misc.rs:133-164`）。
- PKIX/X.509 一组符号负责 OID/短名互转、旧式 `/CN=...` 名称格式化、SAN 解析和授权属性预校验（`misc.rs:166-304`）。真实生产调用包括 `pkg/executor/grant.rs` 的 X.509/SAN 检查，以及 `pkg/privilege/privileges/cache.rs` 的 SAN 解析。
- `ColumnMetadata`、`ProtoColumnInfo`、`ColumnToProto`、`ColumnsToProto` 定义列元数据到下推协议友好结构的抽象转换边界（`misc.rs:306-377`），避免该通用 crate 直接依赖完整 planner/model 类型。
- `GetSequenceByName` 与 `SequenceTable` 是为打破 expression、infoschema、table 之间依赖环而保留的注入边界（`misc.rs:384-400`）。当前文件只声明契约，不负责注册具体实现。
- `LoadTLSCertificates`、`TlsConfig`、`CreateCertificates` 负责服务端证书加载、自动生成和热加载；`InternalHTTPClient`、`InternalHTTPSchema`、`ComposeURL` 负责集群内部 HTTP(S) 客户端的进程级惰性初始化（`misc.rs:403-654`）。`pkg/server/server.rs` 直接调用证书加载/生成，`cmd/tidb-server/main.rs` 在启动路径触发内部客户端初始化。
- `GetLocalIP` 选择首个全局单播地址，供 `cmd/tidb-server/main.rs` 在未显式设置时生成 `AdvertiseAddress`；`GetTypeFlagsForInsert` 与 `GetTypeFlagsForImportInto` 集中表达写入路径的 SQL mode 转换策略（`misc.rs:656-798`）。

## 主要符号

- 常量：`DefaultMaxRetries = 30`、`RetryInterval = 500` 是调用者可复用的默认重试参数；`SyntaxErrorPrefix` 是兼容 MySQL/TiDB 的错误前缀；`Country`、`Organization`、`OrganizationalUnit`、`Locality`、`Email`、`CommonName`、`Province` 与 `URI`、`DNS`、`IP` 是证书字段名常量。
- 错误类型：`SqlSyntaxError { message, warning }` 实现 `std::error::Error`；已经是该类型的错误由 `syntax_issue` 原样返回，避免重复添加前缀。
- 证书名称类型：`PkixAttributeTypeAndValue` 保存数字 OID 和字符串值，`PkixName` 保存有序属性列表；`pkixAttributeTypeNames` 是 OID 到短名的静态表，`pkixTypeNameAttributes` 是由 `OnceLock` 惰性创建的反向表。
- 列转换类型：`ColumnMetadata` 是输入 trait，`ProtoColumnInfo` 是本地输出 DTO。`ColumnsToProto<C>` 批量转换并设置 `PkHandle`；`ColumnToProto<C>` 复制列属性并处理 TiFlash 虚拟生成列、多值索引元素类型和 binary collation。
- 序列边界：`GetSequenceByNameFn` 接收不透明 infoschema、schema/sequence 字符串并返回 `Box<dyn SequenceTable>`；`GetSequenceByName: OnceLock<_>` 允许进程内只注册一次；`SequenceTable` 暴露 ID、NEXTVAL 和 SETVAL。
- TLS 类型：`ClientAuthPolicy` 表示四档客户端证书策略，`TlsVersion` 限定 TLS 1.2/1.3，`LoadedCertificate` 持有 OpenSSL 证书和私钥，`TlsConfig` 保存 CA、策略、版本、套件以及受 `Arc<RwLock<_>>` 保护的可替换证书。
- 证书生成枚举：`PublicKeyAlgorithm` 支持 RSA、P-256 ECDSA、Ed25519；`SignatureAlgorithm` 支持默认/SHA-256、SHA-384、SHA-512。`createTLSCertificates` 是内部 RSA 便捷封装，目前自动 TLS 路径直接调用公开 `CreateCertificates`。
- 进程级入口：`SetRequireSecureTransport`、`RequireSecureTransportEnabled`、`SetMinimumTLSVersion` 操作原子配置；`InternalHTTPClient` 和 `InternalHTTPSchema` 返回由同一个 `Once` 初始化的静态值。

## 执行流程

1. `RunWithRetry` 最多调用闭包 `retryCnt` 次。任一次无错误即成功返回；错误不可重试则立即返回；可重试错误被保存并在线性休眠后继续，耗尽时返回最后一个错误。`retryCnt <= 0` 时闭包不会执行，且因没有最后错误而返回成功（`misc.rs:55-69`）。
2. `WithRecovery` 用 `catch_unwind(AssertUnwindSafe(...))` 包裹闭包：正常完成时也以 `None` 调用可选回调，panic 时以载荷引用调用回调、记录文本后不再传播。`Recover` 是后半段形式，调用者须传入已捕获的 `Option<Box<dyn Any + Send>>`；有载荷时记录上下文、运行无参回调，`quit=true` 则等待 15 秒后退出进程（`misc.rs:72-126`）。
3. `SyntaxError`/`SyntaxWarn` 都进入 `syntax_issue`。输入 `None` 保持 `None`；已有 `SqlSyntaxError` 保持对象身份和原始字段；其余错误被格式化为 `SyntaxErrorPrefix: 原错误`，并设置对应的 warning 位（`misc.rs:145-164`）。
4. `ParseAndCheckSAN` 先按逗号拆条目，再以第一个冒号拆键值；键会 trim 并转大写，只接受 URI/DNS/IP，相同键的值按输入顺序追加。`CheckSupportX509NameOneline` 按 `/` 和 `=` 检查格式及字段短名；`X509NameOnline` 只输出映射表认识的 OID，未知 OID 被忽略（`misc.rs:235-304`）。
5. `ColumnsToProto` 对每列调用 `ColumnToProto`，之后在“表以主键为 handle 且列为主键”或列 ID 为 `-1` 时设置 `PkHandle`。单列转换在 TiFlash 虚拟生成列上置 `1 << 23`；索引路径将类型替换为数组元素类型，并在原列是数组时强制 collation 为 `63`（`misc.rs:334-377`）。
6. `LoadTLSCertificates` 在证书或私钥路径为空时检查 `autoTLS`：禁用则返回 `(None, false)`，启用则在全局临时目录创建 `cert.pem`/`key.pem`。随后加载并核对密钥对，根据原子开关和 CA 解析结果选择客户端认证策略，根据最低版本原子值选择 TLS 1.2/1.3，最后构造可热加载的 `TlsConfig`（`misc.rs:488-568`）。
7. `TlsConfig::reload_certificate` 尝试从原路径加载新密钥对；成功时在写锁内替换，失败时警告并返回旧证书。调用者因而不会因单次轮换文件错误失去当前可用证书（`misc.rs:437-458`）。
8. 首次调用 `InternalHTTPClient` 或 `InternalHTTPSchema` 时，`initInternalClient` 读取全局 cluster SSL 配置，构造五分钟超时的阻塞式 `reqwest` client；存在任一 TLS 配置即选择 `https`，并按可用路径附加 CA 和客户端身份。随后两个值在进程期固定（`misc.rs:587-645`）。
9. `CreateCertificates` 生成所选私钥、建立 CN 固定且含本机 hostname SAN 的 90 天自签证书，用所选摘要签名并写 PEM；私钥使用 PKCS#8，Unix 上权限为 `0600`（`misc.rs:698-765`）。
10. `GetTypeFlagsForInsert` 根据严格模式、`ignoreErr`、ALLOW_INVALID_DATES、NO_ZERO_IN_DATE 和 NO_ZERO_DATE 组合四个 Flags；IMPORT INTO 复用 INSERT 且固定 `ignoreErr=false`（`misc.rs:779-798`）。

## 数据与状态

本文件的普通转换函数大多无持久状态：重试闭包状态由调用者持有；PKIX、SAN 和列转换都创建并返回拥有所有权的新集合。值得关注的进程级状态有四组：

- `pkixTypeNameAttributes: OnceLock<HashMap<...>>` 在第一次反向查询或显式 `init()` 时建立，之后只读。
- `GetSequenceByName: OnceLock<GetSequenceByNameFn>` 是一次性函数指针注册位；重复设置会由调用注册者面对 `OnceLock::set` 的失败结果。
- `RequireSecureTransport: AtomicBool` 和 `MinimumTlsVersion: AtomicU8` 使用 Release 写、Acquire 读，影响后续 `LoadTLSCertificates`，但不会自动改写已经构造的 `TlsConfig`。
- `internalClientInit`、`internalHTTPClient`、`internalHTTPSchema` 构成一次性快照。全局安全配置在首次访问后发生变化，不会重建客户端或 schema。

`TlsConfig` 自身可克隆证书内容，但类型没有实现 `Clone`；其 `certificate` 字段通过 `Arc<RwLock<LoadedCertificate>>` 允许共享读取与原子式替换，路径、CA、认证策略、版本和套件则在构造后不变。

## 依赖与调用关系

上游直接证据如下：

- `pkg/executor/grant.rs` 调用 `CheckSupportX509NameOneline` 和 `ParseAndCheckSAN` 校验授权语句中的 X.509/SAN 条件；`pkg/privilege/privileges/cache.rs` 再次解析已存 SAN 权限值。
- `pkg/server/server.rs` 调用 `CreateCertificates` 和 `LoadTLSCertificates` 组装服务端 TLS；`pkg/server/tests/tls/tls_test.rs` 覆盖多种认证策略、热加载和错误路径。
- `cmd/tidb-server/main.rs` 调用 `GetLocalIP` 补全广播地址，并调用 `InternalHTTPClient` 触发集群客户端初始化。
- `tests/globalkilltest/util.rs` 同时使用 `ComposeURL` 和 `InternalHTTPClient` 访问组件健康/状态端点；这说明 URL schema 与 client TLS 配置必须来自同一安全配置快照。
- `pkg/planner/core/plan_to_pb_test.rs` 直接导入 `ColumnMetadata`、`ColumnToProto`、`ColumnsToProto`，验证真实 planner 需要的 flag、collation、enum 元素、数组和 TiFlash 分支。

下游依赖按职责分组：重试依赖 `std::thread::sleep`；panic 依赖 `std::panic` 与 `log`；取消检查依赖 `tokio_util::sync::CancellationToken`；TLS/证书依赖 OpenSSL、文件系统、hostname 和 `task_config`；内部 HTTP 依赖阻塞式 `reqwest`；地址选择依赖 `get_if_addrs`；类型 flags 依赖 `task_mysql::SQLMode` 和 `task_types_group::Flags`。RustCodeGraph 对整文件报告 58 个使用文件，但精确 `callers/callees` 对 Rust/Go 同名符号未产出可消歧的边，因此上述调用边由精确 `rg` 结果和对应文件核验，而不是据图推断。

## 错误处理与边界

- `RunWithRetry` 不捕获 panic，且 `backoff * attempt as u64` 没有显式溢出保护；调用者应传入合理的非负次数和间隔。它在零/负次数时返回成功，这与 Go 循环行为一致，但容易掩盖错误配置。
- `WithRecovery` 通过 `AssertUnwindSafe` 接受并不保证 unwind-safe 的闭包，且刻意吞掉 panic；只应包裹有明确清理/降级语义的任务。`Recover(quit=true)` 会不可恢复地退出进程，普通库路径不应随意启用。
- `MockPkixAttribute` 遇到未知短名会 panic；它由注释明确定位为测试帮助函数。`ParseAndCheckSAN` 接受空值（例如 `DNS:`）但拒绝无冒号条目和未知键；`CheckSupportX509NameOneline` 使用完整 `split('=')`，因此值中额外的 `=` 会被判为非法。
- `ColumnMetadata` 的 ID/flag/type/collation 都由实现者提供；本文件不验证数值合法性。`-1` 被硬编码为额外 handle ID，`1 << 23` 和 collation `63` 也属于跨语言协议常量，修改时必须和 Go/MySQL/tipb 语义同步。
- `LoadTLSCertificates` 对 CA PEM 使用 `X509::stack_from_pem(...).unwrap_or_default()`；无法解析的 CA 会退化为空 CA 集合和较弱的客户端认证策略，而不是返回解析错误。证书/私钥读取、解析或公钥不匹配则返回错误。
- `TlsConfig` 的锁若 poisoned 会 panic。热加载失败返回旧证书而非错误，调用者要通过日志监控轮换失败。
- `initInternalClient` 对 CA、身份文件和 client 构造错误使用 `expect`，首次访问可能 panic；只配置 cert 或只配置 key 时不会装入 client identity，但仍会选择 HTTPS。
- `ComposeURL` 仅做字符串拼接，不规范化斜杠或验证 URL。`GetLocalIP` 依赖系统枚举顺序，无法保证在多网卡环境选中期望接口，错误或无合适地址时返回空串。
- `CreateCertificates` 会截断目标文件；证书文件写入没有像私钥那样显式设置权限，也没有跨两个文件的事务性替换。调用方应使用受控目录并处理部分写入。

## 并发与资源生命周期

`RunWithRetry` 和证书生成都是同步阻塞操作；前者占用当前线程睡眠，后者执行密钥生成和文件 I/O。`InternalHTTPClient` 也是 `reqwest::blocking::Client`，应在同步调用路径使用，不能假定其为 Tokio 异步 client。

进程级惰性初始化由 `Once`/`OnceLock` 保证并发调用只发布一份反向 OID 表、HTTP client 和 schema。原子 TLS 开关采用 Acquire/Release，足以发布单个标量配置；它们没有与 `TlsConfig` 构造形成多字段事务。证书热加载用读写锁保护：`certificate()` 在读锁内克隆 OpenSSL 句柄，`reload_certificate()` 先在锁外完成文件 I/O/解析，成功后才短暂持有写锁替换，从而避免慢磁盘操作阻塞读者。

`WithRecovery` 只覆盖当前闭包所在的线程栈，不管理新线程或异步任务的 join/cancel 生命周期。`CancellationToken` 也仅由 `HasCancelled` 读状态，取消令牌的创建与触发归调用者所有。文件句柄方面，证书使用 `fs::write`，私钥文件在函数退出时由 RAII 关闭；写私钥前显式 `flush`，但没有 `sync_all` 持久化保证。

## 与 Go 版本的对应关系

直接对照文件为 [`pkg/util/misc.go`](misc.go)，独立回归为 [`pkg/util/misc_test.go`](misc_test.go) 和 [`pkg/util/misc_test.rs`](misc_test.rs)。主要结构与 Go 一致：三种重试终态、恢复回调、语法错误/警告区分、PKIX/SAN 规则、列 Proto 分支、序列解耦接口、TLS 认证策略、内部 HTTP 五分钟超时、全局单播 IP、自签证书算法以及类型 Flags 公式均保留。

已验证的实现差异必须视为当前事实，而非等价细节：

- Go `RunWithRetry` 每次可重试失败会增加 `RetryableErrorCount`，Rust 版本只保留错误与退避，没有指标副作用；两者退避都是线性而非注释所称“指数式”。
- Go `WithRecovery` 使用 `defer/recover` 并记录栈，Rust 使用 `catch_unwind`，当前日志只有格式化后的载荷；两者都会在正常返回时以空值调用回调。Go `Recover` 在测试中的特定 assert panic 会重新抛出并增加 PanicCounter，Rust 版本没有这两项行为，而且要求调用者显式传入捕获载荷。
- Go 语法错误识别 `terror.Error`/stack 并生成 parser 错误码；Rust 只识别本地 `SqlSyntaxError`，可保留消息和 warning 标记，但不是 Go parser error 的完整类型替代。
- Go 列转换直接接收 `model.ColumnInfo` 并返回 tipb 类型；Rust 以 trait/DTO 解耦。Rust 的 `-1`、`1 << 23`、`63` 对应 Go 的 `model.ExtraHandleID`、`mysql.GeneratedColumnFlag` 和 binary collation，但缺少命名常量和 Go 的新 collation ID 重写调用，具体数值正确性由实现 `ColumnMetadata` 的上游负责。
- Go 的最低 TLS 版本来自全局字符串配置并拒绝早于 1.2；Rust 由 `SetMinimumTLSVersion` 写入仅含 1.2/1.3 的原子枚举。Go 使用标准库 `tls.Config.GetCertificate` 每次握手重读证书；Rust 暴露显式 `reload_certificate`。Go 对非法 CA PEM 保留非空 pool 但不升级认证策略，Rust 得到空 CA 列表，效果近似但表示不同。
- Go 自动证书路径用 `filepath.Join(tempStoragePath, "/cert.pem")`；Rust 在空配置时显式回退系统临时目录。Rust 证书签名基于 OpenSSL，Ed25519 强制 null digest，其他算法将 unspecified 映射 SHA-256。
- Go 内部 HTTP 通过完整 `ClusterSecurity().ToTLSConfig()` 构造 transport；Rust 手工加载 CA/PEM identity。两者均一次初始化，后续配置变化不生效。

## 扩展指南

- 增加或改变重试策略时修改 `RunWithRetry`，在独立 [`pkg/util/misc_test.rs`](misc_test.rs) 中同步覆盖成功、不可重试、耗尽、零次数及时间/溢出边界；若恢复 Go 指标副作用，应先确认 util crate 的依赖方向，避免为指标引入循环依赖。
- 扩充证书 DN 或 SAN 类型时同时更新 `pkixAttributeTypeNames`/反向表或 SAN 白名单逻辑，并同步 `MockPkixAttribute`、`X509NameOnline`、授权入口和 Go 对照测试。注意 SAN 值是否允许空串、冒号和逗号属于兼容协议，不能只改解析器一侧。
- 接入新的列模型时实现 `ColumnMetadata`，不要让 `astersql-util` 反向依赖 planner/model。协议字段变化应同时检查 `ColumnToProto`、`ColumnsToProto` 和 `pkg/planner/core/plan_to_pb_test.rs`，尤其是额外 handle、collation 重写、数组元素类型和 TiFlash generated flag。
- 注册序列查找器时在明确的进程初始化点调用 `GetSequenceByName.set(...)` 并处理重复注册；扩展 `SequenceTable` 会影响所有 trait object 实现，必须先搜索实现者与调用者。
- 修改 TLS 策略时同时评估 `LoadTLSCertificates`、`TlsConfig::reload_certificate`、`CreateCertificates`、`pkg/server/server.rs` 和 `pkg/server/tests/tls/tls_test.rs`。高风险点包括认证降级、最低协议、密码套件、密钥权限、轮换并发和文件部分写入。
- 修改内部 HTTP 配置必须记住 `Once` 快照语义；若需要运行时刷新，应设计显式生命周期和并发替换，而不是仅更改全局配置。URL 规范化如需增强，应在 `ComposeURL` 的 Rust/Go 测试中固定双斜杠、已有 scheme、IPv6 地址和路径查询串行为。
- Rust 单元测试继续保存在独立 `misc_test.rs` 或相关调用方测试中，不要嵌入生产源文件；本文件行为涉及的服务端 TLS 集成测试继续放在 `pkg/server/tests/tls/tls_test.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/util/misc.rs` 读取了 798 行完整实现并报告该文件被 58 个文件使用；`query` 分别定位了 Rust/Go 的 `RunWithRetry`、`LoadTLSCertificates`、`InternalHTTPClient`、`ColumnsToProto`、`GetTypeFlagsForInsert`。精确 `callers/callees` 未对同名跨语言符号返回可用边，因此没有把空结果写成无调用者结论。
- 已读源码与边界：`pkg/util/misc.rs`、`pkg/util/lib.rs`、`pkg/util/Cargo.toml`；`pkg/util` 目录没有 `doc.go`，因此无更近的包级 Go 契约可读。
- 已读对照与测试：`pkg/util/misc.go`、`pkg/util/misc_test.go`、`pkg/util/misc_test.rs`。额外直接证据来自 `pkg/executor/grant.rs`、`pkg/privilege/privileges/cache.rs`、`pkg/server/server.rs`、`cmd/tidb-server/main.rs`、`tests/globalkilltest/util.rs`、`pkg/server/tests/tls/tls_test.rs` 和 `pkg/planner/core/plan_to_pb_test.rs` 的符号引用。
- 独立 Rust 测试证明：重试的三种终态；PKIX 输出顺序；panic 载荷交付；语法错误的空值、包装与原样保留；基础列转换；URL scheme 拼接；IPv4/IPv6 link-local 排除。更完整的 TLS 和列协议分支分别由服务端 TLS 测试与 planner 测试提供证据。
- 本任务是只读分析加 Markdown 输出，按计划不运行 Cargo。交付前执行固定章节结构检查，并人工核对本文能回答文件存在原因、执行路径、共享状态、失败边界、Go 差异和安全扩展位置。
