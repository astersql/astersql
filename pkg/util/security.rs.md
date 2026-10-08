# `pkg/util/security.rs`

## 文件定位

本文件是 `astersql-util` crate 的通用 TLS 边界；对应源码为 [`pkg/util/security.rs`](security.rs)，并由 `pkg/util/lib.rs` 以 `pub mod security` 导出。它把证书材料和验证策略转换成 `rustls` 客户端/服务端配置，并进一步提供阻塞 HTTP 客户端、明文/TLS TCP 监听器及流类型。应用侧的直接入口包括 `pkg/server/server.rs` 的 SQL TLS 配置与对端 CN 校验、`pkg/server/http_status.rs` 的状态端口 TLS，以及 BR 等组件调用的 `ToTLSConfig`；因此它位于配置输入与实际网络握手之间，不负责读取全局配置或管理服务器任务。

crate 边界由 `pkg/util/Cargo.toml` 确认：运行时直接依赖 `anyhow`、`reqwest`（blocking + rustls）、`rustls`、`rustls-native-certs`、`rustls-pemfile`、`rustls-webpki`（源码别名 `webpki`）和 `x509-parser`；证书生成仅在测试依赖 `rcgen` 中出现。

## 核心职责

1. `NewTLSConfig` 汇总路径、内存 PEM、CN 白名单和最低协议版本，完成 CA/密钥材料的早期校验并产生可克隆的 `TlsConfig`；完全没有证书材料时返回 `Ok(None)`。
2. `TlsConfig::client_config` 与 `server_config` 把中立配置转换为 `rustls` 配置，并根据 CA、CN 白名单和兼容策略选择验证器；两者都设置 ALPN 为 `http/1.1`、`h2`。
3. `ToTLSConfig` / `ToTLSConfigWithVerify` 保留 Go 路径式 API；`NewTLS`、`ClientWithTLS`、`HttpClient::Get` 负责 HTTP 使用方式和 `http(s)://host` 拼装。
4. `TLS::WrapListener`、`Listener::accept`、`Connection` 把同一配置用于阻塞 TCP 服务端，并让明文和 TLS 连接共同实现 `Read`/`Write`。
5. 私有验证器将 rustls 的证书链/签名验证与可选 CN 白名单组合；`NoCertificateVerifier` 则显式承载兼容 Go 的完全跳过校验分支。

## 主要符号

- `TLS12`、`TLS13`、`TLS12_AND_TLS13`、`TLS13_ONLY`：TLS wire version 常量及 rustls 协议集合。`protocol_versions` 只接受 `0x0303` 和 `0x0304`。
- `TlsConfigBuilder`：选项收集器，保存路径、内存内容、CN 列表和最低版本，不对外导出。
- `TLSConfigOption` 与 `WithCAPath`、`WithCertAndKeyPath`、`WithVerifyCommonName`、`WithCAContent`、`WithCertAndKeyContent`、`WithMinTLSVersion`：公开构造选项。路径 CA 优先于内存 CA；证书/密钥路径在使用时优先于内存对。
- `TlsConfig`：已解析配置。`roots` 和 `verify_cn` 用 `Arc` 共享；Debug 输出只透露是否存在 roots、路径、CN、版本与策略，不打印私钥/证书内容。`min_tls_version`、`verify_common_name[_pem]` 是检查接口。
- `NewTLSConfig`：主构造器。默认最低版本为 TLS 1.2；CN 会先 `trim`；内存密钥对在构造期解析，路径密钥对在实际构建 config 时读取。
- `client_config`：有 CA 时可校验证书链；有 CN 时在标准 server-name/链验证后追加 CN；无 CN 时走兼容分支——有 CA 只验链、不验主机名，无 CA 使用 `NoCertificateVerifier` 完全信任。
- `server_config`：服务端必须有证书和私钥。有 CN 时必须同时有 CA，并要求及校验客户端证书；只有 CA 时使用 `allow_unauthenticated`，即客户端证书可选；无 CA 时不做客户端认证。
- `parse_ca`、`parse_key_pair`、`protocol_versions`、`verify_common_name`：PEM、版本与 CN 的集中校验函数。
- `CommonNameServerVerifier` / `CommonNameClientVerifier`：先委托 rustls 内层验证器验证链、名称和签名，再检查叶证书 Subject CN；签名方案能力完全转发给内层验证器。
- `CertificateChainServerVerifier`：以 WebPKI 验证 server-auth 链和握手签名，但故意忽略 `ServerName`，用于“有 CA、无 CN”的兼容路径。
- `NoCertificateVerifier`：证书、TLS 1.2/1.3 握手签名均直接返回成功，仅从 aws-lc provider 报告支持的签名方案，是明确的高风险兼容实现。
- `HttpClient`、`ClientWithTLS`、`TLS`、`NewTLS`：阻塞 HTTP 适配层。每次 `Get` 新建 reqwest client；`TLS.url` 根据 `inner` 是否存在选择 HTTP 或 HTTPS。
- `Listener`、`Connection`、`ClientTlsStream`：明文/TLS TCP 抽象；`Connection` 透明实现 `Read`/`Write`，类型别名描述客户端侧 rustls 流。

## 执行流程

构造流程从 `NewTLSConfig(options)` 开始：依次覆盖 builder 字段；若 CA、证书和私钥的路径与内容全空，立即返回 `None`。否则优先读取 `ca_path`，没有路径才使用 `ca_content`，通过 `parse_ca` 形成 `RootCertStore`。当使用内存证书/私钥且两者均非空时立即调用 `parse_key_pair`；只给出其中一项不会在这里形成密钥对。CN 列表会去除每项首尾空白，最低版本的零值转成 TLS 1.2。若内存密钥对完整，构造末尾额外调用一次 `client_config`，提前暴露版本或客户端配置错误。

客户端配置由 `client_config` 生成：先由 `protocol_versions` 选择 TLS 1.2+1.3 或仅 1.3；有显式 CA 就克隆它，无显式 CA且需要主机名验证时加载系统根证书，无显式 CA且跳过主机名时使用空 roots。随后加载可选客户端密钥对。验证策略按 `skip_hostname_verification` 和 `verify_cn` 分派：路径兼容入口会将前者改为 `false`；通用 builder 无 CN 时则为 `true`。最后设置 ALPN 并返回共享配置。

服务端配置由 `server_config` 生成：先强制取得密钥对，再选择协议版本。CN 非空时要求 CA，使用 `WebPkiClientVerifier` 加 `CommonNameClientVerifier`；CN 空但有 CA 时允许匿名客户端、对提交证书的客户端做链验证；CA 也为空时不认证客户端。三条分支最终都安装服务端证书并设置 ALPN。

HTTP 流程是 `NewTLS -> ToTLSConfigWithVerify -> NewTLSConfig`，然后 `HttpClient::Get` 在每次请求时调用 `client_config`、构建 reqwest client 并发送 GET。监听流程是 `TLS::WrapListener` 选择 `Listener::Plain/Tls`；TLS 分支的 `accept` 接收 socket、重新生成 `ServerConfig` 并创建 `ServerConnection`，首次 `Read/Write` 由 `StreamOwned` 推进握手和传输。

## 数据与状态

`TlsConfig` 自身没有可变全局状态。CA 在构造期解析为不可变 `Arc<RootCertStore>`；CN 白名单也是 `Arc<Vec<String>>`。证书和私钥的内存内容由实例持有并在生成 config 时克隆；路径只保存 `PathBuf`，`certificate_and_key` 每次生成 client/server config 都重新读文件。因此 `WithCertAndKeyPath` 支持文件替换后的证书轮转，而 CA 路径不支持同样的热重载：CA 在 `NewTLSConfig` 时只读一次。

选项是按传入顺序覆盖同类字段；路径和内容可以同时保留，但使用时路径优先。证书与私钥必须成对才会加载；不完整的路径或内容不会自动补齐另一种来源。`skip_hostname_verification` 不是公开选项，而是由 CN 是否为空初始化，且路径式 `ToTLSConfigWithVerify` 会强制改为 `false`。

`HttpClient` 仅持有可选共享配置，没有连接池跨请求复用，因为每次 `Get` 都重建 reqwest client。`TLS` 同时持有同一 `Arc<TlsConfig>` 的 `inner` 和 `client` 克隆，并缓存无尾斜杠的 scheme/host 字符串。

## 依赖与调用关系

RustCodeGraph 的主调用链为 `NewTLS -> ToTLSConfigWithVerify -> NewTLSConfig`，以及 `ToTLSConfig -> ToTLSConfigWithVerify -> NewTLSConfig`。`NewTLSConfig` 下游调用 `parse_ca`、`parse_key_pair` 和 `client_config`；`ToTLSConfigWithVerify` 还会先读取路径密钥对并调用 `parse_key_pair`。图中直接 Rust 调用方包括 `pkg/server/server.rs` 的 `build_sql_tls_config` / `verify_peer_common_name`、`pkg/server/http_status.rs` 的 `build_status_tls_config`、`cmd/tidb-server/main.rs` 的启动客户端路径，以及 BR 的 common/operator 和测试工具。

外部库分工清晰：`rustls-pemfile` 解码 PEM，`x509-parser` 读取 Subject CN，`rustls-webpki` 执行忽略主机名时仍需保留的 server-auth 链验证，`rustls-native-certs` 提供系统根，rustls aws-lc provider 提供协议和签名算法，reqwest 消费预构建 `ClientConfig`。标准库负责文件、阻塞 TCP、共享所有权和 I/O trait。

模块装配证据在 `pkg/util/lib.rs`；独立单元测试通过同文件的 `#[path = "security_test.rs"] mod security_test` 挂载。另一个正式测试 target `security_2_aster_unit_test` 由 `pkg/util/Cargo.toml` 指向 `security_formal_aster_unit_test.rs`，后者 include `security_2_aster_unit_test.rs`。

## 错误处理与边界

所有构造与 HTTP/监听操作使用 `anyhow::Result`，底层 I/O、PEM、rustls、reqwest 错误通过 `?` 传播；关键文件操作用 `Context` 补充“could not read ca certificate”或“could not load client key pair”。`parse_ca` 在 PEM 中没有任何可接受证书时返回 `failed to append ca certs`；`parse_key_pair` 区分没有证书和没有私钥；服务端无密钥对、CN 校验无 CA、非法最低版本都有专门错误。

CN 比较是修剪配置值后的精确、区分大小写字符串比较；只检查传入的叶证书 Subject CN，不把 SAN 当作白名单，也不遍历中间证书。空白 CN 配置项会被修剪为空字符串但仍令白名单“非空”，因此可能开启强制客户端认证；扩展或配置层不应把仅含空白的列表当作未配置。

安全边界必须显式理解：通用 `NewTLSConfig` 在 CN 为空时跳过主机名；有 CA仍校验证书链，无 CA则 `NoCertificateVerifier` 完全跳过证书和握手签名验证。相反，`ToTLSConfigWithVerify` 的空 CA 直接返回 `None`，非空 CA会强制恢复标准 server-name 验证。调用者不应把这两个入口视为完全等价。

TLS 1.0/1.1 和未知版本在 `protocol_versions` 被拒绝。`Listener::accept` 只完成 socket 接收和 rustls 流装配；握手错误通常在后续 `Read/Write` 才出现。`HttpClient::Get` 只返回 response，不在此负责消费或关闭响应体。

## 并发与资源生命周期

`TlsConfig` 可 `Clone`，内部只读 roots/CN 通过 `Arc` 共享，适合跨线程使用；本文件无 mutex、后台线程、异步任务或通道。每次配置生成都创建独立 rustls provider/config，路径证书在该时点读取，因此轮转以“下一次 `client_config` / `server_config` 调用”为生效边界。已有 `StreamOwned` 已持有生成时的 rustls connection，不会因文件变化而改变。

`Listener` 拥有底层 `TcpListener`；`accept(&self)` 可反复接受连接。返回的 `Connection` 拥有 `TcpStream`，离开作用域即关闭；TLS 连接还拥有 `ServerConnection` 的会话状态。`HttpClient::Get` 每次构建并在调用末尾丢弃 reqwest client，response 的资源生命周期交给调用方。测试 `pkg/util/security_test.rs` 用线程和停止标志管理示例服务，这些并发设施不属于生产文件本身。

## 与 Go 版本的对应关系

主要 API 对应 `pkg/util/security.go` 的同名 `TLS`、`ToTLSConfig[WithVerify]`、选项函数、`NewTLSConfig`、`NewTLS`、`ClientWithTLS` 和 `WrapListener`。共同语义包括：空路径式 CA 禁用 TLS、默认最低 TLS 1.2、路径优先于内容、密钥对路径支持轮转、CN 值 trim 后匹配，以及 CA/CN/密钥错误的主要错误文本。

实现并非逐字段等价。Go 用 `crypto/tls` 回调和 `x509.CertPool`，Rust 用 rustls verifier trait；Go 的 CN 回调遍历 `verifiedChains`，Rust只检查 verifier 收到的叶证书。Go builder 的默认 `InsecureSkipVerify=true` 由自定义 CA/CN 回调弥补，Rust则拆成标准 server-name、只验链和完全信任三种验证器。Go 路径证书由 `GetClientCertificate` / `GetCertificate` 回调按握手加载；Rust是在每次创建 rustls config 时加载，当前 HTTP 每请求、监听器每 accept 都会重新创建 config，因此现有封装下仍能观察轮转。

协议方面，Rust明确只实现 TLS 1.2/1.3；Go 测试通过设置 Min/Max 测试 1.0/1.1 失败。Go `NewTLSConfig` 源码的 ALPN 含 `http/1.2`，路径构造为 `h2`/`http/1.1`；Rust两条路径统一为 `http/1.1`/`h2`。Go `TestCA` 的“无 CA”场景会在测试中注入可信 roots，Rust对应测试直接覆盖完全信任兼容分支，安全强度不同，不能据测试名称误认为二者验证链完全一致。

## 扩展指南

- 新增证书来源或优先级时修改 `TLSConfigOption`、`TlsConfigBuilder`、`NewTLSConfig` 和 `certificate_and_key`，并在独立的 `pkg/util/security_test.rs` 增加路径/内容冲突、不完整配对和轮转用例；不要把测试嵌入生产文件。
- 新增协议版本时同时更新版本常量、静态集合、`protocol_versions`、默认行为和 `test_tls_version`；还需确认 rustls provider 实际支持该版本，而非仅接受数值。
- 改动验证策略应分别覆盖四个维度：客户端 server-name、证书链、CN、服务端客户端认证。尤其不要无意扩大 `NoCertificateVerifier` 的使用范围；安全收紧也要评估依赖 Go 兼容行为的调用方。
- CN 规则若迁移到 SAN、大小写归一化或多证书链语义，应集中修改 `verify_common_name`，并同步 `CommonNameServerVerifier`、`CommonNameClientVerifier`、Rust/Go 对照测试及用户可见错误文本。
- 若追求 HTTP 连接复用或减少 provider/config 构建成本，应调整 `HttpClient` 持有预建 reqwest client；但需先定义证书轮转触发和并发一致性，否则会改变当前“每次 GET 重读路径密钥对”的行为。
- 服务端热重载若改为缓存 `ServerConfig`，必须保留或显式替代当前每次 accept 读取路径证书的生命周期，并在 server 集成测试中验证旧/新连接边界。

## 验证依据

- 生产源码：`pkg/util/security.rs`，核对全部 769 行；关键符号为 `NewTLSConfig`、`TlsConfig::{client_config,server_config}`、`ToTLSConfigWithVerify`、四个 verifier、`NewTLS`、`Listener::accept` 和 `TLS::WrapListener`。
- crate/装配：`pkg/util/Cargo.toml`、`pkg/util/lib.rs`；前者确认依赖和两个测试入口，后者确认公开模块与 `security_test.rs` 独立挂载。目标目录没有 `doc.go`。
- Go 对照：`pkg/util/security.go`、`pkg/util/security_test.go`；核对构造优先级、CN、轮转、TLS 版本、CA 和监听器行为。
- Rust 测试：`pkg/util/security_test.rs` 的 `test_invalid_tls`、`test_verify_common_name_and_rotate`、`test_tls_version`、`test_ca`；`pkg/util/security_2_aster_unit_test.rs` 的 `security_builder_validates_input_and_common_names`；`pkg/util/security_formal_aster_unit_test.rs` 确认正式 target 挂载。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/util/security.rs` 分段核对全文件；`explore "pkg/util/security.rs TLSConfig ToTLSConfig NewTLS"` 得到主调用链及 server/BR/测试调用方；`query ToTLSConfig`、`query TLSConfig` 消除 Go/Rust 同名歧义；`callees NewTLSConfig`、`callees ToTLSConfigWithVerify`、`callees WrapListener` 核对直接下游。
- 本任务是纯文档分析，按计划不运行 Cargo；验收只执行任务规定的十一章结构命令，并人工检查上述结论均能回溯至源码、图查询、Cargo、Go 或测试证据。
