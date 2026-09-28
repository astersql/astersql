// Copyright 2026 AsterSQL.

// DDL Mock 基础设施的单元测试。
//
// 覆盖共享 `Controller` 的期望匹配与消费语义，并验证 SchemaLoader、系统表
// Manager 两类 Mock 能通过生产 trait 正确转发参数、返回预设值及保留错误。

use super::*;

/// 仅作为生产 `Session` trait 的类型占位；Manager Mock 不应执行真实 SQL。
struct DummySession;

impl ddl_systable::Session for DummySession {
    fn execute(
        &mut self,
        _context: &ddl_systable::Context,
        _sql: &str,
        _label: &str,
    ) -> Result<Vec<ddl_systable::Row>, ddl_systable::Error> {
        // 测试只校验会话参数被标记并参与匹配，任何执行都表示 Mock 越过了边界。
        unreachable!("mock manager must not execute the real session")
    }
}

#[test]
/// 默认匹配全部未满足期望，而非强制按录制顺序消费。
fn controller_matches_unsatisfied_expectations_without_default_ordering() {
    let controller = Controller::default();
    controller.record(
        "second",
        vec![Matcher::Exact(Argument::Int(2))],
        Ok(ReturnValue::Int(20)),
    );
    controller.record(
        "first",
        vec![Matcher::Exact(Argument::Int(1))],
        Ok(ReturnValue::Int(10)),
    );

    assert_eq!(
        controller.call("first", vec![Argument::Int(1)]).unwrap(),
        ReturnValue::Int(10)
    );
    assert_eq!(
        controller.call("second", vec![Argument::Int(2)]).unwrap(),
        ReturnValue::Int(20)
    );
    controller.verify().unwrap();
}

#[test]
/// 参数不匹配不能消费期望，且命中后应原样返回预设的 Mock 错误。
fn controller_preserves_unmatched_expectations_and_error_results() {
    let controller = Controller::default();
    controller.record(
        "reload",
        vec![Matcher::Exact(Argument::Text("expected".into()))],
        Err(MockError("reload failed".into())),
    );

    assert!(
        controller
            .call("reload", vec![Argument::Text("other".into())])
            .is_err()
    );
    assert!(controller.verify().is_err());
    assert_eq!(
        controller
            .call("reload", vec![Argument::Text("expected".into())])
            .unwrap_err(),
        MockError("reload failed".into())
    );
    controller.verify().unwrap();
}

#[test]
/// SchemaLoader 将错误返回类型转成生产错误，并让未调用期望留待校验发现。
fn schema_loader_reports_wrong_return_type_and_missing_calls() {
    let controller = Controller::default();
    let loader = new_mock_schema_loader(controller.clone());
    controller.record("Reload", Vec::new(), Ok(ReturnValue::Bool(true)));
    assert_eq!(
        loader.reload().unwrap_err(),
        ddl_systable::SchemaLoaderError::new("Reload returned the wrong type")
    );

    loader.expect().reload(Ok(()));
    assert!(controller.verify().is_err());
    loader.reload().unwrap();
    controller.verify().unwrap();
}

#[test]
/// Mock SchemaLoader 可经生产 trait 对象调用，而不只支持自身的固有接口。
fn mock_schema_loader_implements_the_production_ddl_trait() {
    let controller = Controller::default();
    let loader = new_mock_schema_loader(controller.clone());
    loader.expect().reload(Ok(()));
    let production: std::sync::Arc<dyn ddl_systable::SchemaLoader> = std::sync::Arc::new(loader);
    production.reload().unwrap();
    controller.verify().unwrap();
}

#[test]
/// 经生产 Manager trait 覆盖 Job 解码、会话占位及各类系统表查询的参数映射。
fn mock_manager_implements_the_production_systable_trait() {
    let controller = Controller::default();
    let manager = new_mock_manager(controller.clone());
    let production: &dyn ddl_systable::Manager = &manager;
    let context = ddl_systable::Context {
        request_id: "request-7".into(),
    };

    controller.record(
        "GetJobByID",
        vec![
            Matcher::Exact(Argument::Text("request-7".into())),
            Matcher::Exact(Argument::Int(42)),
        ],
        Ok(ReturnValue::Job(br#"{"id":42}"#.to_vec())),
    );
    let job = production.get_job_by_id(&context, 42).unwrap();
    assert_eq!(job.job.id, 42);
    assert_eq!(job.bytes, br#"{"id":42}"#);

    controller.record(
        "GetJobBytesByIDWithSe",
        vec![
            Matcher::Any,
            Matcher::Exact(Argument::Session),
            Matcher::Exact(Argument::Int(43)),
        ],
        Ok(ReturnValue::Bytes(vec![4, 3])),
    );
    let mut session = DummySession;
    assert_eq!(
        production
            .get_job_bytes_by_id_with_session(&context, &mut session, 43)
            .unwrap(),
        vec![4, 3]
    );

    manager
        .expect()
        .get_mdl_version(Matcher::Any, Matcher::Exact(Argument::Int(44)), Ok(144));
    assert_eq!(production.get_mdl_version(&context, 44).unwrap(), 144);

    manager
        .expect()
        .get_min_job_id(Matcher::Any, Matcher::Exact(Argument::Int(45)), Ok(145));
    assert_eq!(production.get_min_job_id(&context, 45).unwrap(), 145);

    manager.expect().has_flashback_cluster_job(
        Matcher::Any,
        Matcher::Exact(Argument::Int(46)),
        Ok(true),
    );
    assert!(production.has_flashback_cluster_job(&context, 46).unwrap());
    controller.verify().unwrap();
}

#[test]
/// Recorder 配置的生产错误必须保持具体变体，不能降级成通用 Mock 错误。
fn mock_manager_preserves_the_configured_production_error() {
    let controller = Controller::default();
    let manager = new_mock_manager(controller.clone());
    let context = ddl_systable::Context {
        request_id: "request-error".into(),
    };
    manager.expect().get_job_by_id(
        Matcher::Any,
        Matcher::Exact(Argument::Int(99)),
        Err(ddl_systable::Error::NotFound),
    );

    assert_eq!(
        ddl_systable::Manager::get_job_by_id(&manager, &context, 99)
            .err()
            .expect("configured manager error must be returned"),
        ddl_systable::Error::NotFound
    );
    controller.verify().unwrap();
}
