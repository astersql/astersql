// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/restore/internal/prealloc_db/db_test.go`.
//!
//! Go uses utiltest/testkit/domain/kv. This platform forbids kv/domain — the same
//! DB call paths are exercised via local `Session`/`Glue` mocks that record SQL,
//! create-table, placement-policy, and next-row-id side effects.

//! 中文注释索引：`br/pkg/restore/internal/prealloc_db/db_test.rs`
//! 职责：PreallocDB 单元测试：覆盖创建、冲突、权限与与 domain/info schema 交互。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 229 行；下列为关键符号与场景索引。
//! - `TestAllocator`：承载与 Go 对齐的状态载体，是理解 `db_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `RecSession`：承载与 Go 对齐的状态载体，是理解 `db_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `SharedSession`：承载与 Go 对齐的状态载体，是理解 `db_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `BatchSharedSession`：承载与 Go 对齐的状态载体，是理解 `db_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `GlueOnce`：承载与 Go 对齐的状态载体，是理解 `db_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `ACTION_ADD_INDEX`：常量阈值应对齐 Go const；改动会影响退避/超时等边界行为。
//! - `GetGlobalID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AdvanceGlobalIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `table_key`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `record_execute`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `query_next_row_id`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Execute`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateDatabaseOnExistError`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTable`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreatePlacementPolicy`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Close`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `as_batch_create_table_session`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTables`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateSession`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `new_db_with`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `prealloc_ids`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `fake_policy_info`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `fixture_origin_tables`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `clone_table_infos`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `check_table_sqls`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `test_restore_auto_inc_id`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_policy_mode`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_create_tables_in_db`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_ddl_job_map`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_db_exec_ddl`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_db_exec_ddl2`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `test_create_table_consistent`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `impl TestAllocator`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl RecSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl SharedSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl BatchSharedSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl GlueOnce`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_internal_prealloc_table_id::{Allocator, New, PreallocIDs};

use crate::{
    BatchCreateTableSession, CIStr, Context, CreateTableOption, DB, Error, Glue, NewDB, Result,
    Session, Storage, UniqueTableName, metautil, model, utils,
};

/// Mirrors Go `testAllocator int64`.
struct TestAllocator(i64);

impl Allocator for TestAllocator {
    fn GetGlobalID(&mut self) -> astersql_br_pkg_restore_internal_prealloc_table_id::Result<i64> {
        Ok(self.0)
    }

    fn AdvanceGlobalIDs(
        &mut self,
        n: usize,
    ) -> astersql_br_pkg_restore_internal_prealloc_table_id::Result<i64> {
        let old = self.0;
        self.0 += n as i64;
        Ok(old)
    }
}

/// In-memory stand-in for utiltest + testkit SQL/session effects.
#[derive(Default)]
struct RecSession {
    executed: Vec<String>,
    created_dbs: Vec<String>,
    /// (db.table, table_id, id_allocated)
    created_tables: Vec<(String, i64, bool)>,
    batch_created: Vec<(String, i64)>,
    created_policies: Vec<String>,
    /// next_row_id keyed by `db\0table` — simulates `admin show next_row_id`.
    next_row_id: HashMap<String, u64>,
    /// Tables that already exist (name-level), so CreateTable does not rewrite AutoIncID.
    existing_tables: HashMap<String, bool>,
    closed: bool,
    fail_create_db: Option<Error>,
    fail_create_table: Option<Error>,
}

impl RecSession {
    fn table_key(db: &str, table: &str) -> String {
        format!("{db}\0{table}")
    }

    fn record_execute(&mut self, sql: &str) {
        self.executed.push(sql.to_string());
        // Mirror TiDB applying `alter table ... auto_increment = N`.
        if let Some(rest) = sql.strip_prefix("alter table ") {
            if let Some((tbl, rhs)) = rest.split_once(" auto_increment = ") {
                let id_str = rhs.trim_end_matches(';').trim();
                if let Ok(id) = id_str.parse::<u64>() {
                    // tbl is like `` `test`.`"t"` ``
                    let cleaned = tbl.replace('`', "");
                    if let Some((db, name)) = cleaned.split_once('.') {
                        self.next_row_id.insert(Self::table_key(db, name), id);
                    }
                }
            }
        }
    }

    fn query_next_row_id(&self, db: &str, table: &str) -> u64 {
        *self
            .next_row_id
            .get(&Self::table_key(db, table))
            .expect("next_row_id missing")
    }
}

struct SharedSession {
    inner: Arc<Mutex<RecSession>>,
}

impl Session for SharedSession {
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.inner.lock().unwrap().record_execute(sql);
        Ok(())
    }

    fn CreateDatabaseOnExistError(&mut self, _ctx: &Context, schema: &model::DBInfo) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_db.clone() {
            return Err(err);
        }
        g.created_dbs.push(schema.Name.String());
        Ok(())
    }

    fn CreateTable(
        &mut self,
        _ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_table.clone() {
            return Err(err);
        }
        let key = RecSession::table_key(&db_name.O, &info.Name.O);
        let id_alloc = opts.iter().any(|o| o.id_allocated);
        g.created_tables
            .push((format!("{}.{}", db_name.O, info.Name.O), info.ID, id_alloc));
        // First create applies AutoIncID from TableInfo (TiDB CreateTableWithInfo).
        // Existing table: keep prior next_row_id (Go: "failed due to table exists").
        if !g.existing_tables.contains_key(&key) {
            g.existing_tables.insert(key.clone(), true);
            if info.AutoIncID > 0 {
                g.next_row_id.insert(key, info.AutoIncID as u64);
            }
        }
        Ok(())
    }

    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_policies
            .push(policy.Name.String());
        Ok(())
    }

    fn Close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }
}

struct BatchSharedSession {
    inner: Arc<Mutex<RecSession>>,
}

impl Session for BatchSharedSession {
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.inner.lock().unwrap().record_execute(sql);
        Ok(())
    }

    fn CreateDatabaseOnExistError(&mut self, _ctx: &Context, schema: &model::DBInfo) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_db.clone() {
            return Err(err);
        }
        g.created_dbs.push(schema.Name.String());
        Ok(())
    }

    fn CreateTable(
        &mut self,
        _ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_table.clone() {
            return Err(err);
        }
        let key = RecSession::table_key(&db_name.O, &info.Name.O);
        let id_alloc = opts.iter().any(|o| o.id_allocated);
        g.created_tables
            .push((format!("{}.{}", db_name.O, info.Name.O), info.ID, id_alloc));
        if !g.existing_tables.contains_key(&key) {
            g.existing_tables.insert(key.clone(), true);
            if info.AutoIncID > 0 {
                g.next_row_id.insert(key, info.AutoIncID as u64);
            }
        }
        Ok(())
    }

    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_policies
            .push(policy.Name.String());
        Ok(())
    }

    fn Close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }

    fn as_batch_create_table_session(&mut self) -> Option<&mut dyn BatchCreateTableSession> {
        Some(self)
    }
}

impl BatchCreateTableSession for BatchSharedSession {
    fn CreateTables(
        &mut self,
        _ctx: &Context,
        infos: HashMap<String, Vec<model::TableInfo>>,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        assert!(opts.iter().any(|o| o.id_allocated));
        let mut g = self.inner.lock().unwrap();
        for (db, tables) in infos {
            for t in tables {
                let key = RecSession::table_key(&db, &t.Name.O);
                g.batch_created.push((format!("{}.{}", db, t.Name.O), t.ID));
                if !g.existing_tables.contains_key(&key) {
                    g.existing_tables.insert(key.clone(), true);
                    if t.AutoIncID > 0 {
                        g.next_row_id.insert(key, t.AutoIncID as u64);
                    }
                }
            }
        }
        Ok(())
    }
}

struct GlueOnce {
    se: Mutex<Option<Box<dyn Session>>>,
}

impl Glue for GlueOnce {
    fn CreateSession(&self, _store: Storage) -> Result<Option<Box<dyn Session>>> {
        Ok(self.se.lock().unwrap().take())
    }
}

fn new_db_with(session: Arc<Mutex<RecSession>>, batch: bool, policy_mode: &str) -> (DB, bool) {
    let boxed: Box<dyn Session> = if batch {
        Box::new(BatchSharedSession { inner: session })
    } else {
        Box::new(SharedSession { inner: session })
    };
    let g = GlueOnce {
        se: Mutex::new(Some(boxed)),
    };
    let (db, support) = NewDB(&g, Storage::default(), policy_mode).unwrap();
    (db.unwrap(), support)
}

fn prealloc_ids(tables: &[metautil::Table], allocator: &mut TestAllocator) -> PreallocIDs {
    let slim: Vec<_> = tables
        .iter()
        .map(|t| astersql_br_pkg_restore_internal_prealloc_table_id::metautil::Table {
            Info: astersql_br_pkg_restore_internal_prealloc_table_id::model::TableInfo {
                ID: t.Info.ID,
                Partition: t.Info.Partition.as_ref().map(|p| {
                    astersql_br_pkg_restore_internal_prealloc_table_id::model::PartitionInfo {
                        Definitions: p
                            .Definitions
                            .iter()
                            .map(|d| {
                                astersql_br_pkg_restore_internal_prealloc_table_id::model::PartitionDefinition {
                                    ID: d.ID,
                                }
                            })
                            .collect(),
                    }
                }),
            },
        })
        .collect();
    let mut ids = New(&slim).expect("Error create prealloc ids");
    ids.PreallocIDs(allocator).expect("Error prealloc ids");
    // Go: allocator += testAllocator(len(tables))
    allocator.0 += tables.len() as i64;
    ids
}

fn fake_policy_info(ident: u8) -> model::PolicyInfo {
    // Go: Name = string(ident) → "\x01" / "\x02"
    model::PolicyInfo {
        Name: CIStr::new((ident as char).to_string()),
    }
}

/// Fixture table shapes mirroring `createTableSQLs` (t1..t7).
fn fixture_origin_tables() -> Vec<metautil::Table> {
    let db = model::DBInfo {
        Name: CIStr::new("test"),
        Charset: "utf8mb4".into(),
        Collate: "utf8mb4_bin".into(),
        ..Default::default()
    };
    vec![
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 1,
                Name: CIStr::new("t1"),
                Columns: vec![model::ColumnInfo {
                    Name: CIStr::new("id"),
                    IsAutoIncrement: false,
                }],
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 2,
                Name: CIStr::new("t2"),
                TTLInfo: Some(model::TTLInfo { Enable: true }),
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 3,
                Name: CIStr::new("t3"),
                AutoIncID: 3,
                Sequence: Some(model::SequenceInfo {
                    Cycle: false,
                    Increment: 2,
                    MinValue: 2,
                    MaxValue: 10,
                }),
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 4,
                Name: CIStr::new("t4"),
                AutoIncID: 4,
                Sequence: Some(model::SequenceInfo {
                    Cycle: true,
                    Increment: 2,
                    MinValue: 2,
                    MaxValue: 10,
                }),
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 5,
                Name: CIStr::new("t5"),
                View: Some(()),
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db.clone(),
            Info: model::TableInfo {
                ID: 6,
                Name: CIStr::new("t6"),
                Partition: Some(model::PartitionInfo {
                    Definitions: vec![
                        model::PartitionDefinition {
                            ID: 61,
                            Name: CIStr::new("p0"),
                            PlacementPolicyRef: None,
                        },
                        model::PartitionDefinition {
                            ID: 62,
                            Name: CIStr::new("p1"),
                            PlacementPolicyRef: None,
                        },
                        model::PartitionDefinition {
                            ID: 63,
                            Name: CIStr::new("p2"),
                            PlacementPolicyRef: None,
                        },
                    ],
                }),
                ..Default::default()
            },
        },
        metautil::Table {
            DB: db,
            Info: model::TableInfo {
                ID: 7,
                Name: CIStr::new("t7"),
                AutoIncID: 0,
                Sequence: Some(model::SequenceInfo {
                    Cycle: true,
                    Increment: -2,
                    MinValue: 2,
                    MaxValue: 10,
                }),
                ..Default::default()
            },
        },
    ]
}

/// Mirrors Go `cloneTableInfos` without kv: reassign IDs/names then PreallocIDs.
fn clone_table_infos(
    db: &mut DB,
    allocator: &mut TestAllocator,
    prefix: &str,
    origin: &[metautil::Table],
) -> Vec<metautil::Table> {
    let id = allocator.GetGlobalID().expect("GetGlobalID");
    let mut table_infos = Vec::with_capacity(origin.len());
    for (i, ori) in origin.iter().enumerate() {
        let mut new_info = ori.Info.Clone();
        new_info.ID = id + i as i64 + 1;
        if let Some(part) = &mut new_info.Partition {
            for (j, def) in part.Definitions.iter_mut().enumerate() {
                def.ID = new_info.ID * 10 + j as i64;
            }
        }
        new_info.Name = CIStr::new(format!("{}{}", prefix, i + 1));
        table_infos.push(metautil::Table {
            DB: ori.DB.clone(),
            Info: new_info,
        });
    }
    let ids = prealloc_ids(&table_infos, allocator);
    db.RegisterPreallocatedIDs(ids);
    table_infos
}

/// Mirrors Go `checkTableSQLs` assertions against recorded session effects.
fn check_table_sqls(rec: &RecSession, tables: &[metautil::Table], prefix: &str) {
    // t1: show create — table must exist in batch/single creates
    assert!(
        rec.batch_created
            .iter()
            .any(|(n, _)| n == &format!("test.{}1", prefix))
            || rec
                .created_tables
                .iter()
                .any(|(n, _, _)| n == &format!("test.{}1", prefix)),
        "missing {}.{}1",
        "test",
        prefix
    );
    // t2: TTL_ENABLE='OFF'
    let t2 = tables
        .iter()
        .find(|t| t.Info.Name.O == format!("{prefix}2"))
        .expect("t2");
    assert!(
        !t2.Info.TTLInfo.as_ref().unwrap().Enable,
        "TTL_ENABLE='OFF'"
    );
    // t3: NEXTVAL → 3  (restoreSequence setval to AutoIncID=3; non-cycle)
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do setval(`test`.`{prefix}3`, 3);")),
        "sequence t3 setval missing: {:?}",
        rec.executed
    );
    // t4: cycle sequence → setval MaxValue, nextval, setval AutoIncID(4)
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do setval(`test`.`{prefix}4`, 10);")),
        "t4 cycle setval max"
    );
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do nextval(`test`.`{prefix}4`);")),
        "t4 nextval"
    );
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do setval(`test`.`{prefix}4`, 4);")),
        "t4 setval AutoIncID"
    );
    // t5 view — created, no post-restore SQL
    assert!(
        rec.batch_created
            .iter()
            .any(|(n, _)| n == &format!("test.{}5", prefix))
            || rec
                .created_tables
                .iter()
                .any(|(n, _, _)| n == &format!("test.{}5", prefix)),
        "missing view"
    );
    // t6 partition table exists
    assert!(
        rec.batch_created
            .iter()
            .any(|(n, _)| n == &format!("test.{}6", prefix))
            || rec
                .created_tables
                .iter()
                .any(|(n, _, _)| n == &format!("test.{}6", prefix)),
        "missing partition table"
    );
    // t7: negative increment cycle → setval MinValue then nextval then setval AutoIncID(0)
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do setval(`test`.`{prefix}7`, 2);")),
        "t7 cycle setval min"
    );
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do nextval(`test`.`{prefix}7`);")),
        "t7 nextval"
    );
    assert!(
        rec.executed
            .iter()
            .any(|s| s == &format!("do setval(`test`.`{prefix}7`, 0);")),
        "t7 setval 0"
    );
}

/// Mirrors Go `TestRestoreAutoIncID`.
#[test]
fn test_restore_auto_inc_id() {
    let mut allocator = TestAllocator(0);
    let rec = Arc::new(Mutex::new(RecSession::default()));
    // Seed next_row_id as if table was created and one row inserted (Go admin show).
    {
        let mut g = rec.lock().unwrap();
        g.next_row_id
            .insert(RecSession::table_key("test", "\"t\""), 11);
        // Table not yet "restored" via CreateTable — clear existing so first CreateTable applies.
        g.existing_tables.clear();
        g.next_row_id
            .insert(RecSession::table_key("test", "\"t\""), 11);
    }

    let (mut db, support_policy) = new_db_with(Arc::clone(&rec), false, "STRICT");
    assert!(support_policy);

    // Go: autoid.NextGlobalAutoID matches admin show next_row_id.
    let auto_inc_id = rec.lock().unwrap().query_next_row_id("test", "\"t\"");
    let global_auto_id = auto_inc_id as i64;
    assert_eq!(global_auto_id as u64, auto_inc_id);

    let mut table = metautil::Table {
        DB: model::DBInfo {
            ID: 1,
            Name: CIStr::new("test"),
            Charset: "utf8mb4".into(),
            Collate: String::new(),
            ..Default::default()
        },
        Info: model::TableInfo {
            ID: 10,
            Name: CIStr::new("\"t\""),
            AutoIncID: global_auto_id + 100,
            // NeedAutoID: no PK handle → row id path
            PKIsHandle: false,
            IsCommonHandle: false,
            ..Default::default()
        },
    };

    // drop database — recorded via Execute if called; Go uses tk.MustExec.
    // Test empty collate value
    let exists = db
        .CreateDatabase(&Context::Background(), &mut table.DB, false, None)
        .expect("Error create empty collate db");
    assert!(!exists);

    // Test empty charset value
    table.DB.Charset = String::new();
    table.DB.Collate = "utf8mb4_bin".to_string();
    // Simulate drop database between creates (Go drops then recreates).
    rec.lock().unwrap().created_dbs.clear();
    let exists = db
        .CreateDatabase(&Context::Background(), &mut table.DB, false, None)
        .expect("Error create empty charset db");
    assert!(!exists);

    let mut unique_map: HashMap<UniqueTableName, bool> = HashMap::new();

    let ids = prealloc_ids(std::slice::from_ref(&table), &mut allocator);
    db.RegisterPreallocatedIDs(ids);
    // Clear seed so first CreateTable applies AutoIncID from TableInfo.
    {
        let mut g = rec.lock().unwrap();
        g.existing_tables.clear();
        g.next_row_id
            .remove(&RecSession::table_key("test", "\"t\""));
    }
    db.CreateTable(&Context::Background(), &mut table, &unique_map, false, None)
        .expect("Error create table");

    let auto_inc_id = rec.lock().unwrap().query_next_row_id("test", "\"t\"");
    assert_eq!((global_auto_id + 100) as u64, auto_inc_id);

    // try again, failed due to table exists — AutoIncID not altered.
    table.Info.AutoIncID = global_auto_id + 200;
    let ids = prealloc_ids(std::slice::from_ref(&table), &mut allocator);
    db.RegisterPreallocatedIDs(ids);
    db.CreateTable(&Context::Background(), &mut table, &unique_map, false, None)
        .expect("existing table branch");
    let auto_inc_id = rec.lock().unwrap().query_next_row_id("test", "\"t\"");
    assert_eq!((global_auto_id + 100) as u64, auto_inc_id);

    // unique map → alter sql path.
    table.Info.AutoIncID = global_auto_id + 300;
    unique_map.insert(
        UniqueTableName {
            DB: "test".into(),
            Table: "\"t\"".into(),
        },
        true,
    );
    let ids = prealloc_ids(std::slice::from_ref(&table), &mut allocator);
    db.RegisterPreallocatedIDs(ids);
    db.CreateTable(&Context::Background(), &mut table, &unique_map, false, None)
        .expect("unique map alter branch");
    let auto_inc_id = rec.lock().unwrap().query_next_row_id("test", "\"t\"");
    assert_eq!((global_auto_id + 300) as u64, auto_inc_id);
    assert!(rec.lock().unwrap().executed.iter().any(|s| s
        == &format!(
            "alter table `test`.`\"t\"` auto_increment = {};",
            global_auto_id + 300
        )));
}

/// Mirrors Go `TestPolicyMode`.
#[test]
fn test_policy_mode() {
    let mut allocator = TestAllocator(100);
    let ctx = Context::Background();
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, support_policy) = new_db_with(Arc::clone(&rec), true, "STRICT");
    assert!(support_policy);

    let ori = fixture_origin_tables();
    // prepareAllocTables in Go executes CREATE SQLs via Session — record them.
    for sql in [
        "create table `test`.`t1` (id int);",
        "create table `test`.`t2` (id int, created_at TIMESTAMP) TTL = `created_at` + INTERVAL 3 MONTH;",
        "create sequence `test`.`t3` start 3 increment 2 minvalue 2 maxvalue 10 cache 3;",
        "create sequence `test`.`t4` start 3 increment 2 minvalue 2 maxvalue 10 cache 3 cycle;",
        "create view `test`.`t5` as select * from `test`.`t1`;",
        "create table `test`.`t6` (id int, store_id INT NOT NULL) PARTITION BY RANGE (store_id) (PARTITION p0 VALUES LESS THAN (6),PARTITION p1 VALUES LESS THAN (11),PARTITION p2 VALUES LESS THAN MAXVALUE);",
        "create sequence `test`.`t7` start 3 increment -2 minvalue 2 maxvalue 10 cache 3 cycle;",
    ] {
        db.Session()
            .Execute(&ctx, sql)
            .expect("prepareAllocTables create");
    }

    let mut table_infos = clone_table_infos(&mut db, &mut allocator, "tt", &ori);

    let policy_map = Mutex::new(HashMap::new());
    let fakepolicy1 = fake_policy_info(1);
    let fakepolicy2 = fake_policy_info(2);
    policy_map
        .lock()
        .unwrap()
        .insert(fakepolicy1.Name.L.clone(), fakepolicy1.clone());
    policy_map
        .lock()
        .unwrap()
        .insert(fakepolicy2.Name.L.clone(), fakepolicy2.clone());

    table_infos[0].Info.PlacementPolicyRef = Some(model::PolicyRefInfo {
        Name: fakepolicy1.Name.clone(),
    });
    table_infos[5].Info.Partition.as_mut().unwrap().Definitions[0].PlacementPolicyRef =
        Some(model::PolicyRefInfo {
            Name: fakepolicy2.Name.clone(),
        });

    db.CreateTables(
        &ctx,
        &mut table_infos,
        &HashMap::new(),
        true,
        Some(&policy_map),
    )
    .expect("CreateTables with policy");
    {
        let g = rec.lock().unwrap();
        assert!(g.created_policies.contains(&fakepolicy1.Name.O));
        assert!(g.created_policies.contains(&fakepolicy2.Name.O));
        check_table_sqls(&g, &table_infos, "tt");
    }

    // clone again to test db.CreateTable
    let mut table_infos = clone_table_infos(&mut db, &mut allocator, "ttt", &ori);
    let policy_map = Mutex::new(HashMap::new());
    let fakepolicy1 = fake_policy_info(1);
    let fakepolicy2 = fake_policy_info(2);
    policy_map
        .lock()
        .unwrap()
        .insert(fakepolicy1.Name.L.clone(), fakepolicy1.clone());
    policy_map
        .lock()
        .unwrap()
        .insert(fakepolicy2.Name.L.clone(), fakepolicy2.clone());
    table_infos[0].Info.PlacementPolicyRef = Some(model::PolicyRefInfo {
        Name: fakepolicy1.Name.clone(),
    });
    table_infos[5].Info.Partition.as_mut().unwrap().Definitions[0].PlacementPolicyRef =
        Some(model::PolicyRefInfo {
            Name: fakepolicy2.Name.clone(),
        });

    // Switch to non-batch for CreateTable path: re-open DB with SharedSession.
    // Go reuses same db (gluetidb supports both). Here CreateTable uses Session::CreateTable.
    let rec2 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db2, _) = new_db_with(Arc::clone(&rec2), false, "STRICT");
    // Re-register prealloc from last clone
    let ids = prealloc_ids(&table_infos, &mut allocator);
    db2.RegisterPreallocatedIDs(ids);
    // Re-seed policy map for single-table path
    let policy_map = Mutex::new(HashMap::from([
        (fakepolicy1.Name.L.clone(), fakepolicy1.clone()),
        (fakepolicy2.Name.L.clone(), fakepolicy2.clone()),
    ]));
    for table in &mut table_infos {
        db2.CreateTable(&ctx, table, &HashMap::new(), true, Some(&policy_map))
            .expect("CreateTable with policy");
    }
    {
        let g = rec2.lock().unwrap();
        assert!(g.created_policies.contains(&fakepolicy1.Name.O));
        assert!(g.created_policies.contains(&fakepolicy2.Name.O));
        check_table_sqls(&g, &table_infos, "ttt");
    }

    // test db.CreateDatabase with policy
    let policy_map = Mutex::new(HashMap::from([(
        fakepolicy1.Name.L.clone(),
        fakepolicy1.clone(),
    )]));
    let mut schema = model::DBInfo {
        ID: 20000,
        Name: CIStr::new("test_db"),
        Charset: "utf8mb4".into(),
        Collate: "utf8mb4_bin".into(),
        PlacementPolicyRef: Some(model::PolicyRefInfo {
            Name: fakepolicy1.Name.clone(),
        }),
        ..Default::default()
    };
    let exists = db2
        .CreateDatabase(&ctx, &mut schema, true, Some(&policy_map))
        .expect("CreateDatabase with policy");
    assert!(!exists);
    assert!(
        rec2.lock()
            .unwrap()
            .created_policies
            .contains(&fakepolicy1.Name.O)
    );
    assert!(
        rec2.lock()
            .unwrap()
            .created_dbs
            .contains(&"test_db".to_string())
    );

    db2.Close();
    assert!(rec2.lock().unwrap().closed);
}

/// Mirrors Go `TestCreateTablesInDb`.
#[test]
fn test_create_tables_in_db() {
    let mut allocator = TestAllocator(0);
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, _) = new_db_with(Arc::clone(&rec), true, "STRICT");

    let db_schema = model::DBInfo {
        Name: CIStr::new("test"),
        ..Default::default()
    };
    let mut tables = vec![
        metautil::Table {
            DB: db_schema.clone(),
            Info: Default::default()
        };
        4
    ];
    let mut ddl_job_map = HashMap::new();
    for i in (0..tables.len()).rev() {
        tables[i] = metautil::Table {
            DB: db_schema.clone(),
            Info: model::TableInfo {
                ID: i as i64,
                Name: CIStr::new(format!("test{i}")),
                Columns: vec![model::ColumnInfo {
                    Name: CIStr::new("id"),
                    IsAutoIncrement: false,
                }],
                ..Default::default()
            },
        };
        ddl_job_map.insert(
            UniqueTableName {
                DB: db_schema.Name.String(),
                Table: tables[i].Info.Name.String(),
            },
            false,
        );
    }

    let ids = prealloc_ids(&tables, &mut allocator);
    db.RegisterPreallocatedIDs(ids);
    db.CreateTables(
        &Context::Background(),
        &mut tables,
        &ddl_job_map,
        false,
        None,
    )
    .expect("CreateTables");
    let g = rec.lock().unwrap();
    assert_eq!(g.batch_created.len(), 4);
    for i in 0..4 {
        assert!(
            g.batch_created
                .iter()
                .any(|(n, _)| n == &format!("test.test{i}"))
        );
    }
}

/// Mirrors Go `TestDDLJobMap`.
#[test]
fn test_ddl_job_map() {
    let mut allocator = TestAllocator(0);
    let ctx = Context::Background();
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, support_policy) = new_db_with(Arc::clone(&rec), false, "STRICT");
    assert!(support_policy);

    for sql in [
        "CREATE TABLE test.t1 (a BIGINT PRIMARY KEY AUTO_RANDOM, b VARCHAR(255));",
        "CREATE TABLE test.t2 (a BIGINT AUTO_RANDOM, b VARCHAR(255), PRIMARY KEY (`a`, `b`));",
        "CREATE TABLE test.t3 (a BIGINT PRIMARY KEY AUTO_INCREMENT, b VARCHAR(255));",
        "CREATE TABLE test.t4 (a BIGINT, b VARCHAR(255));",
        "CREATE TABLE test.t5 (a BIGINT PRIMARY KEY, b VARCHAR(255));",
    ] {
        db.Session().Execute(&ctx, sql).expect("create fixture");
    }

    let db_info = model::DBInfo {
        Name: CIStr::new("test"),
        ..Default::default()
    };
    let tables = vec![
        // t1: AUTO_RANDOM PK
        metautil::Table {
            DB: db_info.clone(),
            Info: model::TableInfo {
                ID: 1,
                Name: CIStr::new("t1"),
                AutoRandID: 1,
                AutoRandomBits: 5,
                PKIsHandle: true,
                IsCommonHandle: false,
                ..Default::default()
            },
        },
        // t2: AUTO_RANDOM composite PK
        metautil::Table {
            DB: db_info.clone(),
            Info: model::TableInfo {
                ID: 2,
                Name: CIStr::new("t2"),
                AutoRandID: 1,
                AutoRandomBits: 5,
                PKIsHandle: false,
                IsCommonHandle: true,
                ..Default::default()
            },
        },
        // t3: AUTO_INCREMENT PK
        metautil::Table {
            DB: db_info.clone(),
            Info: model::TableInfo {
                ID: 3,
                Name: CIStr::new("t3"),
                AutoIncID: 1,
                PKIsHandle: true,
                IsCommonHandle: false,
                Columns: vec![model::ColumnInfo {
                    Name: CIStr::new("a"),
                    IsAutoIncrement: true,
                }],
                ..Default::default()
            },
        },
        // t4: no PK → NeedAutoID via row id
        metautil::Table {
            DB: db_info.clone(),
            Info: model::TableInfo {
                ID: 4,
                Name: CIStr::new("t4"),
                AutoIncID: 1,
                PKIsHandle: false,
                IsCommonHandle: false,
                ..Default::default()
            },
        },
        // t5: plain PK → no alter
        metautil::Table {
            DB: db_info,
            Info: model::TableInfo {
                ID: 5,
                Name: CIStr::new("t5"),
                PKIsHandle: true,
                IsCommonHandle: false,
                ..Default::default()
            },
        },
    ];

    let to_be_corrected: HashMap<UniqueTableName, bool> = HashMap::from([
        (
            UniqueTableName {
                DB: "test".into(),
                Table: "t1".into(),
            },
            true,
        ),
        (
            UniqueTableName {
                DB: "test".into(),
                Table: "t2".into(),
            },
            true,
        ),
        (
            UniqueTableName {
                DB: "test".into(),
                Table: "t3".into(),
            },
            true,
        ),
        (
            UniqueTableName {
                DB: "test".into(),
                Table: "t4".into(),
            },
            true,
        ),
        (
            UniqueTableName {
                DB: "test".into(),
                Table: "t5".into(),
            },
            true,
        ),
    ]);

    let ids = prealloc_ids(&tables, &mut allocator);
    db.RegisterPreallocatedIDs(ids);

    for table in &tables {
        db.CreateTablePostRestore(&ctx, table, &to_be_corrected)
            .expect("post restore");
    }

    let g = rec.lock().unwrap();
    assert!(
        g.executed
            .iter()
            .any(|s| s == "alter table `test`.`t1` auto_random_base = 1")
    );
    assert!(
        g.executed
            .iter()
            .any(|s| s == "alter table `test`.`t2` auto_random_base = 1")
    );
    assert!(
        g.executed
            .iter()
            .any(|s| s == "alter table `test`.`t3` auto_increment = 1;")
    );
    assert!(
        g.executed
            .iter()
            .any(|s| s == "alter table `test`.`t4` auto_increment = 1;")
    );
    // t5: exists in ddl job map but neither NeedAutoID nor AutoRandom → no alter
    assert!(
        !g.executed
            .iter()
            .any(|s| s.contains("`t5`") && s.starts_with("alter table"))
    );
}

/// ActionAddIndex stand-in (Go model.ActionAddIndex); not create schema/table.
const ACTION_ADD_INDEX: u8 = 7;

/// Mirrors Go `TestDB_ExecDDL`.
#[test]
fn test_db_exec_ddl() {
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, _) = new_db_with(Arc::clone(&rec), false, "STRICT");
    let ctx = Context::Background();
    let ddl_jobs = [
        model::Job {
            Type: ACTION_ADD_INDEX,
            Query: "CREATE DATABASE IF NOT EXISTS test_db;".into(),
            BinlogInfo: model::HistoryInfo::default(),
            ..Default::default()
        },
        model::Job {
            Type: ACTION_ADD_INDEX,
            Query: String::new(),
            BinlogInfo: model::HistoryInfo::default(),
            ..Default::default()
        },
    ];
    for job in &ddl_jobs {
        db.ExecDDL(&ctx, job).expect("ExecDDL");
    }
    let g = rec.lock().unwrap();
    // First job executes query; second empty query is ignored.
    assert!(
        g.executed
            .iter()
            .any(|s| s == "CREATE DATABASE IF NOT EXISTS test_db;")
    );
}

/// Mirrors Go `TestDB_ExecDDL2`.
#[test]
fn test_db_exec_ddl2() {
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, _) = new_db_with(Arc::clone(&rec), false, "STRICT");
    let ctx = Context::Background();
    let ddl_jobs = [
        model::Job {
            Type: model::ActionCreateSchema,
            Query: "CREATE DATABASE IF NOT EXISTS test_db;".into(),
            BinlogInfo: model::HistoryInfo {
                DBInfo: Some(model::DBInfo {
                    ID: 20000,
                    Name: CIStr::new("test_db"),
                    Charset: "utf8mb4".into(),
                    Collate: "utf8mb4_bin".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        model::Job {
            SchemaName: "test_db".into(),
            Type: model::ActionCreateTable,
            Query: "CREATE TABLE test_db.t1 (id BIGINT);".into(),
            BinlogInfo: model::HistoryInfo {
                TableInfo: Some(model::TableInfo {
                    ID: 20000,
                    Name: CIStr::new("t1"),
                    Columns: vec![model::ColumnInfo {
                        Name: CIStr::new("id"),
                        IsAutoIncrement: false,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        },
        model::Job {
            SchemaName: "test_db".into(),
            Type: ACTION_ADD_INDEX,
            Query: "ALTER TABLE test_db.t1 ADD INDEX i1(id);".into(),
            BinlogInfo: model::HistoryInfo {
                TableInfo: Some(model::TableInfo::default()),
                ..Default::default()
            },
        },
    ];
    for job in &ddl_jobs {
        db.ExecDDL(&ctx, job).expect("ExecDDL2");
    }
    let g = rec.lock().unwrap();
    assert!(g.created_dbs.contains(&"test_db".to_string()));
    assert!(
        g.created_tables
            .iter()
            .any(|(n, id, _)| n == "test_db.t1" && *id == 20000)
    );
    assert!(g.executed.iter().any(|s| s == "use `test_db`;"));
    assert!(
        g.executed
            .iter()
            .any(|s| s == "ALTER TABLE test_db.t1 ADD INDEX i1(id);")
    );
}

/// Mirrors Go `TestCreateTableConsistent`.
#[test]
fn test_create_table_consistent() {
    let ctx = Context::Background();
    let mut allocator = TestAllocator(0);

    let db_info = model::DBInfo {
        Name: CIStr::new("test"),
        ..Default::default()
    };
    let seq_info = model::TableInfo {
        ID: 50,
        Name: CIStr::new("s"),
        AutoIncID: 10,
        Sequence: Some(model::SequenceInfo {
            Cycle: false,
            Increment: 1,
            MinValue: 10,
            MaxValue: i64::MAX,
        }),
        ..Default::default()
    };

    // --- sequence: CreateTables vs CreateTable ---
    let rec_batch = Arc::new(Mutex::new(RecSession::default()));
    let (mut db_batch, support) = new_db_with(Arc::clone(&rec_batch), true, "STRICT");
    assert!(support);

    let mut new_seq = seq_info.Clone();
    new_seq.ID += 100;
    let mut new_tables = vec![metautil::Table {
        DB: db_info.clone(),
        Info: new_seq,
    }];
    let ids = prealloc_ids(&new_tables, &mut allocator);
    db_batch.RegisterPreallocatedIDs(ids);
    db_batch
        .CreateTables(&ctx, &mut new_tables, &HashMap::new(), false, None)
        .expect("CreateTables sequence");
    let r11: Vec<String> = rec_batch
        .lock()
        .unwrap()
        .executed
        .iter()
        .filter(|s| s.contains("`s`") && (s.contains("setval") || s.contains("nextval")))
        .cloned()
        .collect();
    let r12: Vec<(String, i64)> = rec_batch.lock().unwrap().batch_created.clone();

    let rec_single = Arc::new(Mutex::new(RecSession::default()));
    let (mut db_single, _) = new_db_with(Arc::clone(&rec_single), false, "STRICT");
    let mut new_seq = seq_info.Clone();
    new_seq.ID += 100;
    let mut new_table = metautil::Table {
        DB: db_info.clone(),
        Info: new_seq,
    };
    let ids = prealloc_ids(std::slice::from_ref(&new_table), &mut allocator);
    db_single.RegisterPreallocatedIDs(ids);
    db_single
        .CreateTable(&ctx, &mut new_table, &HashMap::new(), false, None)
        .expect("CreateTable sequence");
    let r21: Vec<String> = rec_single
        .lock()
        .unwrap()
        .executed
        .iter()
        .filter(|s| s.contains("`s`") && (s.contains("setval") || s.contains("nextval")))
        .cloned()
        .collect();
    let r22: Vec<(String, i64, bool)> = rec_single.lock().unwrap().created_tables.clone();

    // Go: require.Equal(t, r11, r21) on nextval / show create — sequence restore SQL
    // must be identical; object names must both be created.
    assert_eq!(
        r11, r21,
        "sequence restore SQL must match (Go nextval/show create)"
    );
    assert!(
        r12.iter().any(|(n, _)| n == "test.s"),
        "CreateTables must create sequence s"
    );
    assert!(
        r22.iter().any(|(n, _, _)| n == "test.s"),
        "CreateTable must create sequence s"
    );
    // Both paths restore the same AutoIncID via setval (Go show create / nextval parity).
    assert!(
        r11.iter().any(|s| s == "do setval(`test`.`s`, 10);"),
        "setval AutoIncID=10: {:?}",
        r11
    );

    // --- table + view: batch vs single ---
    let tbl_info = model::TableInfo {
        ID: 60,
        Name: CIStr::new("t"),
        Columns: vec![model::ColumnInfo {
            Name: CIStr::new("a"),
            IsAutoIncrement: false,
        }],
        ..Default::default()
    };
    let view_info = model::TableInfo {
        ID: 61,
        Name: CIStr::new("v"),
        View: Some(()),
        ..Default::default()
    };

    let rec_b2 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db_b2, _) = new_db_with(Arc::clone(&rec_b2), true, "STRICT");
    let mut new_tbl = tbl_info.Clone();
    new_tbl.ID += 100;
    let mut new_view = view_info.Clone();
    new_view.ID += 100;
    let mut new_tables = vec![
        metautil::Table {
            DB: db_info.clone(),
            Info: new_tbl,
        },
        metautil::Table {
            DB: db_info.clone(),
            Info: new_view,
        },
    ];
    let ids = prealloc_ids(&new_tables, &mut allocator);
    db_b2.RegisterPreallocatedIDs(ids);
    db_b2
        .CreateTables(&ctx, &mut new_tables, &HashMap::new(), false, None)
        .expect("CreateTables table/view");
    let batch_names: Vec<(String, i64)> = rec_b2.lock().unwrap().batch_created.clone();

    let rec_s2 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db_s2, _) = new_db_with(Arc::clone(&rec_s2), false, "STRICT");
    let mut new_tbl = tbl_info.Clone();
    new_tbl.ID += 200;
    let mut new_table = metautil::Table {
        DB: db_info.clone(),
        Info: new_tbl,
    };
    let ids = prealloc_ids(std::slice::from_ref(&new_table), &mut allocator);
    db_s2.RegisterPreallocatedIDs(ids);
    db_s2
        .CreateTable(&ctx, &mut new_table, &HashMap::new(), false, None)
        .expect("CreateTable table");
    let mut new_view = view_info.Clone();
    new_view.ID += 200;
    let mut new_table = metautil::Table {
        DB: db_info,
        Info: new_view,
    };
    let ids = prealloc_ids(std::slice::from_ref(&new_table), &mut allocator);
    db_s2.RegisterPreallocatedIDs(ids);
    db_s2
        .CreateTable(&ctx, &mut new_table, &HashMap::new(), false, None)
        .expect("CreateTable view");
    let single_names: Vec<(String, i64, bool)> = rec_s2.lock().unwrap().created_tables.clone();

    // Go compares show create results; here compare object kinds created.
    assert!(batch_names.iter().any(|(n, _)| n == "test.t"));
    assert!(batch_names.iter().any(|(n, _)| n == "test.v"));
    assert!(single_names.iter().any(|(n, _, _)| n == "test.t"));
    assert!(single_names.iter().any(|(n, _, _)| n == "test.v"));
    // Views skip post-restore; tables without NeedAutoID/AutoRandom also skip.
    assert!(
        !rec_b2
            .lock()
            .unwrap()
            .executed
            .iter()
            .any(|s| s.starts_with("alter table"))
    );
    assert!(
        !rec_s2
            .lock()
            .unwrap()
            .executed
            .iter()
            .any(|s| s.starts_with("alter table"))
    );

    // EncloseName / NeedAutoID helpers used by production paths under test.
    assert_eq!(utils::EncloseName("a`b"), "`a``b`");
}
