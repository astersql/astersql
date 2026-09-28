// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::conn::CancellationToken;
use crate::driver::{
    CursorResultSet, DriverContext, Error, Expression, IDriver, PreparedStatement, ResultSet,
    RowContainer, SessionExtensions, TlsState,
};

struct EmptyResultSet;

impl ResultSet for EmptyResultSet {
    fn next(&mut self, cancel: &CancellationToken) -> Result<Option<Vec<Expression>>, Error> {
        if cancel.is_cancelled() {
            return Err(Error("query cancelled".into()));
        }
        Ok(None)
    }

    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

struct TestStatement;

impl PreparedStatement for TestStatement {
    fn ID(&self) -> i32 {
        1
    }

    fn Execute(
        &mut self,
        cancel: &CancellationToken,
        _args: &[Expression],
    ) -> Result<Box<dyn ResultSet>, Error> {
        if cancel.is_cancelled() {
            return Err(Error("query cancelled".into()));
        }
        Ok(Box::new(EmptyResultSet))
    }

    fn AppendParam(&mut self, _param_id: usize, _data: &[u8]) -> Result<(), Error> {
        Ok(())
    }

    fn NumParams(&self) -> usize {
        0
    }

    fn BoundParams(&self) -> &[Option<Vec<u8>>] {
        &[]
    }

    fn SetParamsType(&mut self, _params_type: Vec<u8>) {}

    fn GetParamsType(&self) -> &[u8] {
        &[]
    }

    fn StoreResultSet(&mut self, _result_set: Option<Box<dyn CursorResultSet>>) {}

    fn GetResultSet(&mut self) -> Option<&mut (dyn CursorResultSet + '_)> {
        None
    }

    fn Reset(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn GetCursorActive(&self) -> bool {
        false
    }

    fn SetCursorActive(&mut self, _active: bool) {}

    fn StoreRowContainer(&mut self, _container: Option<Box<dyn RowContainer>>) {}

    fn GetRowContainer(&mut self) -> Option<&mut (dyn RowContainer + '_)> {
        None
    }
}

struct TestContext;

impl DriverContext for TestContext {
    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

struct TestDriver {
    saw_absent_extensions: AtomicBool,
}

impl IDriver for TestDriver {
    fn OpenCtx(
        &self,
        _conn_id: u64,
        _capability: u32,
        _collation: u8,
        _db_name: &str,
        _tls_state: Option<TlsState>,
        extensions: Option<SessionExtensions>,
    ) -> Result<Box<dyn DriverContext>, Error> {
        self.saw_absent_extensions
            .store(extensions.is_none(), Ordering::Release);
        Ok(Box::new(TestContext))
    }
}

#[test]
fn prepared_execution_and_row_fetch_preserve_go_cancellation_context() {
    let cancel = CancellationToken::new();
    cancel.cancel();

    let mut statement = TestStatement;
    assert_eq!(
        statement.Execute(&cancel, &[]).err().unwrap(),
        Error("query cancelled".into())
    );

    let mut result_set = EmptyResultSet;
    assert_eq!(
        result_set.next(&cancel).unwrap_err(),
        Error("query cancelled".into())
    );
}

#[test]
fn driver_open_context_preserves_nil_session_extensions() {
    let driver = TestDriver {
        saw_absent_extensions: AtomicBool::new(false),
    };

    driver.OpenCtx(1, 0, 45, "", None, None).unwrap();

    assert!(driver.saw_absent_extensions.load(Ordering::Acquire));
}
