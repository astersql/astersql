// Copyright 2026 AsterSQL.

use super::global_init::{
    DBInfo, GlobalInitRuntime, InfoSchema, SchemaDiff, initGlobalVarFromSystemDB, systemDBFilter,
};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn system_db_filter_matches_go_metadef_is_system_db() {
    let filter = systemDBFilter;
    assert!(!filter.SkipLoadSchema(&DBInfo {
        name: "mysql".into()
    }));
    assert!(!filter.SkipLoadSchema(&DBInfo {
        name: "MySQL".into()
    }));
    for name in [
        "information_schema",
        "performance_schema",
        "metrics_schema",
        "sys",
        "workload_schema",
        "application",
    ] {
        assert!(
            filter.SkipLoadSchema(&DBInfo { name: name.into() }),
            "Go metadef.IsSystemDB does not classify {name} as mysql.SystemDB"
        );
    }
    assert!(!filter.SkipLoadDiff(&SchemaDiff, &InfoSchema));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestError {
    Domain,
    Session,
    Timezone,
    Collation,
}

struct TestSession(Rc<RefCell<Vec<String>>>);

impl Drop for TestSession {
    fn drop(&mut self) {
        self.0.borrow_mut().push("drop_session".into());
    }
}

struct TestRuntime {
    fail_at: Option<TestError>,
    events: Rc<RefCell<Vec<String>>>,
}

impl TestRuntime {
    fn new(fail_at: Option<TestError>) -> Self {
        Self {
            fail_at,
            events: Rc::new(RefCell::new(Vec::new())),
        }
    }

    fn event(&self, value: &str) {
        self.events.borrow_mut().push(value.into());
    }
}

impl GlobalInitRuntime for TestRuntime {
    type Error = TestError;
    type Store = ();
    type Domain = ();
    type Session = TestSession;

    fn get_domain_for_global_var_init(
        &mut self,
        _store: &Self::Store,
        _filter: systemDBFilter,
        server_info_options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> Result<Self::Domain, Self::Error> {
        assert_eq!(
            server_info_options,
            &[astersql_domain_serverinfo::SyncerOption::WithoutStatusEndpointClaim]
        );
        self.event("domain");
        (self.fail_at != Some(TestError::Domain))
            .then_some(())
            .ok_or(TestError::Domain)
    }

    fn create_session(
        &mut self,
        _store: &Self::Store,
        _domain: &Self::Domain,
    ) -> Result<Self::Session, Self::Error> {
        self.event("session");
        if self.fail_at == Some(TestError::Session) {
            Err(TestError::Session)
        } else {
            Ok(TestSession(Rc::clone(&self.events)))
        }
    }

    fn table_value(
        &mut self,
        _session: &Self::Session,
        table: &str,
        key: &str,
    ) -> Result<String, Self::Error> {
        self.event(&format!("table_value:{table}:{key}"));
        if self.fail_at == Some(TestError::Timezone) {
            Err(TestError::Timezone)
        } else {
            Ok("Asia/Shanghai".into())
        }
    }

    fn load_collation_parameter(&mut self, _session: &Self::Session) -> Result<bool, Self::Error> {
        self.event("load_collation");
        if self.fail_at == Some(TestError::Collation) {
            Err(TestError::Collation)
        } else {
            Ok(true)
        }
    }

    fn set_system_timezone(&mut self, timezone: String) {
        self.event(&format!("set_timezone:{timezone}"));
    }

    fn set_new_collation_enabled_for_test(&mut self, enabled: bool) {
        self.event(&format!("set_collation:{enabled}"));
    }

    fn close_domain(&mut self, _domain: Self::Domain) {
        self.event("close_domain");
    }
}

#[test]
fn global_init_preserves_go_side_effect_and_cleanup_order() {
    let mut runtime = TestRuntime::new(None);
    assert_eq!(initGlobalVarFromSystemDB(&mut runtime, &()), Ok(()));
    assert_eq!(
        &*runtime.events.borrow(),
        &[
            "domain",
            "session",
            "table_value:tidb:system_tz",
            "set_timezone:Asia/Shanghai",
            "load_collation",
            "set_collation:true",
            "drop_session",
            "close_domain",
        ]
    );
}

#[test]
fn global_init_closes_domain_after_every_post_creation_error() {
    for error in [
        TestError::Session,
        TestError::Timezone,
        TestError::Collation,
    ] {
        let mut runtime = TestRuntime::new(Some(error));
        assert_eq!(initGlobalVarFromSystemDB(&mut runtime, &()), Err(error));
        assert_eq!(
            runtime.events.borrow().last().map(String::as_str),
            Some("close_domain")
        );
    }

    let mut runtime = TestRuntime::new(Some(TestError::Domain));
    assert_eq!(
        initGlobalVarFromSystemDB(&mut runtime, &()),
        Err(TestError::Domain)
    );
    assert_eq!(&*runtime.events.borrow(), &["domain"]);
}
