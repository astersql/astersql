# [`pkg/util/id_generator.rs`](id_generator.rs)

## 文件定位

本文件属于 `astersql-util` crate。crate 清单 `pkg/util/Cargo.toml` 将库入口设为 `pkg/util/lib.rs`，入口通过 `pub mod id_generator;` 公开本模块，因此外部依赖该 crate 的 Rust 代码可以用 `astersql_util::id_generator::IDGenerator` 访问生成器；入口没有在 crate 根重导出该类型。

它是一个只维护进程内整数状态的通用工具，不访问数据库、存储或网络。全仓 Rust 引用搜索目前只找到独立测试 `pkg/util/cpu_posix_1_aster_unit_test.rs`，未找到业务主链调用。Go 对应实现则由 planner 的 Runtime Filter 生成流程使用；这说明 Rust 文件已经移植并公开，但尚不能据此认定 Rust planner 已接入本实现。

## 核心职责

`IDGenerator` 提供最小的自增序号状态：每次 `GetNextID` 先返回当前值，再把内部值增加一。默认实例从 `0` 开始，因而首次两次调用依次返回 `0`、`1`。

该类型只保证单个实例上的调用序列；它不提供全局唯一性、持久化、随机性、跨进程协调或并发同步。公开字段允许调用者选择任意起点，也意味着调用者可以绕过方法改写序列。

## 主要符号

- `pub struct IDGenerator`：唯一生产类型，派生 `Clone`、`Debug` 和 `Default`。
- `pub nextID: isize`：下一次要返回的值。`Default` 令其为 `0`；字段公开，测试也直接把它设为 `isize::MAX`。
- `pub fn GetNextID(&mut self) -> isize`：唯一方法。可变借用使一次调用中的读取和更新针对同一个实例按顺序完成；命名保留 Go API 的 `GetNextID` 风格，crate 根的 `#![allow(non_snake_case)]` 允许该命名。

文件没有常量、trait、枚举、错误类型、条件编译项或内嵌测试。

## 执行流程

调用 `GetNextID` 时只有三步：

1. 从 `self.nextID` 复制当前 `isize` 到局部变量 `curID`。
2. 用 `self.nextID.wrapping_add(1)` 更新下一值；在 `isize::MAX` 后明确回绕到 `isize::MIN`。
3. 返回更新前保存的 `curID`，因此语义等价于后缀自增而不是前缀自增。

独立测试 `gogc_and_id_generator_preserve_go_state_transitions` 验证了默认序列 `0, 1`，并验证设置为 `isize::MAX` 后连续返回 `isize::MAX`、`isize::MIN`。

## 数据与状态

全部状态只有一个机器字宽的有符号整数 `nextID`。类型使用 `isize`，其宽度随 Rust 目标平台变化；对应 Go 字段使用 `int`，同样是实现相关字宽。没有“已分配 ID”集合，因此不会检测重复；完整走过整数空间或外部重设字段都可能再次返回旧值。

`Clone` 会复制当前计数值，之后两个实例独立推进并可能产生相同 ID。`Default` 只是零初始化，不注册任何全局状态。`Debug` 仅提供结构化调试输出，不改变状态。

## 依赖与调用关系

目标实现只使用 Rust 核心整数和派生 trait 能力，没有引用 `pkg/util/Cargo.toml` 中的任何第三方依赖。上游装配边为 `pkg/util/lib.rs -> pub mod id_generator`；已验证的 Rust 调用边为 `pkg/util/cpu_posix_1_aster_unit_test.rs::gogc_and_id_generator_preserve_go_state_transitions -> IDGenerator::default/GetNextID`。

RustCodeGraph 确认 `IDGenerator` 和 `GetNextID` 定义，但精确 callers/callees 查询没有返回业务调用边；全仓 Rust 搜索也未发现除上述测试外的引用。不要把 Go 调用边直接当成 Rust 接线：Go 中 `pkg/planner/core/optimizer.go::generateRuntimeFilter` 创建 `util.IDGenerator`，`pkg/planner/core/operator/physicalop/physical_plan_misc.go::NewRuntimeFilter` 调用 `GetNextID` 为每个 Runtime Filter 分配 ID；Rust planner 当前另有自己的 `RuntimeFilterIDGenerator`。

## 错误处理与边界

本 API 不返回 `Result`，也没有显式失败分支。溢出不是 panic：`wrapping_add(1)` 在所有构建模式都规定为二进制补码回绕。由此带来的边界风险是回绕后的负数和潜在 ID 重复，而不是运行时错误。

公开 `nextID` 不校验起点，因此负数、最大值或任意中间值都有效。调用方若要求非负、永不重复或固定宽度 ID，必须在本类型之外建立约束，或者扩展 API 并同步定义耗尽行为。

## 并发与资源生命周期

生成器不创建线程、任务、锁、通道、文件句柄或堆资源；生命周期就是拥有它的 Rust 值的生命周期，析构时无需清理。`GetNextID` 要求 `&mut self`，安全 Rust 中同一实例不能被两个调用者同时可变借用；若要跨线程共享推进，调用方需要用互斥锁等同步容器包裹它。

克隆不是并发共享方案，因为克隆会分叉计数状态。若未来需要跨线程低开销分配，应明确选择原子整数及其内存序，并独立验证回绕、范围和 Go 兼容性，而不应仅把当前字段放进无同步共享结构。

## 与 Go 版本的对应关系

直接对照文件 `pkg/util/id_generator.go` 定义同名结构和方法：Go 的私有 `nextID int` 对应 Rust 的 `pub nextID: isize`，两者都先保存当前值、再递增、最后返回旧值。Rust 用显式 `wrapping_add` 固化整数边界回绕；Go 使用 `g.nextID++`。两边的整数宽度都与目标架构相关。

可见性是当前的重要差异：Go 字段仅包内可见，Rust 字段对所有模块公开，因而 Rust 外部调用者能任意设定下一值。Go 生产调用存在于 planner Runtime Filter 链，而 Rust 全仓目前仅见测试调用；Rust planner 的同名用途由 `pkg/planner/core/operator/physicalop/physical_plan_misc.rs::RuntimeFilterIDGenerator` 自行实现，不能视为本类型的调用者。

Go 同目录未发现 `id_generator_test.go` 或直接针对该类型的测试；Rust 的现有边界覆盖位于独立文件 `pkg/util/cpu_posix_1_aster_unit_test.rs`。

## 扩展指南

若只需改变序列推进规则，主要修改点是 `IDGenerator::GetNextID`；若要限制起点或隐藏状态，应同时调整 `nextID` 的可见性并提供构造/重置 API。任何改变都应先核对 `pkg/util/id_generator.go`，明确是保持 Go 行为还是记录有意差异。尤其要评估默认首值、返回前后更新顺序、目标平台字宽、最大值行为、负数和克隆后重复等兼容风险。

测试必须继续与生产源文件分离。优先在 `pkg/util` 下建立或扩展独立 `*_test.rs`，并由 `lib.rs` 的 `#[cfg(test)]` 模块接入；至少覆盖默认值、连续调用、自定义起点、最大值回绕和克隆分叉。若把本生成器接入 Rust Runtime Filter，还需在 planner 的独立测试中验证一次生成多种 filter 类型时 ID 顺序与唯一性，并检查是否应替换而不是并存于现有 `RuntimeFilterIDGenerator`。

若新增并发或耗尽错误语义，API、调用方和测试必须一起迁移；不能用“编译通过”代替行为验证，也不应把测试逻辑内嵌进 `id_generator.rs`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标仓库已初始化。
- RustCodeGraph `node --file pkg/util/id_generator.rs --offset 1 --limit 260`：读取目标文件完整 40 行，确认 `IDGenerator`、`nextID` 和 `GetNextID` 的实现。
- RustCodeGraph `query IDGenerator --json`、`query GetNextID --json`：确认 Rust/Go 对应符号以及 Rust planner 中独立的 Runtime Filter 生成器；精确 `callers/callees` 未返回目标方法的可用业务边。
- `pkg/util/Cargo.toml`、`pkg/util/lib.rs`：确认 `astersql-util` crate 边界、库入口、公开模块以及独立测试装配。
- `pkg/util/cpu_posix_1_aster_unit_test.rs`：确认默认递增和 `isize` 最大值回绕的 Rust 测试证据。
- `pkg/util/id_generator.go`：确认 Go 的字段、后缀自增顺序和返回类型语义。
- `pkg/planner/core/optimizer.go`、`pkg/planner/core/operator/physicalop/physical_plan_misc.go`：确认 Go Runtime Filter 的实例创建与生产调用边。
- `pkg/planner/core/operator/physicalop/physical_plan_misc.rs`：确认 Rust planner 使用独立生成器，而非本文件类型。
- 全仓 `rg` 引用搜索：Rust 侧除模块声明和独立测试外未发现 `pkg/util::id_generator::IDGenerator` 的业务使用；`pkg/util` 下未发现直接 Go 测试。

人工复核结论：该文件存在是为了提供与 Go `pkg/util.IDGenerator` 对齐的轻量后缀自增工具；运行时仅执行一次读取、显式回绕加一和返回；安全扩展必须保护调用顺序与边界语义，并在独立测试中覆盖状态转换和实际接线。
