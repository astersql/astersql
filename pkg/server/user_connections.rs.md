# `pkg/server/user_connections.rs` 逻辑说明

## 文件定位

`pkg/server/user_connections.rs` 属于 `astersql-server` crate。`pkg/server/Cargo.toml` 以 `lib.rs` 为 crate 根，`pkg/server/lib.rs` 通过公开模块 `user_connections` 暴露本文件，并仅在 `cfg(test)` 下挂载独立测试 `pkg/server/user_connections_test.rs`。

本文件移植 Go 的 `pkg/server/user_connections.go`，提供按认证账号统计和限制 MySQL 客户端连接数所需的数据结构与算法。当前 Rust 仓库中的生产代码没有调用本文件的公开函数；RustCodeGraph 只识别到 `pkg/server/user_connections_test.rs` 使用它。因此它是“已公开、已单测，但尚未接入 Rust 建连主链”的迁移模块。完整应用中的预期位置可由 Go 主链直接核对：`pkg/server/conn.go` 的鉴权流程调用 `checkUserConnectionCount`，`pkg/server/server.go` 在运行连接前调用 `increaseUserConnectionsCount`，并以 `defer` 保证退出时调用 `decreaseUserConnectionCount`。这些 Go 调用边是移植语义的参照，不是 Rust 已接线的证明。

## 核心职责

本文件围绕同一个不变量组织逻辑：共享注册表中的键是认证后账号字符串 `auth_user@auth_host`，值是该账号当前已计入的活动连接数。

- `calculateConnectionLimit` 选择有效限额：只要用户级限额大于零，就覆盖全局限额；否则采用全局值；结果为零表示不限。
- `checkUserConnectionCount` 在建连前匹配真实账号、读取限额并做预检，超限时记录日志。
- `increaseUserConnectionsCount` 在连接真正建立时再次读取限额，并在持锁区内检查和递增，避免并发连接都通过预检后突破限额。
- `decreaseUserConnectionCount` 在连接结束时释放计数，归零后删除键。
- `getUserConnectionCount` 提供持锁只读查询。

它不负责网络监听、握手、权限数据存储或连接关闭调度；这些能力由调用方及 `UserConnectionRuntime` 的实现提供。当前文件也没有生产级 runtime 适配器。

## 主要符号

- `UserIdentity`：公开身份值对象，包含登录时的 `username`/`hostname` 和权限系统匹配后的 `auth_username`/`auth_hostname`。派生 `Clone`、`Debug`、`Eq`、`Hash`、`PartialEq`。
- `UserIdentity::String`：生成计数键。认证用户名非空时采用认证身份，否则回退到登录身份；方法名保留 Go 风格以便对照 `auth.UserIdentity.String`。
- `UserIdentity::LoginString`：始终生成登录身份 `user@host`，用于预检超限日志和错误参数。
- `ConnectionError`：公开错误枚举，含运行时/资源错误 `Resource(String)`、超限 `TooManyUserConnections(String)`、互斥锁中毒 `LockPoisoned`；实现 `Display` 和标准 `Error`。
- `UserConnectionRuntime`：公开、要求 `Send + Sync` 的依赖抽象。`global_limit`、`user_limit`、`match_identity`、`log_limit_exceeded` 分别隔离全局配置、权限资源查询、账号匹配和日志副作用。
- `UserConnectionRegistry`：公开、可克隆的共享注册表；内部为 `Arc<Mutex<HashMap<String, usize>>>`，字段私有以强制通过本模块操作计数。
- `ClientConn`：本模块所需的最小公开连接上下文，持有 `UserIdentity`、`Arc<dyn UserConnectionRuntime>` 和 `UserConnectionRegistry`。它不是 `pkg/server/conn.rs` 的完整连接类型。
- `calculateConnectionLimit`、`increaseUserConnectionsCount`、`decreaseUserConnectionCount`、`getUserConnectionCount`、`checkUserConnectionCount`：均为公开自由函数；本文件无模块级常量、宏、条件编译项或其他 `impl`。

## 执行流程

建连流程分为预检和提交两阶段：

1. 调用方将客户端提交的身份和连接来源主机交给 `checkUserConnectionCount`。
2. `UserConnectionRuntime::match_identity` 返回权限系统实际匹配的账号；随后用匹配结果查询用户级上限，并与 `global_limit` 合成有效限额。
3. 有效限额为零时立即放行；否则 `getUserConnectionCount` 按匹配身份的 `String` 键读取当前值。
4. 已达到限额时，以匹配身份的 `LoginString` 调用 `log_limit_exceeded`，然后返回 `TooManyUserConnections`；未达到则只表示预检通过，不修改计数。
5. 连接真正建立时调用 `increaseUserConnectionsCount`。该函数按 `cc.user.String()` 取计数键，重新读取限额，在注册表锁内再次判断并递增。这个第二次检查才是并发下防止越界的提交点。
6. 连接结束时调用 `decreaseUserConnectionCount`；存在键时饱和减一，归零后删除条目。

Go 生产主链把步骤 1 放在 `pkg/server/conn.go` 的认证阶段，把步骤 5 和 6 放在 `pkg/server/server.go` 的连接运行生命周期中。Rust 目前没有对应调用边，扩展时不能只接预检而漏掉提交和清理。

## 数据与状态

共享状态只有 `UserConnectionRegistry::counts`。克隆 registry 只会克隆 `Arc`，所以多个 `ClientConn` 可以共享同一张计数表；克隆或重新创建 `ClientConn` 本身不会自动增减计数。`pkg/server/user_connections_test.rs` 的 `authenticated_identity_shares_count_across_client_hosts` 验证两个客户端主机只要映射到同一 `alice@%`，就累加到同一键。

计数使用 `usize`，配置限额使用 `u32`，比较时将限额转换为 `usize`。键是新分配的 `String`：递增路径进入 `HashMap::entry` 前克隆一次键；查询和递减也会由 `UserIdentity::String` 构造键。条目仅在首次递增时创建，并在最后一次递减后删除，因而表的稳定规模接近当前有计数的认证账号数，而非历史账号总数。

身份字段存在明确语义差异：递增直接使用 `cc.user` 的认证字段查询限额和构造键；预检先调用 `match_identity`。生产接线必须保证递增前 `cc.user` 已写入同一个认证身份，否则预检与实际计数可能使用不同键。

## 依赖与调用关系

直接 Rust 标准库依赖为 `HashMap`、`fmt`、`Arc` 和 `Mutex`；本文件不直接使用 `pkg/server/Cargo.toml` 中的其他 workspace crate 或第三方 crate。权限系统、全局变量和日志被抽象到 `UserConnectionRuntime`，所以测试无需启动服务器或存储。

已验证的 Rust 调用关系如下：

- `pkg/server/lib.rs` → `pub mod user_connections`：crate 公开模块装配。
- `pkg/server/user_connections_test.rs` → 本文件所有核心函数和类型：当前唯一已索引使用者。
- `checkUserConnectionCount` → `match_identity`、`user_limit`、`calculateConnectionLimit`、`getUserConnectionCount`、`LoginString`、`log_limit_exceeded`。
- `increaseUserConnectionsCount` → `UserIdentity::String`、`user_limit`、`global_limit`、`calculateConnectionLimit`、`Mutex::lock` 和 `HashMap::entry`。
- `decreaseUserConnectionCount`/`getUserConnectionCount` → `UserIdentity::String` 与同一注册表锁。

Go 对照主链为 `pkg/server/conn.go:checkUserConnectionCount` → `pkg/server/user_connections.go`，以及 `pkg/server/server.go:increaseUserConnectionsCount` → 连接运行 → 延迟执行 `decreaseUserConnectionCount`。仓库级 Rust 搜索未找到测试外的对应调用者，这一缺口必须在描述架构或规划扩展时保留。

## 错误处理与边界

- `global_limit == 0 && user_limit == 0` 表示不限；测试 `zero_limits_are_unlimited` 连续递增 100 次证明该路径仍会计数，只是不拒绝连接。
- 用户级限额只要大于零就拥有绝对优先级，即使它高于全局限额也不取较小值；`global_limit_and_higher_per_user_override_match_go` 覆盖了这一兼容约定。
- 达到限额的判定是 `current >= limit`，所以允许的最大成功计数恰为 `limit`。
- `user_limit` 和 `match_identity` 的错误原样传播。`ConnectionError::Resource` 是 runtime 实现可用的承载形式，但本文件不主动构造它。
- 任意一次 `Mutex::lock` 遇到锁中毒都会返回 `ConnectionError::LockPoisoned`；递减也可能失败，因此生产调用方不能假设清理永远成功。
- 对不存在的键执行递减是成功的空操作；`saturating_sub` 避免下溢，值归零即删除。
- 预检超限会记录日志，递增阶段发现超限则只返回错误，不调用 `log_limit_exceeded`，与 Go 中两个阶段的日志位置一致。
- `TooManyUserConnections` 的字符串在预检路径来自 `LoginString`，递增路径来自认证计数键；二者在客户端主机与认证主机模式不同（例如 `client` 与 `%`）时可能不同。Rust 测试分别验证了 `alice@client` 日志和 `alice@%` 递增错误。

## 并发与资源生命周期

`UserConnectionRuntime: Send + Sync`、`Arc<dyn ...>` 和 `Arc<Mutex<...>>` 允许连接跨线程共享依赖和注册表。所有映射读写都由同一互斥锁串行化；特别是递增将“读取当前值、检查限额、加一”放在一次持锁区内，从而使计数提交相对于其他递增是原子的。预检读取与后续递增不是一个原子事务，但递增会重新查询限额并再次检查，避免 TOCTOU 导致超额。

锁只覆盖内存映射访问。`user_limit`、`global_limit`、身份匹配和超限日志都在锁外执行，避免把外部权限查询或日志 I/O 带入临界区。代价是所有用户共享一把全局锁，高连接并发下不同账号也会竞争；当前实现没有分片或读写锁优化。

本模块不提供 RAII 连接守卫，生命周期配对依赖外层代码：成功递增一次必须最终递减一次。Go 的 `pkg/server/server.go` 用 `defer` 建立该保证；未来 Rust 接线应使用作用域守卫或等价的所有退出路径清理机制，并明确处理递增后、正式注册前失败的路径。仅丢弃 `ClientConn` 或 registry 克隆不会自动清理计数。

## 与 Go 版本的对应关系

Rust 函数逐一对应 `pkg/server/user_connections.go` 中的同名函数，限额优先级、零值语义、认证身份计数键、达到上限时拒绝、归零删除等核心行为保持一致。`pkg/server/user_connections_test.rs` 还将 Go `TestUserConnectionCount` 的五类限额组合和跨客户端主机共享认证账号计数的意图拆成多个独立测试。

实现形态存在以下差异：

- Go 在 `Server` 上持有 `userResource map` 和 `userResLock`，方法绑定完整 `clientConn`；Rust 抽出可克隆的 `UserConnectionRegistry` 和最小 `ClientConn`，未与 Rust 生产连接类型集成。
- Go 从 `vardef.MaxUserConnectionsValue`、privilege manager、session context 和后台日志器取依赖；Rust 用 `UserConnectionRuntime` trait 注入这些行为，当前仅测试 runtime 有实现。
- Go 使用 `sync.RWMutex`，但读函数也调用 `Lock`；Rust 使用普通 `Mutex`，实际语义同样是所有访问互斥。
- Go 的递减不返回错误；Rust 必须表达互斥锁中毒，因此返回 `Result`。
- Go 返回带 MySQL server 错误码/消息的 `servererr.ErrTooManyUserConnections`；Rust 当前是轻量 `ConnectionError`，尚未证明已映射到协议层错误码。
- Rust `increaseUserConnectionsCount` 在首次插入前也执行统一的上限判断；Go 首次键不存在时直接插入 1。由于有效正限额最小为 1，两者可观察结果一致。

因此，本文只能确认算法层移植和单元测试对应，不能宣称 Rust 已具备 Go 生产链中的权限管理器适配、协议错误映射或自动生命周期接线。

## 扩展指南

接入 Rust 生产服务器时，最可能修改或实现的扩展点是 `UserConnectionRuntime` 的生产适配器、生产连接上下文到本模块 `ClientConn` 的桥接，以及服务器级共享 `UserConnectionRegistry`。接线顺序应保持 Go 约束：认证身份确定后调用 `checkUserConnectionCount`；开始服务连接前调用 `increaseUserConnectionsCount`；只有递增成功才安装必达的递减清理。

修改限额选择规则应集中在 `calculateConnectionLimit`，并同步 `pkg/server/user_connections_test.rs` 的全局/用户级组合测试和 Go 对照 `pkg/server/user_connections_test.go` 的意图。修改身份键规则应同时审查 `UserIdentity::String`、`checkUserConnectionCount` 与 `increaseUserConnectionsCount`，覆盖登录主机不同但认证账号相同、认证字段为空回退、通配认证主机等场景。

若新增协议级错误兼容，不能只改 `Display` 文本；应在调用边验证 MySQL 错误码 1203 及客户端可见消息，并保持 `Resource`、身份匹配和锁错误的传播区别。若为性能分片计数表，必须保证单个认证键的检查加递增仍原子，并验证递减删除不会与并发递增丢失更新。若引入 RAII 守卫，应测试正常断开、认证后启动失败、运行循环报错和重复关闭，避免漏减或双减。

相关 Rust 测试应继续放在独立文件 `pkg/server/user_connections_test.rs`，不应内嵌到生产源文件。生产接线后还需在服务器连接层增加集成测试，因为当前单元测试无法证明实际 runtime、共享 registry 和所有退出路径已正确组合。

## 验证依据

事实依据包括：目标实现 `pkg/server/user_connections.rs`；crate 声明 `pkg/server/Cargo.toml`；模块入口 `pkg/server/lib.rs`；独立 Rust 测试 `pkg/server/user_connections_test.rs`；Go 对照实现与测试 `pkg/server/user_connections.go`、`pkg/server/user_connections_test.go`；Go 生产调用点 `pkg/server/conn.go` 和 `pkg/server/server.go`。

RustCodeGraph `status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边。`node --file pkg/server/user_connections.rs` 返回完整 188 行源码，并报告该文件仅被 `pkg/server/user_connections_test.rs` 使用；对五个公开函数的 `query` 均找到了 Rust/Go 同名定义。`callers`/`callees` 未返回额外 Rust 生产调用边，随后仓库级 Rust 搜索也确认测试外没有这些符号、`UserConnectionRegistry` 或 `UserConnectionRuntime` 的引用。

人工核对的测试证据为：用户限额覆盖全局且递减释放槽位；预检超限记录匹配后的登录串；双零限额不限流；认证身份为空时回退登录身份；仅全局限额及高于全局的用户限额；不同客户端主机共享同一认证账号计数。本文没有运行 Cargo，符合纯文档任务约束；验证范围是静态代码、索引调用边、Go 对照和既有测试意图，不包含运行时执行证明。
