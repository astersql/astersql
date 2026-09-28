// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::http_status::Router;
use crate::server::{Server, ServerConfig, ServerDriver};
use crate::standby::{StandbyController, StandbyReadyServer, StandbyShutdownServer};

struct Driver;

impl ServerDriver for Driver {
    fn name(&self) -> &str {
        "standby-parity"
    }
}

struct Controller {
    handler_received_server: AtomicBool,
}

impl Controller {
    fn new() -> Self {
        Self {
            handler_received_server: AtomicBool::new(false),
        }
    }
}

impl StandbyController for Controller {
    fn wait_for_activate(&self) {}

    fn end_standby(&self, _result: Result<(), String>) {}

    fn handler(&self, _server: Arc<dyn StandbyShutdownServer>) -> Option<(String, Router)> {
        self.handler_received_server.store(true, Ordering::Release);
        None
    }

    fn on_connection_active(&self) {}

    fn prepare_for_activation(&self, server: &dyn StandbyReadyServer) -> Result<(), String> {
        server.init_tidb_listener()
    }

    fn on_server_created(&self, _server: &dyn StandbyReadyServer) {}

    fn on_server_shutdown(&self, _server: &dyn StandbyShutdownServer) {}
}

#[test]
fn standby_handler_receives_the_live_server_like_go() {
    let controller = Arc::new(Controller::new());
    let server = Server::with_standby(
        ServerConfig::default(),
        Arc::new(Driver),
        Arc::clone(&controller) as Arc<dyn StandbyController>,
    )
    .expect("create server");

    assert!(server.standby_handler().is_none());
    assert!(controller.handler_received_server.load(Ordering::Acquire));
}
