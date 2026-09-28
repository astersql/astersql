// Copyright 2024 PingCAP, Inc.
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

//! 实例级计划缓存的预处理 DML 对照用例。
//!
//! 主要通过相同初始数据比较“占位符绑定后执行”与“参数直接写入 SQL”两条路径，
//! 并覆盖基础增删改、TPCC 风格更新、指定分区更新及分区表 DML。

use super::support::{Harness, exec, query, rand};

/// TPCC 仓库表的最小化测试结构。
pub const tpccWarehouse: &str =
    "create table warehouse (w_id int primary key, w_ytd decimal(12,2))";
/// TPCC 地区表的最小化测试结构。
pub const tpccDistrict: &str = "create table district (d_id int, d_w_id int, d_ytd decimal(12,2), d_next_o_id int, primary key(d_w_id,d_id))";
/// TPCC 客户表的最小化测试结构。
pub const tpccCustomer: &str = "create table customer (c_id int, c_d_id int, c_w_id int, c_balance decimal(12,2), c_ytd_payment decimal(12,2), c_payment_cnt int, c_delivery_cnt int, c_data varchar(500), primary key(c_w_id,c_d_id,c_id))";
/// TPCC 库存表的最小化测试结构。
pub const tpccStock: &str =
    "create table stock (s_i_id int, s_w_id int, s_quantity int, primary key(s_w_id,s_i_id))";
/// TPCC 订单表的最小化测试结构。
pub const tpccOrders: &str = "create table orders (o_id int, o_d_id int, o_w_id int, o_c_id int, o_carrier_id int, primary key(o_w_id,o_d_id,o_id))";
/// TPCC 新订单表的最小化测试结构。
pub const tpccNewOrder: &str = "create table new_order (no_o_id int, no_d_id int, no_w_id int, primary key(no_w_id,no_d_id,no_o_id))";

#[derive(Debug, Clone, PartialEq, Eq)]
/// 一组语义等价的预处理 DML 与直接 DML，用于比较两条执行路径。
pub struct DmlPair {
    /// 创建预处理语句的 SQL。
    pub prep: String,
    /// 为占位符设置会话变量的 SQL。
    pub set: String,
    /// 使用会话变量执行预处理语句的 SQL。
    pub exec: String,
    /// 将相同参数直接写入语句的对照 SQL。
    pub direct: String,
}

/// 依次执行预处理路径及其直接 SQL 对照路径。
fn execute_pair(tk: &mut astersql_testkit::TestKit, pair: &DmlPair) {
    exec(tk, &pair.prep);
    exec(tk, &pair.set);
    exec(tk, &pair.exec);
    exec(tk, &pair.direct);
}

/// 忽略返回顺序，比较两条查询的完整结果集。
fn rows_equal(tk: &astersql_testkit::TestKit, left: &str, right: &str) {
    let mut left_rows = query(tk, left);
    let mut right_rows = query(tk, right);
    left_rows.Sort();
    right_rows.Sort();
    left_rows.Check(right_rows.Rows());
}

#[test]
/// 在两张同构表上分别执行预处理和直接 DML，验证最终数据一致。
pub fn TestInstancePlanCacheDMLBasic() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t1(a int,b int,c int,key(a),key(b,c))",
    );
    exec(
        &mut harness.tk,
        "create table t2(a int,b int,c int,key(a),key(b,c))",
    );
    for _ in 0..50 {
        let a = rand::intn(100);
        let b = rand::intn(100);
        let c = rand::intn(100);
        let insert = DmlPair {
            prep: "prepare stmt from 'insert into t1 values (?,?,?)'".into(),
            set: format!("set @a={a},@b={b},@c={c}"),
            exec: "execute stmt using @a,@b,@c".into(),
            direct: format!("insert into t2 values ({a},{b},{c})"),
        };
        execute_pair(&mut harness.tk, &insert);
    }
    for _ in 0..100 {
        let a = rand::intn(100);
        let b = rand::intn(100);
        let c = rand::intn(100);
        let pair = match rand::intn(3) {
            0 => DmlPair {
                prep: "prepare stmt from 'insert into t1 values (?,?,?)'".into(),
                set: format!("set @a={a},@b={b},@c={c}"),
                exec: "execute stmt using @a,@b,@c".into(),
                direct: format!("insert into t2 values ({a},{b},{c})"),
            },
            1 => DmlPair {
                prep: "prepare stmt from 'delete from t1 where a<? and b<? or c<?'".into(),
                set: format!("set @a={a},@b={b},@c={c}"),
                exec: "execute stmt using @a,@b,@c".into(),
                direct: format!("delete from t2 where a<{a} and b<{b} or c<{c}"),
            },
            _ => DmlPair {
                prep: "prepare stmt from 'update t1 set a=? where b=? or c=?'".into(),
                set: format!("set @a={a},@b={b},@c={c}"),
                exec: "execute stmt using @a,@b,@c".into(),
                direct: format!("update t2 set a={a} where b={b} or c={c}"),
            },
        };
        execute_pair(&mut harness.tk, &pair);
        rows_equal(&harness.tk, "select * from t1", "select * from t2");
    }
}

#[test]
/// 用两个会话和两套 TPCC 数据验证启用实例计划缓存后的更新结果一致性。
pub fn TestInstancePlanCacheDMLTPCC() {
    let mut harness = Harness::new();
    let mut tk2 = harness.session();
    let mut tk1 = harness.tk;

    exec(&mut tk1, "create database tpcc1");
    exec(&mut tk1, "use tpcc1");
    exec(&mut tk1, tpccWarehouse);
    exec(&mut tk1, tpccDistrict);
    exec(&mut tk1, tpccCustomer);
    for i in 0..100 {
        exec(&mut tk1, &format!("insert into warehouse values ({i},0.1)"));
        exec(
            &mut tk1,
            &format!("insert into district values ({i},{i},0.1,0)"),
        );
        exec(
            &mut tk1,
            &format!("insert into customer values ({i},{i},{i},0.1,0,0,0,'data')"),
        );
    }

    exec(&mut tk2, "create database tpcc2");
    exec(&mut tk2, "use tpcc2");
    exec(&mut tk2, tpccWarehouse);
    exec(&mut tk2, tpccDistrict);
    exec(&mut tk2, tpccCustomer);
    for i in 0..100 {
        exec(&mut tk2, &format!("insert into warehouse values ({i},0.1)"));
        exec(
            &mut tk2,
            &format!("insert into district values ({i},{i},0.1,0)"),
        );
        exec(
            &mut tk2,
            &format!("insert into customer values ({i},{i},{i},0.1,0,0,0,'data')"),
        );
    }

    exec(&mut tk1, "set global tidb_enable_instance_plan_cache=1");
    for i in 0..100 {
        exec(
            &mut tk1,
            "prepare stmt from 'update district set d_ytd=d_ytd+? where d_w_id=? and d_id=?'",
        );
        exec(&mut tk1, &format!("set @amount=1.0,@w={i},@d={i}"));
        exec(&mut tk1, "execute stmt using @amount,@w,@d");
        exec(
            &mut tk1,
            &format!("update warehouse set w_ytd=w_ytd+1.0 where w_id={i}"),
        );
        exec(
            &mut tk2,
            &format!("update warehouse set w_ytd=w_ytd+1.0 where w_id={i}"),
        );
    }
    let mut left_rows = query(&tk1, "select * from warehouse");
    let mut right_rows = query(&tk2, "select * from warehouse");
    left_rows.Sort();
    right_rows.Sort();
    left_rows.Check(right_rows.Rows());
}

/// 构造付款流程中累加地区年累计额的预处理/直接更新对。
pub fn paymentUpdateDistrict() -> DmlPair {
    DmlPair {
        prep: "prepare stmt from 'update district set d_ytd=d_ytd+? where d_w_id=? and d_id=?'"
            .into(),
        set: "set @amount=1,@w_id=1,@d_id=1".into(),
        exec: "execute stmt using @amount,@w_id,@d_id".into(),
        direct: "update district set d_ytd=d_ytd+1 where d_w_id=1 and d_id=1".into(),
    }
}

/// 构造付款流程中累加仓库年累计额的预处理/直接更新对。
pub fn paymentUpdateWarehouse() -> DmlPair {
    DmlPair {
        prep: "prepare stmt from 'update warehouse set w_ytd=w_ytd+? where w_id=?'".into(),
        set: "set @amount=1,@w_id=1".into(),
        exec: "execute stmt using @amount,@w_id".into(),
        direct: "update warehouse set w_ytd=w_ytd+1 where w_id=1".into(),
    }
}

/// 构造付款流程中同步客户余额、付款计数和附加数据的更新对。
pub fn paymentUpdateCustomerWithData() -> DmlPair {
    DmlPair { prep: "prepare stmt from 'update customer set c_balance=c_balance-?,c_ytd_payment=c_ytd_payment+?,c_payment_cnt=c_payment_cnt+1,c_data=? where c_w_id=? and c_d_id=? and c_id=?'".into(), set: "set @a=1,@data='data',@w=1,@d=1,@c=1".into(), exec: "execute stmt using @a,@a,@data,@w,@d,@c".into(), direct: "update customer set c_balance=c_balance-1,c_ytd_payment=c_ytd_payment+1,c_payment_cnt=c_payment_cnt+1,c_data='data' where c_w_id=1 and c_d_id=1 and c_id=1".into() }
}

/// 构造新订单流程中递增地区下一订单号的更新对。
pub fn newOrderUpdateDistrict() -> DmlPair {
    DmlPair { prep: "prepare stmt from 'update district set d_next_o_id=d_next_o_id+1 where d_id=? and d_w_id=?'".into(), set: "set @d=1,@w=1".into(), exec: "execute stmt using @d,@w".into(), direct: "update district set d_next_o_id=d_next_o_id+1 where d_id=1 and d_w_id=1".into() }
}

/// 构造配送流程中更新客户余额和配送计数的更新对。
pub fn deliveryUpdateCustomer() -> DmlPair {
    DmlPair { prep: "prepare stmt from 'update customer set c_balance=c_balance+?,c_delivery_cnt=c_delivery_cnt+1 where c_w_id=? and c_d_id=? and c_id=?'".into(), set: "set @a=1,@w=1,@d=1,@c=1".into(), exec: "execute stmt using @a,@w,@d,@c".into(), direct: "update customer set c_balance=c_balance+1,c_delivery_cnt=c_delivery_cnt+1 where c_w_id=1 and c_d_id=1 and c_id=1".into() }
}

/// 构造基础表插入的预处理/直接 DML 对。
pub fn randInsert() -> DmlPair {
    DmlPair {
        prep: "prepare stmt from 'insert into t1 values (?,?,?)'".into(),
        set: "set @a=1,@b=2,@c=3".into(),
        exec: "execute stmt using @a,@b,@c".into(),
        direct: "insert into t2 values (1,2,3)".into(),
    }
}
/// 构造基础表删除的预处理/直接 DML 对。
pub fn randDelete() -> DmlPair {
    DmlPair {
        prep: "prepare stmt from 'delete from t1 where a<?'".into(),
        set: "set @a=1".into(),
        exec: "execute stmt using @a".into(),
        direct: "delete from t2 where a<1".into(),
    }
}
/// 构造基础表更新的预处理/直接 DML 对。
pub fn randUpdate() -> DmlPair {
    DmlPair {
        prep: "prepare stmt from 'update t1 set a=? where b=?'".into(),
        set: "set @a=4,@b=2".into(),
        exec: "execute stmt using @a,@b".into(),
        direct: "update t2 set a=4 where b=2".into(),
    }
}

#[test]
/// 逐个指定范围分区执行更新，并与直接更新后的同构表比较。
pub fn TestInstancePlanCacheUpdateSpecifiedPartition() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t1(a int,b int) partition by range(a)(partition p0 values less than(10),partition p1 values less than(20),partition p2 values less than(30),partition p3 values less than(40))",
    );
    exec(&mut harness.tk, "create table t2 like t1");
    for i in 0..40 {
        exec(&mut harness.tk, &format!("insert into t1 values ({i},{i})"));
        exec(&mut harness.tk, &format!("insert into t2 values ({i},{i})"));
    }
    exec(&mut harness.tk, "set @v=1");
    for _ in 0..100 {
        let partition = rand::intn(5);
        if partition < 4 {
            exec(
                &mut harness.tk,
                &format!("prepare st from 'update t1 partition(p{partition}) set b=b+?'"),
            );
            exec(&mut harness.tk, "execute st using @v");
            exec(&mut harness.tk, "execute st using @v");
            query(&harness.tk, "select @@last_plan_from_cache").Check(vec![vec!["1"]]);
            exec(
                &mut harness.tk,
                &format!("update t2 partition(p{partition}) set b=b+2"),
            );
        } else {
            exec(&mut harness.tk, "prepare st from 'update t1 set b=b+?'");
            exec(&mut harness.tk, "execute st using @v");
            exec(&mut harness.tk, "execute st using @v");
            query(&harness.tk, "select @@last_plan_from_cache").Check(vec![vec!["1"]]);
            exec(&mut harness.tk, "update t2 set b=b+2");
        }
        rows_equal(&harness.tk, "select * from t1", "select * from t2");
    }
}

#[test]
/// 覆盖哈希分区表上的 UPDATE、DELETE 与 REPLACE 预处理执行路径。
pub fn TestInstancePlanCacheDMLPartitioning() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "use test");
    exec(
        &mut harness.tk,
        "set global tidb_enable_instance_plan_cache=1",
    );
    exec(
        &mut harness.tk,
        "create table t1(a int,b int) partition by range(a)(partition p0 values less than(10),partition p1 values less than(20),partition p2 values less than(30),partition p3 values less than(40))",
    );
    exec(
        &mut harness.tk,
        "create table t2(a int,b int) partition by hash(a) partitions 4",
    );
    exec(
        &mut harness.tk,
        "create table t3(a int,b int) partition by list columns(a)(partition p0 values in(0,1,2),partition p1 values in(3,4,5),partition p2 values in(6,7,8),partition p3 values in(9,10,11))",
    );
    for table in ["t1", "t2", "t3"] {
        let cases = [
            (format!("insert into {table} values (?,?)"), "@a,@b"),
            (format!("insert into {table}(a,b) values (?,?)"), "@a,@b"),
            (format!("delete from {table}"), ""),
            (format!("delete from {table} where a=?"), "@a"),
            (format!("delete from {table} where a=? and b=?"), "@a,@b"),
            (format!("update {table} set b=1"), ""),
            (format!("update {table} set b=1 where a=?"), "@a"),
            (format!("update {table} set b=1 where a=? and b=?"), "@a,@b"),
            (format!("update {table} partition(p0) set b=1"), ""),
            (format!("update {table} partition(p0,p1) set b=1"), ""),
            (format!("update {table} partition(p0,p1,p2) set b=1"), ""),
            (format!("replace into {table} values (?,?)"), "@a,@b"),
            (format!("replace into {table}(a,b) values (?,?)"), "@a,@b"),
        ];
        for (sql, args) in cases {
            exec(&mut harness.tk, &format!("prepare st from '{sql}'"));
            exec(&mut harness.tk, "set @a=1,@b=2");
            let execute = if args.is_empty() {
                "execute st".to_string()
            } else {
                format!("execute st using {args}")
            };
            exec(&mut harness.tk, &execute);
            exec(&mut harness.tk, &execute);
            query(&harness.tk, "select @@last_plan_from_cache").Check(vec![vec!["1"]]);
        }
    }
}
