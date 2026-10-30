# pkg/ddl/persistent_add_index.rs

## 文件定位

pkg/ddl/persistent_add_index.rs；本来源限定前置适配，不是Go完整子系统复刻。

## 核心职责

限定前置适配：状态None→DeleteOnly→WriteOnly→WriteReorganization；复用ingest_modified_indexes及merge_modified_indexes，完成临时索引合并后Public。rollback删除未发布元数据并用ModifyIndexArgs交给既有delete-range GC。

## 主要符号

step、write；JobExecutionContext、Job、IndexInfo、TableInfo。

## 执行流程

读取真实事务元数据，持久化在线索引schema状态，经既有Backfill DXF导入及临时索引合并后发布；取消进入rollback元数据与既有GC流程。

实际分配的IndexID在首个schema步骤持久化回prepared_index及标准index_args；恢复时按ID和名称共同定位。回滚仅清理本任务已进入schema状态且尚未Public的索引，启动前取消不以SQL预测ID或同名索引证明所有权。

## 数据与状态

prepared_index保存SQL预检后的IndexInfo；实际ID由MaxIndexID分配。job.schema_state与index.State同步，BackfillState分别Running/ReadyToMerge/Merging/Inapplicable。

## 依赖与调用关系

persistent_actions分派，SQL normal_ddl_submit创建任务，SystemSession的ingest_modified_indexes/merge_modified_indexes连接既有DXF和真实存储。

## 错误处理与边界

前置信息缺失返回错误；同名索引取消任务；不将普通索引扩展为主键/唯一/列存实现。未支持的存储能力返回错误，不回退事务导入。

## 并发与资源生命周期

复用正常DDL owner与schema版本屏障；WriteOnly起DML维护临时索引，合并完成后才Public。引擎与worker由既有DXF生命周期清理。

## 与 Go 版本的对应关系

0766a90d525d4e78b2739f170157408e39f1ec87增加NextGen ingest disk-full重试。本文件是用户授权补齐的必要生产链，复用现有组件，不代表完整Go onCreateIndex已复刻。

## 扩展指南

新增能力先复用既有Go-wire与状态机。不要把PostgreSQL或其他索引类型绕入本限定入口；需额外行为回归后再扩大。

## 验证依据

`cargo test -p astersql-session --features nextgen --lib port_0766a90d52_ -- --test-threads=1`：2通过、0忽略。恢复回归覆盖真实SQL、持久化任务Running、retry指标增加、PauseReason为空、并发INSERT/DELETE、容量恢复、Public及索引结果；另一回归验证启动前回滚不能移除同名的既有Public索引，该断言修复前失败。
