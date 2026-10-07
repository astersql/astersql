# `pkg/lightning/common/security.rs`

## 文件定位

本文件属于 `astersql-lightning-common` crate 的 TLS/安全适配层。crate 入口 `pkg/lightning/common/lib.rs` 以 `mod security` 编译本模块，再通过 `pub use security::*` 将公开符号暴露给调用方；`pkg/lightning/common/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/lightning/common`，本文件本身没有直接声明第三方依赖或 feature/条件编译项。

从设计角色看，它应位于 Lightning 配置与连接端点之间；从当前仓库事实看，精确检索没有确认 `NewTLS`、`WrapListener` 等函数被 Rust 生产代码调用。`lightning/pkg/server` 和 `lightning/cmd/tidb-lightning-ctl` 虽有同名调用，却引用 `lightning/pkg/server/stubs.rs` 中的本地 `crate::common` stub，并非本文件。当前 Rust 类型主要保存材料和接口形状：`TLSConfig` 不是 `rustls`/OpenSSL 配置，`DialOption` 只是布尔标志，`TLSListener` 只把 listener 与配置装在一起，`GetJSON` 只支持显式注入的回调。因此它既不能被描述为已接入 Lightning 生产主链，也不能被描述为已经实现 Go 版本的真实 TLS 握手与 HTTP/gRPC transport。

RustCodeGraph 已索引 `security.rs` 的 25 个符号，并能精确定位 `NewTLS`、`WithHost`、`GetJSON` 等入口；其 `explore`、文件 `node` 和精确 ID 的 callers/callees 本轮未返回内容，所以调用边又以源码范围精确检索和直接读取调用点核验，未把同名候选当作真实边。

## 核心职责

本文件承担四组职责：

1. 用 `TLS` 保存证书路径、内存材料、加载后的 `TLSConfig`、基础 URL 和可选 HTTP 回调，并由 `NewTLS` 决定明文或安全模式。
2. 将 `TLS` 投影为不同消费者需要的轻量结构：`DialOption`、`TLSListener<L>`、`PDSecurityOption` 与 `TiKVSecurityConfig`。
3. 为地址替换和 HTTP GET 提供 `WithHost`、`GetJSON`，并为独立测试提供 `MockTLSServer`、`NewTLSFromMockServer` 和 `GetMockTLSUrl`。
4. 保留 Go `pkg/lightning/common/security.go` 的公开调用面和路径/内存材料优先级，为后续替换成真实 transport 留出接入点。

安全边界必须明确：当前 `looks_like_pem` 只搜索文本标记，不解析证书、密钥或证书链；`WrapListener` 不执行握手；`ToGRPCDialOption` 不创建 gRPC credentials；普通 `NewTLS` 不创建 HTTP client。这些是可观测的当前实现限制，而不是文档遗漏。

## 主要符号

- `TLSConfig { CA, Cert, Key }`：加载后的三组原始字节。字段公开，类型可克隆、比较和调试；它不含协议版本、根证书池、服务名或校验器。
- `DialOption { Secure }`：gRPC 拨号意图的轻量表示；是否安全只由 `Option<TLSConfig>` 是否为 `Some` 决定。
- `PDSecurityOption`：同时保留 CA/证书/密钥路径与原始字节，字段映射 Go `pd.SecurityOption`。
- `TiKVSecurityConfig`：只保留三条路径与 `ClusterVerifyCN`；当前转换总是生成空 CN 列表。
- `TLSListener<L>`：泛型包装值，包含原始 `Listener` 和克隆的可选配置；没有实现 `Read`、`Write`、`Accept` 或 TLS 协议 trait。
- `HTTPGetter`：`Arc<dyn Fn(&Context, &str) -> Result<Vec<u8>, CommonError> + Send + Sync>`，是唯一 HTTP 执行抽象，可跨线程共享。
- `MockTLSServer`：测试夹具，保存可选配置、完整 URL 和 `HTTPGetter`；虽为公开类型，源码明确将其定位为 mock。
- `TLS`：核心句柄。路径、原始字节、`inner`、`client` 与 `url` 均私有，只能通过方法读取或转换；派生 `Clone` 会复制字符串/字节/config，并共享 `HTTPGetter` 的 `Arc`。
- `read_path_or_content`、`looks_like_pem`：私有辅助函数，分别实现“非空路径优先于内容”和最低限度 PEM 标记检查。
- `NewTLS`：公开构造器，加载材料、选择证书/密钥对、做标记检查、决定 scheme 并构造 `TLS`。
- `NewTLSFromMockServer`、`GetMockTLSUrl`：复制 mock 状态以及读取当前 URL 的测试辅助入口。
- `TLS::{WithHost, ToGRPCDialOption, WrapListener, GetJSON, ToPDSecurityOption, ToTiKVSecurityConfig, TLSConfig}`：地址派生、消费者适配和内部配置只读访问方法。
- 自由函数 `ToGRPCDialOption`：把 `Option<&TLSConfig>` 映射为 `DialOption`；同名方法只委托给它。

本文件没有模块级常量、trait、宏或条件编译项。

## 执行流程

`NewTLS` 首先计算 `has_any_material`：六个路径/字节输入只要任意一个非空，就视为启用安全模式。随后 `read_path_or_content` 加载 CA；非空 `caPath` 必须可读且覆盖 `caBytes`，否则直接使用内存字节。

客户端证书与密钥必须成对选择。若 `certPath` 与 `keyPath` 都非空，就分别读文件，并让完整路径对覆盖内存对；否则仅当 `certBytes` 与 `keyBytes` 都非空时才采用内存对；其余孤立的路径或字节被忽略并转为空向量。选中的 cert/key 任一非空时，两者都必须通过 `looks_like_pem`；非空 CA 也必须通过同一检查。失败返回 `CommonError(kind="tls")`。

若六项输入全部为空，`inner=None`，URL 为 `http://{host}`；否则即使有效载荷最终为空（例如只有孤立 cert path），也会得到 `inner=Some(TLSConfig { ... })` 和 `https://{host}`。构造器保存原始路径和原始字节，而 `inner` 保存实际读取/选择出的字节；两者用途不同。

`WithHost` 先至多剥离一个开头的 `http://` 或 `https://`，克隆整个 `TLS`，再依据原实例 `inner` 是否存在重建 scheme。`ToGRPCDialOption`、`WrapListener`、`ToPDSecurityOption`、`ToTiKVSecurityConfig` 都是同步投影，不做 I/O。`GetJSON` 要求 `client=Some`，拼接 `self.url + path` 后调用回调；当前只有 `NewTLSFromMockServer` 会注入 client，普通 `NewTLS` 总是留下 `None`。

## 数据与状态

`TLS` 构造后没有内部可变性。`WithHost` 返回新值而不改原对象；`TLSConfig()` 返回借用，不能经该 API 修改 `inner`。公开的配置投影返回拥有所有权的新值，因此调用者修改结果不会回写 `TLS`。

路径字段和字节字段保留调用者输入，`inner` 则保留加载结果。这一差异决定转换行为：`ToPDSecurityOption` 返回原路径及原始内存字节，不返回从文件读出的字节；`ToTiKVSecurityConfig` 只返回原路径，无法传递内存材料；`TLSConfig` 才能看到加载后的 CA/cert/key。证书文件在构造时读取一次，没有轮换或重新加载机制。

“是否安全”的唯一状态判据是 `inner.is_some()`，不是材料是否完整或是否能够建立真实会话。因此孤立 cert/key 输入会启用 HTTPS/`Secure=true`，但内部 cert/key 为空；独立 Rust 测试 `test_incomplete_key_pair_is_ignored` 明确固定了这一行为。

`MockTLSServer::Client` 和 `TLS::client` 通过 `Arc` 共享同一个闭包；克隆 `TLS` 不复制闭包状态。其他字段使用深拷贝。`TLSListener<L>` 拥有传入 listener，因此包装调用会移动 listener，而不是借用它。

## 依赖与调用关系

向上，`pkg/lightning/common/lib.rs` 再导出本文件全部公开符号。仓库范围的精确 Rust 符号检索只确认 `pkg/lightning/common/security_test.rs` 调用 `NewTLS`、`NewTLSFromMockServer`、`WithHost`、`GetJSON` 和 `TLSConfig`；没有确认函数级 Rust 生产调用方。`lightning/pkg/server/lightning.rs` 的 `common::NewTLS`/`WrapListener` 实际解析到 `lightning/pkg/server/stubs.rs::common`；`lightning/cmd/tidb-lightning-ctl/stubs.rs` 又重导出该 server stub。因此这些同名调用不是本文件的上游边，只能作为另一套缩减实现的背景，不能用来证明本文件已经接线。

向下，本文件只直接依赖标准库 `fs`、`Arc`，以及 crate 内 `CommonError` 和 `Context`。`CommonError` 定义于 `pkg/lightning/common/errors.rs`；`Context` 定义于 `pkg/lightning/common/pause.rs`，本文件不读取取消状态，只将其传给注入的 `HTTPGetter`。`pkg/lightning/common/Cargo.toml` 的 crate 依赖是 `astersql-lightning-log` 与 `libc`，但本文件均未直接使用。

同 crate 的 `pkg/lightning/common/util.rs` 在 `MySQLConfig`/`MySQLConnectParam` 中也持有本文件的 `TLSConfig`，用于传递 MySQL TLS 材料；这是一条类型依赖，不经过 `TLS` 构造和 HTTP/listener 流程。多个 crate 在 Cargo manifest 中依赖 `astersql-lightning-common`，但仓库精确符号检索没有据此推断它们调用安全 API。

## 错误处理与边界

CA 路径非空但不可读时，`read_path_or_content` 把 `fs::read` 错误转换为 `CommonError::new("tls", error.to_string())`。完整 cert/key 路径对中任一读取失败也同样返回 `tls` 错误；错误保留操作系统文本，但没有补充具体是 CA、cert 还是 key 字段，也没有保留结构化 I/O source。

PEM 检查仅将字节按损失式 UTF-8 转换并查找 `-----BEGIN `。它能拒绝测试中的普通非法文本，却会接受含该子串但无法解析的证书、错误私钥、cert/key 不匹配和不可信 CA。与 Go `util.NewTLSConfig` 的 X.509/key-pair 解析相比，这只是形状检查，不构成密码学验证。

不完整 cert/key 对不会报错，而会被忽略；但输入非空仍使 `inner=Some`。路径对只有两条路径都非空才会访问文件，所以孤立且不存在的 cert path 不触发 I/O 错误。CA 则只要路径非空就必须读取。`WithHost` 不解析 URL，不校验空 host、端口、路径、查询参数，也不会处理大写 scheme；它只是字符串前缀处理。

`GetJSON` 不解析 JSON：返回的是回调给出的原始字节。没有 client 时返回 `CommonError(kind="http", message="no HTTP transport has been configured")`；路径直接拼接，调用者负责 `/`、转义和 URL 安全。回调错误原样返回。`ToTiKVSecurityConfig` 丢弃内存材料且不支持 CN；`WrapListener` 和 `ToGRPCDialOption` 也不提供真实安全保证。

## 并发与资源生命周期

`HTTPGetter` 要求 `Send + Sync` 并由 `Arc` 持有，因此带 mock client 的 `TLS` 可在线程间共享/克隆；文件本身没有锁、原子变量、异步任务或通道。所有转换方法只读 `self`，`NewTLS` 中的文件读取是同步阻塞 I/O。

本文件不持有文件句柄或 socket：证书文件读取后立即关闭，`TLSListener` 仅取得 listener 所有权，资源释放依赖 `L` 自身的 `Drop`。没有为 `TLS` 或 `TLSListener` 实现 `Drop`，也没有证书热更新。`Arc<HTTPGetter>` 在最后一个 `TLS`/`MockTLSServer` 引用释放时销毁；如果闭包捕获外部资源，其清理由闭包捕获类型负责。

`NewTLSFromMockServer` 克隆 `TLSConfig`、URL，并 `Arc::clone` client；之后修改原 mock 的普通字段不会更新既有 `TLS`，但闭包内部共享状态仍可能被双方观察。`GetJSON` 对闭包并发安全的要求由 `Send + Sync` 在类型层表达，具体回调内部一致性由实现者负责。

## 与 Go 版本的对应关系

直接基准是 `pkg/lightning/common/security.go`，相关 Go 测试位于 `pkg/lightning/common/security_test.go`。两边都保留 `TLS` 的路径/字节输入，以是否存在配置决定 HTTP/HTTPS，`WithHost` 都剥离输入 scheme 后按内部配置重建 URL，PD/TiKV 转换的字段形状也一致。Rust 测试复刻了 Go 的明文/安全 mock GET、host 替换和非法证书场景，并额外固定 CA 路径优先级与不完整 key pair 行为。

当前 Rust 与 Go 的关键差异是：

- Go `NewTLS` 调用 `pkg/util/security.go::NewTLSConfig`，构建真实 `*tls.Config`，解析 CA 与 key pair，并创建 `httputil.NewClient(inner)` 或明文 `http.Client`；Rust只保存字节并做 PEM 子串检查，`NewTLS` 不注入 HTTP transport。
- Go `ToGRPCDialOption` 构建 TLS 或 insecure gRPC credentials；Rust只返回 `DialOption { Secure }`。
- Go `WrapListener` 在有配置时调用 `tls.NewListener`，否则返回原 listener；Rust无论哪种模式都返回 `TLSListener<L>` 容器，不执行 TLS 协议。
- Go `GetJSON` 使用 HTTP client 解码 JSON 到调用者对象；Rust调用回调并返回原始字节，函数名不代表已反序列化。
- Go mock 直接包装 `httptest.Server`；Rust使用仓库自定义 `MockTLSServer` 和闭包，不启动网络服务。
- Go 的底层 TLS 配置支持真实证书校验行为与客户端证书轮换闭包；本文件没有协议版本、主机名/CN 校验、根池或轮换。
- Go `ToTiKVSecurityConfig` 同样只传路径并把 CN 留空，源码已有“不支持传内容”和 CN FIXME；Rust忠实保留这个限制。

因此 Rust 当前主要对齐配置选择和调用接口，不是 Go 网络安全能力的等价实现。扩展时不能仅凭测试中的 `Secure=true` 或 PEM 标记断言端到端 TLS 已生效。

## 扩展指南

若要实现真实 TLS，首要接入点是 `TLSConfig`、`NewTLS`、`ToGRPCDialOption` 和 `WrapListener`：应复用仓库已有 `pkg/util/security.rs` 的解析/验证能力或经批准的上游依赖，不要再扩展 `looks_like_pem` 成自制解析器。需要明确客户端/服务端配置、最低协议版本、主机名/CN 校验、ALPN 和证书轮换契约，并在独立的 `pkg/lightning/common/security_test.rs` 增加真实握手、错误 CA、错配 key、过期/主机名错误等回归测试；测试逻辑不要放入生产文件。

若要让 `GetJSON` 用于生产，应把真实 HTTP transport 注入策略落实到 `NewTLS`，同时决定 API 是返回原始响应、执行 JSON 反序列化，还是改名避免误导。应测试无 client、HTTP 状态、超时/取消、URL 拼接、响应体错误与连接复用，并确认 `Context` 取消能被 transport 消费。

若只扩展投影类型，应同步维护 `ToPDSecurityOption`、`ToTiKVSecurityConfig` 及其消费者，尤其注意路径与内存材料并非等价：TiKV 当前会丢失内存证书，PD 则两者都保留。新增 CN、server name 或 verify 模式时，必须定义默认值并与 Go 文件及 Go 测试对齐。

任何行为修复都应先在 `security_test.rs` 添加会失败的回归测试，再修改 `security.rs`；Rust 源与测试继续分文件。若引入外部 Rust 依赖，按仓库规则在独立上游仓库移植、提交、打 tag，并让所有 Cargo manifest 使用同一已发布 tag，不能加入本地 `[patch]` 或 vendor 副本。

## 验证依据

- 计划与范围：只读检查 `.plans/2026-10-07-rust-全架构逐文件解析/plan.md` 和任务 1454；任务无前置依赖，限定输出即本文档。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/lightning/common` 确认源、Go 对照与两套独立测试均被索引；`query` 精确定位 `security.rs::NewTLS`、`NewTLSFromMockServer`、`WithHost`、`GetJSON`、两种 `ToGRPCDialOption` 及转换方法。`explore`、文件 `node` 与精确 ID callers/callees 未返回正文，故调用关系使用精确源码检索复核，并明确记录该图查询限制。
- Rust 源与 crate 边界：`pkg/lightning/common/security.rs`、`pkg/lightning/common/lib.rs`、`pkg/lightning/common/Cargo.toml`、`pkg/lightning/common/errors.rs`、`pkg/lightning/common/pause.rs`、`pkg/lightning/common/util.rs`。
- 调用排歧：读取 `lightning/pkg/server/lightning.rs`、`lightning/pkg/server/stubs.rs`、`lightning/cmd/tidb-lightning-ctl/stubs.rs` 与 `main.rs`，确认其中同名 `common::NewTLS`/`WrapListener` 来自 server 本地 stub，不属于目标文件；目标 API 当前只确认被 `pkg/lightning/common/security_test.rs` 直接调用。
- Go 对照：`pkg/lightning/common/security.go` 和其下游 `pkg/util/security.go::NewTLSConfig`；它们证明 Go 侧真实 TLS config、HTTP client、gRPC credentials 与 listener 包装行为。
- 独立测试：`pkg/lightning/common/security_test.rs` 覆盖 HTTP/HTTPS mock GET、scheme 重建、非法 PEM、CA 路径优先和不完整 key pair；`pkg/lightning/common/security_test.go` 覆盖对应的 Go 基线场景。
- 本任务为纯文档分析，按计划未运行 Cargo。交付检查使用任务指定的 11 章节结构命令、文档链接/事实复核与 diff 自审；仓库指令所称 `.agents/skills/tidb-verify-profile` 在当前工作树不存在，因此无法加载其 Ready 命令集合。
