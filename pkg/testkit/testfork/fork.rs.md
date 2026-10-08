# `pkg/testkit/testfork/fork.rs`

## 文件定位

[`fork.rs`](./fork.rs) 是 `astersql-testkit-testfork` crate 的组合测试枚举器实现。crate 入口 [`lib.rs`](./lib.rs) 通过 `pub mod fork` 声明模块并用 `pub use fork::*` 把本文件的公开 API 提升到 crate 根；仓库根 [`pkg/lib.rs`](../../lib.rs) 又在 `testkit::testfork` facade 下重新导出该 crate。它服务于测试用例的参数组合展开，不参与 SQL 接收、规划、执行或存储等生产运行时主链。

[`Cargo.toml`](./Cargo.toml) 将 crate 的库入口设为 `lib.rs`，没有运行时依赖或 feature，并以 `package.metadata.porting.go-package = "pkg/testkit/testfork"` 指向同目录 Go 包。当前 Rust 源码中的直接调用集中在独立测试 [`fork_test.rs`](./fork_test.rs) 和 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)；`pkg/sessiontxn/Cargo.toml` 与 `pkg/sessiontxn/isolation/Cargo.toml` 把该 crate 声明为开发依赖，但当前相应 Rust 测试尚未调用其 API。Go 侧则已在 `pkg/sessiontxn` 的多项事务测试中使用同路径 `testfork` 包。

## 核心职责

本文件解决“一个测试体内按执行路径动态声明多层候选值，并穷尽所有实际可达组合”的问题：

1. `Pick` / `PickEnum` 在测试闭包执行到某个选择点时，为该深度登记候选集合并返回当前候选。
2. `pickStack::NextStack` 从最深选择层向前做类似混合进制的进位，使下一轮闭包取得下一组值。
3. 选择层可依赖前面已经选出的值，因此每轮实际走到的分支决定本轮栈的深度和候选类型；这不是预先计算固定维度的笛卡尔积。
4. `RunTest` 驱动闭包反复执行，输出失败组合，并保证某一组合 panic 后仍访问其余组合，最后再恢复首个 panic。

核心不变量是：`pickStack.stack[i][0]` 始终是第 `i` 层当前选择；`pos` 是本轮闭包下一次 `Pick` 所在的深度；一轮结束后 `NextStack` 必须把 `pos` 重置为零。该行为由 `fork_test.rs::TestForkSubTest` 和 `migration_aster_unit_test.rs::run_test_enumerates_the_same_branch_dependent_cartesian_product_as_go` 的 12 组分支依赖组合验证。

## 主要符号

- `AnyValue { value: Box<dyn Any>, text: String }`：保存类型擦除后的候选值及其失败诊断文本。`AnyValue::new<E: Debug + 'static>` 对 `String` 和 `&str` 加双引号，其他类型使用 `Debug`；私有 `downcast_ref<E>` 在返回强类型结果时恢复引用。其自定义 `Debug` 实现只写出预先生成的 `text`。
- `PickError`：`PickValue` 的结构化错误。`EmptyValues` 表示没有候选值；`IllegalState { pos, stack_len }` 表示选择深度跳过了尚未建立的层。它实现 `Display`、`Error`、`PartialEq` 和 `Eq`。
- `pickStack { stack, pos, valid }`：枚举状态机。类型名及方法名为对齐 Go 而保留非 Rust 惯用命名，并由 `lib.rs` 的 crate 级 allow 放宽 lint。
- `newPickStack() -> Box<pickStack>`：创建空栈并令 `valid=true`，因此 `RunTest` 即使还没有候选层也会先执行一次闭包。
- `pickStack::PickValue`：检查候选和状态；首次到达新深度时保存整组候选，随后返回该层第一个值。每次成功取值都会把 `pos` 加一。
- `pickStack::NextStack`：消费最深层的当前值；若该层耗尽则删除该层并继续向前进位，最后重置 `pos`，并以栈是否非空更新 `valid`。
- `pickStack::Values` / `ValuesText`：分别返回各层当前值引用和形如 `["a" 10]` 的失败诊断串。
- `pickStack::Valid`：暴露枚举循环条件。
- `T<'a> { stack: &'a mut pickStack }`：把一轮闭包中的所有选择共享到同一状态机。它不是通用测试上下文，也不持有 Rust 测试框架句柄。
- `RunTest<F: FnMut(&mut T)>`：顶层驱动入口。它逐轮构造 `T`、捕获 panic、记录首个失败、推进栈，穷举结束后恢复首个 panic。
- `Pick<E: Clone + Debug + 'static>`：把强类型候选包装为 `AnyValue`，经 `PickValue` 选取，再向下转型并克隆为 `E`。
- `PickEnum<E>`：接受一个必选首项和其余候选向量，合并后委托 `Pick`；它保证调用者至少提供一个候选。

## 执行流程

以 `RunTest(|t| { let x = Pick(...); let y = PickEnum(...); ... })` 为例：

1. `newPickStack` 创建 `stack=[]、pos=0、valid=true`，所以循环进入第一轮。
2. 第一处 `Pick` 调用 `AnyValue::new` 包装候选；`PickValue` 发现 `pos == stack.len()`，把候选向量压入第 0 层，递增 `pos`，返回该层索引 0 的值。
3. 后续选择点依次建立更深层。若控制流根据前序值走不同分支，压入的是该分支实际提供的候选集合。
4. 闭包正常结束或 panic 后，`RunTest` 都调用 `NextStack`。该方法先删除最深层的当前候选；该层还有值时停止，否则删除空层并继续向前进位。
5. 下一轮 `pos` 从 0 开始。已有层不会用本轮传入的新向量替换，而是继续使用栈中剩余候选；走到被上轮进位删除的深度时，新的分支候选才会重新压栈。
6. 当所有层都耗尽，`stack` 为空、`valid=false`，循环退出。若任一轮 panic，恢复第一次捕获的 panic；否则正常返回。

由此产生的是深度优先、最后一层变化最快的顺序。独立 Rust/Go 测试都验证了 `x={1,2,3}`、`y={a,b}` 且第三层随 `x` 切换类型时的 12 项顺序。

## 数据与状态

`pickStack` 是单次 `RunTest` 调用内唯一的可变状态：

- `stack: Vec<Vec<AnyValue>>` 的外层索引对应动态选择深度，内层保存尚未枚举的候选；当前候选总是内层第一个元素。
- `pos` 只描述当前一轮闭包已经经过多少个选择点。成功 `PickValue` 后递增，`NextStack` 后归零。
- `valid` 初始为真；第一次执行后由 `NextStack` 根据栈是否为空决定后续轮次。因此没有调用任何 `Pick` 的闭包只运行一次。
- `AnyValue.value` 拥有候选值，要求值为 `'static`；`Pick` 返回克隆值而不是暴露栈内引用，因而公开泛型还要求 `Clone + Debug`。
- `AnyValue.text` 在候选入栈时生成一次，失败日志无需再次进行类型分派；代价是每个候选额外保存一份字符串。

已建立的层不会验证后续轮次传入候选向量的长度、内容或类型是否与首次建立时一致。安全使用依赖测试控制流在相同前缀选择下保持同一层语义；改变选择点顺序或让候选集合受非确定外部状态影响，可能造成转型 panic或漏掉预期组合。

## 依赖与调用关系

本文件只依赖标准库：`std::any::Any` 用于类型擦除/向下转型，`std::fmt` 和 `Debug` 用于诊断文本，`std::error::Error` 用于错误接口，`std::panic::{catch_unwind, AssertUnwindSafe, resume_unwind}` 用于失败隔离与延后传播。`eprintln!` 把失败组合写到标准错误。

内部调用链为：

- `RunTest` → `newPickStack` → 每轮用户闭包 → `Pick`/`PickEnum` → `AnyValue::new` → `pickStack::PickValue`。
- 每轮结束：`RunTest` → `pickStack::ValuesText` → `Values`（仅失败时），随后 → `NextStack`。
- `PickEnum` → `Pick`；`Pick` → `PickValue` → `AnyValue::downcast_ref`。

RustCodeGraph 对目标文件的索引列出 18 个符号，并确认 `RunTest` 调用 `newPickStack`、`Valid`、`ValuesText` 和 `NextStack`，`Pick` 调用 `PickValue`，`PickEnum` 调用 `Pick`。上游图和源码搜索确认 Rust 直接调用者目前是 `fork_test.rs::TestForkSubTest`、`migration_aster_unit_test.rs::run_test_enumerates_the_same_branch_dependent_cartesian_product_as_go` 与 `run_test_reports_failure_after_visiting_remaining_combinations`。仓库根 Cargo workspace 以 `facade_testkit_testfork` 依赖并从 `pkg/lib.rs::testkit::testfork` 再导出；这提供公共可达路径，不代表生产运行时会调用它。

## 错误处理与边界

- 空候选：`PickValue([])` 返回 `PickError::EmptyValues`；公开 `Pick` 对该结果调用 `expect`，所以公开 API 表现为 panic。`PickEnum` 由签名保证至少有首项，不会构造空候选。
- 非法深度：`pos > stack.len()` 返回带两个数值的 `PickError::IllegalState`；公开 `Pick` 同样把它转为 panic。`migration_aster_unit_test.rs::pick_stack_rejects_empty_values_and_illegal_depth` 验证两种文案。
- 类型恢复失败：若同一深度保存的实际类型与当前 `Pick<E>` 所期待的 `E` 不一致，`downcast_ref` 失败并 panic。正常确定性控制流下，候选由同一选择点建立，类型应保持一致。
- 子测试失败：`RunTest` 通过 `catch_unwind` 捕获每轮 panic，打印当轮 `ValuesText`，继续推进组合，只保存首个 panic payload；全部组合访问后用 `resume_unwind` 传播它。后续 panic 不会替换首个失败。
- panic 边界：只有闭包执行位于 `catch_unwind` 内；失败后的 `ValuesText`、`NextStack` 以及最后的 `resume_unwind` 不在捕获范围。`AssertUnwindSafe` 是对可变闭包和栈的显式断言，调用者仍应避免依赖 panic 后可能破坏的不变量。
- 格式边界：字符串按 Go 诊断习惯加双引号；其他类型使用 Rust `Debug`，不保证与 Go `fmt.Sprintf("%v")` 对所有自定义类型逐字一致。

## 并发与资源生命周期

实现没有线程、异步任务、锁、通道、事务或外部句柄。`RunTest` 在当前线程同步、串行枚举组合；`FnMut` 允许闭包跨轮修改外部状态，但不会并行调用。`T` 对 `pickStack` 的独占可变借用只存在于一次闭包调用期间，借用结束后 `RunTest` 才能推进栈。

候选值由 `Box<dyn Any>` 拥有，所在层被 `NextStack::truncate` 删除或整个栈离开作用域时自动释放。`remove(0)` 会移动同层后续元素，单次推进成本与该层剩余候选数线性相关；它适用于测试规模的小组合，不适合大规模性能敏感枚举。panic payload 在第一次失败后由 `first_failure` 持有到枚举结束，再被恢复；其他失败 payload 在对应轮结束后丢弃。

## 与 Go 版本的对应关系

直接对照文件是 [`fork.go`](./fork.go)，测试对照是 [`fork_test.go`](./fork_test.go)。主要映射如下：

| Go | Rust | 对应关系与差异 |
| --- | --- | --- |
| `pickStack.stack [][]any` | `Vec<Vec<AnyValue>>` | Rust 用 `AnyValue` 同时保存 `Any` 值和诊断文本。 |
| `newPickStack` | `newPickStack` | 都以 `valid=true` 开始，确保至少运行一次。 |
| `NextStack` | `NextStack` | 都消费最深层首项、空层向前进位；Rust 用 `remove(0)` 与 `truncate`。 |
| `PickValue([]any)` | `PickValue(Vec<AnyValue>)` | 空值和非法深度错误文案一致；Rust 返回 `PickError`。 |
| `Values` / `ValuesText` | 同名方法 | 都输出每层当前值；字符串加引号，其他值的通用格式仅在常见类型上对齐。 |
| 嵌入 `*testing.T` 的 `T` | 仅含 `&mut pickStack` 的 `T<'a>` | Rust 上下文不能直接提供 Go `testing.T` 的断言、清理或子测试能力。 |
| `RunTest(t, f)` 调用 `t.Run` | `RunTest(f)` 直接调用闭包 | Go 每组是测试框架子测试；Rust 没有命名子测试，改用 panic 捕获实现失败后继续枚举。 |
| `require.NoError` / 类型断言 | `expect` / `downcast_ref` / `clone` | 错误最终都令公开调用失败，但 Rust 需要 `Clone + Debug + 'static`。 |
| `PickEnum(t, item, other...)` | `PickEnum(t, item, Vec<E>)` | Rust 没有变参泛型，调用方显式传其余候选向量。 |

Go `RunTest` 依赖 `testing.T.Run` 返回的成功标志；子测试失败通常不会以可捕获 panic 表示。Rust 版本则把 panic 当作失败协议，并特意在全部组合之后恢复首个 panic。`migration_aster_unit_test.rs::run_test_reports_failure_after_visiting_remaining_combinations` 是这一 Rust 适配行为的直接证据，不能把它描述为 Go 测试框架语义的完全等价实现。

## 扩展指南

- 新增选择 API 时优先委托 `Pick` 或 `PickValue`，避免另建与 `pos`/进位规则不一致的状态；同时在独立测试文件中加入分支依赖组合及顺序断言。
- 修改栈推进算法时，应同步验证 `fork_test.rs::TestForkSubTest` 的 12 组序列、`migration_aster_unit_test.rs` 的空候选/非法深度、诊断格式和“失败后继续”四类契约。Rust 单元测试继续放在独立 `*_test.rs` 文件，不应嵌入 `fork.rs`。
- 若要支持非 `Clone` 值，需要重新设计公开返回所有权及栈内候选生命周期；不能简单移除 trait bound，因为当前值仍归 `AnyValue` 所有。
- 若要让候选随同一前缀动态变化，应先定义集合变化的兼容规则并在 `PickValue` 校验；当前实现会忽略已存在层的新传入向量。
- 若要提供类似 Go `testing.T` 的上下文，应新增明确的 Rust 测试上下文抽象，而不是假定现有 `T` 已具备子测试、清理或断言能力。
- 优化大量候选时，可评估以当前索引替代 `remove(0)`，但必须保持枚举顺序、动态分支删层和失败诊断值不变；这属于性能/状态布局兼容风险。
- 扩展诊断格式时同步比较 `AnyValue::new`、`Debug::fmt`、`ValuesText` 与 Go `ValuesText`，并为字符串、自定义类型和转义字符添加对照测试，避免声称所有 `%v` 格式都天然等价。

## 验证依据

- RustCodeGraph 索引状态：仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/testkit/testfork` 找到 `fork.rs`、两个独立 Rust 测试及 Go 对照文件。
- RustCodeGraph 源码与符号：`node --file pkg/testkit/testfork/fork.rs --offset 1 --limit 260` 读取目标文件完整 253 行；`query RunTest`、`query pickStack`、`query Pick` 定位本文件的驱动、状态机和选择 API；`explore "pkg/testkit/testfork/fork.rs symbols callers callees"` 给出上述内部边及 Rust/Go 测试调用者。
- crate/导出证据：[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、仓库根 `Cargo.toml` 的 `facade_testkit_testfork`、[`pkg/lib.rs`](../../lib.rs) 的 `testkit::testfork` 再导出，以及 `pkg/sessiontxn{,/isolation}/Cargo.toml` 的开发依赖。
- Go 对照证据：[`fork.go`](./fork.go) 与 [`fork_test.go`](./fork_test.go)；另以 `pkg/sessiontxn/txn_context_test.go`、`pkg/sessiontxn/isolation/{main,readcommitted,repeatable_read,serializable}_test.go` 核实 Go 包的真实上游用途。
- Rust 测试证据：[`fork_test.rs`](./fork_test.rs) 验证动态分支的 12 组枚举；[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 还验证错误文案、字符串诊断格式和 panic 后继续枚举。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含“文件定位”至“验证依据”的 11 个固定二级标题；链接、符号名称和“当前已接线/尚未接线”陈述需结合以上源码人工复核。
