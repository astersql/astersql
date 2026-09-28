// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `SHOW CREATE DATABASE` 及相关辅助函数的单元测试。
//
// 覆盖标识符转义、非默认 collation 输出、utf8mb4 默认校对规则判断，
// 以及将 `information_schema` 挪到库名列表前端的行为。

use crate::show::{
    ConstructResultOfShowCreateDatabase, CreateDatabaseInput, FillOneImportJobInfo, ImportJobInfo,
    ShowValue, getDefaultCollate, isUTF8MB4AndDefaultCollation, moveInfoSchemaToFront,
};
use std::time::{Duration, SystemTime};

#[test]
/// 反引号转义库名/策略名，并在非默认 collation 时发出 CHARACTER SET 子句。
fn show_create_database_escapes_names_and_emits_non_default_collation() {
    let sql = ConstructResultOfShowCreateDatabase(&CreateDatabaseInput {
        name: "db`name".into(),
        if_not_exists: true,
        charset: "utf8mb4".into(),
        collation: "utf8mb4_general_ci".into(),
        placement_policy: Some("p`1".into()),
    })
    .unwrap();
    assert_eq!(
        sql,
        "CREATE DATABASE IF NOT EXISTS `db``name` /*!40100 DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci */ /*T![placement] PLACEMENT POLICY=`p``1` */"
    );
    assert_eq!(getDefaultCollate("UTF8MB4"), "utf8mb4_bin");
    assert_eq!(
        isUTF8MB4AndDefaultCollation("utf8mb4", "UTF8MB4_BIN").unwrap(),
        (true, true)
    );
    let mut databases = vec!["test".into(), "information_schema".into(), "mysql".into()];
    moveInfoSchemaToFront(&mut databases);
    assert_eq!(databases[0], "information_schema");
}

#[test]
fn finished_import_job_uses_end_time_as_update_time() {
    let end_time = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
    let stale_update_time = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
    let mut rows = Vec::new();
    FillOneImportJobInfo(
        &mut rows,
        &ImportJobInfo {
            status: "finished".into(),
            end_time: Some(end_time),
            update_time: Some(stale_update_time),
            ..ImportJobInfo::default()
        },
        None,
    );

    assert_eq!(
        rows[0][14],
        ShowValue::String("20.000000000".into()),
        "Go pins a finished job's displayed update time to its end time"
    );
}

#[test]
fn import_job_byte_sizes_match_go_units_format() {
    let mut rows = Vec::new();
    FillOneImportJobInfo(
        &mut rows,
        &ImportJobInfo {
            source_file_size: Some(100_000),
            ..ImportJobInfo::default()
        },
        None,
    );

    assert_eq!(rows[0][7], ShowValue::String("97.66KiB".into()));
}
