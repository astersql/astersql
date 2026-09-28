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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `test_backup_and_restore` 负责 备份 and 恢复。
// 中文总览：函数 `test_restore_multi_tables` 负责 恢复 multi 表集合。

//! Go-equivalent tests for `backup_restore_test.go`.
//!
//! Mapping:
//! - `TestBackupAndRestore` → [`test_backup_and_restore`]
//! - `TestRestoreMultiTables` → [`test_restore_multi_tables`]

use astersql_tests_realtikvtest_brietest::harness::{
    TestCtx, get_backup_temp_dir, init_test_kit, require, reset_engine, serial_guard, testkit,
};
use std::collections::HashMap;
use std::path::Path;

fn remove_backup_dir(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

#[test]
fn remove_backup_dir_only_ignores_missing_paths() {
    let t = TestCtx::new();
    let path =
        std::env::temp_dir().join(format!("astersql-backup-cleanup-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"not a directory").expect("create cleanup error fixture");

    require::True(&t, remove_backup_dir(&path).is_err());

    std::fs::remove_file(path).expect("remove cleanup error fixture");
}

/// `TestBackupAndRestore`.
// 该用例覆盖 备份 and 恢复。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_backup_and_restore() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    tk.MustExec("create database if not exists br");
    tk.MustExec("use br");
    tk.MustExec("create table t1(v int)");
    tk.MustExec("insert into t1 values (1)");
    tk.MustExec("insert into t1 values (2)");
    tk.MustExec("insert into t1 values (3)");
    tk.MustQuery("select count(*) from t1")
        .Check(&testkit::Rows(&["3"]));

    tk.MustExec("create database if not exists br02");
    tk.MustExec("use br02");
    tk.MustExec("create table t1(v int)");

    let tmp_dir = Path::new(&get_backup_temp_dir()).join("bk1");
    let tmp = tmp_dir.to_string_lossy().into_owned();
    require::NoError(&t, remove_backup_dir(&tmp_dir));
    tk.MustQuery(&format!("backup database br to 'local://{tmp}'"));

    tk.MustExec("drop database br");
    tk.MustExec("drop database br02");

    tk.MustQuery(&format!("restore database * from 'local://{tmp}'"));
    tk.MustExec("use br");
    tk.MustQuery("select count(*) from t1")
        .Check(&testkit::Rows(&["3"]));
    tk.MustExec("drop database br");
}

/// `TestRestoreMultiTables`.
// 该用例覆盖 恢复 multi 表集合。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_restore_multi_tables() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    tk.MustExec("create database if not exists br");
    tk.MustExec("use br");

    let mut tables_name_set: HashMap<String, ()> = HashMap::new();
    let table_num = 100;
    for i in 0..table_num {
        tk.MustExec(&format!(
            "create table table_{i} (a int primary key, b json, c varchar(20))"
        ));
        tk.MustExec(&format!(
            "insert into table_{i} values (1, '{{\"a\": 1, \"b\": 2}}', '123')"
        ));
        tk.MustQuery(&format!("select count(*) from table_{i}"))
            .Check(&testkit::Rows(&["1"]));
        tables_name_set.insert(format!("table_{i}"), ());
    }

    let tmp_dir = Path::new(&get_backup_temp_dir()).join("bk1");
    let tmp = tmp_dir.to_string_lossy().into_owned();
    require::NoError(&t, remove_backup_dir(&tmp_dir));
    tk.MustQuery(&format!("backup database br to 'local://{tmp}'"));
    tk.MustExec("drop database br");
    tk.MustQuery(&format!("restore database * from 'local://{tmp}'"));
    tk.MustExec("use br");
    let ddl_create_tables_rows = tk
        .MustQuery("admin show ddl jobs where JOB_TYPE = 'create tables'")
        .Rows()
        .to_vec();
    let mut cnt = 0;
    for row in &ddl_create_tables_rows {
        let tables = &row[2];
        require::NotEqual(&t, "", tables.as_str());
        for table in tables.split(',') {
            require::True(&t, tables_name_set.contains_key(table));
            cnt += 1;
        }
    }
    require::Equal(&t, table_num, cnt);
    for i in 0..table_num {
        tk.MustQuery(&format!("select count(*) from table_{i}"))
            .Check(&testkit::Rows(&["1"]));
    }
    tk.MustExec("drop database br");
}
