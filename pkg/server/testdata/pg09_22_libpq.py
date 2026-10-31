# Copyright 2026 AsterSQL.
"""Ordered-set libpq/raw TCP differential. Reuses existing client implementations.

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

CASES = [('disc-median',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [['2']]),
 ('disc-lower',
  'SELECT percentile_disc(0) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [['1']]),
 ('disc-upper',
  'SELECT percentile_disc(1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [['4']]),
 ('disc-desc',
  'SELECT percentile_disc(0.25) WITHIN GROUP(ORDER BY x DESC) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [['4']]),
 ('disc-null-fraction',
  'SELECT percentile_disc(NULL::float8) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [[None]]),
 ('disc-empty',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 AS x WHERE '
  'false) t',
  [23],
  [[None]]),
 ('disc-all-null',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(NULL::int4),(NULL::int4)) t(x)',
  [23],
  [[None]]),
 ('disc-mixed',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(NULL::int4),(1::int4),(3::int4)) t(x)',
  [23],
  [['1']]),
 ('disc-duplicate',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(1::int4),(4::int4)) t(x)',
  [23],
  [['1']]),
 ('disc-single',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(7::int4)) t(x)',
  [23],
  [['7']]),
 ('disc-filter-empty',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) FILTER(WHERE false) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [[None]]),
 ('disc-filter',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) FILTER(WHERE x>1) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  [23],
  [['3']]),
 ('disc-zero-groups',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 AS x WHERE '
  'false) t GROUP BY x',
  [23],
  []),
 ('cont-median',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [['2.5']]),
 ('cont-lower',
  'SELECT percentile_cont(0) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [['1']]),
 ('cont-upper',
  'SELECT percentile_cont(1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [['4']]),
 ('cont-desc',
  'SELECT percentile_cont(0.25) WITHIN GROUP(ORDER BY x DESC) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [['3.25']]),
 ('cont-null-fraction',
  'SELECT percentile_cont(NULL::float8) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [[None]]),
 ('cont-empty',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 AS x WHERE '
  'false) t',
  [701],
  [[None]]),
 ('cont-all-null',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(NULL::float8),(NULL::float8)) t(x)',
  [701],
  [[None]]),
 ('cont-mixed',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(NULL::float8),(1::float8),(3::float8)) t(x)',
  [701],
  [['2']]),
 ('cont-duplicate',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(1::float8),(4::float8)) t(x)',
  [701],
  [['1']]),
 ('cont-single',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(7::float8)) t(x)',
  [701],
  [['7']]),
 ('cont-filter-empty',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) FILTER(WHERE false) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [[None]]),
 ('cont-filter',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) FILTER(WHERE x>1) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  [701],
  [['3']]),
 ('cont-zero-groups',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 AS x WHERE '
  'false) t GROUP BY x',
  [701],
  [])]
ERRORS = [('disc-below-normal',
  'SELECT percentile_disc(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '22003'),
 ('disc-below-empty',
  'SELECT percentile_disc(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 AS x WHERE '
  'false) t',
  '22003'),
 ('disc-below-null',
  'SELECT percentile_disc(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::int4)) t(x)',
  '22003'),
 ('disc-above-normal',
  'SELECT percentile_disc(1.1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '22003'),
 ('disc-above-empty',
  'SELECT percentile_disc(1.1) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 AS x WHERE '
  'false) t',
  '22003'),
 ('disc-above-null',
  'SELECT percentile_disc(1.1) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::int4)) t(x)',
  '22003'),
 ('disc-nan-normal',
  "SELECT percentile_disc('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '22003'),
 ('disc-nan-empty',
  "SELECT percentile_disc('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 AS x "
  'WHERE false) t',
  '22003'),
 ('disc-nan-null',
  "SELECT percentile_disc('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::int4)) "
  't(x)',
  '22003'),
 ('disc-infinity-normal',
  "SELECT percentile_disc('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '22003'),
 ('disc-infinity-empty',
  "SELECT percentile_disc('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 "
  'AS x WHERE false) t',
  '22003'),
 ('disc-infinity-null',
  "SELECT percentile_disc('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(NULL::int4)) t(x)',
  '22003'),
 ('disc-negative-infinity-normal',
  "SELECT percentile_disc('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '22003'),
 ('disc-negative-infinity-empty',
  "SELECT percentile_disc('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::int4 "
  'AS x WHERE false) t',
  '22003'),
 ('disc-negative-infinity-null',
  "SELECT percentile_disc('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(NULL::int4)) t(x)',
  '22003'),
 ('disc-missing-within', 'SELECT percentile_disc(0.5) AS v', '42883'),
 ('disc-distinct',
  'SELECT percentile_disc(DISTINCT 0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '42601'),
 ('disc-over',
  'SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) OVER() AS v FROM '
  '(VALUES(1::int4),(2::int4),(3::int4),(4::int4)) t(x)',
  '0A000'),
 ('disc-ungrouped',
  'SELECT percentile_disc(p) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(0.0,1::int4),(1.0,2::int4)) t(p,x)',
  '42803'),
 ('cont-below-normal',
  'SELECT percentile_cont(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '22003'),
 ('cont-below-empty',
  'SELECT percentile_cont(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 AS x WHERE '
  'false) t',
  '22003'),
 ('cont-below-null',
  'SELECT percentile_cont(-0.1) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::float8)) t(x)',
  '22003'),
 ('cont-above-normal',
  'SELECT percentile_cont(1.1) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '22003'),
 ('cont-above-empty',
  'SELECT percentile_cont(1.1) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 AS x WHERE '
  'false) t',
  '22003'),
 ('cont-above-null',
  'SELECT percentile_cont(1.1) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::float8)) t(x)',
  '22003'),
 ('cont-nan-normal',
  "SELECT percentile_cont('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '22003'),
 ('cont-nan-empty',
  "SELECT percentile_cont('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 AS x "
  'WHERE false) t',
  '22003'),
 ('cont-nan-null',
  "SELECT percentile_cont('NaN'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(NULL::float8)) "
  't(x)',
  '22003'),
 ('cont-infinity-normal',
  "SELECT percentile_cont('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '22003'),
 ('cont-infinity-empty',
  "SELECT percentile_cont('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1::float8 "
  'AS x WHERE false) t',
  '22003'),
 ('cont-infinity-null',
  "SELECT percentile_cont('Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(NULL::float8)) t(x)',
  '22003'),
 ('cont-negative-infinity-normal',
  "SELECT percentile_cont('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '22003'),
 ('cont-negative-infinity-empty',
  "SELECT percentile_cont('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT "
  '1::float8 AS x WHERE false) t',
  '22003'),
 ('cont-negative-infinity-null',
  "SELECT percentile_cont('-Infinity'::float8) WITHIN GROUP(ORDER BY x) AS v FROM "
  '(VALUES(NULL::float8)) t(x)',
  '22003'),
 ('cont-missing-within', 'SELECT percentile_cont(0.5) AS v', '42883'),
 ('cont-distinct',
  'SELECT percentile_cont(DISTINCT 0.5) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '42601'),
 ('cont-over',
  'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) OVER() AS v FROM '
  '(VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)',
  '0A000'),
 ('cont-ungrouped',
  'SELECT percentile_cont(p) WITHIN GROUP(ORDER BY x) AS v FROM '
  '(VALUES(0.0,1::float8),(1.0,2::float8)) t(p,x)',
  '42803')]
# Remaining source19/20 fixtures: int4 limits, grouped direct parameters,
# descending interpolation, and non-finite ordered values.
CASES += [
    ('disc-limits', "SELECT percentile_disc(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES('-2147483648'::int4),(2147483647::int4)) t(x)", [23], [['-2147483648']]),
    ('cont-desc-median', 'SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x DESC) AS v FROM (VALUES(1::float8),(2::float8),(3::float8),(4::float8)) t(x)', [701], [['2.5']]),
    ('cont-infinite-endpoint', "SELECT percentile_cont(1) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(1::float8),('Infinity'::float8)) t(x)", [701], [['Infinity']]),
    ('cont-infinite-interpolation', "SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(1::float8),('Infinity'::float8)) t(x)", [701], [['Infinity']]),
    ('cont-nan-value', "SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES('NaN'::float8)) t(x)", [701], [['NaN']]),
]
for fn, ty, oid in (('disc','int4',23),('cont','float8',701)):
    CASES.append((fn+'-grouped-direct', f'SELECT percentile_{fn}(p::float8) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(0::float8,1::{ty}),(0::float8,9::{ty}),(1::float8,2::{ty}),(1::float8,8::{ty})) t(p,x) GROUP BY p ORDER BY p', [oid], [['1'],['8']]))
for direction,result in (('ASC','Infinity'),('DESC','-Infinity')):
    CASES.append(('cont-finite-extremes-'+direction, f"SELECT percentile_cont(0.5) WITHIN GROUP(ORDER BY x {direction}) AS v FROM (VALUES('-1.7976931348623157e308'::float8),('1.7976931348623157e308'::float8)) t(x)", [701], [[result]]))
for fn,ty in (('disc','int4'),('cont','float8')):
    for label,p in (('below','-0.1'),('above','1.1'),('nan',"'NaN'::float8"),('infinity',"'Infinity'::float8"),('negative-infinity',"'-Infinity'::float8")):
        ERRORS.append((fn+'-'+label+'-filter-empty', f'SELECT percentile_{fn}({p}) WITHIN GROUP(ORDER BY x) FILTER(WHERE false) AS v FROM (VALUES(1::{ty})) t(x)', '22003'))
for fn,ty,oid,result in (('disc','int4',23,'1'),('cont','float8',701,'1.5')):
    CASES.append((fn+'-filter-sibling-count', f'SELECT percentile_{fn}(0.5) WITHIN GROUP(ORDER BY x) FILTER(WHERE x<=2) AS v, count(*) AS n FROM (VALUES(1::{ty}),(2::{ty}),(9::{ty})) t(x)', [oid,20], [[result,'3']]))
for fn,ty in (('disc','int4'),('cont','float8')):
    ERRORS.append((fn+'-cte-original-position', f"WITH q(p,x) AS (VALUES(0.0,1::{ty}),(1.0,2::{ty})) SELECT 'é' AS padding, percentile_{fn}(p) WITHIN GROUP(ORDER BY x) AS v FROM q t", '42803'))
for fn,ty in (('disc','int4'),('cont','float8')):
    for label,p in (('fraction-precision','-0.123456789'),('fraction-small','-1e-10'),('fraction-large','1e10')):
        ERRORS.append((fn+'-'+label, f'SELECT percentile_{fn}({p}::float8) WITHIN GROUP(ORDER BY x) AS v FROM (VALUES(1::{ty})) t(x)', '22003'))
BIND_VALUES=(0.5,None,1.0,0.0,-0.1,1.1,float('nan'),float('inf'),float('-inf'),None,0.5)
WIDTH={20:8,23:4,701:8}
TOTAL=3+len(CASES)*4+6+24*len(BIND_VALUES)+len(ERRORS)*3

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
        assert obs['rows']==[[None if v is None else ((struct.pack('!d',float(v)) if oid==701 else struct.pack('!q' if oid==20 else '!i',int(v))) if fmt else v.encode()).hex() for oid,v in zip(oids,row)] for row in rows],obs
    def interleave(label):
        for side,conn in conns.items():
            obs=save(side,label,'recovery',query(conn,'SELECT 1::int4 AS v'))
            check(obs,[23],[['1']])
        for fn in ('disc','cont'):
            sql=f'SELECT percentile_{fn}(0.5) WITHIN GROUP(ORDER BY x) AS v FROM (SELECT 1 AS x) t'
            obs=save('mysql',label,fn+'-reject',mysql.query(sql))
            assert obs['code']==1064 and obs['state']=='42000',obs
            prepared=save('mysql',label,fn+'-prepare-reject',mysql.prepared(sql,[()]))
            assert prepared[0]['code']==1064 and prepared[0]['state']=='42000',prepared
        obs=save('mysql',label,'native-query',mysql.query('SELECT CAST(7 AS SIGNED) AS v'))
        assert obs['code']==0 and obs['rows']==[['37']] and obs['metadata']==[['v',253,0,255,1024,0]],obs
        prepared=save('mysql',label,'native-prepared',mysql.prepared('SELECT CAST(? AS SIGNED) AS v',[b'7',None,b'9']))
        assert prepared[0]['code']==0 and prepared[0]['parameters']==1 and prepared[0]['metadata']==[['v',253,0,46,5,31]],prepared
        assert [x['rows'] for x in prepared[1:]]==[[['37']],[[None]],[['39']]],prepared
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference login failed (DSN redacted)'
        identity={s:query(admin,s) for s in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW TimeZone','SHOW extra_float_digits','SHOW lc_numeric')}
        save('reference','identity','setup',identity)
        assert identity['SHOW server_version_num']['rows']==[['180006'.encode().hex()]],'PG18.6 required; other versions are separate evidence'
        binary=Path(os.environ['PG09_REFERENCE_BIN'])/'postgres'
        save('build','identity','setup',dict(reference_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),reference_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG09_22_RUST_BINARY']).read_bytes()).hexdigest()))
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
            name=('ordered_'+label).encode()
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
        for fn, ty, oid in (('disc', 'int4', 23), ('cont', 'float8', 701)):
            for shape in ('rows', 'empty', 'zero-groups'):
                zero = shape != 'rows'
                zero_groups = shape == 'zero-groups'
                source = (f'(SELECT 1::{ty} AS x WHERE false) t' if zero else
                          f'(VALUES(1::{ty}),(2::{ty}),(3::{ty}),(4::{ty})) t(x)')
                sql = f'SELECT percentile_{fn}($1::float8) WITHIN GROUP(ORDER BY x) AS v FROM {source}' + (' GROUP BY x' if zero_groups else '')
                name = ('param_' + fn + shape).encode()
                oids = [oid]
                def prepare_param():
                    for side, conn in conns.items():
                        assert pq.observe(pq.PQprepare(conn, name, sql.encode(), 0, None))['status'] == 1
                        desc = save(side, 'params', 'statement-describe', pq.observe(pq.PQdescribePrepared(conn, name)))
                        assert desc['parameters'] == [701], desc
                        metadata(desc, oids)
                    for side, w in wires.items():
                        assert w.phase(b'P', name + b'\0' + sql.encode() + b'\0\0\0', '1')['tags'] == '1'
                        desc = save(side, 'params', 'raw-statement-describe', w.phase(b'D', b'S' + name + b'\0', 'Tn'))
                        assert desc['parameters'] == [701], desc
                        metadata(desc, oids)
                attempt('param-prepare-' + fn + shape, prepare_param)
                for infmt in (0, 1):
                    for outfmt in (0, 1):
                        for iteration, value in enumerate(BIND_VALUES):
                            label = f'bind-{fn}-{shape}-{infmt}-{outfmt}-{iteration}'
                            def bind():
                                invalid = not zero_groups and value is not None and not 0 <= value <= 1
                                encoded = (None if value is None else struct.pack('!d', value)
                                           if infmt else str(value).encode())
                                result = (None if zero or value is None else '4' if value == 1
                                          else '1' if value == 0 else '2' if fn == 'disc' else '2.5')
                                peers = {}
                                raw = {}
                                for side, conn in conns.items():
                                    obs = save(side, label, 'libpq-bind-execute', pq.observe(pq.PQexecPrepared(
                                        conn, name, 1, (c.c_char_p * 1)(encoded),
                                        (c.c_int * 1)(len(encoded) if encoded else 0),
                                        (c.c_int * 1)(infmt), outfmt)))
                                    if invalid:
                                        assert obs['status'] == 7 and obs['state'] == '22003', obs
                                        assert not obs['rows'] and not obs['metadata'], obs
                                        assert tx(conn) == 0
                                        check(save(side, label, 'libpq-recovery', query(conn, 'SELECT 1::int4 AS v')), [23], [['1']])
                                    else:
                                        check(obs, oids, [] if zero_groups else [[result]], outfmt)
                                    peers[side] = obs
                                compare(peers['reference'], peers['A'])
                                compare(peers['reference'], peers['B'])
                                if invalid:
                                    assert all({k:o['errors'].get(k, '') for k in 'SVCMDHP'} ==
                                               {k:peers['reference']['errors'].get(k, '') for k in 'SVCMDHP'}
                                               for o in peers.values()), peers
                                for side, w in wires.items():
                                    body = (b'\0' + name + b'\0' + struct.pack('!HHH', 1, infmt, 1) +
                                            struct.pack('!i', -1 if encoded is None else len(encoded)) +
                                            (encoded or b'') + struct.pack('!HH', 1, outfmt))
                                    assert save(side, label, 'Bind', w.phase(b'B', body, '2'))['tags'] == '2'
                                    metadata(save(side, label, 'portal-describe', w.phase(b'D', b'P\0', 'Tn')), oids, outfmt)
                                    w.send(b'E', b'\0\0\0\0\0')
                                    w.send(b'S')
                                    obs = save(side, label, 'Execute-Sync', w.read('Z'))
                                    if obs['errors']:
                                        tail = save(side, label, 'Sync-ready', w.read('Z'))
                                        obs['tags'] += tail['tags']
                                        obs['ready'] = tail['ready']
                                    raw[side] = obs
                                    assert obs['ready'] == 'I' and obs['tags'] == ('EZ' if invalid else 'CZ' if zero_groups else 'DCZ') and not obs['metadata'], obs
                                    assert obs['rows'] == peers[side]['rows'], obs
                                    if invalid:
                                        assert obs['errors'] and {k:obs['errors'][0].get(k, '') for k in 'SVCMDHP'} == {k:peers[side]['errors'].get(k, '') for k in 'SVCMDHP'}, obs
                                        assert save(side, label, 'raw-recovery', w.query('SELECT 1::int4 AS v'))['rows'] == [['31']]
                                    elif not zero_groups and (zero or value is None):
                                        assert obs['lengths'] == [[-1]], obs
                                compare_wire(raw['reference'], raw['A'])
                                compare_wire(raw['reference'], raw['B'])
                            attempt(label, bind)
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
                assert clients['reference']==clients['A']==clients['B'],clients
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
