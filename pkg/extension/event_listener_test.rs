// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Extension session-event contract tests. The server crate owns SQL execution;
//! this module verifies the extension-side recording and dispatch contracts.

use crate::{
    ConnEventInfo, ConnEventTp, Extensions, HasStmtEventListeners, OnConnectionEvent, OnStmtEvent,
    SessionHandler, StmtEventInfo, StmtEventTp, WithSessionHandlerFactory, ast, auth_identity,
    parser, stmtctx, types, variable,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TestStmtInfo {
    db: String,
    alias: String,
    original: String,
    normalized: String,
    affected: u64,
    error: Option<crate::ExtensionError>,
}

impl StmtEventInfo for TestStmtInfo {
    fn User(&self) -> Option<&auth_identity::UserIdentity> {
        None
    }
    fn ActiveRoles(&self) -> Vec<&auth_identity::RoleIdentity> {
        vec![]
    }
    fn CurrentDB(&self) -> &str {
        &self.db
    }
    fn ConnectionInfo(&self) -> Option<&variable::ConnectionInfo> {
        None
    }
    fn SessionAlias(&self) -> &str {
        &self.alias
    }
    fn StmtNode(&self) -> Option<&dyn ast::Node> {
        None
    }
    fn ExecuteStmtNode(&self) -> Option<&ast::ExecuteStmt> {
        None
    }
    fn ExecutePreparedStmt(&self) -> Option<&dyn ast::Node> {
        None
    }
    fn PreparedParams(&self) -> Vec<types::Datum> {
        vec![]
    }
    fn OriginalText(&self) -> &str {
        &self.original
    }
    fn SQLDigest(&self) -> (String, Option<Arc<parser::Digest>>) {
        (self.normalized.clone(), None)
    }
    fn AffectedRows(&self) -> u64 {
        self.affected
    }
    fn RelatedTables(&self) -> Vec<stmtctx::TableEntry> {
        vec![]
    }
    fn GetError(&self) -> Option<&crate::ExtensionError> {
        self.error.as_ref()
    }
}

#[derive(Debug, Eq, PartialEq)]
struct StmtRecord {
    listener: u8,
    tp: StmtEventTp,
    db: String,
    alias: String,
    original: String,
    normalized: String,
    affected: u64,
    error: Option<String>,
}

fn statement_manifest(listener: u8, records: Arc<Mutex<Vec<StmtRecord>>>) -> Arc<crate::Manifest> {
    let (manifest, _clear) =
        crate::manifest::newManifestWithSetup(format!("listener-{listener}"), move || {
            let records = Arc::clone(&records);
            Ok(vec![WithSessionHandlerFactory(move || {
                let records = Arc::clone(&records);
                Some(SessionHandler {
                    OnStmtEvent: Some(Arc::new(move |tp, info| {
                        records.lock().unwrap().push(StmtRecord {
                            listener,
                            tp,
                            db: info.CurrentDB().into(),
                            alias: info.SessionAlias().into(),
                            original: info.OriginalText().into(),
                            normalized: info.SQLDigest().0,
                            affected: info.AffectedRows(),
                            error: info.GetError().map(ToString::to_string),
                        });
                    })),
                    ..SessionHandler::default()
                })
            })])
        })
        .unwrap();
    Arc::new(manifest)
}

#[test]
fn statement_events_fan_out_in_order_and_preserve_go_record_fields() {
    let records = Arc::new(Mutex::new(vec![]));
    let session = Extensions::from_manifests(vec![
        statement_manifest(1, Arc::clone(&records)),
        statement_manifest(2, Arc::clone(&records)),
    ])
    .NewSessionExtensions();
    assert!(HasStmtEventListeners(Some(&session)));
    assert!(!HasStmtEventListeners(None));

    let success = TestStmtInfo {
        db: "test".into(),
        alias: "alias123".into(),
        original: "insert into t values (1), (2)".into(),
        normalized: "insert into `t` values ( ... )".into(),
        affected: 2,
        error: None,
    };
    let failure = TestStmtInfo {
        db: "test".into(),
        alias: "alias123".into(),
        original: "invalid sql".into(),
        normalized: "invalid sql".into(),
        affected: 0,
        error: Some(crate::ExtensionError::new("parse error")),
    };
    OnStmtEvent(Some(&session), StmtEventTp::StmtSuccess, &success);
    OnStmtEvent(Some(&session), StmtEventTp::StmtError, &failure);
    OnStmtEvent(None, StmtEventTp::StmtError, &failure);

    let records = records.lock().unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!((records[0].listener, records[1].listener), (1, 2));
    assert_eq!((records[2].listener, records[3].listener), (1, 2));
    assert_eq!(
        records[0],
        StmtRecord {
            listener: 1,
            tp: StmtEventTp::StmtSuccess,
            db: "test".into(),
            alias: "alias123".into(),
            original: "insert into t values (1), (2)".into(),
            normalized: "insert into `t` values ( ... )".into(),
            affected: 2,
            error: None,
        }
    );
    assert_eq!(
        records[2],
        StmtRecord {
            listener: 1,
            tp: StmtEventTp::StmtError,
            db: "test".into(),
            alias: "alias123".into(),
            original: "invalid sql".into(),
            normalized: "invalid sql".into(),
            affected: 0,
            error: Some("parse error".into()),
        }
    );
}

#[test]
fn connection_events_fan_out_in_order_and_none_is_no_op() {
    let events = Arc::new(Mutex::new(vec![]));
    let manifests = (1..=2)
        .map(|listener| {
            let events = Arc::clone(&events);
            let (manifest, _clear) =
                crate::manifest::newManifestWithSetup(format!("listener-{listener}"), move || {
                    let events = Arc::clone(&events);
                    Ok(vec![WithSessionHandlerFactory(move || {
                        let events = Arc::clone(&events);
                        Some(SessionHandler {
                            OnConnectionEvent: Some(Arc::new(move |tp, info| {
                                events.lock().unwrap().push((
                                    listener,
                                    tp,
                                    info.SessionAlias.clone(),
                                    info.Error.as_ref().map(ToString::to_string),
                                ));
                            })),
                            ..SessionHandler::default()
                        })
                    })])
                })
                .unwrap();
            Arc::new(manifest)
        })
        .collect();
    let session = Extensions::from_manifests(manifests).NewSessionExtensions();
    let info = ConnEventInfo {
        SessionAlias: "rejected".into(),
        Error: Some(crate::ExtensionError::new("handshake rejected")),
        ..ConnEventInfo::default()
    };
    OnConnectionEvent(Some(&session), ConnEventTp::ConnHandshakeRejected, &info);
    OnConnectionEvent(None, ConnEventTp::ConnDisconnected, &info);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].0, events[1].0), (1, 2));
    assert_eq!(events[0].1, ConnEventTp::ConnHandshakeRejected);
    assert_eq!(events[0].2, "rejected");
    assert_eq!(events[0].3.as_deref(), Some("handshake rejected"));
}

#[test]
fn event_discriminants_match_go_iota_order() {
    assert_eq!(ConnEventTp::ConnConnected as u8, 0);
    assert_eq!(ConnEventTp::ConnHandshakeAccepted as u8, 1);
    assert_eq!(ConnEventTp::ConnHandshakeRejected as u8, 2);
    assert_eq!(ConnEventTp::ConnReset as u8, 3);
    assert_eq!(ConnEventTp::ConnDisconnected as u8, 4);
    assert_eq!(StmtEventTp::StmtError as u8, 0);
    assert_eq!(StmtEventTp::StmtSuccess as u8, 1);
}
