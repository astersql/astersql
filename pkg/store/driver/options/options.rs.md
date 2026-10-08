# `pkg/store/driver/options/options.rs`

## 文件定位

本文件属于 `astersql-store-driver-options` crate。crate 边界由同目录的 `Cargo.toml` 定义，入口是 `lib.rs`；入口以私有模块 `mod options` 装载本文件，再通过 `pub use options::*` 对外导出这里的公开枚举和函数。该 crate 还用 `include!("../../../kv/option.rs")` 形成 `crate::kv`，因此本文件接收的 `kv::ReplicaReadType` 实际定义在 `pkg/kv/option.rs`。

它是 TiDB/AsterSQL 侧七种副本读策略与 TiKV store 层五种策略之间的适配边界。当前 Rust 仓库中，目标 API 的直接调用者只有同 crate 的独立迁移测试 `pkg/store/driver/options/migration_aster_unit_test.rs`；`pkg/store/copr/Cargo.toml` 虽将本 crate 声明为可选依赖，workspace 根 `Cargo.toml` 也以 `facade_store_driver_options` 暴露它，但未搜索到 Rust 生产代码直接调用 `GetTiKVReplicaReadType`。因此它目前是已实现、已由单元测试约束、但尚未进入 Rust 生产请求主链的迁移接口。

## 核心职责

本文件只承担两项职责：

1. 用 `TiKVReplicaReadType` 表达 TiKV store 层可消费的五种副本选择策略，并固定其 `u8` 判别值。
2. 用 `GetTiKVReplicaReadType` 将 `pkg/kv/option.rs::ReplicaReadType` 的七个输入变体映射到上述五个输出变体。其中 `ReplicaReadMixed`、`ReplicaReadClosest` 和 `ReplicaReadClosestAdaptive` 都折叠为 `ReplicaReadMixed`。

它不负责选择 Region、发送 RPC、重试、读取拓扑或执行实际路由。Go 生产代码会把映射结果交给 client-go；Rust 生产侧的 `pkg/store/driver/kv_adapter.rs` 当前有另一套直接映射到 coprocessor 策略的逻辑，不能据此宣称本文件已接入该路径。

## 主要符号

- `TiKVReplicaReadType`：公开的 `#[repr(u8)]` 枚举，派生 `Clone`、`Copy`、`Debug`、`Default`、`Eq` 和 `PartialEq`。五个判别值分别是 `ReplicaReadLeader = 0`、`ReplicaReadFollower = 1`、`ReplicaReadMixed = 2`、`ReplicaReadLearner = 3`、`ReplicaReadPreferLeader = 4`；`ReplicaReadLeader` 由 `#[default]` 指定为默认值。这些数值由 `store_policy_discriminants_match_client_go_abi` 测试锁定。
- `GetTiKVReplicaReadType(t: kv::ReplicaReadType) -> TiKVReplicaReadType`：公开、同步、无副作用的总映射函数。它对 `kv::ReplicaReadType` 的全部七个 Rust 枚举变体进行穷尽匹配，不返回 `Result` 或可空值。
- `use crate::kv`：把 crate 入口中内嵌的 KV option 契约引入当前模块；输入类型并非外部 `tikv-client` 类型。

## 执行流程

调用者先提供一个 `kv::ReplicaReadType`，函数随后执行一次穷尽 `match`：

1. `ReplicaReadLeader` 原样映射为 store 层 Leader。
2. `ReplicaReadFollower` 原样映射为 store 层 Follower。
3. `ReplicaReadMixed`、`ReplicaReadClosest`、`ReplicaReadClosestAdaptive` 进入同一匹配分支，统一返回 store 层 Mixed。折叠发生在此适配边界，意味着输出不再保留 Closest 与 ClosestAdaptive 的区别。
4. `ReplicaReadLearner` 原样映射为 store 层 Learner。
5. `ReplicaReadPreferLeader` 原样映射为 store 层 PreferLeader。

函数没有后续步骤，也不会根据集群状态动态改写策略。独立测试 `maps_every_tidb_replica_read_policy_like_go` 以表驱动方式覆盖全部七个输入及五种期望输出。

## 数据与状态

输入 `kv::ReplicaReadType` 也是 `#[repr(u8)]` 枚举，但有七个判别值 `0..=6`；输出 `TiKVReplicaReadType` 只有五个判别值 `0..=4`。两者不能通过整数直接等值转换：例如输入 `ReplicaReadLearner` 的值为 `5`，输出同名策略的值为 `3`，必须经过显式映射。

本文件不维护全局变量、缓存或可变状态。输入和输出均为 `Copy` 值，调用期间只产生一个枚举返回值。`TiKVReplicaReadType::default()` 固定为 Leader，这一默认语义和字节判别值都属于对外兼容契约，而非普通实现细节。

## 依赖与调用关系

- 上游类型依赖：`crate::kv::ReplicaReadType`，由 `pkg/store/driver/options/lib.rs` 内嵌 `pkg/kv/option.rs` 后提供。
- 模块导出：`pkg/store/driver/options/lib.rs` 的 `pub use options::*` 将两个公开符号提升到 crate 根。
- Rust 直接调用者：索引和 `rg` 均只发现 `pkg/store/driver/options/migration_aster_unit_test.rs::maps_every_tidb_replica_read_policy_like_go`。
- crate 声明关系：根 `Cargo.toml` 声明 `facade_store_driver_options`；`pkg/store/copr/Cargo.toml` 声明同 crate 的可选路径依赖，但当前 Rust 源码未引用其 crate 名或本文件符号。
- Go 生产调用者：`pkg/store/copr/coprocessor.go`、`pkg/store/copr/region_cache.go`、`pkg/store/driver/txn/snapshot.go` 和 `pkg/store/driver/txn/txn_driver.go` 调用同路径 Go 函数，把 TiDB 策略转换为 client-go 的 store 策略。
- 相邻 Rust 路由：`pkg/store/driver/kv_adapter.rs` 直接把 Leader/PreferLeader、Follower、Mixed/Closest/ClosestAdaptive、Learner 分组映射到 coprocessor 类型；这是迁移状态证据，不是本函数的调用边。

## 错误处理与边界

Rust 输入枚举是封闭集合，`match` 覆盖当前全部变体，所以函数没有未知值分支、错误返回或 panic 路径。若未来在 `kv::ReplicaReadType` 中新增变体，编译器会要求这里补充分支，这比 Go 实现的末尾 `return 0` 更能暴露遗漏。

语义边界是 Closest 类策略的信息损失：`ReplicaReadClosest` 与 `ReplicaReadClosestAdaptive` 都降级为 Mixed；本文件不会执行同可用区筛选或响应大小阈值判断。字节 ABI 边界也需要谨慎维护：不能按输入枚举的判别值强转输出枚举，因为 Learner 和 PreferLeader 在两侧的数值不同。

本文件不验证策略是否适合当前请求，也不处理底层存储不可用、RPC 失败或拓扑缺失；这些错误属于实际请求执行层。

## 并发与资源生命周期

`GetTiKVReplicaReadType` 是纯同步值转换：不加锁、不启动线程或异步任务、不使用通道、不持有事务、网络连接或文件句柄。`TiKVReplicaReadType` 为 `Copy` 类型，没有析构顺序或所有权转移负担，因此多个线程可各自对输入值执行转换；本文件本身不存在共享可变状态或取消生命周期。

资源生命周期从函数调用开始，到枚举值返回即结束。后续如何保存和消费返回值由调用者负责；Go 生产链路中返回值随请求构造进入 client-go，而当前 Rust 侧尚无对应的生产调用证据。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/driver/options/options.go`。两版对七种输入的正常映射一致：Leader、Follower、Learner、PreferLeader 保持同名策略，Mixed 和两种 Closest 输入统一映射到 Mixed。Go 输出类型是 `github.com/tikv/client-go/v2/kv.ReplicaReadType`；Rust 因当前 TiKV client 未暴露相同路由枚举，在本 crate 内定义 `TiKVReplicaReadType` 并显式固定五个字节值。

主要差异在未知输入处理。Go 的输入是 `byte` 别名，`switch` 未命中时返回数值 `0`，等价于 Leader；Rust 输入是封闭枚举，穷尽 `match` 不需要兜底。Rust 若通过不安全手段制造无效枚举值将违反 Rust 有效性要求，不属于本函数承诺的输入范围。

测试对应关系由 `pkg/store/driver/options/migration_aster_unit_test.rs` 提供，而同目录没有 Go 专用 `options_test.go`。Rust 测试分别锁定七项映射、五个输出判别值和 Leader 默认值；其中 `transaction_schema_checker_uses_shared_error_contract` 测试的是同 crate 内嵌 option 的其他契约，不是本文件逻辑。

## 扩展指南

- 新增 TiDB 侧副本策略时，先修改真实定义 `pkg/kv/option.rs::ReplicaReadType`，再为 `GetTiKVReplicaReadType` 明确选择对应的 store 策略；不要通过数值强转绕过语义判断。
- 若 TiKV store 层新增独立策略，需要同步更新 `TiKVReplicaReadType` 的变体和 `#[repr(u8)]` 数值，并确认这些数值与目标 client ABI 一致。改变既有判别值会有兼容风险。
- 所有映射变化都应在独立测试文件 `pkg/store/driver/options/migration_aster_unit_test.rs` 中增加或调整表驱动用例；不要把测试写回 `options.rs`。
- 若要接入 Rust 生产链路，应先选择单一转换边界，并审查 `pkg/store/driver/kv_adapter.rs` 的现有映射，避免出现两套逻辑漂移。接线还需确认 `pkg/store/copr/Cargo.toml` 的可选依赖 feature 如何启用，而不能仅凭依赖声明认定已生效。
- 对 Closest 或 ClosestAdaptive 增加真正的拓扑语义时，应在拥有 Region/zone/响应大小信息的路由层实现；仅扩充本值转换函数不足以完成该行为，并可能影响路由兼容性与性能。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标路径已被索引；`files --filter pkg/store/driver/options` 列出 `lib.rs`、`options.rs`、Go 对照和独立测试。
- RustCodeGraph `node --file pkg/store/driver/options/options.rs --offset 1 --limit 240`：核对完整 62 行源文件、公开枚举、派生属性、判别值和映射分支；结果同时指出该文件被独立迁移测试使用。
- RustCodeGraph `query GetTiKVReplicaReadType` 与 `node GetTiKVReplicaReadType`：定位 Rust/Go 两个定义并逐分支对照；`explore` 找到 Rust 测试调用边 `migration_aster_unit_test.rs::maps_every_tidb_replica_read_policy_like_go`。
- RustCodeGraph `node --file pkg/kv/option.rs --offset 115 --limit 85`：核对输入枚举七个变体、判别值及辅助方法。
- 已读路径：`pkg/store/driver/options/Cargo.toml`、`pkg/store/driver/options/lib.rs`、`pkg/store/driver/options/options.go`、`pkg/store/driver/options/migration_aster_unit_test.rs`、`pkg/store/copr/Cargo.toml`、`pkg/kv/option.go`、`pkg/store/driver/options/BUILD.bazel`。
- `rg` 引用检查：确认 Rust 直接调用仅在独立迁移测试；确认 Go 生产调用分布在 coprocessor、region cache、snapshot 和 transaction driver；确认 `pkg/store/driver/kv_adapter.rs` 存在相邻但独立的 Rust 策略映射。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付时以任务指定命令校验文档存在且恰有十一个固定二级标题，并人工复核当前接线、边界和扩展建议均有上述源码或配置依据。
