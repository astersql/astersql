# `pkg/config/config_util.rs`

## 文件定位

`pkg/config/config_util.rs` 属于 `astersql-config` crate（`pkg/config/Cargo.toml:1-8`），集中放置不属于某一配置结构体的通用操作：配置深拷贝、动态字段合并、嵌套映射展平、热重载回调类型，以及事务作用域读取门面。`pkg/config/lib.rs:37-39` 以私有模块 `config_util` 装配本文件，再通过 `pub use config_util::*` 从 crate 根重导出公开符号。

它不是配置结构定义或配置文件解析入口；主 `Config`、全局配置快照和加载/校验逻辑位于 `pkg/config/config.rs`。当前 Rust 生产接线中，`GetTxnScopeFromConfig` 会被 `pkg/kv/txn_scope_var.rs::NewDefaultTxnScopeVar` 调用；其余工具在仓库内只找到本文件内部调用和独立单元测试调用。尤其是 Go 侧 `pkg/executor/memtable_reader.go:257` 已用 `FlattenConfigItems` 处理 `SHOW CONFIG` 数据，但相应 Rust executor 目前没有直接调用，不能把 Go 接线当作 Rust 已实现的运行链。

## 核心职责

- `CloneConf` 通过 JSON 序列化/反序列化往返创建与原对象不共享内部可变数据的副本（`pkg/config/config_util.rs:38-45`）。
- `dynamicConfigItems` 定义运行期间允许覆盖的 15 条 Go 风格字段路径白名单；`MergeConfigItems`/`mergeConfigItems` 只应用白名单内且实际发生变化的叶子值，其他变化只报告、不写入（`pkg/config/config_util.rs:56-74,86-147`）。
- `goFieldName` 把 serde 产生的 kebab-case/snake_case 键还原成合并白名单使用的 Go 导出字段名，并处理 `tikv` 与慢日志阈值两个非普通大小写映射（`pkg/config/config_util.rs:149-172`）。
- `FlattenConfigItems`/`flatten` 把任意深度 JSON Object 展开成点号路径；数组和所有标量作为不可继续展开的叶子整体保留（`pkg/config/config_util.rs:185-222`）。
- `ConfReloadFunc` 固定配置重载回调的旧/新 `Config` 参数形状；本文件只声明类型，不保存或调用回调（`pkg/config/config_util.rs:174-177`）。
- `GetTxnScopeFromConfig` 将事务作用域查询转发到 crate 内的 `tikvcfg` 门面（`pkg/config/config_util.rs:224-232`）。

## 主要符号

- `pub fn CloneConf<T>(&T) -> Result<T, serde_json::Error>`：对任意同时实现 `Serialize + DeserializeOwned` 的类型做 JSON 深拷贝。它比 Go 的同名函数更泛型，但错误仍来自编码或解码阶段。
- `pub fn dynamicConfigItems() -> HashSet<&'static str>`：每次调用构造新集合，元素是 `Performance.MaxProcs`、`TiKVClient.StoreLimit`、`Instance.SlowThreshold` 等 15 条动态字段路径。它是公开函数而非 Go 的包级私有变量。
- `pub fn MergeConfigItems<T>(&mut T, &T) -> (Vec<String>, Vec<String>)`：公开合并入口。先把同类型的目标值和新值转换成 JSON 树，递归合并后再反序列化回目标对象。
- `pub fn mergeConfigItems(&mut Value, &Value, &str) -> (Vec<String>, Vec<String>)`：公开但主要供 `MergeConfigItems` 使用的递归实现。返回值分别收集已接受与已拒绝的完整字段路径。
- `fn goFieldName(&str) -> String`：私有路径转换函数。按 `-`、`_` 切词并将词首大写；`tikv` 变为 `TiKV`，`tidb_slow_log_threshold` 直接变为 `SlowThreshold`。
- `pub type ConfReloadFunc = fn(&Config, &Config)`：非捕获函数指针别名，不接受闭包环境，也没有错误返回值。
- `pub fn FlattenConfigItems(HashMap<String, Value>) -> HashMap<String, Value>`：消耗输入映射并返回新扁平映射。
- `pub fn flatten(&mut HashMap<String, Value>, Value, &str)`：公开递归辅助函数；Object 下钻，非 Object 写入当前前缀。
- `pub fn GetTxnScopeFromConfig() -> String`：薄转发函数，最终读取全局 `Config.labels["zone"]`，缺失时返回 `"global"`（`pkg/config/lib.rs:27-34`）。

本文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项。

## 执行流程

深拷贝流程：`CloneConf` 调用 `serde_json::to_vec` 生成独立字节表示，再由 `serde_json::from_slice` 构造新的拥有所有权的 `T`；任一步失败立即通过 `?` 返回。独立测试修改克隆体中的字符串、端口、向量和原子布尔后确认原对象不变（`pkg/config/config_util_test.rs::test_clone_conf`、`pkg/config/config_util_1_aster_unit_test.rs::clone_conf_is_a_deep_json_clone`）。

动态合并流程：

1. `MergeConfigItems` 将 `dst_conf` 与 `new_conf` 分别序列化成 `serde_json::Value`。
2. `mergeConfigItems` 先比较当前两棵子树；完全相同则立即返回两个空列表，不报告未变化字段。
3. 两边都是 Object 时，只遍历目标对象已有的键；新对象缺少同名键时跳过。每下钻一层用 `goFieldName` 生成 Go 风格路径并用 `.` 拼接。
4. 只要任一侧不是 Object，当前节点就按叶子处理。路径在 `dynamicConfigItems` 中则克隆新值到目标树并加入 `accepted`，否则保留旧值并加入 `rejected`。
5. 子结果按目标 Object 的遍历次序追加，随后整个 JSON 树反序列化回 `dst_conf`。

展平流程：`FlattenConfigItems` 先把输入 `HashMap` 转成 JSON Object，再以空前缀调用 `flatten`。递归遇到 Object 时枚举键并拼出 `父.子` 路径；遇到字符串、数字、布尔、null 或数组时，将完整值插入输出映射。测试中的 `k4.k4-1` 会被展开，而对象数组 `k3` 作为一个数组保留（`pkg/config/config_util_test.rs::test_flatten_config`）。

事务作用域流程：`pkg/kv/txn_scope_var.rs::NewDefaultTxnScopeVar` 调用本文件的 `GetTxnScopeFromConfig`，后者转发到 `pkg/config/lib.rs::tikvcfg::GetTxnScopeFromConfig`；该函数获取 `config.rs` 中受 `RwLock<Arc<Config>>` 保护的全局快照，读取 `labels["zone"]`。存在 zone 时用其构造对外显示为 `local` 的 `TxnScopeVar`，否则使用 `global`（`pkg/kv/txn_scope_var.rs:34-43`）。

## 数据与状态

本文件自身没有持久全局状态。白名单由 `dynamicConfigItems` 每次重新创建；合并和展平使用函数局部的 `serde_json::Value`、`Vec<String>` 与 `HashMap<String, Value>`。因此调用之间不会共享 accepted/rejected 结果或扁平映射。

`MergeConfigItems` 的路径契约来自序列化键而不是 Rust 字段名本身。`Config` 与多数分区使用 `#[serde(rename_all = "kebab-case")]`，`Instance` 使用 snake_case，故 `goFieldName` 是 serde 表示与 Go 白名单之间的桥。`Instance.slow_threshold` 被显式序列化为 `tidb_slow_log_threshold`（`pkg/config/config.rs:629-635`），需要特例才能命中 `Instance.SlowThreshold`。`Config.txn_local_latches` 带 `#[serde(skip)]`（`pkg/config/config.rs:1079-1082`），所以尽管白名单含 `TxnLocalLatches.Capacity`，当前 JSON 合并树中不会出现该字段；该白名单项在当前 Rust 实现中无法由 `MergeConfigItems<Config>` 更新。

动态合并并不保证遍历结果顺序是 API 契约：结果取决于 `serde_json::Map` 的对象键迭代顺序。现有测试只检查数量、成员归属或排序后的明确静态路径，没有依赖 accepted 的原始顺序。

事务作用域所读的状态位于 `pkg/config/config.rs::GLOBAL_CONFIG`，本文件不拥有该锁和快照。`get_global_config` 返回 `Arc<Config>` 克隆，因此读取期间使用一致快照（`pkg/config/config.rs:1732-1747`）。

## 依赖与调用关系

直接语言依赖为 `serde::{Serialize, de::DeserializeOwned}`、`serde_json::{Value, Error}` 以及标准库 `HashMap`/`HashSet`。`pkg/config/Cargo.toml:10-21` 声明了 `serde` 的 derive feature、`serde_json` 和 `toml` 等 crate；本文件直接使用前两者，不直接解析 TOML。`Config` 与 `tikvcfg` 通过 `super` 从 crate 根取得。

RustCodeGraph 和限定范围源码检索确认的内部调用边为：

- `MergeConfigItems -> mergeConfigItems`；
- `mergeConfigItems -> mergeConfigItems`（递归）、`dynamicConfigItems`、`goFieldName`；
- `FlattenConfigItems -> flatten`，`flatten -> flatten`（递归）；
- `GetTxnScopeFromConfig -> tikvcfg::GetTxnScopeFromConfig -> get_global_config`；
- `pkg/kv/txn_scope_var.rs::NewDefaultTxnScopeVar -> GetTxnScopeFromConfig`。

除测试外，仓库中没有找到 Rust 对 `CloneConf`、`MergeConfigItems`、`FlattenConfigItems` 或 `ConfReloadFunc` 的直接调用。测试上游是 `pkg/config/config_util_test.rs` 与 `pkg/config/config_util_1_aster_unit_test.rs`；`pkg/config/lib.rs:61-72` 以独立 `#[cfg(test)]` 模块挂载它们。`pkg/executor/test/distsqltest/distsql_test.rs` 还验证 zone label 能经 `GetTxnScopeFromConfig` 返回。

## 错误处理与边界

`CloneConf` 是本文件唯一把可恢复错误返回调用者的入口：序列化和反序列化错误都保留为 `serde_json::Error`。相反，`MergeConfigItems` 对初始序列化和最终反序列化使用 `expect`；即使泛型约束满足，用户自定义 `Serialize`/`Deserialize` 实现仍可能报错，因此该泛型 API 存在 panic 边界。其“同类型值合并后必可反序列化”假设对普通 `Config` 成立，但不是由类型系统严格证明。

合并只遍历目标 Object 的键：新值独有的键被忽略；目标独有的键若新值缺失也被跳过。Object 与非 Object 类型不匹配时整体按叶子判断；若恰好白名单命中，会用不同形状的新值替换，最终可能在反序列化时 panic。公开的 `mergeConfigItems` 允许调用者传入任意路径和值树，不能自动保证路径真实存在于 `Config`。

只要叶子值不同，非白名单路径就进入 `rejected`，包括未知或空路径。根节点直接传入两个不同标量时会报告空字符串。数组不逐元素合并：整个数组是一个叶子，要么整体接受，要么整体拒绝。

`FlattenConfigItems` 使用 `serde_json::to_value(nested_config).unwrap_or_default()`。标准 `HashMap<String, Value>` 正常可序列化；若未来改变输入类型或自定义序列化行为，失败会静默退化成 `Value::Null`，随后 `flatten` 会以空字符串为键写入 null。空 Object 则不产生任何输出；对象中空 Object 同样没有叶子条目。重复点号路径通常不会由树形输入产生，但原始键本身含 `.` 时可能与嵌套路径碰撞，后插入值会覆盖先前值。

`GetTxnScopeFromConfig` 不返回错误；锁中毒会在 `get_global_config` 的 `expect` 处 panic，缺少 zone label 则明确回退到 `global`。

## 并发与资源生命周期

深拷贝、合并和展平均为同步、调用栈内执行，不启动线程、异步任务或通道，也不持有文件、网络连接或事务。临时 JSON 字节、值树、集合和字符串在函数返回后按 Rust 所有权规则释放。递归深度与输入配置/映射嵌套深度一致；极端不可信深度可能增加栈使用，但正常 `Config` 层级固定且较浅。

这些工具不在原对象上长期持锁。`MergeConfigItems` 接受独占 `&mut T`，同一目标不能被多个安全 Rust 调用并发修改；`new_conf` 只读借用。`CloneConf` 和 `FlattenConfigItems` 创建独立所有权结果。`ConfReloadFunc` 只是函数指针类型，没有注册表、调用顺序或生命周期管理。

事务作用域读取的并发保证来自下游 `GLOBAL_CONFIG: RwLock<Arc<Config>>`：读取仅在克隆 `Arc` 时持读锁，随后锁已释放；本文件返回拥有所有权的 `String`，不把配置快照借用泄漏给调用方。

## 与 Go 版本的对应关系

`pkg/config/config_util.go` 是直接对照。Rust 保留了 Go 的符号名称、15 项动态白名单、accepted/rejected 含义、Object/结构体递归、数组不展平、回调参数含义和 TiKV 事务作用域转发。

主要语言映射与迁移差异如下：

- Go `CloneConf` 只接受 `*Config` 并返回 `(*Config, error)`；Rust 版本对任意 serde 可往返的 `T` 泛型化，返回拥有所有权的 `T`。
- Go `dynamicConfigItems` 是一次初始化的包级 map；Rust 是每次调用分配的公开函数。高频叶子合并会反复构造相同集合。
- Go `mergeConfigItems` 用反射读取真实结构体字段名，显式解引用指针并对 `AtomicBool` 特判；Rust 把值转换成 JSON 树，不需要反射，但依赖 serde 键名转换。被 `#[serde(skip)]` 的字段不会参与 Rust 合并，这是与 Go 的实质差异。
- Go 合并入口固定为 `*Config` 且递归函数私有；Rust 两者都公开且泛型/JSON 化，暴露了更宽的输入面和 panic 边界。
- Go 的 `map[string]any` 与 Rust 的 `HashMap<String, Value>` 展平语义一致：只递归 map/Object，数组整体保留。
- Go `ConfReloadFunc` 可表示普通函数值；Rust `fn` 别名只能表示函数项或不捕获闭包，不能保存捕获环境。
- Go `tikvcfg` 来自 `github.com/tikv/client-go/v2/config`；Rust 当前 `tikvcfg` 是 `pkg/config/lib.rs` 内部精简实现，从 AsterSQL 全局配置的 `labels["zone"]` 推导 scope。

Go 测试 `pkg/config/config_util_test.go` 与两份 Rust 独立测试覆盖深拷贝、动态/静态字段和数组展平。值得注意的是 Go 测试注释把 `Instance.SlowThreshold` 放在 rejected 区域，但白名单包含该路径且断言期望 6 个 accepted；Rust 测试明确把它作为第六个动态项，并验证值已更新，符合实际 Go 代码与最终断言。

## 扩展指南

新增或删除动态配置项时，应同步检查 `dynamicConfigItems`、对应 `Config`/分区字段的 serde 名称及 `goFieldName`。若字段被 `#[serde(skip)]`，不能只加入白名单；必须先决定它是否应进入序列化合并表示，或为该字段设计显式合并路径。新增缩写词（例如不是 `tikv` 的全大写名称）或特殊 rename 时，需要添加明确转换并在独立测试中断言最终 Go 风格路径。

修改合并语义时，优先扩展 `pkg/config/config_util_test.rs`，并保持测试逻辑与 `pkg/config/config_util_test.go` 一致；AsterSQL 专有边界可放在 `pkg/config/config_util_1_aster_unit_test.rs`。应覆盖：未变化短路、动态叶子接受、静态叶子拒绝、数组整体处理、缺失键、serde 特殊名、序列化失败与顺序非契约。测试必须继续与源文件分离。

若把 `FlattenConfigItems` 接入 Rust `SHOW CONFIG`，接线位置应是读取节点配置并完成 JSON 解码之后、隐藏字段过滤和展示格式化之前，对照 `pkg/executor/memtable_reader.go:252-274`；同时在 executor 的独立测试中验证数组、空对象、含点号原始键和敏感项过滤。若允许任意外部键，需先解决点号碰撞和空对象丢失的兼容定义。

若需要带状态的重载回调，应新增适当的 trait object/泛型接口，而不是把捕获闭包强塞入 `ConfReloadFunc`。若要提高动态合并性能，可把白名单改为惰性静态集合或一次构造后传入递归；这会改变公开 API，应先测量配置重载频率并保持 Go 字段路径兼容。

正确性风险集中在 serde 路径与白名单漂移、`expect` panic 和被跳过字段；兼容性风险集中在 accepted/rejected 路径拼写及展平键格式；性能风险主要是整份配置的 JSON 往返、每个变化叶子重建白名单和递归分配路径字符串。

## 验证依据

- RustCodeGraph `status`：索引有效，含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/config` 将 `config_util.rs` 标为含 9 个符号。
- RustCodeGraph `query` 定位了 Rust/Go 的 `CloneConf`、`MergeConfigItems`、`mergeConfigItems`、`FlattenConfigItems`、`flatten`、`GetTxnScopeFromConfig`，并定位两份 Rust 测试入口；图输出确认 `MergeConfigItems -> mergeConfigItems -> dynamicConfigItems/goFieldName` 与 `FlattenConfigItems -> flatten`。精确 `callers`/`callees` 在约 30 秒内产生歧义/超时，因此上游调用由限定 Rust/Go 文件范围的 `rg` 与源码逐行复核补齐。
- 已读 Rust 实现与装配：`pkg/config/config_util.rs`、`pkg/config/lib.rs`、`pkg/config/Cargo.toml`、`pkg/config/config.rs`、`pkg/kv/txn_scope_var.rs`。
- 已读 Go 对照与生产接线：`pkg/config/config_util.go`、`pkg/executor/memtable_reader.go`，并检索了 Go 的事务作用域调用点。
- 已读独立测试：`pkg/config/config_util_test.rs`、`pkg/config/config_util_1_aster_unit_test.rs`、`pkg/config/config_util_test.go`；另核对 `pkg/executor/test/distsqltest/distsql_test.rs` 的 zone label 断言。
- 人工复核结论：本文件存在于配置 crate 的通用转换边界；当前 Rust 生产链只直接使用事务作用域门面，其余能力处于已实现、已单测但未见生产调用的迁移状态。扩展时必须同步 serde 字段表示、Go 风格白名单路径和独立测试。
- 本任务只新增说明文档，按计划不运行 Cargo。固定章节结构验证与交付时的工作区核对另行执行并记录退出码。
