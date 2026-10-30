# `pkg/session/runtime/mview_source.rs` 逻辑说明

## 文件定位

canonical session 与 MV merge/apply sink 的真实数据源、事务 writer 桥接。

## 核心职责

提取定义元数据，生成并优化 SELECT，绑定固定 MVCC 版本的 KV 扫描；提供真实 DML batch、statement staging、MIN/MAX 组键 lookup 与扫描统计。

## 主要符号

`definition`、`compile_mview_source`、`RefreshResources`、`Batch`、`ObservedSource`、`ObservedIterator`。

## 执行流程

从已提交 InfoSchema 和定义 SQL 建立 MViewDefinition；使用 canonical planner 优化派生源，按实际物理 table/index scan 编码范围；读取 typed chunk 后调用现有 merge/apply sink。MIN/MAX lookup 使用同一 read TSO。

## 数据与状态

源扫描保留 Datum 类型、行句柄和 commit timestamp；批写使用 maintenance session 的真实事务；统计计数来自实际 KV 迭代器，不用聚合输出数量代替扫描行数。

## 依赖与调用关系

调用 planner/core/mview_refresh_source、typed_adapter_bridge 的真实 snapshot/range encoder、executor 的现有 delta merge 与 complete apply。

## 错误处理与边界

错误沿 BuildError/SessionError/MutationRuntime 边界返回。statement batch 失败回滚 staging 并恢复 pending stats delta；整个 refresh 的事务 rollback 由协调器负责。时间区与复杂聚合边界仍需验证。

## 并发与资源生命周期

索引统计由同一实际 scan source 的共享 observation 聚合多个 iterator；query_total 按查询记一次，KV request 和消费行数独立累计，最后释放 observation 时写入会话 collector。

ConcreteSession 保持 Rc/RefCell 的线程内使用；snapshot retriever 由 Domain 保持存储生命；迭代器 Close/Drop 只结束一次并上报真实索引访问样本。

## 与 Go 版本的对应关系

compile_mview_source 另从真实物理树记录 explain_id、估算行数、算子类型及 explain_info，供数据变更步骤 observer 消费；该观测不生成模拟计划。

a87bf8db6499ac38075781cef7357709f07ea002 的快照、数据变更与内部统计接线；派生 SELECT 算法来自获授权的前置 34d5c2b75b4f422f43f82c96b97bdf1667305d71。

## 扩展指南

WHERE、nullable 分组、MIN/MAX 覆盖索引回查、有界 FAST 和隐藏行句柄均由真实 snapshot/事务回归验证。复用 canonical schema 与完整聚合描述符，不禁用优化器或放宽类型检查；新增源形态时先验证 canonical 输出顺序。
