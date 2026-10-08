# [`pkg/store/mockstore/unistore/raw_handler.rs`](raw_handler.rs)

## 文件定位

本文件属于 `astersql-store-mockstore-unistore` crate；crate 边界由同目录 `Cargo.toml` 的 `[package] name = "astersql-store-mockstore-unistore"` 与 `[lib] path = "lib.rs"` 确定。`lib.rs` 以 `pub mod raw_handler` 声明模块，并通过 `pub use raw_handler::*` 再导出其公开类型和函数。

在完整的进程内 UniStore mock 链路中，本文件实现一套独立于 MVCC 的 Raw KV 内存存储。直接上游是 `rpc.rs` 中的 `RPCClient`：构造函数 `RPCClient::new` 创建一个空 `RawHandler`，`RPCClient::dispatch` 将 `Request::RawGet`、`RawBatchGet`、`RawPut`、`RawBatchPut`、`RawDelete`、`RawBatchDelete`、`RawDeleteRange`、`RawScan` 分发到本文件的同名方法。事务型请求则分发给 `Server::kv_*`，所以 Raw 数据不会进入 MVCC 版本链、锁表或事务提交流程。

该实现仅保存于进程内存。它没有磁盘持久化、Region 路由校验、时间戳、TTL、Column Family 或 RPC protobuf 编解码职责；这些能力不能从本文件的 API 推断为已支持。

## 核心职责

1. `RawHandler` 用一棵按字节序排列的 `BTreeMap<Vec<u8>, Vec<u8>>` 保存 Raw 键值，并用 `RwLock` 为共享访问提供同步。
2. 提供单键与批量的读取、覆盖写入和幂等删除语义；批量方法在一次锁持有期间完成整批操作。
3. 提供正向和反向的有序范围扫描，并明确实现与 Go `rawHandler.RawScan` 一致的边界：正向 `[start, end)`，反向 `(end..start)` 的遍历表现为 start 排他、end 包含。
4. 提供半开区间 `[start, end)` 删除，先复制待删键再修改映射，避免迭代期间改变容器。
5. 通过拥有型 `KvPair` 和 `safeCopy` 返回深拷贝，避免把内部映射的借用或可变存储别名暴露给调用者。

## 主要符号

- `KvPair { key: Vec<u8>, value: Vec<u8> }`：Raw 批量读写和扫描的拥有型键值对象。派生 `Clone`、`Default`、`Eq`、`PartialEq`，因此测试可直接比较完整结果。
- `RawGetResponse { value: Vec<u8>, not_found: bool }`：单键读取结果。`not_found` 由返回值是否为空计算，因此“键不存在”和“键存在但值为空”在此协议中不可区分；这是 Go 实现 `len(val) == 0` 的对齐行为。
- `RawHandler { store: RwLock<BTreeMap<Vec<u8>, Vec<u8>>> }`：唯一的状态持有者。字段私有，调用者只能经公开方法访问。
- `RawHandler::new() -> Self`：创建空映射；等价于 `Default::default()`。
- `raw_get(&self, key) -> RawGetResponse`、`raw_batch_get(&self, keys) -> Vec<KvPair>`：读路径。批量读取保持输入键的顺序，并为缺失键保留对应条目、返回空值。
- `raw_put`、`raw_batch_put`：插入或覆盖。批量方法逐项克隆传入的键和值。
- `raw_delete`、`raw_batch_delete`：删除存在或不存在的键，不返回删除计数或错误。
- `raw_delete_range(start, end)`：删除 `[start, end)`；当 `start >= end` 时直接返回。
- `raw_scan(start, end, limit, reverse)`：按键序扫描并最多返回 `limit` 项。正向空 `end` 表示无上界；反向空 `end` 表示无下界，但空 `start` 仍因 `start <= end` 返回空结果，与 Go 的 `SeekForPrev([])` 行为对齐。
- `keys() -> Vec<Vec<u8>>`：按序复制全部键，供测试或调试观察；生产分发链不依赖它。
- 私有 `copy_pair`：把 `BTreeMap` 条目转为拥有型 `KvPair`。
- `newRawHandler() -> RawHandler` 与 `safeCopy(&[u8]) -> Vec<u8>`：保留 Go 风格命名的公开兼容辅助函数；当前 `RPCClient` 直接使用 `RawHandler::new`，扫描通过 `copy_pair` 间接使用 `safeCopy`。

## 执行流程

Raw 请求的主链为：上层构造 `rpc.rs::Request::Raw*`，`RPCClient::send_request` 完成客户端关闭状态、可选拦截器及 store 地址校验后调用私有 `dispatch`，Raw 分支再调用本文件方法并包装成 `Response::RawGet`、`Response::RawPairs` 或 `Response::Empty`。

单键读取在取得读锁后调用 `BTreeMap::get`，克隆命中值；未命中以空 `Vec` 代替，随后以 `value.is_empty()` 设置 `not_found`。批量读取只获取一次读锁，按请求键迭代并逐项复制，因此输出长度和顺序与输入一致。

单键写入或删除取得写锁后直接调用 `insert` 或 `remove`。批量写入、删除同样只取得一次写锁，然后依请求顺序循环；若同一键在批量写入中重复出现，后面的值覆盖前面的值。

范围删除先检查 `start >= end`，避免把倒置区间传给会 panic 的 `BTreeMap::range`。有效范围用 `(Included(start), Excluded(end))` 枚举，先将键收集到独立 `Vec`，再逐个删除。因此删除边界为 `[start, end)`，且容器不会在活动迭代器存在时被修改。

正向扫描先拒绝非空上界下的倒置或空区间；随后以 `Included(start)` 为下界，以 `Excluded(end)` 或 `Unbounded` 为上界，按升序取前 `limit` 项。反向扫描先拒绝 `start <= end`，再以 `Included(end)`（或空 end 对应 `Unbounded`）和 `Excluded(start)` 建立范围，调用 `rev()` 降序遍历并截取 `limit` 项。两条分支最终都通过 `copy_pair` 返回独立数据。

## 数据与状态

键和值都是拥有型字节向量，不施加 UTF-8 或 SQL 类型约束；`BTreeMap` 使用 `Vec<u8>` 的字典序，因此范围边界和结果顺序是原始字节序。`raw_put` 覆盖既有键，删除不存在的键是无操作。`RawHandler::default/new` 始终从空状态开始，`RPCClient` 的每个实例各自持有一个 handler，因此 Raw 状态不在不同客户端实例间自动共享。

读方法克隆值，扫描克隆键和值，`keys` 克隆键；调用方修改返回对象不会改变存储内容。反过来，写方法接收拥有型键和值或克隆批量输入，也不会保留调用方可修改的引用。

空值需要特别处理：映射能够存储空 `Vec<u8>`，但 `raw_get` 会把它标记为 `not_found = true`；`raw_batch_get` 则没有独立缺失标记，缺失键和空值键都表现为空 value。该不变量直接继承 Go 版本的返回约定。

## 依赖与调用关系

本文件的标准库依赖只有 `BTreeMap`、`Bound::{Included, Excluded, Unbounded}` 和 `RwLock`，没有直接使用 `Cargo.toml` 中列出的外部 crate。`BTreeMap` 替代 Go 侧 `lockstore.MemStore`，为 seek/scan 提供确定的有序语义；`RwLock` 对应 Go 的 `sync.RWMutex`。

已核验的直接上游调用边位于 `pkg/store/mockstore/unistore/rpc.rs`：`RPCClient::new -> RawHandler::new`，以及 `RPCClient::dispatch -> raw_get/raw_batch_get/raw_put/raw_batch_put/raw_delete/raw_batch_delete/raw_delete_range/raw_scan`。`RPCClient::raw_handler` 还返回只读 handler 引用，供 crate 内外的测试或调试调用这些 API。

模块装配边为 `lib.rs -> pub mod raw_handler`，并由 glob re-export 暴露符号。独立 Rust 测试通过 `lib.rs` 的 `#[path = "raw_handler_test.rs"] mod raw_handler_test` 接入，而不是内嵌在生产源文件中。

RustCodeGraph 对目标文件识别出 `RawHandler`、全部方法、`copy_pair`、`newRawHandler` 和 `safeCopy`，但其 callers/callees 命令未为这些方法返回静态边；因此上述上游边另由已索引的 `rpc.rs` 源码和全仓符号引用搜索复核，不把图中缺边解释为“未接线”。

## 错误处理与边界

公开方法不返回 `Result`。正常的缺失键、重复覆盖和删除不存在键均以值或无操作表达。唯一显式失败方式是 `RwLock` 中毒：每次加锁都用 `expect("raw-store lock poisoned")`，若其他线程持锁期间 panic，后续访问也会 panic，而不是恢复中毒锁或向 RPC 返回结构化错误。

`raw_delete_range`、正向扫描和反向扫描都在调用 `BTreeMap::range` 前检查边界，避免倒置范围触发标准库 panic。`limit == 0` 由迭代器的 `take(0)` 自然返回空列表。正向 `end.is_empty()` 被解释为无上界；反向只有 `end.is_empty()` 被解释为无下界，`start.is_empty()` 不代表无上界，相关测试要求其返回空结果。

本文件没有校验键大小、值大小或请求上下文，也不产生网络/存储错误。地址、关闭状态和请求拦截错误由上游 `RPCClient::send_request` 处理；将来若增加可失败的存储操作，必须同步改变这里的方法返回类型以及 `rpc.rs::dispatch` 的响应/错误传播。

## 并发与资源生命周期

`RawHandler` 通过 `RwLock` 允许多个读取并发，写入互斥且阻塞所有读取。单次批量操作和单次扫描在整个循环期间持有一把锁，因此同一调用看到稳定映射，其他写入不会穿插；代价是大批量或大范围扫描会延长锁持有时间。

`raw_delete_range` 在写锁内同时完成键收集和删除，既保证区间选择与删除之间没有并发变化，也带来与命中键数量成正比的临时键拷贝。扫描返回的所有数据均在读锁释放前复制完成，返回值不依赖锁或映射条目的生命周期。

没有后台任务、通道、异步运行时、显式关闭或外部资源句柄。handler 随所属 `RPCClient` 析构，其 `BTreeMap` 和所有字节向量由 Rust 自动释放。`RPCClient::send_request_async` 可以从其他线程进入同一个客户端，因此 `RwLock` 是实际并发路径所需，而非仅测试装饰。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/raw_handler.go`，测试对照是 `raw_handler_test.go`。Rust 保留 Go 的核心结构和行为：独立 Raw 存储、读写锁、单键/批量 CRUD、范围删除、正反向扫描、空值表示未找到，以及扫描结果深拷贝。Go `rpc.go` 的 Raw 命令分发与 Rust `rpc.rs::RPCClient::dispatch` 覆盖同一组八种操作。

实现载体不同：Go 使用 `*lockstore.MemStore` 和显式 iterator，Rust 使用 `BTreeMap` 的范围迭代；Go 方法接收 `context.Context` 与 kvproto 请求并返回 protobuf 响应和 `error`，Rust handler 只接收领域字段并返回本地结构，由 `rpc.rs` 负责请求/响应枚举适配。Rust 的 `newRawHandler` 返回值而非指针，正常共享由包含它的 `RPCClient` 管理。

Rust 为保持 Go 迭代器在倒置边界上的空结果，额外在 `raw_delete_range` 和 `raw_scan` 前做比较检查；否则 `BTreeMap::range` 会 panic。Rust 测试也增加了 Go 测试未单独列出的反向空 start 与倒置区间回归用例。当前 Rust `raw_handler.rs` 没有 Go protobuf/context 外壳，但不应据此把 Go 的边界行为简化掉。

另需避免混淆 `pkg/store/mockstore/unistore/tikv/server.rs` 中带 TTL/时间参数的另一套 `raw_*` 方法：本文件由顶层 `rpc.rs` 的 `RPCClient::raw_handler` 分支调用，状态和能力边界不同。

## 扩展指南

- 新增 Raw 命令时，先在 `RawHandler` 增加最小领域方法，再同步接入 `rpc.rs` 的 `Request`、`Response` 与 `RPCClient::dispatch`；若需保持 Go 对齐，同时检查 `raw_handler.go` 和 `rpc.go` 的对应命令。
- 修改扫描边界必须同时验证正向/反向、空 start、空 end、相等/倒置边界和 `limit == 0`。不要直接删除现有的预检查，因为标准库范围 API 与 Go iterator 对无效区间的失败模式不同。
- 修改缺失值表达时必须同时调整 `RawGetResponse::not_found`、批量读取契约、RPC 响应适配和 Go 兼容预期；允许空值与区分缺失是一个协议级决策。
- 需要原子批量语义时，可复用当前“一次批量调用持有一把锁”的结构；若为降低锁竞争改成分段锁或释放锁后处理，必须评估并发写入造成的快照变化。
- 若加入持久化、TTL、Column Family、错误恢复或 Region 语义，不宜只扩张本文件的内存映射；应先确认是否应复用 `tikv/server.rs` 的 Raw 状态或在 RPC 层明确两套实现的归属。
- 测试应继续放在独立的 `pkg/store/mockstore/unistore/raw_handler_test.rs`，不要嵌入生产文件。至少扩充与改动方法对应的 Rust 回归，并以 `raw_handler_test.go` 的既有意图为基线；涉及分发时还应补 `rpc_test.rs` 的端到端请求覆盖。
- 性能风险集中于全量克隆、大范围扫描持有读锁、范围删除在写锁内分配临时键列表，以及批量请求的线性处理；优化时不得牺牲稳定顺序、拥有型返回值和调用级一致性而不更新契约。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点；目标文件可完整读取为 197 行。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/raw_handler.rs`：核验 `KvPair`、`RawGetResponse`、`RawHandler`、十个公开方法（含构造函数与 `keys`）、`copy_pair`、`newRawHandler`、`safeCopy` 的签名与实现。
- RustCodeGraph `query`：精确定位 `RawHandler`、各 `raw_*` 方法、`newRawHandler` 和 `safeCopy`；`callers/callees` 对目标方法未返回边，此限制已在“依赖与调用关系”中披露。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/rpc.rs --offset 380 --limit 410`：核验 `RPCClient` 持有 handler、构造边和八个 Raw 请求分发边，以及异步请求可从线程进入客户端。
- `pkg/store/mockstore/unistore/Cargo.toml`、`lib.rs`：核验 crate 名称、库入口、模块声明、公开再导出和独立测试模块接线。
- `pkg/store/mockstore/unistore/raw_handler.go`、`rpc.go`、`raw_handler_test.go`：核验 Go 侧锁、MemStore iterator、Raw 命令转发、边界与测试意图。
- `pkg/store/mockstore/unistore/raw_handler_test.rs`：核验 Rust 单键/批量 CRUD、限量正向扫描、半开范围删除、反向空 start 和倒置范围行为。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只检查文档固定章节、路径/符号引用和差异范围。
