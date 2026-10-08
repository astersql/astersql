# `pkg/util/engine/engine.rs`

## 文件定位

`engine.rs` 是 Cargo 包 `astersql-util-engine` 的业务实现文件，负责把 PD/kvproto store 标签归一为三个 TiFlash 节点判定 API。crate 入口 `pkg/util/engine/lib.rs` 通过 `pub mod engine` 和 `pub use engine::*` 公开本文件的类型与函数；根 facade 又在 `pkg/lib.rs` 的 `util::engine` 下重导出该 crate。`pkg/util/engine/Cargo.toml` 没有声明运行时依赖，说明传输类型适配被刻意留给调用方，而不是由该 crate 直接依赖 kvproto 或 PD HTTP 模型。

文件对应 Go 包 `pkg/util/engine`，但 Rust 版本用 `Label`、`LabelStore` 两个 trait 代替 Go 函数签名中的具体 `metapb.Store` 和 `pdhttp.MetaStore`。因此它当前是轻量、同步、只读的标签策略层，而不是 store 获取、状态管理或网络访问层。

## 核心职责

- 用 `Label` 抽象一条标签的 `key`/`value` 读取能力，用 `LabelStore` 抽象 store 的标签切片。
- 用私有函数 `is_tiflash_label` 定义通用 TiFlash 集合：标签必须同时满足 `key == "engine"`，且值为 `"tiflash"` 或 `"tiflash_compute"`。
- `IsTiFlash` 与 `IsTiFlashHTTPResp` 对不同来源保持相同识别语义：Classic TiFlash、NextGen 写节点和 NextGen compute 节点均视为 TiFlash。
- `IsTiFlashWriteHTTPResp` 定义更窄的“可写/承载 Region 的 TiFlash”集合：只接受 `engine=tiflash`，从而排除 `engine=tiflash_compute`。

本文件不检查 `engine_role=write` 或 `exclusive=no-data` 是否同时存在。其判定合同只依赖 `engine` 标签；这是与 Go 实现一致的有意行为，而非完整验证 PD 标签组合的校验器。

## 主要符号

- `pub trait Label`：公开适配边界，要求实现者以借用字符串形式提供 `fn key(&self) -> &str` 和 `fn value(&self) -> &str`。该接口不拥有、不复制标签文本。
- `pub trait LabelStore`：公开 store 适配边界；关联类型 `type Label: Label` 固定切片元素类型，`fn labels(&self) -> &[Self::Label]` 借用全部标签。
- `fn is_tiflash_label(label: &impl Label) -> bool`：私有共享谓词。只做大小写敏感的精确字符串比较，避免两个宽口径公开函数漂移。
- `pub fn IsTiFlash(store: &(impl LabelStore + ?Sized)) -> bool`：kvproto 语义入口。`?Sized` 允许接收不要求 `Sized` 的 store 实现引用；通过 `iter().any(...)` 判断是否至少有一条 TiFlash engine 标签。
- `pub fn IsTiFlashHTTPResp(store: &(impl LabelStore + ?Sized)) -> bool`：PD HTTP 语义入口，当前实现与 `IsTiFlash` 相同，保留独立名称以对齐 Go API 和调用语境。
- `pub fn IsTiFlashWriteHTTPResp(store: &(impl LabelStore + ?Sized)) -> bool`：PD HTTP 写节点过滤入口，只接受精确的 `engine=tiflash`。

三个公开函数保留 Go 风格名称，并以 `#[allow(non_snake_case)]` 局部豁免 Rust 命名规范；它们均返回 `bool`，不分配持久状态，也没有条件编译分支。

## 执行流程

`IsTiFlash` 和 `IsTiFlashHTTPResp` 的流程相同：

1. 调用传入 `LabelStore::labels` 借用标签切片。
2. 按原有顺序迭代标签，并将每个元素交给 `is_tiflash_label`。
3. 私有谓词先精确比较 key 是否为 `engine`，再比较 value 是否为 `tiflash_compute` 或 `tiflash`。
4. `Iterator::any` 在首个命中处短路返回 `true`；全部不匹配或切片为空时返回 `false`。

`IsTiFlashWriteHTTPResp` 也借用标签切片并用 `any` 短路，但将条件内联为 `key == "engine" && value == "tiflash"`。因此 NextGen 写节点即使带有额外的 `engine_role=write` 也会命中；NextGen compute 的 `engine=tiflash_compute` 不会命中。

真实 Rust 接线可见于 `br/pkg/conn/util/util.rs`：该文件用 `EngineLabel` 和 `EngineLabelSlice` 实现两个 trait，`is_store_tiflash` 把 kvproto 标签转换后调用 `IsTiFlash`，再用于 BR 获取 TiKV store 时的报错、跳过或仅保留 TiFlash 策略。RustCodeGraph 未为本文件两个 HTTP 入口找到直接生产调用；它们当前应描述为已公开且已有测试的兼容 API，而非宣称已进入所有 Go 对应调用链。

## 数据与状态

本文件没有模块级常量、结构体、枚举、全局变量、缓存或可变状态。判定所需数据完全来自调用方借用的标签切片，生命周期受调用方 store 控制。

关键数据合同如下：

- key/value 都按 UTF-8 `&str` 做大小写敏感、逐字节等值比较；例如 `Engine`、`TiFlash`、前后带空格的值均不会匹配。
- 标签顺序不影响最终布尔值，只影响短路位置；重复匹配标签仍只产生一个布尔结果。
- 无标签、只有 `engine_role=write`、只有 `exclusive=no-data` 或 `engine=not_tiflash` 都返回 `false`。
- 宽口径函数接受 `tiflash_compute`；写口径函数拒绝它。这个集合差异是本文件最重要的不变量。

## 依赖与调用关系

下游依赖仅是 Rust 标准库提供的切片与迭代器能力；`Cargo.toml` 没有 `[dependencies]`。本文件内部调用边为：

- `IsTiFlash -> LabelStore::labels -> Iterator::any -> is_tiflash_label -> Label::{key,value}`。
- `IsTiFlashHTTPResp -> LabelStore::labels -> Iterator::any -> is_tiflash_label -> Label::{key,value}`。
- `IsTiFlashWriteHTTPResp -> LabelStore::labels -> Iterator::any -> Label::{key,value}`。

上游边界分为三层：`pkg/util/engine/lib.rs` 重导出公开 API；根 `pkg/lib.rs` 以 `facade_util_engine` 再导出；具体调用方为自身数据模型实现 trait。已验证的直接生产调用是 `br/pkg/conn/util/util.rs::is_store_tiflash -> astersql_util_engine::IsTiFlash`。工作区根 `Cargo.toml` 以 `facade_util_engine = { package = "astersql-util-engine", path = "pkg/util/engine" }` 注册 facade；`br/pkg/conn/util/Cargo.toml` 直接声明该包依赖。`pkg/domain`、`pkg/ddl`、`pkg/ingestor/ingestctrl` 和一处 RealTiKV 测试清单也声明了该 crate，但仅凭依赖声明不能推断它们已经调用本文件的三个函数。

## 错误处理与边界

API 没有 `Result`、错误类型或 panic 分支：未知、缺失和不完整标签统一折叠为 `false`。调用者若需要区分“不是 TiFlash”和“标签数据损坏/缺失”，必须在本层之外验证数据来源。

函数参数是引用而非 `Option`，所以 Rust 类型系统排除了 Go 风格 nil store；但 trait 实现自身若在 `labels`、`key` 或 `value` 中 panic，本文件不会捕获。判定不会验证 store 状态（Up/Offline/Tombstone）、地址、Region、角色标签一致性或标签唯一性。特别地，带 `engine=tiflash` 的任意 store 都会被写节点函数接受，即使没有 `engine_role=write`；这与 `pkg/util/engine/engine.go` 当前行为一致。

测试边界由 `pkg/util/engine/engine_test.rs` 和 `pkg/util/engine/migration_aster_unit_test.rs` 证明：Classic 标签为真；NextGen 写标签为真；compute 在宽口径为真而在写口径为假；非 TiFlash 和空标签为假；迁移测试还覆盖前置无关标签不阻止后续命中。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件、网络连接或堆资源句柄。所有函数只持有调用期间的共享借用，不修改 store 或标签，因此其自身没有清理步骤和跨调用生命周期。

并发安全性取决于调用方类型：这些 trait 没有要求 `Send` 或 `Sync`，本 crate 不承诺实现者可跨线程共享；但只要调用方能合法提供共享引用，函数内部是无副作用、可重入的。时间复杂度为 `O(n)`，其中 `n` 为标签数；因 `any` 短路，最佳情况为首标签命中。额外空间为 `O(1)`，但调用方适配层可能自行分配，例如 `br/pkg/conn/util/util.rs::is_store_tiflash` 当前先构造 `Vec<EngineLabel>`。

## 与 Go 版本的对应关系

`pkg/util/engine/engine.go` 提供同名的三个函数，匹配集合与 Rust 完全一致：

- Go `IsTiFlash(*metapb.Store)` 对应 Rust `IsTiFlash(&impl LabelStore)`。
- Go `IsTiFlashHTTPResp(*pdhttp.MetaStore)` 对应 Rust `IsTiFlashHTTPResp(&impl LabelStore)`。
- Go `IsTiFlashWriteHTTPResp(*pdhttp.MetaStore)` 对应 Rust `IsTiFlashWriteHTTPResp(&impl LabelStore)`。

主要实现差异是类型接线。Go 直接访问具体类型的 `Labels`、`Key`、`Value` 字段；Rust 通过 trait 隔离传输模型，所以 crate 自身没有 kvproto/PD HTTP 依赖，调用方需提供适配实现。两边都按顺序扫描并在首个命中处返回，且都不要求 NextGen 写节点显式出现 `engine_role=write`。

Go 测试 `pkg/util/engine/engine_test.go` 只覆盖两个 HTTP 函数；Rust 的 `engine_test.rs` 复刻相同表格，而 `migration_aster_unit_test.rs` 额外覆盖 `IsTiFlash` 及无关标签在前的情形。Rust 测试与生产逻辑保持在独立文件中，并由 `lib.rs` 的 `#[cfg(test)]`/`#[path]` 接线。

## 扩展指南

若新增 engine 值或调整节点分类，应先明确它属于“所有 TiFlash”还是“可写 TiFlash”：前者应修改 `is_tiflash_label`，后者还需评估 `IsTiFlashWriteHTTPResp` 的独立条件。不要为了减少重复而直接让写函数调用宽口径函数，否则会把 compute 节点错误纳入写节点集合。

若接入新的 store 数据模型，优先在数据模型所属 crate 实现 `Label`/`LabelStore`，或像 `br/pkg/conn/util/util.rs` 一样建立局部适配器；不要把传输层依赖加入本 crate，除非确有必要改变其无依赖边界。若适配会复制全部字符串，应评估能否直接借用原模型以避免每次判定分配。

行为修改必须同步独立测试文件 `pkg/util/engine/engine_test.rs` 和 `pkg/util/engine/migration_aster_unit_test.rs`，并核对 Go 的 `pkg/util/engine/engine.go`、`engine_test.go`。应至少保留 Classic、NextGen write、NextGen compute、未知 engine、空标签、无关标签先于有效标签、大小写/空白精确匹配等用例。兼容风险主要是调用方筛选集合改变；性能风险主要来自高频调用时的标签遍历和调用方适配分配。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 Rust/Go 源与测试均已索引。
- RustCodeGraph `node --file pkg/util/engine/engine.rs`：核对 `Label`、`LabelStore`、`is_tiflash_label` 以及三个公开函数的完整实现。
- RustCodeGraph `query`、`callers`、`callees` 与精确 `explore`：确认两个宽口径函数调用 `labels`，写口径函数调用 `labels`/`key`/`value`；调用者查询存在同名歧义，因此又以精确源码引用补充核验，未把 Go 调用边误记为 Rust 调用边。
- RustCodeGraph `node`：读取 `pkg/util/engine/lib.rs`、`pkg/util/engine/engine_test.rs`、`pkg/util/engine/migration_aster_unit_test.rs`、`pkg/util/engine/engine.go`、`pkg/util/engine/engine_test.go`、`br/pkg/conn/util/util.rs` 与 `pkg/lib.rs`，核对模块导出、测试合同、Go 对照和直接生产接线。
- 原始配置读取：`pkg/util/engine/Cargo.toml`、`pkg/util/engine/BUILD.bazel`、工作区根 `Cargo.toml` 及相关 crate manifest 引用，核对 crate 名称、Go 包映射、无 Rust 外部依赖边界和工作区接线。
- `rg` 补充图未能无歧义覆盖的引用搜索：确认本文件公开符号在 Rust 生产源码中的直接引用，并区分同名字段、局部 stand-in 与真实 crate 导入。
- 未运行 Cargo 或 Go 测试：本任务只创建说明文档，任务计划明确要求以事实查询和结构检查验证，不运行 Cargo。
