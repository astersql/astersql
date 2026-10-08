# `pkg/store/driver/txn/scanner.rs`

## 文件定位

本文件属于 `astersql-store-driver-txn` crate（见 `pkg/store/driver/txn/Cargo.toml`），是事务驱动层的键值扫描器适配器。`pkg/store/driver/txn/lib.rs` 以 `mod scanner` 装配该模块，并通过 `pub use scanner::*` 导出其中的 `tikvScanner`。它把调用方已经按扫描顺序物化好的 `Vec<(Key, Vec<u8>)>` 暴露为 crate 统一的 `KvIterator` trait，而不负责访问 TiKV、计算扫描边界或排序。

在当前 Rust 调用链中，`tikvSnapshot::Iter` / `IterReverse`、`memBuffer::Iter` / `IterReverse` / `SnapshotIter` / `SnapshotIterReverse` 会先在各自模块中选择并排列行，再构造本文件的扫描器；`tikvTxn::Iter` / `IterReverse` 随后把快照扫描器与 dirty memBuffer 扫描器交给 `NewUnionIter` 合并。因此，本文件位于“范围行已生成”和“事务层合并/消费迭代器”之间。

## 核心职责

- 保存一组拥有所有权的键值行，并以 `position` 指向当前行；新建非空扫描器后当前行就是索引 0，调用方可立即通过 `valid`、`key`、`value` 读取它。
- 实现 `KvIterator` 的前进、当前键值访问、有效性判断和关闭协议，使快照、内存缓冲与 `UnionIter` 可以通过 `Box<dyn KvIterator>` 使用同一接口。
- 通过 `with_next_error` 在指定游标位置安装一次性的 `DriverError`，模拟底层扫描在前进时失败。
- 提供 Go 风格的 `Next`、`Key`、`Value`、`Valid`、`Close` 方法，作为同名 trait 方法的薄包装；其中 `Key` 和 `Value` 返回拥有所有权的副本。

本文件不决定正向/反向顺序、不解释空值为删除标记、不合并 dirty/snapshot 数据，也不把 TiKV 客户端错误转换为 `DriverError`；这些职责分别位于构造者、`union_iter.rs` 和更外层驱动代码。

## 主要符号

- `pub struct tikvScanner`：扫描器状态。名称沿用 Go 版本，crate 根允许 `non_camel_case_types`。
  - `rows: Vec<(Key, Vec<u8>)>`：已物化、已排好遍历顺序的键值行；`Key` 是 crate 根定义的 `Vec<u8>` 别名。
  - `position: usize`：当前行索引。初值为 0；等于 `rows.len()` 时表示扫描结束。
  - `closed: bool`：显式关闭标志。关闭后即使仍有行，扫描器也无效。
  - `next_errors: HashMap<usize, DriverError>`：按游标位置保存的确定性、一次性前进错误。
- `tikvScanner::new(rows) -> Self`：取得整组行的所有权，初始化到首行、未关闭、无注入错误状态。它不校验或重排输入。
- `tikvScanner::with_next_error(position, error) -> Self`：builder 风格注入；相同位置再次插入会覆盖旧错误。
- `Next` / `Key` / `Value` / `Valid` / `Close`：保留 Go 命名的公开门面。`Next`、`Valid`、`Close` 直接委托 trait 实现，`Key`、`Value` 将借用切片复制为 `Vec<u8>`。
- `impl KvIterator for tikvScanner`：真实迭代协议实现：`next`、`key`、`value`、`valid`、`close`。

文件没有模块级常量、额外 trait、条件编译项或异步函数。

## 执行流程

1. 上游先生成有序行。正向快照扫描由 `snapshot.rs::tikvSnapshot::range_rows` 生成 `[key, upper_bound)` 的行，反向扫描由 `range_rows_reverse` 生成逆序行；`unionstore_driver.rs::memBuffer` 有对应的 `rows` / `rows_reverse` 路径。
2. 上游把结果传给 `tikvScanner::new`。若 `rows` 非空，`valid()` 立即为真，当前 `key()` / `value()` 指向第 0 行；若为空则立即无效。
3. 消费者遵循 `while iter.valid() { read key/value; iter.next()?; }`。`snapshot_test.rs::check_iter`、`pkg/store/driver/snap_interceptor_test.rs::check_iter` 和 `pkg/store/driver/txn_test.rs::TestTxnScan` 都以此顺序使用接口。
4. `next()` 首先拒绝已关闭或已经越过末尾的扫描器，返回 `DriverError::Backend("iterator is invalid")`。
5. 若 `next_errors` 在当前 `position` 有条目，`next()` 先通过 `remove` 取出并返回该错误，游标不移动；同一位置的下一次调用不会再次得到该错误，而会正常前进。
6. 无错误时 `position += 1`。前进到 `rows.len()` 会成功返回 `Ok(())`，之后 `valid()` 为假；只有再调用一次 `next()` 才返回 invalid 错误。
7. `close()` 只把 `closed` 设为真。之后 `valid()` 为假、`next()` 返回 invalid；重复关闭仍安全，因为赋值是幂等的。

## 数据与状态

扫描器完全拥有 `rows`，因此不借用快照锁，也不会观察构造之后存储中的变化。键值内容在构造时已经复制/收集；trait 的 `key()`、`value()` 返回对当前行内部字节的借用，借用期受 `&self` 约束，而 Go 风格 `Key()`、`Value()` 每次复制当前字节。

核心状态不变量为：有效状态等价于 `!closed && position < rows.len()`。`key()` 和 `value()` 使用同一个 `position` 查询，所以有效时必然来自同一行；无效时两者都通过 `unwrap_or_default()` 返回空切片，而不是报错。由此不能用空键或空值判断迭代器是否有效，调用者必须先检查 `valid()`；同时，合法行的值可以为空，文件注释明确空值行仍会暴露给上层。

`with_next_error` 的位置是调用 `next()` 时的“当前行位置”，不是下一行索引。错误被消费后游标保持原位，调用者可读取相同当前行或重试前进。`HashMap` 允许预装多个不同位置的错误，但当前仓库文本搜索未发现该方法的外部调用，因而这项注入能力目前没有直接回归测试证据。

## 依赖与调用关系

直接源码依赖只有标准库 `HashMap` 与 crate 根的 `DriverError`、`Key`、`KvIterator`。`Cargo.toml` 表明 crate 还依赖 `astersql-kv`、`astersql-tablecodec`、`astersql-errors` 和带固定 tag `v0.4.2-aster.10` 的 `tikv-client`，但本文件没有直接调用这些外部 crate；远端读取或键错误翻译不发生在这里。

已核实的上游构造点包括：

- `snapshot.rs::tikvSnapshot::Iter` 与 `IterReverse`；拦截器未接管时，将范围结果装入 `Box<dyn KvIterator>`。
- `unionstore_driver.rs::memBuffer::Iter` 与 `IterReverse`；扫描事务写缓冲。
- `unionstore_driver.rs::memBuffer::SnapshotIter` 与 `SnapshotIterReverse`；流水线 DML 时构造空扫描器，否则构造物化扫描器。

下游主要通过 `KvIterator` 动态分派访问。`txn_driver.rs::tikvTxn::Iter` / `IterReverse` 同时取得 memBuffer 和 snapshot 两侧迭代器，再交给 `union_iter.rs::NewUnionIter`；后者读取 `valid/key/value` 并调用 `next` 合并有序数据。RustCodeGraph 能识别本文件 14 个符号及文件被使用关系，但对 `tikvScanner` 和 `with_next_error` 的精确 callers/callees 查询返回空边；上述接线因此由模块源码与 `rg` 引用位置补证，而不是把空图结果解释为“没有调用者”。

## 错误处理与边界

- `new` 本身不失败，也不验证顺序、重复键或边界；调用者传入什么顺序，扫描器就按什么顺序暴露。
- 对空 `rows`，`valid()` 为假，`key()` / `value()` 为空切片，首次 `next()` 返回 `DriverError::Backend("iterator is invalid")`。
- 正常消费最后一行后调用 `next()` 会成功把游标推进到末尾；末尾状态下再次 `next()` 才报 invalid。这与仓库测试中的 `while valid { ...; next().unwrap(); }` 模式配套。
- `close()` 不返回错误且不释放/清空 `rows`；关闭后的读取方法仍返回空切片，不能取回旧的当前行。
- 注入错误优先于游标递增，并通过 `HashMap::remove` 保证一次性。若为越界位置注入错误，该错误永远不可达，因为 invalid 检查先于错误表查询。
- `Key()` / `Value()` 在无效状态返回空 `Vec`，不会 panic；代价是复制当前键值，热路径应优先通过 trait 的借用接口消费。

范围边界不是本文件实现的。现有 `snapshot_test.rs::TestSnapshotWithoutInterceptor` 证明正向扫描下界包含、上界排除，以及反向扫描的上界排除/下界包含语义；这些结果在传入 `tikvScanner` 前已经由 `snapshot.rs` 形成。

## 并发与资源生命周期

`KvIterator: Send` 要求扫描器可在线程间转移，但接口的变更操作都要求 `&mut self`，本类型自身没有锁、原子量或共享所有权，也没有声明 `Sync`。一个扫描器应由单一消费者串行推进；若需跨线程共享，必须由上层提供同步。

构造时扫描器取得 `rows` 所有权，整个迭代期间不持有 `tikvSnapshot` 的 `RwLock` 守卫，也没有网络连接、异步任务或通道。`close()` 仅改变逻辑状态；真正的内存释放发生在扫描器被 drop 时。该行为适合当前物化实现，但不同于可能需要显式释放远端扫描资源的实现，未来替换后不能假设 `close` 永远只是布尔赋值。

作为 `UnionIter` 的子迭代器时，其生命周期由 `union_iter.rs` 管理。相关 `union_iter_test.rs` 使用 `TrackedIter` 验证联合迭代器关闭两侧资源且重复关闭安全；这验证的是共同的 `KvIterator` 生命周期契约，不是 `tikvScanner::close` 的专门测试。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/store/driver/txn/scanner.go`。两者都定义私有命名风格的 `tikvScanner`，向上层提供迭代器接口，并让 `Key` 返回 TiDB 键类型。

实现方式存在重要差异：Go 结构体内嵌 `*txnsnapshot.Scanner`，`Next()` 调用真实 client-go scanner 并通过 `extractKeyErr` 转换错误；`Key()` 直接转换底层 scanner 的当前键，其余 `Value`、`Valid`、`Close` 行为来自内嵌类型。Go 的 `snapshot.go::Iter` / `IterReverse` 调用 `KVSnapshot` 创建底层 scanner 后再包装。

Rust 版本则保存已经物化的 `Vec`，所有迭代操作都在内存中同步完成；它没有包裹 `tikv-client` 的 scanner，也没有在 `next` 中执行远端请求或等价的 `extractKeyErr`。Rust 的 `snapshot.rs` 与 `unionstore_driver.rs` 在构造前完成范围选择和排序。`with_next_error` 是 Rust 额外提供的确定性错误注入能力，Go 文件没有同名机制。故当前 Rust 行为对齐的是上层 `KvIterator` 消费协议和扫描结果语义，而不是 Go 底层 scanner 的惰性网络执行模型。

## 扩展指南

- 若增加分页、惰性拉取或真实 TiKV scanner，应优先修改 `tikvScanner` 的状态和 `KvIterator::next`，并同步审查 `snapshot.rs::Iter/IterReverse` 的构造方式。必须保持“新建后首行可读、末行前进成功后变 invalid”的现有消费协议，除非同时迁移全部调用者。
- 若改变范围或排序规则，应修改生成 `rows` 的 `snapshot.rs::range_rows(_reverse)` 或 `unionstore_driver.rs::rows(_reverse)`，不要在本文件重复过滤/排序；同时覆盖正向与反向边界。
- 若扩展错误注入，应明确错误是一次性还是持久、失败后游标是否移动，并为 `with_next_error` 新增独立 Rust 测试文件。按照仓库规则，测试不应内嵌在 `scanner.rs`；可新建同目录 `scanner_test.rs` 并从 `lib.rs` 的 `#[cfg(test)]` 区域接入，或在直接消费路径的现有独立测试中覆盖。
- 若修改关闭语义，应同步检查 `union_iter.rs` 的双侧关闭和错误路径，确保幂等关闭及部分初始化失败时的资源释放。
- 若改变 `Key` / `Value` 的所有权行为，应评估复制成本与 Go 风格公开 API 的兼容性；trait 借用方法和 Go 风格复制方法目前服务于不同调用习惯。
- 重点风险包括：把初始位置误改为“首行之前”造成跳过首行；在最后一次 `next` 上错误报 invalid 破坏现有循环；以空值代表无效而吞掉合法空值行；让注入错误推进游标导致重试丢行；为惰性扫描引入锁或网络资源后仍保留当前无操作式关闭。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，本文件在索引中有 14 个符号；`node --file pkg/store/driver/txn/scanner.rs` 读取了完整 108 行源码；对 `tikvScanner`、`with_next_error` 的 `callers` / `callees` / `impact` 精确查询未返回图边，因此调用关系另以源码引用核对。
- 目标实现：`pkg/store/driver/txn/scanner.rs`，重点符号为 `tikvScanner`、`new`、`with_next_error` 和 `impl KvIterator`。
- crate 边界：`pkg/store/driver/txn/Cargo.toml`；模块装配与接口契约：`pkg/store/driver/txn/lib.rs` 的 `mod scanner`、`pub use scanner::*`、`KvIterator`。
- 直接构造与主链：`pkg/store/driver/txn/snapshot.rs` 的 `tikvSnapshot::Iter/IterReverse`，`pkg/store/driver/txn/unionstore_driver.rs` 的 `memBuffer::Iter/IterReverse/SnapshotIter/SnapshotIterReverse`，`pkg/store/driver/txn/txn_driver.rs` 的 `tikvTxn::Iter/IterReverse`，以及 `pkg/store/driver/txn/union_iter.rs`。
- Go 对照：`pkg/store/driver/txn/scanner.go` 与 `pkg/store/driver/txn/snapshot.go`。
- Rust 测试证据：`pkg/store/driver/txn/snapshot_test.rs::TestSnapshotWithoutInterceptor`，`pkg/store/driver/snap_interceptor_test.rs::check_iter` 及扫描边界用例，`pkg/store/driver/txn_test.rs::TestTxnScan`，`pkg/store/driver/txn/union_iter_test.rs` 的遍历、错误传播和关闭契约用例。仓库搜索未发现直接构造 `tikvScanner` 或调用本类型 `with_next_error` 的独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构检查和人工事实复核作为验证。
