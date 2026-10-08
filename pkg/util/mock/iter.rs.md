# `pkg/util/mock/iter.rs`

## 文件定位

本文件属于 `astersql-util-mock` crate。`pkg/util/mock/Cargo.toml` 将 `lib.rs` 设为 crate 入口，`lib.rs` 以 `mod iter; pub use iter::*;` 将这里的公开符号提升到 crate 根，并把依赖 `astersql-kv` 重新导出为 `crate::kv`。因此它是测试辅助层中的 KV 迭代器实现：用内存中的固定记录模拟 `pkg/kv/kv.rs` 定义的 `kv::Iterator`，供测试确定性地制造遍历、推进失败和资源关闭场景，而不是 SQL 请求或存储引擎的生产迭代路径。

当前 Rust 仓库中，`NewSliceIter`、`MockedIter` 的直接使用证据位于同 crate 的 `pkg/util/mock/iter_test.rs` 和 `pkg/util/mock/migration_aster_unit_test.rs`；全仓 Rust 搜索没有发现 `NewMockIterFromRecords` 的外部调用。对应 Go 辅助实现则被 `pkg/store/driver/txn/union_iter_test.go` 和 `pkg/table/temptable/main_test.go` 等测试使用。这个差异表示 Rust API 已实现并导出，但不能据此宣称所有 Go 测试调用点都已迁移。

## 核心职责

- `SliceIter` 提供最小的内存顺序游标。它持有一组 `kv::Entry`，从下标 0 开始，只按输入顺序前进，不排序、不反向、不筛选，也不修改记录。
- `MockedIter` 装饰任意 `Box<dyn kv::Iterator>`，把 `Valid`、`Key`、`Value` 和正常的 `Next`、`Close` 委托给底层迭代器，同时增加可持续注入的 `Next` 错误、关闭状态观测和可选的重复关闭失败。
- 两个类型都实现 `kv::Iterator`，所以需要该 trait 的测试代码可以通过 trait object 使用它们，而测试也可调用额外的固有方法检查内部测试状态。
- `NewMockIterFromRecords` 把“记录集合 → `SliceIter` → `MockedIter`”组合成一个便捷入口；`iterator_error` 统一把文本包装成 KV 层的 `SharedError`。

## 主要符号

- `fn iterator_error(message: impl Into<String>) -> kv::errors::SharedError`：文件内私有错误适配器，以 `std::io::Error::other` 为源构造共享错误。`SliceIter::Next` 和 `MockedIter::InjectNextError` 使用它。
- `pub struct SliceIter { data: Vec<kv::Entry>, cur: isize }`：拥有记录和有符号游标。字段私有，调用方只能通过方法观察或推进。
- `pub fn NewSliceIter(data: Vec<kv::Entry>) -> Box<SliceIter>`：取得整个向量的所有权，设置 `cur = 0`；空向量创建后立即无效。
- `SliceIter::{GetSlice, Valid, Key, Value, Next, Close}`：分别返回只读记录切片、检查游标、克隆当前键、克隆当前值、推进一次、把游标置为 `-1`。
- `impl kv::Iterator for SliceIter`：五个 trait 方法显式转发到同名固有方法，保证固有调用与 trait-object 调用语义一致。
- `pub struct MockedIter`：包含底层 `Box<dyn kv::Iterator>`、`Option<SharedError>` 注入槽、`closed` 观测位和 `fail_on_multi_close` 策略位。
- `MockedIter::new(iterator, fail_on_multi_close)`：接管底层迭代器，初始无注入错误且 `closed = false`。
- `InjectNextError`、`InjectNextErrorValue`、`ClearInjectedNextError`、`GetInjectedNextError`：分别从消息构造错误、直接保存共享错误、清空错误、借用当前错误。注入错误不会在一次 `Next` 后自动消费。
- `FailOnMultiClose`、`Closed`：运行时调整重复关闭策略，并观察包装器是否至少关闭过一次。
- `MockedIter::{Valid, Key, Value, Next, Close}` 与其 `kv::Iterator` 实现：读取操作直接委托；`Next` 优先返回注入错误；`Close` 先检查重复关闭策略，再标记并关闭底层对象。
- `NewMockIterFromRecords(records, fail_on_multi_close) -> Box<MockedIter>`：公开组合工厂，调用边为 `NewMockIterFromRecords → NewSliceIter → MockedIter::new`。

本文件没有模块级常量、枚举、条件编译项或异步函数。

## 执行流程

1. 调用 `NewSliceIter` 时，输入 `Vec<kv::Entry>` 被移动进 `SliceIter.data`，游标初始化为 0。若数据非空，`Valid` 为真；若为空，`0 < 0` 不成立，迭代器立即无效。
2. 读取当前项时，`Key`/`Value` 先调用 `Valid`。有效时从 `data[cur]` 取值并克隆；无效时分别返回 `kv::Key::default()` 和空 `Vec<u8>`，从而不会发生越界索引。
3. `SliceIter::Next` 只在当前游标有效时执行 `cur += 1`。推进越过最后一项本身成功，之后 `Valid` 变为假；若进入 `Next` 时已经无效，则返回文本为 `iterator is invalid` 的 `SharedError`。
4. `SliceIter::Close` 把 `cur` 置为 `-1`。此后读取返回空键值，`Next` 返回无效错误；`data` 仍保留，`GetSlice` 仍可检查原记录和顺序。
5. 构造 `MockedIter` 后，未注入错误时 `Next` 直接调用底层 `Next`。设置 `next_err` 后，每次 `Next` 都克隆并返回同一共享错误，且不调用底层迭代器，所以游标不推进；只有显式 `ClearInjectedNextError` 或以另一错误覆盖它才恢复委托。
6. 第一次 `MockedIter::Close` 把 `closed` 设为真并调用底层 `Close`。之后再次关闭：若 `fail_on_multi_close` 为真，在再次委托前以 `Multi close iter` panic；若为假，则仍会再次调用底层 `Close`。

## 数据与状态

`SliceIter` 的核心不变量是：仅当 `0 <= cur < data.len()` 时游标有效。`cur` 使用 `isize`，使 `-1` 可以作为显式关闭哨兵；正常推进只会逐次增加，不提供回退或重新定位。`GetSlice` 返回借用的 `&[kv::Entry]`，不会转移或修改容器；但 `Key` 和 `Value` 返回拥有值的克隆，因此调用者对返回值的修改不会改变内部记录。构造器取得 `Vec` 所有权，源调用方若需要继续持有同一批记录，必须像测试一样在调用前克隆。

`MockedIter.next_err` 是一个持久状态槽，不是“一次性故障计数器”。`closed` 只记录包装器的 `Close` 是否被调用，并不参与 `Valid`/`Key`/`Value` 的分支；实际关闭后的可见数据行为由底层迭代器决定。`fail_on_multi_close` 可在运行中由 `FailOnMultiClose` 改变。文件中没有全局状态、缓存、随机性或时间依赖。

## 依赖与调用关系

- crate 边界：`pkg/util/mock/Cargo.toml` 声明包名 `astersql-util-mock`，直接依赖路径 crate `astersql-kv`，无本文件专属 feature。`pkg/util/mock/lib.rs` 重新导出 `kv_crate` 和本模块的所有公开项。
- 下游接口：`pkg/kv/kv.rs:923` 的 `kv::Iterator` 要求 `Valid`、`Key`、`Value`、可失败的 `Next` 和由调用方执行的 `Close`；`SliceIter`、`MockedIter` 都完整实现这五个方法。
- 文件内调用边：`SliceIter::{Key,Value,Next} → SliceIter::Valid`；无效 `Next → iterator_error`；`MockedIter::InjectNextError → iterator_error`；`MockedIter::{Valid,Key,Value}` 和无注入时的 `Next`、成功检查后的 `Close` → 底层 `dyn kv::Iterator`；`NewMockIterFromRecords → NewSliceIter + MockedIter::new`。
- Rust 上游：`pkg/util/mock/iter_test.rs::test_slice_iter` 验证基础游标；`pkg/util/mock/migration_aster_unit_test.rs` 的三个相关测试验证精确错误消息、注入不推进、清除注入、关闭状态和重复关闭 panic。`lib.rs` 将 API 暴露给依赖该 crate 的代码，但当前搜索未发现 Rust 外部直接调用该文件的公开构造器。
- Go 上游对照：`pkg/store/driver/txn/union_iter_test.go` 用两个 `MockedIter` 验证合并迭代器的错误传播与关闭；`pkg/table/temptable/main_test.go` 用它返回范围筛选后的正向/反向迭代器并注入推进错误，`pkg/table/temptable/interceptor_test.go` 再检查这些迭代器被关闭。

RustCodeGraph 对同名方法的图查询存在歧义，因此调用边结论以精确文件节点、符号查询结果和全仓定向搜索交叉核对；没有把图工具返回的其他模块同名 `Next`/`Close` 计入本文件调用者。

## 错误处理与边界

- 空输入和已遍历完的 `SliceIter` 都是无效状态：`Valid == false`，键和值为空，`Next` 返回错误；构造空迭代器本身不报错。
- 从最后一个有效项执行 `Next` 返回 `Ok(())`，只是把游标移到末尾之后；只有再执行一次 `Next` 才报错。这是调用者应使用 `Valid` 控制循环的重要边界。
- `Close` 后 `SliceIter::Next` 与越界时一样返回 `iterator is invalid`，无法仅凭错误区分“自然结束”和“已关闭”。
- `MockedIter` 原样传播底层 `Next` 错误；有注入错误时优先返回注入值并保持底层位置不变。`InjectNextErrorValue` 可保留调用方构造的 `SharedError`，`InjectNextError` 则只接受消息并用 I/O 错误包装。
- 重复关闭检测采用 `assert!`，触发的是 panic 而非 `Result`。策略开启时第二次关闭不会再次调用底层；策略关闭时重复关闭是否安全取决于底层 `Iterator::Close` 的实现。这里的 `SliceIter::Close` 是幂等赋值。
- 本实现不验证输入键的排序、唯一性或键值长度，也不提供 `Seek`。把它用于依赖有序输入的算法时，测试构造者负责按目标算法所需顺序提供记录。

## 并发与资源生命周期

两个类型都通过 `&mut self` 修改游标、错误槽和关闭状态，本文件没有锁、原子、通道、任务或异步生命周期管理，也没有声明额外的 `Send`/`Sync` 约束。是否可跨线程移动取决于 `kv::Entry`、`SharedError` 以及尤其是 `Box<dyn kv::Iterator>` 的 trait-object 约束；本文件本身不承诺并发共享，调用方不应把它当作线程安全容器。

资源由 Rust 所有权回收：`SliceIter` 被丢弃时其 `Vec` 自动释放，`MockedIter` 被丢弃时底层 `Box` 自动释放。但没有实现 `Drop` 来自动调用 `kv::Iterator::Close`，而该 trait 的注释要求调用方执行 `Close`。因此需要验证显式关闭协议的测试应调用 `Close` 并通过 `Closed` 或底层状态确认，不能把离开作用域等同于业务层关闭。

## 与 Go 版本的对应关系

`pkg/util/mock/iter.go` 是直接对照。两版的 `SliceIter` 都以位置 0 开始，使用相同有效性条件，保持输入顺序，在无效状态返回空语义，非法 `Next` 使用相同错误文本，并以 `-1` 表示关闭。`pkg/util/mock/iter_test.rs` 对照 `pkg/util/mock/iter_test.go::TestSliceIter`，覆盖 nil/空、1 至 3 项、不排序输入、自然结束、关闭以及记录不被遍历修改。

语言所有权带来几项可见差异：Go 保存 `[]*kv.Entry` 并返回底层 key/value 切片，Rust 保存拥有的 `Vec<kv::Entry>`，`Key`/`Value` 会克隆；Go 的无效返回是 `nil`，Rust 分别用默认空 `kv::Key` 和空向量表达，不能保留 nil 与 empty 的区别。Rust `NewSliceIter` 也无法区分 Go 测试中的 nil slice 与非 nil 空 slice，这两者都成为空 `Vec`，但当前迭代行为一致。

`MockedIter` 的核心语义同样对齐：注入错误优先于底层 `Next`，`Closed` 跟踪关闭，策略开启时重复关闭令测试失败。Go 保存 `*testing.T` 并调用 `assert.FailNow`；Rust 构造器不接收测试句柄，而以 panic 直接失败当前测试。Rust 额外提供 `InjectNextErrorValue`、`ClearInjectedNextError`，并把接收文本的 `InjectNextError` 与 Go 接收任意 `error` 的接口分开。Go 外部测试调用点多于当前 Rust 调用点，所以功能对应不等于测试迁移覆盖已经完全对应。

## 扩展指南

- 新增游标行为（例如 seek、反向或范围边界）前，先确认 `kv::Iterator` 是否应该扩展；若只是测试构造策略，优先增加独立包装器，避免改变 `SliceIter` 目前“仅按输入顺序前进”的简单契约。
- 修改有效性、结束或关闭语义时，应同步 `pkg/util/mock/iter_test.rs` 和 `pkg/util/mock/migration_aster_unit_test.rs`，并逐项核对 `pkg/util/mock/iter_test.go`；还要评估 Go 中 `union_iter` 与临时表测试所依赖的错误传播、关闭状态和重复关闭行为。
- 若要做一次性错误注入，应新增明确命名的 API 或计数状态，不要悄悄把现有 `next_err` 改为自动消费，因为现有实现和测试依据是“错误持续存在直到清除”。
- 若要更改 `Close`，需要保留“先检测重复关闭、再委托底层”的顺序，并决定关闭失败应继续 panic 还是需要上移到返回 `Result` 的新接口；当前 `kv::Iterator::Close` 没有错误返回值。
- 若将其用于跨线程测试，必须先在 `kv::Iterator` trait object 边界明确 `Send`/`Sync` 契约并增加独立并发测试，不能仅给结构体添加锁便宣称线程安全。
- 性能上，`Key`/`Value` 每次读取都会克隆。若测试数据变大而需要借用返回值，必须连同 `kv::Iterator` trait 签名一起设计，不能只在本文件局部消除克隆。
- Rust 单元测试应继续放在独立的 `iter_test.rs` 或其他独立测试文件中，不要内嵌到 `iter.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目索引含 11,467 个文件；`files --filter pkg/util/mock` 找到 `iter.rs`、`iter_test.rs` 及 Go 对照文件。
- RustCodeGraph 源码节点：`pkg/util/mock/iter.rs`（全 224 行）、`pkg/util/mock/iter_test.rs`（全 88 行）、`pkg/util/mock/iter.go`（全 133 行）、`pkg/util/mock/iter_test.go`（全 93 行）、`pkg/kv/kv.rs:923-929` 的 `Iterator` trait，以及 `pkg/util/mock/migration_aster_unit_test.rs:70-123` 的相关测试。
- RustCodeGraph/搜索调用证据：`NewMockIterFromRecords → NewSliceIter` 与 `MockedIter::new`；全仓定向搜索确认 Rust 直接使用集中在 `pkg/util/mock` 测试，而 Go 调用位于 `pkg/store/driver/txn/union_iter_test.go:181-263`、`pkg/table/temptable/main_test.go:222-258` 和 `pkg/table/temptable/interceptor_test.go:1204-1229`。
- crate 与模块证据：`pkg/util/mock/Cargo.toml`、`pkg/util/mock/lib.rs`；前者确认 `astersql-kv` 路径依赖和无 feature，后者确认模块声明与公开再导出。
- 边界测试证据：`iter_test.rs::test_slice_iter` 覆盖空/多项、自然结束和关闭后行为；`migration_aster_unit_test.rs::{slice_iter_matches_go_cursor_and_close_behavior,mocked_iter_injects_error_without_advancing_and_tracks_close,mocked_iter_can_fail_on_multiple_close_calls}` 覆盖空键值、精确错误、持久注入、清除、关闭状态及重复关闭 panic。
- 本任务是纯文档分析，按计划未运行 Cargo；最终仅以固定章节结构检查和人工事实复核验证文档。
