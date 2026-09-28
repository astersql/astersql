// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Plan Replayer（执行计划重放）任务收集、dump 与过期文件 GC。
//
// Plan Replayer 用于捕获 SQL 的执行计划及相关上下文（schema、stats、bindings 等），
// 打包成可下载的 dump 文件，便于离线复现与排查优化问题。
// 本模块覆盖：
// - dump 文件名时间戳解析与按租约清理过期文件；
// - capture 任务 key 收集/去重；
// - dump 任务投递、运行中占用与完成状态管理。

// limitations under the License.

// plan replayer/trace dump 文件 GC、capture 任务收集与后台 dump worker 的控制流。
//
// dumpFileGcChecker 对应 Go 结构体：周期清理 plan replayer 和 trace plan 的 dump 文件。
// pub struct dumpFileGcChecker {
//     pub Mutex: sync::Mutex,
//     pub gcLease: time::Duration,
//     pub paths: Vec<String>,
//     pub sctx: Option<sessionctx::Context>,
//     pub planReplayerTaskStatus: *mut planReplayerDumpTaskStatus,
// }
//
// parseTime 对应 Go 函数：从文件名最后一个下划线和点号之间解析纳秒时间戳。
// pub fn parseTime(s: &str) -> Result<time::Time, errors::Error> {
//     let startIdx = strings::LastIndex(s, "_");
//     if startIdx == -1 {
//         return Err(errors::New(format!("failed to parse the file :{}", s)));
//     }
//     let endIdx = strings::LastIndex(s, ".");
//     if endIdx == -1 || endIdx <= startIdx + 1 {
//         return Err(errors::New(format!("failed to parse the file :{}", s)));
//     }
//     let i = strconv::ParseInt(&s[startIdx + 1..endIdx], 10, 64)
//         .map_err(|_| errors::New(format!("failed to parse the file :{}", s)))?;
//     Ok(time::Unix(0, i))
// }
//
// impl dumpFileGcChecker {
// GCDumpFiles 对应 Go 方法：持锁遍历所有注册目录并执行过期文件清理。
//     pub fn GCDumpFiles(&mut self, ctx: context::Context, gcDurationDefault: time::Duration, gcDurationForCapture: time::Duration) {
//         let _guard = self.Mutex.Lock();
//         for path in self.paths.clone() {
//             self.gcDumpFilesByPath(ctx.clone(), &path, gcDurationDefault, gcDurationForCapture);
//         }
//     }
//
// setupSctx 对应 Go 方法：保存 sessionctx，供清理 plan replayer status 表时使用。
//     pub fn setupSctx(&mut self, sctx: sessionctx::Context) {
//         self.sctx = Some(sctx);
//     }
//
// gcDumpFilesByPath 对应 Go 方法：走外部存储目录，按创建时间和 capture 类型删除过期文件。
//     pub fn gcDumpFilesByPath(&mut self, ctx: context::Context, path: &str, gcDurationDefault: time::Duration, gcDurationForCapture: time::Duration) {
//         let gcTargetTimeDefault = time::Now().Add(-gcDurationDefault);
//         let gcTargetTimeForCapture = time::Now().Add(-gcDurationForCapture);
//
//         let storage = match extstore::GetGlobalExtStorage(ctx.clone()) {
//             Ok(storage) => storage,
//             Err(err) => {
//                 logutil::BgLogger().Warn("get global ext storage failed", zap::String("category", "dumpFileGcChecker"), zap::Error(err));
//                 return;
//             }
//         };
//         let opt = storeapi::WalkOption { SubDir: path.to_string() };
//         let walk_err = storage.WalkDir(ctx.clone(), &opt, |fileName: String, _size: i64| {
//             let baseName = filepath::Base(&fileName);
//             let createTime = match parseTime(&baseName) {
//                 Ok(t) => t,
//                 Err(err) => {
// Go 对不可解析文件只打 warning 并继续 walk，避免单个异常文件阻断 GC。
//                     logutil::BgLogger().Warn("parseTime failed", zap::String("category", "dumpFileGcChecker"), zap::Error(err), zap::String("filename", &fileName));
//                     return Ok(());
//                 }
//             };
//             let isPlanReplayer = strings::Contains(&baseName, "replayer");
//             let isPlanReplayerCapture = strings::Contains(&baseName, "capture");
//             let canGC = if isPlanReplayer && isPlanReplayerCapture {
//                 !createTime.After(gcTargetTimeForCapture)
//             } else {
//                 !createTime.After(gcTargetTimeDefault)
//             };
//             if canGC {
//                 if let Err(err) = storage.DeleteFile(ctx.clone(), &fileName) {
//                     logutil::BgLogger().Warn("remove file failed", zap::String("category", "dumpFileGcChecker"), zap::Error(err), zap::String("filename", &fileName));
//                     return Ok(());
//                 }
//                 logutil::BgLogger().Info("dumpFileGcChecker successful", zap::String("filename", &fileName));
//                 if isPlanReplayer && self.sctx.is_some() {
//                     deletePlanReplayerStatus(ctx.clone(), self.sctx.clone().unwrap(), &baseName);
//                     unsafe {
// Go 删除 dump 文件后清空已完成任务缓存，避免后续 capture 因旧内存状态被跳过。
//                         (*self.planReplayerTaskStatus).clearFinishedTask();
//                     }
//                 }
//             }
//             Ok(())
//         });
//         if let Err(err) = walk_err {
//             logutil::BgLogger().Warn("walk dir failed", zap::String("category", "dumpFileGcChecker"), zap::Error(err), zap::String("path", path));
//         }
//     }
// }
//
// deletePlanReplayerStatus 对应 Go 函数：删除 mysql.plan_replayer_status 中 token 匹配的记录。
// pub fn deletePlanReplayerStatus(ctx: context::Context, sctx: sessionctx::Context, token: &str) {
//     let ctx1 = kv::WithInternalSourceType(ctx, kv::InternalTxnStatsForegroundPriority);
//     let exec = sctx.GetRestrictedSQLExecutor();
//     if let Err(err) = exec.ExecRestrictedSQL(ctx1, None, "delete from mysql.plan_replayer_status where token = %?", token) {
//         logutil::BgLogger().Warn("delete mysql.plan_replayer_status record failed", zap::String("token", token), zap::Error(err));
//     }
// }
//
// insertPlanReplayerStatus 对应 Go 函数：按成功/失败状态写入 mysql.plan_replayer_status。
// pub fn insertPlanReplayerStatus(ctx: context::Context, sctx: sessionctx::Context, records: Vec<PlanReplayerStatusRecord>) {
//     let ctx1 = kv::WithInternalSourceType(ctx, kv::InternalTxnStatsForegroundPriority);
//     let instance = match infosync::GetServerInfo() {
//         Ok(serverInfo) => net::JoinHostPort(&serverInfo.IP, &strconv::FormatUint(serverInfo.Port as u64, 10)),
//         Err(err) => {
//             logutil::BgLogger().Warn("failed to get server info", zap::Error(err));
//             "unknown".to_string()
//         }
//     };
//     for record in records {
//         if !record.FailedReason.is_empty() {
//             insertPlanReplayerErrorStatusRecord(ctx1.clone(), sctx.clone(), &instance, record);
//         } else {
//             insertPlanReplayerSuccessStatusRecord(ctx1.clone(), sctx.clone(), &instance, record);
//         }
//     }
// }
//
// insertPlanReplayerErrorStatusRecord 对应 Go 函数：插入包含 fail_reason 的状态行。
// pub fn insertPlanReplayerErrorStatusRecord(ctx: context::Context, sctx: sessionctx::Context, instance: &str, record: PlanReplayerStatusRecord) {
//     let exec = sctx.GetRestrictedSQLExecutor();
//     if let Err(err) = exec.ExecRestrictedSQL(
//         ctx,
//         None,
//         "insert into mysql.plan_replayer_status (sql_digest, plan_digest, origin_sql, fail_reason, instance) values (%?,%?,%?,%?,%?)",
//         record.SQLDigest.clone(),
//         record.PlanDigest.clone(),
//         record.OriginSQL.clone(),
//         record.FailedReason.clone(),
//         instance,
//     ) {
//         logutil::BgLogger().Warn(
//             "insert mysql.plan_replayer_status record failed",
//             zap::String("sqlDigest", &record.SQLDigest),
//             zap::String("planDigest", &record.PlanDigest),
//             zap::String("sql", &record.OriginSQL),
//             zap::String("failReason", &record.FailedReason),
//             zap::String("instance", instance),
//             zap::Error(err),
//         );
//     }
// }
//
// insertPlanReplayerSuccessStatusRecord 对应 Go 函数：优先写入 origin_sql，失败后退化为不带原 SQL 的插入。
// pub fn insertPlanReplayerSuccessStatusRecord(ctx: context::Context, sctx: sessionctx::Context, instance: &str, record: PlanReplayerStatusRecord) {
//     let exec = sctx.GetRestrictedSQLExecutor();
//     let err = exec.ExecRestrictedSQL(
//         ctx.clone(),
//         None,
//         "insert into mysql.plan_replayer_status (sql_digest, plan_digest, origin_sql, token, instance) values (%?,%?,%?,%?,%?)",
//         record.SQLDigest.clone(),
//         record.PlanDigest.clone(),
//         record.OriginSQL.clone(),
//         record.Token.clone(),
//         instance,
//     );
//     if let Err(err) = err {
//         logutil::BgLogger().Warn(
//             "insert mysql.plan_replayer_status record failed",
//             zap::String("sqlDigest", &record.SQLDigest),
//             zap::String("planDigest", &record.PlanDigest),
//             zap::String("sql", &record.OriginSQL),
//             zap::String("token", &record.Token),
//             zap::String("instance", instance),
//             zap::Error(err),
//         );
// Go 这里为了规避 origin_sql 太大或不可写的情况，再尝试只写 digest/token/instance。
//         if let Err(err) = exec.ExecRestrictedSQL(
//             ctx,
//             None,
//             "insert into mysql.plan_replayer_status (sql_digest, plan_digest, token, instance) values (%?,%?,%?,%?)",
//             record.SQLDigest.clone(),
//             record.PlanDigest.clone(),
//             record.Token.clone(),
//             instance,
//         ) {
//             logutil::BgLogger().Warn(
//                 "insert mysql.plan_replayer_status record failed",
//                 zap::String("sqlDigest", &record.SQLDigest),
//                 zap::String("planDigest", &record.PlanDigest),
//                 zap::String("token", &record.Token),
//                 zap::String("instance", instance),
//                 zap::Error(err),
//             );
//         }
//     }
// }
//
// planReplayerHandle 对应 Go 结构体：组合任务收集端和 dump worker 投递端。
// pub struct planReplayerHandle {
//     pub planReplayerTaskCollectorHandle: *mut planReplayerTaskCollectorHandle,
//     pub planReplayerTaskDumpHandle: *mut planReplayerTaskDumpHandle,
// }
//
// impl planReplayerHandle {
// SendTask 对应 Go 方法：非阻塞投递 dumpTask 到后台 worker channel。
//     pub fn SendTask(&mut self, task: *mut PlanReplayerDumpTask) -> bool {
//         let sent = unsafe { (*self.planReplayerTaskDumpHandle).taskCH.try_send(task) };
//         match sent {
//             Ok(_) => {
//                 unsafe {
// 投递成功后，非持续 capture 的 key 直接从待处理集合移除；dump 失败会在下一轮重新收集。
//                     if !(*task).IsContinuesCapture {
//                         (*self.planReplayerTaskCollectorHandle).removeTask((*task).PlanReplayerTaskKey.clone());
//                     }
//                 }
//                 domain_metrics::PlanReplayerCaptureTaskSendCounter.Inc();
//                 true
//             }
//             Err(_) => {
//                 domain_metrics::PlanReplayerCaptureTaskDiscardCounter.Inc();
// Go default 分支表示 channel 满时丢弃，避免阻塞查询路径。
//                 unsafe {
//                     logutil::BgLogger().Warn(
//                         "discard one plan replayer dump task",
//                         zap::String("sql-digest", &(*task).SQLDigest),
//                         zap::String("plan-digest", &(*task).PlanDigest),
//                     );
//                 }
//                 false
//             }
//         }
//     }
// }
//
// planReplayerTaskCollectorHandle 对应 Go 结构体：维护从 mysql.plan_replayer_task 收集出的待处理 key。
// pub struct planReplayerTaskCollectorHandle {
//     pub taskMu: taskKeyMapWithRWMutex,
//     pub ctx: context::Context,
//     pub sctx: sessionctx::Context,
// }
//
// taskKeyMapWithRWMutex 对应 Go 匿名结构体 taskMu，显式拆出是为了表达 RWMutex + map 的组合。
// pub struct taskKeyMapWithRWMutex {
//     pub RWMutex: sync::RWMutex,
//     pub tasks: std::collections::HashMap<replayer::PlanReplayerTaskKey, ()>,
// }
//
// impl planReplayerTaskCollectorHandle {
// CollectPlanReplayerTask 对应 Go 方法：收集所有尚未完成的 capture task 并刷新内存集合。
//     pub fn CollectPlanReplayerTask(&mut self) -> Result<(), errors::Error> {
//         let allKeys = self.collectAllPlanReplayerTask(self.ctx.clone())?;
//         let mut tasks = Vec::new();
//         for key in allKeys {
//             let unhandled = checkUnHandledReplayerTask(self.ctx.clone(), self.sctx.clone(), key.clone())?;
//             if unhandled {
//                 logutil::BgLogger().Debug(
//                     "collect plan replayer task success",
//                     zap::String("category", "plan-replayer-task"),
//                     zap::String("sql-digest", &key.SQLDigest),
//                     zap::String("plan-digest", &key.PlanDigest),
//                 );
//                 tasks.push(key);
//             }
//         }
//         self.setupTasks(tasks.clone());
//         domain_metrics::PlanReplayerRegisterTaskGauge.Set(tasks.len() as f64);
//         Ok(())
//     }
//
// GetTasks 对应 Go 方法：复制当前 map 中所有任务 key。
//     pub fn GetTasks(&self) -> Vec<replayer::PlanReplayerTaskKey> {
//         let _guard = self.taskMu.RWMutex.RLock();
//         self.taskMu.tasks.keys().cloned().collect()
//     }
//
// setupTasks 对应 Go 方法：用新 map 原子替换待处理任务集合。
//     pub fn setupTasks(&mut self, tasks: Vec<replayer::PlanReplayerTaskKey>) {
//         let mut r = std::collections::HashMap::new();
//         for task in tasks {
//             r.insert(task, ());
//         }
//         let _guard = self.taskMu.RWMutex.Lock();
//         self.taskMu.tasks = r;
//     }
//
// removeTask 对应 Go 方法：投递成功后删除对应 task key。
//     pub fn removeTask(&mut self, taskKey: replayer::PlanReplayerTaskKey) {
//         let _guard = self.taskMu.RWMutex.Lock();
//         self.taskMu.tasks.remove(&taskKey);
//     }
//
// collectAllPlanReplayerTask 对应 Go 方法：查询 mysql.plan_replayer_task 表并构造 task key 列表。
//     pub fn collectAllPlanReplayerTask(&self, ctx: context::Context) -> Result<Vec<replayer::PlanReplayerTaskKey>, errors::Error> {
//         let exec = self.sctx.GetSQLExecutor();
//         let rs = exec.ExecuteInternal(ctx.clone(), "select sql_digest, plan_digest from mysql.plan_replayer_task")?;
//         if rs.is_none() {
//             return Ok(Vec::new());
//         }
//         let rs = rs.unwrap();
// Go 使用 defer terror.Call(rs.Close) 收尾；保留 close guard 语义。
//         let rows = sqlexec::DrainRecordSet(ctx, rs.clone(), 8).map_err(errors::Trace)?;
//         terror::Call(rs.Close);
//         let mut allKeys = Vec::with_capacity(rows.len());
//         for row in rows {
//             let sqlDigest = row.GetString(0);
//             let planDigest = row.GetString(1);
//             allKeys.push(replayer::PlanReplayerTaskKey { SQLDigest: sqlDigest, PlanDigest: planDigest });
//         }
//         Ok(allKeys)
//     }
// }
//
// planReplayerDumpTaskStatus 对应 Go 结构体：记录运行中和已完成的任务 key，避免重复处理。
// pub struct planReplayerDumpTaskStatus {
//     pub runningTaskMu: taskStatusMapWithRWMutex,
//     pub finishedTaskMu: taskStatusMapWithRWMutex,
// }
//
// taskStatusMapWithRWMutex 对应 Go 中两个匿名 RWMutex + map 字段。
// pub struct taskStatusMapWithRWMutex {
//     pub RWMutex: sync::RWMutex,
//     pub tasks: std::collections::HashMap<replayer::PlanReplayerTaskKey, ()>,
// }
//
// impl planReplayerDumpTaskStatus {
// GetRunningTaskStatusLen used for unit test
//     pub fn GetRunningTaskStatusLen(&self) -> usize {
//         let _guard = self.runningTaskMu.RWMutex.RLock();
//         self.runningTaskMu.tasks.len()
//     }
//
// CleanFinishedTaskStatus clean then finished tasks, only used for unit test
//     pub fn CleanFinishedTaskStatus(&mut self) {
//         let _guard = self.finishedTaskMu.RWMutex.Lock();
//         self.finishedTaskMu.tasks = std::collections::HashMap::new();
//     }
//
// GetFinishedTaskStatusLen used for unit test
//     pub fn GetFinishedTaskStatusLen(&self) -> usize {
//         let _guard = self.finishedTaskMu.RWMutex.RLock();
//         self.finishedTaskMu.tasks.len()
//     }
//
// occupyRunningTaskKey 对应 Go 方法：抢占运行中 key，已被其他 worker 占用时返回 false。
//     pub fn occupyRunningTaskKey(&mut self, task: &PlanReplayerDumpTask) -> bool {
//         let _guard = self.runningTaskMu.RWMutex.Lock();
//         if self.runningTaskMu.tasks.contains_key(&task.PlanReplayerTaskKey) {
//             return false;
//         }
//         self.runningTaskMu.tasks.insert(task.PlanReplayerTaskKey.clone(), ());
//         true
//     }
//
// releaseRunningTaskKey 对应 Go 方法：任务结束后释放运行中 key。
//     pub fn releaseRunningTaskKey(&mut self, task: &PlanReplayerDumpTask) {
//         let _guard = self.runningTaskMu.RWMutex.Lock();
//         self.runningTaskMu.tasks.remove(&task.PlanReplayerTaskKey);
//     }
//
// checkTaskKeyFinishedBefore 对应 Go 方法：持续 capture 任务会跳过已完成 key。
//     pub fn checkTaskKeyFinishedBefore(&self, task: &PlanReplayerDumpTask) -> bool {
//         let _guard = self.finishedTaskMu.RWMutex.RLock();
//         self.finishedTaskMu.tasks.contains_key(&task.PlanReplayerTaskKey)
//     }
//
// setTaskFinished 对应 Go 方法：成功处理持续 capture 后标记完成。
//     pub fn setTaskFinished(&mut self, task: &PlanReplayerDumpTask) {
//         let _guard = self.finishedTaskMu.RWMutex.Lock();
//         self.finishedTaskMu.tasks.insert(task.PlanReplayerTaskKey.clone(), ());
//     }
//
// clearFinishedTask 对应 Go 方法：GC 删除 dump 文件后清空完成缓存。
//     pub fn clearFinishedTask(&mut self) {
//         let _guard = self.finishedTaskMu.RWMutex.Lock();
//         self.finishedTaskMu.tasks = std::collections::HashMap::new();
//     }
// }
//
// planReplayerTaskDumpWorker 对应 Go worker：从 taskCH 读取 PlanReplayerDumpTask 并写出 dump 文件。
// pub struct planReplayerTaskDumpWorker {
//     pub ctx: context::Context,
//     pub sctx: sessionctx::Context,
//     pub taskCH: chan::Receiver<*mut PlanReplayerDumpTask>,
//     pub status: *mut planReplayerDumpTaskStatus,
// }
//
// impl planReplayerTaskDumpWorker {
// run 对应 Go 方法：range channel 直到关闭。
//     pub fn run(&mut self) {
//         logutil::BgLogger().Info("planReplayerTaskDumpWorker started.");
//         while let Some(task) = self.taskCH.recv() {
//             unsafe { self.handleTask(&mut *task); }
//         }
//         logutil::BgLogger().Info("planReplayerTaskDumpWorker exited.");
//     }
//
// handleTask 对应 Go 方法：处理重复检查、运行中占用、panic recover 与 debug 日志。
//     pub fn handleTask(&mut self, task: &mut PlanReplayerDumpTask) {
//         let sqlDigest = task.SQLDigest.clone();
//         let planDigest = task.PlanDigest.clone();
//         let mut check = true;
//         let mut occupy = true;
//         let mut handleTask = true;
// Go defer 在退出时记录处理状态；用 scopeguard 表达同样的收尾意图。
//         let _log_guard = scopeguard::guard((), |_| {
//             logutil::BgLogger().Debug(
//                 "handle task",
//                 zap::String("category", "plan-replayer-capture"),
//                 zap::String("sql-digest", &sqlDigest),
//                 zap::String("plan-digest", &planDigest),
//                 zap::Bool("check", check),
//                 zap::Bool("occupy", occupy),
//                 zap::Bool("handle", handleTask),
//             );
//         });
//         util::Recover(metrics::LabelDomain, "PlanReplayerTaskDumpWorker", None, false);
//
//         unsafe {
//             if task.IsContinuesCapture && (*self.status).checkTaskKeyFinishedBefore(task) {
//                 check = false;
//                 return;
//             }
//             occupy = (*self.status).occupyRunningTaskKey(task);
//             if !occupy {
//                 return;
//             }
//             handleTask = self.HandleTask(task);
//             (*self.status).releaseRunningTaskKey(task);
//         }
//     }
//
// HandleTask handled task
//     pub fn HandleTask(&mut self, task: &mut PlanReplayerDumpTask) -> bool {
//         let taskKey = task.PlanReplayerTaskKey.clone();
//         let success = match checkUnHandledReplayerTask(self.ctx.clone(), self.sctx.clone(), taskKey.clone()) {
//             Ok(false) => true, // 任务已经处理过，Go 直接跳过并视为成功。
//             Ok(true) => self.dumpUnhandledTask(task, taskKey),
//             Err(err) => {
//                 logutil::BgLogger().Warn(
//                     "check task failed",
//                     zap::String("category", "plan-replayer-capture"),
//                     zap::String("sqlDigest", &taskKey.SQLDigest),
//                     zap::String("planDigest", &taskKey.PlanDigest),
//                     zap::Error(err),
//                 );
//                 false
//             }
//         };
//         if success && task.IsContinuesCapture {
//             unsafe { (*self.status).setTaskFinished(task); }
//         }
//         success
//     }
//
// dumpUnhandledTask 对应 Go HandleTask 中真正生成文件并调用 DumpPlanReplayerInfo 的分支。
//     fn dumpUnhandledTask(&mut self, task: &mut PlanReplayerDumpTask, taskKey: replayer::PlanReplayerTaskKey) -> bool {
//         let storage = match extstore::GetGlobalExtStorage(self.ctx.clone()) {
//             Ok(storage) => storage,
//             Err(err) => {
//                 logutil::BgLogger().Warn("get global ext storage failed", zap::String("category", "plan-replayer-capture"), zap::String("sqlDigest", &taskKey.SQLDigest), zap::String("planDigest", &taskKey.PlanDigest), zap::Error(err));
//                 return false;
//             }
//         };
//         let (file, fileName) = match replayer::GeneratePlanReplayerFile(self.ctx.clone(), storage, task.IsCapture, task.IsContinuesCapture, vardef::EnableHistoricalStatsForCapture.Load()) {
//             Ok(v) => v,
//             Err(err) => {
//                 logutil::BgLogger().Warn("generate task file failed", zap::String("category", "plan-replayer-capture"), zap::String("sqlDigest", &taskKey.SQLDigest), zap::String("planDigest", &taskKey.PlanDigest), zap::Error(err));
//                 return false;
//             }
//         };
//         task.Zf = Some(file);
//         task.FileName = fileName;
//         if let Err(err) = DumpPlanReplayerInfo(self.ctx.clone(), self.sctx.clone(), task) {
//             logutil::BgLogger().Warn("dump task result failed", zap::String("category", "plan-replayer-capture"), zap::String("sqlDigest", &taskKey.SQLDigest), zap::String("planDigest", &taskKey.PlanDigest), zap::Error(err));
//             return false;
//         }
//         true
//     }
// }
//
// planReplayerTaskDumpHandle 对应 Go 结构体：保存任务 channel、共享状态和 worker 列表。
// pub struct planReplayerTaskDumpHandle {
//     pub taskCH: chan::Sender<*mut PlanReplayerDumpTask>,
//     pub status: *mut planReplayerDumpTaskStatus,
//     pub workers: Vec<*mut planReplayerTaskDumpWorker>,
// }
//
// impl planReplayerTaskDumpHandle {
// GetTaskStatus used for test
//     pub fn GetTaskStatus(&self) -> *mut planReplayerDumpTaskStatus {
//         self.status
//     }
//
// GetWorker used for test
//     pub fn GetWorker(&self) -> *mut planReplayerTaskDumpWorker {
//         self.workers[0]
//     }
//
// Close make finished flag true
//     pub fn Close(&mut self) {
//         self.taskCH.close();
//     }
//
// DrainTask drain a task for unit test
//     pub fn DrainTask(&mut self) -> *mut PlanReplayerDumpTask {
//         self.taskCH.recv().unwrap()
//     }
// }
//
// checkUnHandledReplayerTask 对应 Go 函数：查询 status 表，确认没有成功完成记录。
// pub fn checkUnHandledReplayerTask(ctx: context::Context, sctx: sessionctx::Context, task: replayer::PlanReplayerTaskKey) -> Result<bool, errors::Error> {
//     let exec = sctx.GetSQLExecutor();
//     let sql = format!(
//         "select * from mysql.plan_replayer_status where sql_digest = '{}' and plan_digest = '{}' and fail_reason is null",
//         task.SQLDigest, task.PlanDigest
//     );
//     let rs = exec.ExecuteInternal(ctx.clone(), &sql)?;
//     if rs.is_none() {
//         return Ok(true);
//     }
//     let rs = rs.unwrap();
//     let rows = sqlexec::DrainRecordSet(ctx, rs.clone(), 8).map_err(errors::Trace)?;
//     terror::Call(rs.Close);
//     Ok(rows.is_empty())
// }
//
// CheckPlanReplayerTaskExists checks whether plan replayer capture task exists already
// pub fn CheckPlanReplayerTaskExists(ctx: context::Context, sctx: sessionctx::Context, sqlDigest: &str, planDigest: &str) -> Result<bool, errors::Error> {
//     let exec = sctx.GetSQLExecutor();
//     let sql = format!(
//         "select * from mysql.plan_replayer_task where sql_digest = '{}' and plan_digest = '{}'",
//         sqlDigest, planDigest
//     );
//     let rs = exec.ExecuteInternal(ctx.clone(), &sql)?;
//     if rs.is_none() {
//         return Ok(false);
//     }
//     let rs = rs.unwrap();
//     let rows = sqlexec::DrainRecordSet(ctx, rs.clone(), 8).map_err(errors::Trace)?;
//     terror::Call(rs.Close);
//     Ok(!rows.is_empty())
// }
//
// PlanReplayerStatusRecord indicates record in mysql.plan_replayer_status
// pub struct PlanReplayerStatusRecord {
//     pub SQLDigest: String,
//     pub PlanDigest: String,
//     pub OriginSQL: String,
//     pub Token: String,
//     pub FailedReason: String,
// }
//
// PlanReplayerDumpTask wrap the params for plan replayer dump
// pub struct PlanReplayerDumpTask {
//     pub PlanReplayerTaskKey: replayer::PlanReplayerTaskKey,
//
// 查询执行期间暂存的 table stats；Go 使用 map[int64]any。
//     pub TblStats: std::collections::HashMap<i64, Box<dyn std::any::Any>>,
//
// dump plan 所需的 session、SQL、binding、trace 等上下文。
//     pub StartTS: u64,
//     pub SessionBindings: Vec<Vec<*mut bindinfo::Binding>>,
//     pub EncodedPlan: String,
//     pub SessionVars: *mut variable::SessionVars,
//     pub ExecStmts: Vec<ast::StmtNode>,
//     pub Analyze: bool,
//     pub HistoricalStatsTS: u64,
//     pub DebugTrace: Vec<Box<dyn std::any::Any>>,
//
//     pub FileName: String,
//     pub PresignedURL: String,
//     pub Zf: Option<Box<dyn io::WriteCloser>>,
//
// IsCapture indicates whether the task is from capture
//     pub IsCapture: bool,
// IsContinuesCapture indicates whether the task is from continues capture
//     pub IsContinuesCapture: bool,
//
// Go 通过匿名嵌入 PlanReplayerTaskKey 暴露 SQLDigest/PlanDigest；显式冗余字段方便阅读。
//     pub SQLDigest: String,
//     pub PlanDigest: String,
// }
// */
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Plan Replayer 任务唯一键：SQL digest + plan digest（摘要哈希）。
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlanReplayerTaskKey {
    /// SQL 文本摘要。
    pub sql_digest: String,
    /// 执行计划摘要。
    pub plan_digest: String,
}

/// 一次 dump 所需的全部上下文：表统计、会话变量、语句、计划与 capture 标志。
#[derive(Clone, Debug, Default)]
pub struct PlanReplayerDumpTask {
    /// 任务键。
    pub key: PlanReplayerTaskKey,
    /// 表 ID 到统计信息载荷的映射（简化为字符串）。
    pub table_stats: Vec<(i64, String)>,
    /// 事务开始时间戳（Timestamp，TiKV MVCC 版本号）。
    pub start_ts: u64,
    /// 会话级 SQL Binding（强制走指定计划的绑定规则）。
    pub session_bindings: Vec<String>,
    /// 已编码的执行计划；非空时可直接写出，跳过 explain。
    pub encoded_plan: String,
    /// 会话变量快照。
    pub session_variables: Vec<(String, String)>,
    /// 待 dump 的 SQL 语句列表。
    pub statements: Vec<String>,
    /// 是否执行 `EXPLAIN ANALYZE`（带真实执行统计）。
    pub analyze: bool,
    /// 历史统计快照时间戳；0 表示用最新统计。
    pub historical_stats_ts: u64,
    /// 优化器 debug trace 载荷。
    pub debug_trace: Vec<String>,
    /// dump 文件名（亦用作 status 表 token）。
    pub file_name: String,
    /// 外部存储预签名下载 URL。
    pub presigned_url: String,
    /// 是否来自 capture（自动捕获）路径。
    pub is_capture: bool,
    /// 是否持续 capture；成功后标记 finished，避免重复 dump。
    pub is_continuous_capture: bool,
}

/// 写入 `mysql.plan_replayer_status` 的状态记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanReplayerStatusRecord {
    /// SQL 文本摘要。
    pub sql_digest: String,
    /// 执行计划摘要。
    pub plan_digest: String,
    /// 原始 SQL 文本。
    pub origin_sql: String,
    /// 对应 dump 文件 token。
    pub token: String,
    /// 失败原因；空表示成功。
    pub failed_reason: String,
}

/// 从 dump 文件名解析创建时间。
///
/// 约定文件名形如 `..._<nanos>.zip`：取最后一个 `_` 与 `.` 之间的纳秒时间戳。
pub fn parse_dump_time(file_name: &str) -> Result<SystemTime, String> {
    // 只取 basename，避免路径干扰解析。
    let base = Path::new(file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?;
    let start = base
        .rfind('_')
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?
        + 1;
    let end = base
        .rfind('.')
        .filter(|end| *end > start)
        .ok_or_else(|| format!("failed to parse the file: {file_name}"))?;
    let nanos = base[start..end]
        .parse::<i64>()
        .map_err(|_| format!("failed to parse the file: {file_name}"))?;
    if nanos >= 0 {
        UNIX_EPOCH
            .checked_add(Duration::from_nanos(nanos as u64))
            .ok_or_else(|| format!("failed to parse the file: {file_name}"))
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_nanos(nanos.unsigned_abs()))
            .ok_or_else(|| format!("failed to parse the file: {file_name}"))
    }
}

/// dump 文件存储抽象：列举、删除文件，以及删除 status 表记录。
pub trait DumpFileStore {
    /// 列举指定子目录下的文件。
    fn list(&self, path: &str) -> Result<Vec<String>, String>;
    /// 删除单个 dump 文件。
    fn delete(&self, path: &str) -> Result<(), String>;
    /// 按 token 清理 `plan_replayer_status` 记录。
    fn delete_status(&self, token: &str) -> Result<(), String>;
}

/// 周期性清理 plan replayer / trace dump 过期文件的检查器。
pub struct DumpFileGcChecker {
    /// 需要扫描的目录列表。
    paths: Vec<String>,
    /// 串行化 GC，避免并发清理。
    lock: Mutex<()>,
}

impl DumpFileGcChecker {
    /// 注册待 GC 的路径。
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            lock: Mutex::new(()),
        }
    }

    /// 按默认租约与 capture 租约删除过期文件。
    ///
    /// capture 类文件通常保留更久；删除 replayer 文件时同步清理 status。
    pub fn gc<S: DumpFileStore>(
        &self,
        store: &S,
        now: SystemTime,
        default_lease: Duration,
        capture_lease: Duration,
    ) -> Result<Vec<String>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "dump file GC lock poisoned".to_string())?;
        // 计算两类过期截止时间：普通 dump 与 capture dump。
        let default_cutoff = now.checked_sub(default_lease).unwrap_or(UNIX_EPOCH);
        let capture_cutoff = now.checked_sub(capture_lease).unwrap_or(UNIX_EPOCH);
        let mut deleted = Vec::new();
        for path in &self.paths {
            let Ok(files) = store.list(path) else {
                // Go 仅记录 WalkDir 错误，并继续清理其它注册目录。
                continue;
            };
            for file in files {
                let Ok(created) = parse_dump_time(&file) else {
                    continue;
                };
                let base = Path::new(&file)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                let replayer = base.contains("replayer");
                // capture 使用更长租约，避免刚捕获的文件被过早删除。
                let cutoff = if replayer && base.contains("capture") {
                    capture_cutoff
                } else {
                    default_cutoff
                };
                if created <= cutoff {
                    if store.delete(&file).is_err() {
                        // 单个文件删除失败不能中断同一目录中其它文件的 GC。
                        continue;
                    }
                    if replayer {
                        // Go 的状态表清理是 best-effort；dump 文件已删除即视为 GC 成功。
                        let _ = store.delete_status(base);
                    }
                    deleted.push(file);
                }
            }
        }
        Ok(deleted)
    }
}

/// 内存中待处理的 capture 任务集合（已过滤掉 handled）。
#[derive(Default)]
pub struct PlanReplayerTaskCollector {
    /// 读写锁保护的任务 key 集合。
    tasks: RwLock<BTreeSet<PlanReplayerTaskKey>>,
}

/// SQL boundary used by plan-replayer task collection.
///
/// Implementations must execute both lookups with the supplied internal-request
/// context so slow logging and statement-summary filtering see the query as
/// internal, matching Go's `ExecuteInternal` path.
pub trait PlanReplayerTaskSource {
    /// Return every registered capture task.
    fn registered_tasks(
        &self,
        context: &astersql_kv::Context,
        sql: &str,
    ) -> Result<Vec<PlanReplayerTaskKey>, String>;

    /// Return whether one registered task still needs handling.
    fn is_unhandled(
        &self,
        context: &astersql_kv::Context,
        key: &PlanReplayerTaskKey,
    ) -> Result<bool, String>;
}

/// SQL used by Go's `collectAllPlanReplayerTask` implementation.
pub const COLLECT_PLAN_REPLAYER_TASK_SQL: &str =
    "select sql_digest, plan_digest from mysql.plan_replayer_task";

impl PlanReplayerTaskCollector {
    /// Collect registered, unhandled tasks through an internal SQL context.
    ///
    /// The current set is replaced only after every lookup succeeds, preserving
    /// Go's error path where `setupTasks` is not reached on query failure.
    pub fn collect_plan_replayer_tasks<S: PlanReplayerTaskSource>(
        &self,
        source: &S,
    ) -> Result<(), String> {
        let context = astersql_kv::WithInternalSourceType(
            astersql_kv::Context::todo(),
            astersql_kv::InternalTxnStatsForegroundPriority,
        );
        let registered = source.registered_tasks(&context, COLLECT_PLAN_REPLAYER_TASK_SQL)?;
        let mut tasks = Vec::new();
        for key in registered {
            if source.is_unhandled(&context, &key)? {
                tasks.push(key);
            }
        }
        self.collect(tasks, &BTreeSet::new());
        Ok(())
    }

    /// 用已注册但尚未 handled 的任务覆盖当前集合。
    pub fn collect(
        &self,
        registered: Vec<PlanReplayerTaskKey>,
        handled: &BTreeSet<PlanReplayerTaskKey>,
    ) {
        *self
            .tasks
            .write()
            .expect("plan replayer collector poisoned") = registered
            .into_iter()
            .filter(|key| !handled.contains(key))
            .collect();
    }
    /// 返回当前待处理任务副本。
    pub fn tasks(&self) -> Vec<PlanReplayerTaskKey> {
        self.tasks
            .read()
            .expect("plan replayer collector poisoned")
            .iter()
            .cloned()
            .collect()
    }
    /// 投递成功后移除对应 key。
    pub fn remove(&self, key: &PlanReplayerTaskKey) {
        self.tasks
            .write()
            .expect("plan replayer collector poisoned")
            .remove(key);
    }
}

/// dump worker 的运行中/已完成任务状态，用于去重与并发占用。
#[derive(Default)]
pub struct PlanReplayerDumpTaskStatus {
    /// 正在被 worker 处理的 key。
    running: RwLock<BTreeSet<PlanReplayerTaskKey>>,
    /// 持续 capture 已成功完成的 key。
    finished: RwLock<BTreeSet<PlanReplayerTaskKey>>,
}

impl PlanReplayerDumpTaskStatus {
    /// 运行中任务数（测试用）。
    pub fn running_len(&self) -> usize {
        self.running
            .read()
            .expect("plan replayer status poisoned")
            .len()
    }
    /// 已完成任务数（测试用）。
    pub fn finished_len(&self) -> usize {
        self.finished
            .read()
            .expect("plan replayer status poisoned")
            .len()
    }
    /// 清空已完成缓存（GC 删除文件后或测试用）。
    pub fn clean_finished(&self) {
        self.finished
            .write()
            .expect("plan replayer status poisoned")
            .clear();
    }
    /// 尝试抢占运行中 key；已存在则返回 false。
    pub fn occupy(&self, key: &PlanReplayerTaskKey) -> bool {
        self.running
            .write()
            .expect("plan replayer status poisoned")
            .insert(key.clone())
    }
    /// 释放运行中 key。
    pub fn release(&self, key: &PlanReplayerTaskKey) {
        self.running
            .write()
            .expect("plan replayer status poisoned")
            .remove(key);
    }
    /// 是否已在 finished 集合中。
    pub fn was_finished(&self, key: &PlanReplayerTaskKey) -> bool {
        self.finished
            .read()
            .expect("plan replayer status poisoned")
            .contains(key)
    }
    /// 标记任务完成。
    pub fn set_finished(&self, key: PlanReplayerTaskKey) {
        self.finished
            .write()
            .expect("plan replayer status poisoned")
            .insert(key);
    }
}

/// Plan Replayer 对外句柄：收集端 + 有界 channel 投递端。
pub struct PlanReplayerHandle {
    /// 共享的任务收集器。
    collector: Arc<PlanReplayerTaskCollector>,
    /// 有界同步 channel；`None` 表示已关闭。
    sender: Mutex<Option<mpsc::SyncSender<PlanReplayerDumpTask>>>,
}

impl PlanReplayerHandle {
    /// 创建句柄与接收端；容量至少为 1。
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<PlanReplayerDumpTask>) {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        (
            Self {
                collector: Arc::new(PlanReplayerTaskCollector::default()),
                sender: Mutex::new(Some(sender)),
            },
            receiver,
        )
    }
    /// 取得收集器共享引用。
    pub fn collector(&self) -> Arc<PlanReplayerTaskCollector> {
        self.collector.clone()
    }
    /// 非阻塞投递 dump 任务；channel 满则丢弃并返回 false。
    ///
    /// 非持续 capture 投递成功后立即从收集器移除，失败可在下一轮重新收集。
    pub fn send_task(&self, task: PlanReplayerDumpTask) -> bool {
        let key = task.key.clone();
        let sent = self
            .sender
            .lock()
            .expect("plan replayer sender poisoned")
            .as_ref()
            .is_some_and(|sender| sender.try_send(task.clone()).is_ok());
        if sent && !task.is_continuous_capture {
            self.collector.remove(&key);
        }
        sent
    }
    /// 关闭投递端，worker 侧随即结束。
    pub fn close(&self) {
        self.sender
            .lock()
            .expect("plan replayer sender poisoned")
            .take();
    }
}

/// 实际写出 dump 内容并返回 status 记录的后端抽象。
pub trait PlanReplayerDumper {
    /// 执行一次 dump；成功返回待写入的 status 记录。
    fn dump(
        &self,
        task: &mut PlanReplayerDumpTask,
    ) -> Result<Vec<PlanReplayerStatusRecord>, String>;
}

/// 处理单个 dump 任务：跳过已完成、抢占运行锁、调用 dumper、释放并标记完成。
///
/// 对齐 Go worker 的去重与并发保护语义。
pub fn handle_dump_task(
    dumper: &dyn PlanReplayerDumper,
    status: &PlanReplayerDumpTaskStatus,
    task: &mut PlanReplayerDumpTask,
) -> bool {
    // 只有持续 capture 任务需要 finished 去重；普通任务失败后允许再次处理，
    // 与 Go worker 仅在 IsContinuesCapture 分支检查 finishedTask 一致。
    if (task.is_continuous_capture && status.was_finished(&task.key)) || !status.occupy(&task.key) {
        return false;
    }
    let result = dumper.dump(task);
    status.release(&task.key);
    if result.is_ok() && task.is_continuous_capture {
        status.set_finished(task.key.clone());
    }
    result.is_ok()
}
