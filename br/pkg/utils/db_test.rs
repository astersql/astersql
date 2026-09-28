// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/db_test.go` (`package utils_test`).
//!
//! RestrictedSQLExecutor boundary is mocked (no real TiDB SQL / sqlexec).
//! 对应 Go `db_test.go`：用 mock 受限 SQL 执行器覆盖配置读写与日志备份计数。
//! 不连真实 TiDB；断言默认值解析、Set/Get 往返与进程全局计数语义。
//! Mock 按 SQL 子串分支，刻意不解析完整 AST，保持与 Go 测试同级覆盖。
//! 字段元数据一律 TypeString，确保 GetDatum/ToString 路径可走通。
//! 本文件不启动真实集群，仅验证 utils::db 配置读写契约。
//! Region split 与 GC ratio 分测，避免单一 mock 状态串扰。

use std::any::Any;

use astersql_parser_mysql::r#type::TypeString;
use astersql_parser_types::NewFieldType;

use crate::db::{
    CheckLogBackupTaskExist, DefaultGcRatioVal, DisabledGcRatioVal, GetGcRatio, GetRegionSplitInfo,
    GetSplitKeys, GetSplitSize, GetTidbNewCollationEnabled, IsLogBackupInUse,
    LogBackupTaskCountDec, LogBackupTaskCountInc, SetGcRatio, TidbNewCollationEnabled,
};
use crate::stubs::{
    ColumnInfo, GoError, RestrictedSQLExecutor, ResultField, Row, context::Context,
};

/// Mock：可注入错误；`show config` 返回预设行；`set config` 写回第 4 列。
/// `err_happen` 用于未来扩展错误降级用例；当前主路径保持 false。
struct MockRestrictedSQLExecutor {
    rows: Vec<Row>,
    fields: Vec<ResultField>,
    err_happen: bool,
}

impl RestrictedSQLExecutor for MockRestrictedSQLExecutor {
    fn ExecRestrictedSQL(
        &mut self,
        _ctx: &Context,
        _opts: Vec<()>,
        sql: &str,
        args: Vec<Box<dyn Any>>,
    ) -> Result<(Vec<Row>, Vec<ResultField>), GoError> {
        // 注入错误路径：验证调用方是否按预期降级或上抛。
        // show/set 分支互斥；未识别 SQL 返回空结果，模拟无关语句。
        if self.err_happen {
            return Err(Box::new(std::io::Error::other("injected error")));
        }
        // SHOW CONFIG：原样返回构造好的 rows/fields。
        if sql.contains("show config") {
            return Ok((self.rows.clone(), self.fields.clone()));
        }
        // SET CONFIG gc.ratio-threshold：把绑定参数写回 value 列，便于后续 Get。
        if sql.contains("set config") && sql.contains("gc.ratio-threshold") {
            let value = args[0]
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default();
            for r in &mut self.rows {
                r.set_cell(3, value.clone());
            }
        }
        Ok((Vec::new(), Vec::new()))
    }
}

/// 构造 4 列字符串字段元数据，匹配 SHOW CONFIG 列布局。
// 四列字符串字段：Type/Instance/Name/Value。
fn string_fields() -> Vec<ResultField> {
    (0..4)
        .map(|_| ResultField {
            column: Some(ColumnInfo {
                FieldType: NewFieldType(TypeString),
            }),
        })
        .collect()
}

#[test]
fn test_check_log_backup_task_exist() {
    // Counter is process-global; normalize to zero first.
    // 计数器为进程全局，先清零避免与其他测试串扰。
    // Inc/Dec 成对后必须回到不存在状态。
    while CheckLogBackupTaskExist() {
        LogBackupTaskCountDec();
    }
    assert!(!CheckLogBackupTaskExist());
    assert!(!IsLogBackupInUse(&()));
    LogBackupTaskCountInc();
    assert!(CheckLogBackupTaskExist());
    assert!(IsLogBackupInUse(&()));
    LogBackupTaskCountDec();
    assert!(!CheckLogBackupTaskExist());
}

#[test]
fn test_gc() {
    // 模拟多行 SHOW CONFIG；GetGcRatio 只取首行 value。
    // 第二行存在是为了贴近真实多 store 输出，实现仍只读 rows[0]。
    let fields = string_fields();
    let rows = vec![
        Row::from_cells(vec![
            "tikv".into(),
            " 127.0.0.1:20161".into(),
            "log-backup.enable".into(),
            "1.1".into(),
        ]),
        Row::from_cells(vec![
            "tikv".into(),
            " 127.0.0.1:20162".into(),
            "log-backup.enable".into(),
            "1.1".into(),
        ]),
    ];
    let mut s = MockRestrictedSQLExecutor {
        rows,
        fields,
        err_happen: false,
    };
    let ratio = GetGcRatio(&mut s).expect("GetGcRatio");
    assert_eq!(ratio.as_deref(), Some("1.1"));

    // Set 后再次 Get，确认 mock 写回与 DisabledGcRatioVal 路径。
    // `-1.0` 与 DisabledGcRatioVal 常量一致。
    SetGcRatio(&mut s, "-1.0").expect("SetGcRatio");
    let ratio = GetGcRatio(&mut s).expect("GetGcRatio after set");
    assert_eq!(ratio.as_deref(), Some("-1.0"));
}

#[test]
fn test_region_split_info() {
    // `10MB` 经 ByteSize 解析为 10_000_000，对齐 Go units 行为。
    // 注意：decimal MB（1e6）而非二进制 MiB。
    let fields = string_fields();
    let rows = vec![Row::from_cells(vec![
        "tikv".into(),
        "127.0.0.1:20161".into(),
        "coprocessor.region-split-size".into(),
        "10MB".into(),
    ])];
    let mut s = MockRestrictedSQLExecutor {
        rows,
        fields: fields.clone(),
        err_happen: false,
    };
    assert_eq!(GetSplitSize(&mut s), 10_000_000);

    // split-keys 为纯整数，不走 ByteSize。
    // 与 size 路径分离，防止把 keys 误当容量单位解析。
    let rows = vec![Row::from_cells(vec![
        "tikv".into(),
        "127.0.0.1:20161".into(),
        "coprocessor.region-split-keys".into(),
        "100000".into(),
    ])];
    let mut s = MockRestrictedSQLExecutor {
        rows,
        fields,
        err_happen: false,
    };
    assert_eq!(GetSplitKeys(&mut s), 100_000);
}

#[test]
#[should_panic]
fn split_size_rejects_missing_value_field_metadata_like_go() {
    let mut s = MockRestrictedSQLExecutor {
        rows: vec![Row::from_cells(vec![
            "tikv".into(),
            "127.0.0.1:20161".into(),
            "coprocessor.region-split-size".into(),
            "10MB".into(),
        ])],
        fields: Vec::new(),
        err_happen: false,
    };

    let _ = GetSplitSize(&mut s);
}

#[test]
fn config_error_and_invalid_values_follow_go_contract() {
    let mut failing = MockRestrictedSQLExecutor {
        rows: Vec::new(),
        fields: Vec::new(),
        err_happen: true,
    };
    assert_eq!(GetSplitSize(&mut failing), 96 * 1024 * 1024);
    assert_eq!(GetSplitKeys(&mut failing), 960_000);
    assert_eq!(
        GetGcRatio(&mut failing)
            .expect_err("GetGcRatio must propagate SQL errors")
            .to_string(),
        "injected error"
    );
    assert_eq!(
        SetGcRatio(&mut failing, "-1.0")
            .expect_err("SetGcRatio must annotate SQL errors")
            .to_string(),
        "failed to set config `gc.ratio-threshold`=-1.0: injected error"
    );

    let fields = string_fields();
    let mut invalid_size = MockRestrictedSQLExecutor {
        rows: vec![Row::from_cells(vec![
            "tikv".into(),
            "127.0.0.1:20161".into(),
            "coprocessor.region-split-size".into(),
            "not-a-size".into(),
        ])],
        fields: fields.clone(),
        err_happen: false,
    };
    assert_eq!(GetSplitSize(&mut invalid_size), 96 * 1024 * 1024);

    let mut invalid_keys = MockRestrictedSQLExecutor {
        rows: vec![Row::from_cells(vec![
            "tikv".into(),
            "127.0.0.1:20161".into(),
            "coprocessor.region-split-keys".into(),
            "not-an-integer".into(),
        ])],
        fields,
        err_happen: false,
    };
    assert_eq!(GetSplitKeys(&mut invalid_keys), 960_000);
}

#[test]
fn empty_config_and_public_helpers_match_go() {
    let mut empty = MockRestrictedSQLExecutor {
        rows: Vec::new(),
        fields: Vec::new(),
        err_happen: false,
    };
    assert_eq!(GetRegionSplitInfo(&mut empty), (96 * 1024 * 1024, 960_000));
    assert_eq!(
        GetGcRatio(&mut empty).expect("empty config is supported"),
        None
    );

    assert_eq!(DefaultGcRatioVal, "1.1");
    assert_eq!(DisabledGcRatioVal, "-1.0");
    assert_eq!(TidbNewCollationEnabled, "new_collation_enabled");
    assert_eq!(GetTidbNewCollationEnabled(), TidbNewCollationEnabled);
}
