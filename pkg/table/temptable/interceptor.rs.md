# `pkg/table/temptable/interceptor.rs`

源文件：[`interceptor.rs`](./interceptor.rs)；Go 对照：[`interceptor.go`](./interceptor.go)；独立 Rust 测试：[`interceptor_test.rs`](./interceptor_test.rs)。

## 文件定位

本文件位于 `astersql-table-temptable` crate，负责实现“临时表读取不应落到底层持久化快照”这一读路径规则。它把键值读取抽象为本文件内的 `Retriever`、`Snapshot`、`KvIterator` 与 `SnapshotInterceptor`，再由 `TemporaryTableSnapshotInterceptor` 根据 `InfoSchema` 中的表类型，将点查、批量点查和范围扫描路由到会话 `MemBuffer`、底层 `Snapshot`，或二者的有序合并结果（`interceptor.rs:55-79,458-484`）。

crate 根 `lib.rs` 声明并公开再导出 `interceptor` 模块，但当前 `Cargo.toml` 的正常 `[dependencies]` 为空，真实 `astersql-kv`、`astersql-store-driver-txn` 等依赖只列在永不启用的 `target.'cfg(any())'.dependencies` 中。仓库精确搜索也只发现这些 API 在 `interceptor_test.rs` 中使用，没有找到 `session_snapshot_interceptor` 的生产调用者。因此当前代码是一个 crate 内自包含、经过独立测试的 Go 语义移植层，尚不能等同于已经接入 `pkg/store/driver/txn/snapshot.rs` 的生产快照拦截链。

## 核心职责

- 识别 TiDB 表键：`encode_table_prefix`、`decode_table_id`、`get_key_accessed_table_id`、`get_range_accessed_table_id` 和 `not_table_range` 实现表前缀编码、合法 ID 过滤与扫描范围分类（`interceptor.rs:713-749`）。合法表 ID 必须满足 `0 < id < i64::MAX`。
- 路由临时表读取：`TemporaryTableSnapshotInterceptor` 查询 `InfoSchema::table_by_id`，仅将 `TempTableType::Local` 或 `Global` 当作临时表；普通表和未知表继续读取底层快照（`interceptor.rs:495-499,530-551,554-620`）。
- 维护会话内脏数据语义：`MemBuffer` 用有序映射保存会话写入，空 `value` 是删除标记，不是一个可返回的业务值（`interceptor.rs:81-138`）。
- 合并范围结果：`UnionIter` 以键序归并会话迭代器与快照迭代器，同键时会话值覆盖快照值，空值删除同时屏蔽快照旧值；正向和反向扫描共用同一状态机（`interceptor.rs:223-404`）。
- 保证失败路径资源清理：创建会话迭代器或初始化合并迭代器失败时，关闭已经创建的子迭代器（`interceptor.rs:663-710`）。

## 主要符号

- `Key = Vec<u8>`、`ValueEntry { value, commit_ts }`：本文件的键和值表示。`ValueEntry::is_value_empty` 用来识别会话删除标记（`interceptor.rs:29-52`）。
- `KvIterator`：统一迭代协议，包含当前位置访问、推进、关闭和测试用的运行时类型检查；默认 `closed` 为 `false`（`interceptor.rs:54-66`）。
- `Retriever` / `Snapshot`：前者提供 `get/iter/iter_reverse`，后者额外提供 `batch_get`（`interceptor.rs:68-79`）。
- `MemBuffer`：以 `RwLock<BTreeMap<Key, ValueEntry>>` 保存会话数据。`set_table_key` 写入，`delete_table_key` 在校验 table ID 后写空值墓碑；`entries` 物化指定半开区间并可反转（`interceptor.rs:81-161`）。
- `VecIterator`：对预物化条目向量进行无失败推进的基础迭代器（`interceptor.rs:163-221`）。
- `UnionIter`：双路有序归并状态机；`update_cur` 是核心选择逻辑，`next` 推进当前来源后重新选择（`interceptor.rs:223-404`）。
- `EmptyIterator`：始终无有效元素，用于全局临时表、缺少会话数据或缺少快照侧时的空输入（`interceptor.rs:406-456`）。
- `SnapshotInterceptor`：定义 `on_get/on_batch_get/on_iter/on_iter_reverse` 四个读拦截入口（`interceptor.rs:458-478`）。
- `TemporaryTableSnapshotInterceptor`：持有共享 `InfoSchema` 和可选会话 `Retriever`，实现临时表识别、批量键拆分及四个拦截入口（`interceptor.rs:480-620`）。
- `session_snapshot_interceptor`：仅当 `InfoSchema::has_temporary_table` 为真时，从 `SessionVarsProvider` 的 `temporary_table_data` 克隆会话缓冲并构造拦截器（`interceptor.rs:622-641`）。
- `get_session_key` / `create_union_iter`：分别封装临时表点查规则与范围合并构造、错误清理规则（`interceptor.rs:643-710`）。

## 执行流程

1. `session_snapshot_interceptor` 先检查当前 `InfoSchema` 是否包含临时表；没有则返回 `None`，避免为普通会话安装无效拦截器。存在临时表时，它读取会话变量中的 `temporary_table_data`，构造 `TemporaryTableSnapshotInterceptor`。
2. 点查 `on_get` 从键解析 table ID，并经 `temporary_table_info_by_id` 判断是否为临时表。临时表调用 `get_session_key`；普通键、普通表或目录中不存在的表直接调用 `snapshot.get`。
3. 批量点查 `on_batch_get` 先由 `batch_get_temporary_table_keys` 遍历输入：临时表键逐个查会话，未命中不会进入底层快照；其他键收集后一次调用 `snapshot.batch_get`。最终将会话结果扩展进快照结果，因此即使异常快照返回重复键，会话值仍胜出（`interceptor.rs:501-528,565-580`）。
4. 正向范围 `on_iter` 先用 `not_table_range` 排除与表键空间无交集的范围；若范围完整落入单表，由 `iter_table` 根据表类型选择快照、空迭代器或仅会话侧的 `UnionIter`；跨表范围则合并会话与快照（`interceptor.rs:582-601`）。
5. 反向范围 `on_iter_reverse` 对非表范围直接下推，其余范围调用 `create_union_iter(..., reverse = true)`；实现遵循 Go 代码中“反向扫描的 lower bound 通常为空，不能按单表快速路径处理”的结构（`interceptor.rs:603-619`）。
6. `UnionIter::update_cur` 比较两侧当前键。相同键时丢弃快照项并选择会话项；若会话项为空值，则同时跳过两侧。不同键时按扫描方向选择更靠前的一侧，并持续跳过会话删除标记（`interceptor.rs:295-351`）。

## 数据与状态

`MemBuffer.values` 是会话脏写的唯一可变数据，以 `BTreeMap` 保持字节序，外层 `RwLock` 支持共享读取和独占写入。`entries` 在持有读锁时克隆范围内的键值，随后迭代发生在独立 `Vec` 上，所以创建后的 `VecIterator` 不会观察后续缓冲修改，也不会长期持有锁（`interceptor.rs:83-137`）。

`UnionIter` 保存两个可选子迭代器、两侧有效性缓存、当前来源、整体有效性、扫描方向和当前值副本。`current_value` 被克隆出来，使 `value()` 的返回引用不依赖下一次对子迭代器的可变访问。其关键不变量是：有效状态下两个 `Option` 都仍为 `Some`；`cur_is_dirty` 指向的迭代器当前键与 `current_value` 对应；关闭后两个子迭代器都被 `take`，整体失效（`interceptor.rs:224-233,354-404`）。

表键编码为字节 `t` 加上“将有符号位翻转后的 i64 当作 u64”的大端八字节表示，从而保持按有符号 ID 的字节排序。`get_range_accessed_table_id` 只认可终点仍以同一表前缀开头，或恰好等于下一表前缀的半开范围（`interceptor.rs:713-742`）。

## 依赖与调用关系

直接 Rust 依赖来自同 crate 的 `infoschema` 模块：`InfoSchema` 提供按 ID 查表和临时表存在性，`SessionVarsProvider` 暴露会话变量，`TableInfo/TempTableType` 决定路由，`TempTableError` 统一错误类型（`infoschema.rs:59-66,83-92,122-149,272-277,374-384`）。标准库依赖包括 `Arc` 共享所有权、`RwLock` 保护会话映射、`BTreeMap` 提供有序扫描、`HashMap` 承载批量结果和 `Any` 支持测试中的具体迭代器类型断言。

文件内部主要调用链为：`SnapshotInterceptor::on_get -> temporary_table_info_by_id -> get_session_key`；`on_batch_get -> batch_get_temporary_table_keys -> get_session_key`；`on_iter -> iter_table/create_union_iter -> UnionIter::new -> update_cur`；`on_iter_reverse -> create_union_iter -> UnionIter::new`。RustCodeGraph 的 `callees` 查询确认 `get_session_key` 下调 `Retriever::get` 与 `ValueEntry::is_value_empty`，`create_union_iter` 下调正/反向迭代、`close_iterator` 和 `UnionIter::new`。

上游方面，`lib.rs` 将本模块全部公开再导出；`interceptor_test.rs` 是当前仓库唯一精确使用这些 Rust API 的文件。RustCodeGraph 的 `callers` 对关键符号未返回调用边，`rg` 复核也未找到生产调用点。`pkg/store/driver/txn/snapshot.rs` 另有自己的 `SnapshotInterceptor`，两者当前不是同一个 trait，后续接线必须显式适配或改用真实 KV 类型，不能只依赖同名符号。

## 错误处理与边界

- `MemBuffer::get` 对缺失键返回 `KeyNotExist`；`get_session_key` 还把空值墓碑、无会话数据和全局临时表都转换为 `KeyNotExist`，但误将普通表交给会话读取会返回 `NormalTableSessionRead(table_id)`（`interceptor.rs:140-148,643-661`）。
- `delete_table_key` 会核对键内 table ID；不匹配时返回 `Store` 错误，避免把其他表的键写入删除墓碑（`interceptor.rs:109-123`）。
- 批量读取吞掉单键 `KeyNotExist`，传播其他会话错误；底层 `batch_get` 错误原样传播（`interceptor.rs:517-525,565-579`）。
- `create_union_iter` 先创建快照侧，再创建会话侧。会话迭代器创建失败时关闭快照侧；`UnionIter::new` 的首次 `update_cur` 失败时关闭两侧。已成功返回的迭代器由调用者负责 `close`（`interceptor.rs:235-263,670-710`）。
- `VecIterator::key/value` 假定调用者先检查 `valid`；无效位置直接调用会索引越界。`UnionIter` 的内部 `unwrap/expect` 也依赖其状态不变量。`RwLock/Mutex::lock().unwrap()` 在锁中毒时会 panic，而非转换为 `TempTableError`。
- `get_range_accessed_table_id` 计算 `table_id + 1`；由于 `get_key_accessed_table_id` 排除了 `i64::MAX`，该加法不会溢出（`interceptor.rs:727-742`）。

## 并发与资源生命周期

公开的 `Retriever`、`Snapshot`、`SnapshotInterceptor` 均要求 `Send + Sync`，迭代器要求 `Send`。`TemporaryTableSnapshotInterceptor` 通过 `Arc<dyn InfoSchema>` 和可选 `Arc<dyn Retriever>` 共享只读接口；`session_snapshot_interceptor` 只在构造时短暂锁定 `temporary_table_data`，克隆 `Arc` 后即释放锁（`interceptor.rs:68-79,458-484,622-641`）。

`MemBuffer` 的点查和范围快照受 `RwLock` 保护；范围迭代使用创建时的克隆快照，因此不会与后续会话写入发生迭代期间的数据竞争，但也不提供“实时看到新写入”的语义。`UnionIter::close` 对两侧各调用一次 `close` 并移走所有权，重复关闭不会再次触达子迭代器；`closed` 只有在两侧都已移走时为真（`interceptor.rs:391-403`）。`EmptyIterator` 只记录自身关闭标志；它始终 `valid == false`。

独立测试 `test_error_create_union_iter` 注入正向/反向的快照创建、会话创建和首次推进错误，并检查已经创建的子迭代器均关闭；这是本文件资源生命周期最直接的回归证据（`interceptor_test.rs:1137-1267`）。

## 与 Go 版本的对应关系

Rust 主要入口逐一对应 Go `pkg/table/temptable/interceptor.go`：`session_snapshot_interceptor` 对应 `SessionSnapshotInterceptor`，`TemporaryTableSnapshotInterceptor` 及四个 `on_*` 方法对应 Go 同名类型及 `On*` 方法，`get_session_key`、`create_union_iter` 和三个范围/键判断函数也保持相同分支次序。Rust 测试名称与 Go `interceptor_test.go` 的 13 个测试主题一一对应，覆盖键解析、范围判断、本地/全局/普通表点查、批量拆分、合并迭代、错误清理以及正反向扫描。

语义上保持的要点包括：全局临时表读为空；本地临时表只读会话数据；空值是删除墓碑；同键时会话值覆盖快照；批量合并时临时表结果覆盖异常重复的快照结果；迭代器创建失败时关闭已创建资源。

当前 Rust 实现并非机械复用生产组件：Go 直接使用 `kv.Key/ValueEntry/Retriever/Snapshot/Iterator`、`txn.NewUnionIter`、`tablecodec` 与真实 `sessionctx.Context`；Rust 文件目前自定义对应类型、内置 `UnionIter` 和表前缀编解码，并使用简化的 `infoschema.rs` 模型。Go 的 API 还接受 `context.Context` 和 Get/BatchGet options，Rust trait 没有这些参数。这些是接入生产链时必须处理的迁移差异，而不是可以忽略的命名差异。

## 扩展指南

- 新增或改变点查/批量读取策略时，优先修改 `get_session_key`、`batch_get_temporary_table_keys` 和相应 `on_*` 方法，并在独立的 `interceptor_test.rs` 扩展 `test_get_session_temporary_table_key`、`test_interceptor_on_get`、`test_interceptor_batch_get_temporary_table_keys` 或 `test_interceptor_on_batch_get`；不要把测试内嵌回生产文件。
- 改变扫描合并、删除墓碑或同键优先级时，集中修改 `UnionIter::update_cur`，同步覆盖正向与反向顺序、同键覆盖、只剩一侧和空值跳过。需要特别防止重复键、漏推进或反向比较翻转错误。
- 增加新的表类型或会话数据来源时，审查 `temporary_table_info_by_id`、`get_session_key`、`iter_table` 和 `session_snapshot_interceptor` 的所有分支，保证点查、批量与范围语义一致。
- 修改表键编码必须与 Go `tablecodec.EncodeTablePrefix/DecodeTableID/TablePrefix` 保持兼容，并同步 `test_get_key_accessed_table_id`、`test_get_range_accessed_table_id`、`test_not_table_range` 的边界用例；错误编码会把普通持久化数据误路由进会话，是高正确性风险。
- 若要接入真实 Rust 事务快照链，应先决定复用/适配 `pkg/store/driver/txn/snapshot.rs` 的 trait 和类型，并启用明确的 Cargo 依赖；还需补生产调用点与集成测试。不能仅把当前自包含 trait 改名后宣称接入完成。性能上应关注 `MemBuffer::entries` 全量克隆范围、批量临时键逐个 `get` 以及 `UnionIter` 每步克隆 `ValueEntry` 的成本。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引中有 `pkg/table/temptable/interceptor.rs`；`node --file ... --offset 1/501` 读取了目标文件全部 749 行；`query` 定位 `TemporaryTableSnapshotInterceptor`、`session_snapshot_interceptor`、`get_session_key`、`create_union_iter`、`UnionIter` 和 `get_key_accessed_table_id`；`callees` 核对了 `get_session_key` 与 `create_union_iter` 的下游边。关键符号的 `callers` 无返回，因此又以仓库精确搜索复核上游。
- 源码与边界：完整核对 `pkg/table/temptable/interceptor.rs`；读取 `pkg/table/temptable/infoschema.rs` 中 `TempTableType`、`TableInfo`、`TempTableError`、`InfoSchema`、`SessionVariables` 和 `SessionVarsProvider` 的定义；读取 `pkg/table/temptable/lib.rs` 确认模块声明与公开再导出。
- crate 与接线：读取 `pkg/table/temptable/Cargo.toml`，确认正常依赖为空而历史真实依赖位于 `cfg(any())`；用 `rg` 搜索关键 Rust API，确认除目标文件外只在 `pkg/table/temptable/interceptor_test.rs` 出现，且真实事务快照模块有另一套同名 trait。
- Go 对照与测试：完整读取 `pkg/table/temptable/interceptor.go`；核对 `pkg/table/temptable/interceptor_test.go` 的测试清单及错误清理用例；读取 `pkg/table/temptable/interceptor_test.rs` 的全部测试主题与关键点查、批量、合并、资源清理、正反向扫描段落。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行任务指定的 11 章节结构命令，并人工复核本说明只陈述以上源码、图查询、Cargo、Go 对照与独立测试能够支持的事实。
