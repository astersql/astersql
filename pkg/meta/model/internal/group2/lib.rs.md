# `pkg/meta/model/internal/group2/lib.rs`

## 文件定位

`astersql-meta-model-group2` 的生产入口，提供 DDL 作业参数的 V1/V2 兼容 DTO、共享作业身份和编解码接口。它通过 `include!("../../job_args.rs")` 复用参数实现，并非只有模块装配的骨架。

## 核心职责

保留 Go V1 参数数组和 V2 JSON 对象布局；复用 group3 的真实 Job/JobState/JobVersion，避免参数 crate 与持久化 worker 使用不同作业类型。DTO 为参数视图，不取代根 model 的完整表/分区模型。

## 主要符号

- `data_type!` 生成其他兼容 DTO；本次不更改该宏的全局行为。
- `PartitionDefinition` 和 `PartitionInfo` 接收兼容层原有 PascalCase 字段和 Go canonical 小写字段。分区信息没有 canonical `id` 时，扩展身份字段默认为零。
- `ast` 提供 CIStr、Ident 等参数身份；表 DTO 保存已支持的元数据及其他字段。
- `JobArgsCompat`、参数缓存和共享 Job 桥接 V1/V2 填充、解码与完成态编码。

## 执行流程

调用者从持久化 Job 读取 raw args；`job_args.rs` 按 JobVersion 选择数组解码或 V2 对象/缓存。分区完成态 getter 在读取清理 ID 时仍会解码 PartInfo，因此必须能够识别实际 Go JSON 中的 `definitions`、`id`、`name` 和 `less_than`。DTO 解码不重写原 raw args。

## 数据与状态

V1 完成态分区参数保留物理 ID 和 global-index ID 两项数组；V2 仍可包含原 PartNames、完整 PartInfo 和完成清理字段。两个分区 DTO 的别名只扩展反序列化输入，原兼容 DTO 的序列化字段名保持不变。完整 canonical PartInfo 由调用方保留在 raw args 中，不能用这个精简视图重新编码而丢失 LIST 值、DDL 设置或其他字段。

## 依赖与调用关系

依赖 group3、serde 和 serde_json。根 `astersql-meta-model::group_2` 再导出本 crate，DDL worker、取消/回滚与 delete-range 等路径复用其参数类型。独立测试挂载 `../../job_args_2_aster_unit_test.rs`。

## 错误处理与边界

serde 类型错误及作业参数布局错误仍传播为现有字符串错误；默认值只允许 canonical 分区 JSON 省略兼容 DTO 自有的非必填字段，不执行分区类型、名称、值边界或索引验证。这些语义仍由 DDL 调用方负责。

## 并发与资源生命周期

本模块不启动线程或事务、不执行外部 I/O。作业缓存由可变 Job 引用持有；解码不提交元数据，也不修改持久 raw args。调用方负责事务和作业提交的原子性。

## 与 Go 版本的对应关系

参数实现对应 Go `pkg/meta/model/job_args.go`。来源 `9453792437d1076ebfe7247ac685acd2f5f56925` 的最终化继续保留 TablePartitionArgs.PartInfo/PartNames；Rust 新接线保留这些 canonical 字段后，本模块需要上述最小读取兼容，才能由真实完成态 getter 消费 GC 参数。此修改不扩建完整分区模型或其他 DTO，不声称兼容层精简字段已经覆盖根模型全部能力。

## 扩展指南

按实际 Go JSON 字段为具体 DTO 添加别名，避免全局修改 `data_type!` 影响所有参数。新增输入形式必须同时验证原有形式、V1/V2、完成态 getter 和 raw args 不被改写；完整业务元数据应由根 model 序列化。测试保持在独立文件中。
