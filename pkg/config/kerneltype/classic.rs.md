# `pkg/config/kerneltype/classic.rs`

## 文件定位

`IsClassic()` 定义为 `!IsNextGen()`，因此恒为 `true`，并显式维持两个判定互补的不变量。

## 核心职责

本文件只负责回答当前编译产物属于哪一种内核形态：

- `IsNextGen()` 在 Classic 构建中恒为 `false`；
- `IsClassic()` 定义为 `!IsNextGen()`，因此恒为 `true`，并显式维持两个判定互补的不变量。

它不解析配置、不探测集群，也不持有当前内核类型的运行时变量。上层代码通过 crate 根的统一重导出调用相同 API，而编译期 feature 决定实际链接 Classic 还是 NextGen 实现。该设计使内核差异的入口集中于两个布尔判定，并允许编译器消除常量分支。

## 主要符号

- `#![allow(non_snake_case)]`：允许保留 Go 导出的驼峰命名，降低移植接口的命名差异。
- `#[cfg(not(feature = "nextgen"))] pub fn IsNextGen() -> bool`：Classic 分支的基础判定函数，无参数、无副作用，固定返回 `false`。其条件编译属性与 [`classic.go`](classic.go) 的 `//go:build !nextgen` 对应。
- `#[cfg(not(feature = "nextgen"))] pub fn IsClassic() -> bool`：派生判定函数，返回 `!IsNextGen()`；当前分支结果为 `true`，并让互补关系由实现表达而非重复硬编码。

本文件没有常量、类型、trait、结构体、可变静态量或额外内部函数。两个函数均为公开 API，但通常经 `lib.rs` 重导出，而不是通过私有模块路径访问。

## 执行流程

1. Cargo 解析 `astersql-config-kerneltype` 的 feature 集合。默认 feature 为空时，`lib.rs` 的 `#[cfg(not(feature = "nextgen"))]` 选择 `classic` 模块；启用 `nextgen` 时则选择互斥的 `nextgen` 模块。
2. Classic 构建的调用方从 crate 根调用 `IsNextGen()` 或 `IsClassic()`。
3. `IsNextGen()` 直接返回 `false`；`IsClassic()` 调用 `IsNextGen()` 后取反，返回 `true`。
4. 同 crate 的 [`type.rs`](type.rs) 使用所选分支的 `IsNextGen()`：`Name()` 据此返回 `"Classic"`，`IsMatch()` 再用该名称校验 PD 上报值，并把旧 PD 的空内核类型视为 Classic。
5. 仓库中的消费者据此选择内核专属路径。例如 [`br/pkg/utils/common.rs`](../../../br/pkg/utils/common.rs) 用 `IsClassic()` 拒绝 Classic 加非空 keyspace 的组合，并用 `IsNextGen()` 执行 NextGen 兼容性检查；[`pkg/store/store.rs`](../../store/store.rs) 和 [`pkg/session/runtime/session.rs`](../../session/runtime/session.rs) 也以该判定选择存储或会话行为。

## 数据与状态

本文件没有持久化数据、堆分配、缓存或全局可变状态。唯一语义状态来自 Cargo feature 集合，并在编译时固定：未启用 `nextgen` 即为 Classic。函数每次调用都返回同一结果，既不读取环境变量、配置文件或 PD 响应，也不存在初始化先后顺序。

核心不变量是 `IsClassic() == !IsNextGen()`。在本文件生效的构建中，还可进一步得到 `IsNextGen() == false`、`IsClassic() == true`。当前内核名称和 PD 匹配规则不存放在本文件，而由 `type.rs` 基于该不变量派生。

## 依赖与调用关系

下游依赖极小：`IsClassic()` 只调用同文件的 `IsNextGen()`，`IsNextGen()` 不调用任何函数。RustCodeGraph 对两个精确符号执行 `callees` 未返回额外边，与源码一致。

直接模块关系如下：

- [`lib.rs`](lib.rs) 负责按 feature 二选一装配并重导出本文件或 `nextgen.rs`；
- [`type.rs`](type.rs) 在 Classic 配置下通过 `use super::classic::IsNextGen` 直接引用本实现，用于 `Name()` 与 `IsMatch()`；
- [`Cargo.toml`](Cargo.toml) 声明 `nextgen` feature，但没有运行时依赖；
- 多个 workspace crate 通过路径依赖消费 `astersql-config-kerneltype`，包括 `pkg/config`、`pkg/session`、`pkg/store`、`pkg/ddl`、`pkg/planner/core`、`br/pkg/utils` 与 `br/pkg/version/build`。

RustCodeGraph 将该文件标记为被 61 个文件使用；精确符号检索与仓库引用搜索显示，上游使用覆盖配置默认值、会话/DDL、存储、分布式任务、BR 兼容检查和相关测试。这里的函数是全局内核能力门控信号，但不实现这些上层分支本身。

## 错误处理与边界

两个函数均返回普通 `bool`，没有 `Result`、panic 路径或可恢复错误。其边界不是输入校验，而是构建配置：本文件只在 `not(feature = "nextgen")` 下定义有效符号；`nextgen.rs` 在相反条件下提供同名实现，`lib.rs` 保证对外接口二选一。

本文件不会验证各组件是否使用一致 feature，也不会在运行时检测混合内核部署。若 workspace 中 feature 传播错误，风险体现为编译出的常量判定与预期部署形态不符；安全扩展时必须同时检查上层 crate 的 feature 转发，而不能在这里增加运行时猜测。PD 空值、名称大小写及未知值的兼容边界由 `type.rs::IsMatch` 处理，不应混入这两个基础判定函数。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络连接，也没有需要释放的资源。两个纯函数可由任意线程并发调用，不存在竞态和调用次数限制。其“生命周期”与编译产物一致：feature 在构建时决定实现，进程运行期间不可切换内核类型。

## 与 Go 版本的对应关系

[`classic.go`](classic.go) 使用 `//go:build !nextgen`，其中 `IsNextGen()` 返回 `false`、`IsClassic()` 返回 `!IsNextGen()`；本文件逐项保留了函数名、签名语义和派生关系，仅将 Go build tag 映射为 Rust 的 `#[cfg(not(feature = "nextgen"))]`。

## 扩展指南

- 若只新增某个内核专属业务行为，应在业务所属模块调用现有判定，不要向本文件加入该业务逻辑。
- 若改变内核判定 API，必须同步 `classic.rs`、`nextgen.rs`、`lib.rs` 的条件重导出以及 Go 的 `classic.go`/`nextgen.go`，确保两个构建暴露相同接口。
- 若增加第三种内核形态，布尔互补模型将不再足够；应先设计枚举或等价的单一类型来源，再同步 `type.rs::Name`、`type.rs::IsMatch`、PD 名称协议和所有布尔调用点，不能仅在 `IsClassic()` 中追加分支。
- 若修改 feature 名或默认值，应同步 `pkg/config/kerneltype/Cargo.toml` 以及 `pkg/session`、`pkg/dxf/importinto`、`pkg/dxf/framework/storage` 等转发 `nextgen` 的 manifest，防止不同 crate 得到不一致的能力判断。
- 测试应继续放在独立文件，至少同步 `type_test.rs` 和 `migration_aster_unit_test.rs`；若改动 Go 对齐语义，也应同步 `type_test.go`。兼容性风险主要是旧 PD 空值、名称字符串和上层 feature 传播；性能风险目前仅为可内联的常量判定，扩展时应避免引入每次调用的 I/O 或锁。
