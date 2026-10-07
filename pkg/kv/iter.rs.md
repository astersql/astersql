# `pkg/kv/iter.rs`

## 文件定位

[`iter.rs`](iter.rs) 属于 `astersql-kv` crate（`pkg/kv/Cargo.toml`，库入口为 `pkg/kv/lib.rs`）。`pkg/kv/lib.rs:529-534` 在公开 `iter` 模块中以 `include!("iter.rs")` 纳入本文件，并通过 `pub use iter::*` 把唯一入口 `NextUntil` 重导出到 crate 根；调用方因此使用 `kv::NextUntil` 或 `astersql_kv::NextUntil`。

它位于 KV 游标抽象与上层扫描循环之间，是一个同步的“按谓词推进游标”助手：逐项检查当前键，在谓词首次命中、迭代器失效或推进失败时停止。它不创建迭代器、不读取值、不决定扫描上下界，也不拥有关闭游标的责任。

## 核心职责

- 在访问当前键前先调用 `Iterator::Valid`，避免对失效游标调用 `Key`。
- 把当前 `Key` 交给调用方谓词；谓词返回 `true` 时保留当前位置并成功返回。
- 谓词未命中时恰好调用一次 `Iterator::Next`，重复上述判断，直到命中或自然耗尽。
- 用 `?` 原样传播底层 `Next` 的 `errors::SharedError`；失败后不再比较或推进。
- 不调用 `Iterator::Value` 或 `Iterator::Close`。关闭及后续使用仍由拥有游标的外层代码负责。

## 主要符号

- `pub fn NextUntil(it: &mut dyn Iterator, mut fnKeyCmp: impl FnMut(Key) -> bool) -> Result<(), Error>`（`iter.rs:22-34`）：文件内唯一的类型/函数级符号，也是公开 API。`&mut dyn Iterator` 表明函数借用而不取得游标所有权，并通过可变借用保证本次推进期间不存在其他安全 Rust 可变访问。
- `Iterator` 定义于 `pkg/kv/kv.rs:923-929`，提供 `Valid`、`Key`、`Value`、`Next`、`Close`；本函数只使用前三者中的 `Valid`、`Key`，以及可失败的 `Next`。
- `Key` 定义于 `pkg/kv/key.rs:29-32`，是拥有 `Vec<u8>` 的 tuple struct。`Iterator::Key` 与谓词参数都按值返回/接收，因此每次比较可能发生键克隆或分配，具体成本由迭代器实现决定。
- 返回类型 `Error` 是 `pkg/kv/error.rs:24-25` 的 `errors::SharedError` 别名；与 `Iterator::Next` 的错误类型一致，不需要转换。

本文件没有常量、结构体、枚举、trait、宏、私有辅助函数或条件编译项。相邻 `pkg/kv/kv.rs:920` 仍定义了 Go 对齐的 `FnKeyCmp = fn(Key) -> bool`，但本函数签名直接采用更宽泛的 `impl FnMut(Key) -> bool`，并未引用该别名。

## 执行流程

1. 对 `it` 调用 `Valid()`。若当前已经无效，短路退出循环并返回 `Ok(())`；此时不会访问 `Key`、调用谓词或调用 `Next`。
2. 若有效，调用 `Key()` 取得当前键，再调用 `fnKeyCmp`。谓词返回 `true` 时，`!fnKeyCmp(...)` 为假，函数保留命中项作为当前项并返回 `Ok(())`。
3. 谓词返回 `false` 时，调用一次 `it.Next()`。成功则回到步骤 1，所以即使 `Next` 让游标失效，也不会再读取键。
4. `Next` 返回错误时，`?` 立即返回该错误；循环终止，函数不尝试恢复、关闭或再次推进。
5. 若若干次成功推进后 `Valid()` 变为 `false`，按“未找到匹配项但正常耗尽”处理，返回 `Ok(())`。调用方需要在返回后自行检查 `Valid()`，才能区分“命中”与“耗尽”。

当前生产例子位于 `pkg/session/runtime/system_session.rs:2043-2044,2205-2207`：索引回填扫描在处理一条行记录后，以“不再具有当前行键前缀”为停止谓词，跳过同一行的后续相关 KV 项。外层在生成流程结束后统一调用 `iterator.Close()`（同文件 `2215`），印证本函数不负责资源释放。

## 数据与状态

本文件不保存全局状态或内部集合。可观察状态全部位于两个调用方提供的对象中：游标当前位置由 `Iterator::Next` 修改；闭包捕获的状态可由 `FnMut` 在每次比较时修改。`pkg/kv/iter_test.rs::next_until_accepts_a_state_capturing_predicate` 使用计数器证明闭包会对访问过的每个有效键执行一次，并在第二个键命中后将游标停在位置 1。

关键不变量是调用顺序：每轮严格为 `Valid -> Key -> predicate`，仅在谓词为假时追加 `Next`。因此空迭代器执行零次比较；当前项立即命中执行一次比较、零次推进；没有匹配项时每个有效项各比较一次、推进一次。

函数把 `Key` 按值交给谓词，谓词不能借用并修改迭代器本身；但它可以修改自身捕获状态。函数不缓存原键，也不在 `Next` 失败后回滚位置，失败时的确切游标状态遵循具体 `Iterator` 实现的契约。

## 依赖与调用关系

直接下游依赖全部来自 `pkg/kv/lib.rs:530-532` 的 `use crate::*`：`Iterator`、`Key` 和 `Error`。算法只做 trait 动态分派和闭包调用，不直接依赖 `pkg/kv/Cargo.toml` 中的外部 crate；`default`、`nextgen`、`test-fixtures` feature 均不改变本文件的编译内容。

已核实的 Rust 生产调用边为：

- `pkg/session/runtime/system_session.rs:2043-2044`：读取行键对应值遇到 `ErrNotExist` 时，跳过仍共享该行键前缀的项后继续索引回填。
- `pkg/session/runtime/system_session.rs:2205-2207`：成功生成一行的索引记录后，用相同前缀谓词推进到下一行。
- `pkg/util/admin/admin.rs:379-384` 保存了 Go `NextUntil(RowKeyPrefixFilter(...))` 的行为意图，但当前 Rust 实现以切片位置循环内联相同的前缀跳过逻辑，并不是本函数的调用者。

RustCodeGraph 对精确 Rust 函数节点的 `callers`/`callees` 返回空数组，未把 `include!`、动态 `Iterator` 方法和闭包调用恢复成符号边；文件节点则报告 `iter.rs` 被独立测试和 `system_session.rs` 使用。以上生产调用点因此由精确文本引用及函数体读取补证，不把空图结果误解释为“没有调用”。

## 错误处理与边界

- 唯一错误来源是 `Iterator::Next`。函数不包装、不记录、不分类错误，直接返回同一个 `SharedError`；谓词自身不能返回错误。
- 空迭代器、当前项立即命中和正常耗尽都返回 `Ok(())`。API 不返回是否命中的布尔值，调用方若关心结果必须检查返回后的 `Valid` 和当前 `Key`。
- `Valid`、`Key`、谓词若由实现或调用代码触发 panic，本函数不捕获；Rust trait 也不能表达 Go 风格的 nil iterator，安全调用必须提供真实的 `&mut dyn Iterator`。
- `Next` 失败后的游标位置不由本函数保证。`pkg/kv/assertion_1_aster_unit_test.rs:300-311` 的测试迭代器在失败前不推进，并据此断言位置仍为 0；这验证的是当前助手会立即停止，不应推广为所有底层迭代器都具备事务式推进。
- 若谓词永远返回 `false`，正确终止依赖 `Next` 最终使 `Valid` 变假或返回错误；违反该 trait 进度约定的迭代器可能造成无限循环。
- `Key` 是拥有型值。高频扫描中若实现每次克隆较大的键，比较成本可能显著；本函数没有引用式谓词接口或批量推进优化。

## 并发与资源生命周期

`NextUntil` 不创建线程、异步任务、锁、通道、事务或 I/O 资源。它同步占用游标的独占可变借用直至返回；是否执行网络/磁盘 I/O、持锁或阻塞完全取决于具体 `Iterator::Key`/`Next` 实现。

函数既不拥有也不关闭游标。调用方必须保证在成功、错误以及自身后续处理失败的所有路径上调用 `Close`；`pkg/session/runtime/system_session.rs:2020-2217` 先把扫描结果收集为 `generated`，随后无条件执行 `iterator.Close()`，最后才用 `generated?` 传播扫描错误，避免 `NextUntil` 错误绕过清理。

闭包仅在本次同步调用内使用，没有 `'static`、`Send` 或 `Sync` 要求，可以安全捕获局部可变状态；该设计不意味着同一个迭代器能跨线程并发推进。是否可跨线程共享由具体迭代器类型及其外层所有权决定。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/kv/iter.go:17-29`，接口对照位于 `pkg/kv/kv.go:926-938`。两边循环条件和错误顺序一致：先 `Valid`，再对 `Key` 应用谓词，未命中才 `Next`；`Next` 出错立即返回，命中或耗尽返回成功；都不访问 `Value`，也不调用 `Close`。

接口表达存在两点语言差异：Go 参数类型是命名函数类型 `FnKeyCmp func(Key) bool`，Rust `pkg/kv/kv.rs` 虽保留 `FnKeyCmp = fn(Key) -> bool`，`NextUntil` 实际接受 `impl FnMut(Key) -> bool`。这允许 Rust 调用方传入捕获并修改状态的闭包，能力严格宽于函数指针；`pkg/kv/iter_test.rs` 专门覆盖这一点。Go 的 `error` 可为 `nil`，Rust 用 `Result<(), Error>` 分别表达成功和失败。

Go 当前生产调用还包括 `pkg/util/admin/admin.go:256`、`pkg/ddl/backfilling.go:1202` 和 `pkg/table/tables/tables.go:1401`，均用于跳过共享记录键前缀的 KV 项。Rust 只有 `system_session.rs` 两处直接调用；`admin.rs` 已以内联数组位置推进实现同一效果，不能据 Go 调用清单声称所有链路都已通过本 Rust 函数接线。

仓库没有 `pkg/kv/iter_test.go`；Go 行为依据来自生产实现与这些真实调用点，Rust 边界由独立的 `.rs` 测试覆盖。本任务是纯文档分析，未运行 Cargo，不能把源码阅读等同于测试已执行。

## 扩展指南

- 若改变停止条件或调用顺序，应同时核对 `pkg/kv/iter.go`，并在独立的 `pkg/kv/iter_test.rs` 增加空、立即命中、耗尽及状态捕获用例；错误和位置语义继续放在 `pkg/kv/assertion_1_aster_unit_test.rs` 等独立测试文件，不要把测试嵌入生产 `iter.rs`。
- 若需要谓词报告错误，可新增明确返回 `Result<bool, _>` 的 API，而不要悄悄改变现有 `NextUntil` 的闭包契约；同时决定谓词错误与 `Next` 错误的优先顺序，并评估 Go API 兼容性。
- 若需要知道是否命中，优先在调用方依据 `Valid`/`Key` 判断，或设计一个语义明确的新返回类型。直接把现有返回值改成布尔值会影响 crate 根公开 API 及 `system_session.rs` 调用。
- 若想减少 `Key` 克隆，必须先调整 `Iterator::Key` 的 crate 级 trait 契约及所有实现，影响范围远超本文件；不能只把谓词改成借用，因为当前键已经由 trait 按值返回。
- 若新增取消、最大推进次数或异步推进，应明确谁拥有 `Close`，并为错误、取消、耗尽和命中各自验证资源清理。现有函数应保持轻量、同步和 Go 对齐，避免把扫描策略或重试逻辑塞入通用 KV 助手。
- 性能审查应关注动态分派次数、每项键克隆以及底层 `Next` I/O；兼容性审查应保持“命中项不被越过”“先验证再取键”“错误立即返回”三个不变量。

## 验证依据

- 目标源码与公共类型：`pkg/kv/iter.rs::NextUntil`、`pkg/kv/kv.rs::Iterator`/`FnKeyCmp`、`pkg/kv/key.rs::Key`、`pkg/kv/error.rs::Error`。
- crate 边界与装配：`pkg/kv/Cargo.toml`；`pkg/kv/lib.rs:529-537` 的 `include!`、重导出和独立 `iter_test` 装配。
- Rust 生产调用：`pkg/session/runtime/system_session.rs:2043-2044,2205-2207`；资源关闭证据为同文件 `2215-2217`。
- Rust 独立测试：`pkg/kv/iter_test.rs::next_until_accepts_a_state_capturing_predicate`；`pkg/kv/assertion_1_aster_unit_test.rs::next_until_stops_on_match_and_propagates_next_error`，覆盖命中、空输入、立即命中、耗尽和推进错误。
- Go 对照与调用：`pkg/kv/iter.go::NextUntil`、`pkg/kv/kv.go::FnKeyCmp`/`Iterator`、`pkg/util/admin/admin.go`、`pkg/ddl/backfilling.go`、`pkg/table/tables/tables.go`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件、4415 个 Go 文件；`node --file pkg/kv/iter.rs` 确认 34 行源码及唯一函数；`query NextUntil` 区分 Go/Rust 两个定义；精确 Rust 节点的 `callers`/`callees` 均为空，故调用关系再用上述源码引用交叉验证。
- 本次按任务约束不运行 Cargo。交付结构以固定 11 个二级标题检查，并人工复核文档能说明文件存在原因、执行顺序、真实接线、错误/资源边界和安全扩展点。
