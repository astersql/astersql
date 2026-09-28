// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// SQL 解析器性能基准测试（benchmark）的 Rust 迁移占位。
//
// 对照 `bench_test.go`：覆盖 sysbench 风格 SELECT、复杂多表 SQL fixture，
// 以及短 insert/select；`testing::B` 与 `parser::New`/`Parse` 按 Go 入口形状保留。

// 描述 parser 在简单 SQL、复杂 SQL 和 sysbench 查询上的解析 benchmark。
// testing::B、parser::New 和 Parse 调用均按 Go 入口形状保留为迁移占位。
use parser::New;

/// 模拟 Go `testing.B` 的最小子集，供迁移期 smoke/benchmark 入口复用。
mod testing {
    /// 基准状态：`N` 对应 Go 的迭代次数。
    #[allow(non_snake_case)]
    pub struct B {
        /// 循环迭代次数，对应 Go `b.N`。
        pub N: usize,
    }
    #[allow(non_snake_case)]
    impl B {
        /// 重置计时器；迁移占位为空操作。
        pub fn ResetTimer(&mut self) {}
        /// 报告分配；迁移占位为空操作。
        pub fn ReportAllocs(&mut self) {}
        /// 致命错误：对应 Go `b.Fatal`，此处直接 panic。
        pub fn Fatal<E: std::fmt::Display>(&mut self, error: E) {
            panic!("{error}")
        }
        // Go benchmark calls b.Failed() only as an observation; it does not mark failure.
        /// 记录失败但不中断；对应 Go `b.Failed()` 的观察语义。
        pub fn Failed(&mut self) {}
    }
}

/// BenchmarkSysbenchSelect 对应 Go benchmark：反复解析一条 sysbench 风格 SELECT。
// BenchmarkSysbenchSelect 对应 Go benchmark：反复解析一条 sysbench 风格 SELECT。
pub fn BenchmarkSysbenchSelect(b: &mut testing::B) {
    let mut parser = New();
    let sql = "SELECT pad FROM sbtest1 WHERE id=1;";
    b.ResetTimer();
    for _ in 0..b.N {
        if let Err(err) = parser.Parse(sql, "", "") {
            // Go 中 b.Fatal 会终止当前 benchmark；这里保留失败即停止的语义。
            b.Fatal(err);
        }
    }
    b.ReportAllocs();
}

/// BenchmarkParseComplex 对应 Go benchmark：解析包含子查询、函数调用和 Oracle 风格函数名的长 SQL fixture。
// BenchmarkParseComplex 对应 Go benchmark：解析包含子查询、函数调用和 Oracle 风格函数名的长 SQL fixture。
pub fn BenchmarkParseComplex(b: &mut testing::B) {
    let table = [
        r#"SELECT DISTINCT ca.l9_convergence_code AS atb2, cu.cust_sub_type AS account_type, cst.description AS account_type_desc, ss.prim_resource_val AS msisdn, ca.ban AS ban_key, To_char(mo.memo_date, 'YYYYMMDD') AS memo_date, cu.l9_identification AS thai_id, ss.subscriber_no AS subs_key, ss.dealer_code AS shop_code, cd.description AS shop_name, mot.short_desc, Regexp_substr(mo.attr1value, '[^ ;]+', 1, 3) staff_id, mo.operator_id AS user_id, mo.memo_system_text, co2.soc_name AS first_socname, co3.soc_name AS previous_socname, co.soc_name AS current_socname, Regexp_substr(mo.attr1value, '[^ ; ]+', 1, 1) NAME, co.soc_description AS current_pp_desc, co3.soc_description AS prev_pp_desc, co.soc_cd AS soc_cd, ( SELECT Sum(br.amount) FROM bl1_rc_rates BR, customer CU, subscriber SS WHERE br.service_receiver_id = ss.subscriber_no AND br.receiver_customer = ss.customer_id AND br.effective_date <= br.expiration_date AND (( ss. sub_status <> 'C' AND ss. sub_status <> 'T' AND br.expiration_date IS NULL) OR ( ss. sub_status = 'C' AND br.expiration_date LIKE ss.effective_date)) AND br.pp_ind = 'Y' AND br.cycle_code = cu.bill_cycle) AS pp_rate, cu.bill_cycle AS cycle_code, To_char(Nvl(ss.l9_tmv_act_date, ss.init_act_date),'YYYYMMDD') AS activated_date, To_char(cd.effective_date, 'YYYYMMDD') AS shop_effective_date, cd.expiration_date AS shop_expired_date, ca.l9_company_code AS company_code FROM service_details S, product CO, csm_pay_channel CPC, account CA, subscriber SS, customer CU, customer_sub_type CST, csm_dealer CD, service_details S2, product CO2, service_details S3, product CO3, memo MO , memo_type MOT, logical_date LO, charge_details CHD WHERE ss.subscriber_no = chd.agreement_no AND cpc.pym_channel_no = chd.target_pcn AND chd.chg_split_type = 'DR' AND chd.expiration_date IS NULL AND s.soc = co.soc_cd AND co.soc_type = 'P' AND s.agreement_no = ss.subscriber_no AND ss.prim_resource_tp = 'C' AND cpc.payment_category = 'POST' AND ca.ban = cpc.ban AND ( ca.l9_company_code = 'RF' OR ca.l9_company_code = 'RM' OR ca.l9_company_code = 'TM') AND ss.customer_id = cu.customer_id AND cu.cust_sub_type = cst.cust_sub_type AND cu.customer_type = cst.customer_type AND ss.dealer_code = cd.dealer AND s2.effective_date= ( SELECT Max(sa1.effective_date) FROM service_details SA1, product o1 WHERE sa1.agreement_no = ss.subscriber_no AND co.soc_cd = sa1.soc AND co.soc_type = 'P' ) AND s2.agreement_no = s.agreement_no AND s2.soc = co2.soc_cd AND co2.soc_type = 'P' AND s2.effective_date = ( SELECT Min(sa1.effective_date) FROM service_details SA1, product o1 WHERE sa1.agreement_no = ss.subscriber_no AND co2.soc_cd = sa1.soc AND co.soc_type = 'P' ) AND s3.agreement_no = s.agreement_no AND s3.soc = co3.soc_cd AND co3.soc_type = 'P' AND s3.effective_date = ( SELECT Max(sa1.effective_date) FROM service_details SA1, a product o1 WHERE sa1.agreement_no = ss.subscriber_no AND sa1.effective_date < ( SELECT Max(sa1.effective_date) FROM service_details SA1, product o1 WHERE sa1.agreement_no = ss.subscriber_no AND co3.soc_cd = sa1.soc AND co3.soc_type = 'P' ) AND co3.soc_cd = sa1.soc AND o1.soc_type = 'P' ) AND mo.entity_id = ss.subscriber_no AND mo.entity_type_id = 6 AND mo.memo_type_id = mot.memo_type_id AND Trunc(mo.sys_creation_date) = ( SELECT Trunc(lo.logical_date - 1) FROM lo) trunc(lo.logical_date - 1) AND lo.expiration_date IS NULL AND lo.logical_date_type = 'B' AND lo.expiration_date IS NULL AND ( mot.short_desc = 'BCN' OR mot.short_desc = 'BCNM' )"#,
    ];
    let mut parser = New();
    b.ResetTimer();
    for _ in 0..b.N {
        for v in table {
            if parser.Parse(v, "", "").is_err() {
                // Go 原代码调用 b.Failed() 记录失败但不 fatal；这里保留非中断式失败检查。
                b.Failed();
            }
        }
    }
    b.ReportAllocs();
}

/// BenchmarkParseSimple 对应 Go benchmark：解析三条短 SQL，覆盖 insert values 和 where 条件。
// BenchmarkParseSimple 对应 Go benchmark：解析三条短 SQL，覆盖 insert values 和 where 条件。
pub fn BenchmarkParseSimple(b: &mut testing::B) {
    let table = [
        "insert into t values (1), (2), (3)",
        "insert into t values (4), (5), (6), (7)",
        "select c from t where c > 2",
    ];
    let mut parser = New();
    b.ResetTimer();
    for _ in 0..b.N {
        for v in table {
            if parser.Parse(v, "", "").is_err() {
                b.Failed();
            }
        }
    }
    b.ReportAllocs();
}

/// 冒烟：以 N=1 跑一遍 sysbench SELECT benchmark，确认入口可调用。
#[test]
fn benchmark_sysbench_select_smoke() {
    BenchmarkSysbenchSelect(&mut testing::B { N: 1 });
}

/// 冒烟：以 N=1 跑一遍复杂 SQL benchmark。
#[test]
fn benchmark_parse_complex_smoke() {
    BenchmarkParseComplex(&mut testing::B { N: 1 });
}

/// 冒烟：以 N=1 跑一遍简单 SQL benchmark。
#[test]
fn benchmark_parse_simple_smoke() {
    BenchmarkParseSimple(&mut testing::B { N: 1 });
}
