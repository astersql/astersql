# `pkg/util/generic/bounded_min_heap.rs`

## 文件定位

[`bounded_min_heap.rs`](bounded_min_heap.rs) 位于 `pkg/util/generic`，是 `astersql-util-generic` crate 中维护固定容量 top-N 集合的通用容器实现。crate 的 [`Cargo.toml`](Cargo.toml) 将 `lib.rs` 指定为 crate 根，并用 `package.metadata.porting.go-package = "pkg/util/generic"` 标记 Go 迁移来源；[`lib.rs`](lib.rs) 通过 `pub mod bounded_min_heap` 声明模块，再用 `pub use bounded_min_heap::*` 将其公开符号提升到 crate 根。该 crate 自身没有第三方 Rust 依赖或 feature 开关。

当前仓库搜索没有发现生产 Rust 文件调用 `NewBoundedMinHeap`、`BoundedMinHeap::Add` 或 `BoundedMinHeap::ToSortedSlice`；Rust 直接使用者是独立测试 [`bounded_min_heap_test.rs`](bounded_min_heap_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)。因此这是一项已实现、已从 crate 公开且有测试证据，但尚未接入 Rust 应用主链的迁移能力。

完整 TiDB/AsterSQL 应用中可验证的实际业务位置仍在 Go 侧：[`pkg/statistics/builder.go`](../../statistics/builder.go) 用它从采样值中保留计数最高的 top-N，[`pkg/statistics/histogram.go`](../../statistics/histogram.go) 用它选择分区合并后的全局 top-N。Rust 文件与这些 Go 调用者之间没有跨语言调用边，不能把 Go 接线当作 Rust 已接线。

## 核心职责

该文件用一个“根节点是当前最差元素”的最小堆，在最多 `N = maxSize` 个槽位内维护比较器认定的最好元素：

1. 堆未满时，`Add` 插入元素并上浮，建立堆序。
2. 堆已满时，只将比较结果严格大于零的新元素视为优于根节点；它替换当前最差元素并向下调整。
3. 比较结果小于或等于零的新元素不改变集合，因此满容量时同优先级元素也不会替换已有元素。
4. `ToSortedSlice` 克隆当前内容并按“最好到最差”排序，不破坏内部堆。

比较器遵循 Go 的三值约定：`cmp(a, b) > 0` 表示 `a` 比 `b` 更好，`< 0` 表示更差，`0` 表示等价。容量为 `N` 时，插入复杂度为 `O(log N)`，满堆拒绝元素只需一次比较、为 `O(1)`；输出需要克隆并排序，为 `O(N log N)` 时间和 `O(N)` 额外空间。

## 主要符号

- `pub struct internalHeap<T, F>`：内部存储层，字段是比较器 `cmp: F` 和连续存储 `items: Vec<T>`。名字和角色对应 Go 的 `internalHeap`；尽管语义上是内部实现，当前 Rust 声明本身为 `pub`，并会被 `lib.rs` 的通配再导出暴露。
- `internalHeap::Len(&self) -> usize`：返回 `items.len()`。
- `internalHeap::Less(&self, i, j) -> bool`：当 `cmp(items[i], items[j]) < 0` 时返回真，是“更差元素应靠近根”的堆序判断。
- `internalHeap::Swap(&mut self, i, j)`：交换两个下标，由上浮和下沉过程使用。
- `internalHeap::Push(&mut self, x)`：把元素追加到 `Vec` 尾部，再从新尾下标执行 `sift_up`；它合并了 Go `heap.Push` 对接口 `Push` 的后续调整行为。
- `internalHeap::Pop(&mut self) -> Option<T>`：只弹出 `Vec` 尾部，对齐 Go `heap.Interface.Pop` 回调本身的语义；本文件的 top-N 主流程不调用它，也不负责在任意弹出后重建堆。
- `internalHeap::fix_root`、`sift_up`、`sift_down`：私有堆维护函数。`fix_root` 用于根替换，`sift_up` 用于新元素插入，`sift_down` 每轮从左右子节点中选择比较器认定更差者。
- `pub struct BoundedMinHeap<T, F>`：公开有界容器，组合 `data: internalHeap<T, F>` 与不可变容量 `maxSize: usize`；要求 `F: Fn(&T, &T) -> i32`。
- `pub fn NewBoundedMinHeap(maxSize: isize, cmpFunc: F) -> BoundedMinHeap<T, F>`：公开构造器。负容量 panic；合法容量被转换为 `usize`，并作为 `Vec::with_capacity` 的容量提示及逻辑上限。
- `BoundedMinHeap::Len`、`BoundedMinHeap::Add`：分别读取当前元素数和执行有界插入。
- `BoundedMinHeap::ToSortedSlice(&self) -> Option<Vec<T>>`：仅在 `T: Clone` 时可用；空集合返回 `None`，非空时返回独立、从最好到最差的向量。
- `fn cmp_to_order(i32) -> Ordering`：把负、零、正三值结果转换成 `Ordering`，供 Rust 排序使用。

文件没有 trait、模块级常量、静态状态、条件编译项或异步函数。

## 执行流程

构造时，`NewBoundedMinHeap` 先拒绝负的 `maxSize`，随后用该容量预分配 `Vec`，保存调用者提供的比较闭包。预分配容量不是独立的行为限制；真正保证长度不超过上限的是 `Add` 的分支。

`Add` 的完整路径如下：

1. `maxSize == 0` 时立即返回，传入元素随调用结束被丢弃。
2. `items.len() < maxSize` 时调用 `internalHeap::Push`：先追加元素，再由 `sift_up` 反复与父节点比较；只要子节点更差就交换，直到根方向不再违反堆序。
3. 已满时比较新元素与 `items[0]`。结果不大于零时直接忽略新元素。
4. 结果大于零时覆盖根，再调用 `fix_root`/`sift_down`。下沉每次选择左右子节点中更差的一个；若该子节点不比当前节点更差则结束，否则交换并继续。

因此，只要比较器在元素生命周期内保持一致且形成可排序关系，根始终是已保存集合中的最差元素，长度始终满足 `Len() <= maxSize`。

`ToSortedSlice` 不逐个弹堆。它先克隆 `items`，再用比较结果的反号排序：原比较器认为更好的元素会获得更小的排序结果，从而排到前面。源码用 `saturating_neg` 处理 `i32::MIN`，避免普通取负溢出；最后将排序向量包装为 `Some`。内部堆的元素布局和长度都不会被该读取操作改变。

## 数据与状态

实例的全部状态是 `Vec<T>`、比较闭包 `F` 和逻辑容量 `usize`，没有全局或线程局部状态。`Vec` 同时持有元素所有权和已分配缓冲区；当集合未满时它增长，达到逻辑容量后不再通过 `Add` 增长。替换根会立即析构被淘汰的旧 `T`，拒绝或零容量分支会析构传入但未保存的 `T`。

`ToSortedSlice` 要求 `T: Clone`，返回值与堆分别拥有元素副本；修改或销毁结果不会修改堆。空结果用 `None` 对齐 Go 的 `nil` slice，而不是 `Some(Vec::new())`。`Len` 返回 `usize`，与 Go 版本的 `int` 类型不同，但都表达当前保存数量。

正确性依赖比较器不随外部可变状态发生破坏堆序的变化，并且对参与比较的元素提供一致的三值顺序。源码没有验证反对称性、传递性或结果是否仅为 `-1/0/1`；实际上只检查符号。若比较器不一致，集合内容与排序结果可能不可靠，但容器不会返回专门错误。

## 依赖与调用关系

Rust 装配链是 `pkg/util/generic/Cargo.toml` → [`lib.rs`](lib.rs) → `bounded_min_heap` 模块及其通配再导出。本文件唯一直接导入是标准库 `std::cmp::Ordering`；存储、克隆和排序全部使用标准库 `Vec`、`Clone` 与 slice 排序能力。

RustCodeGraph `query` 定位到 `BoundedMinHeap` 和 `NewBoundedMinHeap`，`node --file` 返回了完整的 224 行实现。精确 `callers NewBoundedMinHeap` 查询在 30 秒窗口内没有产生结果；仓库精确搜索复核只发现 `lib.rs` 的公开装配和两个 Rust 测试模块，未发现生产 Rust 调用。因此当前没有可证实的 Rust SQL、统计或执行主链上游。

Go 对照的生产上游有两组：

- `pkg/statistics/builder.go::BuildHistAndTopN` 构造按 `TopNWithRange.Count` 比较的堆，`processTopNValue` 调用 `Len` 和 `Add`，用于采样直方图构建。
- `pkg/statistics/histogram.go::MergePartTopNAndHistToGlobal` 构造按 `topNCandidate.totalCount` 比较的堆，后续 `selectGlobalTopN` 调用 `ToSortedSlice`，用于合并分区统计信息。

这些 Go 边说明该抽象在完整应用中的设计用途，但 Rust 版本目前仅具备容器实现与测试装配。

## 错误处理与边界

构造器只显式检查负容量，并以 `panic!("maxSize cannot be negative")` 拒绝。零容量合法，所有 `Add` 都是无操作，`Len` 始终为零且 `ToSortedSlice` 返回 `None`。Rust 的泛型闭包值没有 Go `func` 的 `nil` 形态，因此 Rust 版没有对应的空比较函数运行时检查；该差异在测试中被明确记录。

公开 API 不返回 `Result`。内存分配失败沿用标准库行为；比较器自身若 panic，panic 会向上传播。若 `Push` 在追加后执行比较时 panic，元素已经进入 `Vec`，不应假设实例仍满足堆序；若 `Add` 的满堆比较在替换前 panic，集合尚未修改；若根替换后的 `sift_down` 比较 panic，旧根已被丢弃且堆序可能未恢复。

满堆时只有严格 `> 0` 才替换，所以相等元素不会改变已保存对象，稳定性也不承诺“保留最新”等策略。`Less`、`Swap` 接受任意下标，越界会由 `Vec` panic；主流程生成的下标受长度检查约束。`internalHeap::Pop` 对空堆返回 `None`，但它是尾部弹出而不是完整的“删除堆根”操作，扩展者不能单独调用它来实现优先队列弹出。

`ToSortedSlice` 通过 `saturating_neg` 避免 `i32::MIN` 取负溢出，但比较器若返回极值、非传递结果或依赖变化的外部状态，仍可能与 Go 的普通反号比较或预期顺序不同。推荐比较器只返回符号稳定的三值结果。

## 并发与资源生命周期

类型内部没有锁、原子量、通道、任务或异步资源。修改操作需要 `&mut self`，安全 Rust 中同一实例不能在一次可变借用期间被另一调用同时修改；类型是否可跨线程发送或共享由 `T`、`F` 和 `Vec<T>` 的自动 trait 条件决定，文件没有手写 `Send`/`Sync` 实现。

构造后，比较器和 `Vec` 随 `BoundedMinHeap` 一起存活；没有显式关闭过程。实例析构时，保存元素、缓冲区和比较闭包按 Rust 所有权规则释放。`ToSortedSlice` 的克隆结果拥有独立生命周期，可能使元素管理的资源引用或深拷贝成本延长到结果销毁时。

如果未来需要多线程共享，调用方必须自行使用 `Mutex`/`RwLock` 等同步容器，并定义比较闭包捕获状态的同步与稳定性；仅依赖 `&mut self` 不能提供跨线程共享 API。现有测试只覆盖单线程行为。

## 与 Go 版本的对应关系

直接来源是 [`bounded_min_heap.go`](bounded_min_heap.go)。类型和方法映射为：Go `internalHeap` ↔ Rust `internalHeap`，Go `BoundedMinHeap` ↔ Rust `BoundedMinHeap`，以及同名的 `NewBoundedMinHeap`、`Len`、`Add`、`ToSortedSlice`。两版都把更差元素放在根、未满时插入、满时只用严格更优元素替换根，并以最好到最差顺序返回副本。

主要实现差异是：

- Go 用 `container/heap` 调用 `heap.Push` 和 `heap.Fix`；Rust 在本文件用 `sift_up`、`sift_down` 和 `fix_root` 复现对应调整。
- Go 比较器签名是 `func(T, T) int`，按值传参；Rust 是 `Fn(&T, &T) -> i32`，避免比较时转移或复制元素。
- Go 构造器返回指针并检查比较函数是否为 `nil`；Rust 返回拥有值，泛型闭包没有可传入的 `nil` 等价物。
- Go 的 `ToSortedSlice` 对任意 `T` 复制 slice 元素；Rust 为产生独立结果要求 `T: Clone`，空结果以 `Option<Vec<T>>::None` 对齐 Go `nil`。
- Go 排序直接使用 `-cmp(a,b)`；Rust 使用 `saturating_neg` 后映射为 `Ordering`，明确避免 `i32::MIN` 的取负溢出。
- Go 的 `internalHeap.Push(any)` 依赖运行时类型断言；Rust `Push(T)` 在编译期保证类型一致，并把标准库堆调用的上浮动作合并进方法。

[`bounded_min_heap_test.rs`](bounded_min_heap_test.rs) 基本逐项对齐 [`bounded_min_heap_test.go`](bounded_min_heap_test.go)：覆盖空堆、容量内和超容量插入、重复值、容量一/零、自定义结构体、反向比较器、满堆替换、1000 个输入的 top-10 以及负容量 panic。Go 的 nil 比较器测试在 Rust 文件中只保留原因说明，不伪造不可表达的输入。

## 扩展指南

- 修改 top-N 接受规则时，入口是 `BoundedMinHeap::Add`；必须保持 `Len() <= maxSize` 和“根为最差元素”不变量，并同步扩充独立的 [`bounded_min_heap_test.rs`](bounded_min_heap_test.rs)，至少覆盖未满、满堆更好/相等/更差三条分支。
- 修改堆算法时，应成对审查 `Less`、`sift_up`、`sift_down` 和 `fix_root`，用正向与反向比较器验证方向。不要把 Rust 单元测试放入生产源文件。
- 修改输出 API 或排序时，入口是 `ToSortedSlice` 与 `cmp_to_order`；需评估 `T: Clone`、空值 `None`、最好到最差顺序和不修改原堆的兼容承诺。
- 若增加删除根、查看根或调整容量等能力，不应把当前尾部语义的 `internalHeap::Pop` 误当成完整操作；需要明确恢复堆序、容量变化和被淘汰元素的所有权，并在独立测试文件覆盖。
- 若收紧封装，可评估将 `internalHeap` 及其接口方法改为非公开，但这会改变当前 crate 通配再导出的可见 API，需先搜索外部 crate 使用者并作为兼容性变更处理。
- 与 Go 继续保持迁移对齐时，同步核对 [`bounded_min_heap.go`](bounded_min_heap.go) 和 [`bounded_min_heap_test.go`](bounded_min_heap_test.go)。如果 Rust 生产代码要承接统计 top-N 主链，还需逐一移植并验证 `builder.go`/`histogram.go` 的调用语义，而不能仅凭本容器存在宣称接线完成。
- 性能评估应关注 `N` 而非总输入量：流式插入空间为 `O(N)`、接受元素为 `O(log N)`；频繁调用 `ToSortedSlice` 会反复克隆和 `O(N log N)` 排序。大或昂贵克隆的 `T` 可能使输出成本占主导。

## 验证依据

- Rust 源码：[`bounded_min_heap.rs`](bounded_min_heap.rs)，核对 `internalHeap`、`BoundedMinHeap`、构造器、两套 `impl`、本地 sift 算法和 `cmp_to_order`；文件无条件编译项、常量、静态量或 trait。
- crate 边界：[`Cargo.toml`](Cargo.toml) 与 [`lib.rs`](lib.rs)，核对包名、`lib.rs` crate 根、Go 包迁移元数据、公开模块/再导出和两个独立测试模块；目标目录不存在 `doc.go`。
- Go 对照与生产入口：[`bounded_min_heap.go`](bounded_min_heap.go)、[`pkg/statistics/builder.go`](../../statistics/builder.go) 的 `processTopNValue`/`BuildHistAndTopN`，以及 [`pkg/statistics/histogram.go`](../../statistics/histogram.go) 的 `selectGlobalTopN`/`MergePartTopNAndHistToGlobal`。
- 独立测试：[`bounded_min_heap_test.rs`](bounded_min_heap_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 Go [`bounded_min_heap_test.go`](bounded_min_heap_test.go)，共同证明正反比较器、重复项、零/负容量、自定义结构体、替换/忽略和大数据 top-N 边界。
- RustCodeGraph：`status` 报告索引可用（11,467 个文件、307,296 个节点、1,848,419 条边）；`query BoundedMinHeap --kind struct` 与 `query NewBoundedMinHeap --kind function` 定位目标符号；`node --file pkg/util/generic/bounded_min_heap.rs --offset 1 --limit 260` 返回完整 224 行源码。精确 `callers NewBoundedMinHeap --limit 50` 在 30 秒内无输出，故调用关系以文件节点、Go 节点和仓库精确 `rg` 回退复核，未据此虚构 Rust 生产调用边。
- 人工结构与边界复核：确认 11 个固定章节、所有关键行为均可回指真实符号或测试；特别区分 Go 生产接线与 Rust 当前迁移状态，并复核复杂度、所有权、panic 时状态和比较器一致性风险。
