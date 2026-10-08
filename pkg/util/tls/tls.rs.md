# `pkg/util/tls/tls.rs`

## 文件定位

本文件是 `astersql-util-tls` crate 的业务实现文件，crate 入口 `pkg/util/tls/lib.rs` 以 `pub mod tls` 声明模块并通过 `pub use tls::*` 再导出公共项。`pkg/util/tls/Cargo.toml` 将 `lib.rs` 指定为库入口，且不声明第三方依赖；本文件只使用标准库集合、延迟初始化和原子类型。

它不是 TLS 握手或证书加载实现，而是 TLS 元数据兼容层：把协议版本号、密码套件编号转换成 TiDB/MySQL/OpenSSL 对外使用的名称，并公开账户授权校验所需的支持套件集合及一个安全传输开关。实际证书加载和客户端认证策略位于 `pkg/util/misc.rs`。

## 核心职责

1. `VersionName` 将 `u16` TLS 版本号转换为显示名。TLS 1.2、1.3 优先使用 `versionString` 中无空格的 MySQL/OpenSSL 兼容写法 `TLSv1.2`、`TLSv1.3`；旧版本沿用 Go `crypto/tls.VersionName` 风格；未知编号格式化为四位大写十六进制。
2. `CipherSuiteName` 用 `tlsCipherString` 将 25 个标准 cipher suite 编号映射为 MySQL/OpenSSL 兼容名称，未知编号返回空字符串。
3. `SupportCipher` 从同一映射的全部值派生支持名称集合，避免授权校验再维护一份独立名单。
4. `RequireSecureTransport` 提供进程级、可并发读写的 `AtomicBool`，保持 Go 包中 `atomic.Bool` 的数据形状；当前 Rust 生产调用搜索未发现它与 `pkg/util/misc.rs` 的证书加载开关接通。

## 主要符号

- `VERSION_TLS12`、`VERSION_TLS13`：私有协议版本常量，值分别是 `0x0303`、`0x0304`，同时用于映射键和 `VersionName` 回退分支。
- 25 个 `TLS_*` 私有常量：与 Go `crypto/tls` 的 cipher suite 数值对应，覆盖旧式 RSA/RC4/3DES/CBC、ECDHE、GCM、ChaCha20 以及 TLS 1.3 套件。它们只表示编号，不负责启用密码算法。
- `RequireSecureTransport: AtomicBool`：公开静态原子开关，初值为 `false`。本文件不封装内存序，调用者必须在 `load`、`store` 或 `swap` 时自行选择 `Ordering`。
- `versionString: LazyLock<HashMap<u16, &'static str>>`：私有、只读的版本覆盖表，只含 TLS 1.2 和 TLS 1.3。
- `tlsCipherString: LazyLock<HashMap<u16, &'static str>>`：私有、只读的编号到兼容名称映射。
- `SupportCipher: LazyLock<HashSet<&'static str>>`：公开、只读的名称集合，由 `tlsCipherString.values()` 首次访问时构造。
- `pub fn VersionName(version: u16) -> String`：公开版本命名入口，每次返回拥有所有权的 `String`。
- `pub fn CipherSuiteName(n: u16) -> String`：公开套件命名入口；无法识别时以空串表达“不支持/未知”。

## 执行流程

`VersionName` 先触发或读取 `versionString`。若编号为 TLS 1.2/1.3，立即复制兼容名称并返回；否则进入本地 `match`，识别 SSL 3.0、TLS 1.0、TLS 1.1（代码也保留 1.2/1.3 的完整回退分支），最后把未知值格式化为 `0xNNNN`。因此 `0x0303` 和 `0x0304` 的实际结果分别是 `TLSv1.2`、`TLSv1.3`，不会到达带空格的回退结果。

`CipherSuiteName` 首次调用会初始化 `tlsCipherString`，随后按编号查表；命中时复制静态字符串为 `String`，未命中时返回 `String::new()`。`SupportCipher` 首次访问时也会确保该映射已初始化，再复制所有值引用到集合。`pkg/executor/grant.rs::account_tls_options_to_global_priv` 使用集合成员关系判断账户 TLS cipher 条件是否合法；`pkg/sessionctx/variable/statusvar.rs` 使用两个命名函数填充会话 SSL 状态，并按固定 25 个编号构造 `TLS_SUPPORTED_CIPHERS`。

## 数据与状态

三个映射/集合的键和值在初始化完成后不再改变，字符串均是 `&'static str`，进程生命周期内有效。`HashMap`、`HashSet` 的迭代次序没有承诺；本文件只用查找与成员判断。状态变量中的支持套件展示顺序由 `statusvar.rs::TLS_CIPHERS` 数组决定，而不是由 `SupportCipher` 的集合顺序决定。

唯一可变状态是 `RequireSecureTransport`。它默认关闭，独立测试以 `Ordering::SeqCst` 验证 `store`、`load`、`swap`，测试结束时恢复为 `false`。生产代码如直接使用该公开原子值，需要自己维持修改前后恢复规则和一致的内存序。需特别注意：`pkg/util/misc.rs` 另有私有同名 `AtomicBool`，其 `SetRequireSecureTransport`、`RequireSecureTransportEnabled` 和 `LoadTLSCertificates` 使用的是另一个状态，本文件没有自动同步机制。

## 依赖与调用关系

下游依赖全部来自标准库：`HashMap` 保存编号映射，`HashSet` 保存支持名称，`LazyLock` 负责线程安全的一次性初始化，`AtomicBool` 提供跨线程标志。本 crate 的 `Cargo.toml` 没有 `[dependencies]`，因此 TLS 常量值在本地声明，而非从 Rust TLS 库导入。

上游由 `lib.rs` 再导出后，主要存在两条生产调用链：

- 会话状态链：`pkg/sessionctx/variable/lib.rs` 将依赖的 `tls` 模块再导出为 `tlsutil`；`statusvar.rs::TLS_SUPPORTED_CIPHERS` 调 `CipherSuiteName` 生成支持列表，`DefaultStatusStat::Stats` 调 `CipherSuiteName` 和 `VersionName` 生成 `Ssl_cipher`、`Ssl_version`。
- 账户权限链：`pkg/executor/grant.rs::account_tls_options_to_global_priv` 查询 `astersql_util_tls::SupportCipher`，再由 `tls_options_to_global_priv_with_validation` 校验 `CREATE/ALTER USER ... REQUIRE CIPHER` 一类选项。

根 `Cargo.toml` 还以 `facade_util_tls` 注册本 crate，`pkg/lib.rs` 将其内容纳入 facade；`pkg/executor/Cargo.toml` 和 `pkg/sessionctx/variable/Cargo.toml` 是上述调用链的直接 crate 依赖声明。RustCodeGraph 对目标文件列出 3 个函数级符号，并显示文件被多个 Rust/Go 文件引用；因精确 `callers`/`callees` 查询没有返回边，本节的具体 Rust 调用边由上述源码搜索核验。

## 错误处理与边界

两个公开函数均为无错误返回 API，不读文件、不做网络操作，也不触发 TLS 协商。`VersionName` 对所有 `u16` 都有确定结果，未知值用 `format!("0x{version:04X}")` 表示；格式至少四位、十六进制字母大写。`CipherSuiteName` 则用空串表示未知编号，调用者若把结果用于拼接或展示，必须自行区分空串；本文件不会记录日志或返回 `Result`。

`SupportCipher` 仅认可映射值的精确大小写与标点。TLS 1.0/1.1 虽可由 `VersionName` 显示，但不在 `versionString` 的 TiDB 对外覆盖表中；“能命名”不等于“服务器允许协商”。类似地，映射内保留旧式 cipher 名称只证明兼容名称和授权校验名单存在，不证明底层 TLS 实现实际启用了相应算法。

## 并发与资源生命周期

`LazyLock` 保证每张表或集合在首次访问时至多初始化一次，初始化后只读并存活到进程结束；不存在显式释放、锁持有跨调用或后台任务。`SupportCipher` 初始化期间会读取 `tlsCipherString`，该单向依赖不存在循环初始化。

`RequireSecureTransport` 本身无锁并可跨线程访问，但语义取决于调用方选择的 `Ordering`。本文件没有提供复合事务、通知机制或与证书配置的同步屏障，所以更新该原子值不会自动重载证书或改变另一个模块中的开关。测试修改全局原子状态时应串行化或可靠恢复，否则并行测试可能互相影响。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/tls/tls.go`。Rust 的 `versionString`、`tlsCipherString`、`SupportCipher`、`VersionName`、`CipherSuiteName` 与 Go 同名对象逐项对应：25 个 cipher 编号和字符串一致，TLS 1.2/1.3 使用同样的覆盖名，未知 cipher 同样返回空串。Go `init` 循环填充 `SupportCipher`，Rust 改为 `LazyLock<HashSet>` 的按需派生；Go 的 `map[string]struct{}` 对应 Rust 的集合。

Go 对未知/旧版本调用标准库 `tls.VersionName`；Rust 没有使用对应库，因而在 `VersionName` 内显式复现 SSL 3.0、TLS 1.0、TLS 1.1 和未知十六进制格式。`pkg/util/tls/tls_test.go::TestVersionName` 与 `pkg/util/tls/tls_test.rs::test_version_name` 使用相同六组输入输出验证这一点。

Go 的 `RequireSecureTransport` 被 `pkg/sessionctx/variable/sysvar.go` 和 `pkg/executor/simple.go` 直接读写，用于系统变量及 TLS 重载失败处理。当前 Rust 搜索仅在 `pkg/util/tls/migration_aster_unit_test.rs` 发现本文件原子值的有效直接使用；Rust 证书加载路径使用 `pkg/util/misc.rs` 内部的另一原子值。因此只能确认数据结构和单元语义已迁移，不能宣称该开关已完整对齐 Go 的应用接线。

## 扩展指南

新增或改名 cipher suite 时，应同时修改本文件的编号常量与 `tlsCipherString` 条目，并同步 `pkg/util/tls/migration_aster_unit_test.rs::cipher_names_and_supported_set_match_go_tables` 的编号、名称和集合长度断言；若状态展示应包含它，还必须同步 `pkg/sessionctx/variable/statusvar.rs::TLS_CIPHERS` 及其独立测试。名称是 SQL 授权校验和状态展示的兼容接口，修改大小写、连字符或下划线会改变 `SupportCipher.contains` 的结果，需核对 MySQL/OpenSSL 与 Go 行为。

新增版本覆盖时，应修改 `versionString`，必要时补充本地回退 `match`，并同步 `tls_test.rs::test_version_name`、`tls_test.go::TestVersionName` 和迁移一致性测试。不要仅因能显示一个版本就推断底层协商支持它。

若要完成安全传输开关接线，应先决定本文件 `RequireSecureTransport` 与 `pkg/util/misc.rs` 私有开关的唯一所有者，避免两个原子状态分叉；随后在独立 Rust 测试文件中覆盖系统变量、证书加载和并发可见性。按照仓库约束，不应把测试内嵌进 `tls.rs`。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/util/tls` 确认目标、入口、Go 对照和测试均已索引；`node --file pkg/util/tls/tls.rs --offset 1 --limit 260` 读取完整 186 行并报告 3 个符号；`query VersionName`、`query CipherSuiteName`、`query RequireSecureTransport` 用于消歧。精确 `callers`/`callees` 未产生可用输出，因此没有据此虚构调用边。
- 实现与 crate 边界：`pkg/util/tls/tls.rs`、`pkg/util/tls/lib.rs`、`pkg/util/tls/Cargo.toml`。
- Rust 调用证据：`pkg/sessionctx/variable/lib.rs`、`pkg/sessionctx/variable/statusvar.rs`、`pkg/sessionctx/variable/Cargo.toml`、`pkg/executor/grant.rs`、`pkg/executor/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照：`pkg/util/tls/tls.go`、`pkg/sessionctx/variable/sysvar.go`、`pkg/executor/simple.go`。
- 独立测试：`pkg/util/tls/tls_test.rs` 与 `pkg/util/tls/tls_test.go` 覆盖版本命名；`pkg/util/tls/migration_aster_unit_test.rs` 覆盖全部 25 个 cipher 映射、未知 cipher、集合一致性及原子读写/恢复。
- 人工复核边界：本文件只做命名、集合和原子状态，不执行握手；未知版本和未知 cipher 的返回策略不同；Rust 安全传输原子值尚未证明接入 Go 对应的生产链。
