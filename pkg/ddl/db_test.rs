// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL（数据定义语言，如 CREATE/ALTER/DROP）数据库级测试模块。
//

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_variables
)]

use std::time::Duration;

// 引入 DDL 执行器相关类型：
// - Executor：DDL 执行器，负责把 schema 变更转换为 job 并应用到内存元数据；
// - MemoryJobBackend：内存版 job 后端，记录 job 历史，替代真实的 TiKV 存储；
// - OnExist：对象已存在时的处理策略（报错 / 忽略）；
// - SessionContext：会话上下文，携带系统变量与提示信息（notes）。
use crate::executor::{
    Executor, ExecutorError, JobState, MemoryJobBackend, OnExist, SessionContext,
};

/// 构造测试用的 DDL 执行器与默认会话上下文。
/// 使用内存 job 后端和零租约时长（schema lease，schema 版本同步的租约周期），
/// 使测试同步执行、无需等待。
fn executor() -> (Executor<MemoryJobBackend>, SessionContext) {
    (
        Executor::new(MemoryJobBackend::default(), Duration::ZERO),
        SessionContext::default(),
    )
}

/// 验证 CREATE DATABASE：
/// 1) 未显式指定字符集时，采用会话系统变量给出的 utf8mb4 默认排序规则；
/// 2) 同名 schema 已存在时，OnExist::Error 报错、OnExist::Ignore 幂等返回原 id；
/// 3) schema 名大小写不敏感（App/app/APP 视为同一个）。
#[test]
fn create_schema_uses_session_default_collation_and_on_exist_rules() {
    let (mut ddl, mut session) = executor();
    // 通过会话系统变量注入 utf8mb4 的默认排序规则。
    session.system_vars.insert(
        "default_collation_for_utf8mb4".into(),
        "utf8mb4_general_ci".into(),
    );
    let id = ddl
        .create_schema(&mut session, "App", &[], None, OnExist::Error)
        .unwrap();
    let schema = &ddl.schemas["app"];
    assert_eq!(id, schema.id);
    assert_eq!("utf8mb4", schema.charset);
    assert_eq!("utf8mb4_general_ci", schema.collation);
    assert!(matches!(
        ddl.create_schema(&mut session, "app", &[], None, OnExist::Error),
        Err(ExecutorError::SchemaExists(_))
    ));
    assert_eq!(
        id,
        ddl.create_schema(&mut session, "APP", &[], None, OnExist::Ignore)
            .unwrap()
    );
    assert!(matches!(
        ddl.create_schema(&mut session, "app", &[], None, OnExist::Replace),
        Err(ExecutorError::Unsupported(operation)) if operation == "replace schema"
    ));
}

/// 对应 Go 创建 schema 时对重复 charset/collation 选项的归并规则：
/// 同值（忽略大小写）可重复指定，互相冲突的值必须在写入元数据前报错。
#[test]
fn create_schema_rejects_conflicting_charset_options_without_side_effects() {
    let (mut ddl, mut session) = executor();
    let options = vec![
        (Some("UTF8".into()), Some("utf8_bin".into())),
        (Some("utf8".into()), Some("UTF8_BIN".into())),
    ];
    ddl.create_schema(&mut session, "consistent", &options, None, OnExist::Error)
        .unwrap();
    assert_eq!("utf8", ddl.schemas["consistent"].charset);
    assert_eq!("utf8_bin", ddl.schemas["consistent"].collation);

    let history_len = ddl.backend().history().len();
    let conflicting = vec![(Some("utf8".into()), None), (Some("latin1".into()), None)];
    assert!(matches!(
        ddl.create_schema(
            &mut session,
            "conflicting",
            &conflicting,
            None,
            OnExist::Error,
        ),
        Err(ExecutorError::InvalidCharsetCollation)
    ));
    assert!(!ddl.schemas.contains_key("conflicting"));
    assert_eq!(history_len, ddl.backend().history().len());
}

/// 验证 ALTER DATABASE ... CHARACTER SET：
/// 修改字符集/排序规则生效；同值重复修改是幂等操作（不产生新的 DDL job）；
/// 排序规则与字符集不匹配（utf8 配 latin1_bin）时报错。
#[test]
fn alter_schema_charset_validates_collation_and_is_idempotent() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    ddl.alter_schema_charset(&mut session, "test", "utf8", "utf8_bin")
        .unwrap();
    assert_eq!("utf8", ddl.schemas["test"].charset);
    assert_eq!("utf8_bin", ddl.schemas["test"].collation);
    // 记录 job 历史长度；同值（仅大小写不同）的重复修改不应新增 job。
    let history_len = ddl.backend().history().len();
    ddl.alter_schema_charset(&mut session, "test", "UTF8", "UTF8_BIN")
        .unwrap();
    assert_eq!(history_len, ddl.backend().history().len());
    // 字符集与排序规则不匹配应返回 InvalidCharsetCollation 错误。
    assert!(matches!(
        ddl.alter_schema_charset(&mut session, "test", "utf8", "latin1_bin"),
        Err(ExecutorError::InvalidCharsetCollation)
    ));
}

/// 验证 schema 放置策略（placement policy，控制数据副本物理分布的规则）：
/// 可切换到新策略；设置为 "default" 表示恢复默认并清除策略；
/// ignore 模式下不修改策略，只在会话 notes 中记录提示。
#[test]
fn schema_placement_default_and_ignore_clear_the_policy() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(
        &mut session,
        "test",
        &[],
        Some("primary".into()),
        OnExist::Error,
    )
    .unwrap();
    ddl.alter_schema_placement(&mut session, "test", Some("analytics".into()), false)
        .unwrap();
    assert_eq!(
        Some("analytics"),
        ddl.schemas["test"].placement_policy.as_deref()
    );
    ddl.alter_schema_placement(&mut session, "test", Some("default".into()), false)
        .unwrap();
    assert_eq!(None, ddl.schemas["test"].placement_policy);
    ddl.alter_schema_placement(&mut session, "test", Some("ignored".into()), true)
        .unwrap();
    assert_eq!(None, ddl.schemas["test"].placement_policy);
    assert_eq!(vec!["placement is ignored"], session.notes);
}

/// 验证 DROP DATABASE 与 RECOVER（闪回恢复）：
/// 删除后 schema 从元数据消失；重复删除非幂等模式报 SchemaNotFound，
/// if_exists 模式静默成功；恢复后 schema id 与删除前保持一致（元数据身份不变）。
#[test]
fn drop_and_recover_schema_preserve_metadata_identity() {
    let (mut ddl, mut session) = executor();
    let id = ddl
        .create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    // 先保存元数据快照，用于稍后恢复。
    let saved = ddl.schemas["test"].clone();
    ddl.drop_schema(&mut session, "test", false).unwrap();
    assert!(!ddl.schemas.contains_key("test"));
    assert!(matches!(
        ddl.drop_schema(&mut session, "test", false),
        Err(ExecutorError::SchemaNotFound(_))
    ));
    ddl.drop_schema(&mut session, "test", true).unwrap();
    assert_eq!(vec!["schema test does not exist"], session.notes);
    ddl.recover_schema(&mut session, saved).unwrap();
    assert_eq!(id, ddl.schemas["test"].id);
}

/// 对应 Go recover database 的同名对象保护：恢复目标已存在时应报错，
/// 且不得覆盖当前 schema 或提交恢复 job。
#[test]
fn recover_schema_rejects_name_conflict_without_overwriting_metadata() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "test", &[], None, OnExist::Error)
        .unwrap();
    let recover_candidate = ddl.schemas["test"].clone();
    ddl.drop_schema(&mut session, "test", false).unwrap();
    let replacement_id = ddl
        .create_schema(&mut session, "TEST", &[], None, OnExist::Error)
        .unwrap();
    let history_len = ddl.backend().history().len();

    assert!(matches!(
        ddl.recover_schema(&mut session, recover_candidate),
        Err(ExecutorError::SchemaExists(name)) if name == "test"
    ));
    assert_eq!(replacement_id, ddl.schemas["test"].id);
    assert_eq!(history_len, ddl.backend().history().len());
}

/// 验证 DDL job 生命周期：多个 schema 变更提交后，
/// job 历史按提交顺序分配递增 id，且全部到达 Synced（schema 版本已全局同步）终态。
#[test]
fn schema_jobs_reach_synced_history_in_submission_order() {
    let (mut ddl, mut session) = executor();
    ddl.create_schema(&mut session, "one", &[], None, OnExist::Error)
        .unwrap();
    ddl.create_schema(&mut session, "two", &[], None, OnExist::Error)
        .unwrap();
    ddl.drop_schema(&mut session, "one", false).unwrap();
    let history = ddl.backend().history();
    assert_eq!(
        vec![1, 2, 3],
        history.iter().map(|job| job.id).collect::<Vec<_>>()
    );
    assert!(history.iter().all(|job| job.state == JobState::Synced));
}
