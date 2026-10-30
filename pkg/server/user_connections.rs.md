# `pkg/server/user_connections.rs` 逻辑说明

## 文件定位

`increaseUserConnectionsCount` 在连接真正建立时再次读取限额，并在持锁区内检查和递增，避免并发连接都通过预检后突破限额。

## 核心职责

本文件围绕同一个不变量组织逻辑：共享注册表中的键是认证后账号字符串 `auth_user@auth_host`，值是该账号当前已计入的活动连接数。

- `calculateConnectionLimit` 选择有效限额：只要用户级限额大于零，就覆盖全局限额；否则采用全局值；结果为零表示不限。
- `checkUserConnectionCount` 在建连前匹配真实账号、读取限额并做预检，超限时记录日志。
- `increaseUserConnectionsCount` 在连接真正建立时再次读取限额，并在持锁区内检查和递增，避免并发连接都通过预检后突破限额。
- `decreaseUserConnectionCount` 在连接结束时释放计数，归零后删除键。
- `getUserConnectionCount` 提供持锁只读查询。

它不负责网络监听、握手、权限数据存储或连接关闭调度；这些能力由调用方及 `UserConnectionRuntime` 的实现提供。生产连接通过 server.rs 共享注册表并由会话 worker 提供账户资源快照。

## 主要符号

- `UserIdentity`：公开身份值对象，包含登录时的 `username`/`hostname` 和权限系统匹配后的 `auth_username`/`auth_hostname`。派生 `Clone`、`Debug`、`Eq`、`Hash`、`PartialEq`。
- `UserIdentity::String`：生成计数键。认证用户名非空时采用认证身份，否则回退到登录身份；方法名保留 Go 风格以便对照 `auth.UserIdentity.String`。
- `UserIdentity::LoginString`：始终生成登录身份 `user@host`，用于预检超限日志和错误参数。
- `ConnectionError`：公开错误枚举，含运行时/资源错误 `Resource(String)`、超限 `TooManyUserConnections(String)`、互斥锁中毒 `LockPoisoned`；实现 `Display` 和标准 `Error`。
- `UserConnectionRuntime`：公开、要求 `Send + Sync` 的依赖抽象。`global_limit`、`user_limit`、`match_identity`、`log_limit_exceeded` 分别隔离全局配置、权限资源查询、账号匹配和日志副作用。
- `UserConnectionRegistry`：公开、可克隆的共享注册表；内部为 `Arc<Mutex<HashMap<String, usize>>>`，字段私有以强制通过本模块操作计数。
- `ClientConn`：本模块所需的最小公开连接上下文，持有 `UserIdentity`、`Arc<dyn UserConnectionRuntime>` 和 `UserConnectionRegistry`。它不是 `pkg/server/conn.rs` 的完整连接类型。
- `calculateConnectionLimit`、`increaseUserConnectionsCount`、`getUserConnectionCount`、`checkUserConnectionCount` 为公开自由函数；decreaseUserConnectionCount 是 registry 的显式账户键方法。ReleaseUserConnection 是 FnOnce 类型别名，UserConnectionGuard 通过 Drop 绑定清理。

## 执行流程

建连流程分为预检和提交两阶段：

1. 调用方将客户端提交的身份和连接来源主机交给 `checkUserConnectionCount`。
2. `UserConnectionRuntime::match_identity` 返回权限系统实际匹配的账号；随后用匹配结果查询用户级上限，并与 `global_limit` 合成有效限额。
3. 有效限额为零时立即放行；否则 `getUserConnectionCount` 按匹配身份的 `String` 键读取当前值。
4. 已达到限额时，以匹配身份的 `LoginString` 调用 `log_limit_exceeded`，然后返回 `TooManyUserConnections`；未达到则只表示预检通过，不修改计数。
5. 连接真正建立时调用 `increaseUserConnectionsCount`。该函数按 `cc.user.String()` 取计数键，重新读取限额，在注册表锁内再次判断并递增。这个第二次检查才是并发下防止越界的提交点。
6. 仅成功领取才返回捕获原账户的 FnOnce；实际连接安装 UserConnectionGuard，退出时调用 registry.decreaseUserConnectionCount(target_user)，归零删除条目。

Go 生产主链把步骤 1 放在 `pkg/server/conn.go` 的认证阶段，把步骤 5 和 6 放在 `pkg/server/server.go` 的连接运行生命周期中。Rust 的实际 MySQL 服务在握手后通过同一 registry 完成领取与guard清理；原认证阶段预检接口保持，不在本来源补建完整认证子系统。

## 数据与状态

共享状态只有 `UserConnectionRegistry::counts`。克隆 registry 只会克隆 `Arc`，所以多个 `ClientConn` 可以共享同一张计数表；克隆或重新创建 `ClientConn` 本身不会自动增减计数。`pkg/server/user_connections_test.rs` 的 `authenticated_identity_shares_count_across_client_hosts` 验证两个客户端主机只要映射到同一 `alice@%`，就累加到同一键。

计数使用 `usize`，配置限额使用 `u32`，比较时将限额转换为 `usize`。键是新分配的 `String`：递增路径进入 `HashMap::entry` 前克隆一次键；查询由 UserIdentity::String 构造键；递减使用领取时已捕获的键。条目仅在首次递增时创建，并在最后一次递减后删除，因而表的稳定规模接近当前有计数的认证账号数，而非历史账号总数。

身份字段存在明确语义差异：递增直接使用 `cc.user` 的认证字段查询限额和构造键；预检先调用 `match_identity`。生产接线必须保证递增前 `cc.user` 已写入同一个认证身份，否则预检与实际计数可能使用不同键。

## 依赖与调用关系

直接 Rust 标准库依赖为 `HashMap`、`fmt`、`Arc` 和 `Mutex`；本文件不直接使用 `pkg/server/Cargo.toml` 中的其他 workspace crate 或第三方 crate。权限系统、全局变量和日志被抽象到 `UserConnectionRuntime`，所以测试无需启动服务器或存储。

已验证的 Rust 调用关系如下：

- `pkg/server/lib.rs` → `pub mod user_connections`：crate 公开模块装配。
- `pkg/server/user_connections_test.rs` → 本文件所有核心函数和类型：独立测试使用者；生产 registry 调用见本次来源对齐章节。
- `checkUserConnectionCount` → `match_identity`、`user_limit`、`calculateConnectionLimit`、`getUserConnectionCount`、`LoginString`、`log_limit_exceeded`。
- `increaseUserConnectionsCount` → `UserIdentity::String`、`user_limit`、`global_limit`、`calculateConnectionLimit`、`Mutex::lock` 和 `HashMap::entry`。
- `UserConnectionRegistry::decreaseUserConnectionCount` 使用显式账户键与注册表锁；getUserConnectionCount 使用 UserIdentity::String 查询。

Go 对照主链为 `pkg/server/conn.go:checkUserConnectionCount` → `pkg/server/user_connections.go`，以及 `pkg/server/server.go:increaseUserConnectionsCount` → 连接运行 → 延迟执行 `decreaseUserConnectionCount`。实际 MySQL 接线现位于 server.rs::serve_mysql_connection，并用 guard 绑定原账户释放。

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

本模块提供 UserConnectionGuard，对应 Go defer：仅成功取得 ReleaseUserConnection 才安装守卫，正常退出与unwind均调用一次捕获原账户的FnOnce。仅丢弃ClientConn或registry克隆不会自动清理计数。

## 与 Go 版本的对应关系

实现形态存在以下差异：

- Go 在 `Server` 上持有 `userResource map` 和 `userResLock`，方法绑定完整 `clientConn`；Rust 抽出可克隆的 `UserConnectionRegistry` 和最小 `ClientConn`，由server.rs持有并与生产连接运行生命周期集成。
- Go 从 `vardef.MaxUserConnectionsValue`、privilege manager、session context 和后台日志器取依赖；Rust 用 `UserConnectionRuntime` trait 注入这些行为，独立测试实现该trait，生产连接通过ConcreteTiDBContext直接读取同一会话的身份与现有权限资源，再复用registry领取。
- Go 使用 `sync.RWMutex`，但读函数也调用 `Lock`；Rust 使用普通 `Mutex`，实际语义同样是所有访问互斥。
- Go 的递减不返回错误；Rust 必须表达互斥锁中毒，因此返回 `Result`。
- Go 返回带 MySQL server 错误码/消息的 `servererr.ErrTooManyUserConnections`；Rust 当前是轻量 `ConnectionError`，尚未证明已映射到协议层错误码。
- Rust `increaseUserConnectionsCount` 在首次插入前也执行统一的上限判断；Go 首次键不存在时直接插入 1。由于有效正限额最小为 1，两者可观察结果一致。

## 扩展指南

实际服务器已持有共享registry；扩展认证时须保持认证账户快照来自同一会话worker，且仅成功递增后安装guard。不得在COM_CHANGE_USER时重领或按新用户释放原配额。

修改限额选择规则应集中在 `calculateConnectionLimit`，并同步 `pkg/server/user_connections_test.rs` 的全局/用户级组合测试和 Go 对照 `pkg/server/user_connections_test.go` 的意图。修改身份键规则应同时审查 `UserIdentity::String`、`checkUserConnectionCount` 与 `increaseUserConnectionsCount`，覆盖登录主机不同但认证账号相同、认证字段为空回退、通配认证主机等场景。

若新增协议级错误兼容，不能只改 `Display` 文本；应在调用边验证 MySQL 错误码 1203 及客户端可见消息，并保持 `Resource`、身份匹配和锁错误的传播区别。若为性能分片计数表，必须保证单个认证键的检查加递增仍原子，并验证递减删除不会与并发递增丢失更新。若引入 RAII 守卫，应测试正常断开、认证后启动失败、运行循环报错和重复关闭，避免漏减或双减。

相关 Rust 测试应继续放在独立文件 `pkg/server/user_connections_test.rs`，不应内嵌到生产源文件。服务器接线由 server_test.rs 的真实监听器配额回归验证；模块测试另覆盖换用户与 unwind。


## 来源 592082dee7299afe8f838cd3d45c7fe0a88c6aca 的局部对齐

`increaseUserConnectionsCount` 成功后返回 `ReleaseUserConnection`（拥有 registry 克隆与原账户键的 FnOnce）；失败不生成释放函数。`UserConnectionRegistry::acquire` 持锁检查并递增，释放使用捕获键，不读取已被 COM_CHANGE_USER 替换的会话用户。`UserConnectionGuard` 仅由成功领取构造，析构调用一次释放，覆盖正常退出及 unwind。实际 `server.rs::serve_mysql_connection` 共用该 registry，身份/用户限额来自 ConcreteSession 的既有权限缓存；用户上限优先于全局。decreaseUserConnectionCount 现在接收 registry 与明确账户键；实际连接仅调用捕获键的释放函数，旧测试也改用领取句柄。
