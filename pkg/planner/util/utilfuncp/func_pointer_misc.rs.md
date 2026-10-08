# `pkg/planner/util/utilfuncp/func_pointer_misc.rs`

## 文件定位

本文件属于 `astersql-planner-util-utilfuncp` crate，是 Go 包
`pkg/planner/util/utilfuncp` 的 Rust 对应实现。crate 根
[`lib.rs`](./lib.rs) 将本文件私有模块中的所有公开项重新导出；工作区根
`Cargo.toml` 又以 `facade_planner_util_utilfuncp` 引入该 crate，并由
`pkg/lib.rs` 的 facade 模块继续再导出。

文件承担两个相互独立、但都用于解除 planner 子包耦合的职责：

1. `define_callback_slots!` 声明 98 个与 Go 包级函数变量同名的回调槽，并用
   `CallbackSlot`/`CallbackError` 提供线程安全、运行时类型检查的安装与查询接口。
2. `Clone*ForPlanCache` 系列函数按 `SafeToShareAcrossSession` 判断表达式对象能否跨会话共享，
   只在必要时为计划缓存建立独立副本。

当前接线状态必须与能力本身分开理解：这些 API 已由 crate 和顶层 facade 导出，
但仓库内 Rust 生产代码尚未检索到对本文件回调槽的具体 `install`/`get` 调用；
`pkg/planner/core/operator/physicalop/plan_clone_generated.rs` 中的克隆调用也仍是 Go 形态的注释。
`pkg/planner/core/core_init.rs` 维护的是另一套回调**名称集合**，不是本文件
`HashMap<CallbackName, CallbackEntry>` 的安装入口。

## 核心职责

- `define_callback_slots!` 以单一名称清单生成 `CallbackName`、`ALL_CALLBACKS` 和 98 个
  `pub static CallbackSlot`，覆盖逻辑/物理计划选优、统计推导、代价计算、
  `Attach2Task`、`ResolveIndices`、访问路径与 `DoOptimize` 等 Go 注入点。
- `CallbackSlot::install`、`get`、`remove`、`is_installed` 围绕全局注册表提供一致的生命周期操作；
  同一槽位一旦已有值，只允许用同一 Rust 具体类型替换。
- `ClearCallbacks` 清空全局注册表，用于有序拆卸或测试隔离；它不会影响
  `pkg/planner/core/core_init.rs` 的独立名称集合。
- `PlanCacheSlice::Shared` 表示无需复制、借用原切片；`Cloned` 表示至少发现一个不安全元素后，
  返回拥有所有权的新 `Vec`。
- 五个公开克隆入口保留 Go 的外层 `nil`、安全快路径、缓冲区复用和二维逐行处理语义；
  `CloneConstantsForPlanCache` 还保留 issue #66265 所要求的中间空槽位。

本文件不执行优化算法、不计算代价，也不调用已注册回调；它只提供注入设施和缓存克隆策略。

## 主要符号

- `CallbackName`：由宏生成的稳定枚举键。`as_str()` 返回原 Go 变量名；
  `ALL_CALLBACKS` 按源文件声明顺序暴露全部 98 个键。
- 98 个同名静态槽位：例如 `FindBestTask4BaseLogicalPlan`、`GetPlanCost`、
  `AttachPlan2Task`、`GetPossibleAccessPaths`、`DoOptimize`。静态值只携带 `CallbackName`，
  不携带函数签名，签名由调用方传给 `install::<T>`/`get::<T>` 的 `T` 决定。
- `CallbackEntry`：注册表内部记录，保存 `TypeId`、诊断用 `type_name`，以及
  `Box<dyn Any + Send + Sync>` 类型擦除值；该类型不公开。
- `callbacks()`：通过 `OnceLock<RwLock<HashMap<...>>>` 惰性构造进程级注册表。
- `CallbackSlot`：可复制的命名句柄；`name()` 可取回键，`install<T>()` 返回被替换的同类型旧值，
  `get<T>()` 返回克隆值，`remove()`/`is_installed()` 管理或观察槽位状态。
- `CallbackError::{NotInstalled, TypeMismatch}`：查询空槽、或安装/查询类型不一致时的结构化错误；
  同时实现 `Display` 与 `std::error::Error`。
- `ClearCallbacks()`：持写锁清空全部条目。
- `PlanCacheSlice<'a, T>`：`Shared(&'a [T])` 与 `Cloned(Vec<T>)` 的显式联合；
  `AsRef<[T]>` 和 `Deref<Target = [T]>` 让调用者统一按切片读取。
- `reuse_or_new_vec<T>()`：复用传入 `Vec`，先 `clear`，容量不足才 `reserve`。
- `CloneExpressionsForPlanCache`：输入 `Option<&[ExprBox]>`；安全快路径借用原切片，
  慢路径对所有元素调用 `CloneExpr()`。
- `CloneColumnsForPlanCache`：输入允许元素为 `None`；慢路径共享安全列的 `Arc`，深克隆不安全列。
- `CloneConstantsForPlanCache`：扫描和复制阶段均显式跳过/保留 `None`，其余策略与列类似。
- `CloneScalarFunctionsForPlanCache`：安全项 `Arc::clone`，不安全项 `clone_scalar` 后置于新 `Arc`。
- `CloneExpression2DForPlanCache`：保留外层和每行的 `None`，逐行调用表达式克隆入口，
  不接受供二维结果复用的缓冲区。

文件没有 trait、条件编译项或模块级可变常量；宏生成项和上述公开函数构成其公开 API。

## 执行流程

回调注册流程如下：

1. 调用方选择一个静态槽位并以具体 `T: Clone + Send + Sync + 'static` 调用 `install`。
2. `callbacks()` 首次访问时建立空 `HashMap`，随后取得写锁。
3. 若槽位已有不同 `TypeId`，立即返回 `TypeMismatch`，原值保持不变；否则插入新
   `CallbackEntry`，并尝试将旧的类型擦除值向下转换为 `T` 后返回。
4. 消费方以相同 `T` 调用 `get`：持读锁查找键，空槽返回 `NotInstalled`，类型不同返回
   `TypeMismatch`，匹配时克隆 `T` 返回。文件自身没有“调用回调”的步骤。

计划缓存克隆流程如下：

1. 外层输入为 `None` 时立即返回 `None`，对应 Go 的 `nil` 切片。
2. 第一遍扫描 `SafeToShareAcrossSession()`；全部安全时返回 `PlanCacheSlice::Shared`，
   不分配新向量，也不接管传入的备用缓冲区。
3. 一旦发现不安全元素，通过 `reuse_or_new_vec` 清空并复用缓冲区或按输入长度新建向量。
4. 表达式慢路径统一 `CloneExpr`；列、常量和标量函数只深克隆不安全对象，安全对象复用 `Arc`。
5. 二维入口逐行应用步骤 1--4，并收集每行的 `Shared`、`Cloned` 或 `None`。

`CloneColumnsForPlanCache` 有一个刻意保留的尖锐边界：第一遍安全性扫描对元素直接
`unwrap()`，因此扫描到 `None` 会 panic；只有已经被更早的不安全列打断扫描后，复制阶段才会
保留后续 `None`。源码注释说明这是对 Go 扫描阶段解引用 `nil` 行为的对齐。

## 数据与状态

全局状态只有 `callbacks()` 内的惰性注册表。键是 `CallbackName`，值的真实类型由首次成功安装
决定，但该约束不是静态类型系统的一部分，而是在每次安装/查询时用 `TypeId` 验证。
注册表进程级共享，没有作用域或所有者标识；`remove` 与 `ClearCallbacks` 会立刻影响所有线程。

`CallbackSlot` 和 `CallbackName` 都是轻量、可复制值。回调值存于 `Box<dyn Any + Send + Sync>`；
`get` 返回克隆而非注册表中值的借用，因此读锁不会跨越函数返回。若 `T` 是 `Arc<dyn Fn...>`，
克隆通常只增加引用计数；若调用方安装的是重量级可克隆值，查询成本由其 `Clone` 实现决定。

`PlanCacheSlice::Shared` 的生命周期绑定输入切片，不能比源切片活得更久；`Cloned` 拥有独立向量。
列、常量和标量函数元素使用 `Arc`：安全元素在慢路径中仍共享对象，不安全元素生成新对象。
表达式慢路径则统一调用 `CloneExpr`，这是 Rust `ExprBox` 所有权模型相对 Go interface 切片的实现差异。

## 依赖与调用关系

- 标准库依赖：`Any`/`TypeId`/`type_name` 支持类型擦除与诊断；`HashMap` 保存槽位；
  `OnceLock`、`RwLock` 和 `Arc` 管理并发共享；`Display`/`Error` 暴露错误接口。
- 唯一正常 Cargo 依赖是 `astersql-expression`，提供 `ExprBox`、`Column`、`Constant`、
  `ScalarFunction` 及它们的共享安全判断/克隆能力。
- `pkg/planner/util/utilfuncp/Cargo.toml` 中 parser、planner base/property/util/costusage、table、
  execdetails、hint 等依赖位于 `target.'cfg(any())'`，该条件永不启用；它们记录 Go 函数签名所需边界，
  不是当前文件的已编译下游依赖。
- `lib.rs` 直接 `pub use func_pointer_misc::*`；工作区 facade 再导出使其他 crate 可见。
- RustCodeGraph 将本文件识别为 21 个源码符号并报告 22 个文件级使用者；精确检索表明大部分
  `Clone*` 上游仍是生成器输出中的 Go 代码字符串或注释，而不是已编译 Rust 调用。
- `CloneExpression2DForPlanCache -> CloneExpressionsForPlanCache` 是本文件内明确的函数调用边；
  四个一维入口均调用 `reuse_or_new_vec`，并下调用 expression 对象的安全判断与克隆方法。
- `func_pointer_misc_test.rs -> CloneConstantsForPlanCache` 是已编译测试调用边。
- `pkg/planner/implementation/Cargo.toml` 声明了对本 crate 的依赖，但在该目录 Rust 源中未检索到使用；
  因而不能仅凭 manifest 宣称生产优化器已完成槽位安装。

## 错误处理与边界

- 未安装槽位返回 `CallbackError::NotInstalled`；请求或替换类型不一致返回
  `CallbackError::TypeMismatch { name, expected, actual }`。错误文本含 Go 名称和 Rust 类型名。
- `RwLock` 中毒不会继续传播 panic：所有锁操作都用 `PoisonError::into_inner` 取回内部状态。
  这保证注册表仍可操作，但不保证导致中毒的上一操作在业务语义上完整。
- 安装时先检查旧值类型再插入，因此类型冲突不会破坏原条目；同类型替换返回旧值。
- 克隆入口用外层 `Option` 区分 Go `nil` 与空切片；空但非 `None` 的输入走安全快路径并返回
  `Shared(&[])`，不会折叠成 `None`。
- `CloneConstantsForPlanCache` 明确支持任意位置的 `None`；独立 Rust/Go 回归测试验证中间空项不 panic
  且索引保持不变。
- `CloneColumnsForPlanCache` 的扫描阶段不支持 `None`，可能 panic；这是当前源码明确记录的 Go 对齐行为，
  扩展时不能误认为所有 `Option<Arc<_>>` 输入都安全。
- `CloneExpressionsForPlanCache` 慢路径没有复用安全元素，而是统一克隆；这是所有权实现选择，
  不能据函数注释推断为逐元素最小克隆。
- 回调名称完整性只由宏内单一清单保证；这里没有对照 Go 变量签名或数量的本地测试。

## 并发与资源生命周期

注册表由 `OnceLock` 保证只初始化一次，由 `RwLock` 允许并发查询、串行安装/删除/清空。
锁仅覆盖映射访问和 `T::clone`；文件不会在持锁期间调用回调，避免把未知用户逻辑带入临界区。
不过重量级或可重入的 `Clone` 实现仍会延长读锁持有时间，新增回调类型时应优先使用函数指针或 `Arc`。

`remove`、`ClearCallbacks` 与并发 `get` 之间只有锁所提供的线性顺序：先取得锁的一方决定观察结果，
没有等待在途回调完成、版本号或自动恢复机制。已经由 `get` 克隆出的值不受随后删除影响。
静态槽位和注册表存活到进程结束；本文件没有后台任务、通道、异步 future、事务或显式资源关闭。

Plan Cache 克隆函数不访问全局状态，是否线程安全取决于输入 expression 类型已承诺的
`Send`/`Sync` 与 `SafeToShareAcrossSession` 契约。`Shared` 只是借用，`Cloned` 在返回时拥有向量；
传入的备用 `Vec` 会被清空，其旧元素立即 drop，容量则尽量复用。

## 与 Go 版本的对应关系

Go 文件 `func_pointer_misc.go` 用 98 个包级 `var ... func(...)` 打断 import cycle；Rust 用同名
`CallbackSlot` 保存类型擦除值。名称覆盖一致，但 Rust 不在静态声明中编码每个 Go 函数签名，
而是在安装与查询双方的泛型 `T` 上动态达成一致，并新增了显式的缺失/类型错误。

五个克隆入口的主要对应关系为：

- Go `nil` 切片对应 Rust 外层 `Option::None`。
- Go 全部安全时原样返回切片；Rust 用 `PlanCacheSlice::Shared` 明示零拷贝借用。
- Go 复用 `cloned[:0]`；Rust `reuse_or_new_vec` 执行 `clear` 并保留容量。
- Go 列/常量/标量函数慢路径共享安全指针、克隆不安全对象；Rust 用 `Arc::clone` 与新 `Arc` 表达同一意图。
- Go 表达式慢路径只克隆不安全 interface 值；Rust 因 `ExprBox` 的所有权要求，在慢路径统一
  `CloneExpr`，但全安全路径仍零拷贝。
- Go 与 Rust 的常量扫描都跳过 `nil`/`None`；`func_pointer_misc_test.go` 和
  `func_pointer_misc_test.rs` 均以 issue #66265 场景验证中间空项。
- Go 列扫描会在 `nil` 上解引用；Rust 用 `unwrap` 明确保留这一行为。

Go 文件只声明函数变量，不负责它们的赋值；Go 的具体赋值通常由 planner/core 初始化完成。
Rust 当前另有 `pkg/planner/core/core_init.rs` 名称登记，但没有证据表明它向本文件 98 个槽安装了具体函数，
所以迁移状态应描述为“槽位基础设施存在、具体生产接线未验证/尚未检索到”，不能写成完整等价运行链。

## 扩展指南

- 新增或删除 Go 注入点时，在 `define_callback_slots!` 的单一清单中同步名称，并与
  `func_pointer_misc.go`、实际安装方及 `pkg/planner/core/core_init.rs` 的相关名称逐项核对；
  若要提供可调用链，还必须在生产初始化处以唯一、共享的回调类型别名执行 `install`。
- 为避免安装/查询双方独立写出不一致的复杂函数类型，建议新增公开类型别名，并增加独立测试覆盖
  成功安装与获取、同类型替换、错类型安装/查询、删除、清空及并发读取。Rust 测试必须继续放在
  同目录独立 `*_test.rs` 文件中，不要内嵌到本源文件。
- 扩展克隆族时遵循“两遍式”约束：先判断能否整体共享，再决定是否消费备用缓冲；明确说明
  `None`、空切片、空元素、元素顺序和安全/不安全混合输入的行为。
- 修改 `CloneConstantsForPlanCache` 时同步维护
  `func_pointer_misc_test.rs::TestCloneConstantsForPlanCacheWithNilEntry` 和 Go 同名测试；
  修改列的 `None` 行为前必须评估是否仍需严格保留 Go panic 语义，并增加独立回归测试。
- 性能风险集中在计划缓存热路径的额外分配/深克隆，以及注册表 `get` 在读锁内执行重量级 `Clone`；
  兼容风险集中在回调名称、类型和安装顺序不一致，以及把 `None` 与空切片混为一谈。
- 若将生成的物理计划克隆代码从注释迁移为可编译 Rust，优先接入这些公开入口，而不是复制判断逻辑；
  同时用真实编译调用重新验证当前仅由字符串/注释形成的“上游”关系。

## 验证依据

- 源码：[`func_pointer_misc.rs`](./func_pointer_misc.rs)，完整检查宏清单、注册表、错误类型、
  `PlanCacheSlice` 及五个克隆入口；宏清单机械计数为 98。
- crate 边界：[`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)；确认正常依赖仅
  `astersql-expression`，其余签名依赖被置于永不成立的 `cfg(any())`，并确认全部公开项再导出。
- Go 对照：[`func_pointer_misc.go`](./func_pointer_misc.go)，核对 98 个函数变量及五个克隆函数的
  `nil`、安全快路径、缓冲复用和逐元素策略。
- 独立测试：[`func_pointer_misc_test.rs`](./func_pointer_misc_test.rs) 与
  [`func_pointer_misc_test.go`](./func_pointer_misc_test.go)，核对 issue #66265 的中间空常量回归场景。
- 直接接线证据：`pkg/planner/core/core_init.rs`、`pkg/planner/core/core_init_test.rs`、
  `pkg/planner/core/operator/physicalop/plan_clone_generated.rs`、
  `pkg/planner/implementation/Cargo.toml`、工作区 `Cargo.toml` 与 `pkg/lib.rs`；据此区分名称登记、
  manifest 依赖、注释中的 Go 形态和真实 Rust 调用。
- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter` 找到本目录 Rust/Go
  源与测试；`node --file` 完整读取目标、测试、Go 对照和 crate 根；`query` 消除同名 Go/Rust
  `CloneExpressionsForPlanCache` 歧义；`explore` 给出 `callbacks`、`get`、`remove`、
  `reuse_or_new_vec` 和克隆入口的文件级调用关系。精确 callers/callees 查询未返回额外生产边，
  因而又用限定 Rust/Cargo 的 `rg` 核对当前接线边界。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令验证恰有 11 个固定二级标题，
  并人工复核没有把未接线能力描述为已运行的生产路径。
