// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// IMPORT FROM SELECT 出错后清理临时目录的 Go testkit 参考测试。
//
// 通过 failpoint 注入导入错误，验证选行 channel 结束后 import 根目录被清空；
// 当前以字符串化 Go 逻辑形式保存，供迁移完整性断言。

/// 内嵌的 Go `TestImportFromSelectCleanup` 等价逻辑文本。
const GO_REFERENCE: &str = r########"
// IMPORT FROM SELECT 出错后的临时目录清理测试，不会真正运行 failpoint、testkit 或后台导入；
// channel、WaitGroup、chunk、PhysicalSelection 等 Go 语义均以可审计参考形式保留。
//   testkit, types, util, chunk, require

// check_import_dir_empty 对应 Go 的 checkImportDirEmpty：检查 IMPORT 根目录不存在或为空。
fn check_import_dir_empty(t: &mut TestingT) {
    let tidb_cfg = tidb::get_global_config();
    let import_dir = importer::get_import_root_dir(&tidb_cfg);
    match os::stat(&import_dir) {
        Err(err) => {
            // 目录不存在是允许的；其它 stat 错误应失败。
            require::true_(t, os::is_not_exist(err), &import_dir);
        }
        Ok(_) => {
            let entries = os::read_dir(&import_dir);
            require::no_error(t, entries.err());
            require::empty(t, entries.unwrap());
        }
    }
}

// test_import_from_select_cleanup 对应 Go 的 TestImportFromSelectCleanup。
// 通过 failpoint 注入 ImportSelectedRows 错误，验证后台选行 channel 结束后 import 目录被清理。
#[test]
fn test_import_from_select_cleanup() {
    let mut t = TestingT::new();
    let ctx = context::background();
    let store = testkit::create_mock_store(&mut t);
    let tk = testkit::new_test_kit(&mut t, store.clone());
    let mut tidb_cfg = tidb::get_global_config();
    tidb_cfg.temp_dir = t.temp_dir();
    check_import_dir_empty(&mut t);

    require::no_error(
        &mut t,
        failpoint::enable("github.com/pingcap/tidb/pkg/executor/importer/mockImportFromSelectErr", "return(true)"),
    );
    t.cleanup(|| {
        require::no_error(
            &mut t,
            failpoint::disable("github.com/pingcap/tidb/pkg/executor/importer/mockImportFromSelectErr"),
        )
    });

    tk.must_exec("use test");
    tk.must_exec("create table t(a int)");
    let domain = session::get_domain(store.clone());
    require::no_error(&mut t, domain.err());
    let db_info = domain.info_schema().schema_by_name(ast::new_ci_str("test"));
    require::true_(&mut t, db_info.is_some());
    let table = domain
        .info_schema()
        .table_by_name(context::background(), ast::new_ci_str("test"), ast::new_ci_str("t"));
    require::no_error(&mut t, table.err());

    let import_into = plannercore::ImportInto {
        table: resolve::TableNameW {
            table_name: ast::TableName { name: ast::new_ci_str("t"), ..Default::default() },
            db_info: model::DBInfo { name: ast::new_ci_str("test"), id: db_info.id, ..Default::default() },
            ..Default::default()
        },
        select_plan: physicalop::PhysicalSelection::default(),
        ..Default::default()
    }
    .init(tk.session().get_plan_ctx());
    let plan = importer::new_import_plan(&ctx, tk.session(), import_into, table.clone());
    require::no_error(&mut t, plan.err());
    let controller = importer::new_load_data_controller(plan.unwrap(), table, importer::ASTArgs::default());
    require::no_error(&mut t, controller.err());
    let mut table_importer = importer::new_table_importer_for_test(&ctx, controller.unwrap(), "11", store);
    require::no_error(&mut t, table_importer.err());

    // Go 使用 chan importer.QueryChunk 和 WaitGroupWrapper 异步喂两批 selected rows。
    let (tx, rx) = channel::<importer::QueryChunk>();
    table_importer.set_selected_chunk_ch(rx);
    let mut wg = util::WaitGroupWrapper::new();
    wg.run(move || {
        scopeguard::defer(|| close(tx));
        let mut fields = Vec::with_capacity(3);
        fields.push(types::new_field_type(mysql::TypeLong));

        let mut chk = chunk::new(fields.clone(), 2, 2);
        chk.append_int64(0, 1);
        chk.append_int64(0, 2);
        tx.send(importer::QueryChunk { fields: fields.clone(), chk, row_id_offset: 0 });

        let mut chk = chunk::new(fields.clone(), 1, 1);
        chk.append_int64(0, 3);
        tx.send(importer::QueryChunk { fields, chk, row_id_offset: 2 });
    });

    let (_, err) = table_importer.import_selected_rows(&ctx, tk.session());
    require::error_contains(&mut t, err, "mock import from select error");
    wg.wait();
    table_importer.backend().close_engine_mgr();
    check_import_dir_empty(&mut t);
}
"########;

use std::fs;
use std::io;
use std::path::Path;

/// 对应 Go `checkImportDirEmpty`：根目录不存在或已经为空时成功。
fn check_import_dir_empty(import_dir: &Path) -> io::Result<()> {
    match fs::read_dir(import_dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(mut entries) => match entries.next() {
            None => Ok(()),
            Some(_) => Err(io::Error::other(format!(
                "import directory is not empty: {}",
                import_dir.display()
            ))),
        },
    }
}

/// 确认 Go 参考文本保留完整的错误、并发与清理契约。
#[test]
fn import_from_select_cleanup_reference_preserves_go_contract() {
    let required_fragments = [
        "check_import_dir_empty(&mut t);",
        "mockImportFromSelectErr",
        "table_importer.set_selected_chunk_ch(rx);",
        "wg.run(move ||",
        "scopeguard::defer(|| close(tx));",
        "row_id_offset: 0",
        "row_id_offset: 2",
        "table_importer.import_selected_rows",
        "mock import from select error",
        "wg.wait();",
        "table_importer.backend().close_engine_mgr();",
    ];
    for fragment in required_fragments {
        assert!(
            GO_REFERENCE.contains(fragment),
            "missing Go contract: {fragment}"
        );
    }

    let positions = required_fragments.map(|fragment| GO_REFERENCE.find(fragment).unwrap());
    for pair in positions.windows(2) {
        assert!(pair[0] < pair[1], "Go cleanup contract is out of order");
    }

    assert_eq!(
        2,
        GO_REFERENCE
            .matches("check_import_dir_empty(&mut t);")
            .count(),
        "the import root must be checked before and after the failed import"
    );
}

#[test]
fn check_import_dir_empty_matches_go_filesystem_branches() {
    let temp_dir = std::env::temp_dir().join(format!(
        "astersql-import-cleanup-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let import_dir = temp_dir.join("import-4000");

    fs::create_dir_all(&temp_dir).unwrap();
    check_import_dir_empty(&import_dir).unwrap();

    fs::create_dir(&import_dir).unwrap();
    check_import_dir_empty(&import_dir).unwrap();

    fs::write(import_dir.join("leftover-engine"), b"stale").unwrap();
    let error = check_import_dir_empty(&import_dir).unwrap_err();
    assert_eq!(io::ErrorKind::Other, error.kind());
    assert!(error.to_string().contains("not empty"));

    fs::remove_dir_all(temp_dir).unwrap();
}
