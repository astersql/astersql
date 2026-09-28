// Copyright 2026 AsterSQL.

use crate::{
    ImportResourceCalculator, KVSizeSamplerService, LoadDataController, ResourceParams,
    ScheduleTuneFactors,
};
use astersql_dxf_framework_handle as handle;
use astersql_dxf_framework_scheduler as scheduler;
use std::sync::Arc;

/// Uses the registered DXF host runtime for node CPU/tuning and Go RCCalc for sizing.
pub struct HandleImportResourceCalculator {
    pub Context: handle::Context,
    pub Sampler: Arc<dyn KVSizeSamplerService + Send + Sync>,
}

/// Host parser/encoder sampling remains pluggable while CPU, tuning, and RCCalc use DXF.
pub struct HostImportResourceCalculator {
    pub Handle: handle::Context,
    pub SampleService: Arc<dyn ImportResourceCalculator>,
}

impl ImportResourceCalculator for HostImportResourceCalculator {
    fn TargetNodeCPUCnt(&self) -> Result<usize, String> {
        let cpu = handle::GetCPUCountOfNode(&self.Handle).map_err(|error| error.to_string())?;
        usize::try_from(cpu).map_err(|_| format!("invalid node CPU count: {cpu}"))
    }
    fn ScheduleTuneFactors(&self, keyspace: &str) -> Result<ScheduleTuneFactors, String> {
        let factors = handle::GetScheduleTuneFactors(&self.Handle, keyspace)
            .map_err(|error| error.to_string())?;
        Ok(ScheduleTuneFactors {
            AmplifyFactor: factors.AmplifyFactor,
        })
    }
    fn SampleIndexSizeRatio(
        &self,
        controller: &LoadDataController,
        keyspace_codec: &[u8],
    ) -> Result<f64, String> {
        self.SampleService
            .SampleIndexSizeRatio(controller, keyspace_codec)
    }
    fn Calculate(
        &self,
        total_real_size: i64,
        target_node_cpu_count: usize,
        index_size_ratio: f64,
        factors: ScheduleTuneFactors,
    ) -> ResourceParams {
        calculate_go_import_resources(
            total_real_size,
            target_node_cpu_count,
            index_size_ratio,
            factors,
        )
    }
}

fn calculate_go_import_resources(
    total_real_size: i64,
    target_node_cpu_count: usize,
    index_size_ratio: f64,
    factors: ScheduleTuneFactors,
) -> ResourceParams {
    let cpu = i32::try_from(target_node_cpu_count).unwrap_or(i32::MAX);
    let tune = handle::schstatus::TuneFactors {
        AmplifyFactor: factors.AmplifyFactor,
    };
    let calc = scheduler::NewRCCalc(total_real_size, cpu, index_size_ratio, &tune);
    let threads = calc.required_slots();
    let nodes = calc.max_node_count_for_import_into();
    let dist_sql = scheduler::CalcDistSQLConcurrency(threads, nodes, cpu);
    ResourceParams {
        ThreadCnt: threads.max(0) as usize,
        MaxNodeCnt: nodes,
        DistSQLScanConcurrency: dist_sql.max(0) as usize,
    }
}

impl ImportResourceCalculator for HandleImportResourceCalculator {
    fn TargetNodeCPUCnt(&self) -> Result<usize, String> {
        let cpu = handle::GetCPUCountOfNode(&self.Context).map_err(|error| error.to_string())?;
        usize::try_from(cpu).map_err(|_| format!("invalid node CPU count: {cpu}"))
    }

    fn ScheduleTuneFactors(&self, keyspace: &str) -> Result<ScheduleTuneFactors, String> {
        let factors = handle::GetScheduleTuneFactors(&self.Context, keyspace)
            .map_err(|error| error.to_string())?;
        Ok(ScheduleTuneFactors {
            AmplifyFactor: factors.AmplifyFactor,
        })
    }

    fn SampleIndexSizeRatio(
        &self,
        controller: &LoadDataController,
        keyspace_codec: &[u8],
    ) -> Result<f64, String> {
        controller.sampleIndexSizeRatio(keyspace_codec, self.Sampler.as_ref())
    }

    fn Calculate(
        &self,
        total_real_size: i64,
        target_node_cpu_count: usize,
        index_size_ratio: f64,
        factors: ScheduleTuneFactors,
    ) -> ResourceParams {
        calculate_go_import_resources(
            total_real_size,
            target_node_cpu_count,
            index_size_ratio,
            factors,
        )
    }
}
