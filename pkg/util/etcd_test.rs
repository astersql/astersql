// Copyright 2026 AsterSQL.

use anyhow::Error;

use crate::etcd::{CancellationContext, ClientConnectionClosing, NewSession, SessionFactory};

struct ClosingFactory {
    attempts: usize,
}

impl SessionFactory for ClosingFactory {
    type Session = ();

    fn new_session(
        &mut self,
        _ctx: &CancellationContext,
        _ttl: i32,
    ) -> Result<Self::Session, Error> {
        self.attempts += 1;
        Err(Error::new(ClientConnectionClosing))
    }
}

#[test]
fn terminal_session_error_preserves_its_type() {
    let mut factory = ClosingFactory { attempts: 0 };

    let error = NewSession(&CancellationContext::new(), "owner", &mut factory, 2, 60)
        .expect_err("a closing connection must stop session creation");

    assert!(error.is::<ClientConnectionClosing>());
    assert_eq!(factory.attempts, 1);
}
