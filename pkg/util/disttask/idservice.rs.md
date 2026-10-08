# `pkg/util/disttask/idservice.rs`

## 文件定位

本文件属于 `astersql-util-disttask` crate（`pkg/util/disttask/Cargo.toml`），提供分布式任务执行节点 ID 的格式化、匹配和 InfoSync 注册表解析能力。crate 根 `pkg/util/disttask/lib.rs` 通过 `mod idservice; pub use idservice::*;` 将五个公开函数重新导出，并仅在 `cfg(test)` 下装配独立测试文件 `idservice_test.rs`。

执行器 ID 是节点 SQL 地址的文本形式：IPv4、主机名或空主机使用 `host:port`，含冒号的地址使用 `[host]:port`。Go 侧把该值用于把子任务与 TiDB 执行节点关联；可见调用点包括 `pkg/dxf/framework/scheduler/scheduler.go`、`pkg/dxf/framework/handle/status.go`、`pkg/dxf/importinto/scheduler.go`、`pkg/ddl/schemaver/syncer.go` 和 `pkg/executor/set.go`。当前仓库搜索未发现这些 API 的生产 Rust 调用点；因此 Rust 版本应描述为已导出且有独立单元测试的移植实现，而不能声称已经接入完整 Rust 调度主链。

## 核心职责

- `GenerateExecID` 将 `infosync::ServerInfo` 的 `IP` 与 `Port` 规范化为稳定的执行器 ID，格式与 Go `net.JoinHostPort` 在现有测试覆盖范围内一致。
- `FindServerInfo` 按输入切片顺序查找第一个 ID 完全相等的节点并返回下标；`MatchServerInfo` 在此基础上提供布尔判定。
- `GenerateSubtaskExecID` 从生产 InfoSync 注册表按节点 ID 取 `ServerInfo`，再生成执行器 ID；读取失败或节点不存在时返回空串。
- `GenerateSubtaskExecID4Test` 从进程级 mock 注册表执行相同解析，避免单元测试依赖生产注册表。
- 私有函数 `join_host_port` 集中处理 IPv6 方括号规则，避免调用者自行拼接地址。

该文件只做地址标识转换和查询，不负责注册节点、选择调度目标、提交子任务或校验端口范围。

## 主要符号

- `pub fn GenerateExecID(info: &ServerInfo) -> String`：读取 `info.IP` 和 `info.Port`，委托 `join_host_port` 生成拥有所有权的字符串。它不读取 `ServerInfo` 的 ID、标签、状态端口等其他字段。
- `pub fn MatchServerInfo(serverInfos: &[ServerInfo], schedulerID: &str) -> bool`：调用 `FindServerInfo`，以结果是否非负表示匹配结果。
- `pub fn FindServerInfo(serverInfos: &[ServerInfo], schedulerID: &str) -> isize`：用 `Iterator::position` 查找首个满足 `GenerateExecID(server) == schedulerID` 的元素；命中时把 `usize` 下标转换为 `isize`，未命中为 `-1`。
- `pub fn GenerateSubtaskExecID(id: &str) -> String`：调用 `infosync::GetAllServerInfo()`，丢弃错误后按 map key `id` 查找节点并调用 `GenerateExecID`。
- `pub fn GenerateSubtaskExecID4Test(id: &str) -> String`：通过 `infosync::MockGlobalServerInfoManagerEntry()` 获得全局 mock 管理器，读取快照后按 key 查找。
- `fn join_host_port(host: &str, port: u32) -> String`：唯一私有符号；若 `host.contains(':')` 则输出 `[host]:port`，否则输出 `host:port`。

文件没有模块级常量、自定义类型、trait、`impl` 或条件编译项；测试条件编译位于相邻的 `lib.rs`。

## 执行流程

1. 直接格式化路径从 `GenerateExecID` 开始，只读取借用的 `ServerInfo`，随后进入 `join_host_port`。
2. `join_host_port` 以主机字符串是否含 `:` 分支：含冒号时加方括号，否则直接用冒号连接端口。返回后不再进行解析、DNS 查询或网络 I/O。
3. 列表查询路径由 `FindServerInfo` 顺序遍历切片，对每个元素重新生成执行器 ID，并与 `schedulerID` 做区分大小写的完整字符串比较；首个命中即停止。`MatchServerInfo` 只把下标协议转换为布尔值。
4. 生产注册表路径 `GenerateSubtaskExecID` 先调用 `infosync::GetAllServerInfo()`。该函数在 `pkg/domain/infosync/info.rs` 中读取全局 InfoSyncer：有 etcd client 时扫描 `/tidb/server/info` 前缀并反序列化节点；没有 client 时返回本地节点。成功取得 map 后，本文件以传入的节点 ID 作为 key 查找并格式化。
5. 测试注册表路径 `GenerateSubtaskExecID4Test` 从 `OnceLock` 支撑的全局 mock 管理器获取注册信息快照，查找和格式化步骤与生产路径一致。

## 数据与状态

输入节点类型是 `infosync::ServerInfo`。与本文件直接有关的字段只有 `IP: String` 和 `Port: u32`；注册表 map 则以节点的 `ID` 为 key。生成结果没有缓存，每次调用都会分配新的 `String`。

`FindServerInfo` 借用调用者提供的切片，不修改列表，也不保存引用。若不同节点拥有相同的 `IP`/`Port`，它们会生成相同 ID，函数固定返回第一个节点的下标。空切片、空 map 或未知节点 ID 都走未命中路径。

持久或共享状态不在本文件内：生产状态由 InfoSyncer 维护；测试状态由 `MockGlobalServerInfoManagerEntry` 返回的进程级单例维护。`pkg/domain/infosync/mock_info.rs` 显示 mock 管理器读取时先锁住内部状态，再克隆条目并收集为新的 `HashMap`，因此本文件拿到的是一次读取快照。

## 依赖与调用关系

直接 Rust 依赖只有 `infosync`，在 `pkg/util/disttask/Cargo.toml` 中映射到 `astersql-domain-infosync`。主要调用边经 RustCodeGraph 按文件消歧后确认如下：

- `MatchServerInfo -> FindServerInfo -> GenerateExecID -> join_host_port`。
- `GenerateSubtaskExecID -> infosync::GetAllServerInfo`，随后对命中的节点调用 `GenerateExecID`。
- `GenerateSubtaskExecID4Test -> infosync::MockGlobalServerInfoManagerEntry`，读取 mock map 后对命中节点调用 `GenerateExecID`。

根工作区以 `facade_util_disttask` 声明该 crate，`pkg/lib.rs` 再在 `util::disttask` 门面中重新导出；`pkg/ddl/schemaver`、`pkg/dxf/importinto`、`pkg/dxf/framework/scheduler`、`pkg/dxf/framework/handle`、`pkg/executor` 和 `pkg/domain` 的 Cargo 清单也声明了依赖。不过，仓库内 Rust 源码搜索目前只找到 `pkg/lib.rs` 的门面再导出和 DXF proto 注释，没有找到生产函数调用；这些 Cargo 边只能证明可见性/依赖关系，不能证明运行时接线。

Go 对照版本存在真实上游调用：调度器枚举 InfoSync 节点并生成 executor ID，handle/status 获取 owner ID，IMPORT INTO 收集实例 ID，DDL schema version syncer 生成实例键，executor/domain 根据 DDL 节点 ID 解析子任务 executor ID。这些调用说明 API 的设计位置，但不等同于 Rust 路径已经调用它。

## 错误处理与边界

所有公开函数都采用无 `Result` 的兼容接口。`GenerateSubtaskExecID` 用 `.ok()` 把 InfoSync 的初始化失败、etcd `get_prefix` 失败和 JSON 反序列化失败统一折叠为空串；成功读取注册表但找不到 key 也返回空串。因此调用方无法从返回值区分“查询失败”“注册表为空”和“节点不存在”。测试版本没有外部查询错误类型，未知 key 同样返回空串。

格式化函数不校验地址语法和端口范围。空 IP 会生成 `:port`；`u32` 端口可以大于 TCP/UDP 的 65535，测试明确保留 `65537`；任何包含冒号的字符串都会被视作 IPv6 形态并加方括号。已带方括号、带 zone identifier 或其他非标准字符串没有专门规范化逻辑，扩展时不能假设这里完成了通用 socket address 校验。

列表查找是字符串精确匹配：没有大小写折叠、IPv6 等价地址归一化或主机名解析。下标返回类型为 `isize` 是为了保留 Go 的 `-1` 哨兵语义；极端大于 `isize::MAX` 的切片下标转换理论上会截断，但现实中无法构造这种可寻址切片，本实现也没有额外检查。

## 并发与资源生命周期

格式化和切片查询函数没有共享可变状态，只有短生命周期借用和局部 `String` 分配，可由多个线程并发调用。

生产注册表函数本身不持锁跨越返回边界。下游 `GetAllServerInfo` 在读取全局 InfoSyncer 的 etcd client 或本地 `server_info` 时获取读锁并立即克隆；有 client 时随后执行同步前缀读取和 JSON 解码。本文件等待整个 map 构造完成，再进行本地查找。它没有超时、重试、异步任务、通道或显式资源清理逻辑，这些责任属于 InfoSync/etcd 层。

mock 路径访问由 `OnceLock` 初始化的进程级单例；内部 mutex 只在制作 map 快照期间持有。`pkg/util/disttask/idservice_test.rs` 用额外的全局 mutex 串行化会清空或初始化注册表的测试，并在断言后调用 `Close` 清理 mock 状态，避免并发测试互相污染。

## 与 Go 版本的对应关系

`pkg/util/disttask/idservice.go` 是逐函数对照来源。五个公开函数的名称、主要分支、首个命中规则、`-1` 哨兵以及失败/缺失返回空串的行为均被保留。Rust 的 `join_host_port` 是对 Go `net.JoinHostPort(info.IP, strconv(port))` 的本地实现；`pkg/util/disttask/idservice_test.rs::TestGenServerID` 复现 Go 测试中的空地址、IPv4、类主机名、超出常规范围端口和 IPv6 方括号用例。

两端存在接口和实现层差异：Go `GenerateSubtaskExecID` 接收 `context.Context` 并把它传给 InfoSync，Rust 版本没有 context 参数且调用同步的无参 `GetAllServerInfo`；Go 节点类型来自 `domain/serverinfo`，Rust 通过 `infosync` 直接公开的 `ServerInfo` 使用同类字段；Go 使用标准库 `net.JoinHostPort`，Rust 仅以是否含冒号模拟当前需要的格式。Rust 独立测试还增加了列表顺序/未命中、mock 注册表回退，以及通过注入 etcd client 验证生产路径读取远端 IPv6 节点的覆盖，这些不在同路径 Go 单测中。

## 扩展指南

- 若改变 executor ID 的文本协议，应首先修改 `join_host_port`/`GenerateExecID`，同时评估元数据表中已存储的 host 值、调度器匹配、IPv6 表示和 Go/Rust 互操作兼容性；必须同步更新独立 Rust 测试 `pkg/util/disttask/idservice_test.rs` 和 Go 测试 `pkg/util/disttask/idservice_test.go`。
- 若需要区分注册表错误与节点缺失，不应只在现有空串分支上添加日志；应设计返回 `Result<Option<String>>` 等新接口并迁移调用者，保留兼容包装器时明确其降级语义。生产路径测试应覆盖 InfoSync 未初始化、etcd 错误和无效 JSON。
- 若需要更高效地匹配大量节点，可避免在每次比较时重复分配字符串，或预建 ID 索引；改变算法前应保留“按输入顺序返回首个命中”的对外不变量，并增加重复地址测试。
- 若把 Rust 实现接入 DXF、DDL、executor 或 domain，接线点应使用 crate 根/工作区门面公开 API，并为对应消费者增加独立测试；仅在 Cargo 清单声明依赖不能视为接线完成。
- 测试仍应放在 `idservice_test.rs`，由 `lib.rs` 的 `#[cfg(test)] #[path = "idservice_test.rs"]` 引入，不应把测试逻辑嵌回生产源文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标目录的 Rust/Go 实现及测试均已索引。
- RustCodeGraph `files --filter pkg/util/disttask`：确认索引覆盖 `idservice.rs`、`idservice_test.rs`、`lib.rs` 及 Go 对照文件。
- RustCodeGraph 对 `GenerateExecID`、`MatchServerInfo`、`FindServerInfo`、`GenerateSubtaskExecID`、`GenerateSubtaskExecID4Test`、`join_host_port` 执行 `query` 和带 `--file pkg/util/disttask/idservice.rs` 的 `node`：确认符号位置及文件内调用边。
- RustCodeGraph 对 `pkg/domain/infosync/info.rs::GetAllServerInfo`、`pkg/domain/infosync/mock_info.rs::GetAllServerInfo`、`MockGlobalServerInfoManagerEntry` 和 `ServerInfo` 执行 `node`：确认生产注册表读取、mock 快照锁和单例生命周期。
- 逐文件核对：`pkg/util/disttask/idservice.rs`、`pkg/util/disttask/lib.rs`、`pkg/util/disttask/Cargo.toml`、`pkg/util/disttask/idservice_test.rs`、`pkg/util/disttask/idservice.go`、`pkg/util/disttask/idservice_test.go`，以及直接依赖的 InfoSync 符号源码。
- 使用 `rg` 核对工作区 Cargo 依赖、Rust 直接引用和 Go 调用点；结果支持“Rust 已导出但尚无生产调用，Go 主链已接线”的迁移状态判断。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证本文恰有十一个固定二级章节，并人工复核所有当前能力陈述均能追溯到上述源码、图查询或清单证据。
