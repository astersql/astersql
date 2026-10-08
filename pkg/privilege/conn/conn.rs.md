# `pkg/privilege/conn/conn.rs`

## 文件定位

本文件属于独立 workspace crate `astersql-privilege-conn`。`pkg/privilege/conn/Cargo.toml` 将 `lib.rs` 设为 crate 根，并用 `package.metadata.porting.go-package = "pkg/privilege/conn"` 标明它对照 Go 包；`pkg/privilege/conn/lib.rs` 声明 `conn` 模块并公开再导出本文件的符号。因此，本文件提供的是认证期间“插件与客户端继续交换协议包”的最小连接契约，而不是网络连接、MySQL 包编解码或权限判断的具体实现。

该契约位于会话/协议连接与认证实现之间：`pkg/privilege/privilege.rs` 的 `Manager::ConnectionVerification` 接受绑定了上下文和错误类型的 `dyn AuthConn`；`pkg/session/sessionapi/session.rs` 的 `Session::AuthConnection` 也要求实现本 trait；`pkg/extension/lib.rs` 则将其以 `RawAuthConn` 名义再导出，供 `pkg/extension/auth.rs::AuthConnAdapter` 接入扩展认证 API。

## 核心职责

- 以 `AuthConn` trait 抽象认证阶段需要的三项 I/O 能力：向客户端发送 AuthMoreData、读取客户端的下一个普通协议包、按调用方上下文刷新待发送数据（`conn.rs:25-46`）。
- 通过关联类型让实现方决定上下文和 I/O 错误的具体类型，同时保持上层认证流程不依赖具体网络连接类型（`AuthConn::Context`、`AuthConn::Error`）。
- 明确协议职责边界：`WriteAuthMoreData` 的调用方只提供负载，协议规定的 `0x01` 标记由具体连接实现封装；`ReadPacket` 返回普通包内容，不在 trait 内处理认证协议状态（`conn.rs:33-42`，并由 `conn.go:20-31` 的注释核对）。

本文件没有权限缓存、用户匹配、插件选择、超时/取消、重试或缓冲实现；这些都必须由使用方、认证流程或具体连接实现承担。

## 主要符号

- `pub trait AuthConn`（`conn.rs:25`）：公开连接能力接口。方法均使用 `&mut self`，表达一次调用需要对连接状态进行独占可变访问。trait 本身没有 `Send`、`Sync` 或生命周期约束；若上层需要跨线程使用，应像 `pkg/extension/auth.rs:37` 那样在自己的边界额外要求 `Send`。
- `type Context: ?Sized`（`conn.rs:28`）：`Flush` 的调用方上下文。`?Sized` 允许实现选择动态大小上下文，例如 trait object；该文件不读取上下文，只将其以共享引用交给实现。
- `type Error: std::error::Error`（`conn.rs:31`）：三个操作统一使用的实现侧错误类型。本 trait 不包装、记录或分类错误。
- `fn WriteAuthMoreData(&mut self, data: &[u8]) -> Result<(), Self::Error>`（`conn.rs:38`）：发送认证追加数据。输入是借用切片，方法返回后 trait 不承诺保留它；是否复制、缓冲以及何时真正写入均由实现决定。
- `fn ReadPacket(&mut self) -> Result<Vec<u8>, Self::Error>`（`conn.rs:42`）：读取并把下一普通协议包的所有权交给调用方。返回 `Vec<u8>`，因此调用方可以在后续认证轮次中独立持有或修改响应。
- `fn Flush(&mut self, ctx: &Self::Context) -> Result<(), Self::Error>`（`conn.rs:46`）：把已经挂起的输出刷向客户端。上下文按共享引用传入，连接仍以 `&mut self` 更新其 I/O 状态。

文件没有常量、结构体、枚举、默认方法、自由函数或条件编译项。`#[allow(non_snake_case)]` 只为保留 Go API 命名，未改变行为。

## 执行流程

本文件只定义操作顺序所需的原语，不强制状态机。典型多轮认证流程可由 Go LDAP SASL 实现 `pkg/privilege/privileges/ldap/sasl.go:32-94` 直接核对：

1. 认证实现计算一轮服务端凭据。
2. 调用 `WriteAuthMoreData(server_cred)`，由具体连接补上 AuthMoreData 的 `0x01` 协议标记并把包加入输出路径。
3. 调用 `Flush(ctx)`，确保本轮挑战已经发给客户端；写入成功不等同于已经刷新。
4. 若认证尚未完成，调用 `ReadPacket()` 获取客户端下一轮凭据，再重复上述步骤。
5. 任一步骤返回错误时，trait 本身不恢复、不重试，也不执行后续步骤；调用方决定终止认证或映射错误。

Rust 侧 `pkg/extension/auth.rs:66-90` 展示了另一条已接线路径：`AuthConnAdapter<T, C>` 持有一个实现本 trait 的底层连接与对应上下文，将写、读、刷新逐项转发，并把底层错误字符串映射成扩展 API 的 `ExtensionError`。它在 `Flush()` 时补入保存的 `C`，从而把扩展侧无上下文的方法适配到本文件的 `Flush(&Context)`。

## 数据与状态

本文件不拥有字段或全局状态，状态全部留在实现者中。接口仍隐含以下数据约束：

- `data: &[u8]` 是一段不带本 trait 所负责帧头的认证负载；是否在实现内复制到发送缓冲由实现决定。
- `ReadPacket` 每次返回一个拥有所有权的字节向量。Go 对照注释指出它读取的是无额外前缀的普通包，而且客户端最初的认证响应已经由更上层的 `Protocol::AuthSwitchResponse` 携带；该方法面向后续包（`conn.go:24-28`）。Rust trait 保留了“下一普通包”的边界，但没有定义首包来源类型。
- `Context` 只在刷新时出现，允许实现读取取消、日志或请求信息；本 trait 不要求上下文可变、可克隆或线程安全。
- 三个方法共享同一个 `Error` 类型，使一套具体连接的写、读、刷新错误可以沿同一 API 返回；没有内建的部分成功状态或错误清理协议。

独立测试 `pkg/privilege/conn/migration_aster_unit_test.rs` 的 `RecordingConn` 将这些状态落成写入负载列表、待读包队列和已刷新请求 ID，验证三项状态变化彼此可观察但由实现管理。

## 依赖与调用关系

本文件仅依赖标准库的 `std::error::Error`、字节切片、`Vec` 和 `Result`，其 `Cargo.toml` 没有声明第三方依赖或 feature。

向上游的已核对关系如下：

- `pkg/privilege/conn/lib.rs` 通过 `pub use conn::*` 暴露 `AuthConn`。
- `pkg/privilege/lib.rs:52-57` 和 `pkg/privilege/privilege.rs:29` 将其纳入 privilege 门面；后者在 `Manager::ConnectionVerification` 中以 `Context = sessionctx::ExecutionContext`、`Error = PrivilegeError` 的 trait object 使用。
- `pkg/session/sessionapi/lib.rs:21-24` 再导出该 trait，`pkg/session/sessionapi/session.rs:52` 用关联类型 `AuthConnection` 将会话认证入口绑定到它。
- `pkg/extension/lib.rs:38-41` 以 `RawAuthConn` 名义再导出；`pkg/extension/auth.rs::AuthConnAdapter` 是生产代码中已找到的泛型消费者和转发适配器。
- 根 `Cargo.toml` 以 `facade_privilege_conn` 收录此 crate；`pkg/privilege/Cargo.toml`、`pkg/session/sessionapi/Cargo.toml` 和 `pkg/extension/Cargo.toml` 分别以路径依赖接入。

RustCodeGraph 对目标文件识别出 5 个索引符号并显示文件被 privilege 迁移测试引用，但对这些 trait 方法没有返回可靠的静态 callers/callees；因此上述跨 crate 关系均进一步用精确符号引用核对。当前代码搜索没有找到非测试的具体 `impl`；`AuthConnAdapter` 消费任意实现者，而真正将 Rust 服务端网络连接实现为本 trait 的接线在当前仓库中未验证，不能把 Go `clientConn` 实现当成 Rust 已完成实现。

## 错误处理与边界

三个方法都直接返回 `Result<_, Self::Error>`。本文件不吞掉、重试、转换或记录错误，因此实现侧错误可以保持原类型上浮；`migration_aster_unit_test.rs::auth_conn_preserves_implementation_errors` 用 `io::ErrorKind::BrokenPipe` 和原始消息验证了这一点。扩展边界需要统一错误时，由 `AuthConnAdapter` 显式转换为 `ExtensionError`，这属于适配器行为而非本 trait 行为。

需要由实现或调用方处理的边界包括：空 AuthMoreData、空普通包、EOF、半包/超大包、客户端断开、刷新失败、上下文取消以及写入成功后刷新失败。本文件对这些情况没有特殊分支。测试假实现还用 `UnexpectedEof` 表示无可读包，这只是测试实现选择，不是 trait 规定的固定错误。

协议上最重要的边界是不重复加头：调用方传递纯 `data`，具体连接负责 `0x01`。Go 的 `pkg/server/conn.go:2933-2936` 通过构造四字节包头占位、再追加 `0x01` 和负载展示了实际做法；Rust 新实现应在其协议写包层对齐，而不应要求认证插件自行加入标记。

## 并发与资源生命周期

`AuthConn` 没有 `Send`/`Sync` 超 trait，因此它本身不承诺可跨线程移动或共享。三个方法均接收 `&mut self`，安全 Rust 中同一实现实例的这些 I/O 操作默认串行并独占其可变状态；若实现内部再引入并发、锁或异步任务，生命周期和同步责任属于实现者。

接口没有 `async`、连接关闭方法或 `Drop` 约定，也不拥有上下文。`data` 与 `ctx` 都只在调用期间借用，`ReadPacket` 的返回缓冲则转移给调用方。连接创建、套接字关闭、输出缓冲清理和认证失败后的资源回收不在本文件中。`AuthConnAdapter<T, C>` 会同时拥有底层连接与刷新上下文，并允许通过 `into_inner` 取回连接；这是扩展层的资源包装策略。

## 与 Go 版本的对应关系

Rust `AuthConn` 逐项对应 `pkg/privilege/conn/conn.go::AuthConn`：

- Go `WriteAuthMoreData(data []byte) error` 对应 Rust 借用切片并返回关联错误类型；两者都把 `0x01` 前缀交给具体连接。
- Go `ReadPacket() ([]byte, error)` 对应 Rust `Result<Vec<u8>, Error>`；Rust 用所有权明确返回缓冲归调用方。
- Go `Flush(ctx context.Context) error` 对应 Rust `Flush(&Self::Context)`；Rust 没有硬编码 Go context，而用关联类型承载宿主上下文。

命名通过 `#[allow(non_snake_case)]` 原样保留，以降低迁移对照成本。语义差异主要是 Rust 把同一实现的上下文/错误类型静态绑定，并以 `&mut self` 表达连接操作的可变独占访问。Go 生产实现 `pkg/server/conn.go:2930-2946` 证明 `clientConn` 负责加 AuthMoreData 标记、委托普通读包和刷新；它是协议行为的对照证据，不代表仓库中已经存在等价的 Rust `clientConn: AuthConn` 实现。

## 扩展指南

新增具体连接实现时，应在网络/协议连接所属模块实现 `AuthConn`，并重点保持以下契约：

1. 为 `Context` 选择实际刷新路径需要的上下文类型，为 `Error` 选择能覆盖读写刷新的统一错误类型。
2. `WriteAuthMoreData` 接收纯负载，在具体协议封装层恰好添加一次 `0x01` 标记；明确写入缓冲与真正刷新之间的界限。
3. `ReadPacket` 只返回下一普通包的负载，并正确处理 EOF、包长限制和底层读取错误；不要把首次 AuthSwitchResponse 重复读入此路径。
4. `Flush` 必须使用传入上下文，并保留底层失败；若需要取消或超时，应在具体上下文/实现层定义，而不是静默忽略。
5. 若上层需要跨线程，显式增加 `Send`/`Sync` 边界并审查内部连接状态；不要假设本 trait 已提供这些保证。

修改本 trait 的签名会影响 privilege `Manager`、session `Session::AuthConnection`、extension `AuthConnAdapter` 和所有实现者。测试逻辑应继续放在独立文件：直接契约测试同步更新 `pkg/privilege/conn/migration_aster_unit_test.rs`；扩展适配行为同步更新 `pkg/extension/auth_1_aster_unit_test.rs`；若新增真实 Rust 协议连接实现，还应在该实现同目录的独立测试文件覆盖精确帧字节、读包、刷新上下文、错误与断连场景。不要把测试内嵌进 `conn.rs`。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库；`files --filter pkg/privilege/conn` 找到 `conn.rs`、`lib.rs`、Go 对照和独立迁移测试；`node --file pkg/privilege/conn/conn.rs --offset 1 --limit 400` 核对完整 47 行源码；`query AuthConn`、`query WriteAuthMoreData`、`query ReadPacket` 及目标符号的 `callers`/`callees` 查询用于识别同名接口并确认目标 trait 没有可依赖的静态调用边。
- 源与 crate 边界：`pkg/privilege/conn/conn.rs`、`pkg/privilege/conn/lib.rs`、`pkg/privilege/conn/Cargo.toml`、根 `Cargo.toml`。
- Rust 入口与消费者：`pkg/privilege/lib.rs`、`pkg/privilege/privilege.rs`、`pkg/session/sessionapi/lib.rs`、`pkg/session/sessionapi/session.rs`、`pkg/extension/lib.rs`、`pkg/extension/auth.rs`，以及各自 Cargo 路径依赖。
- Go 对照：`pkg/privilege/conn/conn.go`（接口与首包说明）、`pkg/server/conn.go:2930-2946`（具体连接实现）、`pkg/privilege/privileges/ldap/sasl.go:32-94`（多轮认证顺序）、`pkg/privilege/privileges/privileges.go:251-262,632`（认证插件与连接校验入口）。
- 独立测试：`pkg/privilege/conn/migration_aster_unit_test.rs` 覆盖写/读/按上下文刷新及错误保真；`pkg/extension/auth_1_aster_unit_test.rs::auth_conn_adapter_uses_the_migrated_privilege_connection_contract` 覆盖扩展适配转发。`pkg/privilege/migration_aster_unit_test.rs` 仅证明 `Manager` 签名接入该 trait；LDAP 的 Rust 测试使用其局部同名 trait，不作为本文件实现证据。
- 未运行 Cargo 或代码测试：任务为纯文档分析，计划明确排除 Cargo；结构验证和 diff 自审作为交付验证。
