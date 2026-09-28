// Copyright 2026 AsterSQL.

use super::runaway::{
    ResourceControllerConfig, ResourceGroupController, ResourceGroupRuntime, RunawayManager,
};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Controller {
    started: AtomicBool,
    config: ResourceControllerConfig,
}

impl ResourceGroupController for Controller {
    fn start(&self) {
        self.started.store(true, Ordering::SeqCst);
    }

    fn config(&self) -> &ResourceControllerConfig {
        &self.config
    }
}

struct Manager;

impl RunawayManager for Manager {}

#[test]
fn initialize_starts_only_the_resource_controller_like_go() {
    let controller = Arc::new(Controller {
        started: AtomicBool::new(false),
        config: ResourceControllerConfig {
            server_id: 42,
            advertised_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 4000,
            request_unit_mode: true,
        },
    });
    let runtime = ResourceGroupRuntime {
        controller: controller.clone(),
        runaway_manager: Arc::new(Manager),
    };

    let initialized = runtime.initialize();

    assert!(controller.started.load(Ordering::SeqCst));
    assert_eq!(initialized.controller.config().server_id, 42);
}
