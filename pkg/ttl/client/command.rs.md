# `pkg/ttl/client/command.rs`

## 文件定位

`command.rs` 属于 `astersql-ttl-client` crate，由 [`lib.rs`](./lib.rs) 的 `pub mod command` 装配并整体再导出。它定义 TTL 节点间命令的 JSON 协议、发送/订阅/认领/应答抽象，以及两套进程内实现：基于 `EtcdStore` 的 `EtcdClient` 和直接使用通道的 `MockClient`。协议键空间固定为 `/tidb/ttl/cmd/req/` 与 `/tidb/ttl/cmd/resp/`，请求和响应的默认租约均为 180 秒（`TTL_CMD_KEY_LEASE_SECONDS`、`TTL_CMD_KEY_REQUEST_PREFIX`、`TTL_CMD_KEY_RESPONSE_PREFIX`）。

需要特别区分命名与当前接线：Rust 的 `EtcdStore` 是 `Mutex<HashMap<...>>` 支撑的进程内模拟存储，并不连接真实 etcd；当前 [`Cargo.toml`](./Cargo.toml) 的有效依赖为空，历史依赖只位于永不成立的 `cfg(any())` 下。仓库当前可检索到的生产消费者主要是 `pkg/server/handler/ttlhandler/ttl.rs` 对 `TriggerNewTtlJobResponse` 的类型复用；`pkg/session/runtime/ttl_runtime.rs` 则另行实现真实 `etcd_client` watch/take/response 传输，没有调用本文件的 `EtcdClient`。因此本文件目前同时承担协议模型、单元测试传输和可复用 API，而不能被描述为已经接入生产集群的真实 etcd 客户端。

## 核心职责

1. 用 `JsonValue`、`JsonParser` 和 `write_json_string` 提供无第三方依赖的完整 JSON 值、解析与序列化能力，作为 Go `json.RawMessage` 的 Rust 对应物。
2. 用 `CmdRequest`、私有 `CmdResponse`、`TriggerNewTtlJobRequest`、`TriggerNewTtlJobResponse` 和 `TriggerNewTtlJobTableResult` 固化命令线协议；`trigger_new_ttl_job` 封装唯一内建命令类型 `trigger_ttl_job`。
3. 用 `CommandClient` 规定发送、watch、原子式认领和回写响应四步协议，并用 `CommandReceiver` 封装接收端通道。
4. 用 `ClientContext` 在同步 Rust API 中表达 Go `context.Context` 的取消和截止时间语义，用 `ClientError` 统一序列化、后端、响应及通道错误。
5. 用 `EtcdStore`/`EtcdClient` 模拟带租约键值存储与 watch 行为，用 `MockClient` 提供更轻量、可检查错误分支的测试替身。

## 主要符号

- `JsonValue`：公开的 JSON 代数类型，保留 `null`、布尔、原始数字文本、字符串、数组和按键排序的对象。`parse` 委托 `JsonParser`，`to_bytes`/`write_json` 负责输出；对象采用 `BTreeMap`，所以序列化键顺序稳定，但协议不应依赖该顺序。
- `JsonParser`：私有递归下降解析器。`parse_value` 分派值类型；`parse_number` 拒绝前导零并支持小数和指数；`parse_string` 处理转义；`parse_unicode_escape` 合并合法 UTF-16 代理对，并把孤立代理项替换为 U+FFFD，以匹配 Go `encoding/json`。
- `ClientContext`：共享 `Arc<AtomicBool>` 取消标志和可选 `Instant` 截止时间。`with_timeout` 不创建独立取消源，而是共享取消标志并取父截止时间和新截止时间的较早值；`wait_slice` 为阻塞轮询计算短等待区间。
- `ClientError`：公开错误枚举。`Cancelled`/`Timeout` 对应上下文终止，`Serialization` 表示 JSON/协议形状错误，`Backend` 供后端失败，`Response` 表示远端业务错误；`ChannelClosed`、`WatcherBlocked`、`ResponseKeyNotFound` 描述 mock 通道状态。`ResponseTypeMismatch` 当前没有实际产生路径。
- `CmdRequest`：公开请求载体。`get_trigger_ttl_job_request` 仅在 `cmd_type == TTL_CMD_TYPE_TRIGGER_TTL_JOB` 时解析专用载荷；`from_bytes` 对字段类型严格，但通过 `go_struct_string_field` 让缺失或 `null` 字符串保留 Go 零值。
- `CmdResponse`：私有响应载体。`decode_response` 将非空 `error_message` 转成 `ClientError::Response`，否则返回 `data`；解析时并不校验响应中的 `request_id` 是否等于等待的 ID。
- `TriggerNewTtlJobRequest` / `TriggerNewTtlJobResponse` / `TriggerNewTtlJobTableResult`：公开的触发命令协议。表结果的 `partition_name`、`job_id`、`error_message` 为空时由 `to_json` 省略；反序列化时缺失字符串/整数恢复为空串/零，缺失或 `null` 的 `table_result` 恢复为空列表。
- `CommandResult`：响应方的成功 JSON 或错误字符串；`CommandResult::data` 是成功构造器。
- `CommandClient`：核心公开 trait。`command` 总会同时返回生成的 request ID 和结果；`watch_command` 返回请求流；`take_command` 通过删除请求确保仅一个接收者处理；成功认领后调用者必须调用 `response_command` 完成协议。
- `EtcdStore` / `EtcdClient` / `new_command_client`：进程内键值、watch、租约模拟及其客户端。`EtcdStore::fail_next` 可按 FIFO 注入下一次存储操作错误。
- `MockClient` / `new_mock_command_client`：不用键前缀或字节编码的测试实现，分别保存待处理请求和一次性同步响应发送端。
- `new_request_id`：以当前纳秒、进程 ID 和原子序列异或后格式化出 UUID 外形字符串；它用于进程内唯一关联，不是标准 UUID v4 随机生成器，也没有加密唯一性保证。

## 执行流程

发送方的 `EtcdClient::command` 先调用 `send_command`：生成 request ID，将 `CmdRequest` 编码为 JSON，并以 180 秒租约写入请求键。写入成功后，`wait_command_response` 为调用上下文增加最多 180 秒的截止时间，先注册精确响应键 watch；循环中每次至多等待一秒，收到 `Put` 时立即 `decode_response`，没有事件时再 `get` 响应键。这一“先 watch、再周期 get”的组合既消费实时事件，也补偿 watch 断开或事件竞争造成的漏读。当前实现不会在成功后主动删除响应键，依靠租约过期清理。

接收方调用 `watch_command` 订阅请求前缀。`EtcdClient` 为此启动转发线程，只转发 `Put`；解析失败时按 Go 行为发送 `CmdRequest::default()`，而不是丢弃事件或返回错误。多个订阅者都可能看到同一请求，因此每个处理者必须先调用 `take_command`；`EtcdStore::delete` 只有第一次删除返回 `true`，后续竞争者得到 `false`。获胜者处理请求后用 `response_command` 将 `CommandResult` 编码到对应响应键，同样附带 180 秒租约。

`trigger_new_ttl_job` 构造库名/表名载荷，调用 `CommandClient::command`，再将成功 JSON 解为 `TriggerNewTtlJobResponse`。命令发送产生的 request ID 在该便捷函数中被丢弃；传输错误、远端错误和响应形状错误均原样返回为 `ClientError`。

`MockClient::command` 的流程相同但不经过字节协议：`send_command` 在锁内登记请求和容量为 1 的响应通道，并用 `try_send` 广播给现有 watcher；任何 watcher 已满都会使发送返回 `WatcherBlocked`。发送方随后按上下文截止时间等待一次响应。`watch_command` 新建容量为 `16 + 当前待处理请求数` 的通道，先回放所有未认领请求，再在上下文终止后从 watcher 列表移除。`response_command` 原子移除响应发送端，因此同一 request ID 只能响应一次。

## 数据与状态

线协议以 request ID 关联两个键：请求键保存 `{request_id, cmd_type, data}`，响应键保存 `{request_id, error_message, data}`。`data` 始终是任意 `JsonValue`，传输层不认识命令特定字段；新增命令可复用该信封而不改 trait。`trigger_ttl_job` 是当前唯一由常量和专用类型表达的命令。

`EtcdState` 在单个 `Mutex` 下保存 `entries`、有序 watcher 列表和错误注入队列。每个 `EtcdEntry` 用 `Instant` 记录可选过期点；过期项仅在后续 `put`、`get` 或 `delete` 调用时由 `purge_expired` 惰性清理，过期本身不会产生 `Delete` watch 事件。`put`/`delete` 持锁更新状态并同步向匹配 watcher 发送事件；发送端已关闭的 watcher 会被移除。

`MockState` 也受单个 `Mutex` 保护，但 `requests` 与响应发送端分表保存。命令被 `take_command` 后仅从 `requests` 删除，响应发送端仍保留到 `response_command`；若处理者认领后不响应，发送方只能等取消或 180 秒超时。watcher ID 和 request 序列都由 `AtomicU64` 以 `Relaxed` 顺序递增；唯一状态一致性由互斥锁或通道提供，不依赖这些原子的跨线程排序。

## 依赖与调用关系

内部主调用边为：`trigger_new_ttl_job -> CommandClient::command`；`EtcdClient::command -> send_command -> EtcdStore::put`，随后 `wait_command_response -> EtcdStore::watch/get -> decode_response`；接收侧 `watch_command -> EtcdStore::watch -> CmdRequest::from_bytes`，`take_command -> EtcdStore::delete`，`response_command -> EtcdStore::put`。`MockClient` 实现同一 trait，但用 `mpsc::sync_channel` 和 `MockState` 代替字节存储。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 和 [`lib.rs`](./lib.rs) 证明：本文件只用 Rust 标准库，`command` 与 `notification` 共享 `EtcdStore`、`ClientContext` 和部分 mock 状态。RustCodeGraph 将本文件列为被 15 个文件使用，但其宽泛符号匹配包含测试和相邻模块；仓库精确文本检索显示，`pkg/server/handler/ttlhandler/ttl.rs` 当前只复用 `TriggerNewTtlJobResponse`。生产 TTL manager 的 watch/take/response 主链位于 `pkg/session/runtime/ttl_runtime.rs`，使用另一套 `TtlWatchTransport` 和真实 `etcd_client` 事务删除；它与本文件通过相同键格式和 JSON 形状保持协议兼容，而不是 Rust 调用边。

Go 直接对应实现是 [`command.go`](./command.go)，回归测试分别是 Rust [`command_test.rs`](./command_test.rs) 和 Go [`command_test.go`](./command_test.go)。Rust 测试通过同一套 `run_command_client_round_trip` 同时验证 `EtcdClient` 与 `MockClient`，这是两个实现应保持行为一致的直接证据。

## 错误处理与边界

- 所有存储操作先检查 `ClientContext::error`；取消优先于超时。`wait_slice` 在每轮阻塞前重复检查，因此等待可在最多一个切片后响应终止。
- `EtcdClient::command` 即使发送或等待失败也返回已生成的非空 request ID，和 Go 接口注释一致。若请求已写入后调用者取消，其他节点仍可能认领并产生一个无人接收、最终过期的响应。
- JSON 输入必须是 UTF-8，且解析器拒绝尾随数据、非法数字、非法转义和对象/数组结构错误。数字以字符串保留，只有协议整数取值时才解析成 `i64`。
- 为对齐 Go struct 解码，缺失/`null` 的字符串与整数是零值；字段存在但类型错误则失败。`CmdRequest::from_bytes` 对损坏请求返回错误，但 `EtcdClient::watch_command` 故意吞掉该错误并转发零值请求。
- `decode_response` 只依据 `error_message` 判定业务错误，不验证响应内的 request ID；键本身承担关联责任。空错误字符串和 `data: null` 是合法成功响应。
- `EtcdStore` 的锁若被持锁线程 panic 会中毒，代码中的 `unwrap` 会继续 panic；该进程内实现没有恢复策略。`SystemTime` 早于 UNIX epoch 时 `new_request_id` 使用零时长继续生成 ID。
- `MockClient::response_command` 对未知或已响应的 ID 返回 `ResponseKeyNotFound`；响应通道意外已满时忽略 `try_send` 失败并仍返回成功，以对齐 Go mock 的断言后返回语义。`EtcdClient` 则允许直接写响应，不强制验证调用者是否先成功 `take_command`。
- `ResponseTypeMismatch` 和 `ClientError::Backend` 在本文件当前路径中没有构造点；扩展代码不应把它们当作已经覆盖的运行时分支。

## 并发与资源生命周期

`ClientContext` 的克隆共享取消标志，但各自保留截止时间值。`EtcdStore::watch` 每次订阅启动一个清理线程，以 10 毫秒轮询上下文，终止后按 watcher ID 注销；`EtcdClient::watch_command` 另起一个转发线程，以 20 毫秒超时检查终止。`MockClient::watch_command` 同样为每个 watcher 启动 10 毫秒轮询清理线程。这些线程没有暴露 `JoinHandle`，调用者以取消上下文和丢弃 receiver 驱动回收，线程退出是异步的。

`std::sync::mpsc` 接收端不是 `Sync`，因此 `CommandReceiver` 应由单一消费方持有。`EtcdStore` watch 使用无界通道，慢消费者可能积压内存；而 `MockClient` 使用有界通道并将满队列暴露为 `WatcherBlocked`。`EtcdStore::put/delete` 在持锁期间向无界发送端发送，不会因容量阻塞，但 watcher 清理也需要同一锁。

认领的不变量是“删除请求键者获胜”：模拟存储在一个互斥区完成删除；真实 Rust runtime 在 `pkg/session/runtime/ttl_runtime.rs` 使用带 create-revision 条件的 etcd 事务实现跨节点原子认领。响应和请求租约均限制为 180 秒；模拟存储仅惰性淘汰，因此内存中已过期项可能保留到下一次存储操作。

## 与 Go 版本的对应关系

[`command.go`](./command.go) 是结构和语义基线。常量、请求/响应字段、`CommandClient` 四个操作、`TriggerNewTTLJob` 便捷函数、watch 后周期性 get 的防漏读策略、删除请求实现认领、带 180 秒租约的响应，以及 mock 的回放/单次响应行为都在 Rust 中有对应实现。[`command_test.go`](./command_test.go) 的成功往返、重复认领返回 false 和远端错误传播，在 [`command_test.rs`](./command_test.rs) 的 `run_command_client_round_trip` 中对两种 Rust client 重放。

实现差异包括：Go `etcdClient` 包装真实 `clientv3.Client` 并显式 `Grant` 租约，Rust `EtcdClient` 包装进程内 `EtcdStore`；Go 使用 `encoding/json` 和 `json.RawMessage`，Rust以手写 `JsonValue` 保留通用载荷；Go 使用 `google/uuid`，Rust 使用时间/计数/进程 ID 组合；Go watch 关闭后把通道替换为永久阻塞通道并继续轮询，Rust watch 断开时短暂休眠并继续用 `get` 查找；Go malformed watch payload 会记录日志后发送零值请求，Rust发送零值请求但没有日志依赖。

Rust 有额外的 Go 兼容测试：`etcd_response_accepts_missing_request_id_like_go_json_unmarshal`、`trigger_ttl_job_request_accepts_missing_fields_like_go_json_unmarshal`、`mock_take_and_response_ignore_cancelled_context_like_go` 和 `json_string_unicode_surrogates_match_go_encoding_json`。这些测试明确约束零值字段、mock 忽略取消上下文以及 UTF-16 代理项处理，扩展解析器时不可随意收紧。

## 扩展指南

新增命令类型时，应新增命令常量和专用请求/响应类型，复用 `JsonValue` 信封，并像 `trigger_new_ttl_job` 一样提供薄封装；同时在独立的 [`command_test.rs`](./command_test.rs) 增加成功、错误、缺失字段和类型错误用例，在 Go 协议仍是兼容目标时同步核对 [`command.go`](./command.go) 与 [`command_test.go`](./command_test.go)。不要把测试嵌入生产源文件。

修改键格式、租约或认领语义时，必须同步检查 `pkg/session/runtime/ttl_runtime.rs` 的真实 etcd 实现，因为当前两者没有共享常量或 trait，容易发生协议漂移。若要让本 `EtcdClient` 成为生产真实 etcd 客户端，需要显式引入并接线外部后端，而不是继续扩展 `EtcdStore`；这会改变 crate 依赖、异步/线程模型和错误映射，不能当作本文件现状。

扩展 JSON 时优先修改 `JsonParser`/`JsonValue::write_json` 并保留 Go 兼容测试。尤其要评估大载荷的递归深度、无界 watch 队列和多次字符串分配带来的性能风险。若新增错误分支，应明确它来自协议、后端还是业务响应，并补齐 `Display` 和两个 client 实现的一致行为；若修改 watcher 生命周期，应提供可确定终止或 join 的方案，避免积累后台轮询线程。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`node --file pkg/ttl/client/command.rs` 阅读了 1–1324 行，并得到“被 15 个文件使用”的文件级关系；`query trigger_new_ttl_job`、`query new_command_client`、`query watch_command/response_command/take_command`、`query decode_response/new_request_id` 核对了关键符号及签名。精确 callers/callees 查询未返回边，因此没有据此虚构上游，改由精确文本检索确认实际引用。
- 源与边界：完整阅读 [`command.rs`](./command.rs)、[`Cargo.toml`](./Cargo.toml) 和 [`lib.rs`](./lib.rs)，核对公开/私有符号、有效依赖、模块再导出、线程/锁/通道及租约生命周期。
- Go 对照：完整阅读 [`command.go`](./command.go) 与 [`command_test.go`](./command_test.go)，核对真实 etcd、JSON 零值、watch/get、take/response 和 mock 行为。
- Rust 测试：完整阅读 [`command_test.rs`](./command_test.rs)；其覆盖两种 client 的成功与错误往返、取消、缺失 request ID、未知响应 ID、重复/缺失认领、JSON 往返、Go 零值规则与 Unicode 代理项。
- 生产接线复核：精确检索 `astersql_ttl_client`、`CommandClient`、`trigger_new_ttl_job`、`watch_command`、`take_command` 和 `response_command`；读取 `pkg/server/handler/ttlhandler/ttl.rs` 的响应类型使用，以及 `pkg/session/runtime/ttl_runtime.rs` 的真实 etcd watch/事务认领/响应写入路径。
- 本任务是纯文档分析，按计划不运行 Cargo。交付检查使用任务规定的 11 章节结构命令，并额外检查仅新增本说明文档、总计划未修改、Markdown 相对链接目标存在和 diff 无无依据的“已支持”结论。
