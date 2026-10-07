# pkg/meta/model/column.rs

## 文件定位

本文件是表列元数据的 Rust 实现，定义持久化形状 ColumnInfo、默认值兼容表示、DDL 改列/删列期间的临时命名，以及 TiDB 隐式系统列构造器。它不单独形成 crate：pkg/meta/model/internal/group1/lib.rs:446-448 通过 path 属性编译并公开再导出本模块，pkg/meta/model/lib.rs:11-14,37 再由 astersql-meta-model 包根导出。

pkg/meta/model/Cargo.toml 表明顶层包仅聚合 group1–group4，且 package.metadata.porting.go-package 指向 pkg/meta/model。这里使用的 ast、charset、mysql、types、SchemaState、TableInfo 和三个额外列 ID 均来自 group1 父模块。

## 核心职责

- 用 ColumnInfo 保存列 ID、名称、偏移、FieldType、schema 状态、默认值、生成列信息、隐藏标记、改列状态和序列化版本；serde rename 保持 Go JSON 键（column.rs:252-312）。
- 用 DefaultValue 与 go_bytes 适配器保持 Go 标量 JSON 和 BIT 原始字节兼容。
- 将 FieldType 的类型、flag、长度、小数位、字符集、校对规则和元素列表通过列级 Get/Set API 暴露。
- 生成并解析 _Col$_ 与 _Tombstone$_ 生命周期名称，为 DDL 改列和删除提供约定。
- 按名称或 ID 查列，并构造 _tidb_rowid、_tidb_tid、_tidb_commit_ts 三种额外列。

## 主要符号

- ColumnInfoVersion0/1/2、CurrLatestColumnInfoVersion：列元数据版本，当前最新为 2。Go 的 Version 注释说明 0/1 还承担 timestamp 默认值时区兼容（column.go:104-111）。
- DefaultValue：Bool、Int、Uint、Float、String(Vec<u8>) 五种标量。String 普通序列化使用 from_utf8_lossy，任意非 UTF-8 字节不能仅靠该字段保真（column.rs:44-103）。
- go_bytes：私有 serde 适配器。写出 Option<Vec<u8>> 时生成标准 base64；读入接受 null、base64 字符串和旧字节数组（column.rs:106-229）。
- InvalidDefaultError：BIT 收到非字符串、非空默认值时的错误，显示为 Invalid default value for '<column>'。
- ChangeStateInfo：保存 DependencyColumnOffset，描述改列期间的依赖列偏移。
- ColumnInfo：核心结构。serde(default) 允许旧 JSON 缺字段时用 Default 补齐；ChangingFieldType 为 None 时不写出。
- ColumnInfo::New/Clone：前者以 ID、名称和 TypeUnspecified 构造空列；后者执行 Rust 值克隆。
- GetType/GetFlag/GetFlen/GetDecimal/GetCharset/GetCollate/GetElems 及 setter/flag 操作：全部委托 FieldType。
- IsGenerated/IsVirtualGenerated：以生成表达式是否为空及 GeneratedStored 标志判断。
- SetOriginDefaultValue/GetOriginDefaultValue、SetDefaultValue/GetDefaultValue：实现普通列值和 BIT 字节旁路。
- GetTypeDesc：从 FieldType::CompactStr 开始，按类型限制追加 unsigned、zerofill。
- EmptyColumnInfoSize：仅为 ColumnInfo 结构本体的 size_of，不包含容器堆内容。
- GenUniqueChangingColumnName、GenRemovingObjName、FindColumnInfo、FindColumnInfoByID 和三个 NewExtra*ColInfo 是模块级命名、查找与系统列 API。

## 执行流程

1. 建列代码通常先构造 ColumnInfo，再设置 FieldType 和默认值。pkg/ddl/create_table.rs:517-520 是 SetOriginDefaultValue、SetDefaultValue 的生产调用点。
2. 默认值 setter 先写主字段。非 BIT 立即成功；BIT 的 None 成功，String 还复制进对应 *Bit 字段，其他 variant 返回 InvalidDefaultError（column.rs:458-519）。
3. JSON 写出时，普通 String 是 lossy UTF-8 文本，BIT 旁路是 base64；读回后 BIT getter 优先从旁路重建 String。column_test.rs:test_default_value 用 0x19、0xb9 证明新写法保真，旧数据只写主字段时往返不一致。
4. DDL 改列时，GenUniqueChangingColumnName 先收集表中小写列名，再从后缀 0 探测首个无大小写冲突的 _Col$_<old>_<n>。pkg/ddl/persistent_modify_column.rs:547 使用结果；同文件 :873,915 以 GenRemovingObjName 生成幂等墓碑名。
5. Planner 消费额外列：physical_index_scan.rs:564-566 按特殊 ID 构造隐式列或查普通列，base_physical_plan.rs:168-171 确保物理表 ID 列存在。session/runtime/statistics.rs:2488,2559 使用 GetTypeDesc 输出类型。

## 数据与状态

ColumnInfo 是可变元数据快照。State 是 DDL schema 状态；ChangingFieldType 与 ChangeStateInfo 是改列过渡数据；Version 是持久化兼容状态。GeneratedExprString 决定是否为生成列，GeneratedStored 区分 STORED/VIRTUAL，Dependences 保存生成列依赖名集合。

默认值各有“逻辑值”和“BIT 原始字节”字段。成功写入非空 BIT String 后，旁路与输入字节相同，getter 优先旁路。setter 对 None 或非法 variant 不清除旧旁路，并在校验前写主字段，因此错误不会回滚对象状态；column_test.rs:77-86 明确验证首次写非法整数后主字段仍保存该整数。若对象已有旁路，后续设置时更要考虑 getter 的旁路优先级。

Clone 会克隆 Rust 拥有的 String、Vec、HashMap 等容器。FindColumnInfo* 只返回输入 slice 元素的不可变借用，不复制或修改数据。

## 依赖与调用关系

向下依赖中，types::FieldType 提供类型存储和 CompactStr；mysql 提供类型码、flag 与 BIGINT 默认长度/小数位；ast::CIStr 保存原始名 O 和小写名 L；charset 提供 binary 字符集/校对规则；serde 承载 Go 兼容 JSON；TableInfo 只被临时名去重读取。

已核实的向上 Rust 调用包括：

- DDL：pkg/ddl/create_table.rs 写默认值，persistent_drop_column.rs:239 写 origin default，persistent_modify_column.rs 生成 changing/tombstone 名。
- Planner：physical_index_scan.rs 与 base_physical_plan.rs 查找或构造隐式列；logical_plan_builder_runtime.rs:4847,4869,4964 注入 row handle 和 commit-ts。
- Table/session/util：pkg/table/column.rs:150,942、session/runtime/statistics.rs、session/runtime/system_query.rs:2520 和 util/regionsplit/model_handle.rs:44 消费额外列或类型描述。

RustCodeGraph 将目标文件标为被 68 个文件使用。因为 ColumnInfo、Clone、GetType 等名字在 Go/Rust 中大量重名，本说明只采用带文件路径的 Rust 直接命中，不把同名 Go 边当成 Rust 调用。

## 错误处理与边界

- 仅两个默认值 setter 显式返回 Result。BIT 只接受 None 或 String；InvalidDefaultError 无专用错误码/堆栈，上层会映射或字符串化，如 persistent_drop_column.rs:239。
- setter 不事务性回滚，也不主动清除旧 *Bit；改变顺序会改变 Go 已测试语义。
- DefaultValue::String 的普通 JSON 是 lossy 转换；BIT 非 UTF-8 值必须经 setter 写旁路。go_bytes 对非法 base64 长度/字符返回 serde 错误，同时兼容旧数组形状。
- GetChangingOriginName 去前缀后按最后一个下划线切后缀，但不验证后缀是否为数字；无下划线时返回剩余全名。
- GenUniqueChangingColumnName 构建 O(n) 名称集合后逐个探测后缀；正常有限列集下必能返回。候选名按小写比较。
- FindColumnInfo 对输入执行 Unicode to_lowercase 后与 CIStr.L 比较；两种查找均返回第一个命中或 None，不诊断重复数据。
- GetTypeDesc 不给 BIT/YEAR 追加 unsigned，也不给 YEAR 追加 zerofill，这是兼容分支而非遗漏。

## 并发与资源生命周期

本文件没有锁、atomic、通道、异步任务、I/O 或事务。函数均同步执行；更新要求 &mut ColumnInfo，跨线程共享与 schema 快照发布由上层负责。

字符串、字节、依赖集合和 FieldType 由 ColumnInfo 拥有并由 RAII 释放。FindColumnInfo* 的返回借用受输入 slice 生命周期限制；三个 NewExtra* 每次返回独立值。本模块没有全局 ColumnInfo 缓存，也不管理 schema 快照生命周期。

## 与 Go 版本的对应关系

权威对照是 pkg/meta/model/column.go，回归对照是 column_test.go:TestDefaultValue。Rust 保留版本常量、JSON 键、临时命名规则、FieldType 转发、生成列判定、BIT 旁路、类型描述、查找及三种额外列的主语义。column_test.rs:test_default_value 按 Go 用例覆盖普通/BIT 新旧写法、空 origin default、JSON 往返和物理表 ID 列。

差异包括：

- Go 用 any 表示默认值，Rust 用封闭 DefaultValue enum；其他动态类型不能直接装入 Rust 结构。
- Go []byte JSON 天然为 base64；Rust 用 go_bytes 模拟并额外兼容历史数组。
- Go Clone 对非空指针做结构浅拷贝并可对 nil 返回 nil；Rust Clone(&self) -> Self 无 null receiver，且克隆拥有容器。
- Go 嵌入 *ChangeStateInfo，Rust 用具名 Option<ChangeStateInfo>；Go 列集合是指针 slice，Rust 是值 slice 并返回借用。
- Rust 增加 ColumnInfo::New 统一默认构造；额外列 ID、名称、flag、BIGINT 宽度及 binary charset/collation 与 Go 对齐。
- Rust 主 String JSON 的 lossy UTF-8 是具体实现差异；BIT 可观察的原始字节合约依靠旁路维持。

## 扩展指南

- 新增持久化字段时同步检查 ColumnInfo::New/Default、serde 键与 default/skip 策略、Go JSON 形状和旧数据缺字段行为。调整 Version 必须评估新旧节点混部。
- 扩展默认值类型时修改 DefaultValue 及 Serialize/Deserialize visitor，并在独立 pkg/meta/model/column_test.rs 增加 JSON 往返、旧形状和非 UTF-8 边界；测试不要内嵌生产文件。
- 修改 BIT setter 时必须保留或明确迁移“先写主字段再校验”、旁路 getter 优先级及空值/旧数据语义；同步对照 Go TestDefaultValue 和 Rust 的三个默认值测试。
- 修改临时命名时同步检查 IsChanging、IsRemoving、两个 origin-name getter 与 persistent_modify_column.rs；bdr_1_aster_unit_test.rs:64-66 已覆盖大小写冲突和墓碑幂等。
- 修改额外列时检查 planner/table/regionsplit 特殊 ID 分支，保持 handle 的 PK+NOT NULL、physical table ID 的 NOT NULL、commit TS 的 UNSIGNED，以及三者的 BIGINT/binary 属性。
- FindColumnInfo* 是线性查找。若性能证据要求索引，应由拥有 TableInfo 快照生命周期的上层维护并处理 DDL 失效，不宜在本模块引入长寿命全局缓存。

## 验证依据

- RustCodeGraph status：11467 文件、307296 节点、1848419 边；node --file pkg/meta/model/column.rs 读取全部 607 行，目标文件显示被 68 个文件使用。
- 编译/导出：pkg/meta/model/Cargo.toml、internal/group1/lib.rs:446-448、pkg/meta/model/lib.rs:11-14,31-37,46-47。
- 生产调用：pkg/ddl/create_table.rs:517-520、persistent_drop_column.rs:239、persistent_modify_column.rs:547,873,915、physical_index_scan.rs:231,242,564-566、base_physical_plan.rs:168-171、logical_plan_builder_runtime.rs:4847,4869,4964、pkg/table/column.rs:150,942、session/runtime/statistics.rs:2488,2559。
- 对照与测试：pkg/meta/model/column.go、column_test.go、column_test.rs、bdr_1_aster_unit_test.rs:49-68，以及 pkg/table/column_test.rs:587-618。
- 本任务是纯文档分析，按计划不运行 Cargo。交付时运行任务指定的 11 标题结构检查和 git diff --check，并人工复核事实与范围。
