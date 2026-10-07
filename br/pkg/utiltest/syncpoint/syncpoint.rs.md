# `br/pkg/utiltest/syncpoint/syncpoint.rs`

## 文件定位

本文件是 `astersql-br-pkg-utiltest-syncpoint` 库 crate 的有序 failpoint 编排实现，源码由同目录的 [`lib.rs`](./lib.rs) 以 `pub mod syncpoint` 装入并通过 `pub use syncpoint::*` 重新导出。它服务于并发测试：多个线程即使乱序命中 failpoint，也只有当前声明的步骤能够推进并执行回调。它不参与 BR 的备份、恢复生产路径，也不实现 failpoint 注入本身。

[`Cargo.toml`](./Cargo.toml) 将 crate 定义为 library，入口为 `lib.rs`，直接依赖本仓库的 `astersql-testkit-testfailpoint` 和启用 `failpoints` feature 的 `fail 0.5.1`。根 `Cargo.toml` 把该目录列为 workspace member；仓库内没有其他 Cargo manifest 声明对该 crate 的依赖，当前可验证的 Rust 使用者是本 crate 的独立测试 [`syncpoint_test.rs`](./syncpoint_test.rs) 与 [`parity_test.rs`](./parity_test.rs)。

## 核心职责

- `Step` 把完整 failpoint 路径和一个零参数、无返回值、可跨线程共享的回调组装为 `StepDecl`。
- `Script::BeginSeq` 校验并注册一组步骤，将其安装为当前唯一活跃序列，同时监听上下文取消。
- failpoint 命中后，注册包装器调用 `advance`。若命中名称不是当前期望步骤，线程在条件变量上等待；匹配时先推进游标，再把回调返回给包装器在锁外执行。
- `Script::EndSeq` 校验取消错误和完成度，然后清空活跃序列，使同一个 `Script` 可再次使用。
- `Script` 持有每个名称对应的 `FailGuard`；guard 随 `Script` 生命周期存在，避免每轮序列重复注册，并在 `Script` drop 时禁用 failpoint。

模块刻意只支持 `Fn()`。Go 版本通过 `any`、`reflect.Type` 和 `reflect.MakeFunc` 支持带参数的回调；Rust 版本受底层 `enable_call` 零参数回调接口约束，不是通用的 Go 反射调用替代品。

## 主要符号

- `pub type StepFn = Arc<dyn Fn() + Send + Sync + 'static>`：可安全共享给 failpoint 触发线程的用户回调。`Arc` 让 `advance` 能在持锁期间克隆句柄、随后在锁外调用。
- `pub struct StepDecl { name, fn_value }`：公开但字段私有的声明对象，只能通过 `Step` 正常构造。
- `pub fn Step<F>(name, fn_value) -> StepDecl`：公开构造器。类型系统已经保证回调非空、零参数、无返回值且满足 `Send + Sync + 'static`；名称是否为空延迟到 `BeginSeq` 的准备阶段检查。
- `struct ActiveStep`：一次活跃序列中的名称和回调。它与声明分离，使 `BeginSeq` 能先完成注册，再整体安装序列。
- `struct RegisteredStep { _guard: FailGuard }`：注册资源的生命周期锚点。下划线字段表示业务代码不读取它，但 drop 语义必需。
- `pub struct Script`：对外协调器，包含共享的 `state: Arc<State>` 和独立的注册表互斥锁 `registered`。
- `struct State` / `struct StateInner`：前者组合 `Mutex<StateInner>` 与 `Condvar`；后者保存 `seq`、下一步骤下标 `next`、取消错误 `err` 和取消监听句柄 `stop_watch`。
- `pub fn New() -> Script`：构造空脚本。与 Go `New(testing.TB)` 不同，Rust 不保存测试对象；失败通过 panic 报告，资源清理由 RAII 完成。
- `Script::BeginSeq`、`Script::EndSeq`：序列生命周期的公开边界。
- `Script::prepare_step`、`Script::register`：校验名称、保证同名只注册一次并生成 `ActiveStep` 的内部接线。
- `advance(state, name) -> Option<StepFn>`：failpoint 命中时的排序状态机；`None` 表示当前不应执行回调。
- `fatal(msg) -> !`：用 panic 模拟 Go 测试对象的 `Fatal`/`Fatalf`。

本文件没有模块级常量、trait、enum 或条件编译项；测试条件编译位于 `lib.rs`，不在本文件内。

## 执行流程

1. 测试调用 `New`，得到空 `StateInner`、条件变量及空注册表。
2. 测试用 `Step(完整路径, callback)` 创建声明，并把非空 `Vec<StepDecl>` 和 `Some(&Context)` 传给 `BeginSeq`。
3. `BeginSeq` 先短暂读取序列锁，拒绝已有活跃序列；随后不持有序列锁逐项调用 `prepare_step`。这避免在底层 failpoint 注册过程中与 `advance` 互相等待。
4. `prepare_step` 拒绝空名称，`register` 在 `registered` 锁下复用已有 guard，或调用 `astersql_testkit_testfailpoint::enable_call` 注册包装器。包装器捕获 `Arc<State>` 和名称。
5. 所有步骤准备完成后，`BeginSeq` 再次取得序列锁，二次拒绝并发安装；停止旧 watcher，安装步骤，重置 `next = 0` 和 `err = None`，再通过 `stubs::after_func` 建立取消监听。
6. 任意线程 inject 已注册 failpoint 时，包装器调用 `advance`。无活跃序列、已有错误或序列已完成均返回 `None`。名称不等于 `seq[next].name` 时在 `Condvar` 上释放互斥锁并等待。
7. 名称匹配时，`advance` 克隆回调、先增加 `next`；完成最后一步时停止 watcher，随后 `notify_all` 唤醒错序等待者并返回回调。包装器退出状态锁后才执行回调，因此后一步回调可以在前一步回调结束前启动。
8. 调用者观察完步骤副作用后调用 `EndSeq`。该方法不会等待序列完成；它停止 watcher，检查 `err` 为空且 `next == seq.len()`，成功后重置活跃状态。序列不完整、被取消或根本没有活跃序列都会 panic。

取消流程与命中流程共享同一状态锁。上下文取消回调只在序列仍活跃、尚未完成且尚无错误时记录包含步骤下标、名称和上下文原因的错误，并 `notify_all`，使错序等待线程从 `advance` 返回而不执行用户回调。

## 数据与状态

`StateInner::seq` 非空即表示存在活跃序列；代码没有单独的布尔标志。`next` 始终指向下一期望步骤，正常范围为 `0..=seq.len()`：匹配命中将它单调增加，达到长度表示所有步骤已被放行。`err` 只记录序列取消错误，首次写入后不再覆盖；`stop_watch` 对应当前序列的 `after_func` 监听。

`registered` 与活跃序列是两个生命周期不同的状态域。`EndSeq` 清空 `seq`，但不删除 `registered`，所以后续序列可复用同名 guard；此时命中旧 failpoint 会进入 `advance`，因 `seq.is_empty()` 被静默忽略。只有 `Script` drop 才释放全部 `FailGuard`。

同名步骤可以在多轮序列中使用，注册表只缓存 guard，不缓存某一轮的回调；包装器每次根据当前 `seq[next]` 取得实际回调。一个序列内若重复同名，连续命中同一个 failpoint可以依次推进这些位置，这是现有状态机自然允许的行为，测试未单独锁定这一边界。

## 依赖与调用关系

上游装配关系为 `lib.rs -> syncpoint.rs`，并由 `lib.rs` 在 crate 根重新导出 `Context`、`New`、`Step` 等符号。当前直接调用证据来自：

- [`syncpoint_test.rs`](./syncpoint_test.rs)：构造三步乱序并发序列，验证回调按 `a -> b -> c` 放行、相邻回调可重叠，以及序列外命中被忽略。
- [`parity_test.rs`](./parity_test.rs)：进一步覆盖空序列/空上下文 panic、取消唤醒、`EndSeq` 后复用，以及 drop 时移除 guard。

内部调用主链是 `BeginSeq -> prepare_step -> register -> enable_call`；运行时注入主链是 `testfailpoint::inject -> 注册包装器 -> advance -> StepFn`；取消主链是 `BeginSeq -> stubs::after_func -> 写入 StateInner::err + Condvar::notify_all`；结束主链是 `EndSeq -> StopWatch::stop -> 完成度校验 -> 状态重置`。

下游依赖包括标准库的 `Arc`、`Mutex`、`Condvar` 和 `HashMap`，`astersql_testkit_testfailpoint::{enable_call, FailGuard}`，以及同 crate [`stubs.rs`](./stubs.rs) 的 `Context`、`StopWatch`、`after_func`。RustCodeGraph 为目标文件列出 25 个符号，并能定位 `Step`、`BeginSeq`、`EndSeq`、`prepare_step`、`advance`；对这些节点执行 `callers`/`callees` 未返回静态边，因此本节调用关系同时由源码节点、模块入口和直接测试引用核验。图工具列出的其他“used by”文件存在通用符号同名候选，未发现相应 crate 依赖或真实 `syncpoint` API 引用，不能视为已接线调用者。

## 错误处理与边界

所有契约违反都经 `fatal` panic：`BeginSeq(None, ...)`、空步骤列表、活跃序列上再次 `BeginSeq`、空步骤名、无活跃序列时 `EndSeq`、取消错误以及未完成序列。互斥锁或条件变量 poisoned 时，代码中的 `unwrap()` 也会 panic；该测试工具没有恢复 poisoned 状态的分支。

`EndSeq` 注释中的“waits”沿用 Go API 描述，但实现不在条件变量上等待。调用者必须先通过通道、join 或其他副作用确认步骤已推进，再调用它，否则会得到 `syncpoint sequence incomplete` panic。上下文取消只唤醒已经进入错序等待的命中；尚未命中的步骤不会被主动启动。

无活跃序列和已完成序列期间命中注册点会静默返回，不算错误。取消后命中或被唤醒的线程也不执行回调，取消错误留给 `EndSeq` 暴露。完整 failpoint 名称必须由调用者提供；Rust 不执行 Go failpoint 代码生成器的短名展开。

与 Go 相比，Rust 类型系统排除了 nil、非函数、带参数和有返回值的 callback，因此没有对应的运行时反射错误；代价是无法编排带 failpoint 参数的回调。并发 `BeginSeq` 的失败方可能已把新名称注册进 `registered`，但不会安装其序列；这些额外 guard 会保留至 `Script` drop，这是防竞态二次检查下可见的资源边界。

## 并发与资源生命周期

`State::mu` 串行化序列安装、推进、取消与结束，`Condvar` 只负责错序命中的等待/唤醒。等待使用循环重新检查 `err`、完成度和名称，能处理虚假唤醒。`registered` 使用另一把互斥锁，`BeginSeq` 在注册阶段不持有 `State::mu`；这形成明确的锁域分离，避免底层注册回调与 `advance` 形成锁顺序环。

匹配时先推进 `next`、释放锁后再执行用户回调是关键并发契约：它保证排序的是“回调获准开始”，不是“回调完成”。因此步骤回调之间允许重叠，用户若要求完成顺序，必须在回调或测试中增加自己的同步。

每个活跃序列创建一个 `after_func` 后台监听。最后一步推进或 `EndSeq` 会调用 `StopWatch::stop`，避免序列成功后迟到的取消写入错误；取消和 stop 的一次性竞态语义由 `stubs.rs` 的原子标志实现。`RegisteredStep` 的 `FailGuard` 在 `Script` 存活期间保持全局 failpoint 注册，因此使用全局 failpoint 表的测试应像现有测试一样串行化；本 crate 的 `TEST_LOCK` 位于 `lib.rs`，只在 `cfg(test)` 下存在。

回调、状态和名称都满足跨线程所有权要求：状态由 `Arc` 共享，回调是 `Send + Sync + 'static`。`Script` 自身没有显式 `Drop`；字段按 RAII 顺序释放，注册表中的 guard 负责注销。若在未 `EndSeq` 时直接 drop，guard 仍会释放，但 watcher 的闭包持有独立的 `Arc<State>`，直到被停止或上下文取消后才结束；调用者应正常结束或取消序列以避免延长后台监听生命周期。

## 与 Go 版本的对应关系

直接语义来源是同目录 [`syncpoint.go`](./syncpoint.go)，测试对照是 [`syncpoint_test.go`](./syncpoint_test.go)。Rust 保留了 `StepDecl`/active step/registered step、`Script`、互斥状态、条件变量、`BeginSeq`、`EndSeq`、步骤准备、一次注册、错序等待、取消广播、完成后复用等结构和顺序。

主要差异如下：

- Go `New(t testing.TB)` 保存测试接口，用 `Helper`、`Fatalf`、`require` 报错，并借测试 cleanup 禁用 failpoint；Rust `New()` 不接收测试对象，用 panic 和 `FailGuard` drop 完成对应职责。
- Go `Step(name string, fn any)` 通过反射支持不同参数签名，并检查 nil、函数种类、零返回值及同名签名一致；Rust `Step<F>` 只接受 `Fn()`，大部分检查在编译期完成，同名注册无需保存类型。
- Go 的 `context.Context` / `context.AfterFunc` 被本 crate 的轻量 `Context` / `after_func` 替代，以避免引入 BR 完整上下文依赖；这里只覆盖本协调器需要的取消语义。
- Go 的生成式 `failpoint.InjectCall("short-name")` 会扩展包路径；Rust 测试必须显式拼出并注入同一个完整路径。
- Go `register` 使用状态锁保护注册表；Rust 注册表有独立锁，并让注册发生在序列锁外。两者都保证单个脚本内同名只注册一次，但 Rust 的锁分工也服务于底层零参回调注册模型。

现有 Rust `syncpoint_test.rs` 复刻 Go 的核心正常与忽略场景，并额外验证回调可重叠；`parity_test.rs` 补充 Go 公开契约的边界、取消和资源清理证据。未验证 Rust 对带参数 Go callback 的兼容，因为当前 Rust API 明确不支持该能力。

## 扩展指南

新增排序或生命周期能力时，优先从 `StateInner` 的不变量和 `advance` 状态机入手，并同步检查 `BeginSeq` 的安装/取消竞态及 `EndSeq` 的重置路径。若增加新的持久状态，必须明确它属于单轮序列还是整个 `Script`；前者应在成功结束时重置，后者应有 drop 清理策略。

若要支持带参数回调，不能只放宽 `StepFn`：还需确认 `astersql_testkit_testfailpoint::enable_call` 的调用模型，设计类型安全的参数传递和同名签名一致性检查，并补齐 Go `reflect.MakeFunc` 当前承担的行为。不要用无参闭包吞掉参数来声称对齐。

修改等待规则时必须保持循环谓词完整，覆盖取消、序列完成、当前名称三类唤醒条件；用户回调应继续在状态锁外执行，否则容易阻塞其他步骤或造成重入死锁。修改 watcher 生命周期时，应同时证明成功完成、显式结束、取消、未结束 drop 四条路径不会让迟到回调污染下一轮。

测试应继续放在独立文件，不要内嵌到 `syncpoint.rs`。核心行为更新同步修改 [`syncpoint_test.rs`](./syncpoint_test.rs)，Go/Rust 契约差异或边界更新同步修改 [`parity_test.rs`](./parity_test.rs)；若改变对外语义，还应对照 [`syncpoint.go`](./syncpoint.go) 和 [`syncpoint_test.go`](./syncpoint_test.go)。兼容风险集中在 panic 文案、完整名称规则和 Go/Rust API 差异；性能风险主要是全局注册数量、锁竞争、错序线程数量和每轮 watcher 线程生命周期。

## 验证依据

- RustCodeGraph `status`：当前索引包含 7,032 个 Rust 文件；`files --filter br/pkg/utiltest/syncpoint` 找到该目录 7 个已索引源码文件。
- RustCodeGraph `node --file br/pkg/utiltest/syncpoint/syncpoint.rs --offset 1 --limit 400`：读取目标文件 282 行及文件使用候选；精确 `query` 定位 `Step`（第 65 行）、`BeginSeq`（第 123 行）、`EndSeq`（第 187 行）、`prepare_step`（第 215 行）和 `advance`（第 248 行）。对这些节点运行 `callers`/`callees` 无静态边输出，故没有把宽泛候选当成真实调用关系。
- 源码与装配：[`syncpoint.rs`](./syncpoint.rs)、[`lib.rs`](./lib.rs)、[`stubs.rs`](./stubs.rs)、[`Cargo.toml`](./Cargo.toml) 以及根 `Cargo.toml` 的 workspace member 声明。
- Go 对照：[`syncpoint.go`](./syncpoint.go)、[`syncpoint_test.go`](./syncpoint_test.go)。
- Rust 独立测试：[`syncpoint_test.rs`](./syncpoint_test.rs)、[`parity_test.rs`](./parity_test.rs)。这些文件覆盖乱序放行、锁外回调重叠、序列外忽略、输入 panic、取消唤醒、脚本复用和 guard drop。
- 仓库引用核验：对 `astersql-br-pkg-utiltest-syncpoint`、`astersql_br_pkg_utiltest_syncpoint`、`BeginSeq(`、`EndSeq(` 及 syncpoint 导入执行 `rg`；除 workspace member、crate 自身入口与上述独立测试外，未发现实际 Cargo 依赖或外部 API 调用。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时执行任务指定的结构命令，确认文件存在且固定二级标题恰好 11 个；人工复核重点是职责边界、状态推进、取消与资源清理、Go 差异和安全扩展入口均可由上述文件与符号反查。
