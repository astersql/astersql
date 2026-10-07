# `pkg/ddl/placement/common.rs`

## 文件定位

`common.rs` 是 `astersql-ddl-placement` crate 的公共词汇表：它不执行 DDL job，也不直接访问 PD，而是集中定义 PD placement rule 所需的规则组 ID、特殊 key range 名称、优先级、Store 标签键值，以及对象 ID 到规则组 ID 的编码函数。crate 根在 [`lib.rs`](lib.rs) 中以 `mod common; pub use common::*;` 将本文件全部符号再导出，因此既供同 crate 的 `bundle.rs`、`constraint.rs` 使用，也能被 `pkg/domain`、`pkg/executor`、`pkg/session` 等其他 Rust crate 通过 `astersql_ddl_placement::*` 使用。

所属 crate 由 [`Cargo.toml`](Cargo.toml) 声明为 `astersql-ddl-placement`。本文件自身只依赖 Rust 标准库的字符串格式化；常量的实际消费者再通过该 crate 的 `pdtypes`、`meta-model`、`tablecodec-dependency` 等依赖构造 PD 规则。它是 placement 数据建模和协议约定的一部分，不承担 DDL job 持久化、schema state 迁移、reorg、回滚或 schema version 同步。

## 核心职责

本文件有四组职责，均以 [`common.rs`](common.rs) 中的公开符号为准：

1. 统一规则组命名：`BundleIDPrefix` 与 `GroupID` 生成表或分区所属的 `TiDB_DDL_<id>` 规则组；`TiFlashRuleGroupID`、`PDBundleID`、`TiDBBundleRangePrefixForGlobal`、`TiDBBundleRangePrefixForMeta` 表示协议约定的特殊规则组。
2. 统一范围名称和编码起点：`KeyRangeGlobal`、`KeyRangeMeta` 是 SQL/placement 层识别的逻辑范围名，`metaPrefix` 是 meta key 空间的字节前缀。
3. 固定规则优先级：`RuleIndexKeyRangeForGlobal`、`RuleIndexKeyRangeForMeta`、`RuleIndexTable`、`RuleIndexPartition`、`RuleIndexTiFlash` 形成 `20 < 21 < 40 < 80 < 120` 的层次，使更具体或专用的规则具有更高 index。
4. 统一 Store label 协议：`DCLabelKey`、`EngineLabelKey` 及引擎/角色值避免不同模块各自拼写 PD/TiKV/TiFlash 标签。

这些值是跨模块、跨语言的兼容契约。修改字符串或数值不是局部重命名，会改变发送给 PD 的标识、规则覆盖关系或 Store 筛选结果。

## 主要符号

- `TiFlashRuleGroupID: &str = "tiflash"`：TiFlash 专用规则组 ID；Rust 的 `pkg/domain/infosync/tiflash_manager.rs::makeBaseRule` 将其写入 `TiFlashRule.GroupID`。
- `BundleIDPrefix: &str = "TiDB_DDL_"` 与 `pub fn GroupID(id: i64) -> String`：唯一含逻辑的函数直接格式化前缀和十进制有符号整数，例如 `1 -> TiDB_DDL_1`、`-1 -> TiDB_DDL_-1`。函数不校验 ID 是否为正数。
- `PDBundleID: &str = "pd"`：PD 默认 bundle 名称。当前仓库没有发现 Rust 生产调用点；它作为公开的 Go 对齐常量保留。
- `DefaultKwd: &str = "default"`：表示恢复默认 placement 的关键字。Go 的 `pkg/executor/simple.go` 有实际消费；当前仓库没有发现 Rust 生产调用点。
- `TiDBBundleRangePrefixForGlobal` / `TiDBBundleRangePrefixForMeta`：特殊全局/meta bundle ID，分别为 `TiDB_GLOBAL`、`TiDB_META`；`Bundle::RebuildForRange` 使用它们重写 bundle。
- `KeyRangeGlobal` / `KeyRangeMeta`：外部输入使用的逻辑范围名 `global` / `meta`；`Bundle::RebuildForRange` 以它们选择目标 bundle ID 和 group index。
- `metaPrefix: &[u8] = b"m"`：meta key 空间起点；`GetRangeStartAndEndKeyHex` 对其进行 memcomparable 编码，表前缀 `table_id=0` 是终点。
- `RuleIndexKeyRangeForGlobal = 20`、`RuleIndexKeyRangeForMeta = 21`、`RuleIndexTable = 40`、`RuleIndexPartition = 80`、`RuleIndexTiFlash = 120`：PD rule/group 的优先级常量。`bundle.rs` 使用前四项重建范围与表/分区规则，TiFlash 管理器及 session 资源初始化使用最后一项。
- `DCLabelKey = "zone"`：当前把数据中心概念映射为 Store 的 `zone` 标签；Rust MPP coordinator 用它读取 TiFlash Store 所在 zone。
- `EngineLabelKey = "engine"`、`EngineLabelTiFlash = "tiflash"`：`constraint.rs::NewConstraint` 用它们拒绝普通 placement 约束中的正向 `+engine=tiflash`，因为 TiFlash 由专用规则组管理。
- `EngineLabelTiKV`、`EngineLabelTiFlashCompute`、`EngineRoleLabelKey`、`EngineRoleLabelWrite`：与 Go 标签协议保持一致。当前仓库没有发现这些符号的 Rust 生产消费点；Go 侧用于 GC、infoschema 和 TiFlash 计算/写节点识别。

## 执行流程

本文件没有主动执行入口；其值在调用方流程中被读取。主要路径如下：

1. 表/分区 bundle 创建：`bundle.rs::NewBundle(id)` 调用 `GroupID(id)` 写入 `Bundle.ID`；`Bundle::Reset` 也以首个物理 ID 重建组 ID，并给首个 ID 的规则写 `RuleIndexTable`、后续 ID 的规则写 `RuleIndexPartition`。
2. 特殊范围 bundle：`Bundle::RebuildForRange(range_name, policy_name)` 将 `global` 映射为 `TiDB_GLOBAL/20`，将 `meta` 映射为 `TiDB_META/21`，随后设置 override 并重写各 rule 的 group、key range 和 rule index。未知范围不会改变 bundle ID/index，但函数仍继续执行后续 override 和 rule 重写，因此输入校验应在上游完成。
3. Meta key 范围编码：`bundle.rs::GetRangeStartAndEndKeyHex` 仅对 `TiDB_META` 返回由 `metaPrefix` 到 table 0 前缀的编码边界；其他 bundle ID 返回两个空字符串，全局范围因此使用空起止键表达全 key space。
4. 约束解析：`constraint.rs::NewConstraint` 比较 `EngineLabelKey` 和大小写不敏感的 `EngineLabelTiFlash`，阻止把 TiFlash 当作普通 `In` 约束；`NotIn` 不受此限制。
5. TiFlash 规则：`tiflash_manager.rs::makeBaseRule` 用 `TiFlashRuleGroupID` 和 `RuleIndexTiFlash` 生成 Learner 规则模板；`session/runtime/create_table_resources.rs` 也用 `RuleIndexTiFlash` 校验或创建 PD rule group。
6. MPP locality：`local_mpp_coordinator.rs::add_tiflash_store_info` 以 `DCLabelKey` 从 Store 读取 zone，缺失时降级为空字符串。

## 数据与状态

文件中的数据全部是进程内只读常量，唯一的 `static` 是不可变字节切片 `metaPrefix`；没有可变全局状态、缓存、锁或持久化字段。`GroupID` 每次分配一个新的 `String`，其输出由固定前缀和 `i64` 的十进制显示形式唯一决定，不依赖环境、locale 或调用顺序。

需要保持的关键不变量是：

- `GroupID(id)` 的结果必须能由 `bundle.rs::Bundle::ObjectID` 按同一 `BundleIDPrefix` 去前缀并解析；但两者接受域不同，`GroupID` 接受任意 `i64`，`ObjectID` 会拒绝 `<= 0`。
- 规则 index 的相对顺序必须保持全局范围 < meta 范围 < 表 < 分区 < TiFlash；`Bundle::Reset` 依赖表/分区常量区分规则来源和覆盖层次。
- 字符串值必须与 PD、Store labels、Go 实现和已持久化/外部可见规则保持完全一致。
- `metaPrefix` 必须继续与 TiDB meta key 编码约定一致；改变它会移动特殊 meta range 的边界。

## 依赖与调用关系

下游依赖集中在几条明确边上：

- `common.rs::GroupID` → 标准库 `format!`；`bundle.rs` 源码直接显示 `NewBundle` 与 `Bundle::Reset` 的生产调用，RustCodeGraph 的精确节点/调用者结果另确认了 `meta_bundle_test.rs::expected_bundle` 测试调用。图查询也混入同名 Go 调用者，因此本文用文件路径区分语言，不把它们误算为 Rust 静态调用。
- `bundle.rs` 通过 `use crate::common::*` 消费 ID、范围、meta 前缀和 index 常量；它再由 DDL placement 构造流程调用。
- `constraint.rs` 精确导入 `EngineLabelKey` 与 `EngineLabelTiFlash`，将标签协议落实为约束校验规则。
- `pkg/domain/infosync/tiflash_manager.rs` 通过 crate 再导出消费 TiFlash group ID/index；`pkg/session/runtime/create_table_resources.rs` 消费 TiFlash index；`pkg/executor/internal/mpp/local_mpp_coordinator.rs` 消费 `DCLabelKey`。
- `lib.rs` 是公开边界：本文件虽声明为私有模块 `mod common`，但 `pub use common::*` 使所有 `pub` 项成为 crate 根 API。

`Cargo.toml` 没有为本文件设置 feature gate，`common.rs` 也没有条件编译项。测试模块由 `lib.rs` 的 `#[cfg(test)] mod common_test;` 独立装配，符合源文件与测试文件分离的仓库要求。

## 错误处理与边界

`GroupID` 是总函数，不返回 `Result`，也不检查 ID。零和负数会被忠实格式化；[`common_test.rs`](common_test.rs) 与 [`common_test.go`](common_test.go) 都明确覆盖负数 `-1`。这只是编码行为，不表示负 ID 是可用的持久化对象 ID；下游 `Bundle::ObjectID` 会把 `<= 0` 作为 `ErrInvalidBundleID`。

常量本身不会产生错误，但调用方有以下边界：

- `RebuildForRange` 对未知 `range_name` 没有报错分支，只跳过 ID/index 映射后继续改写 bundle；新增调用者必须先限制输入为 `global` 或 `meta`。
- `GetRangeStartAndEndKeyHex` 仅为 meta bundle 生成显式边界，其他输入返回空边界；不能把空字符串自动解释为查找失败。
- `DCLabelKey` 固定为 `zone` 是现状兼容假设，不是动态配置；缺少该标签时 MPP 路径使用空 zone。
- `EngineLabelTiFlash` 的普通正向约束会被 `NewConstraint` 拒绝，但大小写比较只用于值，key 必须精确等于 `engine`。
- 公开但当前无 Rust 生产消费点的常量仍可能是外部 crate API；不能仅依据仓库内搜索结果删除。

## 并发与资源生命周期

本文件没有线程、异步任务、channel、锁、文件描述符、网络连接或显式资源生命周期。所有常量可安全并发读取；`metaPrefix` 是 `'static` 不可变切片；`GroupID` 返回调用者独占的 `String`，无共享可变别名。

并发影响只存在于协议层：多个 DDL/infosync 调用者若用同一对象 ID，会确定性地产生同一 group ID，PD 侧因而把它们视为同一规则组。唯一性来自上游对象 ID 分配，而不是本文件的同步机制。本文件也不负责 owner 选举、job 重试或 PD 请求生命周期。

## 与 Go 版本的对应关系

直接对照文件是 [`common.go`](common.go)，Rust 在符号集合、字符串字面量、数值和 `GroupID` 格式上逐项保持一致。Go 的 `fmt.Sprintf("%s%d", BundleIDPrefix, id)` 对应 Rust 的 `format!("{BundleIDPrefix}{id}")`；两侧测试都断言 `1`、`90`、`-1` 的完全相同输出。

已验证的语义差异/迁移状态：

- Go 的 `metaPrefix` 是可变 slice 变量 `[]byte("m")`，Rust 收紧为不可变 `&'static [u8]`，当前消费者只读，因此不改变预期行为并减少误修改风险。
- Go 常量为包级导出，Rust 常量先在私有 `common` 模块中公开，再由 crate 根统一再导出，外部调用路径等价于 crate 公共 API。
- Go 生产路径对 `DefaultKwd`、各引擎角色常量和特殊 range 常量的接线更广；Rust 当前只验证到本文“依赖与调用关系”列出的消费点。尤其不能由常量已移植推断相应 Go 控制流已经全部移植。
- Rust 标识符保留 Go 风格大小写，并由 `lib.rs` 的 lint allowance 接纳，以降低逐项对照和调用迁移成本。

## 扩展指南

新增或修改 placement 公共协议值时，应按以下边界接入：

1. 若新增规则组命名方式，在 `common.rs` 定义编码常量/函数，并同步检查 `bundle.rs` 的构造、反向解析和 PD 序列化路径，保证编码与解码域一致。
2. 若新增特殊 key range，在这里增加逻辑名、bundle ID 和 index，并同时扩展 `Bundle::RebuildForRange` 与 `GetRangeStartAndEndKeyHex`；仅新增常量不会产生可运行能力。
3. 若调整优先级，先验证 PD 的 index/override 语义以及表、分区、TiFlash 的相对覆盖关系。兼容风险高于普通重构，因为已有规则可能留在 PD。
4. 若新增 Store 标签，明确 key/value 是 TiKV、TiFlash 还是计算节点协议，并在实际筛选或约束解析位置消费；避免创建永远未接线的“支持”声明。
5. 同步更新独立 Rust 测试，不要把测试内嵌回 `common.rs`。`GroupID` 格式应扩展 [`common_test.rs`](common_test.rs)；range/index 组合优先扩展 `bundle_test.rs` 或 `bundle_1_aster_unit_test.rs`；端到端 TiFlash/MPP 行为应在各自模块测试中验证。
6. 始终与 `common.go` 及相关 Go 调用点复核。若是 Rust 独有协议，需明确说明没有 Go 对照，而不能假借同名常量的语义。

主要风险包括：规则 ID 变化导致旧规则无法覆盖/清理，index 变化导致优先级反转，key range 边界变化导致规则作用于错误数据，以及 label 拼写变化导致 Store 无法匹配。性能风险很低（本文件只有常量读取和一次小字符串分配），但协议错误的集群行为风险很高。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/ddl/placement` 确认目标源、Go 对照和独立测试均被索引。
- RustCodeGraph 源/符号查询：读取 `common.rs` 全部 75 行；`query GroupID --kind function` 区分 Go/Rust 同名定义；`node pkg/ddl/placement/common.rs::GroupID` 确认函数体；`callers GroupID --file pkg/ddl/placement/common.rs` 返回 Go 的 `NewBundle`/`Reset` 与 Rust 的 `expected_bundle`，Rust 生产调用则由 `bundle.rs` 的精确源码节点核对。
- RustCodeGraph 调用上下文：读取 `lib.rs` 的模块再导出与测试装配、`bundle.rs::{NewBundle, RebuildForRange, Reset, ObjectID, GetRangeStartAndEndKeyHex}`、`constraint.rs::NewConstraint`、`tiflash_manager.rs::makeBaseRule`、`local_mpp_coordinator.rs::add_tiflash_store_info`。
- crate/对照证据：读取 `pkg/ddl/placement/Cargo.toml`、`common.go`、`common_test.rs`、`common_test.go`；用精确符号搜索核对 Rust 与 Go 的常量消费点。
- 测试事实：Rust 与 Go 的 common 独立测试都覆盖正数和负数 group ID；bundle 独立测试覆盖 meta 范围、表/分区 index 和规则重建。按任务约束，本次为纯文档分析，未运行 Cargo 或代码测试。
- 人工复核结论：本文件存在的原因是集中维护跨 PD、Store labels、Go/Rust 的稳定协议词汇；运行方式是被 bundle、constraint、TiFlash 和 MPP 调用方读取；安全扩展必须同步实际消费者、独立测试与 Go/外部协议，而不能只添加常量。
