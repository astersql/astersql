# `pkg/config/kerneltype/nextgen.rs`

## 文件定位

`nextgen.rs` 是 `astersql-config-kerneltype` crate 的 NextGen 编译分支实现。crate 根文件 [`lib.rs`](lib.rs) 仅在启用 Cargo feature `nextgen` 时声明 `mod nextgen`，并以 `pub use nextgen::*` 将本文件的接口重导出；未启用该 feature 时，同一公共接口由 [`classic.rs`](classic.rs) 提供。因此调用方看到的是稳定的 crate API，而不是两个实现模块。

[`Cargo.toml`](Cargo.toml) 将该 crate 定义为无默认 feature、仅有空 feature `nextgen` 的库；该 feature 本身不引入运行时依赖，只决定条件编译分支。文件顶部的 `#![allow(non_snake_case)]` 则允许公开函数保留 Go 版本的命名，以减少移植接口差异。

## 核心职责

本文件只负责给 NextGen 构建提供两个互补的内核类型判定函数：`IsNextGen()` 恒为 `true`，`IsClassic()` 返回其取反、因而恒为 `false`。它不读取配置、环境变量或集群状态；内核类型由构建时 feature 决定，而不是运行时切换。

这一小型 API 是更大范围功能分支的基础。例如 [`type.rs`](type.rs) 的 `Name()` 用 `IsNextGen()` 选择 `"Next Generation"` 或 `"Classic"`；仓库中的配置默认值、会话启动、存储、分布式导入等代码也通过 crate 重导出的 `IsNextGen`/`IsClassic` 选择形态相关行为。具体业务决策属于这些调用方，本文件只提供一致的构建身份事实。

## 主要符号

- `pub fn IsNextGen() -> bool`：受 `#[cfg(feature = "nextgen")]` 保护的公开函数。在本文件能够参与编译的条件下恒返回 `true`。其名称和返回语义直接对应 [`nextgen.go`](nextgen.go) 的 `IsNextGen`。
- `pub fn IsClassic() -> bool`：同样只在 `nextgen` feature 下编译，通过 `!IsNextGen()` 维持两个标志互补。使用取反而不是独立常量表达了 `IsClassic == !IsNextGen` 的不变量。
- `#![allow(non_snake_case)]`：模块级 lint 许可，用于保留 Go 风格的 `IsNextGen`、`IsClassic` 公共名称。

文件没有常量、类型、trait、宏、可变静态数据或额外的内部辅助函数。

## 执行流程

1. 构建解析 [`Cargo.toml`](Cargo.toml) 的 `nextgen` feature。若未启用，本文件不会由 [`lib.rs`](lib.rs) 纳入模块树；若启用，`classic.rs` 被排除而本文件被纳入。
2. `lib.rs` 将本模块的公开项重导出到 crate 根，调用方使用 `astersql_config_kerneltype::IsNextGen()`、依赖别名后的 `kerneltype::IsNextGen()`，或对应的 `IsClassic()`。
3. 调用 `IsNextGen()` 时直接得到 `true`，无输入、分支和副作用。
4. 调用 `IsClassic()` 时先调用本模块的 `IsNextGen()`，再对结果取反，得到 `false`。
5. 最近的 crate 内部消费链是 `type.rs::Name -> nextgen.rs::IsNextGen`；`Name()` 据此返回规范字符串 `"Next Generation"`。`type.rs::IsMatch` 再用该名称校验 PD 上报值。

因为选择在编译期完成，优化器通常可以折叠这些常量结果及其驱动的局部分支；源码语义本身不依赖这种优化才能正确。

## 数据与状态

两个函数都没有参数，返回普通 `bool`，不持有也不修改状态。结果只由“当前二进制是否以 `nextgen` feature 构建”决定，并在同一二进制生命周期内保持不变。

关键不变量是：在 NextGen 实现中 `IsNextGen() == true`、`IsClassic() == false`，且 `IsClassic() == !IsNextGen()`。[`type_test.rs`](type_test.rs) 和 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 都验证两个标志互补；后者还用 `cfg!(feature = "nextgen")` 核对标志与 `Name()` 的结果一致。

## 依赖与调用关系

下游依赖仅有 `IsClassic -> IsNextGen`，没有外部 crate 调用。源码也不分配内存、不执行 I/O。

上游首先是 [`lib.rs`](lib.rs) 的条件模块装配与公开重导出，以及 [`type.rs`](type.rs) 对 `IsNextGen` 的直接模块内调用。仓库搜索还能确认公共 API 的代表性消费者：

- [`pkg/config/config.rs`](../config.rs) 用 `IsNextGen()` 初始化形态相关默认配置；
- [`pkg/config/deploymode/mode.rs`](../deploymode/mode.rs) 将内核类型与部署模式组合判断；
- [`cmd/tidb-server/main.rs`](../../../cmd/tidb-server/main.rs) 在服务启动和配置检查路径中按内核类型分支；
- [`pkg/store/store.rs`](../../store/store.rs) 与 [`pkg/kv/utils.rs`](../../kv/utils.rs) 在存储及 keyspace 判定中消费该标志；
- [`pkg/dxf/importinto/job.rs`](../../dxf/importinto/job.rs) 等导入调度代码同时使用 `IsNextGen` 和 `IsClassic`。

多个上层 crate 的 `Cargo.toml` 把自己的 `nextgen` feature 转发到 `astersql-config-kerneltype/nextgen`，例如 `pkg/session/Cargo.toml`、`pkg/store/Cargo.toml`、`pkg/kv/Cargo.toml` 与 `pkg/dxf/importinto/Cargo.toml`。因此安全接线的关键不是运行时调用顺序，而是 feature 是否沿依赖图一致传播。

## 错误处理与边界

本文件没有可失败操作，不返回 `Result`/`Option`，也不会 panic。它不校验 feature 传播是否完整；若某个上层构建没有把预期的 `nextgen` feature 转发到本 crate，编译仍可能成功，但得到的是 Classic 实现。这属于构建接线边界，需在相关 Cargo manifest 与 feature 组合测试中发现。

两个函数上的 `#[cfg(feature = "nextgen")]` 与 `lib.rs` 的同条件模块声明目前一致。若将本文件脱离既有模块装配单独引用，或只修改其中一处条件，可能产生符号缺失或重复实现；扩展时必须保持模块声明、重导出和函数条件同步。

本文件也不负责解释未知内核名称或旧 PD 的空类型字段；这些边界由 [`type.rs`](type.rs) 的 `IsMatch()` 处理，其中空字符串仅兼容为 Classic。

## 并发与资源生命周期

函数是无状态的纯读取接口，没有锁、原子变量、线程局部状态、任务、通道、事务、句柄或需要释放的资源。任意线程可并发调用，结果相同，调用之间不存在次序约束。

其“生命周期”与编译产物一致：feature 在编译时固定后，进程运行期间不能把 Classic 切换为 NextGen，反之亦然。需要另一种内核身份时必须构建并部署另一份二进制；[`doc.go`](doc.go) 也明确说明不同内核类型对应不同二进制，不支持依靠运行时混合组件来替代构建选择。

## 与 Go 版本的对应关系

直接对照文件是 [`nextgen.go`](nextgen.go)。Go 通过 `//go:build nextgen` 选择该文件，Rust 通过 Cargo feature 与 `#[cfg(feature = "nextgen")]` 达到相同目的；两边的 `IsNextGen` 都直接返回 `true`，`IsClassic` 都返回 `!IsNextGen()`。

Rust 版本保留了 Go 的函数名和逻辑，没有把判断改成运行时配置，也没有增加第三种内核状态。主要语言层差异是：Go build tag 位于文件级，Rust 同时依靠 `lib.rs` 的模块级条件和函数上的 `cfg`；Rust crate 还由 `Cargo.toml` 显式声明并由上层 feature 转发。

[`type_test.go`](type_test.go) 的 `TestKernelType` 与 Rust 的 [`type_test.rs`](type_test.rs) 保持互补性断言；Go 的 `TestIsMatch` 与 Rust 测试也都覆盖当前构建名称和未知名称。Rust 额外的 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 明确用 `cfg!(feature = "nextgen")` 检查所选构建、标志与名称的一致性，并覆盖大小写敏感及空 PD 类型的兼容语义。

## 扩展指南

若只是新增一个消费内核类型的业务分支，应调用 crate 根重导出的 API，不要直接依赖私有的 `nextgen` 模块，也不要在别处重复读取 feature。这样 Classic 与 NextGen 构建会继续共享同一调用接口。

若要改变内核类型模型或新增状态，需要同步审查 `nextgen.rs`、[`classic.rs`](classic.rs)、[`type.rs`](type.rs)、[`lib.rs`](lib.rs) 和 [`Cargo.toml`](Cargo.toml)，并保持 Go 对照文件的真实语义；当前二值 API 和 `IsClassic == !IsNextGen` 无法表达第三种状态。还应检查所有向本 crate 转发 `nextgen` 的上层 Cargo feature，避免工作区出现身份不一致。

测试应继续放在独立文件，而不是嵌入本生产源文件。至少同步更新 [`type_test.rs`](type_test.rs) 与 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并对照 [`type_test.go`](type_test.go)。涉及 feature 接线时需要分别验证默认构建和启用 `nextgen` 的构建；本次纯文档任务按计划不运行 Cargo。

兼容风险主要是改变公开函数名、返回不变量或规范名称后影响大量跨模块条件分支；正确性风险主要是 feature 漏转发；性能风险很低，因为当前实现无 I/O、分配与同步，且结果可在编译优化中折叠。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，其中 Rust 文件 7,032 个；`files --filter pkg/config/kerneltype` 找到 `nextgen.rs` 及其 Go/Rust 对照与测试。
- RustCodeGraph `node --file pkg/config/kerneltype/nextgen.rs --offset 1 --limit 240`：确认文件共 52 行，公开符号为 `IsNextGen` 和 `IsClassic`，并给出本文件被其他文件使用的索引关系。
- RustCodeGraph `query IsNextGen --kind function` 与 `query IsClassic --kind function`：确认两个目标符号分别位于第 40、50 行，并同时识别 Go 对照符号。精确 `callers`/`callees` 查询在 30 秒内未返回结果，因此调用边以源码装配和 `rg` 的直接引用补证，未据此臆造完整调用图。
- 已读取源码与配置：[`nextgen.rs`](nextgen.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`type.rs`](type.rs)、[`classic.rs`](classic.rs)、[`doc.go`](doc.go) 和 [`nextgen.go`](nextgen.go)。
- 已读取独立测试：[`type_test.rs`](type_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 与 [`type_test.go`](type_test.go)。它们共同证明互补标志、feature 选择、规范名称、空 PD 类型兼容及未知/大小写不匹配边界。
- `rg` 对 Rust 源码确认了 crate 内 `type.rs::Name` 调用，以及配置、服务启动、会话、存储、KV、分布式导入等代表性公共 API 消费者；对 Cargo manifests 的搜索确认多个上层 `nextgen` feature 向本 crate 转发。
- 本文只说明现有代码事实，没有修改 Rust、Go、Cargo 或总计划，也没有运行 Cargo。最终交付以任务文件规定的 11 章节结构命令、链接检查和 diff 自检为准。
