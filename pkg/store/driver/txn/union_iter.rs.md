# `pkg/store/driver/txn/union_iter.rs`

## 文件定位

本文件属于 `astersql-store-driver-txn` crate（`pkg/store/driver/txn/Cargo.toml`），实现事务读取路径中的联合迭代器。crate 根 `pkg/store/driver/txn/lib.rs` 通过 `mod union_iter` 装配模块、以 `pub use union_iter::*` 导出公开符号，并定义两侧共同遵循的 `KvIterator: Send` 接口。

它位于事务扫描的合并层：`txn_driver.rs::tikvTxn::Iter` 和 `IterReverse` 分别从事务 `mem_buffer` 与开始时快照取得两个已经按同一方向有序的 `Box<dyn KvIterator>`，再调用 `NewUnionIter`。本文件不访问 TiKV、不计算范围边界，也不负责给子迭代器排序；它只把“当前事务未提交写入”和“不可变快照数据”合并成一个读己之写、去重且过滤删除标记的有序视图。

## 核心职责

- 对两个有序 `KvIterator` 执行类似双路归并的扫描，正向时输出键字典序，反向时输出反字典序。
- 同键同时存在时选择 dirty 值，并提前推进 snapshot 侧，确保同一键只输出一次。
- 把 dirty 侧的空值解释为删除 tombstone：不向调用者输出该条目；若 snapshot 有同键记录，则同时跳过 snapshot 记录。
- 保存两侧当前有效性和当前选中来源，使 `key`、`value`、`valid` 与 `next` 组成统一的 `KvIterator` 协议。
- 负责已成功构造实例持有的两个子迭代器的关闭：显式 `close` 和 Rust `Drop` 都会关闭两侧，且重复关闭安全。
- 保留 `Next`、`Key`、`Value`、`Valid`、`Close` 这些 Go 风格公开方法；真正行为由 `KvIterator` trait 实现承载。

本文件假设两侧已按 `reverse` 指定的同一方向排序。它不验证排序、不处理范围上下界，也不区分“业务允许的空值”和 tombstone；在此事务适配层中，dirty 空值无条件代表删除。

## 主要符号

- `pub struct UnionIter`：联合迭代状态。
  - `dirty_it`、`snapshot_it: Option<Box<dyn KvIterator>>`：拥有两侧迭代器。使用 `Option` 是为了让 `close` 通过 `take()` 实现幂等释放，也允许构造失败时移出所有权而不调用 `close`。
  - `dirty_valid`、`snapshot_valid`：缓存两侧 `valid()` 结果，每次对应侧推进后刷新。
  - `cur_is_dirty`：当前可见项是否来自 dirty 侧；决定读哪侧以及下次推进哪侧。
  - `is_valid`：联合视图当前是否有效，只有两侧都耗尽后才被置为假。
  - `reverse`：是否按反向顺序比较；它只反转比较结果，不主动反转任何输入。
- `NewUnionIter(dirty_it, snapshot_it, reverse) -> Result<UnionIter, DriverError>`：读取两侧初始有效性，构造状态并立即调用 `update_cur` 定位首个可见项。定位失败时返回原始错误，且不关闭调用方传入的迭代器。
- `dirty_next` / `snapshot_next`：调用对应子迭代器的 `next()`，随后无论前进成功与否都刷新缓存的有效性，并原样返回错误。
- `update_cur`：核心归并状态机，负责耗尽判断、单侧选择、键序比较、同键覆盖和 tombstone 跳过。
- `dirty_key`、`dirty_value`、`snapshot_key`：内部借用访问器；要求对应 `Option` 尚未被关闭，否则以 `expect` 触发 panic。
- `Next` / `Value` / `Key` / `Valid` / `Close`：公开的 Go 风格包装。`Key`、`Value` 会复制当前切片；`Next`、`Valid`、`Close` 委托 trait 方法。
- `impl KvIterator for UnionIter`：实现借用式键值访问、推进、有效性和关闭协议。
- `impl Drop for UnionIter`：实例离开作用域时调用 `close`，补足 Rust 的自动资源清理。

文件没有模块级常量、自定义 trait、异步函数或条件编译项。

## 执行流程

1. `tikvTxn::Iter`（正向）或 `IterReverse`（反向）先创建 dirty 与 snapshot 两个子迭代器。快照构造失败时，事务层会关闭已经打开的 dirty 迭代器；两侧成功后才进入 `NewUnionIter`。
2. `NewUnionIter` 缓存两侧初始 `valid()`，把两个迭代器放入 `Option`，然后执行 `update_cur`。因此成功返回时已经定位到首个可见项，调用者无需先调用 `Next`。
3. `update_cur` 循环处理当前状态：
   - 两侧都无效：令 `is_valid = false` 并结束。
   - 只有 snapshot 有效：选择 snapshot。
   - 只有 dirty 有效：若 dirty 值为空则推进 dirty 并继续循环，否则选择 dirty。
   - 两侧都有效：比较 `dirty_key` 与 `snapshot_key`；反向扫描时把比较结果反转，从而仍可用同一分支选择遍历方向上的较早项。
4. 键相等时，dirty 非空则覆盖 snapshot：先推进 snapshot 去掉重复项，再选择 dirty；dirty 为空则表示删除，依次推进 dirty 和 snapshot，继续寻找下一个可见项。
5. 键不等时，`Ordering::Greater` 表示按当前遍历方向 snapshot 在前，选择 snapshot；`Ordering::Less` 表示 dirty 在前，空 tombstone 被推进并跳过，非空值被选中。
6. 消费者按 `while iter.Valid() { Key/Value; Next()? }` 读取。`KvIterator::next` 只推进当前选中的一侧，再调用 `update_cur` 重新归并定位。
7. 扫描完毕后，最后一次合法 `next` 返回 `Ok(())` 并使 `valid()` 为假；无效状态下再次 `next` 会返回 `DriverError::Backend("iterator is invalid")`。
8. 调用 `Close` 或实例被 drop 时，先关闭 snapshot、再关闭 dirty，并清空两侧 `Option` 与所有有效标志。

## 数据与状态

`UnionIter` 不复制两侧数据，只拥有 trait object 并借用其当前键值。trait 的 `key()` / `value()` 返回子迭代器内部切片，生命周期受 `&self` 限制；Go 风格 `Key()` / `Value()` 则通过 `to_vec()` 返回拥有所有权的副本。调用 `next` 后，之前借用的当前位置不应再被视为有效。

核心不变量是：成功定位后，`is_valid` 为真时 `cur_is_dirty` 指向一侧有效且可见的非 tombstone 项；两侧都无效时 `is_valid` 为假。相等键的 snapshot 会在 dirty 被暴露前推进，所以后续不会重复输出。dirty tombstone 永不成为可见当前位置，因此 `UnionIter::value()` 在有效状态下不会从 dirty 侧返回空值；snapshot 侧的值不由本文件过滤。

`dirty_valid` / `snapshot_valid` 是子迭代器状态的缓存，而非独立事实来源。每次 `dirty_next` / `snapshot_next` 都在调用 `next` 后刷新缓存，即使子迭代器返回错误也会刷新；发生错误时 `update_cur` 不会继续执行，调用者收到错误，联合状态不承诺已重新定位，应按错误终止或由上层明确决定后续处理。

`reverse` 是构造后不变的模式位。正确性依赖两侧输入与它一致：正向输入必须升序，反向输入必须降序。若传入无序数据或方向不一致，本文件可能丢失覆盖关系、错误排序或暴露本应被 tombstone 屏蔽的记录，因为源码没有防御性校验。

## 依赖与调用关系

本文件直接依赖标准库 `std::cmp::Ordering`，以及 crate 根导出的 `DriverError`、`Key`、`KvIterator`。`Cargo.toml` 声明该 crate 还依赖 `astersql-kv`、`astersql-tablecodec`、`astersql-errors` 和固定 tag `v0.4.2-aster.10` 的 `tikv-client`，但本文件没有直接调用这些外部 crate；存储读取与错误转换发生在子迭代器或上层驱动中。

生产上游是 `txn_driver.rs::tikvTxn::Iter` 与 `IterReverse`：两者分别调用 `mem_buffer.Iter/IterReverse` 和 `snapshot.Iter/IterReverse`，然后以 `reverse = false/true` 调用 `NewUnionIter`，最后把结果擦除为 `Box<dyn KvIterator>`。`lib.rs` 的公开再导出使构造器也可被 crate 外调用，但仓库内精确文本搜索到的非测试 Rust 生产构造点就是这两个方法。

RustCodeGraph 识别了本文件 20 个符号，并给出以下内部调用边：`NewUnionIter → update_cur/valid`，`update_cur → dirty_next/snapshot_next/dirty_key/dirty_value/snapshot_key`，trait `next → dirty_next 或 snapshot_next → update_cur`，`Key/Value/Valid/Close` 分别委托相应 trait 方法，`Drop::drop → close`。RustCodeGraph 对构造器的外部 `callers` 查询未返回边，因此外部入口以 `txn_driver.rs` 源码和 `rg` 引用位置补证，不能把空图结果解释为没有调用者。

## 错误处理与边界

- `NewUnionIter` 唯一可能失败的阶段是初次 `update_cur` 为跳过 tombstone 或去重而推进子迭代器。错误原样作为 `DriverError` 返回，不包装上下文。
- 构造失败时，源码先 `take()` 两个 `Option`，使临时 `UnionIter` 的 `Drop` 看不到子迭代器；移出的盒子随后正常析构，但不会调用其 `KvIterator::close`。这刻意对齐 Go 的“返回 nil/error 且不关闭调用方迭代器”契约，Rust 测试以关闭标志验证该行为。
- 已成功构造后的 `Next` 若底层推进失败，会立即传播错误，不自动关闭两侧。实例仍拥有子迭代器；调用方可以显式 `Close`，或让 `Drop` 最终关闭它们。
- 无效状态下 `next` 返回固定的 `DriverError::Backend("iterator is invalid")`。相对 Go 版本，这是 Rust 增加的显式保护；Go 的 `Next` 不先检查联合 `isValid`。
- 无效状态下 `key()` / `value()` 返回空切片，`Key()` / `Value()` 返回空向量，而不是报错。空结果不能代替 `Valid()` 判断。
- `dirty_key` 等内部访问器在迭代器已被 `close` 后直接使用会 panic，但公开 `key` / `value` 先检查 `is_valid`，正常接口路径关闭后只返回空值。
- `close` 无返回值，依次关闭 snapshot 和 dirty；`Option::take` 使重复关闭以及 `Close` 后再次 drop 都不会重复调用子迭代器。

## 并发与资源生命周期

`KvIterator: Send` 允许 `UnionIter` 随其两个 `Box<dyn KvIterator>` 在线程间转移，但该类型没有声明共享并发访问协议。推进和关闭要求 `&mut self`，内部没有锁、原子量、任务或通道；正常模型是单一所有者串行消费。若需要多线程共享，必须由上层同步并确保不会在读取当前位置时并发推进或关闭。

成功构造后，`UnionIter` 独占两个子迭代器，生命周期覆盖整个联合扫描。显式 `close` 的顺序固定为 snapshot 后 dirty，随后清除缓存有效性；`Drop` 再调用一次同一幂等逻辑。这意味着即使调用方忘记 `Close`，正常析构仍会触发两侧的 `KvIterator::close`。相反，构造期间定位失败会解除本类型对两侧的关闭责任，以保留 Go 测试规定的调用方所有权语义。

当前实际子迭代器多为内存物化扫描器，但 `KvIterator` 抽象允许未来实现持有远端游标或其他资源，因此扩展时不能删除显式关闭传播。`union_iter_test.rs::assertClose` 验证两侧都被关闭且重复关闭安全；错误测试验证错误本身不会隐式关闭子迭代器。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/driver/txn/union_iter.go`，测试对照是 `union_iter_test.go`。Rust 基本逐分支保留 Go 的结构与语义：相同的两侧迭代器、有效性缓存、当前来源、反向标志；相同的 dirty 优先、同键去重、空 dirty 删除、正反向比较；相同的构造期与推进期错误透传；以及 snapshot 后 dirty 的幂等关闭顺序。Rust 测试数据和错误案例也与 Go 测试逐项对应。

实现层面的差异如下：

- Go 保存可空接口字段；Rust 用 `Option<Box<dyn KvIterator>>` 表达打开/已关闭状态，并通过 `take` 实现一次性关闭。
- Go `Key` / `Value` 返回底层切片视图；Rust 的同名公开方法返回复制的 `Vec<u8>`，而 trait `key` / `value` 才提供借用切片。
- Rust 在无效状态调用 `next` 时显式报 `iterator is invalid`，且无效状态的键值访问返回空切片；Go 代码直接根据 `curIsDirty` 委托子迭代器。
- Rust 实现 `Drop` 自动关闭成功构造的实例，Go 依赖调用方显式 `Close`。为避免自动析构破坏 Go 的构造失败契约，Rust 在 `NewUnionIter` 出错前先移出两侧。
- Rust 的 trait object 要求 `Send`，但这只保证可转移，不表示可并发共享。

当前 Rust 不是桩或未接线门面：`tikvTxn::Iter/IterReverse` 已实际使用它。不过子扫描器目前主要基于内存物化数据，因此它对齐的是 Go 上层联合迭代行为，不等同于 Go/client-go 底层扫描资源模型。

## 扩展指南

- 若修改合并、覆盖或 tombstone 语义，核心接入点是 `update_cur`。必须同时覆盖：单侧耗尽、dirty/snapshot 键大小三种比较、相等键的非空覆盖与空值删除、正向和反向两种顺序。
- 若改变推进错误后的状态，需连同 `dirty_next`、`snapshot_next` 和 `KvIterator::next` 一起设计，并明确失败后能否重试、缓存有效性是否可信、当前键值是否仍可读。不要仅吞掉错误后继续 `update_cur`，否则可能丢行或重复行。
- 若改变资源所有权或关闭顺序，必须同步审查 `NewUnionIter` 的失败分支、`close` 和 `Drop`，保持“构造失败不 Close、成功实例最终 Close、重复 Close 安全”的现有契约，除非同时有意迁移所有调用者。
- 若增加新的扫描方向、比较器或键编码规则，不应只改 `reverse` 分支；还要保证 `txn_driver.rs` 传入的两侧迭代器使用完全相同的顺序，并评估字节字典序与目标编码顺序是否一致。
- 若公开 API 改为借用或零拷贝，应区分 `KvIterator::key/value` 与 Go 风格 `Key/Value`，评估现有调用者的所有权和性能预期。
- 测试应继续放在独立的 `pkg/store/driver/txn/union_iter_test.rs`，不要嵌入生产源文件；Go 语义变化还应同步核对 `union_iter_test.go`。建议新增的边界包括：两侧初始都空、扫描结束后再次 `Next`、`Close` 后读取/推进、方向不一致的防御策略，以及底层返回错误后是否允许重试。
- 主要兼容风险是误改 dirty 优先级或 tombstone 处理导致读到旧快照值；主要正确性风险是反向比较或相等键推进侧错误造成乱序、重复或漏行；主要性能风险是在归并热路径增加键值复制或重复比较。

## 验证依据

- RustCodeGraph 索引检查：`rustcodegraph status` 显示项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/driver/txn` 确认目标 Rust/Go/测试文件均在索引中。
- 目标实现：RustCodeGraph `node --file pkg/store/driver/txn/union_iter.rs --offset 1 --limit 260` 读取完整 249 行；`query UnionIter`、`query NewUnionIter` 与 `callees` 核对了 `UnionIter`、`NewUnionIter`、`update_cur`、trait 方法和 `Drop` 的内部关系。外部 callers 为空的图查询结果以源码引用补充，而未据此作否定结论。
- crate 与接口边界：`pkg/store/driver/txn/Cargo.toml`；`pkg/store/driver/txn/lib.rs` 的 `mod union_iter`、`pub use union_iter::*` 和 `KvIterator`。
- 生产调用链：`pkg/store/driver/txn/txn_driver.rs::tikvTxn::Iter/IterReverse`，其中分别以 `false/true` 构造联合迭代器；精确 `rg` 搜索确认这些是同目录 Rust 生产代码中的直接构造点。
- Go 对照：`pkg/store/driver/txn/union_iter.go` 与构造入口 `txn_driver.go::Iter/IterReverse`。
- Rust 独立测试：`pkg/store/driver/txn/union_iter_test.rs::TestUnionIter` 覆盖仅 dirty、仅 snapshot、双路归并、覆盖、删除及正反向；`TestUnionIterErrors` 覆盖构造期/推进期两侧错误和不自动关闭；`assertClose` 覆盖双侧关闭及重复关闭。
- Go 测试：`pkg/store/driver/txn/union_iter_test.go::TestUnionIter`、`TestUnionIterErrors`、`assertClose`，用于确认 Rust 测试数据、错误场景和资源契约与 Go 原意一致。
- 本任务只新增文档，按总计划不运行 Cargo。交付验证采用固定十一章节结构命令，并人工复核文档能解释文件存在原因、执行流程、安全扩展点以及已知语义差异。
