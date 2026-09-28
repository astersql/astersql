// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Index LookUp Merge Join（索引查找归并连接）执行器。
//
// 外表按 join key 有序（或先排序）批量 lookup 内表；inner worker 取回内表行后
// 按排序 key 做 merge join（归并连接）：双指针推进相同 key 组并产出结果。
// 适用于内外表均按连接键有序的场景。

// IndexLookUpMergeJoin 如何用外表批量 lookup 内表，再在 inner worker 中按排序 key 做 merge join。
// failpoint、memory tracker 和 executor 调用均作为 Go 语义占位保留，方便后续逐步接线。
//
// IndexLookUpMergeJoin 对应 Go 同名结构：外表有序输出，内表按 lookup key 取回后由 merge join 产生结果。
// pub struct IndexLookUpMergeJoin {
//     pub base_executor: exec::BaseExecutor,
//     pub result_ch: Option<Receiver<LookUpMergeJoinTask>>,
//     pub cancel_func: Option<context::CancelFunc>,
//     pub worker_wg: sync::WaitGroup,
//     pub outer_merge_ctx: OuterMergeCtx,
//     pub inner_merge_ctx: InnerMergeCtx,
//     pub joiners: Vec<Joiner>,
//     pub join_chk_resource_ch: Vec<Channel<chunk::Chunk>>,
//     pub is_outer_join: bool,
//     pub required_rows: AtomicI64,
//     pub task: Option<LookUpMergeJoinTask>,
//     pub index_ranges: ranger::MutableRanges,
//     pub key_off2_idx_off: Vec<i32>,
// LastColHelper 处理最后一列比较条件，复制给每个 worker 以避免并发共享 TmpConstant。
//     pub last_col_helper: Option<physicalop::ColWithCmpFuncManager>,
//     pub mem_tracker: Option<memory::Tracker>,
//     pub prepared: bool,
// }
//
// OuterMergeCtx 对应 Go 外表 merge join 上下文，保存外表 join key、过滤条件和排序比较函数。
// pub struct OuterMergeCtx {
//     pub row_types: Vec<types::FieldType>,
//     pub join_keys: Vec<expression::Column>,
//     pub key_cols: Vec<i32>,
//     pub filter: expression::CNFExprs,
//     pub need_outer_sort: bool,
//     pub compare_funcs: Vec<expression::CompareFunc>,
// }
//
// InnerMergeCtx 对应 Go 内表 merge join 上下文，包含构造 inner executor 和比较内外 key 所需信息。
// pub struct InnerMergeCtx {
//     pub reader_builder: Box<dyn IndexJoinExecutorBuilder>,
//     pub row_types: Vec<types::FieldType>,
//     pub join_keys: Vec<expression::Column>,
//     pub key_cols: Vec<i32>,
//     pub key_col_ids: Vec<i64>,
//     pub key_collators: Vec<collate::Collator>,
//     pub compare_funcs: Vec<expression::CompareFunc>,
//     pub col_lens: Vec<i32>,
//     pub desc: bool,
//     pub key_off2_key_off_order_by_idx: Vec<i32>,
// }
//
// LookUpMergeJoinTask 对应 Go 任务；results 是 inner worker 向主线程返回结果 chunk 的通道。
// pub struct LookUpMergeJoinTask {
//     pub outer_result: chunk::List,
//     pub outer_match: Vec<Vec<bool>>,
//     pub outer_order_idx: Vec<chunk::RowPtr>,
//     pub inner_result: chunk::Chunk,
//     pub inner_iter: chunk::Iterator,
//     pub same_key_inner_rows: Vec<chunk::Row>,
//     pub same_key_iter: chunk::Iterator,
//     pub done_err: Option<Error>,
//     pub results: Channel<IndexMergeJoinResult>,
//     pub mem_tracker: memory::Tracker,
// }
//
// OuterMergeWorker 对应 Go outerMergeWorker：只负责读外表 batch，不做 lookup key 构造。
// pub struct OuterMergeWorker {
//     pub outer_merge_ctx: OuterMergeCtx,
//     pub lookup: *mut IndexLookUpMergeJoin,
//     pub ctx: sessionctx::Context,
//     pub executor: exec::Executor,
//     pub max_batch_size: i32,
//     pub batch_size: i32,
//     pub next_col_compare_filters: Option<physicalop::ColWithCmpFuncManager>,
//     pub result_ch: Sender<LookUpMergeJoinTask>,
//     pub inner_ch: Sender<LookUpMergeJoinTask>,
//     pub parent_mem_tracker: memory::Tracker,
// }
//
// InnerMergeWorker 对应 Go innerMergeWorker：排序外表、构造 lookup key、读取内表并执行 merge join。
// pub struct InnerMergeWorker {
//     pub inner_merge_ctx: InnerMergeCtx,
//     pub task_ch: Receiver<LookUpMergeJoinTask>,
//     pub join_chk_resource_ch: Channel<chunk::Chunk>,
//     pub outer_merge_ctx: OuterMergeCtx,
//     pub ctx: sessionctx::Context,
//     pub inner_exec: Option<exec::Executor>,
//     pub joiner: Joiner,
//     pub ret_field_types: Vec<types::FieldType>,
//     pub max_chunk_size: i32,
//     pub index_ranges: Vec<ranger::Range>,
//     pub next_col_compare_filters: Option<physicalop::ColWithCmpFuncManager>,
//     pub key_off2_idx_off: Vec<i32>,
// }
//
// IndexMergeJoinResult 对应 Go 结果包装，src 用于把消费后的 chunk 还回资源池。
// pub struct IndexMergeJoinResult {
//     pub chk: chunk::Chunk,
//     pub src: Sender<chunk::Chunk>,
// }
//
// impl IndexLookUpMergeJoin {
// Open 对应 Go Executor.Open：打开外表 child 并挂接内存 tracker。
//     pub fn open(&mut self, ctx: context::Context) -> Result<(), Error> {
//         exec::open(ctx, self.children(0))?;
//         self.mem_tracker = Some(memory::NewTracker(self.id(), -1));
//         self.mem_tracker.as_mut().unwrap().AttachTo(self.ctx().GetSessionVars().StmtCtx.MemTracker);
//         Ok(())
//     }
//
// startWorkers 对应 Go：创建 result 通道、每个 worker 的结果 chunk 资源池，并启动 outer/inner workers。
//     pub fn start_workers(&mut self, ctx: context::Context) {
//         let concurrency = self.ctx().GetSessionVars().IndexLookupJoinConcurrency();
//         if self.runtime_stats().is_some() {
//             let mut runtime_stats = execdetails::RuntimeStatsWithConcurrencyInfo::default();
//             runtime_stats.SetConcurrencyInfo(execdetails::NewConcurrencyInfo("Concurrency", concurrency));
//             self.ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.id(), runtime_stats);
//         }
//         let result_ch = make_channel(concurrency);
//         self.result_ch = Some(result_ch.receiver());
//         self.join_chk_resource_ch = Vec::with_capacity(concurrency as usize);
//         for _ in 0..concurrency {
//             let ch = make_channel(numResChkHold);
//             for _ in 0..numResChkHold {
//                 ch.send(self.NewChunk());
//             }
//             self.join_chk_resource_ch.push(ch);
//         }
//         let (worker_ctx, cancel_func) = context::with_cancel(ctx);
//         self.cancel_func = Some(cancel_func.clone());
//         let inner_ch = make_channel(concurrency);
//         self.worker_wg.Add(1);
//         go(self.new_outer_worker(result_ch.sender(), inner_ch.sender()).run(worker_ctx.clone(), &self.worker_wg, cancel_func.clone()));
//         self.worker_wg.Add(concurrency);
//         for i in 0..concurrency {
//             go(self.new_inner_merge_worker(inner_ch.receiver(), i).run(worker_ctx.clone(), &self.worker_wg, cancel_func.clone()));
//         }
//     }
//
// newOuterWorker 对应 Go 构造函数，failpoint 会把 batchSize 改为 1 以复现历史问题。
//     pub fn new_outer_worker(&self, result_ch: Sender<LookUpMergeJoinTask>, inner_ch: Sender<LookUpMergeJoinTask>) -> OuterMergeWorker {
//         let mut omw = OuterMergeWorker {
//             outer_merge_ctx: self.outer_merge_ctx.clone(),
//             ctx: self.ctx(),
//             lookup: self as *const _ as *mut _,
//             executor: self.children(0),
//             result_ch,
//             inner_ch,
//             batch_size: 32,
//             max_batch_size: self.ctx().GetSessionVars().IndexJoinBatchSize,
//             parent_mem_tracker: self.mem_tracker.clone().unwrap(),
//             next_col_compare_filters: self.last_col_helper.clone(),
//         };
//         failpoint::inject("testIssue18068", || omw.batch_size = 1);
//         omw
//     }
//
// newInnerMergeWorker 对应 Go：复制 IndexRanges 和 last-col helper，避免多个 inner worker 并发共享可变数据。
//     pub fn new_inner_merge_worker(&self, task_ch: Receiver<LookUpMergeJoinTask>, work_id: i32) -> InnerMergeWorker {
//         let copied_ranges = self.index_ranges.Range().iter().map(|r| r.Clone()).collect();
//         let mut imw = InnerMergeWorker {
//             inner_merge_ctx: self.inner_merge_ctx.clone(),
//             outer_merge_ctx: self.outer_merge_ctx.clone(),
//             task_ch,
//             ctx: self.ctx(),
//             index_ranges: copied_ranges,
//             key_off2_idx_off: self.key_off2_idx_off.clone(),
//             joiner: self.joiners[work_id as usize].clone(),
//             join_chk_resource_ch: self.join_chk_resource_ch[work_id as usize].clone(),
//             ret_field_types: self.RetFieldTypes(),
//             max_chunk_size: self.MaxChunkSize(),
//             inner_exec: None,
//             next_col_compare_filters: None,
//         };
//         if let Some(helper) = &self.last_col_helper {
//             let mut next_cwf = helper.clone();
//             next_cwf.TmpConstant = helper.TmpConstant.iter().map(|_| expression::Constant { RetType: next_cwf.TargetCol.RetType.clone() }).collect();
//             imw.next_col_compare_filters = Some(next_cwf);
//         }
//         imw
//     }
//
// Next 对应 Go Executor.Next：从当前 task.results 取结果 chunk；task 结束后加载下一个完成 task。
//     pub fn next(&mut self, ctx: context::Context, req: &mut chunk::Chunk) -> Result<(), Error> {
//         if !self.prepared {
//             self.start_workers(ctx.clone());
//             self.prepared = true;
//         }
//         if self.is_outer_join {
//             self.required_rows.Store(req.RequiredRows() as i64);
//         }
//         req.Reset();
//         if self.task.is_none() {
//             self.load_finished_task(ctx.clone());
//         }
//         while let Some(task) = &mut self.task {
//             select! {
//                 result = task.results.recv() => {
//                     if let Some(result) = result {
//                         req.SwapColumns(result.chk.clone());
//                         result.src.send(result.chk);
//                         return Ok(());
//                     }
//                     if let Some(err) = task.done_err.take() {
//                         return Err(err);
//                     }
//                     self.load_finished_task(ctx.clone());
//                 }
//                 _ = ctx.Done() => return Err(ctx.Err()),
//             }
//         }
//         Ok(())
//     }
//
// loadFinishedTask 对应 Go：只负责从 resultCh 取下一个 task，context 取消则置空。
//     pub fn load_finished_task(&mut self, ctx: context::Context) {
//         self.task = select_recv_or_cancel(self.result_ch.as_ref().unwrap(), ctx).unwrap_or(None);
//     }
//
// Close 对应 Go：取消 worker、清空 resultCh、释放结果 chunk 资源池并等待 WaitGroup。
//     pub fn close(&mut self) -> Result<(), Error> {
//         if self.runtime_stats().is_some() {
//             self.ctx().GetSessionVars().StmtCtx.RuntimeStatsColl.RegisterStats(self.id(), self.runtime_stats().unwrap());
//         }
//         if let Some(cancel) = self.cancel_func.take() {
//             cancel();
//         }
//         if let Some(ch) = self.result_ch.take() {
//             channel::Clear(ch);
//         }
//         self.join_chk_resource_ch.clear();
//         self.worker_wg.Wait();
//         self.mem_tracker = None;
//         self.prepared = false;
//         self.base_executor.Close()
//     }
// }
//
// impl OuterMergeWorker {
// run 对应 Go outerMergeWorker.run：panic 转换成已关闭 results 的 task，并取消所有 worker。
//     pub fn run(&mut self, ctx: context::Context, wg: &sync::WaitGroup, cancel_func: context::CancelFunc) {
//         let _region = trace::StartRegion(ctx.clone(), "IndexLookupMergeJoinOuterWorker");
//         defer(|| {
//             close(self.result_ch.clone());
//             close(self.inner_ch.clone());
//             wg.Done();
//         });
//         loop {
//             let task = match self.build_task(ctx.clone()) {
//                 Ok(Some(task)) => task,
//                 Ok(None) => return,
//                 Err((mut task, err)) => {
//                     task.done_err = Some(err);
//                     close(task.results.clone());
//                     self.push_to_chan(ctx.clone(), task, self.result_ch.clone());
//                     return;
//                 }
//             };
//             failpoint::inject("mockIndexMergeJoinOOMPanic", || {});
//             if self.push_to_chan(ctx.clone(), task.clone(), self.inner_ch.clone()) {
//                 return;
//             }
//             if self.push_to_chan(ctx.clone(), task, self.result_ch.clone()) {
//                 return;
//             }
//         }
//         cancel_func();
//     }
//
// pushToChan 对应 Go select 发送，context done 时返回 finished=true。
//     pub fn push_to_chan(&self, ctx: context::Context, task: LookUpMergeJoinTask, dst: Sender<LookUpMergeJoinTask>) -> bool {
//         select! {
//             _ = ctx.Done() => true,
//             _ = dst.send(task) => false,
//         }
//     }
//
// buildTask 对应 Go：读取外表 rows 到 chunk.List，并按 outer join requiredRows 调整批量大小。
//     pub fn build_task(&mut self, ctx: context::Context) -> Result<Option<LookUpMergeJoinTask>, (LookUpMergeJoinTask, Error)> {
//         let mut task = LookUpMergeJoinTask::new(make_channel(numResChkHold), chunk::NewList(self.outer_merge_ctx.row_types.clone(), self.executor.InitCap(), self.executor.MaxChunkSize()));
//         task.mem_tracker = memory::NewTracker(memory::LabelForSimpleTask, -1);
//         task.mem_tracker.AttachTo(self.parent_mem_tracker.clone());
//         self.increase_batch_size();
//         let mut required_rows = self.batch_size;
//         if unsafe { (*self.lookup).is_outer_join } {
//             required_rows = unsafe { (*self.lookup).required_rows.Load() as i32 };
//         }
//         if required_rows <= 0 || required_rows > self.max_batch_size {
//             required_rows = self.max_batch_size;
//         }
//         while required_rows > 0 {
//             let exec_chk = exec::TryNewCacheChunk(self.executor.clone());
//             exec::Next(ctx.clone(), self.executor.clone(), exec_chk.clone()).map_err(|e| (task.clone(), e))?;
//             if exec_chk.NumRows() == 0 {
//                 break;
//             }
//             task.outer_result.Add(exec_chk.clone());
//             required_rows -= exec_chk.NumRows();
//             task.mem_tracker.Consume(exec_chk.MemoryUsage());
//         }
//         if task.outer_result.Len() == 0 {
//             return Ok(None);
//         }
//         Ok(Some(task))
//     }
//
// increaseBatchSize 对应 Go 指数增长，上限为 session IndexJoinBatchSize。
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
// impl InnerMergeWorker {
// run 对应 Go innerMergeWorker.run：每个 task 处理完都会写 doneErr 并关闭 results。
//     pub fn run(&mut self, ctx: context::Context, wg: &sync::WaitGroup, cancel_func: context::CancelFunc) {
//         let _region = trace::StartRegion(ctx.clone(), "IndexLookupMergeJoinInnerWorker");
//         defer(|| wg.Done());
//         loop {
//             let Some(mut task) = select_recv_or_cancel(&self.task_ch, ctx.clone()).unwrap_or(None) else {
//                 return;
//             };
//             task.done_err = self.handle_task(ctx.clone(), &mut task).err();
//             close(task.results.clone());
//         }
//         cancel_func();
//     }
//
// handleTask 对应 Go：外表过滤/排序、构造去重 lookup key、构建 innerExec、取首个 inner chunk 并 merge join。
//     pub fn handle_task(&mut self, ctx: context::Context, task: &mut LookUpMergeJoinTask) -> Result<(), Error> {
//         let num_outer_chks = task.outer_result.NumChunks();
//         if !self.outer_merge_ctx.filter.is_empty() {
//             task.outer_match = vectorized_filter_merge_outer_rows(self, task, num_outer_chks)?;
//         }
//         task.mem_tracker.Consume(task.outer_match.capacity() as i64);
//         task.outer_order_idx = Vec::with_capacity(task.outer_result.Len());
//         for i in 0..num_outer_chks {
//             for j in 0..task.outer_result.GetChunk(i).NumRows() {
//                 task.outer_order_idx.push(chunk::RowPtr { ChkIdx: i as u32, RowIdx: j as u32 });
//             }
//         }
//         task.mem_tracker.Consume(task.outer_order_idx.capacity() as i64);
//         if self.outer_merge_ctx.need_outer_sort {
//             self.sort_outer_order_idx(task);
//         }
//         let mut d_lookup_keys = self.construct_datum_lookup_keys(task)?;
//         d_lookup_keys = self.dedup_datum_lookup_keys(d_lookup_keys);
//         if self.inner_merge_ctx.desc {
//             d_lookup_keys.reverse();
//         }
//         self.inner_exec = Some(self.inner_merge_ctx.reader_builder.build_executor_for_index_join(ctx.clone(), d_lookup_keys, self.index_ranges.clone(), self.key_off2_idx_off.clone(), self.next_col_compare_filters.clone(), false, None, None)?);
//         defer(|| terror::Log(exec::Close(self.inner_exec.take().unwrap())));
//         self.fetch_next_inner_result(ctx.clone(), task)?;
//         self.do_merge_join(ctx, task)
//     }
//
// sortOuterOrderIdx 对应 Go NeedOuterSort 分支：按 join key 和 last-col helper 排外表 row pointer。
//     pub fn sort_outer_order_idx(&self, task: &mut LookUpMergeJoinTask) {
//         let expr_ctx = self.ctx.GetExprCtx();
//         task.outer_order_idx.sort_by(|idx_i, idx_j| {
//             let row_i = task.outer_result.GetRow(*idx_i);
//             let row_j = task.outer_result.GetRow(*idx_j);
//             let mut c = 0_i64;
//             for key_off in &self.inner_merge_ctx.key_off2_key_off_order_by_idx {
//                 let join_key = self.outer_merge_ctx.join_keys[*key_off as usize].clone();
//                 let (cmp, _, err) = self.outer_merge_ctx.compare_funcs[*key_off as usize](expr_ctx.GetEvalCtx(), join_key.clone(), join_key, row_i, row_j);
//                 terror::Log(err);
//                 c = cmp;
//                 if c != 0 {
//                     break;
//                 }
//             }
//             if c == 0 {
//                 if let Some(helper) = &self.next_col_compare_filters {
//                     c = helper.CompareRow(row_i, row_j) as i64;
//                 }
//             }
//             if self.inner_merge_ctx.desc { -c as i32 } else { c as i32 }
//         });
//     }
//
// fetchNewChunkWhenFull 对应 Go：结果 chunk 满时送给主线程，再从资源池取新 chunk。
//     pub fn fetch_new_chunk_when_full(&self, ctx: context::Context, task: &mut LookUpMergeJoinTask, chk: &mut chunk::Chunk) -> bool {
//         if !chk.IsFull() {
//             return true;
//         }
//         if select_send_or_cancel(task.results.clone(), IndexMergeJoinResult { chk: chk.clone(), src: self.join_chk_resource_ch.sender() }, ctx.clone()).is_err() {
//             return false;
//         }
//         let Some(new_chk) = select_recv_or_cancel(&self.join_chk_resource_ch, ctx).unwrap_or(None) else {
//             return false;
//         };
//         *chk = new_chk;
//         chk.Reset();
//         true
//     }
//
// doMergeJoin 对应 Go 主循环：按外表顺序推进 same-key 内表行，匹配失败时调用 OnMissMatch。
//     pub fn do_merge_join(&mut self, ctx: context::Context, task: &mut LookUpMergeJoinTask) -> Result<(), Error> {
//         let mut chk = select_recv_or_cancel(&self.join_chk_resource_ch, ctx.clone())?.unwrap();
//         defer(|| recycle_or_send_result(&self.join_chk_resource_ch, &task.results, chk.clone(), ctx.clone()));
//         let init_cmp_result = if self.inner_merge_ctx.desc { -1 } else { 1 };
//         let mut none_inner_rows_remain = task.inner_result.NumRows() == 0;
//         for outer_idx in task.outer_order_idx.clone() {
//             let outer_row = task.outer_result.GetRow(outer_idx);
//             let (mut has_match, mut has_null, mut cmp_result) = (false, false, init_cmp_result);
//             if !task.outer_match.is_empty() && !task.outer_match[outer_idx.ChkIdx as usize][outer_idx.RowIdx as usize] {
//                 self.on_merge_mismatch(task, outer_row, has_match, has_null, &mut chk, ctx.clone());
//                 continue;
//             }
//             if none_inner_rows_remain && task.same_key_inner_rows.is_empty() {
//                 self.on_merge_mismatch(task, outer_row, has_match, has_null, &mut chk, ctx.clone());
//                 continue;
//             }
//             if !task.same_key_inner_rows.is_empty() {
//                 cmp_result = self.compare(outer_row, task.same_key_iter.Begin())?;
//             }
//             if (cmp_result > 0 && !self.inner_merge_ctx.desc) || (cmp_result < 0 && self.inner_merge_ctx.desc) {
//                 if none_inner_rows_remain {
//                     task.same_key_inner_rows.clear();
//                     self.on_merge_mismatch(task, outer_row, has_match, has_null, &mut chk, ctx.clone());
//                     continue;
//                 }
//                 none_inner_rows_remain = self.fetch_inner_rows_with_same_key(ctx.clone(), task, outer_row)?;
//             }
//             while task.same_key_iter.Current() != task.same_key_iter.End() {
//                 let (matched, is_null) = self.joiner.TryToMatchInners(outer_row, &mut task.same_key_iter, &mut chk)?;
//                 has_match = has_match || matched;
//                 has_null = has_null || is_null;
//                 if !self.fetch_new_chunk_when_full(ctx.clone(), task, &mut chk) {
//                     return Ok(());
//                 }
//             }
//             self.on_merge_mismatch(task, outer_row, has_match, has_null, &mut chk, ctx.clone());
//         }
//         Ok(())
//     }
//
// on_merge_mismatch 保留 Go 标签 missMatch 的语义，避免 Rust 里使用 goto。
//     fn on_merge_mismatch(&self, task: &mut LookUpMergeJoinTask, outer_row: chunk::Row, has_match: bool, has_null: bool, chk: &mut chunk::Chunk, ctx: context::Context) {
//         if !has_match {
//             self.joiner.OnMissMatch(has_null, outer_row, chk);
//             self.fetch_new_chunk_when_full(ctx, task, chk);
//         }
//     }
//
// fetchInnerRowsWithSameKey 对应 Go：收集与当前 outer key 相同的 inner rows，必要时继续拉 inner chunk。
//     pub fn fetch_inner_rows_with_same_key(&mut self, ctx: context::Context, task: &mut LookUpMergeJoinTask, key: chunk::Row) -> Result<bool, Error> {
//         task.same_key_inner_rows.clear();
//         let mut cur_row = task.inner_iter.Current();
//         loop {
//             let cmp_res = self.compare(key, cur_row)?;
//             if !((cmp_res >= 0 && !self.inner_merge_ctx.desc) || (cmp_res <= 0 && self.inner_merge_ctx.desc)) {
//                 break;
//             }
//             if cmp_res == 0 {
//                 task.same_key_inner_rows.push(cur_row);
//             }
//             cur_row = task.inner_iter.Next();
//             if cur_row == task.inner_iter.End() {
//                 cur_row = self.fetch_next_inner_result(ctx.clone(), task)?;
//                 if task.inner_result.NumRows() == 0 {
//                     break;
//                 }
//             }
//         }
//         task.same_key_iter = chunk::NewIterator4Slice(task.same_key_inner_rows.clone());
//         task.same_key_iter.Begin();
//         Ok(task.inner_result.NumRows() == 0)
//     }
//
// compare 对应 Go：按 order-by key 顺序比较外表 row 与内表 row。
//     pub fn compare(&self, outer_row: chunk::Row, inner_row: chunk::Row) -> Result<i32, Error> {
//         let expr_ctx = self.ctx.GetExprCtx();
//         for key_off in &self.inner_merge_ctx.key_off2_key_off_order_by_idx {
//             let (cmp, _, err) = self.inner_merge_ctx.compare_funcs[*key_off as usize](expr_ctx.GetEvalCtx(), self.outer_merge_ctx.join_keys[*key_off as usize].clone(), self.inner_merge_ctx.join_keys[*key_off as usize].clone(), outer_row, inner_row);
//             if err.is_some() || cmp != 0 {
//                 return err.map_or(Ok(cmp as i32), Err);
//             }
//         }
//         Ok(0)
//     }
//
// constructDatumLookupKeys 对应 Go：按 outerOrderIdx 顺序构造 lookup 内容，nil key 表示该行无需 lookup。
//     pub fn construct_datum_lookup_keys(&self, task: &LookUpMergeJoinTask) -> Result<Vec<IndexJoinLookUpContent>, Error> {
//         let mut d_lookup_keys = Vec::with_capacity(task.outer_order_idx.len());
//         for idx in &task.outer_order_idx {
//             if let Some(key) = self.construct_datum_lookup_key(task, *idx)? {
//                 d_lookup_keys.push(key);
//             }
//         }
//         Ok(d_lookup_keys)
//     }
//
// constructDatumLookupKey 对应 Go：NULL、类型转换溢出、转换后不相等都会使该 outer row 跳过 lookup。
//     pub fn construct_datum_lookup_key(&self, task: &LookUpMergeJoinTask, idx: chunk::RowPtr) -> Result<Option<IndexJoinLookUpContent>, Error> {
//         if !task.outer_match.is_empty() && !task.outer_match[idx.ChkIdx as usize][idx.RowIdx as usize] {
//             return Ok(None);
//         }
//         let outer_row = task.outer_result.GetRow(idx);
//         let sc = self.ctx.GetSessionVars().StmtCtx;
//         let mut d_lookup_key = Vec::with_capacity(self.inner_merge_ctx.key_cols.len());
//         for (i, key_col) in self.outer_merge_ctx.key_cols.iter().enumerate() {
//             let outer_value = outer_row.GetDatum(*key_col, self.outer_merge_ctx.row_types[*key_col as usize].clone());
//             if outer_value.IsNull() {
// IndexNestedLoopJoin 的 on 条件保证等值语义，outer NULL 永远不需要 lookup。
//                 return Ok(None);
//             }
//             let inner_col_type = self.inner_merge_ctx.row_types[self.inner_merge_ctx.key_cols[i] as usize].clone();
//             let inner_value = match outer_value.ConvertTo(sc.TypeCtx(), inner_col_type.clone()) {
//                 Ok(v) => v,
//                 Err(err) if terror::ErrorEqual(err, types::ErrOverflow) || terror::ErrorEqual(err, types::ErrWarnDataOutOfRange) => return Ok(None),
//                 Err(err) if terror::ErrorEqual(err, types::ErrTruncated) && (inner_col_type.GetType() == mysql::TypeSet || inner_col_type.GetType() == mysql::TypeEnum) => return Ok(None),
//                 Err(err) => return Err(err),
//             };
//             let cmp = outer_value.Compare(sc.TypeCtx(), inner_value.clone(), self.inner_merge_ctx.key_collators[i].clone())?;
//             if cmp != 0 {
//                 return Ok(None);
//             }
//             d_lookup_key.push(inner_value);
//         }
//         Ok(Some(IndexJoinLookUpContent { keys: d_lookup_key, row: task.outer_result.GetRow(idx), key_cols: Vec::new(), key_col_ids: self.inner_merge_ctx.key_col_ids.clone() }))
//     }
//
// dedupDatumLookUpKeys 对应 Go：外表已经有序，直接顺序去重并考虑 last-col helper。
//     pub fn dedup_datum_lookup_keys(&self, lookup_contents: Vec<IndexJoinLookUpContent>) -> Vec<IndexJoinLookUpContent> {
//         if lookup_contents.len() < 2 {
//             return lookup_contents;
//         }
//         let sc = self.ctx.GetSessionVars().StmtCtx;
//         dedup_lookup_contents(sc, lookup_contents, &self.inner_merge_ctx.key_collators, self.next_col_compare_filters.as_ref())
//     }
//
// fetchNextInnerResult 对应 Go：从 innerExec 拉取一个 chunk 并初始化 iterator。
//     pub fn fetch_next_inner_result(&mut self, ctx: context::Context, task: &mut LookUpMergeJoinTask) -> Result<chunk::Row, Error> {
//         task.inner_result = self.inner_exec.as_ref().unwrap().NewChunkWithCapacity(self.inner_exec.as_ref().unwrap().RetFieldTypes(), self.inner_exec.as_ref().unwrap().InitCap(), self.inner_exec.as_ref().unwrap().MaxChunkSize());
//         exec::Next(ctx, self.inner_exec.as_ref().unwrap(), task.inner_result.clone())?;
//         task.inner_iter = chunk::NewIterator4Chunk(task.inner_result.clone());
//         Ok(task.inner_iter.Begin())
//     }
// }
// */
use crate::index_lookup_join::{
    IndexJoinExecutorBuilder, IndexJoinLookupContent, compare_row, extract_key,
};
use crate::joiner::{Joiner, NaajType, Row};
use crate::row_table_builder::Value;

/// 一批外表行的 merge-join 任务：含 lookup 内容、内表行与输出。
#[derive(Clone, Debug, Default)]
pub struct LookUpMergeJoinTask {
    /// 本批外表行（按 join key 有序）。
    pub outer_rows: Vec<Row>,
    /// 各外表行是否参与 join（保留 Go outerMatch）。
    pub outer_match: Vec<bool>,
    /// 去重后的外表 lookup key 列表。
    pub lookup_contents: Vec<IndexJoinLookupContent>,
    /// 按 key 有序的内表行。
    pub inner_rows: Vec<Row>,
    /// 本任务产出的连接结果。
    pub output: Vec<Row>,
    /// inner worker 是否已完成。
    pub done: bool,
}
/// `next` 返回的结果块：行集与可选错误。
#[derive(Clone, Debug, Default)]
pub struct IndexMergeJoinResult {
    /// 结果行。
    pub rows: Vec<Row>,
    /// 执行错误（若有）。
    pub error: Option<String>,
}

/// 外表 merge worker：可选排序后按相同 key 组切批构造任务。
pub struct OuterMergeWorker {
    /// 外表行（可能已按 key 排序）。
    rows: Vec<Row>,
    /// 外表 join key 列。
    key_columns: Vec<usize>,
    /// 读取游标。
    cursor: usize,
    /// 当前目标批大小。
    batch_size: usize,
    /// 初始批大小，重开执行器时恢复。
    initial_batch_size: usize,
    /// 批大小上限。
    max_batch_size: usize,
}
impl OuterMergeWorker {
    /// 默认需要外表排序的构造入口。
    pub fn new(
        rows: Vec<Row>,
        key_columns: Vec<usize>,
        batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Self::new_with_outer_sort(rows, key_columns, batch_size, max_batch_size, true)
    }

    /// 可选先按 join key 排序外表，再构造 worker。
    pub fn new_with_outer_sort(
        mut rows: Vec<Row>,
        key_columns: Vec<usize>,
        batch_size: usize,
        max_batch_size: usize,
        need_outer_sort: bool,
    ) -> Result<Self, String> {
        if batch_size == 0 || max_batch_size == 0 {
            return Err("merge lookup batch size must be positive".into());
        }
        if need_outer_sort {
            rows.sort_by(|left, right| {
                compare_keys(left, right, &key_columns, &key_columns)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        Ok(Self {
            rows,
            key_columns,
            cursor: 0,
            batch_size,
            initial_batch_size: batch_size,
            max_batch_size,
        })
    }
    /// 切下一批外表行；若批尾与下一批 key 相同则扩展，避免拆散同 key 组。
    pub fn build_task(&mut self) -> Option<LookUpMergeJoinTask> {
        if self.cursor >= self.rows.len() {
            return None;
        }
        let mut end = (self.cursor + self.batch_size).min(self.rows.len());
        // 同 key 组必须完整落入同一批，否则 merge join 会漏匹配。
        while end < self.rows.len()
            && compare_keys(
                &self.rows[end - 1],
                &self.rows[end],
                &self.key_columns,
                &self.key_columns,
            )
            .is_ok_and(|order| order.is_eq())
        {
            end += 1;
        }
        let rows = self.rows[self.cursor..end].to_vec();
        self.cursor = end;
        self.batch_size = (self.batch_size * 2).min(self.max_batch_size);
        Some(LookUpMergeJoinTask {
            outer_match: vec![true; rows.len()],
            outer_rows: rows,
            ..LookUpMergeJoinTask::default()
        })
    }

    /// 复位外表读取状态，供执行器 `Close` 后再次 `Open`。
    fn reset(&mut self) {
        self.cursor = 0;
        self.batch_size = self.initial_batch_size;
    }
}

/// 内表 merge worker：lookup 取回内表后按 key 做双指针归并连接。
pub struct InnerMergeWorker<'a> {
    /// 按 lookup key 拉取内表的 builder。
    builder: &'a dyn IndexJoinExecutorBuilder,
    /// 外表 join key 列。
    outer_key_columns: &'a [usize],
    /// 内表 join key 列。
    inner_key_columns: &'a [usize],
    /// 连接器。
    joiner: &'a Joiner,
}
impl InnerMergeWorker<'_> {
    /// 从外表行构造非 NULL lookup key 并去重。
    pub fn construct_lookup_keys(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        for row in &task.outer_rows {
            let keys = extract_key(row, self.outer_key_columns)?;
            if keys.iter().any(|value| matches!(value, Value::Null)) {
                continue;
            }
            task.lookup_contents.push(IndexJoinLookupContent {
                keys,
                row: row.clone(),
                key_columns: self.inner_key_columns.to_vec(),
                key_column_ids: Vec::new(),
            });
        }
        task.lookup_contents
            .dedup_by(|left, right| left.keys == right.keys);
        Ok(())
    }
    /// 拉取内表、按 key 排序后执行 merge join。
    pub fn handle_task(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        self.construct_lookup_keys(task)?;
        task.inner_rows = self.builder.build(&task.lookup_contents)?;
        // 内表必须有序，merge join 才能用双指针推进。
        task.inner_rows.sort_by(|left, right| {
            compare_keys(left, right, self.inner_key_columns, self.inner_key_columns)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.do_merge_join(task)?;
        task.done = true;
        Ok(())
    }
    /// 双指针归并：相等则扩展同 key 组并 Joiner 匹配，外表更小则 miss，内表更小则跳过。
    fn do_merge_join(&self, task: &mut LookUpMergeJoinTask) -> Result<(), String> {
        let mut outer = 0;
        let mut inner = 0;
        while outer < task.outer_rows.len() {
            if inner >= task.inner_rows.len() {
                self.joiner
                    .on_miss_match(false, &task.outer_rows[outer], &mut task.output);
                outer += 1;
                continue;
            }
            match compare_keys(
                &task.outer_rows[outer],
                &task.inner_rows[inner],
                self.outer_key_columns,
                self.inner_key_columns,
            )? {
                std::cmp::Ordering::Less => {
                    self.joiner
                        .on_miss_match(false, &task.outer_rows[outer], &mut task.output);
                    outer += 1;
                }
                std::cmp::Ordering::Greater => inner += 1,
                std::cmp::Ordering::Equal => {
                    let outer_start = outer;
                    let inner_start = inner;
                    // 扩展外表同 key 区间。
                    while outer < task.outer_rows.len()
                        && compare_keys(
                            &task.outer_rows[outer_start],
                            &task.outer_rows[outer],
                            self.outer_key_columns,
                            self.outer_key_columns,
                        )?
                        .is_eq()
                    {
                        outer += 1;
                    }
                    // 扩展内表同 key 区间。
                    while inner < task.inner_rows.len()
                        && compare_keys(
                            &task.inner_rows[inner_start],
                            &task.inner_rows[inner],
                            self.inner_key_columns,
                            self.inner_key_columns,
                        )?
                        .is_eq()
                    {
                        inner += 1;
                    }
                    for outer_row in &task.outer_rows[outer_start..outer] {
                        let result = self.joiner.try_to_match_inners(
                            outer_row,
                            &task.inner_rows[inner_start..inner],
                            &mut task.output,
                            NaajType::Unknown,
                        )?;
                        if !result.matched {
                            self.joiner
                                .on_miss_match(result.has_null, outer_row, &mut task.output);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Index LookUp Merge Join 执行器：外表有序分批 lookup，内表归并连接。
pub struct IndexLookUpMergeJoin {
    /// 外表分批与可选排序 worker。
    outer_worker: OuterMergeWorker,
    /// 内表索引 lookup builder。
    builder: Box<dyn IndexJoinExecutorBuilder>,
    /// 外表 join key 列。
    outer_key_columns: Vec<usize>,
    /// 内表 join key 列。
    inner_key_columns: Vec<usize>,
    /// 连接器。
    joiner: Joiner,
    /// 已产出结果缓冲。
    output: Vec<Row>,
    /// 结果读取游标。
    cursor: usize,
    /// 是否已 open。
    opened: bool,
}
impl IndexLookUpMergeJoin {
    /// 构造执行器（默认对外表按 key 排序）。
    pub fn new(
        outer_rows: Vec<Row>,
        outer_key_columns: Vec<usize>,
        inner_key_columns: Vec<usize>,
        builder: Box<dyn IndexJoinExecutorBuilder>,
        joiner: Joiner,
        batch_size: usize,
        max_batch_size: usize,
    ) -> Result<Self, String> {
        Self::new_with_outer_sort(
            outer_rows,
            outer_key_columns,
            inner_key_columns,
            builder,
            joiner,
            batch_size,
            max_batch_size,
            true,
        )
    }

    /// 可选跳过外表排序（调用方保证已有序）的构造入口。
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_outer_sort(
        outer_rows: Vec<Row>,
        outer_key_columns: Vec<usize>,
        inner_key_columns: Vec<usize>,
        builder: Box<dyn IndexJoinExecutorBuilder>,
        joiner: Joiner,
        batch_size: usize,
        max_batch_size: usize,
        need_outer_sort: bool,
    ) -> Result<Self, String> {
        if outer_key_columns.len() != inner_key_columns.len() {
            return Err("index merge join key count mismatch".into());
        }
        Ok(Self {
            outer_worker: OuterMergeWorker::new_with_outer_sort(
                outer_rows,
                outer_key_columns.clone(),
                batch_size,
                max_batch_size,
                need_outer_sort,
            )?,
            builder,
            outer_key_columns,
            inner_key_columns,
            joiner,
            output: Vec::new(),
            cursor: 0,
            opened: false,
        })
    }
    /// 打开执行器并清空输出缓冲。
    pub fn open(&mut self) -> Result<(), String> {
        self.output.clear();
        self.cursor = 0;
        self.outer_worker.reset();
        self.opened = true;
        Ok(())
    }
    /// 消费全部外表任务，将各任务 merge 结果追加到 `output`。
    fn execute(&mut self) -> Result<(), String> {
        while let Some(mut task) = self.outer_worker.build_task() {
            InnerMergeWorker {
                builder: self.builder.as_ref(),
                outer_key_columns: &self.outer_key_columns,
                inner_key_columns: &self.inner_key_columns,
                joiner: &self.joiner,
            }
            .handle_task(&mut task)?;
            self.output.extend(task.output);
        }
        Ok(())
    }
    /// 惰性执行并按 `required_rows` 切片返回结果。
    pub fn next(&mut self, required_rows: usize) -> Result<IndexMergeJoinResult, String> {
        if !self.opened {
            self.open()?;
        }
        if self.output.is_empty() && self.cursor == 0 {
            self.execute()?;
        }
        if self.cursor >= self.output.len() || required_rows == 0 {
            return Ok(IndexMergeJoinResult::default());
        }
        let end = (self.cursor + required_rows).min(self.output.len());
        let rows = self.output[self.cursor..end].to_vec();
        self.cursor = end;
        Ok(IndexMergeJoinResult { rows, error: None })
    }
    /// 关闭执行器。
    pub fn close(&mut self) {
        self.output.clear();
        self.cursor = 0;
        self.outer_worker.reset();
        self.opened = false;
    }
}

/// 比较外表行与内表行在各自 key 列上的大小关系。
pub fn compare_keys(
    outer: &Row,
    inner: &Row,
    outer_columns: &[usize],
    inner_columns: &[usize],
) -> Result<std::cmp::Ordering, String> {
    if outer_columns.len() != inner_columns.len() {
        return Err("merge key count mismatch".into());
    }
    Ok(compare_row(
        &extract_key(outer, outer_columns)?,
        &extract_key(inner, inner_columns)?,
    ))
}
