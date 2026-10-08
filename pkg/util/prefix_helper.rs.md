# `pkg/util/prefix_helper.rs`

## 文件定位

`pkg/util/prefix_helper.rs` 是根工具 crate `astersql-util` 中的字节序键前缀辅助模块。`pkg/util/lib.rs` 通过 `pub mod prefix_helper` 将它公开；`pkg/util/Cargo.toml` 指定该 crate 的入口为 `lib.rs`，并声明本文件唯一直接使用的外部依赖 `anyhow`。它位于通用 KV 边界，而不是具体存储引擎：调用者通过本文件定义的 `Retriever`、`RetrieverMutator` 和 `KvIterator` 适配自己的有序 KV 实现。

当前文件同时定义了一套本地 `Key` 与 KV trait，并没有复用 `pkg/kv/key.rs` 的 `Key`。因此它目前是一个自包含的移植边界。RustCodeGraph 对 `ScanMetaWithPrefix`、`DelKeyWithPrefix` 和 `RowKeyPrefixFilter` 的调用者查询只找到 `pkg/util/cpu_posix_1_aster_unit_test.rs::prefix_helpers_scan_stop_delete_and_compare_like_go`，未找到生产调用者；`pkg/util/prefix_helper_test.rs` 还通过 `pkg/util/lib.rs` 的 `cfg(test)` 模块声明覆盖同一 API。这里的“应用位置”应理解为可供 `astersql-util` 用户调用的通用能力，而不能宣称已经接入 SQL、事务或元数据主链。

## 核心职责

- `Key::PrefixNext` 为一个字节前缀计算半开扫描区间的右边界，供 `[prefix, prefix.PrefixNext())` 使用。
- `Key::HasPrefix` 提供纯字节前缀判断，为扫描结果增加防御性边界检查。
- `ScanMetaWithPrefix` 顺序访问指定前缀下的键值，并允许回调通过返回 `false` 提前停止。
- `DelKeyWithPrefix` 先克隆目标键，再逐键删除，避免在遍历过程中直接改变迭代集合。
- `RowKeyPrefixFilter` 构造一个可跨线程传递、共享的谓词：仍带指定行键前缀时返回 `false`，离开此前缀时返回 `true`。
- `KvIterator`、`Retriever`、`RetrieverMutator` 把算法与存储实现解耦；本文件本身不持有数据库连接、事务或全局状态。

## 主要符号

- `pub struct Key(pub Vec<u8>)`：拥有字节序列的键包装。派生 `Eq`/`Ord` 等 trait，因此排序是 `Vec<u8>` 的字典序；`From<Vec<u8>>` 转移所有权，`From<&[u8]>` 复制切片。
- `Key::PrefixNext(&self) -> Key`：从末字节向前做带进位加一。遇到非零结果立即返回；空键或所有字节均为 `0xff` 时恢复原键并追加 `0`。该边界行为逐行对应 `pkg/kv/key.go::PrefixNext` 和 `pkg/kv/key.rs::PrefixNext`。
- `Key::HasPrefix(&self, prefix: &Key) -> bool`：委托给 `Vec<u8>::starts_with`；空前缀按标准切片语义匹配任意键。
- `pub trait KvIterator: Send`：暴露 `valid`、`key`、`value`、可失败的 `next` 和默认空实现的 `close`。`key`/`value` 的引用只借用当前位置；实现者负责在 `valid() == true` 时保证访问安全。
- `pub trait Retriever`：`iter(&self, start, end)` 必须创建 `[start, end)` 半开区间迭代器。
- `pub trait RetrieverMutator: Retriever`：增加拥有式 `delete(Key)`；删除成功与否由 `anyhow::Error` 表达。
- `pub type FnKeyCmp = Box<dyn Fn(&Key) -> bool + Send + Sync>`：装箱的只读键谓词，闭包捕获的数据也必须满足线程传递/共享约束。
- `ScanMetaWithPrefix(retriever, prefix, filter)`：扫描入口；`prefix` 按值传入，过滤器是 `FnMut`，可在访问过程中维护局部状态。
- `DelKeyWithPrefix(rm, prefix)`：批量删除入口；通过 `&mut dyn RetrieverMutator` 串行执行删除。
- `RowKeyPrefixFilter(rowKeyPrefix)`：拥有前缀并返回 `FnKeyCmp`，避免闭包借用调用栈数据。

## 执行流程

`ScanMetaWithPrefix` 的流程如下：

1. 计算 `prefix.PrefixNext()`，调用 `Retriever::iter` 打开 `[prefix, upper)` 迭代器；创建失败时立即返回错误。
2. 只要 `valid()` 且当前 `key().HasPrefix(prefix)`，就把当前键和值交给 `filter`。
3. `filter` 返回 `false` 时正常早停且不再调用 `next`；返回 `true` 时调用 `next`，其错误终止扫描。
4. 无论循环正常结束、回调早停还是 `next` 失败，离开内部闭包后都调用一次 `iter.close()`，再返回扫描结果。`close` 没有返回值，所以关闭失败不在接口模型内。

`DelKeyWithPrefix` 的流程如下：

1. 用相同的 `[prefix, prefix.PrefixNext())` 打开迭代器。
2. 在 `valid && HasPrefix` 条件下克隆每个键到 `Vec<Key>`，每次通过 `next` 前进；读取值不是删除所需步骤。
3. 若收集阶段成功，按收集顺序调用 `delete`；第一次删除失败即停止，先前已成功的删除不会回滚。若 `next` 失败，则完全不进入删除循环。
4. 当前实现是在删除循环完成或失败之后才调用 `iter.close()`。第 130 行注释声称“先关迭代器再删”，但第 131—145 行的真实控制流并非如此；扩展或修复时必须以实际控制流为准。

`RowKeyPrefixFilter` 只捕获给定 `Key`，每次调用返回 `!currentKey.HasPrefix(&rowKeyPrefix)`。它的布尔含义是“是否已经离开该行键前缀”，不是通常意义上的“是否匹配此前缀”。

## 数据与状态

所有状态都由参数或局部变量拥有，没有静态可变状态。`Key` 拥有 `Vec<u8>`；`PrefixNext`、`DelKeyWithPrefix` 的键收集以及从切片构造 `Key` 都会分配或复制。扫描仅借用迭代器给出的键和值，回调不能把这些引用带出本次调用。

`ScanMetaWithPrefix` 的可变状态是迭代器位置和 `FnMut` 回调内部状态。`DelKeyWithPrefix` 的关键中间状态是完整的 `Vec<Key>`：空间复杂度为前缀命中键数与键总字节数的线性量级，换取不在枚举阶段修改集合。删除不是事务性的；成功删除的前缀子集属于外部存储状态，发生后续错误时仍然保留。

区间正确性依赖两个条件：存储按与 `Key::Ord` 一致的字节字典序迭代，并遵守 `[start, end)`。额外的 `HasPrefix` 条件可阻止越过前缀，但不能补救错误排序或错误的 `iter` 区间实现。空键和全 `0xff` 键的 `PrefixNext` 均采用“原键后追加零字节”的 Go 兼容边界；尤其对空前缀，得到的区间上界是 `[0]`，不能在没有存储契约证据时把它描述为“扫描所有键”。

## 依赖与调用关系

上游装配关系为 `pkg/util/Cargo.toml` → `pkg/util/lib.rs` → `pub mod prefix_helper`。测试装配还包括 `pkg/util/lib.rs` 中 `#[path = "prefix_helper_test.rs"] mod prefix_helper_test`。直接外部依赖只有 `anyhow::Error`；字节操作、集合类型和闭包 trait 均来自标准库或调用方实现。

RustCodeGraph 的被调用关系显示：

- `ScanMetaWithPrefix` 调用本文件的 `PrefixNext`、`HasPrefix`、`Retriever::iter`，以及迭代器的 `valid`、`key`、`value`、`next`、`close`。
- `DelKeyWithPrefix` 调用同一组区间/迭代接口（不读取 `value`），并额外调用 `RetrieverMutator::delete`。
- `RowKeyPrefixFilter` 只调用 `HasPrefix`。

调用者查询目前只识别到 `pkg/util/cpu_posix_1_aster_unit_test.rs::prefix_helpers_scan_stop_delete_and_compare_like_go` 对三个公开函数的覆盖，没有识别到生产调用边。`pkg/util/prefix_helper_test.rs` 是模块入口明确挂载的独立单元测试；RustCodeGraph 没有把其中调用报告为 callers，因此调用图证据和模块/源码证据需结合看待，不能据图遗漏该测试。

## 错误处理与边界

`Retriever::iter`、`KvIterator::next` 与 `RetrieverMutator::delete` 的错误均原样以 `anyhow::Error` 向上传播，没有增加上下文或重试。`iter` 创建失败时没有已获得的迭代器可关闭。创建成功后，扫描/收集的 `next` 错误仍会走到显式 `close`；删除错误也会中止剩余删除并在返回前关闭迭代器。由于 `close` 返回 `()`，关闭错误无法表达或覆盖主错误。

回调返回 `false` 是成功早停而非错误。无效迭代器会直接结束；算法依赖短路求值，只有 `valid()` 为真才访问 `key`。如果某个 `KvIterator` 在 `valid() == false` 时仍被调用 `key`/`value`，那是实现者违反接口约定，不是本文件主动处理的情况。

`DelKeyWithPrefix` 不保证原子性：第 N 次删除失败时，前 N-1 个键已经删除。收集阶段出错则尚未调用任何删除。重复键、删除不存在键、并发写入时的可见性均由具体 `RetrieverMutator` 决定，本文件没有去重、快照版本或锁协议。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。`KvIterator: Send` 允许迭代器所有权跨线程移动，但函数本身同步使用它；`Retriever`/`RetrieverMutator` 没有 `Send` 或 `Sync` 上界，不承诺存储可并发共享。`FnKeyCmp` 同时要求 `Send + Sync`，而 `ScanMetaWithPrefix` 的临时 `FnMut` 回调没有这两个约束。

迭代器由 `iter` 创建并由函数显式调用 `close`。这不是 RAII 保证：若回调、迭代器方法或删除实现发生 panic，显式 `close` 可能不会运行；默认 `close` 又可能什么都不做。需要强资源保证的适配器应同时通过自己的 `Drop` 管理资源，不能只依赖该回调。`DelKeyWithPrefix` 的迭代器实际活到删除阶段结束，可能比纯扫描所需时间更长。

并发修改的语义没有在 trait 中规定。调用方必须确保迭代器观察到稳定排序，并决定收集完成到删除之间新增、删除或改写键时如何处理；本函数只删除已收集的键，不会二次扫描。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/prefix_helper.go`，测试是 `pkg/util/prefix_helper_test.go`。三个公开助手在流程上保持一致：都以 `PrefixNext` 建立半开区间、以 `HasPrefix` 防止越界；扫描回调可正常早停；删除先克隆键再逐个删除；行键过滤器对“仍有前缀”返回 `false`。Rust 的 `Key::PrefixNext` 还与 `pkg/kv/key.go::PrefixNext` 的逐字节进位、空键/全 `0xff` 追加零语义一致。

主要类型差异是 Go 直接使用 `pkg/kv` 的 `Key`、`Retriever`、`RetrieverMutator` 和 `FnKeyCmp`，Rust 文件则在本地重新定义对应抽象。Go 用 `errors.Trace` 包装迭代和删除错误；Rust 使用 `anyhow::Error` 原样传播，不保留等价的新增调用栈上下文。Go 的 `defer iter.Close()` 与 Rust 的显式 `iter.close()` 都在函数退出附近执行；当前 Rust 删除路径并未实现源码注释所说的“先关闭再删除”。

`pkg/util/prefix_helper_test.go::TestPrefix` 验证实际 mockstore 事务中的扫描早停和按前缀删除，`TestPrefixFilter` 验证含 NUL 字节的前缀。Rust 独立测试 `pkg/util/prefix_helper_test.rs` 用 `BTreeMap`/快照迭代器覆盖相同核心语义，并额外检查密集数字前缀的整段删除；`pkg/util/cpu_posix_1_aster_unit_test.rs` 还有一组重复的迁移回归。Rust 测试没有真实事务、错误注入、关闭顺序或 `PrefixNext` 极端输入覆盖，不能据此证明这些边界已被动态验证。

## 扩展指南

- 若改变前缀上界算法，修改 `Key::PrefixNext`，并在独立的 `pkg/util/prefix_helper_test.rs` 增加空键、普通进位、跨字节进位和全 `0xff` 用例；同时对照 `pkg/kv/key.go`/`pkg/kv/key.rs`，避免两个 Rust `Key` 实现漂移。
- 若增加扫描控制能力，优先扩展 `ScanMetaWithPrefix` 的返回或回调协议，并保留 `false` 表示成功早停的兼容语义。新增测试应放在独立测试文件，不能内嵌到生产源文件。
- 若修正删除期间的资源生命周期，应围绕 `DelKeyWithPrefix` 明确选择“收集后先关闭再删除”或保持 Go 的退出时关闭语义，并用能记录 `close`/`delete` 顺序的测试迭代器验证；不能只改注释。
- 若要求原子删除、错误回滚或并发一致性，必须由具体存储/事务层提供契约。本地 trait 当前没有事务边界，不能用循环补出原子性。
- 若把该模块接入生产路径，应优先评估复用 `pkg/kv/key.rs::Key` 和已有 KV trait，而不是继续维护平行抽象；同时用 RustCodeGraph 复查新增 callers，确认没有跨 crate 类型转换和重复分配风险。
- 性能风险主要来自 `PrefixNext` 的键复制、删除前收集全部键及每键一次删除调用。大前缀批量删除可考虑有界批次，但必须保持 Go 行为、错误边界和资源关闭语义，并补充兼容性测试。

## 验证依据

- 源码：`pkg/util/prefix_helper.rs`，核对了 `Key`、三个 trait、`FnKeyCmp`、三个公开函数和完整控制流。
- crate 与模块：`pkg/util/Cargo.toml`、`pkg/util/lib.rs`，确认 crate 名、`anyhow` 依赖、公开模块和独立测试装配。
- Go 对照：`pkg/util/prefix_helper.go`、`pkg/kv/key.go::PrefixNext`、`pkg/util/prefix_helper_test.go`。
- Rust 对照与测试：`pkg/kv/key.rs::{PrefixNext, HasPrefix}`、`pkg/util/prefix_helper_test.rs`、`pkg/util/cpu_posix_1_aster_unit_test.rs::prefix_helpers_scan_stop_delete_and_compare_like_go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；对目标文件执行了 `node --file`，对 `ScanMetaWithPrefix`、`DelKeyWithPrefix`、`RowKeyPrefixFilter`、`PrefixNext` 执行了精确 `query`/`callers`/`callees`（用 `--file pkg/util/prefix_helper.rs` 消除 Go/Rust 同名歧义）。图确认内部依赖和当前唯一识别出的测试调用者；模块源码补充了图未报告的 `prefix_helper_test.rs` 测试装配证据。
- 静态人工复核确认：文件没有条件编译项、全局可变状态、异步/锁/通道；错误点仅来自 `iter`、`next`、`delete`；关闭调用位于成功取得迭代器后的公共退出路径，但 panic 不受保护。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且固定二级章节恰好为 11 个。
