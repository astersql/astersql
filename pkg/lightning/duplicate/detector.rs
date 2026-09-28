// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 重复键检测器：外排键后并行扫描相邻相同用户键。
//
// Lightning 物理导入写入前/后可用本模块发现唯一索引或主键冲突。
// 流程：`KeyAdder` 写入编码键 → `ExternalSorter` 排序 → 多 `Worker` 分段扫描，
// 通过 `Handler` 回调报告每个重复组。

use std::error::Error as StdError;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::lightning::log::filter::{Field, Level};
use crate::lightning::log::log::Logger;
use crate::util::extsort::external_sorter::{Error, ExternalSorter, Writer};

use super::internal::{
    InternalKey, compare_internal_key, decode_internal_key, encode_internal_key,
};
use super::worker::{Task, TaskQueue, Worker};

/// 重复检测器：持有外排器与日志，负责发起排序与并行扫描。
pub struct Detector {
    /// 外部排序器，存放并排序全部内部键。
    sorter: Arc<dyn ExternalSorter>,
    /// 组件日志。
    logger: Logger,
}

/// 创建检测器，并在日志上附加 `component=duplicate.Detector` 字段。
pub fn new_detector(sorter: Arc<dyn ExternalSorter>, logger: Logger) -> Detector {
    Detector {
        sorter,
        logger: logger.With([Field::string("component", "duplicate.Detector")]),
    }
}

impl Detector {
    /// 打开外排 Writer，返回用于增量写入内部键的 `KeyAdder`。
    pub fn key_adder(&self, ctx: &CancellationToken) -> Result<KeyAdder, Error> {
        let writer = self.sorter.new_writer(ctx)?;
        Ok(KeyAdder {
            writer,
            key_buf: Vec::new(),
        })
    }

    /// 排序全部键后并行检测重复；返回重复组数量。
    ///
    /// 空范围（无键或起止颠倒）直接返回 0；worker 出错时取消上下文并中止任务队列。
    /// Returns the count even when detection fails, as Go's (numDups, error).
    pub fn detect(
        &self,
        ctx: &CancellationToken,
        opts: Option<&mut DetectOptions>,
    ) -> (i64, Result<(), Error>) {
        let num_dups = Arc::new(AtomicI64::new(0));
        let result = self.detect_inner(ctx, opts, &num_dups);
        (num_dups.load(Ordering::Relaxed), result)
    }

    fn detect_inner(
        &self,
        ctx: &CancellationToken,
        opts: Option<&mut DetectOptions>,
        num_dups: &Arc<AtomicI64>,
    ) -> Result<(), Error> {
        // 补齐并发度与默认空 Handler。
        let mut defaults = DetectOptions::default();
        let opts = opts.unwrap_or(&mut defaults);
        opts.ensure_defaults();

        let log_task = self.logger.Begin(Level::Info, "sort keys");
        let sort_result = self.sorter.sort(ctx);
        let log_error = sort_result
            .as_ref()
            .err()
            .map(|error| error.as_ref() as &(dyn StdError + 'static));
        log_task.End(Level::Error, log_error, []);
        sort_result?;

        // 取排序结果的首末键作为总扫描区间；末键追加 0 字节形成开区间上界。
        let (start_key, end_key) = self.get_range_bounds(ctx)?;
        if compare_internal_key(&start_key, &end_key) >= 0 {
            return Ok(());
        }

        let tasks = Arc::new(TaskQueue::with_initial(Task { start_key, end_key }));
        let first_error = Arc::new(Mutex::new(None::<Error>));
        let worker_context = ctx.child_token();
        let constructor = opts
            .handler_constructor
            .clone()
            .expect("detect options install a handler constructor");

        // 按 concurrency 拉起 worker；任一失败则 cancel + abort，保留首个错误。
        std::thread::scope(|scope| {
            for _ in 0..opts.concurrency {
                let sorter = self.sorter.clone();
                let tasks = tasks.clone();
                let num_dups = num_dups.clone();
                let first_error = first_error.clone();
                let logger = self.logger.clone();
                let context = worker_context.clone();
                let constructor = constructor.clone();

                scope.spawn(move || {
                    let result = constructor(&context).and_then(|handler| {
                        let mut worker = Worker {
                            sorter,
                            tasks: tasks.clone(),
                            num_dups,
                            handler,
                            logger,
                        };
                        worker.run(&context)
                    });
                    if let Err(error) = result {
                        let mut slot = first_error
                            .lock()
                            .expect("duplicate detector error mutex poisoned");
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                        drop(slot);
                        context.cancel();
                        tasks.abort();
                    }
                });
            }

            tasks.wait_until_idle();
            tasks.close();
        });

        // errgroup.Wait cancels the derived context on success as well.
        worker_context.cancel();

        if let Some(error) = first_error
            .lock()
            .expect("duplicate detector error mutex poisoned")
            .take()
        {
            return Err(error);
        }
        Ok(())
    }

    /// 读取外排迭代器的 first/last，解码为内部键区间。
    fn get_range_bounds(
        &self,
        ctx: &CancellationToken,
    ) -> Result<(InternalKey, InternalKey), Error> {
        let mut iterator = self.sorter.new_iterator(ctx)?;
        let result = (|| {
            let mut start_key = InternalKey::default();
            let mut end_key = InternalKey::default();

            if iterator.first() {
                decode_internal_key(iterator.unsafe_key(), &mut start_key)?;
            } else if let Some(error) = iterator.take_error() {
                return Err(error);
            }

            if iterator.last() {
                decode_internal_key(iterator.unsafe_key(), &mut end_key)?;
                end_key.key.push(0);
            } else if let Some(error) = iterator.take_error() {
                return Err(error);
            }
            Ok((start_key, end_key))
        })();
        let _ = iterator.close();
        result
    }
}

/// 向外部排序器写入编码后内部键的增量添加器。
pub struct KeyAdder {
    /// 外排 Writer。
    writer: Box<dyn Writer>,
    /// 复用的编码缓冲，避免每次分配。
    key_buf: Vec<u8>,
}

impl KeyAdder {
    /// 编码 `(key, key_id)` 并写入 sorter；value 为空。
    pub fn add(&mut self, key: &[u8], key_id: &[u8]) -> Result<(), Error> {
        self.key_buf.clear();
        encode_internal_key(
            &mut self.key_buf,
            &InternalKey::new(key.to_vec(), key_id.to_vec()),
        );
        self.writer.put(&self.key_buf, &[])
    }

    /// 刷写底层 Writer 缓冲。
    pub fn flush(&mut self) -> Result<(), Error> {
        self.writer.flush()
    }

    /// 关闭底层 Writer，完成该添加器的写入。
    pub fn close(&mut self) -> Result<(), Error> {
        self.writer.close()
    }
}

/// 为每个 worker 构造 `Handler` 的工厂（需 Send+Sync，可跨线程克隆）。
pub type HandlerConstructor =
    Arc<dyn Fn(&CancellationToken) -> Result<Box<dyn Handler>, Error> + Send + Sync>;

#[derive(Default)]
/// 检测选项：并行度与 Handler 工厂。
pub struct DetectOptions {
    /// 并行 worker 数量；非正值使用运行时 GOMAXPROCS(0)。
    pub concurrency: i64,
    /// 每个 worker 的 Handler 工厂；缺省为 NopHandler。
    pub handler_constructor: Option<HandlerConstructor>,
}

impl DetectOptions {
    /// concurrency<=0 时取运行时 GOMAXPROCS(0)；未设 Handler 时使用空操作 NopHandler。
    fn ensure_defaults(&mut self) {
        if self.concurrency <= 0 {
            self.concurrency = goish::runtime::GOMAXPROCS(0);
        }
        if self.handler_constructor.is_none() {
            self.handler_constructor = Some(Arc::new(|_| Ok(Box::new(NopHandler))));
        }
    }
}

/// 重复组回调：begin(用户键) → 多次 append(key_id) → end；结束时 close。
pub trait Handler: Send {
    /// 开始一个重复组，传入用户键。
    fn begin(&mut self, key: &[u8]) -> Result<(), Error>;
    /// 追加该组内一条记录的来源 key_id。
    fn append(&mut self, key_id: &[u8]) -> Result<(), Error>;
    /// 结束当前重复组。
    fn end(&mut self) -> Result<(), Error>;
    /// 关闭 Handler，释放资源。
    fn close(&mut self) -> Result<(), Error>;
}

/// 默认空 Handler：忽略所有回调，仅用于不需要收集结果的探测。
struct NopHandler;

impl Handler for NopHandler {
    fn begin(&mut self, _: &[u8]) -> Result<(), Error> {
        Ok(())
    }

    fn append(&mut self, _: &[u8]) -> Result<(), Error> {
        Ok(())
    }

    fn end(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
