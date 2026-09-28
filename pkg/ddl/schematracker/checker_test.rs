// Copyright 2026 AsterSQL.

// SchemaTracker 一致性检查器的边界行为测试。
//
// 通过可注入失败的执行器，验证 Checker 在启停检查、真实 DDL 失败以及尚未建模的
// DDL 入口下，是否按约定更新内存跟踪器并返回错误。

use crate::*;

/// 可按命令类型注入失败的最小 DDL 执行器。
struct NoopExecutor {
    /// 同时覆盖删表和删视图命令，便于比较两条路径的状态更新顺序。
    fail_drop_table: bool,
    /// 用于确认本应直接成功的入口没有被转成 Noop 命令下发。
    fail_noop: bool,
}

impl DdlExecutor for NoopExecutor {
    fn Execute(&mut self, command: DdlCommand) -> Result<(), Error> {
        if self.fail_drop_table
            && matches!(
                command,
                DdlCommand::DropTable(..) | DdlCommand::DropView(..)
            )
        {
            return Err(Error::Mismatch("real DROP TABLE failed".into()));
        }
        if self.fail_noop && matches!(command, DdlCommand::Noop(..)) {
            return Err(Error::Mismatch("real no-op command was called".into()));
        }
        Ok(())
    }
}

fn name(value: &str) -> ast::CIStr {
    ast::NewCIStr(value)
}

fn table(value: &str) -> model::TableInfo {
    model::TableInfo {
        Name: name(value),
        ..Default::default()
    }
}

fn checker(fail_drop_table: bool) -> Checker {
    NewChecker(
        Box::new(NoopExecutor {
            fail_drop_table,
            fail_noop: false,
        }),
        2,
    )
}

#[test]
fn checker_detects_when_real_schema_is_missing() {
    let mut checker = checker(false);

    // 默认执行器查不到真实库，而 CreateSchema 会先把库写入跟踪器，因此应报告漂移。
    assert!(matches!(
        checker.CreateSchema(name("only_in_tracker"), false),
        Err(Error::Mismatch(_))
    ));
}

#[test]
fn disabled_checker_does_not_mirror_create_table() {
    let mut checker = checker(false);
    checker.CreateTestDB();
    // 禁用检查后仍调用真实执行器，但不会把建表结果镜像到跟踪器。
    checker.Disable();

    checker
        .CreateTable(CreateTableSpec {
            schema: name("test"),
            table: table("not_mirrored"),
            if_not_exists: false,
        })
        .unwrap();

    assert!(
        checker
            .tracker
            .InfoStore
            .TableByName(&name("test"), &name("not_mirrored"))
            .is_err()
    );
}

#[test]
fn drop_table_updates_tracker_even_when_real_executor_fails() {
    let mut checker = checker(true);
    checker.CreateTestDB();
    checker
        .tracker
        .CreateTableWithInfo(name("test"), table("tracked"), false)
        .unwrap();

    // DropTable 无论真实执行结果如何都会尝试清理跟踪器，最后再返回原始结果。
    assert!(
        checker
            .DropTable(name("test"), vec![name("tracked")], false)
            .is_err()
    );
    assert!(
        checker
            .tracker
            .InfoStore
            .TableByName(&name("test"), &name("tracked"))
            .is_err()
    );
}

#[test]
fn drop_view_does_not_update_tracker_when_real_executor_fails() {
    let mut checker = checker(true);
    checker.CreateTestDB();
    checker
        .tracker
        .CreateTableWithInfo(name("test"), table("base"), false)
        .unwrap();

    // DropView 会先传播真实执行器错误，因而不会继续删除跟踪器中的对象。
    assert!(
        checker
            .DropView(name("test"), vec![name("base")], false)
            .is_err()
    );
    assert!(
        checker
            .tracker
            .InfoStore
            .TableByName(&name("test"), &name("base"))
            .is_ok()
    );
}

#[test]
fn recover_schema_and_resource_groups_do_not_call_real_executor() {
    let mut checker = NewChecker(
        Box::new(NoopExecutor {
            fail_drop_table: false,
            fail_noop: true,
        }),
        2,
    );

    // 这些入口当前直接成功；若误下发 Noop，fail_noop 会使断言失败。
    assert!(checker.RecoverSchema().is_ok());
    assert!(checker.AddResourceGroup().is_ok());
    assert!(checker.DropResourceGroup().is_ok());
    assert!(checker.AlterResourceGroup().is_ok());
}

/// 保存真实执行结果中的库元数据，供 Checker 做 SHOW CREATE 一致性核对。
#[derive(Default)]
struct SchemaInfoExecutor {
    schema: Option<model::DBInfo>,
}

impl DdlExecutor for SchemaInfoExecutor {
    fn Execute(&mut self, command: DdlCommand) -> Result<(), Error> {
        match command {
            DdlCommand::CreateSchema(name, _) => {
                self.schema = Some(model::DBInfo {
                    Name: name,
                    Charset: "utf8mb4".into(),
                    Collate: "utf8mb4_bin".into(),
                    ..Default::default()
                });
            }
            DdlCommand::CreateSchemaWithInfo(info, _) => self.schema = Some(info),
            _ => {}
        }
        Ok(())
    }

    fn SchemaByName(&self, name: &ast::CIStr) -> Result<Option<model::DBInfo>, Error> {
        Ok(self.schema.clone().filter(|schema| schema.Name.L == name.L))
    }

    fn ConstructResultOfShowCreateDatabase(&self, info: &model::DBInfo) -> Result<String, Error> {
        Ok(format!("{}:{}:{}", info.Name.O, info.Charset, info.Collate))
    }
}

#[test]
fn create_schema_with_info_preserves_complete_metadata() {
    let mut checker = NewChecker(Box::<SchemaInfoExecutor>::default(), 2);
    let info = model::DBInfo {
        Name: name("latin_db"),
        Charset: "latin1".into(),
        Collate: "latin1_bin".into(),
        ..Default::default()
    };

    checker.CreateSchemaWithInfo(info, false).unwrap();

    let tracked = checker
        .tracker
        .InfoStore
        .SchemaByName(&name("latin_db"))
        .unwrap();
    assert_eq!(tracked.Charset, "latin1");
    assert_eq!(tracked.Collate, "latin1_bin");
}
