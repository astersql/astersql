// Copyright 2026 AsterSQL.

use std::sync::Arc;

/// Object-safe Domain boundary for the Go resource-group consumption reporter.
pub trait RUV2ConsumptionReporter: Send + Sync {
    fn report_ruv2_consumption(&self, resource_group: &str, tikv: f64, tidb: f64, tiflash: f64);
}

/// Bridges a concrete resource-group ConsumptionReporter into Domain storage.
pub struct ResourceGroupReporterBridge<R>(pub Arc<R>);

impl<R> RUV2ConsumptionReporter for ResourceGroupReporterBridge<R>
where
    R: astersql_resourcegroup::ConsumptionReporter + Send + Sync,
{
    fn report_ruv2_consumption(&self, resource_group: &str, tikv: f64, tidb: f64, tiflash: f64) {
        self.0
            .report_ruv2_consumption(resource_group, tikv, tidb, tiflash);
    }
}
