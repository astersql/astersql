// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Concurrent DML workload for add-index tests
//! (Go `tests/realtikvtest/testutils/workload.go`).

use crate::common::{SuiteContext, TkPool};
use crate::stubs::{logutil, require, testkit};
use chrono::{Datelike, Local};
use rand::Rng;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
// Receiver is !Sync; keep it behind Mutex so Workload can live in Arc<Mutex<_>>.
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

static SKIPPED_ERROR_CODE: &[&str] = &[
    "8028",     // ErrInfoSchemaChanged
    "9007",     // ErrWriteConflict
    "global:2", // execution result undetermined
];

/// Shared package-level workload pointer target (Go `wCtx`).
pub(crate) fn shared_workload_arc() -> Arc<Mutex<Workload>> {
    static ARC: OnceLock<Arc<Mutex<Workload>>> = OnceLock::new();
    ARC.get_or_init(|| Arc::new(Mutex::new(Workload::default())))
        .clone()
}

pub(crate) fn initWorkloadParams(ctx: &mut SuiteContext) {
    ctx.cancelled = Arc::new(AtomicBool::new(false));
    let wl = shared_workload_arc();
    ctx.workload = Some(wl);
    ctx.tkPool = Some(Arc::new(TkPool::new(ctx.t.clone(), ctx.store.clone())));
}

pub(crate) fn initWorkLoadContext(ctx: &mut Workload, col_ids: &[i32]) {
    if col_ids.len() < 2 {
        return;
    }
    ctx.tableID = col_ids[0];
    ctx.tableName = format!("t{}", ctx.tableID);
    ctx.colID.clear();
    for &id in &col_ids[1..] {
        ctx.colID.push(id);
    }
    // set started insert id to 10000
    *ctx.insertID.lock().unwrap() = 10000;
    ctx.date = "2008-02-02".into();
}

struct WorkChan {
    err: Option<String>,
    finished: bool,
}

struct Worker {
    id: i32,
    load_type: i32, // 0 insert, 1 update, 2 delete
    tx: Mutex<Option<SyncSender<WorkChan>>>,
    rx: Mutex<Option<Receiver<WorkChan>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Worker {
    fn new(w_id: i32, ty: i32) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        Self {
            id: w_id,
            load_type: ty,
            tx: Mutex::new(Some(tx)),
            rx: Mutex::new(Some(rx)),
            handle: Mutex::new(None),
        }
    }
}

/// Workload state matching Go `workload`.
#[derive(Default)]
pub struct Workload {
    pub tableName: String,
    pub tableID: i32,
    pub colID: Vec<i32>,
    pub workerNum: i32,
    wr: Vec<Worker>,
    pub insertID: Arc<Mutex<isize>>,
    pub date: String,
}

impl Workload {
    pub(crate) fn start(&mut self, ctx: &SuiteContext, col_ids: &[i32]) {
        initWorkLoadContext(self, col_ids);
        // Drop any previous workers.
        self.wr.clear();
        for i in 0..3 {
            let worker = Worker::new(i, i);
            let tx = worker.tx.lock().unwrap().take().unwrap();
            let load_type = worker.load_type;
            let cancelled = ctx.cancelled.clone();
            let t = ctx.t.clone();
            let store = ctx.store.clone();
            let pool = ctx.tkPool.clone().expect("tkPool");
            let table_name = self.tableName.clone();
            let date = self.date.clone();
            let col_id = self.colID.clone();
            let insert_id_for_worker = self.insertID.clone();
            let row_num = ctx.rowNum;
            let is_pk = ctx.isPK.clone();
            let is_unique = ctx.isUnique.clone();
            *worker.handle.lock().unwrap() = Some(thread::spawn(move || {
                let tk = pool.get();
                tk.MustExec("use addindex");
                tk.MustExec("set @@tidb_general_log = 1;");
                let mut err = None;
                loop {
                    if cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    let r = match load_type {
                        0 => insertWorker(
                            &tk,
                            &table_name,
                            &date,
                            &insert_id_for_worker,
                            &is_pk,
                            &is_unique,
                        ),
                        1 => updateWorker(&tk, &table_name, &col_id, row_num, &is_pk, &is_unique),
                        2 => deleteWorker(&tk, &table_name, row_num, &is_pk, &is_unique),
                        _ => Ok(()),
                    };
                    if let Err(e) = r {
                        err = Some(e);
                        break;
                    }
                }
                let _ = t;
                let _ = store;
                let _ = tx.send(WorkChan {
                    err,
                    finished: true,
                });
            }));
            self.wr.push(worker);
        }
    }

    pub(crate) fn stop(&mut self, ctx: &SuiteContext, _table_id: i32) -> Result<(), String> {
        ctx.cancel();
        let mut count = 3;
        let mut first_err = None;
        for i in 0..3 {
            let rx = self.wr[i].rx.lock().unwrap().take().expect("worker rx");
            let wchan = rx.recv().unwrap_or(WorkChan {
                err: Some("worker channel closed".into()),
                finished: true,
            });
            if let Some(e) = wchan.err {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            if wchan.finished {
                count -= 1;
            }
            if let Some(h) = self.wr[i].handle.lock().unwrap().take() {
                let _ = h.join();
            }
            if count == 0 {
                break;
            }
        }
        match first_err {
            Some(e) => {
                require::NoError(&ctx.t, Err(e.clone()));
                Err(e)
            }
            None => Ok(()),
        }
    }
}

pub(crate) fn isSkippedError(err: &Option<String>, is_pk: bool, is_unique: bool) -> bool {
    let Some(e) = err else {
        return true;
    };
    if is_pk || is_unique {
        if e.find("1062").is_some_and(|pos| pos > 0) {
            return true;
        }
    }
    for code in SKIPPED_ERROR_CODE {
        if e.contains(code) {
            return true;
        }
    }
    false
}

pub(crate) fn insertStr(table_name: &str, id: isize, date: &str) -> String {
    format!(
        "insert into addindex.{table_name}(c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16, c17, c18, c19, c20, c21, c22, c23, c24, c25, c26, c27, c28) values({id},3, 3, 3, 3, 3, {id}, 3, 3.0, 3.0, 1113.1111, adddate('{date}', {}), '11:11:13', '2001-01-03 11:11:13', '2001-01-03 11:11:11.123456', 2001, 'cccc', 'cccc', 'cccc', 'aaaa{id}', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', 'cccc', '{{\"name\": \"Beijing\", \"population\": 102}}')",
        id.wrapping_sub(10000)
    )
}

pub(crate) fn insertWorker(
    tk: &testkit::TestKit,
    table_name: &str,
    date: &str,
    insert_id: &Mutex<isize>,
    is_pk: &AtomicBool,
    is_unique: &AtomicBool,
) -> Result<(), String> {
    let mut id = insert_id.lock().unwrap();
    let ins_str = insertStr(table_name, *id, date);
    let (rs, err) = tk.ExecWithResult(&ins_str);
    if !isSkippedError(
        &err,
        is_pk.load(Ordering::SeqCst),
        is_unique.load(Ordering::SeqCst),
    ) {
        logutil::BgLogger().Info(
            "workload insert failed",
            &[
                ("category", "add index test".into()),
                ("sql", ins_str),
                ("error", err.clone().unwrap_or_default()),
            ],
        );
        return Err(err.unwrap_or_else(|| "unknown".into()));
    }
    if let Some(rs) = rs {
        rs.Close()?;
    }
    *id = id.wrapping_add(1);
    drop(id);
    thread::sleep(Duration::from_millis(10));
    Ok(())
}

pub(crate) fn updateStr(row_num: i32, table_name: &str, col_id: &[i32]) -> String {
    let mut rng = rand::thread_rng();
    let id = rng.gen_range(0..=i64::from(row_num));
    let mut update_str = String::new();
    for (i, &cid) in col_id.iter().enumerate() {
        let col_new_value = genColval(cid);
        if col_new_value.is_empty() {
            return String::new();
        }
        if i == 0 {
            update_str = format!(" set c{cid}={col_new_value}");
        } else {
            update_str.push_str(&format!(", c{cid}={col_new_value}"));
        }
    }
    format!("update addindex.{table_name}{update_str} where c0={id}")
}

pub(crate) fn updateWorker(
    tk: &testkit::TestKit,
    table_name: &str,
    col_id: &[i32],
    row_num: i32,
    is_pk: &AtomicBool,
    is_unique: &AtomicBool,
) -> Result<(), String> {
    let up_str = updateStr(row_num, table_name, col_id);
    let (rs, err) = tk.ExecWithResult(&up_str);
    if !isSkippedError(
        &err,
        is_pk.load(Ordering::SeqCst),
        is_unique.load(Ordering::SeqCst),
    ) {
        logutil::BgLogger().Info(
            "workload update failed",
            &[
                ("category", "add index test".into()),
                ("sql", up_str),
                ("error", err.clone().unwrap_or_default()),
            ],
        );
        return Err(err.unwrap_or_else(|| "unknown".into()));
    }
    if let Some(rs) = rs {
        rs.Close()?;
    }
    thread::sleep(Duration::from_millis(10));
    Ok(())
}

pub(crate) fn deleteStr(table_name: &str, id: i64) -> String {
    format!("delete from addindex.{table_name} where c0 ={id}")
}

pub(crate) fn deleteWorker(
    tk: &testkit::TestKit,
    table_name: &str,
    row_num: i32,
    is_pk: &AtomicBool,
    is_unique: &AtomicBool,
) -> Result<(), String> {
    let mut rng = rand::thread_rng();
    let id = rng.gen_range(0..=i64::from(row_num));
    let del_str = deleteStr(table_name, id);
    let (rs, err) = tk.ExecWithResult(&del_str);
    if !isSkippedError(
        &err,
        is_pk.load(Ordering::SeqCst),
        is_unique.load(Ordering::SeqCst),
    ) {
        logutil::BgLogger().Info(
            "workload delete failed",
            &[
                ("category", "add index test".into()),
                ("sql", del_str),
                ("error", err.clone().unwrap_or_default()),
            ],
        );
        return Err(err.unwrap_or_else(|| "unknown".into()));
    }
    if let Some(rs) = rs {
        rs.Close()?;
    }
    thread::sleep(Duration::from_millis(10));
    Ok(())
}

/// Go `genColval` — including FormatFloat(fmt=10) → `"%\n"`.
pub(crate) fn genColval(col_id: i32) -> String {
    match col_id {
        1 | 2 | 3 | 4 | 5 | 7 => format!("c{col_id} + 1"),
        6 => format!("c{col_id} - 64"),
        8 | 9 | 10 => "%\n".to_string(), // strconv.FormatFloat(..., 10, 16, 32)
        11 => "adddate(c11, 90)".into(),
        12 | 13 | 14 | 15 => go_time_now_string(),
        17 => Local::now().year().to_string(),
        19 => "c19 + c0".into(),
        18 | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 => "ABCDEEEF".into(),
        28 => "json_object('name', 'NanJing', 'population', 2566)".into(),
        _ => String::new(),
    }
}

fn go_time_now_string() -> String {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = ORIGIN.get_or_init(Instant::now);
    let now = Local::now();
    let fraction = format!("{:09}", now.timestamp_subsec_nanos());
    let fraction = fraction.trim_end_matches('0');
    let fraction = if fraction.is_empty() {
        String::new()
    } else {
        format!(".{fraction}")
    };
    let monotonic = origin.elapsed();
    format!(
        "{}{fraction} {} m=+{}.{:09}",
        now.format("%Y-%m-%d %H:%M:%S"),
        format!("{} {}", now.format("%z"), local_zone(now.timestamp())),
        monotonic.as_secs(),
        monotonic.subsec_nanos()
    )
}

fn local_zone(timestamp: i64) -> String {
    let timestamp = timestamp as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    // localtime_r initializes `local` on success and owns the static zone string.
    unsafe {
        assert!(!libc::localtime_r(&timestamp, local.as_mut_ptr()).is_null());
        let local = local.assume_init();
        std::ffi::CStr::from_ptr(local.tm_zone)
            .to_string_lossy()
            .into_owned()
    }
}
