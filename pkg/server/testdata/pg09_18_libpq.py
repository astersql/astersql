# Copyright 2026 AsterSQL.
"""Window libpq/raw TCP differential. Reuses existing client implementations.

PORT DATABASE EVIDENCE MYSQL_PORT; --self-test needs no database.
Only uniquely named test databases are created/dropped. DSNs are never printed.
"""
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
from pg_function_23_libpq import Libpq, Wire, compare, compare_wire, self_test
from pg_function_27_libpq import Mysql

# Explicit expected values retain the successful fixtures from tasks 13–17.
CASES = [
    ('t13-0', 'SELECT last_value(y) OVER(ORDER BY x) AS r FROM (VALUES(10,1::int4),(10,2::int4),(20,3::int4)) t(x,y)', [23], [['2'], ['2'], ['3']]),
    ('t13-1', 'SELECT last_value(x) OVER() AS r FROM (VALUES(1::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t13-2', 'SELECT last_value(x) OVER(ORDER BY x) AS r FROM (VALUES(NULL::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t13-3', 'SELECT last_value(x) OVER(ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING) AS r FROM (VALUES(1::int4)) t(x)', [23], [[None]]),
    ('t13-4', 'SELECT last_value(x) OVER(ORDER BY x) AS r FROM (SELECT 1::int4 AS x WHERE false) t', [23], []),
    ('t13-5', 'SELECT last_value(x) OVER(PARTITION BY p ORDER BY x) AS r FROM (VALUES(1,10::int4),(1,20::int4),(2,5::int4)) t(p,x)', [23], [['10'], ['20'], ['5']]),
    ('t14-6', "SELECT last_value(y) OVER(ORDER BY x RANGE BETWEEN 1 PRECEDING AND CURRENT ROW) AS r FROM (VALUES('-9223372036854775808'::int8,1::int4),('-9223372036854775807'::int8,2::int4),(9223372036854775807::int8,3::int4)) t(x,y)", [23], [['1'], ['2'], ['3']]),
    ('t14-7', 'SELECT last_value(y) OVER(ORDER BY x RANGE BETWEEN 1 FOLLOWING AND 1 FOLLOWING) AS r FROM (VALUES(9223372036854775807::int8,3::int4)) t(x,y)', [23], [[None]]),
    ('t14-8', 'SELECT last_value(y) OVER(ORDER BY x DESC NULLS FIRST RANGE BETWEEN CURRENT ROW AND 0.1 FOLLOWING) AS r FROM (VALUES(9007199254740992.1::numeric,1::int4),(9007199254740992.2::numeric,2::int4),(NULL::numeric,3::int4)) t(x,y) ORDER BY y', [23], [['1'], ['1'], ['3']]),
    ('t14-9', 'SELECT last_value(y) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 0.2 FOLLOWING) AS r FROM (VALUES(0.1::float8,1::int4),(0.30000000000000004::float8,2::int4)) t(x,y)', [23], [['2'], ['2']]),
    ('t14-10', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING) AS r FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)', [23], [['10'], ['10'], ['20']]),
    ('t14-11', 'SELECT last_value(y) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 0.1 FOLLOWING) AS r FROM (VALUES(9007199254740992.1::numeric,1::int4),(9007199254740992.2::numeric,2::int4),(9007199254740992.4::numeric,3::int4)) t(x,y)', [23], [['2'], ['2'], ['3']]),
    ('t14-12', 'SELECT last_value(y) OVER(ORDER BY x DESC RANGE BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(9007199254740992::int8,1::int4),(9007199254740993::int8,2::int4),(9007199254741000::int8,3::int4)) t(x,y) ORDER BY y', [23], [['1'], ['1'], ['3']]),
    ('t14-13', 'SELECT last_value(x) OVER(ORDER BY x NULLS LAST RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING) AS r FROM (VALUES(10::int4),(NULL::int4),(20::int4)) t(x) ORDER BY x NULLS LAST', [23], [['10'], ['20'], [None]]),
    ('t14-14', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING) AS r FROM (VALUES(NULL::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t14-15', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND 5 FOLLOWING) AS r FROM (SELECT 1::int4 AS x WHERE false) t', [23], []),
    ('t15-16', 'SELECT lag((1/(id-1))::int4,NULL::int4,7::int4) OVER(ORDER BY id) AS r FROM (VALUES(1),(2)) t(id)', [23], [[None], [None]]),
    ('t15-17', 'SELECT lag(NULL,1,7::int4) OVER() AS r', [23], [['7']]),
    ('t15-18', 'SELECT lag(NULL,1,7::int4) OVER() AS r FROM (SELECT 1 AS x WHERE false) t', [23], []),
    ('t15-19', 'SELECT lag(x,(o+0),(id+0)::int4) OVER(ORDER BY id) AS r FROM (VALUES(1,10::int4,1),(2,20::int4,2),(3,30::int4,0)) t(id,x,o)', [23], [['1'], ['2'], ['30']]),
    ('t15-20', 'SELECT lag(x) OVER(ORDER BY id) AS r FROM (VALUES(1,10::int4),(2,NULL::int4),(3,30::int4)) t(id,x)', [23], [[None], ['10'], [None]]),
    ('t15-21', 'SELECT lead(x,0) OVER() AS r FROM (VALUES(NULL::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t15-22', "SELECT lag(x,'-2147483648'::int4,7::int4) OVER() AS r FROM (VALUES(10::int4)) t(x)", [23], [['7']]),
    ('t15-23', 'SELECT lead(x,2147483647,7::int4) OVER() AS r FROM (VALUES(10::int4)) t(x)', [23], [['7']]),
    ('t15-24', 'SELECT lag(x,o,id::int4) OVER(ORDER BY id) AS r FROM (VALUES(1,10::int4,1),(2,20::int4,2),(3,30::int4,0)) t(id,x,o)', [23], [['1'], ['2'], ['30']]),
    ('t15-25', 'SELECT lead(x,NULL::int4,99::int4) OVER(ORDER BY id) AS r FROM (VALUES(1,10::int4),(2,20::int4)) t(id,x)', [23], [[None], [None]]),
    ('t15-26', 'SELECT lag(x,-1,99::int4) OVER(ORDER BY id ROWS BETWEEN CURRENT ROW AND CURRENT ROW) AS r FROM (VALUES(1,10::int4),(2,NULL::int4),(3,30::int4)) t(id,x)', [23], [[None], ['30'], ['99']]),
    ('t15-27', 'SELECT lead(x,1,id::int4) OVER(PARTITION BY p ORDER BY id) AS r FROM (VALUES(1,1,10::int4),(1,2,20::int4),(2,3,30::int4)) t(p,id,x)', [23], [['20'], ['2'], ['3']]),
    ('t15-28', 'SELECT lag(x) OVER() AS r FROM (SELECT 1::int4 AS x WHERE false) t', [23], []),
    ('t15-29', 'SELECT lead(x,0,99::int4) OVER() AS r FROM (VALUES(NULL::int4)) t(x)', [23], [[None]]),
    ('t15-30', 'SELECT lead(x,-1,99::int4) OVER(ORDER BY id) AS r FROM (VALUES(1,10::int4),(2,20::int4)) t(id,x)', [23], [['99'], ['10']]),
    ('t16-31', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)', [23], [['20'], ['20'], ['20']]),
    ('t16-32', 'SELECT last_value(x) OVER(PARTITION BY p ORDER BY x GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(1,10::int4),(1,10::int4),(1,20::int4),(2,30::int4)) t(p,x)', [23], [['20'], ['20'], ['20'], ['30']]),
    ('t16-33', 'SELECT last_value(y) OVER(ORDER BY x DESC GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) AS r FROM (VALUES(10::int4,1::int4),(10::int4,2::int4),(20::int4,3::int4)) t(x,y) ORDER BY y', [23], [['2'], ['2'], ['3']]),
    ('t16-34', 'SELECT last_value(y) OVER(ORDER BY x,z GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(10::int4,1,1::int4),(10::int4,1,2::int4),(10::int4,2,3::int4),(20::int4,1,4::int4)) t(x,z,y)', [23], [['3'], ['3'], ['4'], ['4']]),
    ('t16-35', 'SELECT last_value(y) OVER(ORDER BY x NULLS FIRST GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(NULL::int4,1::int4),(NULL::int4,2::int4),(10::int4,3::int4)) t(x,y)', [23], [['3'], ['3'], ['3']]),
    ('t16-36', 'SELECT last_value(y) OVER(ORDER BY x NULLS LAST GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(10::int4,1::int4),(NULL::int4,2::int4),(NULL::int4,3::int4)) t(x,y)', [23], [['3'], ['3'], ['3']]),
    ('t16-37', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND 1 FOLLOWING) AS r FROM (VALUES(NULL::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t16-38', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN 1 FOLLOWING AND 1 FOLLOWING) AS r FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)', [23], [['20'], ['20'], [None]]),
    ('t16-39', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN 2 PRECEDING AND 1 PRECEDING) AS r FROM (VALUES(10::int4),(10::int4),(20::int4)) t(x)', [23], [[None], [None], ['10']]),
    ('t16-40', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN 2 FOLLOWING AND 1 FOLLOWING) AS r FROM (VALUES(10::int4),(20::int4),(30::int4)) t(x)', [23], [[None], [None], [None]]),
    ('t16-41', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND 9223372036854775807 FOLLOWING) AS r FROM (VALUES(10::int4),(20::int4)) t(x)', [23], [['20'], ['20']]),
    ('t16-42', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS r FROM (VALUES(10::int4)) t(x)', [23], [['10']]),
    ('t16-43', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND CURRENT ROW) AS r FROM (SELECT 1::int4 AS x WHERE false) t', [23], []),
    ('t17-44', 'SELECT last_value(x) OVER(ORDER BY id ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE CURRENT ROW) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [[None], ['10'], ['10']]),
    ('t17-45', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE CURRENT ROW) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-46', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-47', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-48', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE NO OTHERS) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-49', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE CURRENT ROW) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-50', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-51', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-52', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE NO OTHERS) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-53', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE CURRENT ROW) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-54', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['10']]),
    ('t17-55', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-56', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE NO OTHERS) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['20'], ['20'], ['20']]),
    ('t17-57', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['10'], ['10'], ['20']]),
    ('t17-58', 'SELECT last_value(x) OVER(ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [[None], [None], [None]]),
    ('t17-59', 'SELECT last_value(x) OVER(ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [['10'], ['10'], ['20']]),
    ('t17-60', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING EXCLUDE TIES) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [[None], ['20'], [None]]),
    ('t17-61', 'SELECT last_value(x) OVER(ORDER BY x RANGE BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(2,10::int4),(3,20::int4)) t(id,x)', [23], [[None], [None], [None]]),
    ('t17-62', 'SELECT last_value(x) OVER(ORDER BY x GROUPS BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE CURRENT ROW) AS r FROM (VALUES(NULL::int4),(NULL::int4)) t(x)', [23], [[None], [None]]),
    ('t17-63', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE CURRENT ROW) AS r FROM (SELECT 1::int4 AS x WHERE false) t', [23], []),
    ('t17-64', 'SELECT last_value(x) OVER(ORDER BY x ROWS BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE CURRENT ROW) AS r FROM (VALUES(10::int4)) t(x)', [23], [[None]]),
    ('t17-65', 'SELECT last_value(x) OVER(PARTITION BY p ORDER BY x DESC GROUPS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE GROUP) AS r FROM (VALUES(1,10::int4),(1,20::int4),(2,30::int4)) t(p,x) ORDER BY p,x', [23], [['20'], [None], [None]]),
    ('t17-66', 'SELECT last_value(x) OVER(ORDER BY k NULLS FIRST, id RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE GROUP) AS r FROM (VALUES(NULL::int4,1,10::int4),(NULL::int4,1,20::int4),(1,2,30::int4)) t(k,id,x)', [23], [[None], [None], ['20']]),
    ('t17-67', 'SELECT last_value(x) OVER(ORDER BY k RANGE BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE CURRENT ROW) AS r FROM (VALUES(10,1::int4),(10,2::int4),(20,3::int4)) t(k,x)', [23], [['2'], ['1'], [None]]),
    ('t17-68', 'SELECT last_value(x) OVER(ORDER BY k RANGE BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE GROUP) AS r FROM (VALUES(10,1::int4),(10,2::int4),(20,3::int4)) t(k,x)', [23], [[None], [None], [None]]),
    ('t17-69', 'SELECT last_value(x) OVER(ORDER BY k RANGE BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE TIES) AS r FROM (VALUES(10,1::int4),(10,2::int4),(20,3::int4)) t(k,x)', [23], [['1'], ['2'], ['3']]),
    ('t17-70', 'SELECT last_value(x) OVER(ORDER BY k RANGE BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE NO OTHERS) AS r FROM (VALUES(10,1::int4),(10,2::int4),(20,3::int4)) t(k,x)', [23], [['2'], ['2'], ['3']]),
    ('t17-71', 'SELECT last_value(x) OVER(ORDER BY k RANGE BETWEEN 0 PRECEDING AND 0 FOLLOWING EXCLUDE CURRENT ROW) AS r FROM (VALUES(1.0::numeric,1::int4),(1.00::numeric,2::int4),(2.0::numeric,3::int4)) t(k,x)', [23], [['2'], ['1'], [None]]),
    ('ranking', 'SELECT row_number() OVER(PARTITION BY p ORDER BY x) AS n,rank() OVER(PARTITION BY p ORDER BY x) AS r FROM (VALUES(1,10::int4),(1,10::int4),(1,20::int4),(2,NULL::int4)) t(p,x) ORDER BY p,x', [20, 20], [['1', '1'], ['2', '1'], ['3', '3'], ['1', '1']]),
    ('five-single', 'SELECT row_number() OVER() AS n,rank() OVER() AS r,lag(x) OVER() AS l,lead(x) OVER() AS d,last_value(x) OVER() AS v FROM (VALUES(7::int4)) t(x)', [20, 20, 23, 23, 23], [['1', '1', None, None, '7']]),
    ('five-null', 'SELECT row_number() OVER() AS n,rank() OVER() AS r,lag(x) OVER() AS l,lead(x) OVER() AS d,last_value(x) OVER() AS v FROM (VALUES(NULL::int4)) t(x)', [20, 20, 23, 23, 23], [['1', '1', None, None, None]]),
    ('five-empty', 'SELECT row_number() OVER() AS n,rank() OVER() AS r,lag(x) OVER() AS l,lead(x) OVER() AS d,last_value(x) OVER() AS v FROM (SELECT 7::int4 AS x WHERE false) t', [20, 20, 23, 23, 23], []),
 ]
ERRORS = [
 ('ignore-nulls','SELECT lag(x) IGNORE NULLS OVER() AS v FROM (VALUES(1::int4)) t(x)','42601'),
 ('missing-over','SELECT row_number() AS v','42809'),
 ('invalid-frame','SELECT last_value(x) OVER(ROWS BETWEEN CURRENT ROW AND 1 PRECEDING) AS v FROM (VALUES(1::int4)) t(x)','42P20'),
 ('negative-offset','SELECT last_value(x) OVER(ROWS BETWEEN -1 PRECEDING AND CURRENT ROW) AS v FROM (VALUES(1::int4)) t(x)','22013'),
 ('null-offset','SELECT last_value(x) OVER(ROWS BETWEEN NULL PRECEDING AND CURRENT ROW) AS v FROM (VALUES(1::int4)) t(x)','22004'),
 ('lag-overload','SELECT lag(x,true) OVER() AS v FROM (VALUES(1::int4)) t(x)','42883'),
]
WIDTH={20:8,23:4}
TOTAL=3+len(CASES)*4+2+24+len(ERRORS)*3

def run(port,database,evidence,mysql_port):
    evidence.mkdir(parents=True,exist_ok=False)
    assert database.replace('_','').isalnum()
    pq=Libpq(); tx=pq.lib.PQtransactionStatus; tx.restype=c.c_int; tx.argtypes=[c.c_void_p]
    conns={}; wires={}; admin=None; mysql=None; owned=False
    failures={}; passed=[]; expected=[]; cleanup={}
    def save(side,case,phase,obs):
        with (evidence/(side+'.jsonl')).open('a') as f:
            f.write(json.dumps(dict(case_id=case,phase=phase,observation=obs))+'\n')
        return obs
    def attempt(case,body):
        expected.append(case)
        try: body(); passed.append(case)
        except (AssertionError,ValueError,KeyError) as e: failures[case]=str(e)
    def query(conn,sql): return pq.observe(pq.PQexec(conn,sql.encode()))
    def metadata(obs,oids,fmt=0):
        assert [x[:4] for x in obs['metadata']]==[[o,WIDTH[o],-1,fmt] for o in oids],obs
    def check(obs,oids,rows,fmt=0):
        assert obs['status']==2,obs
        metadata(obs,oids,fmt)
        assert obs['rows']==[[None if v is None else (struct.pack('!q' if oid==20 else '!i',int(v)) if fmt else v.encode()).hex() for oid,v in zip(oids,row)] for row in rows],obs
    def interleave(label):
        for side,conn in conns.items():
            obs=save(side,label,'recovery',query(conn,'SELECT 1::int4 AS v'))
            check(obs,[23],[['1']])
        sql="SELECT row_number() OVER(ORDER BY id) AS n,rank() OVER(ORDER BY id) AS r,lag(x) OVER(ORDER BY id) AS l,lead(x) OVER(ORDER BY id) AS d,last_value(x) OVER(ORDER BY id) AS v FROM (SELECT 1 AS id,7 AS x UNION ALL SELECT 2,NULL) t ORDER BY id"
        obs=save('mysql',label,'query',mysql.query(sql))
        assert obs['code']==0 and obs['rows']==[['31','31',None,None,'37'],['32','32','37',None,None]],obs
        assert [m[0] for m in obs['metadata']]==['n','r','l','d','v'],obs
        prepared=save('mysql',label,'prepare-execute',mysql.prepared('SELECT last_value(x) OVER() AS v FROM (SELECT CAST(? AS SIGNED) AS x) t',[b'7',None,b'9']))
        assert prepared[0]['code']==0 and prepared[0]['parameters']==1,prepared
        assert [x['rows'] for x in prepared[1:]]==[[['37']],[[None]],[['39']]],prepared
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference login failed (DSN redacted)'
        identity={s:query(admin,s) for s in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW TimeZone','SHOW extra_float_digits','SHOW lc_numeric')}
        save('reference','identity','setup',identity)
        assert identity['SHOW server_version_num']['rows']==[['180006'.encode().hex()]],'PG18.6 required; other versions are separate evidence'
        binary=Path(os.environ['PG09_REFERENCE_BIN'])/'postgres'
        save('build','identity','setup',dict(reference_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),reference_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG09_18_RUST_BINARY']).read_bytes()).hexdigest()))
        assert query(admin,f'CREATE DATABASE {database}')['status']==1; owned=True
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('A',f'host=127.0.0.1 port={port} user=root dbname={database}'),('B',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
            conn=pq.PQconnectdb(dsn.encode()); assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)'
            conns[side]=conn; wires[side]=Wire(pq,dsn)
            save(side,'identity','setup',{sql:query(conn,sql) for sql in identity})
        mysql=Mysql('127.0.0.1',mysql_port,'root','',database)
        save('mysql','identity','setup',dict(client_version=mysql.mysql_get_client_version(),settings={sql:mysql.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection')}))
        attempt('interleave-before',lambda:interleave('before'))
        for label,sql,oids,rows in CASES:
            def normal():
                peers={side:save(side,label,'Q',query(conn,sql)) for side,conn in conns.items()}
                for obs in peers.values():check(obs,oids,rows)
                compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                raw={side:save(side,label,'raw-Q',w.query(sql)) for side,w in wires.items()}
                for side,obs in raw.items():
                    assert not obs['errors'] and obs['ready']=='I' and obs['rows']==peers[side]['rows'],obs
                    metadata(obs,oids)
                compare_wire(raw['reference'],raw['A']);compare_wire(raw['reference'],raw['B'])
            attempt('Q-'+label,normal)
        attempt('interleave-middle',lambda:interleave('middle'))
        for label,sql,oids,rows in CASES:
            name=('win_'+label).encode()
            def prepare():
                for side,conn in conns.items():
                    assert save(side,label,'libpq-Parse',pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))['status']==1
                    desc=save(side,label,'statement-describe',pq.observe(pq.PQdescribePrepared(conn,name)))
                    metadata(desc,oids)
                    assert desc['parameters']==[],desc
                for side,w in wires.items():
                    assert save(side,label,'Parse',w.phase(b'P',name+b'\0'+sql.encode()+b'\0\0\0','1'))['tags']=='1'
                    desc=save(side,label,'raw-statement-describe',w.phase(b'D',b'S'+name+b'\0','Tn'))
                    metadata(desc,oids)
                    assert desc['parameters']==[],desc
            attempt('prepare-'+label,prepare)
            for fmt in (0,1):
                def execute():
                    peers={}; raw={}
                    for side,conn in conns.items():
                        obs=save(side,label,'libpq-Execute-'+str(fmt),pq.observe(pq.PQexecPrepared(conn,name,0,None,None,None,fmt)))
                        check(obs,oids,rows,fmt);peers[side]=obs
                    compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                    for side,w in wires.items():
                        assert save(side,label,'Bind',w.phase(b'B',b'\0'+name+b'\0'+struct.pack('!HHHH',0,0,1,fmt),'2'))['tags']=='2'
                        metadata(save(side,label,'portal-describe',w.phase(b'D',b'P\0','Tn')),oids,fmt)
                        w.send(b'E',b'\0\0\0\0\0');w.send(b'S');obs=save(side,label,'Execute-Sync-'+str(fmt),w.read('Z'));raw[side]=obs
                        assert obs['ready']=='I' and obs['tags']=='D'*len(rows)+'CZ' and not obs['metadata'],obs
                        assert obs['rows']==peers[side]['rows'],obs
                        assert obs['lengths']==[[-1 if v is None else len(bytes.fromhex(v)) for v in row] for row in obs['rows']],obs
                    compare_wire(raw['reference'],raw['A']);compare_wire(raw['reference'],raw['B'])
                attempt('execute-'+label+'-'+str(fmt),execute)
        # Same descriptor through text/binary parameter and result formats, with
        # value -> NULL -> value rebinding, including empty input.
        for zero in (False,True):
            sql='SELECT row_number() OVER() AS n,rank() OVER() AS r,lag(x,0) OVER() AS l,lead(x,0) OVER() AS d,last_value(x) OVER() AS v FROM (SELECT $1::int4 AS x'+(' WHERE false' if zero else '')+') t'
            name=('param_'+str(zero)).encode();oids=[20,20,23,23,23]
            def prepare_param():
                for side,conn in conns.items():
                    assert pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None))['status']==1
                    desc=save(side,'params','statement-describe',pq.observe(pq.PQdescribePrepared(conn,name)))
                    assert desc['parameters']==[23],desc
                    metadata(desc,oids)
                for side,w in wires.items():
                    assert w.phase(b'P',name+b'\0'+sql.encode()+b'\0\0\0','1')['tags']=='1'
                    desc=save(side,'params','raw-statement-describe',w.phase(b'D',b'S'+name+b'\0','Tn'))
                    assert desc['parameters']==[23],desc
                    metadata(desc,oids)
            attempt('param-prepare-'+str(zero),prepare_param)
            for infmt in (0,1):
              for outfmt in (0,1):
               for value in (7,None,-9):
                label=f'bind-{zero}-{infmt}-{outfmt}-{value}'
                def bind():
                    encoded=None if value is None else struct.pack('!i',value) if infmt else str(value).encode()
                    expected_rows=[] if zero else [['1','1']+[None if value is None else str(value)]*3]
                    peers={};raw={}
                    for side,conn in conns.items():
                        obs=save(side,label,'libpq-bind-execute',pq.observe(pq.PQexecPrepared(conn,name,1,(c.c_char_p*1)(encoded),(c.c_int*1)(len(encoded) if encoded else 0),(c.c_int*1)(infmt),outfmt)))
                        check(obs,oids,expected_rows,outfmt);peers[side]=obs
                    compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                    for side,w in wires.items():
                        body=b'\0'+name+b'\0'+struct.pack('!HHH',1,infmt,1)+struct.pack('!i',-1 if encoded is None else len(encoded))+(encoded or b'')+struct.pack('!HH',1,outfmt)
                        assert save(side,label,'Bind',w.phase(b'B',body,'2'))['tags']=='2'
                        metadata(save(side,label,'portal-describe',w.phase(b'D',b'P\0','Tn')),oids,outfmt)
                        w.send(b'E',b'\0\0\0\0\0');w.send(b'S');obs=save(side,label,'Execute-Sync',w.read('Z'));raw[side]=obs
                        assert obs['ready']=='I' and obs['tags']==('CZ' if zero else 'DCZ') and not obs['metadata'],obs
                        assert obs['rows']==peers[side]['rows'],obs
                        if not zero and value is None:assert obs['lengths'][0][2:]==[-1,-1,-1],obs
                    compare_wire(raw['reference'],raw['A']);compare_wire(raw['reference'],raw['B'])
                attempt(label,bind)
        for label,sql,state in ERRORS:
          for phase in ('Q','extended','transaction'):
            def error():
                clients={}
                for side,conn in conns.items():
                    if phase=='transaction':assert query(conn,'BEGIN')['status']==1
                    if phase=='extended':
                        statement=('libpq_bad_'+label).encode()
                        obs=save(side,label,'libpq-Parse',pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None)))
                        if obs['status']==1:
                            obs=save(side,label,'libpq-Bind-Execute',pq.observe(pq.PQexecPrepared(conn,statement,0,None,None,None,0)))
                    else:obs=save(side,label,'libpq-'+phase,query(conn,sql))
                    assert obs['status']==7 and obs['state']==state,obs
                    assert not obs['rows'] and not obs['metadata'],obs
                    clients[side]={k:obs['errors'].get(k,'') for k in 'SVCMDHP'}
                    assert tx(conn)==(3 if phase=='transaction' else 0)
                    if phase=='transaction':assert query(conn,'ROLLBACK')['status']==1
                    recovery=save(side,label,'libpq-recovery',query(conn,'SELECT 1::int4 AS v'))
                    check(recovery,[23],[['1']]);assert tx(conn)==0
                assert clients['reference']==clients['A']==clients['B'],clients
                peers={}
                for side,w in wires.items():
                    if phase=='transaction':assert w.query('BEGIN')['ready']=='T'
                    if phase=='extended':
                        statement=('bad_'+label).encode()
                        stage='Parse';obs=w.phase(b'P',statement+b'\0'+sql.encode()+b'\0\0\0','1')
                        if not obs['errors']:
                            stage='Bind';obs=w.phase(b'B',b'\0'+statement+b'\0'+struct.pack('!HHH',0,0,0),'2')
                        if not obs['errors']:stage='Execute';obs=w.phase(b'E',b'\0\0\0\0\0','C')
                        save(side,label,stage,obs);ready=save(side,label,'Sync',w.sync())
                    else:
                        stage='Q';obs=save(side,label,phase,w.query(sql));ready=obs.get('tail',obs)
                    assert obs['errors'] and obs['errors'][0]['C']==state,obs
                    assert all(obs['errors'][0].get(f) for f in 'SVCM'),obs
                    peers[side]=(stage,{k:obs['errors'][0].get(k,'') for k in 'SVCMDHP'})
                    assert ready['ready']==('E' if phase=='transaction' else 'I'),ready
                    if phase=='transaction':assert w.query('ROLLBACK')['ready']=='I'
                    recovery=save(side,label,'recovery',w.query('SELECT 1::int4 AS v'))
                    assert recovery['ready']=='I' and recovery['rows']==[['31']],recovery
                assert peers['reference']==peers['A']==peers['B'],peers
            attempt('error-'+label+'-'+phase,error)
            for conn in conns.values():query(conn,'ROLLBACK')
            for w in wires.values():
                # Restore failed assertions as well, before the next scenario.
                w.query('ROLLBACK')
        attempt('interleave-after',lambda:interleave('after'))
    finally:
        for w in wires.values():w.close()
        for conn in conns.values():pq.PQfinish(conn)
        if mysql:mysql.close()
        cleanup['connections_closed']=True
        if owned:cleanup['reference_database_dropped']=query(admin,f'DROP DATABASE {database}')['status']==1
        if admin:pq.PQfinish(admin)
        summary=dict(expected=TOTAL,attempted=len(expected),unique=len(set(expected)),passed=len(passed),failed=len(failures),failures=failures,negative_self_checks=3,**cleanup)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
        print(json.dumps(summary),flush=True)
    assert len(expected)==len(set(expected))==TOTAL,summary
    assert not failures,failures


if __name__=='__main__':
    assert self_test()==3
    if sys.argv[1:] == ['--self-test']:print('negative_self_checks=3')
    else:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
