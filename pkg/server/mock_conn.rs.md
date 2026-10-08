# `pkg/server/mock_conn.rs`

## 文件定位

`pkg/server/mock_conn.rs` 是 `astersql-server` crate 中公开的测试辅助模块。crate 根文件 `pkg/server/lib.rs` 通过 `pub mod mock_conn` 暴露它，同时只在 `cfg(test)` 下挂载同目录的独立测试 `pkg/server/mock_conn_test.rs`。`pkg/server/Cargo.toml` 指定 crate 名为 `astersql-server`、库入口为 `lib.rs`，并以 `package.metadata.porting.go-package = "pkg/server"` 标明 Go 对照包。

该文件位于服务端边界，但不承担真实 TCP 监听、MySQL 包解析或生产会话实现。它用 `MockDriver`/`MockSession` 两层 trait 隔离真实驱动和会话，再用 `mockConn` 提供可观察的输出缓冲、连接登记与关闭行为，供 server 单元测试和跨模块兼容测试构造轻量连接。当前源码没有模块级业务常量、条件编译项或异步运行时接线；唯一的函数内静态量是 `CreateMockConn` 中的 `NEXT_CONNECTION_ID`。

## 核心职责

1. 定义测试会话边界：`MockSession` 抽象 SQL 查询、原始协议数据分发、关闭和 root 认证；`MockDriver` 按连接 ID 与 collation 创建会话。
2. 维护最小服务器状态：`Server` 持有驱动和 `connection_id -> session` 的活跃会话表，`CreateMockServer` 只负责创建这一容器。
3. 提供连接测试门面：`MockConn` 及其实现 `mockConn` 将查询/分发结果写入内部缓冲，暴露会话、连接 ID、关闭操作和“取走并轮换输出缓冲”的能力。
4. 模拟连接生命周期：`CreateMockConn` 分配 ID、打开会话、执行 root 认证并登记会话；`Close` 调用会话关闭后尽力从服务器表注销。
5. 提供 Auth Socket 测试桩：`MockOSUserForAuthSocket`、`ClearOSUserForAuthSocket` 与 `AuthSocketUserMatches` 管理并检查一个进程级模拟 OS 用户名。

这些职责都服务于测试可控性。RustCodeGraph 显示 Rust `CreateMockConn` 的已索引调用者为 `pkg/server/mock_conn_test.rs` 的单连接测试和 `pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs` 的批量资源释放测试；没有证据表明该轻量连接处于生产监听请求主链。

## 主要符号

- `Error(pub String)`：公开的轻量错误类型，实现 `Display` 和 `std::error::Error`。本文件用消息区分外部 trait 错误与多类锁中毒错误；它不保存错误码或底层 source 链。
- `MockSession: Send + Sync`：公开会话契约。`handle_query(&str, &mut Vec<u8>)` 和 `dispatch(&[u8], &mut Vec<u8>)` 由实现方把响应追加到调用者提供的缓冲；`close` 和 `authenticate_root` 返回相同的 `Result<(), Error>`。
- `MockDriver: Send + Sync`：公开驱动契约。`open(connection_id, collation)` 返回共享的动态 `MockSession`。
- `Server`：公开结构体，公开 `driver`，私有 `clients: Mutex<HashMap<u64, Arc<dyn MockSession>>>`。`Server::new` 建立空会话表。
- `MockConn`：公开连接 trait，方法名保留 Go 风格：`HandleQuery`、`Context`、`Dispatch`、`Close`、`ID`、`GetOutput`。返回 `Box<dyn MockConn>` 的工厂使调用者不依赖私有实现类型。
- `mockConn`：私有实现，保存 `connection_id`、强引用 `session`、对 `Server` 的 `Weak` 引用，以及双层同步的输出端点 `Mutex<Arc<Mutex<Vec<u8>>>>`。
- `CreateMockServer`：公开工厂，仅包装 `Server::new`；调用者必须自行提供 `MockDriver`，这里不会创建真实存储、domain 或监听器。
- `CreateMockConn`：公开连接工厂。使用全局 `AtomicU64` 从 1 开始分配 ID，以 collation `45` 打开会话，认证成功后登记到 `Server.clients`，最终返回 `Box<dyn MockConn>`。
- `mock_os_user`：私有惰性初始化器，借助 `OnceLock<Mutex<Option<String>>>` 创建进程级 Auth Socket 状态。
- `MockOSUserForAuthSocket` / `ClearOSUserForAuthSocket`：公开设置和清除函数，锁中毒时返回 `Error`。
- `AuthSocketUserMatches`：公开检查函数。非 Unix socket、未设置 OS 用户、或用户名/非空映射名均不匹配时返回 `false`；OS 用户等于 MySQL 用户，或等于非空 `auth_string` 时返回 `true`。

## 执行流程

创建连接的主流程由 `CreateMockConn` 驱动：

1. `NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed)` 取得本进程内不重复的递增 ID；该顺序只要求原子唯一性，不表达跨线程先后关系。
2. 调用 `server.driver.open(connection_id, 45)`。驱动错误立即用 `?` 返回，尚未产生会话登记。
3. 调用 `session.authenticate_root()`。认证失败同样立即返回，并且不会把会话插入 `clients`。
4. 锁定 `Server.clients` 并插入 `session.clone()`；锁中毒时返回 `server client lock poisoned`。
5. 构造 `mockConn`：连接自身持有会话强引用、服务器弱引用和一个空输出缓冲。

查询路径 `mockConn::HandleQuery` 先锁定输出“端点”，克隆当前 `Arc<Mutex<Vec<u8>>>`，再锁定具体字节缓冲，把 `&mut Vec<u8>` 交给 `MockSession::handle_query`。`Dispatch` 采用完全相同的锁序，只把输入改为原始协议字节并调用 `MockSession::dispatch`。因此协议或 SQL 的具体语义完全由注入的会话实现决定，本文件只编排缓冲与错误传播。

读取输出时，`GetOutput` 在端点锁内用一个新的空 `Arc<Mutex<Vec<u8>>>` 替换当前缓冲，并返回旧缓冲。后续查询写入新缓冲，先前返回的 `Arc` 仍保留原响应。`pkg/server/mock_conn_test.rs::mock_connection_dispatches_and_rotates_output_between_reads` 验证旧查询结果不被后续 `Dispatch` 改写、两次返回不是同一个 `Arc`；批量测试还验证连续第二次 `GetOutput` 得到空缓冲。

关闭路径 `mockConn::Close` 先保存 `session.close()` 的结果，再尝试升级 `Weak<Server>`。若服务器仍存在且 `clients` 锁可用，就按 ID 删除登记；最后返回原始会话关闭结果。注销不依赖关闭成功，但服务器已释放或注销锁中毒时会静默跳过。

Auth Socket 路径独立于连接路径：设置/清除函数更新全局 `Option<String>`；`AuthSocketUserMatches` 先拒绝非 Unix socket，再读取快照并应用“OS 用户等于登录名或非空映射名”的判断。

## 数据与状态

- 连接 ID：`NEXT_CONNECTION_ID: AtomicU64` 是函数内静态状态，初值为 1。`Relaxed` 足以保证每次原子递增，但溢出行为未在本文件中专门处理。
- 活跃会话：`Server.clients` 以 ID 为键持有 `Arc<dyn MockSession>`。连接和服务器表各持有一份会话强引用；关闭时移除表项，连接对象被丢弃后最后一份强引用才会释放。`closing_many_connections_releases_every_server_session` 用 100 个会话的弱引用验证这一生命周期。
- 服务器所有权：`mockConn.server` 是 `Weak<Server>`，避免 `Server -> session` 与连接回指服务器形成所有权环，也允许服务器先于连接释放。
- 输出状态：外层 `Mutex` 保护“当前输出端点”的替换，内层 `Mutex` 保护字节向量内容。`Arc` 让测试可在连接轮换到新缓冲后继续持有和检查旧结果。
- Auth Socket 状态：`OnceLock<Mutex<Option<String>>>` 在进程内共享，首次访问后永久保留容器；清除只是把值设为 `None`。它不是线程局部或测试局部状态，因此并行测试必须负责恢复。

## 依赖与调用关系

本文件只直接依赖 Rust 标准库：`HashMap`、格式化 trait、`AtomicU64`、`Arc`/`Weak`、`Mutex` 和 `OnceLock`。`pkg/server/Cargo.toml` 没有为该模块声明专属 feature；模块随 `astersql-server` 库公开编译，只有 `mock_conn_test.rs` 的挂载受 `cfg(test)` 限制。

主要下游边如下：

- `CreateMockConn -> MockDriver::open -> MockSession::authenticate_root`，随后写入 `Server.clients`。
- `HandleQuery -> MockSession::handle_query`，`Dispatch -> MockSession::dispatch`。
- `Close -> MockSession::close`，随后尽力从 `Server.clients` 删除连接。
- `GetOutput -> std::mem::replace`，在保持旧缓冲可读的同时安装新缓冲。

RustCodeGraph 的明确上游证据包括：

- `pkg/server/mock_conn_test.rs::mock_connection_dispatches_and_rotates_output_between_reads` 调用 `CreateMockServer`、`CreateMockConn`、`HandleQuery`、`Dispatch`、`GetOutput` 和 `Close`。
- `pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs::closing_many_connections_releases_every_server_session` 批量调用连接工厂、查询、输出轮换和关闭。
- `pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs::auth_socket_os_user_override_is_always_recoverable` 直接覆盖 Auth Socket 匹配、替换和清除。

`AuthSocketUserMatches` 在索引和文本引用中只发现测试侧调用；不要据此声称它已接入 Rust 生产认证链。真实生产连接、协议状态机与监听生命周期位于 `pkg/server/conn.rs`、`pkg/server/runtime.rs` 和 `pkg/server/server.rs` 等模块，本文件没有调用这些实现。

## 错误处理与边界

- 驱动打开、root 认证、查询、分发和会话关闭的错误均原样向调用者传播；工厂不会把部分初始化的会话登记到服务器。
- 输出端点锁、输出字节锁、服务器会话表锁及 OS 用户锁中毒时，除两个特殊点外都转换为带固定消息的 `Error`。
- `GetOutput` 使用 `expect` 而不是返回 `Result`，所以端点锁中毒会 panic。这与 `HandleQuery`/`Dispatch` 的可恢复错误接口不同，扩展时应保留或有意识地调整这一 API 契约。
- `Close` 对 `Server` 已被释放和 `clients` 锁中毒采取尽力而为策略，不用注销失败覆盖 `MockSession::close` 的结果。这可能留下表项直到服务器释放，且调用者无法从返回值获知注销失败。
- `Close` 没有幂等标记：重复调用会重复调用 `MockSession::close`，而从 map 删除不存在的 ID 仍是无害的。具体会话是否允许重复关闭由 trait 实现决定。
- `HandleQuery` 与 `Dispatch` 是追加写入当前缓冲；只有 `GetOutput` 会切换为空缓冲。本文件不自动截断、解析或限制响应大小。
- `AuthSocketUserMatches` 明确拒绝 TCP 等非 Unix socket；空 `auth_string` 不能作为映射名匹配；未设置 mock OS 用户时返回 `false`，而不是错误。
- 当前测试驱动断言 collation 为 45，但源码注释将它描述为默认 collation 编号；若服务器默认值变化，这个硬编码及测试必须同步。

## 并发与资源生命周期

`MockSession` 和 `MockDriver` 要求 `Send + Sync`，服务器、连接和会话可以跨线程共享。连接 ID 用原子递增避免并发分配冲突；会话表、输出端点、实际输出字节和 Auth Socket 桩分别由不同的 `Mutex` 保护。

查询/分发的锁顺序固定为“端点锁 -> 克隆当前 `Arc` -> 释放端点锁 -> 缓冲锁”。由于外层临时守卫在语句末释放，执行 `MockSession` 回调期间只持有当前缓冲锁；并发 `GetOutput` 可以更换端点，而已经取得旧端点的操作仍可能继续向旧缓冲写入。因此 `GetOutput` 提供所有权/端点轮换，不提供与正在执行请求之间的全局屏障；测试若并发调用，需要自己约定何时响应已完成。

`Close` 先调用外部会话实现、再锁服务器表，没有同时持有这两类锁，降低锁顺序耦合。连接对服务器使用弱引用，服务器表对会话使用强引用；正常关闭删除表项后，连接释放即可销毁会话。`closing_many_connections_releases_every_server_session` 的 `Weak::upgrade().is_none()` 断言证明当前测试路径没有遗留强引用。

全局 OS 用户桩虽受互斥锁保护，不会产生数据竞争，却会造成测试间的语义干扰。所有设置它的测试都应在结束路径清除；若测试可能 panic，宜引入 RAII guard 或串行化策略，避免污染后续用例。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/server/mock_conn.go`。两边保留了 `MockConn`、`mockConn`、`CreateMockServer`、`CreateMockConn`、`MockOSUserForAuthSocket` 和 `ClearOSUserForAuthSocket` 的总体用途，以及查询、分发、上下文、关闭、ID 和输出轮换等接口意图，但 Rust 当前是聚焦测试所需行为的独立抽象，并非 Go 实现的逐字段包装。

关键差异如下：

- Go `mockConn` 嵌入真实 `*clientConn`，`HandleQuery`/`Dispatch` 直接走真实服务端方法；Rust `mockConn` 委托给注入的 `MockSession`，不会自行执行 SQL 或解析 MySQL 命令。
- Go `CreateMockServer` 从 `kv.Storage` 创建 `TiDBDriver`、配置真实 `Server` 并设置 domain；Rust 仅接收 `Arc<dyn MockDriver>` 并创建会话表。
- Go 使用随机 `uint64` 连接 ID，并用 `DefaultCollationID`；Rust 使用从 1 开始的进程级原子递增 ID 和硬编码 45。
- Go 工厂通过 `testing.T` 的断言终止失败路径，并初始化 allocator、PacketIO、extensions、session manager、connection info 和 root 身份；Rust 通过 `Result` 返回错误，只执行 `open`、`authenticate_root` 和 map 登记。
- Go `GetOutput` 替换真实 PacketIO 的 writer 并返回 `bytes.Buffer`；Rust 轮换 `Arc<Mutex<Vec<u8>>>`。两者都保证后续写入进入新缓冲，但底层协议栈覆盖度不同。
- Go `Close` 无返回值并用 `require.NoError` 检查真实连接关闭；Rust 返回会话关闭错误，并自行尽力删除会话表项。
- Go 的 OS 用户桩写入认证代码共享的原子指针；Rust 在本模块维护独立 `OnceLock<Mutex<Option<String>>>`，并额外提供 `AuthSocketUserMatches` 来直接验证匹配规则。现有调用证据只证明 Rust 测试使用该函数，不能推导它已替代生产 auth_socket 路径。

Go `pkg/server/tests/commontest/tidb_test.go::TestAuthSocket` 通过实际登录场景调用设置/清除函数；Rust 对照测试 `tidb_part4_aster_unit_test.rs::auth_socket_os_user_override_is_always_recoverable` 直接检查布尔匹配规则，覆盖层级更轻。

## 扩展指南

- 新增连接级测试行为时，优先扩充 `MockSession` 或 `MockConn` 的最小契约，并在 `pkg/server/mock_conn_test.rs` 增加独立测试；不要把测试写回生产 `.rs` 文件。若目标是验证真实协议栈，应改用 `pkg/server/conn.rs`/`runtime.rs` 的现有适配，而不是让本模块复制完整子系统。
- 修改创建流程时，应同时检查 `CreateMockConn` 的失败原子性：`open` 或认证失败不得登记客户端；登记成功后必须能由 `Close` 清理。相关回归可放在 `mock_conn_test.rs`，资源泄漏与大量连接行为同步检查 `tests/commontest/tidb_part3_aster_unit_test.rs`。
- 改动输出模型时，必须保留或明确改变三个不变量：查询/分发写入当前端点、`GetOutput` 返回旧端点、后续写入不污染旧响应。还需考虑并发请求与轮换之间没有完成屏障这一事实。
- 改动关闭语义时，决定重复关闭、会话关闭失败、服务器已释放及 map 锁中毒各自的可观察结果；特别注意当前实现会在会话关闭失败后仍尝试注销。
- 改动 collation 或连接 ID 策略时，同步 `pkg/server/mock_conn_test.rs::Driver::open` 和 `tidb_part3_aster_unit_test.rs::TrackingDriver::open` 的断言，并重新核对 Go `tmysql.DefaultCollationID` 的语义，避免只为测试硬编码新的魔数。
- 扩展 Auth Socket 模拟时，同步 `pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs` 和 Go `TestAuthSocket` 的意图；若要接入生产认证，必须另外验证真实认证模块调用边，不能仅修改此测试辅助函数。
- 为全局 OS 用户状态增加并行测试时，建议使用自动恢复的 guard 或测试串行化。仅在测试末手工清除无法覆盖 panic/提前返回路径。
- 对公开 API 的签名或 Go 风格方法名做重构会影响外部 crate 测试（它们以 `astersql_server::mock_conn::*` 引用），应先用 RustCodeGraph 查询 callers/impact，并检查 `pkg/server/lib.rs` 的公开模块边界。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph 文件读取：`node --file pkg/server/mock_conn.rs` 核对了完整 215 行源码；`node MockConn`、`node CreateMockConn` 与 `callees` 查询核对 trait、工厂及 `open`、`authenticate_root`、`handle_query`、`dispatch` 等下游边。
- RustCodeGraph 上游证据：`CreateMockConn` 的 Rust 调用者包括 `mock_connection_dispatches_and_rotates_output_between_reads` 与 `closing_many_connections_releases_every_server_session`；Auth Socket 查询定位到 Rust 的 `auth_socket_os_user_override_is_always_recoverable` 和 Go 的 `TestAuthSocket`。
- 已读取源码/边界：`pkg/server/mock_conn.rs`、`pkg/server/lib.rs`、`pkg/server/Cargo.toml`。
- 已读取 Go 对照：`pkg/server/mock_conn.go`；Go Auth Socket 测试调用边来自 `pkg/server/tests/commontest/tidb_test.go::TestAuthSocket`。
- 已读取 Rust 测试：`pkg/server/mock_conn_test.rs`、`pkg/server/tests/commontest/tidb_part3_aster_unit_test.rs` 的批量关闭测试、`pkg/server/tests/commontest/tidb_part4_aster_unit_test.rs` 的 Auth Socket 测试。
- 人工复核结论：文档区分了轻量测试抽象与真实服务端主链，逐项说明创建、写入、输出轮换、关闭、全局桩、错误和并发生命周期，并明确标注当前未发现生产 Auth Socket 接线的验证边界。
- 本任务是纯文档分析，按计划未运行 Cargo；结构命令已以退出码 0 验证目标文件存在且恰好包含规定的 11 个二级标题。
