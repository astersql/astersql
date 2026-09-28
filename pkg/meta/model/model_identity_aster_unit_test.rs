// Copyright 2026 AsterSQL.

// 模型类型身份一致性的单元测试。
//
// 验证 formal group（group_1..4）与 crate 根导出的 `ColumnInfo`/`IndexInfo`/`TableInfo`
// 是同一类型身份，以及表锁类型等元数据判别值与 Go 六值枚举对齐。

use super::*;

/// 各 formal group 暴露的生产常量/构造与根模块一致。
#[test]
fn formal_groups_expose_production_types() {
    assert_eq!(group_1::ExtraHandleID, -1);
    assert_eq!(group_2::JobVersion1, group_3::JobVersion::V1);
    assert_eq!(group_2::ActionAddColumn, group_3::ACTION_ADD_COLUMN);
    assert_eq!(group_3::arc_table(7).ID, 7);
    assert_eq!(group_4::StatePublic, group_1::StatePublic);
}

#[test]
fn job_and_history_models_share_canonical_identity() {
    fn accept_full_job(_: group_3::Job) {}
    fn accept_root_history(_: &DBInfo, _: &TableInfo) {}

    accept_full_job(group_2::Job::default());
    let db = group_3::DBInfo::default();
    let table = group_3::TableInfo::default();
    accept_root_history(&db, &table);
}

#[test]
/// group_4 类型可直接作为根模块 API 参数，证明无平行类型分叉。
fn root_and_compatibility_groups_share_model_type_identity() {
    fn accept_root(_: &ColumnInfo, _: &IndexInfo, _: &TableInfo) {}

    let column = group_4::ColumnInfo::default();
    let index = group_4::IndexInfo::default();
    let table = group_4::TableInfo::default();
    accept_root(&column, &index, &table);
}

#[test]
/// 根模块 TableInfo 是完整 Go 模型，而非仅含少量字段的门面。
fn canonical_table_is_the_complete_go_model_not_a_minimal_facade() {
    let table = TableInfo {
        PKIsHandle: true,
        Charset: "utf8mb4".to_owned(),
        ..Default::default()
    };
    assert!(table.PKIsHandle);
    assert_eq!("utf8mb4", table.Charset);
}

#[test]
/// 表锁类型保持 Go 的六值判别（None/Read/ReadLocal/ReadOnly/Write/WriteLocal）。
fn table_metadata_uses_go_six_value_lock_type() {
    assert_eq!(group_1::ast::model::TableLockNone.0, 0);
    assert_eq!(group_1::ast::model::TableLockRead.0, 1);
    assert_eq!(group_1::ast::model::TableLockReadLocal.0, 2);
    assert_eq!(group_1::ast::model::TableLockReadOnly.0, 3);
    assert_eq!(group_1::ast::model::TableLockWrite.0, 4);
    assert_eq!(group_1::ast::model::TableLockWriteLocal.0, 5);

    let table = TableInfo {
        Lock: Some(TableLockInfo {
            Tp: group_1::ast::model::TableLockReadOnly,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        table.Lock.unwrap().Tp,
        group_4::ast::model::TableLockReadOnly
    );
}
