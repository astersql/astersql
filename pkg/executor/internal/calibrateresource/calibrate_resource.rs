// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// CALIBRATE RESOURCE 执行器与 RU 容量估算核心逻辑。
//
// 资源校准根据集群实例 CPU 配额与负载（TPCC/OLTP 等）估算集群 RU（Request Unit）
// 容量：静态校准用基准成本表，动态校准按历史 CPU/RU 时序采样并丢弃极端分位后取均值。
// 文件前半为 Go 逻辑的迁移草稿（整块注释），后半为可编译的精简实现。

// workloadBaseRUCostMap contains the base resource cost rate per 1 kv cpu within 1 second,
// the data is calculated from benchmark result, these data might not be very accurate,
// but is enough here because the maximum RU capacity is depended on both the cluster and
// the workload.
// pub fn workloadBaseRUCostMap() -> HashMap<ast::CalibrateResourceType, baseResourceCost> {
//     HashMap::from([
//         (ast::TPCC, baseResourceCost {
//             tidbToKVCPURatio: 0.6,
//             kvCPU: 0.15,
//             readBytes: units::MiB / 2,
//             writeBytes: units::MiB,
//             readReqCount: 300,
//             writeReqCount: 1750,
//         }),
//         (ast::OLTPREADWRITE, baseResourceCost {
//             tidbToKVCPURatio: 1.25,
//             kvCPU: 0.35,
//             readBytes: (units::MiB as f64 * 4.25) as u64,
//             writeBytes: units::MiB / 3,
//             readReqCount: 1600,
//             writeReqCount: 1400,
//         }),
//         (ast::OLTPREADONLY, baseResourceCost {
//             tidbToKVCPURatio: 2.0,
//             kvCPU: 0.52,
//             readBytes: units::MiB * 28,
//             writeBytes: 0,
//             readReqCount: 4500,
//             writeReqCount: 0,
//         }),
//         (ast::OLTPWRITEONLY, baseResourceCost {
//             tidbToKVCPURatio: 1.0,
//             kvCPU: 0.0,
//             readBytes: 0,
//             writeBytes: units::MiB,
//             readReqCount: 0,
//             writeReqCount: 3550,
//         }),
//     ])
// }
//
// serverTypeTiDB is tidb's instance type name
// pub const serverTypeTiDB: &str = "tidb";
// serverTypeTiKV is tikv's instance type name
// pub const serverTypeTiKV: &str = "tikv";
// serverTypeTiFlash is tiflash's instance type name
// pub const serverTypeTiFlash: &str = "tiflash";
//
// the resource cost rate of a specified workload per 1 tikv cpu.
// pub struct baseResourceCost {
// represents the average ratio of TiDB CPU time to TiKV CPU time, this is used to calculate whether tikv cpu
// or tidb cpu is the performance bottle neck.
//     pub tidbToKVCPURatio: f64,
// the kv CPU time for calculate RU, it's smaller than the actual cpu usage. The unit is seconds.
//     pub kvCPU: f64,
// the read bytes rate per 1 tikv cpu.
//     pub readBytes: u64,
// the write bytes rate per 1 tikv cpu.
//     pub writeBytes: u64,
// the average tikv read request count per 1 tikv cpu.
//     pub readReqCount: u64,
// the average tikv write request count per 1 tikv cpu.
//     pub writeReqCount: u64,
// }
//
// valuableUsageThreshold is the threshold used to determine whether the CPU is high enough.
// The sampling point is available when the CPU utilization of tikv or tidb is higher than the valuableUsageThreshold.
// pub const valuableUsageThreshold: f64 = 0.2;
// lowUsageThreshold is the threshold used to determine whether the CPU is too low.
// When the CPU utilization of tikv or tidb is lower than lowUsageThreshold, but neither is higher than valuableUsageThreshold, the sampling point is unavailable
// pub const lowUsageThreshold: f64 = 0.1;
// For quotas computed at each point in time, the maximum and minimum portions are discarded, and discardRate is the percentage discarded
// pub const discardRate: f64 = 0.1;
//
// duration Indicates the supported calibration duration
// pub const maxDuration: time::Duration = time::Hour * 24;
// pub const minDuration: time::Duration = time::Minute;
//
// Executor is used as executor of calibrate resource.
// pub struct Executor {
//     pub OptionList: Vec<*mut ast::DynamicCalibrateResourceOption>,
//     pub BaseExecutor: exec::BaseExecutor,
//     pub WorkloadType: ast::CalibrateResourceType,
//     pub done: bool,
// }
//
// impl Executor {
//     pub fn parseTsExpr(&mut self, ctx: context::Context, tsExpr: ast::ExprNode) -> Result<time::Time, errors::Error> {
//         let ts = staleread::CalculateAsOfTsExpr(ctx, self.BaseExecutor.Ctx().GetPlanCtx(), tsExpr)?;
//         Ok(oracle::GetTimeFromTS(ts))
//     }
//
//     pub fn parseCalibrateDuration(
//         &mut self,
//         ctx: context::Context,
//     ) -> Result<(time::Time, time::Time), errors::Error> {
//         let mut dur = time::Duration::default();
// startTimeExpr and endTimeExpr are used to calc endTime by FuncCallExpr when duration begin with `interval`.
//         let mut startTimeExpr: Option<ast::ExprNode> = None;
//         let mut endTimeExpr: Option<ast::ExprNode> = None;
//         let mut startTime = time::Time::default();
//         let mut endTime = time::Time::default();
//
//         for op in &self.OptionList {
//             match unsafe { (*(*op)).Tp } {
//                 ast::CalibrateStartTime => {
//                     startTimeExpr = Some(unsafe { (*(*op)).Ts.clone() });
//                     startTime = self.parseTsExpr(ctx.clone(), startTimeExpr.clone().unwrap())?;
//                 }
//                 ast::CalibrateEndTime => {
//                     endTimeExpr = Some(unsafe { (*(*op)).Ts.clone() });
//                     endTime = self.parseTsExpr(ctx.clone(), unsafe { (*(*op)).Ts.clone() })?;
//                 }
//                 _ => {}
//             }
//         }
//
//         for op in &self.OptionList {
//             if unsafe { (*(*op)).Tp } != ast::CalibrateDuration {
//                 continue;
//             }
// string duration
//             if !unsafe { (*(*op)).StrValue.is_empty() } {
//                 dur = duration::ParseDuration(unsafe { (*(*op)).StrValue.clone() })?;
// If startTime is not set, startTime will be now() - duration.
//                 if startTime.IsZero() {
//                     let mut toTime = endTime;
//                     if toTime.IsZero() {
//                         toTime = time::Now();
//                     }
//                     startTime = toTime.Add(-dur);
//                 }
// If endTime is set, duration will be ignored.
//                 if endTime.IsZero() {
//                     endTime = startTime.Add(dur);
//                 }
//                 continue;
//             }
//
// interval duration
// If startTime is not set, startTime will be now() - duration.
//             if startTimeExpr.is_none() {
//                 let mut toTimeExpr = endTimeExpr.clone();
//                 if endTime.IsZero() {
//                     toTimeExpr = Some(ast::FuncCallExpr {
//                         FnName: ast::NewCIStr("CURRENT_TIMESTAMP"),
//                         Args: vec![],
//                     }.into());
//                 }
//                 startTimeExpr = Some(ast::FuncCallExpr {
//                     FnName: ast::NewCIStr("DATE_SUB"),
//                     Args: vec![toTimeExpr.unwrap(), unsafe { (*(*op)).Ts.clone() }, ast::TimeUnitExpr { Unit: unsafe { (*(*op)).Unit } }.into()],
//                 }.into());
//                 startTime = self.parseTsExpr(ctx.clone(), startTimeExpr.clone().unwrap())?;
//             }
// If endTime is set, duration will be ignored.
//             if endTime.IsZero() {
//                 endTime = self.parseTsExpr(ctx.clone(), ast::FuncCallExpr {
//                     FnName: ast::NewCIStr("DATE_ADD"),
//                     Args: vec![startTimeExpr.clone().unwrap(), unsafe { (*(*op)).Ts.clone() }, ast::TimeUnitExpr { Unit: unsafe { (*(*op)).Unit } }.into()],
//                 }.into())?;
//             }
//         }
//
//         if startTime.IsZero() {
//             return Err(errors::Errorf("start time should not be 0"));
//         }
//         if endTime.IsZero() {
//             endTime = time::Now();
//         }
// check the duration
//         dur = endTime.Sub(startTime);
// add the buffer duration
//         if dur > maxDuration + time::Minute {
//             return Err(errors::Errorf(format!(
//                 "the duration of calibration is too long, which could lead to inaccurate output. Please make the duration between {} and {}",
//                 minDuration.String(), maxDuration.String()
//             )));
//         }
// We only need to consider the case where the duration is slightly enlarged.
//         if dur < minDuration {
//             return Err(errors::Errorf(format!(
//                 "the duration of calibration is too short, which could lead to inaccurate output. Please make the duration between {} and {}",
//                 minDuration.String(), maxDuration.String()
//             )));
//         }
//         Ok((startTime, endTime))
//     }
//
// Next implements the interface of Executor.
//     pub fn Next(&mut self, ctx: context::Context, req: *mut chunk::Chunk) -> Result<(), errors::Error> {
//         unsafe { (*req).Reset() };
//         if self.done {
//             return Ok(());
//         }
//         self.done = true;
//         if !vardef::EnableResourceControl.Load() {
//             return Err(infoschema::ErrResourceGroupSupportDisabled);
//         }
//         let ctx = kv::WithInternalSourceType(ctx, kv::InternalTxnOthers);
//         if !self.OptionList.is_empty() {
//             return self.dynamicCalibrate(ctx, req);
//         }
//         self.staticCalibrate(req)
//     }
//
//     pub fn dynamicCalibrate(&mut self, ctx: context::Context, req: *mut chunk::Chunk) -> Result<(), errors::Error> {
//         let restricted_exec = self.BaseExecutor.Ctx().GetRestrictedSQLExecutor();
//         let (startTs, endTs) = self.parseCalibrateDuration(ctx.clone())?;
//         let clusterInfo = infoschema::GetClusterServerInfo(self.BaseExecutor.Ctx())?;
//         let tidbQuota = self.getTiDBQuota(ctx.clone(), restricted_exec.clone(), clusterInfo.clone(), startTs, endTs);
//         let tiflashQuota = self.getTiFlashQuota(ctx, restricted_exec, clusterInfo, startTs, endTs);
//         if tidbQuota.is_err() && tiflashQuota.is_err() {
//             return tidbQuota.map(|_| ()).map_err(|err| err);
//         }
// Go 只在 TiDB/TiFlash quota 都失败时返回错误；成功部分以 0 兜底参与相加。
//         let total = tidbQuota.unwrap_or(0.0) + tiflashQuota.unwrap_or(0.0);
//         unsafe { (*req).AppendUint64(0, total as u64) };
//         Ok(())
//     }
//
//     pub fn getTiDBQuota(
//         &mut self,
//         ctx: context::Context,
//         restricted_exec: sqlexec::RestrictedSQLExecutor,
//         serverInfos: Vec<infoschema::ServerInfo>,
//         startTs: time::Time,
//         endTs: time::Time,
//     ) -> Result<f64, errors::Error> {
//         let startTime = startTs.In(self.BaseExecutor.Ctx().GetSessionVars().Location()).Format(time::DateTime);
//         let endTime = endTs.In(self.BaseExecutor.Ctx().GetSessionVars().Location()).Format(time::DateTime);
//
//         let totalKVCPUQuota = getTiKVTotalCPUQuota(serverInfos.clone())
//             .map_err(|err| errNoCPUQuotaMetrics().FastGenByArgs(err.Error()))?;
//         let totalTiDBCPU = getTiDBTotalCPUQuota(serverInfos.clone())
//             .map_err(|err| errNoCPUQuotaMetrics().FastGenByArgs(err.Error()))?;
//         let mut rus = getRUPerSec(ctx.clone(), self.BaseExecutor.Ctx(), restricted_exec.clone(), startTime.clone(), endTime.clone())?;
//         let mut tikvCPUs = getComponentCPUUsagePerSec(ctx.clone(), self.BaseExecutor.Ctx(), restricted_exec.clone(), "tikv", startTime.clone(), endTime.clone())?;
//         let mut tidbCPUs = getComponentCPUUsagePerSec(ctx, self.BaseExecutor.Ctx(), restricted_exec, "tidb", startTime, endTime)?;
//
//         failpoint::Inject("mockMetricsDataFilter", || {
// 测试 failpoint 会过滤时间窗口外的样本；保留三组时序值同步过滤的意图。
//             rus.vals.retain(|point| !point.tp.After(endTs) && !point.tp.Before(startTs));
//             tikvCPUs.vals.retain(|point| !point.tp.After(endTs) && !point.tp.Before(startTs));
//             tidbCPUs.vals.retain(|point| !point.tp.After(endTs) && !point.tp.Before(startTs));
//         });
//
//         let mut quotas: Vec<f64> = Vec::new();
//         let mut _lowCount = 0;
//         loop {
//             if rus.isEnd() || tikvCPUs.isEnd() || tidbCPUs.isEnd() {
//                 break;
//             }
// make time point match
//             let mut maxTime = rus.getTime();
//             if tikvCPUs.getTime().After(maxTime) {
//                 maxTime = tikvCPUs.getTime();
//             }
//             if tidbCPUs.getTime().After(maxTime) {
//                 maxTime = tidbCPUs.getTime();
//             }
//             if !rus.advance(maxTime) || !tikvCPUs.advance(maxTime) || !tidbCPUs.advance(maxTime) {
//                 continue;
//             }
//             let tikvQuota = tikvCPUs.getValue() / totalKVCPUQuota;
//             let tidbQuota = tidbCPUs.getValue() / totalTiDBCPU;
// If one of the two cpu usage is greater than the `valuableUsageThreshold`, we can accept it.
// And if both are greater than the `lowUsageThreshold`, we can also accept it.
//             if tikvQuota > valuableUsageThreshold || tidbQuota > valuableUsageThreshold {
//                 quotas.push(rus.getValue() / tikvQuota.max(tidbQuota));
//             } else if tikvQuota < lowUsageThreshold || tidbQuota < lowUsageThreshold {
//                 _lowCount += 1;
//             } else {
//                 quotas.push(rus.getValue() / tikvQuota.max(tidbQuota));
//             }
//             rus.next();
//             tidbCPUs.next();
//             tikvCPUs.next();
//         }
//         setupQuotas(quotas)
//     }
//
//     pub fn getTiFlashQuota(
//         &mut self,
//         ctx: context::Context,
//         restricted_exec: sqlexec::RestrictedSQLExecutor,
//         serverInfos: Vec<infoschema::ServerInfo>,
//         startTs: time::Time,
//         endTs: time::Time,
//     ) -> Result<f64, errors::Error> {
//         let startTime = startTs.In(self.BaseExecutor.Ctx().GetSessionVars().Location()).Format(time::DateTime);
//         let endTime = endTs.In(self.BaseExecutor.Ctx().GetSessionVars().Location()).Format(time::DateTime);
//
//         let mut quotas: Vec<f64> = Vec::new();
//         let totalTiFlashLogicalCores = getTiFlashLogicalCores(serverInfos)
//             .map_err(|err| errNoCPUQuotaMetrics().FastGenByArgs(err.Error()))?;
//         let mut tiflashCPUs = getTiFlashCPUUsagePerSec(ctx.clone(), self.BaseExecutor.Ctx(), restricted_exec.clone(), startTime.clone(), endTime.clone())?;
//         let mut tiflashRUs = getTiFlashRUPerSec(ctx, self.BaseExecutor.Ctx(), restricted_exec, startTime, endTime)?;
//         loop {
//             if tiflashRUs.isEnd() || tiflashCPUs.isEnd() {
//                 break;
//             }
// make time point match
//             let mut maxTime = tiflashRUs.getTime();
//             if tiflashCPUs.getTime().After(maxTime) {
//                 maxTime = tiflashCPUs.getTime();
//             }
//             if !tiflashRUs.advance(maxTime) || !tiflashCPUs.advance(maxTime) {
//                 continue;
//             }
//             let tiflashQuota = tiflashCPUs.getValue() / totalTiFlashLogicalCores;
//             if tiflashQuota > lowUsageThreshold {
//                 quotas.push(tiflashRUs.getValue() / tiflashQuota);
//             }
//             tiflashRUs.next();
//             tiflashCPUs.next();
//         }
//         setupQuotas(quotas)
//     }
//
//     pub fn staticCalibrate(&mut self, req: *mut chunk::Chunk) -> Result<(), errors::Error> {
//         let resourceGroupCtl = domain::GetDomain(self.BaseExecutor.Ctx()).ResourceGroupsController();
// first fetch the ru settings config.
//         if resourceGroupCtl.is_none() {
//             return Err(errors::New("resource group controller is not initialized"));
//         }
//         let clusterInfo = infoschema::GetClusterServerInfo(self.BaseExecutor.Ctx())?;
//         let ruCfg = resourceGroupCtl.unwrap().GetConfig();
//         if self.WorkloadType == ast::TPCH10 {
//             return staticCalibrateTpch10(req, clusterInfo, ruCfg);
//         }
//
//         let mut totalKVCPUQuota = getTiKVTotalCPUQuota(clusterInfo.clone())
//             .map_err(|err| errNoCPUQuotaMetrics().FastGenByArgs(err.Error()))?;
//         let totalTiDBCPUQuota = getTiDBTotalCPUQuota(clusterInfo)
//             .map_err(|err| errNoCPUQuotaMetrics().FastGenByArgs(err.Error()))?;
//
// The default workload to calculate the RU capacity.
//         if self.WorkloadType == ast::WorkloadNone {
//             self.WorkloadType = ast::TPCC;
//         }
//         let costs = workloadBaseRUCostMap();
//         let baseCost = costs.get(&self.WorkloadType)
//             .ok_or_else(|| errors::Errorf(format!("unknown workload '{:?}'", self.WorkloadType)))?;
//
//         if totalTiDBCPUQuota / baseCost.tidbToKVCPURatio < totalKVCPUQuota {
//             totalKVCPUQuota = totalTiDBCPUQuota / baseCost.tidbToKVCPURatio;
//         }
//         let ruPerKVCPU = ruCfg.ReadBaseCost as f64 * baseCost.readReqCount as f64
//             + ruCfg.CPUMsCost as f64 * baseCost.kvCPU * 1000.0 // convert to ms
//             + ruCfg.ReadBytesCost as f64 * baseCost.readBytes as f64
//             + ruCfg.WriteBaseCost as f64 * baseCost.writeReqCount as f64
//             + ruCfg.WriteBytesCost as f64 * baseCost.writeBytes as f64;
//         let quota = totalKVCPUQuota * ruPerKVCPU;
//         unsafe { (*req).AppendUint64(0, quota as u64) };
//         Ok(())
//     }
// }
//
// pub fn errLowUsage() -> errors::Error {
//     errors::Errorf("The workload in selected time window is too low, with which TiDB is unable to reach a capacity estimation; please select another time window with higher workload, or calibrate resource by hardware instead")
// }
//
// pub fn errNoCPUQuotaMetrics() -> errors::NormalizedError {
//     errors::Normalize("There is no CPU quota metrics, %v")
// }
//
// pub fn setupQuotas(mut quotas: Vec<f64>) -> Result<f64, errors::Error> {
//     if quotas.len() < 2 {
//         return Err(errLowUsage());
//     }
//     quotas.sort_by(|a, b| b.partial_cmp(a).unwrap());
//     let lowerBound = math::Round(quotas.len() as f64 * discardRate) as usize;
//     let upperBound = quotas.len() - lowerBound;
//     let mut sum = 0.0;
//     for i in lowerBound..upperBound {
//         sum += quotas[i];
//     }
//     Ok(sum / (upperBound - lowerBound) as f64)
// }
//
// pub fn staticCalibrateTpch10(
//     req: *mut chunk::Chunk,
//     clusterInfo: Vec<infoschema::ServerInfo>,
//     ruCfg: *mut resourceControlClient::RUConfig,
// ) -> Result<(), errors::Error> {
// TPCH10 only considers the resource usage of the TiFlash including cpu and read bytes. Others are ignored.
// cpu usage: 105494.666484 / 20 / 20 = 263.74
// read bytes: 401799161689.0 / 20 / 20 = 1004497904.22
//     const cpuTimePerCPUPerSec: f64 = 263.74;
//     const readBytesPerCPUPerSec: f64 = 1004497904.22;
//     let ruPerCPU = unsafe { (*ruCfg).CPUMsCost } as f64 * cpuTimePerCPUPerSec
//         + unsafe { (*ruCfg).ReadBytesCost } as f64 * readBytesPerCPUPerSec;
//     let totalTiFlashLogicalCores = getTiFlashLogicalCores(clusterInfo)?;
//     let quota = totalTiFlashLogicalCores * ruPerCPU;
//     unsafe { (*req).AppendUint64(0, quota as u64) };
//     Ok(())
// }
//
// pub fn getTiDBTotalCPUQuota(clusterInfo: Vec<infoschema::ServerInfo>) -> Result<f64, errors::Error> {
//     let mut cpuQuota = runtime::GOMAXPROCS(0) as f64;
//     failpoint::Inject("mockGOMAXPROCS", |val| {
//         if !val.is_nil() {
//             cpuQuota = val.as_i32() as f64;
//         }
//     });
//     let instanceNum = count(clusterInfo, serverTypeTiDB);
//     Ok(cpuQuota * instanceNum as f64)
// }
//
// pub fn getTiKVTotalCPUQuota(clusterInfo: Vec<infoschema::ServerInfo>) -> Result<f64, errors::Error> {
//     let instanceNum = count(clusterInfo.clone(), serverTypeTiKV);
//     if instanceNum == 0 {
//         return Err(errors::New("no server with type 'tikv' is found"));
//     }
//     let cpuQuota = fetchServerCPUQuota(clusterInfo, serverTypeTiKV, "tikv_server_cpu_cores_quota")?;
//     Ok(cpuQuota * instanceNum as f64)
// }
//
// pub fn getTiFlashLogicalCores(clusterInfo: Vec<infoschema::ServerInfo>) -> Result<f64, errors::Error> {
//     let instanceNum = count(clusterInfo.clone(), serverTypeTiFlash);
//     if instanceNum == 0 {
//         return Ok(0.0);
//     }
//     let cpuQuota = fetchServerCPUQuota(clusterInfo, serverTypeTiFlash, "tiflash_proxy_tikv_server_cpu_cores_quota")?;
//     Ok(cpuQuota * instanceNum as f64)
// }
//
// pub fn getTiFlashRUPerSec(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     restricted_exec: sqlexec::RestrictedSQLExecutor,
//     startTime: String,
//     endTime: String,
// ) -> Result<timeSeriesValues, errors::Error> {
//     let query = format!("SELECT time, value FROM METRICS_SCHEMA.tiflash_resource_manager_resource_unit where time >= '{}' and time <= '{}' ORDER BY time asc", startTime, endTime);
//     getValuesFromMetrics(ctx, sctx, restricted_exec, query)
// }
//
// pub fn getTiFlashCPUUsagePerSec(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     restricted_exec: sqlexec::RestrictedSQLExecutor,
//     startTime: String,
//     endTime: String,
// ) -> Result<timeSeriesValues, errors::Error> {
//     let query = format!("SELECT time, sum(value) FROM METRICS_SCHEMA.tiflash_process_cpu_usage where time >= '{}' and time <= '{}' and job = 'tiflash' GROUP BY time ORDER BY time asc", startTime, endTime);
//     getValuesFromMetrics(ctx, sctx, restricted_exec, query)
// }
//
// pub struct timePointValue {
//     pub tp: time::Time,
//     pub val: f64,
// }
//
// pub struct timeSeriesValues {
//     pub vals: Vec<timePointValue>,
//     pub idx: usize,
// }
//
// impl timeSeriesValues {
//     pub fn isEnd(&self) -> bool { self.idx >= self.vals.len() }
//     pub fn next(&mut self) { self.idx += 1; }
//     pub fn getTime(&self) -> time::Time { self.vals[self.idx].tp }
//     pub fn getValue(&self) -> f64 { self.vals[self.idx].val }
//
//     pub fn advance(&mut self, target: time::Time) -> bool {
//         while self.idx < self.vals.len() {
// `target` is maximal time in other timeSeriesValues,
// so we should find the time which offset is less than 10s.
//             if self.vals[self.idx].tp.Add(time::Second * 10).After(target) {
//                 return self.vals[self.idx].tp.Add(-time::Second * 10).Before(target);
//             }
//             self.idx += 1;
//         }
//         false
//     }
// }
//
// pub fn getRUPerSec(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     restricted_exec: sqlexec::RestrictedSQLExecutor,
//     startTime: String,
//     endTime: String,
// ) -> Result<timeSeriesValues, errors::Error> {
//     let query = format!("SELECT time, value FROM METRICS_SCHEMA.resource_manager_resource_unit where time >= '{}' and time <= '{}' ORDER BY time asc", startTime, endTime);
//     getValuesFromMetrics(ctx, sctx, restricted_exec, query)
// }
//
// pub fn getComponentCPUUsagePerSec(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     restricted_exec: sqlexec::RestrictedSQLExecutor,
//     component: &str,
//     startTime: String,
//     endTime: String,
// ) -> Result<timeSeriesValues, errors::Error> {
//     let query = format!("SELECT time, sum(value) FROM METRICS_SCHEMA.process_cpu_usage where time >= '{}' and time <= '{}' and job like '%{}' GROUP BY time ORDER BY time asc", startTime, endTime, component);
//     getValuesFromMetrics(ctx, sctx, restricted_exec, query)
// }
//
// pub fn getValuesFromMetrics(
//     ctx: context::Context,
//     sctx: sessionctx::Context,
//     restricted_exec: sqlexec::RestrictedSQLExecutor,
//     query: String,
// ) -> Result<timeSeriesValues, errors::Error> {
//     let (rows, _, _) = restricted_exec.ExecRestrictedSQL(ctx, vec![sqlexec::ExecOptionUseCurSession], query)
//         .map_err(errors::Trace)?;
//     let mut ret: Vec<timePointValue> = Vec::with_capacity(rows.len());
//     for row in rows {
// Go 忽略单行时间解析错误；也只收集可成功转换的采样点。
//         if let Ok(tp) = row.GetTime(0).AdjustedGoTime(sctx.GetSessionVars().Location()) {
//             ret.push(timePointValue { tp, val: row.GetFloat64(1) });
//         }
//     }
//     Ok(timeSeriesValues { idx: 0, vals: ret })
// }
//
// pub fn count(clusterInfo: Vec<infoschema::ServerInfo>, ty: &str) -> i32 {
//     let mut num = 0;
//     for e in clusterInfo {
//         if e.ServerType == ty {
//             num += 1;
//         }
//     }
//     num
// }
//
// pub fn fetchServerCPUQuota(
//     serverInfos: Vec<infoschema::ServerInfo>,
//     serverType: &str,
//     metricName: &str,
// ) -> Result<f64, errors::Error> {
//     let mut cpuQuota = 0.0;
//     let err = fetchStoreMetrics(serverInfos, serverType, |addr, resp| {
//         if resp.StatusCode != http::StatusOK {
//             return Err(errors::Errorf(format!("request {} failed: {}", addr, resp.Status)));
//         }
//         let scanner = bufio::NewScanner(resp.Body);
//         for line in scanner.lines() {
//             if !strings::HasPrefix(&line, metricName) {
//                 continue;
//             }
// the metrics format is like following:
// tikv_server_cpu_cores_quota 8
//             let parsed = strconv::ParseFloat(&line[metricName.len() + 1..], 64);
//             if let Ok(quota) = parsed {
//                 cpuQuota = quota;
//             }
//             return parsed.map(|_| ()).map_err(errors::Trace);
//         }
//         Err(errors::Errorf(format!("metrics '{}' not found from server '{}'", metricName, addr)))
//     });
//     err.map(|_| cpuQuota)
// }
//
// pub fn fetchStoreMetrics<F>(
//     serversInfo: Vec<infoschema::ServerInfo>,
//     serverType: &str,
//     mut onResp: F,
// ) -> Result<(), errors::Error>
// where
//     F: FnMut(String, http::Response) -> Result<(), errors::Error>,
// {
//     let mut firstErr: Option<errors::Error> = None;
//     for srv in serversInfo {
//         if srv.ServerType != serverType {
//             continue;
//         }
//         if srv.StatusAddr.is_empty() {
//             continue;
//         }
//         let url = format!("{}://{}/metrics", util::InternalHTTPSchema(), srv.StatusAddr);
//         let req = http::NewRequest(http::MethodGet, url, None)?;
//         let mut resp: Option<http::Response> = None;
//         failpoint::Inject("mockMetricsResponse", |val| {
//             if !val.is_nil() {
//                 let data = base64::StdEncoding.DecodeString(val.as_string()).unwrap_or_default();
//                 resp = Some(http::Response {
//                     StatusCode: http::StatusOK,
//                     Body: noopCloserWrapper { Reader: strings::NewReader(String::from_utf8_lossy(&data).to_string()) },
//                     ..Default::default()
//                 });
//             }
//         });
//         if resp.is_none() {
// ignore false positive go line, can't use defer here because it's in a loop.
//nolint:bodyclose
//             match util::InternalHTTPClient().Do(req) {
//                 Ok(real_resp) => resp = Some(real_resp),
//                 Err(err1) => {
//                     if firstErr.is_none() {
//                         firstErr = Some(err1);
//                     }
//                     continue;
//                 }
//             }
//         }
//         let mut resp = resp.unwrap();
//         let err = onResp(srv.Address, resp.clone());
// Go 在回调之后立即 Close body；这里显式保留循环内资源收尾点。
//         resp.Body.Close();
//         return err;
//     }
//     if firstErr.is_none() {
//         firstErr = Some(errors::Errorf(format!("no server with type '{}' is found", serverType)));
//     }
//     Err(firstErr.unwrap())
// }
//
// pub struct noopCloserWrapper {
//     pub Reader: io::Reader,
// }
//
// impl noopCloserWrapper {
//     pub fn Close(&self) -> Result<(), errors::Error> {
//         Ok(())
//     }
// }
// */
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// 判定采样点“有价值”的 CPU 利用率下限：TiKV 或 TiDB 任一超过该值即可采用。
pub const VALUABLE_USAGE_THRESHOLD: f64 = 0.2;
/// 双方都低于该利用率且无一达到有价值阈值时，采样点不可用。
pub const LOW_USAGE_THRESHOLD: f64 = 0.1;
/// 对按时间点算出的配额序列，两端各丢弃该比例后再取平均。
pub const DISCARD_RATE: f64 = 0.1;
/// 校准时间窗口下限（1 分钟）。
pub const MIN_DURATION: Duration = Duration::from_secs(60);
/// 校准时间窗口上限（24 小时）。
pub const MAX_DURATION: Duration = Duration::from_secs(24 * 3600);

/// 校准所用的工作负载类型；`None` 在静态校准时默认落到 TPCC。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkloadType {
    None,
    Tpcc,
    OltpReadWrite,
    OltpReadOnly,
    OltpWriteOnly,
    Tpch10,
}
/// 每 1 个 TiKV CPU 在指定工作负载下的基准资源消耗率（来自压测近似值）。
#[derive(Clone, Copy, Debug)]
pub struct BaseResourceCost {
    /// TiDB CPU 与 TiKV CPU 的平均比值，用于判断瓶颈在哪一侧。
    pub tidb_to_kv_cpu_ratio: f64,
    /// 计入 RU 的 KV CPU 秒数（通常小于实际 CPU 占用）。
    pub kv_cpu: f64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_requests: u64,
    pub write_requests: u64,
}
/// RU 单价配置：读写请求基数、CPU 毫秒、读写字节成本。
#[derive(Clone, Copy, Debug)]
pub struct RuConfig {
    pub read_base_cost: f64,
    pub cpu_ms_cost: f64,
    pub read_bytes_cost: f64,
    pub write_base_cost: f64,
    pub write_bytes_cost: f64,
}
/// 集群中单个实例的类型与 CPU 核数配额。
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub server_type: String,
    pub cpu_cores: f64,
}
/// 动态校准用的时间序列采样点。
#[derive(Clone, Debug)]
pub struct TimePointValue {
    pub timestamp: SystemTime,
    pub value: f64,
}

/// 按时间升序消费的采样序列，对应 Go 的 `timeSeriesValues`。
///
/// `advance` 的窗口是严格小于 10 秒，且会跳过早于目标窗口的样本；这是
/// 动态校准把 RU、TiKV CPU 和 TiDB CPU 三条时序关联起来的关键语义。
#[derive(Clone, Debug)]
pub struct TimeSeriesValues {
    pub values: Vec<TimePointValue>,
    pub index: usize,
}

impl TimeSeriesValues {
    pub fn new(mut values: Vec<TimePointValue>) -> Self {
        values.sort_by_key(|point| point.timestamp);
        Self { values, index: 0 }
    }

    pub fn is_end(&self) -> bool {
        self.index >= self.values.len()
    }

    pub fn next(&mut self) {
        self.index += 1;
    }

    pub fn get_time(&self) -> SystemTime {
        self.values[self.index].timestamp
    }

    pub fn get_value(&self) -> f64 {
        self.values[self.index].value
    }

    /// 将当前点移动到目标时间附近；边界 10 秒与 Go 的严格比较保持一致。
    pub fn advance(&mut self, target: SystemTime) -> bool {
        const MATCH_WINDOW: Duration = Duration::from_secs(10);
        while self.index < self.values.len() {
            let timestamp = self.values[self.index].timestamp;
            let after_lower_bound = timestamp
                .checked_add(MATCH_WINDOW)
                .is_none_or(|upper_bound| upper_bound > target);
            if after_lower_bound {
                return timestamp
                    .checked_sub(MATCH_WINDOW)
                    .is_none_or(|lower_bound| lower_bound < target);
            }
            self.index += 1;
        }
        false
    }
}

/// 返回各工作负载的基准成本表（单位：每 1 TiKV CPU）。
pub fn workload_costs() -> BTreeMap<WorkloadType, BaseResourceCost> {
    let mib = 1024 * 1024;
    BTreeMap::from([
        (
            WorkloadType::Tpcc,
            BaseResourceCost {
                tidb_to_kv_cpu_ratio: 0.6,
                kv_cpu: 0.15,
                read_bytes: mib / 2,
                write_bytes: mib,
                read_requests: 300,
                write_requests: 1750,
            },
        ),
        (
            WorkloadType::OltpReadWrite,
            BaseResourceCost {
                tidb_to_kv_cpu_ratio: 1.25,
                kv_cpu: 0.35,
                read_bytes: mib * 17 / 4,
                write_bytes: mib / 3,
                read_requests: 1600,
                write_requests: 1400,
            },
        ),
        (
            WorkloadType::OltpReadOnly,
            BaseResourceCost {
                tidb_to_kv_cpu_ratio: 2.0,
                kv_cpu: 0.52,
                read_bytes: mib * 28,
                write_bytes: 0,
                read_requests: 4500,
                write_requests: 0,
            },
        ),
        (
            WorkloadType::OltpWriteOnly,
            BaseResourceCost {
                tidb_to_kv_cpu_ratio: 1.0,
                kv_cpu: 0.0,
                read_bytes: 0,
                write_bytes: mib,
                read_requests: 0,
                write_requests: 3550,
            },
        ),
    ])
}

/// 解析校准起止时间：可由显式起止或 duration 推导，并校验窗口长度。
pub fn parse_calibrate_duration(
    start: Option<SystemTime>,
    end: Option<SystemTime>,
    duration: Option<Duration>,
    now: SystemTime,
) -> Result<(SystemTime, SystemTime), String> {
    // 优先使用显式 start；否则用 end/now 减去 duration。
    let start = match (start, duration) {
        (Some(start), _) => start,
        (None, Some(duration)) => end
            .unwrap_or(now)
            .checked_sub(duration)
            .ok_or_else(|| "calibration start underflow".to_string())?,
        (None, None) => return Err("start time should not be 0".to_string()),
    };
    let end = end
        .or_else(|| duration.and_then(|duration| start.checked_add(duration)))
        .unwrap_or(now);
    let elapsed = end
        .duration_since(start)
        .map_err(|_| "calibration end precedes start".to_string())?;
    if elapsed < MIN_DURATION {
        return Err("the duration of calibration is too short".to_string());
    }
    // 允许比上限略放大 1 分钟缓冲，与 Go 行为一致。
    if elapsed > MAX_DURATION + Duration::from_secs(60) {
        return Err("the duration of calibration is too long".to_string());
    }
    Ok((start, end))
}

/// 解析 Go `duration.ParseDuration` 在校准语句中使用的常见时长格式。
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return Err(format!("invalid duration {value:?}"));
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    let mut rest = value;
    let mut seconds = 0.0;
    let mut parsed = false;
    while !rest.is_empty() {
        let number_end = rest
            .find(|character: char| !character.is_ascii_digit() && character != '.')
            .ok_or_else(|| format!("invalid duration {value:?}"))?;
        if number_end == 0 {
            return Err(format!("invalid duration {value:?}"));
        }
        let number: f64 = rest[..number_end]
            .parse()
            .map_err(|_| format!("invalid duration {value:?}"))?;
        rest = &rest[number_end..];
        let (unit, multiplier) = [
            ("ns", 1e-9),
            ("us", 1e-6),
            ("µs", 1e-6),
            ("μs", 1e-6),
            ("ms", 1e-3),
            ("s", 1.0),
            ("m", 60.0),
            ("h", 3600.0),
        ]
        .into_iter()
        .find(|(unit, _)| rest.starts_with(unit))
        .ok_or_else(|| format!("invalid duration {value:?}"))?;
        seconds += number * multiplier;
        rest = &rest[unit.len()..];
        parsed = true;
    }
    if !parsed || !seconds.is_finite() {
        return Err(format!("invalid duration {value:?}"));
    }
    Ok(Duration::from_secs_f64(seconds))
}

/// 字符串 duration 版本的校准时间解析入口，对应 Go executor 的第二阶段解析。
pub fn parse_calibrate_duration_text(
    start: Option<SystemTime>,
    end: Option<SystemTime>,
    duration: Option<&str>,
    now: SystemTime,
) -> Result<(SystemTime, SystemTime), String> {
    let duration = duration.map(parse_duration).transpose()?;
    parse_calibrate_duration(start, end, duration, now)
}

/// 对配额样本降序排序后丢弃两端极端值，再对剩余样本取平均。
pub fn setup_quotas(mut quotas: Vec<f64>) -> Result<f64, String> {
    if quotas.len() < 2 {
        return Err("workload in selected time window is too low".to_string());
    }
    // Go 的 `sort.Slice` 只按数值降序排列，不会在这里额外过滤样本。
    // 采样值由 metrics 查询产生，过滤应由上游时间/利用率逻辑完成。
    quotas.sort_by(|left, right| right.partial_cmp(left).unwrap_or(std::cmp::Ordering::Equal));
    let discard = (quotas.len() as f64 * DISCARD_RATE).round() as usize;
    let upper = quotas.len().saturating_sub(discard);
    let values = quotas
        .get(discard..upper)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| "no quota samples remain".to_string())?;
    Ok(values.iter().sum::<f64>() / values.len() as f64)
}

/// 动态估算 TiDB/TiKV 侧可用 RU 配额：按同时刻 CPU 利用率过滤采样点后归一化。
pub fn dynamic_tidb_quota(
    ru: &[TimePointValue],
    tikv_cpu: &[TimePointValue],
    tidb_cpu: &[TimePointValue],
    tikv_cores: f64,
    tidb_cores: f64,
) -> Result<f64, String> {
    if tikv_cores <= 0.0 || tidb_cores <= 0.0 {
        return Err("there is no CPU quota metrics".to_string());
    }
    let mut quotas = Vec::new();
    let mut rus = TimeSeriesValues::new(ru.to_vec());
    let mut tikv_cpus = TimeSeriesValues::new(tikv_cpu.to_vec());
    let mut tidb_cpus = TimeSeriesValues::new(tidb_cpu.to_vec());
    while !rus.is_end() && !tikv_cpus.is_end() && !tidb_cpus.is_end() {
        let max_time = rus
            .get_time()
            .max(tikv_cpus.get_time())
            .max(tidb_cpus.get_time());
        if !rus.advance(max_time) || !tikv_cpus.advance(max_time) || !tidb_cpus.advance(max_time) {
            continue;
        }
        let kv_ratio = tikv_cpus.get_value() / tikv_cores;
        let db_ratio = tidb_cpus.get_value() / tidb_cores;
        if kv_ratio > VALUABLE_USAGE_THRESHOLD
            || db_ratio > VALUABLE_USAGE_THRESHOLD
            || (kv_ratio >= LOW_USAGE_THRESHOLD && db_ratio >= LOW_USAGE_THRESHOLD)
        {
            quotas.push(rus.get_value() / kv_ratio.max(db_ratio));
        }
        rus.next();
        tikv_cpus.next();
        tidb_cpus.next();
    }
    setup_quotas(quotas)
}

/// 动态估算 TiFlash 侧 RU 配额；无 CPU 配额时返回 0。
pub fn dynamic_tiflash_quota(
    ru: &[TimePointValue],
    cpu: &[TimePointValue],
    cores: f64,
) -> Result<f64, String> {
    if cores <= 0.0 {
        return setup_quotas(Vec::new());
    }
    let mut rus = TimeSeriesValues::new(ru.to_vec());
    let mut cpus = TimeSeriesValues::new(cpu.to_vec());
    let mut quotas = Vec::new();
    while !rus.is_end() && !cpus.is_end() {
        let max_time = rus.get_time().max(cpus.get_time());
        if !rus.advance(max_time) || !cpus.advance(max_time) {
            continue;
        }
        let ratio = cpus.get_value() / cores;
        if ratio > LOW_USAGE_THRESHOLD {
            quotas.push(rus.get_value() / ratio);
        }
        rus.next();
        cpus.next();
    }
    setup_quotas(quotas)
}

/// 静态校准：按工作负载基准成本与集群 CPU 核数估算 RU 容量。
pub fn static_calibrate(
    mut workload: WorkloadType,
    servers: &[ServerInfo],
    config: RuConfig,
    local_tidb_cores: f64,
) -> Result<u64, String> {
    let count = |kind: &str| {
        servers
            .iter()
            .filter(|server| server.server_type == kind)
            .count()
    };
    // TPCH10 走 TiFlash 专用公式。
    if workload == WorkloadType::Tpch10 {
        let count = count("tiflash");
        let cores = servers
            .iter()
            .find(|server| server.server_type == "tiflash")
            .map_or(0.0, |server| server.cpu_cores * count as f64);
        return Ok((cores
            * (config.cpu_ms_cost * 263.74 + config.read_bytes_cost * 1_004_497_904.22))
            .max(0.0) as u64);
    }
    if workload == WorkloadType::None {
        workload = WorkloadType::Tpcc;
    }
    let base = workload_costs()
        .get(&workload)
        .copied()
        .ok_or_else(|| format!("unknown workload {workload:?}"))?;
    let tikv_count = count("tikv");
    let mut kv_cores = servers
        .iter()
        .find(|server| server.server_type == "tikv")
        .map_or(0.0, |server| server.cpu_cores * tikv_count as f64);
    if tikv_count == 0 {
        return Err("no server with type 'tikv' is found".to_string());
    }
    let tidb_cores = local_tidb_cores * count("tidb") as f64;
    // 有效 TiKV 核数受 TiDB CPU 与负载比值约束，避免高估。
    kv_cores = kv_cores.min(tidb_cores / base.tidb_to_kv_cpu_ratio);
    let ru_per_core = config.read_base_cost * base.read_requests as f64
        + config.cpu_ms_cost * base.kv_cpu * 1000.0
        + config.read_bytes_cost * base.read_bytes as f64
        + config.write_base_cost * base.write_requests as f64
        + config.write_bytes_cost * base.write_bytes as f64;
    Ok((kv_cores * ru_per_core).max(0.0) as u64)
}

/// 可由 metrics 查询产生的单个响应；`body` 的所有权表示 Go 回调返回后已完成关闭。
#[derive(Clone, Debug)]
pub struct MetricsResponse {
    pub status_code: u16,
    pub status: String,
    pub body: String,
}

/// 集群实例的状态地址与实例地址，供纯 Rust 测试替代 Go HTTP client。
#[derive(Clone, Debug)]
pub struct MetricsServer {
    pub server_type: String,
    pub address: String,
    pub status_address: String,
}

/// 对应 Go `fetchStoreMetrics`：跳过无效实例，记录首个请求错误，成功取得
/// 第一个响应后立即交给回调并返回。
pub fn fetch_store_metrics<F, G>(
    servers: &[MetricsServer],
    server_type: &str,
    mut request: F,
    mut on_response: G,
) -> Result<(), String>
where
    F: FnMut(&str) -> Result<MetricsResponse, String>,
    G: FnMut(&str, &MetricsResponse) -> Result<(), String>,
{
    let mut first_error = None;
    for server in servers {
        if server.server_type != server_type || server.status_address.is_empty() {
            continue;
        }
        let response = match request(&server.status_address) {
            Ok(response) => response,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        return on_response(&server.address, &response);
    }
    Err(first_error.unwrap_or_else(|| format!("no server with type '{server_type}' is found")))
}

/// 从 Prometheus 文本响应读取单个 CPU quota 指标。
pub fn fetch_server_cpu_quota<F>(
    servers: &[MetricsServer],
    server_type: &str,
    metric_name: &str,
    request: F,
) -> Result<f64, String>
where
    F: FnMut(&str) -> Result<MetricsResponse, String>,
{
    let mut cpu_quota = 0.0;
    fetch_store_metrics(servers, server_type, request, |address, response| {
        if response.status_code != 200 {
            return Err(format!(
                "request {address} failed: {}",
                if response.status.is_empty() {
                    response.status_code.to_string()
                } else {
                    response.status.clone()
                }
            ));
        }
        for line in response.body.lines() {
            if !line.starts_with(metric_name) {
                continue;
            }
            let value = line
                .strip_prefix(metric_name)
                .and_then(|rest| rest.strip_prefix(' '))
                .ok_or_else(|| format!("invalid metric '{metric_name}' from server '{address}'"))?
                .parse::<f64>()
                .map_err(|error| error.to_string())?;
            cpu_quota = value;
            return Ok(());
        }
        Err(format!(
            "metrics '{metric_name}' not found from server '{address}'"
        ))
    })
    .map(|()| cpu_quota)
}

/// 生成 Go `getValuesFromMetrics` 使用的行式结果；坏时间行被忽略，查询错误原样返回。
pub fn get_values_from_metrics(
    rows: Result<Vec<Result<TimePointValue, String>>, String>,
) -> Result<TimeSeriesValues, String> {
    let rows = rows?;
    Ok(TimeSeriesValues::new(
        rows.into_iter().filter_map(Result::ok).collect(),
    ))
}

pub fn get_ru_query(start_time: &str, end_time: &str) -> String {
    format!(
        "SELECT time, value FROM METRICS_SCHEMA.resource_manager_resource_unit where time >= '{start_time}' and time <= '{end_time}' ORDER BY time asc"
    )
}

pub fn get_component_cpu_query(component: &str, start_time: &str, end_time: &str) -> String {
    format!(
        "SELECT time, sum(value) FROM METRICS_SCHEMA.process_cpu_usage where time >= '{start_time}' and time <= '{end_time}' and job like '%{component}' GROUP BY time ORDER BY time asc"
    )
}

pub fn get_tiflash_ru_query(start_time: &str, end_time: &str) -> String {
    format!(
        "SELECT time, value FROM METRICS_SCHEMA.tiflash_resource_manager_resource_unit where time >= '{start_time}' and time <= '{end_time}' ORDER BY time asc"
    )
}

pub fn get_tiflash_cpu_query(start_time: &str, end_time: &str) -> String {
    format!(
        "SELECT time, sum(value) FROM METRICS_SCHEMA.tiflash_process_cpu_usage where time >= '{start_time}' and time <= '{end_time}' and job = 'tiflash' GROUP BY time ORDER BY time asc"
    )
}

/// CALIBRATE RESOURCE 执行器精简版：一次 Next 产出静态校准结果。
pub struct Executor {
    pub workload: WorkloadType,
    /// 对应 `tidb_enable_resource_control`，关闭时拒绝校准。
    pub enabled: bool,
    done: bool,
}
impl Executor {
    /// 创建尚未产出结果的执行器。
    pub fn new(workload: WorkloadType, enabled: bool) -> Self {
        Self {
            workload,
            enabled,
            done: false,
        }
    }
    /// 对应 Executor::Next 的静态路径：仅返回一次结果，之后为空。
    pub fn next_static(
        &mut self,
        servers: &[ServerInfo],
        config: RuConfig,
        local_tidb_cores: f64,
    ) -> Result<Option<u64>, String> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        if !self.enabled {
            return Err("Resource control feature is disabled".to_string());
        }
        static_calibrate(self.workload, servers, config, local_tidb_cores).map(Some)
    }

    /// 对应 Go `dynamicCalibrate`：TiDB/TiFlash 任一路径成功即可合并结果，
    /// 仅在两路都失败时返回首个错误。
    pub fn next_dynamic(
        &mut self,
        ru: &[TimePointValue],
        tikv_cpu: &[TimePointValue],
        tidb_cpu: &[TimePointValue],
        tikv_cores: f64,
        tidb_cores: f64,
        tiflash_ru: &[TimePointValue],
        tiflash_cpu: &[TimePointValue],
        tiflash_cores: f64,
    ) -> Result<Option<u64>, String> {
        if self.done {
            return Ok(None);
        }
        self.done = true;
        if !self.enabled {
            return Err("Resource control feature is disabled".to_string());
        }
        let tidb = dynamic_tidb_quota(ru, tikv_cpu, tidb_cpu, tikv_cores, tidb_cores);
        let tiflash = dynamic_tiflash_quota(tiflash_ru, tiflash_cpu, tiflash_cores);
        if tidb.is_err() && tiflash.is_err() {
            return Err(tidb.unwrap_err());
        }
        Ok(Some((tidb.unwrap_or(0.0) + tiflash.unwrap_or(0.0)) as u64))
    }
}
