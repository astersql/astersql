// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Index LookUp Join（索引查找连接）执行器。
//
// 外表 outer worker 按批构造 lookup task；多个 inner worker 根据外表 key
// 构造索引 ranges、拉取内表行并构建 lookup map；主线程按外表顺序与 Joiner
// 产出连接结果。Index Lookup 指按索引定位内表行后再回表取完整行。

// IndexLookUpJoin 的外表批量读取、内表并发 lookup、哈希表构建和主线程 join 流程。
// memory、failpoint、runtime stats 等均沿用 Go 名称作为迁移占位，便于人工核对原始控制流。
//
// Go: var _ exec.Executor = &IndexLookUpJoin{}
// IndexLookUpJoin 对应 Go 的同名执行器：一个 outer worker 与多个 inner worker 并发执行，并保持外表顺序。
// pub struct IndexLookUpJoin {
//     pub base_executor: exec::BaseExecutor,
//     pub result_ch: Option<Receiver<LookUpJoinTask>>,
//     pub cancel_func: Option<context::CancelFunc>,
//     pub worker_wg: sync::WaitGroup,
//     pub outer_ctx: OuterCtx,
//     pub inner_ctx: InnerCtx,
//     pub task: Option<LookUpJoinTask>,
//     pub join_result: chunk::Chunk,
//     pub inner_iter: Option<chunk::Iterator4Slice>,
//     pub joiner: Joiner,
//     pub is_outer_join: bool,
//     pub required_rows: AtomicI64,
//     pub index_ranges: ranger::MutableRanges,
//     pub key_off2_idx_off: Vec<i32>,
//     pub inner_ptr_bytes: Vec<Vec<u8>>,
// LastColHelper 保存最后一列比较辅助信息；Go 中用于 col > x_col && col < x_col + 100 这类复杂过滤。
//     pub last_col_helper: Option<physicalop::ColWithCmpFuncManager>,
//     pub mem_tracker: Option<memory::Tracker>,
//     pub stats: Option<IndexLookUpJoinRuntimeStats>,
//     pub finished: AtomicValue,
//     pub prepared: bool,
// }
//
// OuterCtx 对应 Go 外表上下文，保存外表 key/hash 列与过滤条件。
// pub struct OuterCtx {
//     pub row_types: Vec<types::FieldType>,
//     pub key_cols: Vec<i32>,
//     pub hash_types: Vec<types::FieldType>,
//     pub hash_cols: Vec<i32>,
//     pub filter: expression::CNFExprs,
// }
//
// IndexJoinExecutorBuilder 对应 Go 接口，用于打断循环 import，由 planner 侧按 lookup key 构造内表执行器。
// pub trait IndexJoinExecutorBuilder {
//     fn build_executor_for_index_join(
//         &self,
//         ctx: context::Context,
//         lookup_contents: Vec<IndexJoinLookUpContent>,
//         index_ranges: Vec<ranger::Range>,
//         key_off2_idx_off: Vec<i32>,
//         cwc: Option<physicalop::ColWithCmpFuncManager>,
//         can_reorder_handles: bool,
//         mem_tracker: Option<memory::Tracker>,
//         interrupt_signal: Option<AtomicValue>,
//     ) -> Result<exec::Executor, Error>;
// }
//
// InnerCtx 对应 Go 内表上下文，保存内表列、join key、collator、前缀列长度与 null-safe equal 标记。
// pub struct InnerCtx {
//     pub reader_builder: Box<dyn IndexJoinExecutorBuilder>,
//     pub row_types: Vec<types::FieldType>,
//     pub key_cols: Vec<i32>,
//     pub key_col_ids: Vec<i64>,
//     pub key_collators: Vec<collate::Collator>,
//     pub hash_types: Vec<types::FieldType>,
//     pub hash_cols: Vec<i32>,
//     pub hash_collators: Vec<collate::Collator>,
// HashIsNullEQ 标记对应 hash key 是否来自 null-safe equal (<=>)。
//     pub hash_is_null_eq: Vec<bool>,
//     pub col_lens: Vec<i32>,
//     pub has_prefix_col: bool,
// }
//
// LookUpJoinTask 对应 Go 的任务对象，outer worker 生产，inner worker 填充内表结果，主线程消费。
// pub struct LookUpJoinTask {
//     pub outer_result: chunk::List,
//     pub outer_match: Vec<Vec<bool>>,
//     pub inner_result: Option<chunk::List>,
//     pub encoded_lookup_keys: Vec<chunk::Chunk>,
//     pub lookup_map: mvmap::MVMap,
//     pub matched_inners: Vec<chunk::Row>,
//     pub inner_exec: Option<exec::Executor>,
//     pub done_ch: Channel<Result<(), Error>>,
//     pub cursor: chunk::RowPtr,
//     pub has_match: bool,
//     pub has_null: bool,
//     pub mem_tracker: memory::Tracker,
// }
//
// OuterWorker 对应 Go outerWorker：读取外表 chunk、构造 lookup task，并把同一 task 推给 result/inner 两条通道。
// pub struct OuterWorker {
//     pub outer_ctx: OuterCtx,
//     pub lookup: *mut IndexLookUpJoin,
//     pub ctx: sessionctx::Context,
//     pub executor: exec::Executor,
//     pub max_batch_size: i32,
//     pub batch_size: i32,
//     pub result_ch: Sender<LookUpJoinTask>,
//     pub inner_ch: Sender<LookUpJoinTask>,
//     pub parent_mem_tracker: memory::Tracker,
// }
//
// InnerWorker 对应 Go innerWorker：按外表 key 构造 ranges，拉取内表结果并构建 lookupMap。
// pub struct InnerWorker {
//     pub inner_ctx: InnerCtx,
//     pub task_ch: Receiver<LookUpJoinTask>,
//     pub outer_ctx: OuterCtx,
//     pub ctx: sessionctx::Context,
//     pub lookup: *mut IndexLookUpJoin,
//     pub index_ranges: Vec<ranger::Range>,
//     pub next_col_compare_filters: Option<physicalop::ColWithCmpFuncManager>,
//     pub key_off2_idx_off: Vec<i32>,
//     pub max_fetch_size: i32,
//     pub stats: Option<InnerWorkerRuntimeStats>,
//     pub mem_tracker: memory::Tracker,
// }
//
// impl IndexLookUpJoin {
// Open 对应 Go Executor.Open：打开外表 child、校验 null-safe 标记、初始化内存跟踪与运行统计。
//     pub fn open(&mut self, ctx: context::Context) -> Result<(), Error> {
//         exec::open(ctx, self.children(0))?;
//         if self.inner_ctx.hash_is_null_eq.len() != self.inner_ctx.hash_cols.len() {
//             return Err(errors::new("index lookup join: hash null-eq flags length must match hash cols length"));
//         }
//         self.mem_tracker = Some(memory::NewTracker(self.id(), -1));
//         self.mem_tracker.as_mut().unwrap().AttachTo(self.ctx().GetSessionVars().StmtCtx.MemTracker);
//         self.inner_ptr_bytes = Vec::with_capacity(8);
//         self.finished.Store(false);
//         if self.runtime_stats().is_some() {
//             self.stats = Some(IndexLookUpJoinRuntimeStats::default());
//         }
//         self.cancel_func = None;
//         Ok(())
//     }
//
// startWorkers 对应 Go startWorkers：创建 result/inner 通道，派发一个 outer worker 和 N 个 inner worker。
//     pub fn start_workers(&mut self, ctx: context::Context, init_batch_size: i32) {
//         let concurrency = self.ctx().GetSessionVars().IndexLookupJoinConcurrency();
//         if let Some(stats) = &mut self.stats {
//             stats.concurrency = concurrency;
//         }
// Go 这里使用 context.WithCancel，并通过 WaitGroup 等待所有 worker 收尾。
//         let (worker_ctx, cancel_func) = context::with_cancel(ctx);
//         self.cancel_func = Some(cancel_func);
//         let result_ch = make_channel(concurrency);
//         let inner_ch = make_channel(concurrency);
//         self.result_ch = Some(result_ch.receiver());
//         self.worker_wg.Add(1);
//         go(self.new_outer_worker(result_ch.sender(), inner_ch.sender(), init_batch_size).run(worker_ctx.clone(), &self.worker_wg));
//         for _ in 0..concurrency {
//             self.worker_wg.Add(1);
//             go(self.new_inner_worker(inner_ch.receiver()).run(worker_ctx.clone(), &self.worker_wg));
//         }
//     }
//
// newOuterWorker 对应 Go 构造函数，batchSize 取 init 与 session 上限的较小值。
//     pub fn new_outer_worker(&self, result_ch: Sender<LookUpJoinTask>, inner_ch: Sender<LookUpJoinTask>, init_batch_size: i32) -> OuterWorker {
//         let max_batch_size = self.ctx().GetSessionVars().IndexJoinBatchSize;
//         OuterWorker {
//             outer_ctx: self.outer_ctx.clone(),
//             ctx: self.ctx(),
//             executor: self.children(0),
//             result_ch,
//             inner_ch,
//             batch_size: min(init_batch_size, max_batch_size),
//             max_batch_size,
//             parent_mem_tracker: self.mem_tracker.clone().unwrap(),
//             lookup: self as *const _ as *mut _,
//         }
//     }
//
// newInnerWorker 对应 Go 构造函数；每个 worker 复制 IndexRanges，避免并发修改造成 data race。
//     pub fn new_inner_worker(&self, task_ch: Receiver<LookUpJoinTask>) -> InnerWorker {
//         let mut copied_ranges = Vec::with_capacity(self.index_ranges.Range().len());
//         for ran in self.index_ranges.Range() {
//             copied_ranges.push(ran.Clone());
//         }
//         if !copied_ranges.is_empty() {
// Go 将 range 复制的生命周期记到 statement tracker，而不是 inner worker tracker。
//             self.ctx().GetSessionVars().StmtCtx.MemTracker.Consume(2 * types::EstimatedMemUsage(copied_ranges[0].LowVal, copied_ranges.len()));
//         }
//         let mut iw = InnerWorker {
//             inner_ctx: self.inner_ctx.clone(),
//             outer_ctx: self.outer_ctx.clone(),
//             task_ch,
//             ctx: self.ctx(),
//             index_ranges: copied_ranges,
//             key_off2_idx_off: self.key_off2_idx_off.clone(),
//             stats: self.stats.as_ref().map(|s| s.inner_worker.clone()),
//             lookup: self as *const _ as *mut _,
//             mem_tracker: memory::NewTracker(memory::LabelForIndexJoinInnerWorker, -1),
//             next_col_compare_filters: None,
//             max_fetch_size: 0,
//         };
//         failpoint::inject("inlNewInnerPanic", || panic!("test inlNewInnerPanic"));
//         iw.mem_tracker.AttachTo(self.mem_tracker.clone().unwrap());
//         if let Some(helper) = &self.last_col_helper {
// 每个 inner worker 复制 TmpConstant，避免并行执行时共享临时常量。
//             let mut next_cwf = helper.clone();
//             next_cwf.TmpConstant = helper.TmpConstant.iter().map(|_| expression::Constant { RetType: next_cwf.TargetCol.RetType.clone() }).collect();
//             iw.next_col_compare_filters = Some(next_cwf);
//         }
//         iw
//     }
//
// Next 对应 Go Executor.Next：取已完成 task，按外表 row 顺序查 lookupMap 并调用 Joiner 产生结果。
//     pub fn next(&mut self, ctx: context::Context, req: &mut chunk::Chunk) -> Result<(), Error> {
//         if !self.prepared {
//             self.start_workers(ctx.clone(), req.RequiredRows());
//             self.prepared = true;
//         }
//         if self.is_outer_join {
//             self.required_rows.Store(req.RequiredRows() as i64);
//         }
//         req.Reset();
//         self.join_result.Reset();
//         loop {
//             let task = match self.get_finished_task(ctx.clone())? {
//                 Some(task) => task,
//                 None => return Ok(()),
//             };
//             let start_time = time::Now();
//             if self.inner_iter.is_none() || self.inner_iter.as_ref().unwrap().Current() == self.inner_iter.as_ref().unwrap().End() {
//                 self.lookup_matched_inners(&task, task.cursor);
//                 self.inner_iter = Some(chunk::NewIterator4Slice(task.matched_inners.clone()));
//                 self.inner_iter.as_mut().unwrap().Begin();
//             }
//             let outer_row = task.outer_result.GetRow(task.cursor);
//             if self.inner_iter.as_ref().unwrap().Current() != self.inner_iter.as_ref().unwrap().End() {
//                 let (matched, is_null) = self.joiner.TryToMatchInners(outer_row, self.inner_iter.as_mut().unwrap(), req)?;
//                 task.has_match = task.has_match || matched;
//                 task.has_null = task.has_null || is_null;
//             }
//             if self.inner_iter.as_ref().unwrap().Current() == self.inner_iter.as_ref().unwrap().End() {
//                 if !task.has_match {
//                     self.joiner.OnMissMatch(task.has_null, outer_row, req);
//                 }
// Go 用 RowPtr 在 chunk 内推进，跨 chunk 时重置 RowIdx。
//                 advance_task_cursor(&task);
//             }
//             if let Some(stats) = &mut self.stats {
//                 stats.probe += time::Since(start_time);
//             }
//             if req.IsFull() {
//                 return Ok(());
//             }
//         }
//     }
//
// getFinishedTask 对应 Go 同名方法：复用未处理完的 task；新 task 必须等待 inner worker doneCh 完成。
//     pub fn get_finished_task(&mut self, ctx: context::Context) -> Result<Option<LookUpJoinTask>, Error> {
//         if let Some(task) = &self.task {
//             if task.cursor.ChkIdx < task.outer_result.NumChunks() {
//                 return Ok(self.task.clone());
//             }
//             task.mem_tracker.Detach();
//         }
//         let task = select_recv_or_cancel(self.result_ch.as_ref().unwrap(), ctx.clone())?;
//         if task.is_none() {
//             return Ok(None);
//         }
//         select_done_or_cancel(task.as_ref().unwrap().done_ch.clone(), ctx)?;
//         self.task = task.clone();
//         Ok(task)
//     }
//
// lookUpMatchedInners 对应 Go：从 encoded lookup key 找到 row pointer bytes，再转为 innerResult 中的 Row。
//     pub fn lookup_matched_inners(&mut self, task: &LookUpJoinTask, row_ptr: chunk::RowPtr) {
//         let outer_key = task.encoded_lookup_keys[row_ptr.ChkIdx as usize].GetRow(row_ptr.RowIdx as i32).GetBytes(0);
//         self.inner_ptr_bytes = task.lookup_map.Get(outer_key, self.inner_ptr_bytes.drain(..).collect());
//         task.matched_inners.clear();
//         for bytes in &self.inner_ptr_bytes {
// Go 通过 unsafe.Pointer 将 8 字节还原为 chunk.RowPtr；仅保留这层危险转换语义。
//             let ptr = unsafe_row_ptr_from_bytes(bytes);
//             task.matched_inners.push(task.inner_result.as_ref().unwrap().GetRow(ptr));
//         }
//     }
//
// Close 对应 Go Executor.Close：注册运行统计、取消 context、等待 worker、重置状态。
//     pub fn close(&mut self) -> Result<(), Error> {
//         if let Some(stats) = &self.stats {
//             self.ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.id(), stats.clone());
//         }
//         if let Some(cancel) = self.cancel_func.take() {
//             cancel();
//         }
//         self.worker_wg.Wait();
//         self.mem_tracker = None;
//         self.task = None;
//         self.finished.Store(false);
//         self.prepared = false;
//         self.base_executor.Close()
//     }
// }
//
// impl OuterWorker {
// run 对应 Go outerWorker.run：panic 会转成 task 错误发送给主线程，最后关闭 result/inner 通道并 Done。
//     pub fn run(&mut self, ctx: context::Context, wg: &sync::WaitGroup) {
//         let _region = trace::StartRegion(ctx.clone(), "IndexLookupJoinOuterWorker");
//         defer(|| {
//             close(self.result_ch.clone());
//             close(self.inner_ch.clone());
//             wg.Done();
//         });
//         loop {
//             failpoint::inject("TestIssue30211", || {});
//             failpoint::inject("ConsumeRandomPanic", || {});
//             let task = match self.build_task(ctx.clone()) {
//                 Ok(Some(task)) => task,
//                 Ok(None) => return,
//                 Err((mut task, err)) => {
//                     task.done_ch.send(Err(err));
//                     self.push_to_chan(ctx.clone(), task, self.result_ch.clone());
//                     return;
//                 }
//             };
//             if self.push_to_chan(ctx.clone(), task.clone(), self.inner_ch.clone()) {
//                 return;
//             }
//             if self.push_to_chan(ctx.clone(), task, self.result_ch.clone()) {
//                 return;
//             }
//         }
//     }
//
// pushToChan 对应 Go select：context 取消则通知调用方结束。
//     pub fn push_to_chan(&self, ctx: context::Context, task: LookUpJoinTask, dst: Sender<LookUpJoinTask>) -> bool {
//         select! {
//             _ = ctx.Done() => true,
//             _ = dst.send(task) => false,
//         }
//     }
//
// buildTask 对应 Go：读取一批 outer rows、可选执行外表过滤、为每个 chunk 准备 encoded lookup key 存储。
//     pub fn build_task(&mut self, ctx: context::Context) -> Result<Option<LookUpJoinTask>, (LookUpJoinTask, Error)> {
//         let mut task = LookUpJoinTask::new(new_list(self.executor.clone()), mvmap::NewMVMap());
//         task.mem_tracker.AttachTo(self.parent_mem_tracker.clone());
//         failpoint::inject("ConsumeRandomPanic", || {});
//         self.increase_batch_size();
//         let mut required_rows = self.batch_size;
//         if unsafe { (*self.lookup).is_outer_join } {
// outer join 下父算子 RequiredRows 会下推；Open 时还未设置，所以 0 表示继续使用 batchSize。
//             let parent_required = unsafe { (*self.lookup).required_rows.Load() } as i32;
//             if parent_required != 0 {
//                 required_rows = parent_required;
//             }
//         }
//         read_outer_rows_into_task(ctx, self, &mut task, required_rows).map_err(|e| (task.clone(), e))?;
//         if task.outer_result.Len() == 0 {
//             return Ok(None);
//         }
//         if !self.outer_ctx.filter.is_empty() {
// Go 对每个 chunk 调 VectorizedFilter，并记录 bool slice 供 lookup key 构造阶段跳过未命中行。
//             vectorized_filter_outer_rows(self, &mut task).map_err(|e| (task.clone(), e))?;
//         }
//         task.encoded_lookup_keys = alloc_lookup_key_chunks(self.executor.clone(), &task.outer_result);
//         Ok(Some(task))
//     }
//
// increaseBatchSize 对应 Go 指数增长，不能超过 session 配置的 maxBatchSize。
//     pub fn increase_batch_size(&mut self) {
//         if self.batch_size < self.max_batch_size {
//             self.batch_size *= 2;
//         }
//         if self.batch_size > self.max_batch_size {
//             self.batch_size = self.max_batch_size;
//         }
//     }
// }
//
// newList 对应 Go 辅助函数：按 executor 返回列类型创建 chunk.List。
// pub fn new_list(e: exec::Executor) -> chunk::List {
//     chunk::NewList(e.RetFieldTypes(), e.InitCap(), e.MaxChunkSize())
// }
//
// IndexJoinLookUpContent 对应 Go 内容对象；KeyColIDs 是动态分区裁剪需要的原表列 ID。
// pub struct IndexJoinLookUpContent {
//     pub keys: Vec<types::Datum>,
//     pub row: chunk::Row,
//     pub key_cols: Vec<i32>,
//     pub key_col_ids: Vec<i64>,
// }
//
// impl InnerWorker {
// run 对应 Go innerWorker.run：从 taskCh 消费任务，handleTask 的错误通过 task.doneCh 回传。
//     pub fn run(&mut self, ctx: context::Context, wg: &sync::WaitGroup) {
//         let _region = trace::StartRegion(ctx.clone(), "IndexLookupJoinInnerWorker");
//         defer(|| wg.Done());
//         loop {
//             let Some(mut task) = select_recv_or_cancel(&self.task_ch, ctx.clone()).unwrap_or(None) else {
//                 return;
//             };
//             let err = self.handle_task(ctx.clone(), &mut task);
//             task.done_ch.send(err.map(|_| ()));
//         }
//     }
//
// handleTask 对应 Go：构造 lookup 内容、拉内表结果、构建 outer key 到 inner row pointer 的 map。
//     pub fn handle_task(&mut self, ctx: context::Context, task: &mut LookUpJoinTask) -> Result<(), Error> {
//         let start = time::Now();
//         defer(|| {
//             self.mem_tracker.Consume(-self.mem_tracker.BytesConsumed());
//             if let Some(stats) = &mut self.stats {
//                 stats.total_time += time::Since(start);
//             }
//         });
//         let lookup_contents = self.construct_lookup_content(task)?;
//         self.fetch_inner_results(ctx, task, lookup_contents)?;
//         self.build_lookup_map(task)?;
//         Ok(())
//     }
//
// constructLookupContent 对应 Go：为每个 outer row 同时构造 range lookup key 和 hash lookup key。
//     pub fn construct_lookup_content(&mut self, task: &mut LookUpJoinTask) -> Result<Vec<IndexJoinLookUpContent>, Error> {
//         let mut lookup_contents = Vec::with_capacity(task.outer_result.Len());
//         let mut key_buf = Vec::with_capacity(64);
//         for chk_idx in 0..task.outer_result.NumChunks() {
//             let chk = task.outer_result.GetChunk(chk_idx);
//             for row_idx in 0..chk.NumRows() {
//                 let (mut d_lookup_key, d_hash_key) = match self.construct_datum_lookup_key(task, chk_idx, row_idx) {
//                     Ok(v) => v,
//                     Err(err) if terror::ErrorEqual(err, types::ErrWrongValue) => {
// Go 忽略 invalid datetime 行，并 append null 保持 encodedLookUpKeys 与 outerResult 等长。
//                         task.encoded_lookup_keys[chk_idx].AppendNull(0);
//                         continue;
//                     }
//                     Err(err) => return Err(err),
//                 };
//                 if d_hash_key.is_none() {
//                     task.encoded_lookup_keys[chk_idx].AppendNull(0);
//                     continue;
//                 }
//                 key_buf.clear();
//                 key_buf = codec::EncodeKey(self.ctx.GetSessionVars().StmtCtx.TimeZone(), key_buf, d_hash_key.unwrap())?;
//                 task.encoded_lookup_keys[chk_idx].AppendBytes(0, key_buf.clone());
//                 if self.inner_ctx.has_prefix_col {
// 前缀索引列在构造 range 前裁剪 datum；排序去重在后续统一做。
//                     cut_prefix_columns(&mut d_lookup_key, &self.inner_ctx);
//                 }
//                 lookup_contents.push(IndexJoinLookUpContent {
//                     keys: d_lookup_key,
//                     row: chk.GetRow(row_idx),
//                     key_cols: self.inner_ctx.key_cols.clone(),
//                     key_col_ids: self.inner_ctx.key_col_ids.clone(),
//                 });
//             }
//         }
//         for chk in &task.encoded_lookup_keys {
//             task.mem_tracker.Consume(chk.MemoryUsage());
//         }
//         Ok(self.sort_and_dedup_lookup_contents(lookup_contents))
//     }
//
// constructDatumLookupKey 对应 Go：处理外表过滤、NULL、类型转换、collation 比较和 null-safe equal。
//     pub fn construct_datum_lookup_key(&self, task: &LookUpJoinTask, chk_idx: usize, row_idx: usize) -> Result<(Vec<types::Datum>, Option<Vec<types::Datum>>), Error> {
//         if !task.outer_match.is_empty() && !task.outer_match[chk_idx][row_idx] {
//             return Ok((Vec::new(), None));
//         }
//         let outer_row = task.outer_result.GetChunk(chk_idx).GetRow(row_idx);
//         let sc = self.ctx.GetSessionVars().StmtCtx;
//         let key_len = self.inner_ctx.key_cols.len();
//         let mut d_lookup_key = Vec::with_capacity(key_len);
//         let mut d_hash_key = Vec::with_capacity(self.inner_ctx.hash_cols.len());
//         for (i, hash_col) in self.outer_ctx.hash_cols.iter().enumerate() {
//             let outer_value = outer_row.GetDatum(*hash_col, self.outer_ctx.row_types[*hash_col as usize].clone());
//             if outer_value.IsNull() {
//                 if !self.inner_ctx.hash_is_null_eq[i] {
//                     return Ok((Vec::new(), None));
//                 }
//                 if i < key_len {
//                     d_lookup_key.push(outer_value.clone());
//                 }
//                 d_hash_key.push(outer_value);
//                 continue;
//             }
//             let inner_col_type = self.inner_ctx.row_types[self.inner_ctx.hash_cols[i] as usize].clone();
//             let inner_value = outer_value.ConvertTo(sc.TypeCtx(), inner_col_type)?;
//             let cmp = outer_value.Compare(sc.TypeCtx(), inner_value.clone(), self.inner_ctx.hash_collators[i].clone())?;
//             if cmp != 0 {
// 转换后值不等于原外表值时，Go 直接跳过 lookup，避免错误匹配。
//                 return Ok((Vec::new(), None));
//             }
//             if i < key_len {
//                 d_lookup_key.push(inner_value.clone());
//             }
//             d_hash_key.push(inner_value);
//         }
//         Ok((d_lookup_key, Some(d_hash_key)))
//     }
//
// sortAndDedupLookUpContents 对应 Go：按 lookup key 排序，并结合 last-col helper 做去重。
//     pub fn sort_and_dedup_lookup_contents(&self, mut lookup_contents: Vec<IndexJoinLookUpContent>) -> Vec<IndexJoinLookUpContent> {
//         if lookup_contents.len() < 2 {
//             return lookup_contents;
//         }
//         let sc = self.ctx.GetSessionVars().StmtCtx;
//         lookup_contents.sort_by(|i, j| {
//             let cmp = compare_row(sc, &i.keys, &j.keys, &self.inner_ctx.key_collators);
//             if cmp != 0 || self.next_col_compare_filters.is_none() {
//                 return cmp;
//             }
//             self.next_col_compare_filters.as_ref().unwrap().CompareRow(i.row, j.row)
//         });
//         dedup_lookup_contents(sc, lookup_contents, &self.inner_ctx.key_collators, self.next_col_compare_filters.as_ref())
//     }
//
// fetchInnerResults 对应 Go：首次构建 innerExec，循环 Next 内表 chunk，必要时关闭 innerExec。
//     pub fn fetch_inner_results(&mut self, ctx: context::Context, task: &mut LookUpJoinTask, lookup_content: Vec<IndexJoinLookUpContent>) -> Result<(), Error> {
//         if task.inner_exec.is_none() {
//             let inner_exec = self.inner_ctx.reader_builder.build_executor_for_index_join(ctx.clone(), lookup_content, self.index_ranges.clone(), self.key_off2_idx_off.clone(), self.next_col_compare_filters.clone(), true, Some(self.mem_tracker.clone()), Some(unsafe { (*self.lookup).finished.clone() }))?;
//             task.inner_exec = Some(inner_exec);
//             task.inner_result = Some(chunk::NewList(exec::RetTypes(task.inner_exec.as_ref().unwrap()), self.ctx.GetSessionVars().InitChunkSize, self.ctx.GetSessionVars().MaxChunkSize));
//             task.inner_result.as_mut().unwrap().GetMemTracker().AttachTo(task.mem_tracker.clone());
//         } else if let Some(inner_result) = &mut task.inner_result {
//             inner_result.Reset();
//         }
//         let mut need_close = false;
//         loop {
//             if ctx.Done().is_ready() {
//                 need_close = true;
//                 return Err(ctx.Err());
//             }
//             let executor_chk = task.inner_result.as_mut().unwrap().AllocChunk();
//             exec::Next(ctx.clone(), task.inner_exec.as_ref().unwrap(), executor_chk.clone())?;
//             failpoint::inject("ConsumeRandomPanic", || {});
//             if executor_chk.NumRows() == 0 {
//                 need_close = true;
//                 break;
//             }
//             task.inner_result.as_mut().unwrap().Add(executor_chk);
//             if self.max_fetch_size > 0 && task.inner_result.as_ref().unwrap().Len() >= self.max_fetch_size {
//                 break;
//             }
//         }
//         if need_close {
//             terror::Log(exec::Close(task.inner_exec.take().unwrap()));
//         }
//         Ok(())
//     }
//
// buildLookUpMap 对应 Go：按内表 hash key 编码，把 RowPtr 以 unsafe 字节写入 mvmap。
//     pub fn build_lookup_map(&self, task: &mut LookUpJoinTask) -> Result<(), Error> {
//         let mut key_buf = Vec::with_capacity(64);
//         let mut val_buf = vec![0_u8; 8];
//         for i in 0..task.inner_result.as_ref().unwrap().NumChunks() {
//             let chk = task.inner_result.as_ref().unwrap().GetChunk(i);
//             for j in 0..chk.NumRows() {
//                 let inner_row = chk.GetRow(j);
//                 if self.has_null_in_join_key(inner_row) {
//                     continue;
//                 }
//                 key_buf.clear();
//                 for key_col in &self.inner_ctx.hash_cols {
//                     let d = inner_row.GetDatum(*key_col, self.inner_ctx.row_types[*key_col as usize].clone());
//                     key_buf = codec::EncodeKey(self.ctx.GetSessionVars().StmtCtx.TimeZone(), key_buf, vec![d])?;
//                 }
//                 write_row_ptr_bytes(&mut val_buf, chunk::RowPtr { ChkIdx: i as u32, RowIdx: j as u32 });
//                 task.lookup_map.Put(key_buf.clone(), val_buf.clone());
//             }
//         }
//         Ok(())
//     }
//
// hasNullInJoinKey 对应 Go：普通等值 join key 遇到 NULL 需要跳过，null-safe equal 允许保留。
//     pub fn has_null_in_join_key(&self, row: chunk::Row) -> bool {
//         for (i, key_col) in self.inner_ctx.hash_cols.iter().enumerate() {
//             if row.IsNull(*key_col) && !self.inner_ctx.hash_is_null_eq[i] {
//                 return true;
//             }
//         }
//         false
//     }
// }
//
// compareRow 对应 Go 顶层函数：逐列按 collator 比较，原实现只比较同类型 datum，错误仅记录。
// pub fn compare_row(sc: stmtctx::StatementContext, left: &[types::Datum], right: &[types::Datum], ctors: &[collate::Collator]) -> i32 {
//     for idx in 0..left.len() {
//         let (cmp, err) = left[idx].Compare(sc.TypeCtx(), right[idx].clone(), ctors[idx].clone());
//         terror::Log(err);
//         if cmp > 0 {
//             return 1;
//         }
//         if cmp < 0 {
//             return -1;
//         }
//     }
//     0
// }
//
// IndexLookUpJoinRuntimeStats 对应 Go runtime stats，保留 concurrency/probe/inner worker 耗时聚合。
// #[derive(Default, Clone)]
// pub struct IndexLookUpJoinRuntimeStats {
//     pub concurrency: i32,
//     pub probe: i64,
//     pub inner_worker: InnerWorkerRuntimeStats,
// }
//
// #[derive(Default, Clone)]
// pub struct InnerWorkerRuntimeStats {
//     pub total_time: i64,
//     pub task: i64,
//     pub construct: i64,
//     pub fetch: i64,
//     pub build: i64,
//     pub join: i64,
// }
//
// impl IndexLookUpJoinRuntimeStats {
// String 对应 Go RuntimeStats.String，按原格式输出 inner/probe 片段。
//     pub fn string(&self) -> String {
//         let mut buf = String::with_capacity(16);
//         if self.inner_worker.total_time > 0 {
//             buf.push_str("inner:{total:");
//             buf.push_str(&execdetails::FormatDuration(self.inner_worker.total_time));
//             buf.push_str(", concurrency:");
//             buf.push_str(if self.concurrency > 0 { &self.concurrency.to_string() } else { "OFF" });
//             buf.push_str(", task:");
//             buf.push_str(&self.inner_worker.task.to_string());
//             buf.push_str(", construct:");
//             buf.push_str(&execdetails::FormatDuration(self.inner_worker.construct));
//             buf.push_str(", fetch:");
//             buf.push_str(&execdetails::FormatDuration(self.inner_worker.fetch));
//             buf.push_str(", build:");
//             buf.push_str(&execdetails::FormatDuration(self.inner_worker.build));
//             if self.inner_worker.join > 0 {
//                 buf.push_str(", join:");
//                 buf.push_str(&execdetails::FormatDuration(self.inner_worker.join));
//             }
//             buf.push('}');
//         }
//         if self.probe > 0 {
//             buf.push_str(", probe:");
//             buf.push_str(&execdetails::FormatDuration(self.probe));
//         }
//         buf
//     }
//
// Clone/Merge/Tp 对应 Go RuntimeStats 接口方法。
//     pub fn clone_stats(&self) -> Self {
//         self.clone()
//     }
//
//     pub fn merge(&mut self, rs: IndexLookUpJoinRuntimeStats) {
//         self.probe += rs.probe;
//         self.inner_worker.total_time += rs.inner_worker.total_time;
//         self.inner_worker.task += rs.inner_worker.task;
//         self.inner_worker.construct += rs.inner_worker.construct;
//         self.inner_worker.fetch += rs.inner_worker.fetch;
//         self.inner_worker.build += rs.inner_worker.build;
//         self.inner_worker.join += rs.inner_worker.join;
//     }
//
//     pub fn tp(&self) -> i32 {
//         execdetails::TpIndexLookUpJoinRuntimeStats
//     }
// }
// */
use crate::joiner::{Joiner, NaajType, Predicate, Row};
use crate::row_table_builder::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use astersql_executor_internal_exec::adaptive_limit_controller::{
    AdaptiveLimitController, AdaptiveLimitSnapshot,
};

/// 行级过滤谓词，与 Joiner 的 `Predicate` 同型。
pub type RowFilter = Predicate;

/// 单次索引 lookup 所需内容：外表 key、行及内表 key 列元数据。
#[derive(Clone, Debug, PartialEq)]
pub struct IndexJoinLookupContent {
    /// 外表连接键值。
    pub keys: Row,
    /// 对应外表完整行。
    pub row: Row,
    /// 内表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 内表 join key 列 ID（对齐 Go keyColumnIDs）。
    pub key_column_ids: Vec<i64>,
}

/// 根据外表 lookup 内容构造内表执行器并拉取匹配行。
pub trait IndexJoinExecutorBuilder: Send + Sync {
    fn build(&self, lookup_contents: &[IndexJoinLookupContent]) -> Result<Vec<Row>, String>;
}

/// 外表上下文：待连接行、key 列与过滤条件。
#[derive(Clone, Default)]
pub struct OuterCtx {
    /// 外表行缓冲。
    pub rows: Vec<Row>,
    /// 外表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 外表行过滤器（不满足则不参与 lookup）。
    pub filters: Vec<RowFilter>,
}

/// 内表上下文：索引执行器构建器与 key 列信息。
pub struct InnerCtx {
    /// 按 lookup key 构建内表扫描的 builder。
    pub builder: Box<dyn IndexJoinExecutorBuilder>,
    /// 内表 join key 列下标。
    pub key_columns: Vec<usize>,
    /// 内表 join key 列 ID。
    pub key_column_ids: Vec<i64>,
    /// 是否对所有 lookup key 启用 NULL-safe equal（`<=>`）语义。
    pub null_safe: bool,
}

/// 一批外表行的 lookup 任务：含匹配标记、内表结果与 key→行 哈希表。
#[derive(Clone, Debug, Default)]
pub struct LookUpJoinTask {
    /// 本批外表行。
    pub outer_rows: Vec<Row>,
    /// 各外表行是否通过 filter。
    pub outer_match: Vec<bool>,
    /// 去重后的 lookup 内容列表。
    pub lookup_contents: Vec<IndexJoinLookupContent>,
    /// 内表拉取到的行。
    pub inner_rows: Vec<Row>,
    /// 编码 key → 内表行列表，供主线程 join。
    pub lookup_map: HashMap<Vec<u8>, Vec<Row>>,
    /// 任务内游标（保留 Go 字段语义）。
    pub cursor: usize,
    /// inner worker 是否已完成该任务。
    pub done: bool,
}

impl LookUpJoinTask {
    /// 用外表行构造任务，默认全部 `outer_match=true`。
    pub fn new(outer_rows: Vec<Row>) -> Self {
        let length = outer_rows.len();
        Self {
            outer_rows,
            outer_match: vec![true; length],
            ..Self::default()
        }
    }
}

/// 外表 worker：按递增 batch 从 `OuterCtx` 切出 lookup 任务。
#[derive(Clone, Debug)]
pub struct OuterWorker {
    /// 每次 open 使用的初始批大小。
    initial_batch_size: usize,
    /// 当前批大小（会逐步加倍直至上限）。
    batch_size: usize,
    /// 批大小上限。
    max_batch_size: usize,
    /// 外表行读取游标。
    cursor: usize,
}
impl OuterWorker {
    /// 校验批大小后创建 worker。
    pub fn new(initial_batch_size: usize, max_batch_size: usize) -> Result<Self, String> {
        if initial_batch_size == 0 || max_batch_size == 0 {
            return Err("batch sizes must be positive".into());
        }
        Ok(Self {
            initial_batch_size: initial_batch_size.min(max_batch_size),
            batch_size: initial_batch_size.min(max_batch_size),
            max_batch_size,
            cursor: 0,
        })
    }
    /// 切下一批外表行并评估 filter，随后放大下一批 batch。
    pub fn build_task(&mut self, context: &OuterCtx) -> Result<Option<LookUpJoinTask>, String> {
        if self.cursor >= context.rows.len() {
            return Ok(None);
        }
        // Go buildTask 在计算 requiredRows 前先扩大当前 batch。
        self.increase_batch_size();
        let end = (self.cursor + self.batch_size).min(context.rows.len());
        let mut task = LookUpJoinTask::new(context.rows[self.cursor..end].to_vec());
        self.cursor = end;
        // 不满足 filter 的外表行标记为 false，后续不再构造 lookup key。
        for (index, row) in task.outer_rows.iter().enumerate() {
            task.outer_match[index] = filters_match(row, &context.filters)?;
        }
        Ok(Some(task))
    }
    /// 将 batch 加倍，不超过 `max_batch_size`。
    pub fn increase_batch_size(&mut self) {
        self.batch_size = (self.batch_size.saturating_mul(2)).min(self.max_batch_size);
    }
    /// 重置外表读取游标，供重新 open。
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.batch_size = self.initial_batch_size;
    }
}

/// 内表 worker：构造 lookup key、拉取内表行并建 lookup map。
pub struct InnerWorker<'a> {
    /// 共享内表上下文。
    pub context: &'a InnerCtx,
}
impl InnerWorker<'_> {
    /// 从通过 filter 的外表行提取非 NULL key，排序去重后写入 `lookup_contents`。
    pub fn construct_lookup_content(
        &self,
        task: &mut LookUpJoinTask,
        outer_key_columns: &[usize],
    ) -> Result<(), String> {
        task.lookup_contents.clear();
        for (index, row) in task.outer_rows.iter().enumerate() {
            if !task.outer_match[index] {
                continue;
            }
            let mut keys = Vec::with_capacity(outer_key_columns.len());
            let mut valid = true;
            for column in outer_key_columns {
                let value = row
                    .get(*column)
                    .ok_or_else(|| format!("outer key column {column} is out of bounds"))?;
                if !self.context.null_safe && matches!(value, Value::Null) {
                    valid = false;
                    break;
                }
                keys.push(value.clone());
            }
            if valid {
                task.lookup_contents.push(IndexJoinLookupContent {
                    keys,
                    row: row.clone(),
                    key_columns: self.context.key_columns.clone(),
                    key_column_ids: self.context.key_column_ids.clone(),
                });
            }
        }
        // 排序去重，减少重复索引 ranges 扫描。
        task.lookup_contents
            .sort_by(|left, right| compare_row(&left.keys, &right.keys));
        task.lookup_contents
            .dedup_by(|left, right| compare_row(&left.keys, &right.keys).is_eq());
        Ok(())
    }
    /// 调用 builder 按 lookup 内容拉取内表行。
    pub fn fetch_inner_results(&self, task: &mut LookUpJoinTask) -> Result<(), String> {
        task.inner_rows = self.context.builder.build(&task.lookup_contents)?;
        Ok(())
    }
    /// 按内表 key 编码建立 `lookup_map`，供主线程按外表顺序探测。
    pub fn build_lookup_map(&self, task: &mut LookUpJoinTask) -> Result<(), String> {
        task.lookup_map.clear();
        for row in &task.inner_rows {
            let key = extract_key(row, &self.context.key_columns)?;
            if !self.context.null_safe && key.iter().any(|value| matches!(value, Value::Null)) {
                continue;
            }
            task.lookup_map
                .entry(encode_key(&key))
                .or_default()
                .push(row.clone());
        }
        Ok(())
    }
    /// 依次执行构造 key → 拉内表 → 建 map，并标记任务完成。
    pub fn handle_task(
        &self,
        task: &mut LookUpJoinTask,
        outer_key_columns: &[usize],
    ) -> Result<(), String> {
        self.construct_lookup_content(task, outer_key_columns)?;
        self.fetch_inner_results(task)?;
        self.build_lookup_map(task)?;
        task.done = true;
        Ok(())
    }
}

/// Index LookUp Join 执行器：外表分批 + 内表并发 lookup + Joiner 保序输出。
pub struct IndexLookUpJoin {
    /// 外表行与 key/filter 配置。
    pub outer_context: OuterCtx,
    /// 内表 builder 与 key 配置。
    pub inner_context: InnerCtx,
    /// 连接器：负责 key 匹配与未匹配补行。
    pub joiner: Joiner,
    /// 是否保持外表顺序（当前实现主线程本就按批顺序输出）。
    pub keep_outer_order: bool,
    /// 外表分批 worker。
    worker: OuterWorker,
    /// 已产出的结果行缓冲。
    output: Vec<Row>,
    /// 结果读取游标。
    cursor: usize,
    /// 是否已 open。
    opened: bool,
    /// 是否已 close；close 后必须显式 open，不得由 next 隐式重开。
    closed: bool,
    /// 当前 open 周期是否已执行，避免空结果被重复执行。
    executed: bool,
    /// 运行时统计。
    pub stats: IndexLookUpJoinRuntimeStats,
    /// True when the physical outer property requires ordered output.
    pub adaptive_limit_eligible: bool,
    /// Controller shared with the eligible outer lookup reader and LIMIT.
    pub adaptive_limit_controller: Option<Arc<AdaptiveLimitController>>,
}

impl IndexLookUpJoin {
    /// 创建执行器并初始化外表 worker。
    pub fn new(
        outer_context: OuterCtx,
        inner_context: InnerCtx,
        joiner: Joiner,
        keep_outer_order: bool,
        initial_batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Ok(Self {
            outer_context,
            inner_context,
            joiner,
            keep_outer_order,
            worker: OuterWorker::new(initial_batch_size, max_batch_size)?,
            output: Vec::new(),
            cursor: 0,
            opened: false,
            closed: false,
            executed: false,
            stats: IndexLookUpJoinRuntimeStats::default(),
            adaptive_limit_eligible: keep_outer_order,
            adaptive_limit_controller: None,
        })
    }
    /// 打开执行器：重置 worker 与输出缓冲。
    pub fn open(&mut self) -> Result<(), String> {
        self.worker.reset();
        self.output.clear();
        self.cursor = 0;
        self.opened = true;
        self.closed = false;
        self.executed = false;
        Ok(())
    }
    /// 驱动全部外表任务：inner worker 建 lookup map 后按外表顺序 Joiner 出结果。
    fn execute(&mut self) -> Result<(), String> {
        let start = Instant::now();
        while let Some(mut task) = self.worker.build_task(&self.outer_context)? {
            let inner_start = Instant::now();
            {
                let worker = InnerWorker {
                    context: &self.inner_context,
                };
                worker.handle_task(&mut task, &self.outer_context.key_columns)?;
            }
            self.stats.inner_worker.total_time += inner_start.elapsed();
            self.stats.inner_worker.tasks += 1;
            // 按外表顺序探测：未过 filter / 无匹配时走 on_miss_match。
            for (index, outer) in task.outer_rows.iter().enumerate() {
                if !task.outer_match[index] {
                    self.joiner.on_miss_match(false, outer, &mut self.output);
                    continue;
                }
                let key = extract_key(outer, &self.outer_context.key_columns)?;
                let inners = task
                    .lookup_map
                    .get(&encode_key(&key))
                    .cloned()
                    .unwrap_or_default();
                let result = self.joiner.try_to_match_inners(
                    outer,
                    &inners,
                    &mut self.output,
                    NaajType::Unknown,
                )?;
                if !result.matched {
                    self.joiner
                        .on_miss_match(result.has_null, outer, &mut self.output);
                }
            }
        }
        self.stats.probe += start.elapsed();
        self.executed = true;
        Ok(())
    }
    /// 惰性执行并按 `required_rows` 切片返回结果。
    pub fn next(&mut self, required_rows: usize) -> Result<Vec<Row>, String> {
        if self.closed {
            return Err("cannot reopen closed index lookup join".into());
        }
        if !self.opened {
            self.open()?;
        }
        if required_rows == 0 {
            return Ok(Vec::new());
        }
        if !self.executed {
            self.execute()?;
        }
        if self.cursor >= self.output.len() {
            return Ok(Vec::new());
        }
        let end = (self.cursor + required_rows).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(rows)
    }
    /// 关闭执行器并释放当前执行状态；保留输入以允许再次 open。
    pub fn close(&mut self) {
        if let Some(controller) = &self.adaptive_limit_controller {
            self.stats.adaptive_limit_snapshot = Some(controller.Snapshot());
        }
        self.output.clear();
        self.cursor = 0;
        self.opened = false;
        self.closed = true;
        self.executed = false;
    }
}

/// 内表 worker 各阶段耗时与任务计数。
#[derive(Clone, Debug, Default)]
pub struct InnerWorkerRuntimeStats {
    /// 处理全部任务的总耗时。
    pub total_time: Duration,
    /// 完成的任务数。
    pub tasks: u64,
    /// 构造 lookup key 耗时。
    pub construct: Duration,
    /// 拉取内表耗时。
    pub fetch: Duration,
    /// 建 lookup map 耗时。
    pub build: Duration,
    /// join 阶段耗时（保留 Go 字段）。
    pub join: Duration,
}
/// Index LookUp Join 整体运行时统计。
#[derive(Clone, Debug, Default)]
pub struct IndexLookUpJoinRuntimeStats {
    /// 内表并发度。
    pub concurrency: usize,
    /// 内表 worker 细分统计。
    pub inner_worker: InnerWorkerRuntimeStats,
    /// 主线程 probe/join 总耗时。
    pub probe: Duration,
    /// One executor-lifecycle snapshot; merges retain rather than add it.
    pub adaptive_limit_snapshot: Option<AdaptiveLimitSnapshot>,
}
impl IndexLookUpJoinRuntimeStats {
    /// 统计类型编号（对齐 Go TpIndexLookUpJoinRuntimeStats）。
    pub const TYPE: u8 = 6;
    /// 合并另一份统计；与 Go 一致，concurrency 保留接收者的值。
    pub fn merge(&mut self, other: &Self) {
        self.probe += other.probe;
        self.inner_worker.total_time += other.inner_worker.total_time;
        self.inner_worker.tasks += other.inner_worker.tasks;
        self.inner_worker.construct += other.inner_worker.construct;
        self.inner_worker.fetch += other.inner_worker.fetch;
        self.inner_worker.build += other.inner_worker.build;
        self.inner_worker.join += other.inner_worker.join;
        if self.adaptive_limit_snapshot.is_none() {
            self.adaptive_limit_snapshot = other.adaptive_limit_snapshot;
        }
    }
}
impl std::fmt::Display for IndexLookUpJoinRuntimeStats {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.inner_worker.total_time.is_zero() {
            write!(
                formatter,
                "inner:{{total:{:?}, concurrency:{}, task:{}, construct:{:?}, fetch:{:?}, build:{:?}",
                self.inner_worker.total_time,
                if self.concurrency > 0 {
                    self.concurrency.to_string()
                } else {
                    "OFF".into()
                },
                self.inner_worker.tasks,
                self.inner_worker.construct,
                self.inner_worker.fetch,
                self.inner_worker.build,
            )?;
            if !self.inner_worker.join.is_zero() {
                write!(formatter, ", join:{:?}", self.inner_worker.join)?;
            }
            write!(formatter, "}}")?;
        }
        if !self.probe.is_zero() {
            write!(formatter, ", probe:{:?}", self.probe)?;
        }
        if let Some(snapshot) = self.adaptive_limit_snapshot {
            if !self.inner_worker.total_time.is_zero() || !self.probe.is_zero() {
                write!(formatter, ", ")?;
            }
            write!(
                formatter,
                "adaptive:{{outer:{}/{}, lookup:{}/{}, outstanding:{}/{}, blocked:outer={:?},lookup={:?}}}",
                snapshot.outer_fetched,
                snapshot.outer_consumed,
                snapshot.lookup_handles,
                snapshot.lookup_rows,
                snapshot.outer_outstanding_at_stop,
                snapshot.lookup_outstanding_at_stop,
                snapshot.outer_admission_blocked,
                snapshot.lookup_admission_blocked,
            )?;
        }
        Ok(())
    }
}

/// 按列逐个比较两行 key 值。
pub fn compare_row(left: &[Value], right: &[Value]) -> std::cmp::Ordering {
    left.iter()
        .zip(right)
        .map(compare_value)
        .find(|order| !order.is_eq())
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}
/// 按列下标从行中抽取 join key；越界报错。
pub fn extract_key(row: &Row, columns: &[usize]) -> Result<Row, String> {
    columns
        .iter()
        .map(|column| {
            row.get(*column)
                .cloned()
                .ok_or_else(|| format!("key column {column} is out of bounds"))
        })
        .collect()
}
/// 将 key 行编码为 lookup map 的字节键（Debug 格式占位，对齐迁移语义）。
pub fn encode_key(row: &[Value]) -> Vec<u8> {
    format!("{row:?}").into_bytes()
}
/// 所有 filter 均返回 `Some(true)` 才视为匹配。
fn filters_match(row: &Row, filters: &[RowFilter]) -> Result<bool, String> {
    for filter in filters {
        if filter(row)? != Some(true) {
            return Ok(false);
        }
    }
    Ok(true)
}
/// 单列 Value 比较：NULL 最小，同型按自然序，异型回退到 Debug 字符串。
fn compare_value(left: (&Value, &Value)) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match left {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Less,
        (_, Value::Null) => Ordering::Greater,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::UInt(a), Value::UInt(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        (a, b) => format!("{a:?}").cmp(&format!("{b:?}")),
    }
}
