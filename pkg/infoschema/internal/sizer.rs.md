# `pkg/infoschema/internal/sizer.rs`

## 文件定位

该文件属于 `astersql-infoschema-internal` crate。crate 入口 `pkg/infoschema/internal/lib.rs` 以 `pub mod sizer` 声明模块，并以 `pub use sizer::*` 再导出其公开项；因此下游既可以通过 `sizer` 模块，也可以从 crate 根访问 `SizeCache`、`MemoryUsage`、`sizeof` 和 `Sizeof`。`pkg/infoschema/internal/Cargo.toml` 将该 crate 对应到 Go 包 `pkg/infoschema/internal`；本文件自身只依赖 Rust 标准库，不使用 Cargo 中的 mockstore 或 Windows 条件依赖。

它提供的是与 Go `pkg/infoschema/internal/sizer.go` 相对应的内存占用估算基础设施，不是 SQL 请求、InfoSchema 元数据装载或 DDL 执行主链的一环。RustCodeGraph 对该文件的直接使用关系只列出 `pkg/infoschema/internal/sizer_test.rs` 和 `pkg/planner/core/operator/physicalop/base_physical_join_test.rs`；后者只是使用标准库 `size_of` 的测试，未导入本模块，所以目前可确认的真实 API 调用者是本 crate 的独立 sizer 测试。不能据此宣称估算器已经接入生产时内存记账。

## 核心职责

1. 以 `MemoryUsage::memory_usage` 作为类型化递归协议，替代 Go 版本在运行时使用 `reflect.Value` 遍历任意值的做法。
2. 以 `SizeCache` 记录访问过的堆地址，使字符串缓冲、容器或共享指针指向的数据至多被递归计入一次，并为环状指针图提供终止条件。
3. 为标量、字符串、引用、顺序容器、智能指针、可选值、映射、二元组和裸指针提供通用实现，尽量复现 Go `Sizeof` 的头部、容量、padding 和 map bucket 估算口径。
4. 用负数（约定上为 `-1`）传播无法估算的结果。内置实现本身没有返回其他负值的分支，但领域类型的自定义 `MemoryUsage` 实现可以报告失败。

这里的结果是兼容 Go 口径的近似记账值，而不是分配器观测的实际驻留内存：例如 `Arc`/`Rc` 的引用计数、`HashMap` 的 Rust 实际布局均被有意忽略。

## 主要符号

- `pub struct SizeCache { visited: HashSet<usize> }`：一次估算遍历的私有去重状态。字段不公开，外部只能创建默认值并把可变引用交给 `MemoryUsage` 实现。
- `SizeCache::first_visit(address) -> bool`：私有辅助函数。非零地址首次插入返回 `true`，重复地址返回 `false`；地址 `0` 始终返回 `true` 且不写缓存。
- `pub trait MemoryUsage`：唯一方法 `memory_usage(&self, cache: &mut SizeCache) -> isize`。领域结构体必须逐字段调用该方法，并自行补齐结构体 padding，才能获得类似 Go 反射遍历的效果。
- `pub fn sizeof<T: MemoryUsage + ?Sized>(&T) -> isize`：惯用 Rust 命名的顶层入口；每次调用创建全新的默认缓存，所以不同调用之间不会共享去重状态。
- `pub fn Sizeof<T: MemoryUsage + ?Sized>(&T) -> isize`：Go 风格兼容入口，仅转发给 `sizeof`。
- `scalar_usage!`：为 `()`、布尔、整数、浮点和 `char` 批量实现只返回 `size_of::<Self>()` 的估算。它是模块内宏，没有导出。
- `GO_MAP_HEADER_SIZE: isize = 8`：强制采用 64 位 Go map 单指针头的记账值，而不是 Rust `HashMap` 的本体大小。
- `MemoryUsage` 的标准实现：覆盖 `str`、`&T`、`String`、`Vec<T>`、`[T; N]`、`Box<T>`、`Arc<T>`、`Rc<T>`、`Option<T>`、`HashMap<K,V,S>`、`BTreeMap<K,V>`、`(A,B)`、`*const T` 和 `*mut T`。文件没有条件编译项。

## 执行流程

顶层调用从 `sizeof(value)` 或别名 `Sizeof(value)` 开始。入口创建一个空 `SizeCache`，随后静态分派到值类型的 `MemoryUsage` 实现。递归实现遵循以下共同模式：先计算本层固定布局或头部，再遍历子值；任何子值返回负数就立即返回 `-1`，否则累加结果。

不同类型的关键分支如下：

- 标量直接返回静态尺寸；引用 `&T` 不另计引用字宽，而是委托给被引用值。这一规则使 `&str` 进入 `str` 的“字符串头加内容”口径。
- `str`/`String` 先计各自头部；底层字节地址首次出现时再计 `len`，重复出现时只计头部。`String` 特意不计 `capacity - len`，以模拟不可扩容的 Go string。
- `Vec<T>` 若非空缓冲地址已访问则整层返回 `0`；否则计 `Vec` 头、每个已初始化元素的递归值，以及未用 capacity 槽位的 `size_of::<T>()`。这与 Go slice 的“头 + 元素 + 预留容量”形状对应。
- 数组递归累计每个元素，再加入数组布局相对于 `N * size_of::<T>()` 的饱和差值。
- `Box`、`Arc`、`Rc` 每个句柄都计指针字宽；仅首次遇到 pointee 地址时递归计 pointee。重复共享引用只返回句柄尺寸。
- `Option::None` 返回自身静态尺寸；`Some(T)` 返回子值估算加上 `Option<T>` 相对 `T` 的额外布局。这里使用 `saturating_sub`，不会因 niche 优化导致负开销。
- `HashMap` 以 map 对象地址去重，首次访问时从 8 字节 Go map 头开始，累加所有键值，再增加 `len * 10.79` 截断为整数的 bucket 开销。`BTreeMap` 则从 Rust `size_of_val(self)` 开始累加键值，没有 map 去重或 10.79 系数。
- 二元组累加两个成员，再补元组布局 padding；裸指针只计指针本身，不跟随目标。

## 数据与状态

唯一的遍历状态是每次顶层估算私有的 `HashSet<usize>`。缓存键是地址而非所有权身份：字符串用数据指针，`Vec` 用缓冲首地址，`Box`/`Arc`/`Rc` 用 pointee 地址，`HashMap` 用 map 对象自身地址。由此产生的重要不变量是“同一个非零键在一次遍历中只展开一次”，但各类型对重复节点的返回值不同：`Vec` 和 `HashMap` 返回 `0`，指针类型仍返回指针字宽，字符串仍返回头部。

所有累计值使用 `isize`。代码没有显式的溢出检查：普通加法和乘法遵循当前构建配置的 Rust 整数溢出行为；只有布局差值使用 `saturating_sub`。map 的额外开销经过 `usize -> f64 -> isize` 转换并截断小数，因此它本来就是经验估算。尺寸还受目标平台字宽与 Rust 类型布局影响，而 `GO_MAP_HEADER_SIZE = 8` 固定采用 64 位 Go 假设。

## 依赖与调用关系

下游依赖全部来自标准库：`HashSet` 保存访问地址，`HashMap`/`BTreeMap` 是被实现的容器，`Hash`/`Eq`/`Ord` 构成键约束，`size_of`/`size_of_val` 提供布局尺寸，`Rc`/`Arc` 提供共享指针地址。没有 I/O、数据库、事务、异步运行时或外部 crate 调用。

模块装配链为 `pkg/infoschema/internal/lib.rs` → `pub mod sizer` → `pub use sizer::*`。已验证的调用链为 `pkg/infoschema/internal/sizer_test.rs::test_size` 等测试 → `sizeof` → 具体类型的 `memory_usage` → 嵌套成员的 `memory_usage`/`SizeCache::first_visit`。RustCodeGraph 的泛型 `callers sizeof`、`callees sizeof` 和 trait 方法查询未返回可用静态调用边；文件级索引则确认目标文件有 27 个符号并被两份测试文件引用。仓库文本检索没有发现目标测试之外对本模块 `sizeof`、`Sizeof`、`SizeCache` 或该 trait 的明确导入/调用。

## 错误处理与边界

该 API 不返回 `Result`；负数是唯一失败通道。容器、数组、智能指针、`Option::Some` 和二元组发现任一子值为负时统一归一化成 `-1`。`pkg/infoschema/internal/sizer_test.rs::test_tuple_propagates_size_failure` 用自定义 `FailingSize` 验证了这一传播约定。

Rust 版本没有 Go 反射的“未知 Kind 最终返回 -1”入口：未实现 `MemoryUsage` 的类型会在编译期无法调用。它也没有 Go `reflect.Interface` 对 `Storage`、`Client`、`Allocator` 只计 8 字节的名称特判，没有 chan、func、complex 等内置实现，也没有 Go nil 指针分支。裸指针实现从不解引用，因此不会因悬垂指针触发读取，但也只得到一个机器字的估算。

其他边界包括：空字符串不加入缓存但仍只计头部；零 capacity 的 `Vec` 不做地址去重；`HashMap` 用对象地址而非内部 bucket 地址，因而其去重语义不能等同于 Go map 指针；`BTreeMap` 是 Rust 扩展，并不复现 Go map bucket 模型。自定义结构体实现若漏字段、重复计本体或未补 padding，框架无法自动发现。

## 并发与资源生命周期

`sizeof` 在栈上创建 `SizeCache`，递归结束后连同其中的 `HashSet` 一起释放；模块不保留全局状态、锁、通道、任务或后台资源。缓存通过 `&mut SizeCache` 独占传递，因此同一次遍历不能被多个线程并发修改。公开 API 接受共享的 `&T`，不会改变被估算对象。

`Arc<T>` 支持被多个线程持有不意味着一次估算会跨线程运行；实现只读取当前句柄和 pointee。引用计数控制块没有被计入，也没有在遍历期间额外克隆智能指针。`Rc<T>` 同理，但其本身仍受 Rust 的非线程安全约束。地址只在调用期间使用，不在对象生命周期之外保存。

## 与 Go 版本的对应关系

Go `Sizeof(any)` 先 `reflect.Indirect`，再由私有 `sizeOf(reflect.Value, cache)` 按 Kind 递归；Rust `sizeof<T: MemoryUsage>` 则通过 trait 静态分派。两者共同保留了数组逐项、slice/`Vec` 预留容量、字符串头和内容、结构 padding、共享指针去重、map 键值与 `10.79` 经验开销、失败返回负数等核心记账意图。Rust 测试 `test_size` 逐项复刻 Go `TestSize` 的期望值：数组 12、空字符串数组 80、预留容量切片 64、字符串切片 126、字符串 22、map 84、嵌套结构 75。

差异是有意且必须显式维护的：Rust 无字段反射，领域结构体要自行实现 trait；`String`、`Arc`、`Rc`、`Option`、`BTreeMap`、元组和裸指针是 Rust 侧扩展。`Arc`/`Rc` 按 Go `reflect.Ptr` 只计句柄与首次 pointee，不计引用计数；`test_shared_pointers_match_go_pointer_accounting` 固定了该行为。`test_owned_string_uses_length_not_capacity` 固定了 Go string 的长度口径。另一方面，Go 的 interface 特判、nil pointer、channel/function/complex/unsafe pointer 分类并未完整映射，因此不能把当前 trait 覆盖范围描述成 Go 反射器的全类型等价替代。

## 扩展指南

为新的领域结构增加估算时，应在领域源文件中实现 `MemoryUsage`：逐字段复用现有实现，遇到负值立即返回 `-1`，最后补上 `size_of::<Self>()` 相对于字段静态尺寸之和的 padding。不要把测试实现写入生产文件；应在同目录独立 `*_test.rs` 中增加用例。若类型包含共享所有权或可能成环，必须复用同一个 `&mut SizeCache`，并为稳定、非零的分配地址定义“首次展开、重复节点计多少”的规则。

修改已有容器口径时，最先应同步 `pkg/infoschema/internal/sizer_test.rs`，并与 `pkg/infoschema/internal/sizer.go`、`sizer_test.go` 的手算期望核对。新增 Go 已支持的类型时，要决定是精确复刻 Go 头部还是采用 Rust 实际布局，并把差异写入测试；尤其要关注 32/64 位平台、空值、共享缓冲、capacity、padding、map 浮点截断和加法溢出。若要支持任意 trait object 或自动结构遍历，需要新的显式抽象或 derive 机制，不能假定现有 trait 会像 Go reflect 一样自动枚举字段。

## 验证依据

- Rust 源码：`pkg/infoschema/internal/sizer.rs`，核对了全部 297 行、27 个索引符号、公开 API、所有 `MemoryUsage` 实现及无条件编译项。
- crate 边界：`pkg/infoschema/internal/Cargo.toml` 与 `pkg/infoschema/internal/lib.rs`，核对 crate 名、Go 包映射、模块公开与根级再导出；`pkg/infoschema` 下未找到适用于该包的 `doc.go`。
- Go 对照：`pkg/infoschema/internal/sizer.go` 与 `pkg/infoschema/internal/sizer_test.go`，核对 reflect Kind 分支、缓存规则、interface 特判及表驱动期望。
- Rust 测试：`pkg/infoschema/internal/sizer_test.rs`，核对 Go 用例复刻、`String` 长度口径、负值传播、`Arc`/`Rc` 共享 pointee 去重。RustCodeGraph 另列出 `pkg/planner/core/operator/physicalop/base_physical_join_test.rs` 为文件级使用者；人工读取确认它未调用本模块，只使用标准库 `size_of`。
- RustCodeGraph：运行 `status` 确认索引含 11,467 个文件；运行目标文件 `files`/`node`，确认 297 行、27 个符号及文件级使用关系；运行 `query SizeCache`、`query MemoryUsage --kind trait`、`query sizeof --kind function`、`query Sizeof --kind function`；运行精确 `callers`/`callees`，泛型入口和 trait 方法未产生可用静态边，因此调用关系同时用模块入口、测试源码和仓库文本检索验证。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好具有任务规定的十一个二级章节。
