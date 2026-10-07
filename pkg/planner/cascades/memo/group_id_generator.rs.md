# `pkg/planner/cascades/memo/group_id_generator.rs`

## 文件定位

本文件属于 `astersql-planner-cascades-memo` crate，是 Cascades Memo 为等价组分配身份编号的最小状态组件。crate 入口 `pkg/planner/cascades/memo/lib.rs` 以私有模块 `group_id_generator` 装入它，再通过 `pub use group_id_generator::*` 导出 `GroupID` 和 `GroupIDGenerator`。`pkg/planner/cascades/memo/Cargo.toml` 将该 crate 的库入口指定为 `lib.rs`，并用 `package.metadata.porting.go-package = "pkg/planner/cascades/memo"` 明确记录对应 Go 包。

它位于 Memo 建图主链的编号环节：`Memo::NewMemo` 创建一个默认生成器，`Memo::NewGroup` 每创建一个新 `Group` 就调用 `GroupIDGenerator::NextGroupID`，把结果写入 `Group::groupID`，同时登记到 `Memo::groupID2Group`。因此，此文件不负责判断表达式是否等价或创建 Group 的其他字段，只负责在一个 Memo 生命周期内提供编号。

## 核心职责

- 以 `GroupID = u64` 统一表示 Memo 等价组身份。
- 通过 `GroupIDGenerator` 保存“最近一次已经分配的 ID”，默认值为 `0`。
- 通过 `NextGroupID` 先递增、后返回，使默认生成器第一次分配得到 `1`，后续按模 $2^{64}$ 单调推进。
- 通过 crate 内可见的 `reset` 支持 `Memo::Destroy` 清空图后把下一次编号恢复为 `1`。
- 在测试构建中通过 `set` 直接设置当前计数，便于验证非零起点和 `u64::MAX` 后回绕。

编号的唯一性范围是单个生成器在未发生 `u64` 回绕、且未被 `reset` 的生命周期内；该类型不宣称跨 Memo、跨进程或跨线程提供全局唯一 ID。

## 主要符号

- `pub type GroupID = u64`：公开类型别名。它没有引入新的运行时表示或类型隔离，消费者仍可直接使用 `u64` 值；`Group::Hash64`、`Group::Equals`、`Memo::groupID2Group` 等以它表达 Group 身份。
- `pub struct GroupIDGenerator { id: GroupID }`：公开类型、私有状态。派生 `Debug` 和 `Default`；默认构造等价于 `id = 0`，外部 crate 不能直接改写计数器。
- `pub fn NextGroupID(&mut self) -> GroupID`：公开分配入口。要求可变借用，执行 `wrapping_add(1)` 后返回新值；命名保留 Go 风格，crate 根的 `#![allow(non_snake_case)]` 允许这一 API。
- `pub(crate) fn reset(&mut self)`：仅本 crate 可调用，把 `id` 置为 `0`。当前直接调用者是 `Memo::Destroy`。
- `#[cfg(test)] pub(crate) fn set(&mut self, id: GroupID)`：仅 crate 测试构建存在，生产构建没有该符号。当前由 `group_id_generator_test.rs` 用于设置 `100` 和 `u64::MAX`。

本文件没有模块级常量、trait、枚举、自由函数或异步/条件 feature 实现；唯一条件编译项是测试辅助方法 `set`。

## 执行流程

正常建图流程如下：

1. `Memo::NewMemo`（也由 `Memo::default` 转入）以 `GroupIDGenerator::default()` 创建计数值为 `0` 的生成器。
2. `Memo::CopyIn` / `Memo::InsertGroupExpression` 在需要新的等价类时进入 `Memo::NewGroup`。
3. `Memo::NewGroup` 先构造尚未正式编号的 `Group`，再调用 `self.groupIDGen.NextGroupID()`。
4. `NextGroupID` 用 `wrapping_add(1)` 更新内部 `id` 并返回；首次结果为 `1`。
5. `Memo::NewGroup` 把该值写入 `Group::groupID`，随后将 Group 放入 `groups`，并以同一个 ID 为键写入 `groupID2Group`。
6. 后续 `Group::GetGroupID` 暴露编号；`Group::Hash64` 把编号写入 Hasher，`Group::Equals` 用编号判断 Group 身份相等。Memo 的合并、去重和遍历逻辑也以这些已分配编号定位 Group。

销毁流程不同：`Memo::Destroy` 先清理各 Group，再调用 `groupIDGen.reset()`，然后清空根、Group 列表、ID 索引和全局表达式表。复用同一个 `Memo` 值继续建图时，编号序列会重新从 `1` 开始；旧 ID 因相应容器已清空而不再属于当前 Memo。

## 数据与状态

唯一持久状态是私有字段 `id: GroupID`，语义为“当前已分配的最大/最近 ID”，而不是“下一次待分配 ID”。因此 `id = n` 时，通常下一次返回 `n + 1`；测试辅助 `set(100)` 后依次返回 `101`、`102`、`103`。

`wrapping_add` 明确定义了溢出行为：`id == u64::MAX` 时下一次返回并保存 `0`，再下一次返回 `1`。这与 Go `uint64` 无符号加法的回绕一致，但意味着经过完整编号空间后会重新产生已经出现过的数字；生成器本身没有碰撞检测。正常情况下碰撞风险只在不可现实地耗尽 $2^{64}$ 个编号或调用方重置后仍保留旧 Group 时出现，后者由 `Memo::Destroy` 同步清空全部相关状态来避免。

`GroupID` 是类型别名而非 newtype，所以它不携带所属 Memo、代次或有效性信息。生命周期与一致性由持有者 `Memo` 维护。

## 依赖与调用关系

本文件只依赖 Rust 核心语言和派生宏，不导入第三方 crate；`wrapping_add` 是 `u64` 的内建方法。因此 `Cargo.toml` 中列出的 `cascades-base`、`core-base`、`logicalop`、`property`、`planctx` 均不是此文件的直接依赖，而是同一 memo crate 其他模块的依赖。

直接上游关系：

- `pkg/planner/cascades/memo/lib.rs` 声明模块并公开再导出其符号。
- `Memo::NewMemo` / `Memo::default` 构造 `GroupIDGenerator`。
- `Memo::NewGroup` 调用 `NextGroupID`，这是生产路径中的直接分配调用点。
- `Memo::Destroy` 调用 `reset`。
- `TestGroupIDGenerator_NextGroupID` 调用 `NextGroupID` 和测试专用 `set`。

直接下游及状态消费者：

- `NextGroupID` 仅调用 `u64::wrapping_add`。
- `Memo::NewGroup` 将返回值交给 `Group::groupID` 与 `Memo::groupID2Group`。
- `Group::GetGroupID`、`Group::Hash64` 和 `Group::Equals` 读取该编号；`GroupExpression` 的哈希/等价关系进一步依赖输入 Group 的身份。

RustCodeGraph 对目标文件识别到 5 个符号，但其 `callers` / `callees` 查询没有返回 `NextGroupID` 的生产边；通过图未覆盖处的精确 Rust 引用检索确认了上述 `memo.rs` 调用点。因此调用关系结论以源码调用点为准，不把空图结果误解为“未接线”。

## 错误处理与边界

所有 API 都是不可失败的同步操作，不返回 `Result` 或 `Option`，也不主动 panic。`NextGroupID` 用显式回绕避免 debug 构建中的整数溢出 panic，并保持 Go 版本在 `MaxUint64 + 1` 时得到 `0` 的行为。

主要边界如下：

- 默认状态第一次返回 `1`，`0` 在正常未回绕序列中充当未正式分配/初始状态；`Group::NewGroup` 也先以 `groupID = 0` 构造，再由 `Memo::NewGroup` 覆盖。
- `u64::MAX` 的下一项是 `0`，不会报错，也不会跳过保留值。
- `reset` 不检查外部是否还持有旧 Group；安全使用依赖 `Memo::Destroy` 同时清理所有 Memo 内部集合。若未来在其他位置调用它，必须维持这一不变量。
- `GroupID` 只是别名，编译器不会阻止把任意 `u64` 当作 Group ID；合法性由调用方负责。
- 生成器不检测重复 ID，不处理持久化、跨实例协调或随机化。

## 并发与资源生命周期

`GroupIDGenerator` 使用普通 `u64` 与 `&mut self`，没有原子变量、锁、通道、后台任务或堆资源。Rust 的可变借用规则阻止同一生成器在安全代码中同时执行两个 `NextGroupID`，但类型没有实现跨线程协调协议；其设计对应 Go 注释所说的“Memo 优化在单线程中运行，因此生成器非线程安全”。

生成器作为 `Memo` 的内嵌字段创建并随 Memo 释放，没有独立关闭步骤。`Memo` 自身包含 `Rc<RefCell<...>>` 图结构，也指向单线程所有权模型。若未来把 Memo 并行化，不能只共享当前生成器；需要同时审查编号原子性、分配顺序、Group 容器同步、回绕碰撞和确定性测试。

`reset` 的生命周期边界是 `Memo::Destroy`：重置计数与清除旧图状态属于同一同步过程。测试专用 `set` 只在 `cfg(test)` 下编译，不扩大生产 API 或生产状态修改面。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/cascades/memo/group_id_generator.go`，相关测试是 `group_id_generator_test.go`。

- Go `type GroupID uint64` 是独立定义类型；Rust `type GroupID = u64` 是别名。这是类型系统强度上的差异，但当前数值表示与运算语义相同。
- 两端 `GroupIDGenerator` 都只保存一个无符号 64 位 `id`，零值/默认值均从 `0` 开始。
- Go `NextGroupID` 执行 `gi.id++` 后转换成 `GroupID` 返回；Rust `NextGroupID` 用 `wrapping_add(1)` 后返回。默认序列、手动设为 `100` 后的序列和最大值回绕序列一致。
- Go 测试因位于同包可直接改写私有字段 `g.id`；Rust 字段保持私有，以 `#[cfg(test)] pub(crate) set` 提供等价测试能力，避免生产 API 暴露任意改号入口。
- Rust 额外提供 `reset`，供 Rust `Memo::Destroy` 复用同一 Memo 实例时恢复编号；Go 的同名生成器文件没有该方法。该差异是 Rust Memo 生命周期接线，不改变 `NextGroupID` 的移植语义。
- Go 生产调用点是 `memo.go` 的 `Memo.newGroup`（源码中表现为给 `group.groupID` 调用生成器赋值）；Rust 对应点是 `memo.rs` 的 `Memo::NewGroup`。

Rust 独立测试还显式断言回绕到 `0` 后再次调用得到 `1`，覆盖了与 Go 测试相同的核心边界。

## 扩展指南

若要改变编号策略，最可能修改 `GroupID`、`GroupIDGenerator` 字段和 `NextGroupID`；必须同步检查 `Memo::NewGroup`、`Memo::Destroy`、`Group::{GetGroupID,Hash64,Equals}`、`Memo::groupID2Group` 以及依赖 Group ID 的表达式哈希逻辑。任何从别名改为 newtype 的工作都会影响算术、HashMap 键、序列化/格式化和跨 crate API，不能只改本文件。

安全扩展时应保持以下不变量：新 Group 在进入 `groups` 和 `groupID2Group` 前取得稳定 ID；同一存活 Memo 内的 Group ID 不重复；重置编号时旧 Group 与 ID 索引已同步失效；哈希和相等判断使用同一身份定义。若希望避免回绕，应先决定是返回错误、panic 还是扩大/复合标识，并沿 `Memo::NewGroup` 的调用链传播，而不能静默改变现有 Go 兼容行为。

测试应继续放在独立文件 `pkg/planner/cascades/memo/group_id_generator_test.rs`，不要内嵌到生产源文件。至少同步覆盖默认首值、连续递增、非零起点、最大值边界与重置后的首值；若改动影响 Memo 接线，还应扩展独立的 `memo_test.rs`，验证 Group 映射和 Destroy 后复用。需要保持 Go 对齐时，同时核对 `group_id_generator.go` 与 `group_id_generator_test.go`，明确记录有意差异。

性能上该入口位于每个新 Group 的创建路径，当前为常数时间、无分配；加入锁、随机数、持久化或全局协调都会改变热路径成本及确定性。并行化还会使编号顺序变得不可预测，应先确认测试和调试输出是否依赖创建顺序。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/cascades/memo` 确认目标 Rust/Go 源与独立测试均已索引。
- RustCodeGraph `explore "pkg/planner/cascades/memo/group_id_generator.rs GroupIdGenerator next_group_id"`：读取目标文件全貌、对应 Go 文件和 Rust 测试，并确认主要定义及测试覆盖。
- RustCodeGraph `query GroupIDGenerator`、`query NextGroupID`：确认 Rust/Go 定义及独立测试符号；对目标 `NextGroupID` 执行 `callers` / `callees` 未得到边，因此又用精确引用检索补证。
- `pkg/planner/cascades/memo/group_id_generator.rs`：`GroupID`、`GroupIDGenerator`、`NextGroupID`、`reset`、条件编译的 `set` 及回绕实现。
- `pkg/planner/cascades/memo/lib.rs` 与 `Cargo.toml`：模块装配、公开再导出、crate 名称/入口、依赖边界和 Go 包映射。
- `pkg/planner/cascades/memo/memo.rs`：生成器构造、`Memo::NewGroup` 分配调用、`Memo::Destroy` 重置调用、Group 列表和 ID 映射登记。
- `pkg/planner/cascades/memo/group.rs`：ID 字段及 `GetGroupID`、`Hash64`、`Equals` 消费语义。
- `pkg/planner/cascades/memo/group_id_generator_test.rs`：默认序列、`set(100)` 和 `u64::MAX` 回绕的 Rust 证据。
- `pkg/planner/cascades/memo/group_id_generator.go`、`group_id_generator_test.go`、`memo.go`：Go 定义、溢出测试与生产调用点的移植对照。
- 全仓精确引用检索：确认 `reset` 仅由 `Memo::Destroy` 使用，`set` 仅由独立 Rust 测试使用，`NextGroupID` 的 Rust 生产调用点为 `Memo::NewGroup`。

本任务是纯文档分析，按计划未运行 Cargo。结构验证要求本文恰有“文件定位”至“验证依据”共 11 个固定二级标题；事实复核重点是“为何存在、如何运行、如何安全扩展”均能追溯至上述真实符号和文件。
