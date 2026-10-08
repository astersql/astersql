# `pkg/server/internal/handshake/handshake.rs`

## 文件定位

本文件属于独立 crate `astersql-server-internal-handshake`，crate 边界由同目录的 `Cargo.toml` 定义，入口是 `lib.rs`。`lib.rs` 通过 `pub mod handshake` 装载本文件，再以 `pub use handshake::*` 对外再导出其中的公开项。当前文件只有 `Response41` 一个生产符号，没有函数、trait、常量、条件编译项或协议解析实现。

它位于 MySQL 客户端初始握手响应的数据模型层：保存客户端在 Protocol 4.1 握手响应中声明的能力、身份、认证数据和可选元数据。字节流解析位于相邻 crate 的 `pkg/server/internal/parse/parse.rs`，而不是本文件。根 `Cargo.toml` 以 `facade_server_internal_handshake` 登记该 crate，`pkg/server/Cargo.toml` 和 `pkg/server/internal/parse/Cargo.toml` 都通过路径依赖引用它。

当前 Rust 接线只可确认到解析层：仓库中的 Rust 引用由 `pkg/server/internal/parse/lib.rs` 转接，再由 `handshake_response_header` 与 `handshake_response_body` 写入 `Response41`。没有找到与 Go `pkg/server/conn.go` 中连接认证流程等价的 Rust 生产调用者，因此不能把 Go 侧完整的 TLS 升级、认证插件协商和开会话流程描述成 Rust 已接线能力。

## 核心职责

`Response41` 是一次成功初始握手应答的可变数据载体，职责有三项：

1. 保存固定 32 字节前缀中的 `capability` 和 `collation`；对应写入点是 `pkg/server/internal/parse/parse.rs::handshake_response_header`。
2. 保存受 capability 位控制的可变部分，包括 `user`、`auth`、`db_name`、`auth_plugin`、`attrs` 和 `zstd_level`；对应写入点是 `handshake_response_body`。
3. 提供 `Default` 初始状态，使解析器能先建立全空/全零对象，再按实际包内容逐项填充；该约定由 `pkg/server/internal/handshake/migration_aster_unit_test.rs` 验证。

本文件不负责校验包长度、不解释 capability 位、不选择认证算法，也不拥有网络连接。它是解析阶段与后续认证阶段之间的数据契约，而非握手状态机。

## 主要符号

唯一公开类型为 `#[derive(Default)] pub struct Response41`。所有字段均为 `pub`，调用者可以直接读取或改写：

- `attrs: HashMap<Vec<u8>, Vec<u8>>`：连接属性的原始字节键值。解析器仅在 `ClientConnectAtts` 置位且属性行成功解析时替换它。
- `user: Vec<u8>`：NUL 结尾的用户名原始字节。
- `db_name: Vec<u8>`：`ClientConnectWithDB` 置位时的默认数据库名原始字节。
- `auth_plugin: Vec<u8>`：`ClientPluginAuth` 置位时的认证插件名原始字节。
- `auth: Vec<u8>`：认证插件相关载荷；其编码由 capability 决定，可能采用长度编码、单字节长度前缀或 NUL 结尾形式。
- `zstd_level: isize`：`ClientZstdCompressionAlgorithm` 置位时读取的压缩等级。
- `capability: u32`：客户端能力位集合，既决定其余字段的解析方式，也供后续协议协商使用。
- `collation: u8`：固定握手头第 8 字节中的字符集/排序规则编号。

派生的 `Default` 是本类型唯一实现：字节向量和属性表为空，整数为零。类型没有派生 `Clone`、`Debug`、比较或序列化能力；若扩展调用场景需要这些能力，应先确认是否会无意复制认证载荷或把敏感字段输出到日志。

## 执行流程

本文件自身没有可执行函数；围绕该数据结构的已验证 Rust 流程如下：

1. 调用方以 `Response41::default()` 创建空对象。
2. `handshake_response_header` 要求输入至少 32 字节，从偏移 0 读取小端 `u32` 到 `capability`，从偏移 8 读取一个字节到 `collation`，并返回可变体起点 32。
3. `handshake_response_body` 从该偏移读取 NUL 结尾的 `user`。
4. 解析器依据 `capability` 在三种认证载荷格式之间选择并填充 `auth`，然后按对应能力位依次尝试填充 `db_name`、`auth_plugin` 和 `attrs`。
5. 若声明 zstd 能力，解析器最后读取一个字节并转换为 `isize` 写入 `zstd_level`。
6. 后续 Rust 生产消费链在当前仓库中未找到；可验证的直接消费者主要是解析测试。Go 对照流程则在 `pkg/server/conn.go::readOptionalSSLRequestAndHandshakeResponse` 中继续把结构字段复制到连接状态并进入认证插件处理。

真实 MySQL 8.0 抓包路径由 `pkg/server/internal/parse/handshake_test.rs::TestAuthSwitchRequest` 覆盖：先解析头部和主体，再断言 `auth_plugin` 为 `caching_sha2_password`。

## 数据与状态

`Response41` 聚合的是单个客户端握手响应的解析结果，不含全局状态。所有文本型协议字段在 Rust 中保留为 `Vec<u8>`，因此模型层不会提前假设 UTF-8；`pkg/server/internal/parse/migration_aster_unit_test.rs` 明确用 `0xff`、`0xfe`、`0xfd` 验证非 UTF-8 用户名、数据库名、插件名和属性仍可保存。需要字符串语义的后续层必须自行选择严格校验或有损转换策略。

`capability` 是其余字段的解释前提，而非普通附加元数据。未声明相应 capability 时，相关字段保持默认空值；因此“空字段”可能表示客户端未发送、能力未启用或发送了零长度值，单靠结构本身不能区分这些来源。

对象拥有所有字节缓冲区和属性键值，不借用输入包。解析器通过 `to_vec()` 复制切片，因此解析完成后结构生命周期不依赖原始网络缓冲区。代价是每个存在的可变字段及属性键值都会发生独立分配或复制。

## 依赖与调用关系

本文件唯一直接代码依赖是标准库 `std::collections::HashMap`，且以全限定名出现在字段类型中。同目录 `Cargo.toml` 没有 `[dependencies]` 或 feature 声明，说明该数据模型 crate 本身不依赖其他 AsterSQL crate。

已核对的上游与下游关系为：

- 导出：`pkg/server/internal/handshake/lib.rs` 装载并公开再导出 `Response41`。
- crate 消费：`pkg/server/internal/parse/Cargo.toml` 依赖 `../handshake`；`pkg/server/internal/parse/lib.rs` 将它映射到解析模块使用的 `server::internal::handshake::Response41`。
- 写入者：`pkg/server/internal/parse/parse.rs::handshake_response_header` 写 `capability`、`collation`；`handshake_response_body` 写其余六个字段。
- Rust 测试消费者：`pkg/server/internal/handshake/migration_aster_unit_test.rs` 检查结构契约；`pkg/server/internal/parse/migration_aster_unit_test.rs` 检查各 capability 分支和畸形输入；`pkg/server/internal/parse/handshake_test.rs` 检查真实抓包。
- Go 生产消费者：`pkg/server/conn.go::readOptionalSSLRequestAndHandshakeResponse` 创建 `handshake.Response41`，经 Go 解析器填充后用于 TLS/安全传输判断、连接状态初始化、zstd 设置、认证插件协商和最终认证。

RustCodeGraph 对 `Response41` 的精确查询找到了 Rust/Go 两个定义和两个同目录迁移测试；对该结构报告无 callees。结构体不是函数，图中没有调用边属于正常结果。仓库文本引用搜索补足了字段写入者与 crate 依赖；RustCodeGraph 文件级 “used by `pkg/util/security_test.rs`” 未能由精确符号查询或 `rg` 复现，因此未把它认定为真实调用者。

## 错误处理与边界

`Response41` 的构造和字段赋值不返回错误，错误边界全部位于解析器及后续消费者。`handshake_response_header/body` 使用 `ParseError::MalformedPacket` 表示长度、终止符或索引不合法，用 `ConnectionAttributesTooLarge` 拒绝超过 1 MiB 的连接属性。畸形的可选属性行按 Go 行为记录告警并继续握手，不会由本结构保存错误状态。

需要特别保留的边界包括：固定头必须至少 32 字节；认证载荷存在三种 capability 驱动格式；数据库、插件、属性和 zstd 字段均为条件字段；非 UTF-8 字节有效；属性硬上限由解析器实施。直接手工构造 `Response41` 会绕过上述协议校验，所以业务代码不应把“类型可构造”误当成“内容已验证”。

本结构没有隐藏敏感数据：`auth` 公开且随对象存活，不会自动清零。新增日志或 `Debug` 实现时不得直接泄露认证载荷，也应谨慎处理用户名和连接属性中的隐私信息。

## 并发与资源生命周期

类型不含锁、原子变量、引用计数、通道、任务或文件描述符，也没有自定义 `Drop`。所有字段均由对象独占；移动对象会转移其缓冲区所有权，离开作用域时由 `Vec` 和 `HashMap` 正常释放。标准容器字段使该类型在字段类型允许时可在线程间移动，但本文件没有声明或保证多线程共享协议。

解析阶段通过 `&mut Response41` 独占修改对象，天然阻止同一实例被多个安全 Rust 调用方并发写入。若未来需要跨任务共享，应由更高层明确加锁和规定握手状态转换；不应在这个纯数据结构中隐式加入全局同步。解析连接属性时使用的指标锁位于 `pkg/server/internal/parse/parse.rs`，不属于 `Response41` 生命周期。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/server/internal/handshake/handshake.go`。Rust 与 Go 均定义八个同义字段，顺序一致：Attrs、User、DBName、AuthPlugin、Auth、ZstdLevel、Capability、Collation；`u32`/`uint32` 和 `u8`/`uint8` 宽度直接对应，`isize` 对应 Go 的本机宽度 `int`。

存在以下语言层差异：

- Go 的 `string` 能保存任意字节且不可变；Rust 使用可变的 `Vec<u8>`，同样避免强制 UTF-8，但允许调用方原地改变内容。
- Go 的 `map[string]string` 零值是 nil map；Rust 派生默认值得到空 `HashMap<Vec<u8>, Vec<u8>>`。两者读取长度均为零，但 Rust 默认表可直接插入，而 Go nil map 写入前必须初始化。因此同目录测试证明的是当前读取语义和字段形状对齐，不是所有零值操作都完全等价。
- Rust 结构只派生 `Default`，Go 结构可直接按语言规则复制，其中 map 复制仍共享底层映射；Rust 当前没有 `Clone`，避免了未经设计的复制语义。

Go 的完整生产路径已接入 `pkg/server/conn.go`；Rust 当前可确认的是数据结构与解析测试，尚不能宣称连接认证迁移完成。

## 扩展指南

新增握手字段时，应先确认它在 MySQL 包中的出现条件和字节编码，再同步修改以下位置：

1. 在 `Response41` 增加语义明确、保持原始协议信息所需的字段类型；不要为了方便展示而提前把任意协议字节强转 UTF-8。
2. 在 `pkg/server/internal/parse/parse.rs` 的头部或主体解析函数中按协议顺序写入，并保持所有偏移访问有边界检查。
3. 同步 Go 对照语义，或明确记录 Rust 有意差异；若 Go 结构本身变化，也需核对 `pkg/server/conn.go` 的消费者。
4. 更新独立测试，而不是把测试嵌入本源文件。结构默认值和字段保持测试放在 `pkg/server/internal/handshake/migration_aster_unit_test.rs`；解析分支与畸形包测试放在 `pkg/server/internal/parse/migration_aster_unit_test.rs`；真实抓包兼容性放在 `pkg/server/internal/parse/handshake_test.rs`。

兼容性风险主要是 capability 与字段顺序不一致、整数宽度变化、错误地拒绝非 UTF-8 输入，以及把“字段缺失”和“空值”混为一谈。性能风险主要来自对网络包逐字段复制和属性集合的多次分配；若要减少复制，需要连同生命周期和异步连接缓冲区所有权整体设计，不能只把字段替换成借用切片。安全风险包括认证载荷日志泄漏、超大属性分配以及绕过解析器手工构造未经验证的对象。

## 验证依据

本说明基于以下直接证据：

- 目标源码：`pkg/server/internal/handshake/handshake.rs`，确认仅有 `Response41` 及八个公开字段。
- crate 边界：`pkg/server/internal/handshake/Cargo.toml`、`pkg/server/internal/handshake/lib.rs`、根 `Cargo.toml`、`pkg/server/Cargo.toml`。
- Rust 解析与依赖：`pkg/server/internal/parse/Cargo.toml`、`pkg/server/internal/parse/lib.rs`、`pkg/server/internal/parse/parse.rs`。
- Go 对照和生产流程：`pkg/server/internal/handshake/handshake.go`、`pkg/server/internal/parse/parse.go`、`pkg/server/conn.go`。
- 独立测试：`pkg/server/internal/handshake/migration_aster_unit_test.rs`、`pkg/server/internal/parse/migration_aster_unit_test.rs`、`pkg/server/internal/parse/handshake_test.rs`、`pkg/server/internal/parse/handshake_test.go`。
- RustCodeGraph：`status` 确认索引含 11,467 个文件；`files --filter pkg/server/internal/handshake` 确认目标、入口和迁移测试均已索引；`node --file .../handshake.rs` 核对完整源码；`query Response41 --json` 核对 Rust/Go 定义和测试；`callers/callees Response41` 未给出函数调用边，之后以精确 `rg` 引用搜索核对真实数据流。

任务要求的结构校验用于确认本文恰有十一个固定二级章节。按照纯文档任务约束未运行 Cargo；行为描述来自源码、图索引、Cargo 声明、Go 对照和独立测试的静态核验。
