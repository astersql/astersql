# `pkg/util/service_url.rs`

## 文件定位

本文件属于 `astersql-util` crate 的 `service_url` 模块，由 [`pkg/util/lib.rs`](lib.rs) 通过 `pub mod service_url` 对外暴露。它位于服务发现和具体网络客户端之间：把用户配置、PD 成员列表或 mock 配置中的端点字符串解析成统一的 `ServiceURL`，但自身不建立连接、不解析 DNS，也不持有客户端状态。

直接生产使用点包括：[`pkg/util/urls.rs`](urls.rs) 的逗号分隔地址列表解析、[`pkg/metaservice/etcd.rs`](../metaservice/etcd.rs) 的 PD 成员地址筛选与拆解，以及 [`pkg/store/mockstore/unistore/pd.rs`](../store/mockstore/unistore/pd.rs) 的 mock PD 地址规范化和有效性判断。

## 核心职责

- 只接受四种协议：`http`、`https`、`unix`、`unixs`，对应 `URLSchemeHTTP`、`URLSchemeHTTPS`、`URLSchemeUnix`、`URLSchemeUnixs`。
- 将显式 URL 或配合默认协议的裸 `host:port` 解析为不变量明确的 `ServiceURL { scheme, address }`。
- 为调用者提供协议前缀、无协议地址、按需保留协议的 endpoint 和规范化字符串。
- 对齐 Go [`pkg/util/service_url.go`](service_url.go) 的主要契约，同时用局部解析逻辑弥合 Rust `url` crate 与 Go `net/url`、`net.SplitHostPort` 的差异。

本文件不是通用 URL 解析器。它面向可拨号服务端点，不保留 URL 的路径；对 HTTP(S) URL 的 query 和 fragment 也不写入规范化结果。

## 主要符号

- `URLSchemeHTTP`、`URLSchemeHTTPS`、`URLSchemeUnix`、`URLSchemeUnixs`：受支持协议的公开字符串常量。
- `ServiceURL`：公开的、可克隆且可比较的值类型；`scheme` 和 `address` 字段私有，调用者只能通过成功解析获得值并通过方法读取。
- `supported(scheme)`：内部协议白名单判断。
- `split_host_port(address)`：内部语法拆分器。接受普通 `host:port` 和方括号包裹的 IPv6 `[host]:port`；只验证结构，不要求端口为数字，也不解析端口范围。
- `ParseServiceURL(raw) -> anyhow::Result<ServiceURL>`：要求输入自带协议的公开入口；它把空字符串作为默认协议传给内部解析器，因此无协议输入会报错。
- `parse_service_url(raw, default_scheme)`：crate 内共享的核心解析器；[`pkg/util/urls.rs`](urls.rs) 直接调用它为裸地址补 `http`。
- `NormalizeServiceURL(raw, default_scheme) -> anyhow::Result<String>`：解析后通过 `Display` 输出 `scheme://address`。
- `ServiceURL::SchemePrefix()`：返回带 `://` 的协议前缀。
- `ServiceURL::Address()`：借用内部地址，不包含协议。
- `ServiceURL::Endpoint(with_scheme)`：显式要求时返回完整 URL；Unix 系列即使 `with_scheme == false` 也保留协议，保证 Unix socket 地址仍可识别。
- `ServiceURL::IsUnixFamily()`：判断协议是否为 `unix` 或 `unixs`。
- `fmt::Display for ServiceURL`：稳定输出 `scheme://address`，也是标准化结果的最终格式。

## 执行流程

`ParseServiceURL` 与 `NormalizeServiceURL` 最终都进入 `parse_service_url`，后者按以下顺序处理：

1. 对输入执行 `trim`；空结果立即返回 `URL must not be empty`。
2. 若没有 `://`，要求 `default_scheme` 在四协议白名单内，并要求 `split_host_port` 能拆出主机和端口；成功后原样保存裁剪后的地址，并采用默认协议。此分支允许服务名端口，例如 `host:service`。
3. 若以 `unix://` 或 `unixs://` 开头，直接取前缀后的全部内容作为地址；仅拒绝空地址。因此 `unix:///tmp/etcd.sock` 会保存 `/tmp/etcd.sock`，不会套用 HTTP authority/path 规则。
4. 其余显式 URL 先交给 `url::Url::parse` 做语法检查。仅 `ParseError::InvalidPort` 可继续，因为 WHATWG 解析器拒绝服务名端口，而 Go 接受；其他解析错误转换成带原始输入和底层错误的 `anyhow` 错误。
5. 从原字符串分离 scheme 与 `://` 后内容，检查 scheme 白名单；authority 在第一个 `/`、`?` 或 `#` 前结束。若含 userinfo，则使用最后一个 `@` 后的 host/port。
6. `split_host_port` 必须成功，且 host、port 都非空。authority 后若以 `/` 开头则拒绝路径；query 或 fragment 不报错，但不会写入 `address`。
7. 构造 `ServiceURL`。标准化或需要完整 endpoint 时，`Display` 再拼接 `scheme://address`。

例如，测试证明 `NormalizeServiceURL(" [::1]:2379 ", "https")` 变为 `https://[::1]:2379`；`http://host:service?x=1` 变为 `http://host:service`；`unixs:///tmp/etcd.sock` 保留为原形式。

## 数据与状态

`ServiceURL` 只拥有两个 `String`：协议和地址。字段私有确保外部代码无法绕过解析器制造不受支持的协议或缺失地址；`Clone`、`Eq`、`PartialEq` 和 `Debug` 使其适合配置传递、断言与诊断。

解析过程没有全局变量、缓存或可变共享状态。裸地址分支保留裁剪后的原始 host/port 文本；显式 HTTP(S) 分支只保留 authority 中去除 userinfo 后的 host/port；Unix 分支保留协议前缀之后的全部地址。`Endpoint` 和 `Display` 返回新 `String`，`Address` 则返回与 `ServiceURL` 生命周期绑定的 `&str`。

## 依赖与调用关系

crate 边界由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义：包名为 `astersql-util`，库入口为 `lib.rs`；本文件直接使用 `anyhow = "1"` 统一返回动态错误，使用 `url = "2"` 校验显式非 Unix URL，并使用标准库 `std::fmt` 实现显示格式。

RustCodeGraph 核对到的文件内调用边为：

- `ParseServiceURL -> parse_service_url`；
- `NormalizeServiceURL -> parse_service_url -> {supported, split_host_port}`；
- `ServiceURL::Endpoint -> ServiceURL::IsUnixFamily`；
- `supported -> URLScheme*` 常量，`Display` 最终读取两个私有字段。

上游调用关系为：

- `ParseHostPortAddr`（[`pkg/util/urls.rs`](urls.rs)）对逗号分隔的每项调用 `parse_service_url(entry, "http")`，并依据原输入是否显式含 `://` 决定普通网络地址是否保留协议。
- `get_pd_addrs`（[`pkg/metaservice/etcd.rs`](../metaservice/etcd.rs)）用 `ParseServiceURL` 过滤 PD 成员上报的不可用 URL，再通过 `Endpoint(with_scheme)` 输出；`parse_url` 将错误映射成 `MetaServiceError::ServiceUrl`。
- `normalize_mock_pd_addrs` 和 `valid_url`（[`pkg/store/mockstore/unistore/pd.rs`](../store/mockstore/unistore/pd.rs)）以默认 `http` 调用 `NormalizeServiceURL`；前者丢弃解析失败项，后者只取成功与否。

## 错误处理与边界

所有解析失败都通过 `anyhow::Result` 返回，不会 panic；源码中唯一的 `expect("checked above")` 位于已经确认包含 `://` 的分支，因此依赖同一函数内紧邻的不变量。

明确拒绝的情况包括：裁剪后为空；裸地址使用空白或不支持的默认协议；缺失 `host:port` 结构；显式协议不在白名单；HTTP(S) host 或 port 为空；HTTP(S) 带 `/path`；Unix 协议后没有任何地址。错误文本保留原始或裁剪后的输入，便于调用者定位配置问题。

`split_host_port` 是结构兼容层而非完整 socket 校验器：它允许空字段并由显式 URL 分支随后拒绝，但裸地址分支只检查能否拆分，因此裸 `host:` 或 `:port` 的最终约束弱于显式 HTTP(S) 分支。它也不验证数字端口、端口范围、DNS 名是否合法或地址是否可达。Unix 分支有意不施加 host/port 与路径规则。

HTTP(S) 的 query 和 fragment 被接受后丢弃；路径只要 authority 后以 `/` 开始就被拒绝。扩展或复用本 API 时不得把它当作能保真往返任意 URL 的组件。

## 并发与资源生命周期

本文件完全同步、无锁、无线程、无 async task、无通道、无事务，也不进行文件或网络 I/O。每次解析只使用局部借用和新分配的 `String`；返回值拥有数据，不依赖输入缓冲区。`ServiceURL` 没有 `Drop` 行为或外部资源，克隆仅复制两个字符串，因此可以由上层按其自身同步策略跨线程传递。

复杂度与输入长度线性相关，主要成本是字符串扫描、一次可选的 `Url::parse` 和少量字符串分配。Unix 快速分支不调用通用 URL 解析器。

## 与 Go 版本的对应关系

直接对照文件是 [`pkg/util/service_url.go`](service_url.go)，测试是 [`pkg/util/service_url_test.go`](service_url_test.go)。Rust 保留了 Go 的四个协议常量、`ServiceURL` 数据模型、公开函数名和方法语义；`Display` 对应 Go 的 `String()`。

主要实现差异来自标准库：Go 使用 `net/url.Parse` 和 `net.SplitHostPort`，Rust 使用 `url::Url` 加自定义 `split_host_port`。Rust 在 `Url::parse` 返回 `InvalidPort` 时继续做结构检查，以保留 Go 接受 `host:service` 的行为；方括号 IPv6 也由自定义拆分器覆盖。Rust 还显式从 authority 中剥离 userinfo，以得到与 Go `u.Host` 相同的 host/port 部分。

独立 Rust 测试 [`pkg/util/go_merge_34_service_url_test.rs`](go_merge_34_service_url_test.rs) 覆盖了 Go 测试中的 HTTP、HTTPS、Unix、空输入、不支持协议、缺端口、路径和空 Unix 地址，并额外覆盖 IPv6、`unixs`、服务名端口与 query 丢弃。两侧当前共同约束 Unix endpoint 必须保留 scheme。

## 扩展指南

- 新增协议时，应同时更新四处契约：协议常量、`supported`、必要的协议专用解析分支，以及 `IsUnixFamily`/`Endpoint` 是否必须保留 scheme；再同步 Go 文件和独立 Rust/Go 测试。仅把名称加入白名单可能错误地套用 HTTP authority 规则。
- 收紧 host/port 校验时，优先修改 `split_host_port` 与 `parse_service_url` 的分支级不变量，并为裸地址、显式 URL、IPv6 和服务名端口分别加用例；注意 `pkg/util/urls.rs`、PD 服务发现和 mock PD 都会受到影响。
- 若要支持路径、保留 query/fragment 或用户信息，应先明确规范化输出的数据模型；当前 `ServiceURL` 只有 scheme/address，直接放宽检查会造成信息静默丢失。
- 错误类型若从 `anyhow` 改为结构化枚举，需要同步 `pkg/metaservice/etcd.rs::parse_url` 的错误映射及依赖调用者对 `.is_ok()`/`.ok()` 的处理。
- 测试逻辑应继续放在独立文件 [`pkg/util/go_merge_34_service_url_test.rs`](go_merge_34_service_url_test.rs)，并由 [`pkg/util/lib.rs`](lib.rs) 的 `#[cfg(test)]` 模块声明接线，不要内嵌到生产源文件。

兼容风险主要是改变既有输入的接受集合或规范化文本；性能风险较低，但把 Unix/裸地址也统一交给通用 URL 解析器会增加分配并可能破坏 Go 兼容语义。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；使用 `status`、`explore`、`query`、`node`、`callers` 和 `callees` 核对目标文件符号、源码与调用边。精确查询确认 Rust `ParseServiceURL` 的生产调用者为 `get_pd_addrs`、`parse_url`，`NormalizeServiceURL` 的生产调用者为 `normalize_mock_pd_addrs`、`valid_url`；`parse_service_url` 另由 `ParseHostPortAddr` 直接调用。
- 生产源码：[`pkg/util/service_url.rs`](service_url.rs)、[`pkg/util/lib.rs`](lib.rs)、[`pkg/util/urls.rs`](urls.rs)、[`pkg/metaservice/etcd.rs`](../metaservice/etcd.rs)、[`pkg/store/mockstore/unistore/pd.rs`](../store/mockstore/unistore/pd.rs)。
- crate 配置：[`pkg/util/Cargo.toml`](Cargo.toml)，核对 `astersql-util`、`lib.rs`、`anyhow` 与 `url` 依赖。
- Go 对照：[`pkg/util/service_url.go`](service_url.go) 与 [`pkg/util/service_url_test.go`](service_url_test.go)。
- Rust 独立测试：[`pkg/util/go_merge_34_service_url_test.rs`](go_merge_34_service_url_test.rs)，并由 [`pkg/util/lib.rs`](lib.rs) 的 `go_merge_34_service_url_test` 测试模块接入。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查固定章节结构、链接所指路径与差异范围。
