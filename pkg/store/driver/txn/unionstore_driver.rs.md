# `pkg/store/driver/txn/unionstore_driver.rs`

## 文件定位

该文件属于 `astersql-store-driver-txn` crate（`pkg/store/driver/txn/Cargo.toml`），由 `lib.rs` 的 `mod unionstore_driver` 装配并整体再导出。它位于事务适配层的本地写集一侧：`txn_driver.rs::tikvTxn` 持有 `Arc<memBuffer>`，事务点写、提交、语句回滚、范围扫描和流水线 DML flush 都经由这里；已提交数据则由 `snapshot.rs` 负责，二者的扫描结果最终交给 `union_iter.rs::NewUnionIter` 合并。

与同路径 Go 文件不同，Go 的 `memBuffer` 主要包装 `tikv.MemBuffer` 并转换 TiDB/TiKV 类型和错误；当前 Rust 文件直接用 `BTreeMap`、`RwLock` 实现了一套进程内缓冲语义。因此它既是适配边界，也是当前 Rust 事务本地数据结构的实际实现，而非单纯门面。

## 核心职责

- 以字典序保存事务未提交键值；空值是删除墓碑，`RemoveFromBuffer` 才是物理删除。
- 保存并转换键级提交元数据：`KeyFlags`、`FlagsOp`、`AssertionOp` 与 `TiKVFlagsOp`。
- 提供点查、批量查、正反向范围扫描，以及供事务联合读使用的 `Getter`、`BatchBufferGetter`、`KvIterator` 接口。
- 用 `Staging`/`Cleanup`/`Release` 支持嵌套语句暂存，用 `MemDBCheckpoint` 支持事务驱动的整表检查点回滚。
- 在 pipelined DML 模式下让 `SnapshotIter*` 和 `SnapshotGetter` 返回空视图，并通过 `Flush`/`set_flush_error` 暴露当前 Rust 实现的 flush 行为。
- 为 Go 兼容层保留命名和轻量包装，如 `tikvGetter`、`tikvIterator`、`newKVGetter` 以及标志映射函数。

## 主要符号

- `KeyFlags(u16)`：六个内部位中，对外可查询 presume-not-exists、need-locked、三态断言和 prewrite constraint-check；`bits` 可取得原始值。`PREVIOUS_PRESUME_NOT_EXISTS` 可写入但没有单独查询方法。
- `FlagsOp` / `AssertionOp` / `TiKVFlagsOp`：分别表示普通标志操作、互斥断言操作和 TiKV client 侧操作。`apply_flag_ops` 只做置位；`apply_assertion` 保证 Exist/NotExist 的单态或双位 Unknown，并由 `AssertNone` 清除两位。
- `BufferValue`：把原始 `Vec<u8>` 与 `KeyFlags` 绑定；空 `value` 仍是有效条目，表示 tombstone。
- `Stage`：记录栈深句柄及进入该层前的完整 `entries` 副本。`MemBufferState` 统一保存条目、暂存栈和一次性 flush 错误。
- `MemDBCheckpoint`：独立复制完整键、值和标志，用于 `tikvTxn::GetMemDBCheckpoint` / `RollbackMemDBToCheckpoint`。
- `memBuffer`：核心类型。`RwLock<MemBufferState>` 保护可变状态，`is_pipelined_dml` 在构造后不变。
- `newMemBuffer`、`empty`、`from_entries`：构造入口。`newMemBuffer(None, ..)` 返回 `None`；从初始条目构造时所有标志归零。
- `Set` / `SetWithFlags` / `Delete` / `DeleteWithFlags` / `UpdateFlags` / `UpdateAssertionFlags`：写入入口。`set_internal` 保留旧标志再叠加普通操作。
- `Get` / `GetLocal` / `BatchGet` / `GetFlags`：读取入口；`Getter::get` 和 `BatchBufferGetter::batch_get_bytes` 是 trait 实现。
- `Iter` / `IterReverse` / `SnapshotIter*` / `SnapshotGetter`：有序扫描与只读快照视图入口，底层输出 `tikvScanner` 或 `tikvGetter`。
- `Staging` / `Cleanup` / `Release` / `InspectStage`：暂存栈管理及相对某层的差异枚举。
- `checkpoint` / `revert_to_checkpoint` / `entries`：crate 内事务驱动所用的整表复制接口。
- `Flush`：消费一次 `flush_error`，否则返回当前条目数；它没有把数据发送给外部 TiKV。
- `getTiDBKeyFlags` / `getTiKVFlagsOp(s)` / `getTiKVAssertionOp`：两侧标志语义的显式映射。

## 执行流程

1. `txn_driver.rs::NewTiKVTxn` 根据 `pipelined` 创建 `memBuffer::empty`，并以 `Arc` 存入 `tikvTxn`。
2. 写入时，`Set*` 拒绝空值，`Delete*` 则有意写入空值；`set_internal` 在写锁内取得旧标志、叠加新操作并替换条目。只修改标志时，`UpdateFlags`/`UpdateAssertionFlags` 会为不存在的键创建默认空值条目。
3. 点查直接从 `entries` 克隆值；缺失返回 `DriverError::NotFound`。批量查只收集存在键，不为缺失键报错。当前本地实现接受但忽略 get/batch-get options，返回的 `commit_ts` 固定为 0。
4. 正向 `Iter` 选择 `[key, upper_bound)`，反向 `IterReverse` 选择 `[lower_bound, key)`，并先复制匹配行再构造 `tikvScanner`。`tikvTxn::Iter` 把该 dirty iterator 与 snapshot iterator 交给 `NewUnionIter`；后者让本地写覆盖同键快照，空值墓碑屏蔽快照值。
5. `Staging` 复制当前整张表并以当前深度加一作为句柄。栈顶 `Cleanup` 恢复副本，栈顶 `Release` 仅丢弃副本；句柄 0 和超出当前深度的 cleanup 是无操作，非栈顶有效句柄会 panic。释放外层后，新外层可复用句柄 1。
6. `InspectStage` 比较当前表与指定层的 `before`：新增或变化条目回调当前值和标志，已物理移除条目回调默认标志与空值。找不到句柄时直接返回。
7. `tikvTxn::Commit` 通过 `entries` 复制完整写集，先做 `TiDBKVFilter` 检查，再把非空值写入共享 storage、把空值作为删除处理。检查点接口同样整表复制/替换，但不会修改 staging 栈。
8. pipelined DML 下，事务的 `MayFlush` 调用 `Flush`；本文件只返回条目数或一次性注入错误。与此同时 `SnapshotIter*`/`SnapshotGetter` 返回空对象，以匹配 Go 包装器在该模式下不暴露 MemBuffer snapshot 的分支。

## 数据与状态

`entries: BTreeMap<Key, BufferValue>` 同时提供稳定字典序和单键查找。键和值均为自有字节向量；所有对外读取、扫描、快照、检查点和提交导出都复制数据，因此返回对象不借用锁内存，也不会随以后写入改变。代价是扫描和快照均为 O(n) 级复制，嵌套 staging 每层也复制整表。

墓碑与缺失必须区分：缺失键产生 `NotFound`，存在但空值的键可由点查/扫描读到空字节，并由 `UnionIter`/提交路径解释为删除。`Size` 统计所有条目的键长和值长（包括墓碑键），`Len` 统计条目数。

普通 flag 更新是单调置位；断言位例外，会互斥替换、设置双位 Unknown 或全部清除。检查点保存值和 flags，但 `revert_to_checkpoint` 不恢复/清空 `stages` 或 `flush_error`。`flush_error` 被 `Flush` 用 `take` 消费，故只影响下一次 flush。

## 依赖与调用关系

- crate 内依赖来自 `lib.rs`：`DriverError`、`Key`、`ValueEntry`、`GetOption`、`BatchGetOption` 及 `Getter`/`BatchBufferGetter`/`KvIterator` trait；扫描器依赖 `scanner.rs::tikvScanner`。
- 直接上游是 `txn_driver.rs`：`NewTiKVTxn -> memBuffer::empty`，写入大小检查后调用 `Set`，`Commit -> entries`，检查点方法调用 `checkpoint`/`revert_to_checkpoint`，`Iter` 调用缓冲扫描，`GetMemBuffer` 克隆 `Arc`，`MayFlush -> Flush`。
- 范围读的直接下游是 `union_iter.rs::NewUnionIter`：dirty 条目优先于 snapshot，同键 tombstone 会同时推进两侧。
- `Cargo.toml` 声明该 crate 依赖 `astersql-kv`、`astersql-tablecodec`、`astersql-errors` 和固定 tag `v0.4.2-aster.10` 的 `tikv-client`。不过本文件本身没有直接引用外部 `tikv_client` 类型；当前映射目标是本地 `TiKVFlagsOp` 枚举。
- RustCodeGraph 索引显示本文件被 46 个文件使用，并识别到 `GetMemBuffer`、`GetFlags`、`SetWithFlags`、`UpdateFlags`、`DeleteWithFlags`、`Staging` 等上游；精确 Rust 接线再由 `txn_driver.rs` 与 `lib.rs` 源码核实。

## 错误处理与边界

- `Set`/`SetWithFlags` 对空值返回 `DriverError::Backend("cannot set an empty value")`，调用方必须使用 `Delete*` 表达墓碑。
- `Get`、`GetLocal`、`GetFlags` 和 `tikvGetter::get` 对缺失键返回 `DriverError::NotFound`；`BatchGet` 忽略缺失键。
- 所有锁中毒都用 `PoisonError::into_inner` 继续访问状态，而不是把中毒转换为业务错误。因此此前 panic 不会永久封死缓冲，但调用方也收不到锁中毒信号。
- staging 深度不能转换为 `i32` 时 panic；对非栈顶有效句柄执行 `Cleanup`/`Release` 也 panic，这是栈式 API 的不变量。`Release` 对大于深度的句柄同样 panic，而 `Cleanup` 对大于深度的句柄无操作。
- `InspectStage` 只报告条目最终差异，不报告中间修改次数；把键物理删除和写入空墓碑会形成不同的内部状态，但回调值都可能为空，调用方需结合 flags/业务约定理解。
- 范围上界均为排他；反向扫描的 `key=None` 表示无上界。扫描器构造当前不返回后端错误，但公共签名保留 `Result` 以适配统一接口。
- `getTiKVFlagsOp` 和 `getTiKVAssertionOp` 对 Rust 枚举穷举匹配；未来添加枚举成员会触发编译期补齐，而不是默默映射为零值。

## 并发与资源生命周期

`memBuffer` 可通过 `Arc` 在调用者间共享，全部内部可变状态由一个 `std::sync::RwLock` 保护：读、大小统计和复制视图获取读锁，写入、暂存栈操作、回滚及 flush 获取写锁。单次方法内状态变化是原子的，但多次方法组合没有跨调用事务保证；例如先 `GetFlags` 后 `Set` 之间可被其他线程修改。

访问者回调不会在持有写锁时运行，但 `InspectStage` 在整个回调循环期间持有读锁；回调若同步尝试写同一缓冲，可能自我阻塞，因此扩展时不应在 visitor 中回入写 API。`Iter*` 和 `SnapshotGetter` 在锁内复制后立即释放锁，返回对象独立拥有数据。`tikvIterator::close` 把关闭动作转发给底层迭代器；`tikvScanner` 的具体资源语义由 `scanner.rs` 定义。

`Stage`、checkpoint 和 snapshot getter 都是内存副本，没有外部句柄或后台任务。`Flush` 也没有异步任务/网络资源；它只是当前 Rust 事务模拟中的计数与错误注入点。

## 与 Go 版本的对应关系

对应文件为 `pkg/store/driver/txn/unionstore_driver.go`。公开方法集合和关键分支基本逐项对应：nil 构造、普通/带 flag 写删、点查/批量查、正反向迭代、staging、snapshot 视图、getter/iterator 包装与 flag 映射。`unionstore_driver_test.rs` 还专门验证 `AssertNone` 清位、句柄按深度复用以及非栈顶 cleanup/release panic，以对齐 client-go 行为。

实现层存在重要差异：Go 将调用委托给 `tikv.MemBuffer`，并用 `derr.ToTiDBErr` 统一转换错误；Rust 直接操作 `BTreeMap`，没有 context 参数，也没有外部错误转换。Go 的 snapshot/flush 语义来自 client-go MemDB/KVTxn；Rust 的 snapshot 是即时复制，`Flush` 不落盘、不清空缓冲且只返回长度。Rust 额外暴露 `empty`、`from_entries`、checkpoint、错误注入和提交导出，供本地 `tikvTxn` 实现使用。

当前 Rust `Get`/`BatchGet` 忽略 options；Go 会原样下传。Go `InspectStage` 由 TiKV MemBuffer 决定精确变化集合，Rust 通过完整映射比较计算。文档和扩展设计应把这些视为当前实现边界，不应声称 Rust 已具备 client-go MemDB 的所有内部性能或持久化行为。

## 扩展指南

- 新增键级普通标志时，应同步修改位常量、`FlagsOp`、`TiKVFlagsOp`、`apply_flag_ops`、查询方法以及 `getTiDBKeyFlags`/`getTiKVFlagsOp`；断言语义必须继续走 `AssertionOp`，避免混入普通 flag 操作。
- 修改写删语义时，应同时检查 `txn_driver.rs::commit_inner` 和 `union_iter.rs::update_cur` 对空值墓碑的解释，不能把 tombstone 与缺失合并。
- 修改 staging 时要保持栈顶约束、handle 0/越界行为和句柄复用语义；测试应放在独立的 `unionstore_driver_test.rs`，不要内嵌到生产文件。
- 若把 full-clone staging/scan 替换成增量结构，应重点验证稳定字典序、`InspectStage` 的删除报告、嵌套 cleanup/release、并发读写以及返回迭代器脱离锁后的有效性。
- 若实现真实 pipelined flush，接入点是 `memBuffer::Flush` 与 `txn_driver.rs::MayFlush`，还需明确成功后条目生命周期、重复 flush、部分失败和后台资源关闭；不能仅改变返回计数。
- 若支持 get options/commit timestamp，应修改 `Getter`/`BatchBufferGetter` 实现并与 `lib.rs` 中 `apply_commit_ts_option*` 的规则一致。
- 最小同步测试面包括 `unionstore_driver_test.rs`（本文件局部语义）、`txn_driver_test.rs`（提交、检查点、flush 和事务接线）及 `union_iter_test.rs`（墓碑覆盖和扫描顺序）。兼容风险主要在 Go/client-go 行为差异，性能风险主要来自整表复制和长时间持有 `InspectStage` 读锁。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点，目标文件共 732 行；`explore "pkg/store/driver/txn/unionstore_driver.rs memBuffer MemDBCheckpoint newMemBuffer"` 给出符号、调用范围及主要上游；`query memBuffer`、`query newMemBuffer` 消除 Go/Rust 同名歧义；三段 `node --file ...` 核对目标文件全部源码。
- 源码：`pkg/store/driver/txn/unionstore_driver.rs`；模块与接口边界：`pkg/store/driver/txn/lib.rs`；直接事务接线：`pkg/store/driver/txn/txn_driver.rs`；扫描合并：`pkg/store/driver/txn/union_iter.rs`。
- crate 证据：`pkg/store/driver/txn/Cargo.toml` 的 package、lib path、porting metadata 与依赖声明。
- Go 对照：`pkg/store/driver/txn/unionstore_driver.go`、`pkg/store/driver/txn/txn_driver.go`。
- 独立测试：`pkg/store/driver/txn/unionstore_driver_test.rs` 验证断言清除与 staging 栈约束；关联行为测试入口由 `lib.rs` 分别装配 `txn_driver_test.rs` 和 `union_iter_test.rs`。同目录没有独立的 `unionstore_driver_test.go`，因此 Go 语义主要由实现和 client-go 委托关系核对。
- 本任务只新增文档，按计划不运行 Cargo；交付前以固定 11 章节命令做结构验证，并人工检查所有行为陈述均可回溯到上述符号或文件。
