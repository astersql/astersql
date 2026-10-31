# Copyright 2026 AsterSQL.
"""Ordinary aggregate differential, using existing native client/wire helpers.

PORT DATABASE EVIDENCE MYSQL_PORT; --self-test needs no database.
PG18_REFERENCE_DSN and PG_LIBPQ_LIBRARY select the independent reference/client.
PG09_REFERENCE_BIN provides the reference executable directory for provenance.
No credentials or DSNs are archived. Every observation is kept before checking.
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


def inputs(values, typ):
    return '(' + ' UNION ALL '.join(f'SELECT {v}::{typ} AS x' for v in values) + ') t'


# Each family has explicit value and identity assertions independent of comparison.
CASES = []
for typ, sum_oid, avg_oid in [('int2',20,1700),('int4',20,1700),('int8',1700,1700),('numeric',1700,1700),('float4',700,701),('float8',701,701)]:
    for label, source, rows in [
        ('normal',inputs(['1','2','NULL'],typ),['3','2','3','1.5000000000000000' if avg_oid==1700 else '1.5']),
        ('empty',f'(SELECT 1::{typ} AS x WHERE false) t',['0','0',None,None]),
        ('null',inputs(['NULL','NULL'],typ),['2','0',None,None]),
    ]:
        CASES.append((f'{typ}-{label}',f'SELECT count(*) AS n,count(x) AS c,sum(x) AS s,avg(x) AS a FROM {source}',[20,20,sum_oid,avg_oid],[rows]))
CASES += [
 ('bigint-wide','SELECT sum(x) AS v FROM '+inputs(['9223372036854775807','1'],'int8'),[1700],[['9223372036854775808']]),
 ('numeric-scale','SELECT sum(x) AS v FROM '+inputs(["'1.20'","'2.003'"],'numeric'),[1700],[['3.203']]),
 ('distinct-numeric','SELECT count(DISTINCT x) AS n,sum(DISTINCT x) AS s,avg(DISTINCT x) AS a FROM '+inputs(["'1.0'","'1.00'","'2.00'",'NULL'],'numeric'),[20,1700,1700],[['2','3.00','1.5000000000000000']]),
 ('distinct-zero-nan',"SELECT count(DISTINCT x) AS n FROM "+inputs(["'NaN'","'NaN'","'Infinity'","'-Infinity'",'0',"'-0'"],'float8'),[20],[['4']]),
 ('filter',"SELECT count(*) FILTER (WHERE keep) AS n,sum(x) FILTER (WHERE keep) AS s,avg(x) FILTER (WHERE NOT keep) AS a FROM (SELECT 1::int4 AS x,true AS keep UNION ALL SELECT 3::int4,false UNION ALL SELECT 9::int4,NULL::bool) t",[20,20,1700],[['1','1','3.0000000000000000']]),
 ('filter-before-argument',"SELECT sum(1/x) FILTER (WHERE x<>0) AS v FROM "+inputs(['0','1'],'int4'),[20],[['1']]),
 ('order-sensitive',"SELECT sum(x ORDER BY k ASC) AS a,sum(x ORDER BY k DESC) AS b FROM (SELECT 1e16::float8 AS x,1::int4 AS k UNION ALL SELECT 1::float8,3::int4 UNION ALL SELECT '-1e16'::float8,2::int4) t",[701,701],[['1','0']]),
 ('order-distinct-filter',"SELECT sum(DISTINCT x ORDER BY x DESC) FILTER (WHERE keep) AS v FROM (SELECT 1::int4 AS x,true AS keep UNION ALL SELECT 1::int4,false UNION ALL SELECT 2::int4,true UNION ALL SELECT NULL::int4,true) t",[20],[['3']]),
 ('group',"SELECT g::int4 AS g,count(x) AS n,sum(x) AS s,avg(x) AS a FROM (SELECT 1::int4 AS g,1::int4 AS x UNION ALL SELECT 1::int4,3::int4 UNION ALL SELECT 2::int4,NULL::int4) t GROUP BY g ORDER BY g",[23,20,20,1700],[['1','2','4','2.0000000000000000'],['2','0',None,None]]),
 ('zero-description',"SELECT count(*) AS n,sum(x) AS s FROM (SELECT 1::int4 AS x WHERE false) t GROUP BY x",[20,20],[]),
]
for typ,oid in [('float4',700),('float8',701),('numeric',1700)]:
    for token in ('NaN','Infinity','-Infinity'):
        CASES.append((typ+'-'+token,'SELECT sum(x) AS s,avg(x) AS a FROM '+inputs([repr(token)],typ),[oid,1700 if typ=='numeric' else 701],[[token,token]]))
for typ,low,high in [('int2',-32768,32767),('int4',-2147483648,2147483647)]:
    for boundary in (low,high):
        CASES.append((f'{typ}-boundary-{boundary}','SELECT sum(x) AS v FROM '+inputs([repr(str(boundary)),repr(str(boundary))],typ),[20],[[str(boundary*2)]]))
CASES += [
 ('count-star-no-from','SELECT count(*) AS v',[20],[['1']]),
 ('count-null-no-from','SELECT count(NULL::int4) AS v',[20],[['0']]),
 ('distinct-numeric-wide','SELECT count(DISTINCT x) AS v FROM '+inputs(['9007199254740992','9007199254740993','9007199254740992.00'],'numeric'),[20],[['2']]),
]
CASES.append(('count-text','SELECT count(x) AS v FROM '+inputs(["'x'",'NULL'],'text'),[20],[['1']]))
for direction,value in [('ASC','1'),('DESC','0')]:
    CASES.append(('order-null-'+direction,f"SELECT sum(x ORDER BY k {direction}) AS v FROM (SELECT 1e16::float8 AS x,1::int4 AS k UNION ALL SELECT 1::float8,NULL::int4 UNION ALL SELECT '-1e16'::float8,2::int4) t",[701],[[value]]))
CASES.append(('filter-before-order',"SELECT sum(x ORDER BY 1/k) FILTER (WHERE keep) AS v FROM (SELECT 1::int4 AS x,0::int4 AS k,false AS keep UNION ALL SELECT 2::int4,1::int4,true) t",[20],[['2']]))
CASES.append(('float4-avg-width','SELECT sum(x) AS s,avg(x) AS a FROM '+inputs(['16777216','1',"'-16777216'"],'float4'),[700,701],[['0','0.3333333333333333']]))
# Exact boundary fixtures retained from tasks 3–6 and 10.
for label,values,typ,total,average in [
 ('bigint-max-pair',["'9223372036854775807'","'9223372036854775807'"],'int8','18446744073709551614','9223372036854775807'),
 ('numeric-wide',["'999999999999999999999999999999999999.999'","'0.001'"],'numeric','1000000000000000000000000000000000000.000','500000000000000000000000000000000000.000'),
 ('numeric-cancel',["'1.20'","'-1.2'"],'numeric','0.00','0.00000000000000000000'),
 ('third',['1','0','0'],'int2','1','0.33333333333333333333'),
]:
    CASES.append((label,'SELECT sum(x) AS s,avg(x) AS a FROM '+inputs(values,typ),[20 if typ=='int2' else 1700,1700],[[total,average]]))
for typ,oid in [('float4',700),('float8',701)]:
    CASES.append((typ+'-opposite-infinities','SELECT sum(x) AS s,avg(x) AS a FROM '+inputs(["'Infinity'","'-Infinity'"],typ),[oid,701],[['NaN','NaN']]))
for distinct in ('','DISTINCT '):
    for label,source,rows in [
        ('normal',inputs(['1','2','3','4'],'int4'),[['4','10','2.5000000000000000']]),
        ('distinct',inputs(['1','1','2'],'int4'),[['2','3','1.5000000000000000']] if distinct else [['3','4','1.3333333333333333']]),
        ('empty',inputs(['NULL','NULL'],'int4'),[['0',None,None]]),
    ]:
        CASES.append(('state-'+str(bool(distinct))+'-'+label,f'SELECT count({distinct}x) AS n,sum({distinct}x) AS s,avg({distinct}x) AS a FROM '+source,[20,20,1700],rows))
ERRORS = [
 ('numeric-invalid',"SELECT sum('invalid'::numeric) AS v",'22P02'),
 ('float4-invalid',"SELECT sum('bad'::float4) AS v",'22P02'),
 ('float8-invalid',"SELECT avg('bad'::float8) AS v",'22P02'),
 ('count-arity','SELECT count(1,2) AS v','42883'),
 ('count-distinct-arity','SELECT count(DISTINCT 1,2) AS v','42883'),
 ('sum-arity','SELECT sum(1::int8,2::int8) AS v','42883'),
 ('avg-arity','SELECT avg(1::int8,2::int8) AS v','42883'),
 ('filter-type','SELECT count(*) FILTER (WHERE 1::int4) AS v','42804'),
 ('filter-aggregate','SELECT count(*) FILTER (WHERE sum(1::int4)>0) AS v','42803'),
 ('filter-division','SELECT sum(10::int4/x) FILTER (WHERE true) AS v FROM error_input','22012'),
 ('float8-overflow','SELECT sum(x) AS v FROM '+inputs(['1e308','1e308'],'float8'),'22003'),
 ('avg-variance-overflow','SELECT avg(x) AS v FROM '+inputs(['1e154',"'-1e154'"],'float8'),'22003'),
 ('sum-text-unicode',"SELECT 'é'::text AS label,sum('x'::text) AS v",'42883'),
 ('distinct-order-unicode',"SELECT 'é'::text AS label,sum(DISTINCT x ORDER BY k) AS v FROM (SELECT 1::int4 AS x,2::int4 AS k) t",'42P10'),
 ('avg-text',"SELECT avg('x'::text) AS v",'42883'),
 ('sum-text-empty',"SELECT sum(x) AS v FROM (SELECT 'x'::text AS x WHERE false) t",'42883'),
 ('sum-text',"SELECT sum('x'::text) AS v",'42883'),
 ('distinct-order',"SELECT sum(DISTINCT x ORDER BY k) AS v FROM (SELECT 1::int4 AS x,2::int4 AS k) t",'42P10'),
 ('float-overflow','SELECT sum(x) AS v FROM '+inputs(['3e38','3e38'],'float4'),'22003'),
]
WIDTH = {21:2,20:8,23:4,700:4,701:8,1700:-1}


def encode(value,oid):
    if oid==16:return bytes([int(value)])
    if oid==1700:
        return struct.pack('!HHHHH',1,0,0x4000 if value<0 else 0,0,abs(value))
    return struct.pack({21:'!h',23:'!i',20:'!q',700:'!f',701:'!d'}[oid],value)


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
    def check(obs,oids,rows):
        assert obs['status']==2,obs
        metadata(obs,oids)
        assert obs['rows']==[[None if v is None else v.encode().hex() for v in row] for row in rows],obs
    def interleave(label):
        for side,conn in conns.items():
            obs=save(side,label,'recovery',query(conn,'SELECT 1::int4 AS v'))
            check(obs,[23],[['1']])
        sql='SELECT count(*) AS n,count(x) AS c,sum(x) AS s FROM (SELECT 1 AS x UNION ALL SELECT NULL AS x) t'
        obs=save('mysql',label,'query',mysql.query(sql))
        assert obs['code']==0 and obs['rows']==[['32','31','31']],obs
        assert [m[:2] for m in obs['metadata']]==[['n',8],['c',8],['s',246]],obs
        prepared=save('mysql',label,'prepare-execute',mysql.prepared('SELECT count(*) AS n,count(x) AS c,sum(x) AS s FROM (SELECT CAST(? AS SIGNED) AS x) t',[b'7',None,b'9']))
        assert prepared[0]['code']==0 and prepared[0]['parameters']==1,prepared
        assert [x['rows'] for x in prepared[1:]]==[[['31','31','37']],[['31','30',None]],[['31','31','39']]],prepared
        assert all(x['code']==0 and [m[:2] for m in x['metadata']]==[['n',8],['c',8],['s',246]] for x in prepared),prepared
        text_sum=save('mysql',label,'text-sum',mysql.query("SELECT sum('7') AS s"))
        assert text_sum['code']==0 and text_sum['rows']==[['37']],text_sum
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference login failed (DSN redacted)'
        identity={s:query(admin,s) for s in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW TimeZone','SHOW extra_float_digits','SHOW lc_numeric')}
        save('reference','identity','setup',identity)
        assert identity['SHOW server_version_num']['rows']==[['180006'.encode().hex()]],'PG18.6 required; other versions are separate evidence'
        binary=Path(os.environ['PG09_REFERENCE_BIN'])/'postgres'
        save('build','identity','setup',dict(reference_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),reference_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG09_11_RUST_BINARY']).read_bytes()).hexdigest()))
        assert query(admin,f'CREATE DATABASE {database}')['status']==1; owned=True
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('A',f'host=127.0.0.1 port={port} user=root dbname={database}'),('B',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
            conn=pq.PQconnectdb(dsn.encode()); assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)'
            conns[side]=conn; wires[side]=Wire(pq,dsn)
            save(side,'identity','setup',{sql:query(conn,sql) for sql in identity})
            if side!='B':
                assert query(conn,'CREATE TABLE error_input(x int)')['status']==1
                assert query(conn,'INSERT INTO error_input VALUES(0)')['status']==1
        mysql=Mysql('127.0.0.1',mysql_port,'root','',database)
        save('mysql','identity','setup',dict(client_version=mysql.mysql_get_client_version(),settings={sql:mysql.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection')}))
        def secrets():
            credential='pg0911-password-value'
            access_key='pg0911-access-key-value'
            literal=f's3://user:{credential}@bucket/path?secret-access-key={access_key}'
            for side in ('A','B'):
                for typ in ('numeric','float4','float8'):
                    obs=save(side,'redaction','local-security',query(conns[side],f"SELECT sum('{literal}'::{typ}) AS v"))
                    assert obs['status']==7 and obs['state']=='22P02',obs
                    assert credential not in json.dumps(obs) and access_key not in json.dumps(obs),'credential leaked'
                    check(query(conns[side],'SELECT 1::int4 AS v'),[23],[['1']])
        attempt('redaction',secrets)
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
        # Prepared and raw phase matrices use the same typed aggregate SQL, with
        # NULL rebind and a zero-row variant whose descriptor remains fixed.
        for typ,param_oid,total_oid,average_oid in [('int2',21,20,1700),('int4',23,20,1700),('int8',20,1700,1700),('numeric',1700,1700,1700),('float4',700,700,701),('float8',701,701,701),('bool',16,20,1700)]:
          for zero in (False,True):
            result_oids=[20,total_oid,average_oid]
            sql=f'SELECT count(x) AS n,sum(x) AS s,avg(x) AS a FROM (SELECT $1::{typ} AS x) t'+(' GROUP BY x HAVING false' if zero else '')
            if typ=='bool':
                sql='SELECT count(DISTINCT x) FILTER (WHERE $1::bool) AS n,sum(DISTINCT x) FILTER (WHERE $1::bool) AS s,avg(DISTINCT x) FILTER (WHERE $1::bool) AS a FROM '+inputs(['1','1','2','NULL'],'int4')+(' GROUP BY x HAVING false' if zero else '')
            name=('agg_'+typ+str(zero)).encode()
            def prepare():
                for side,conn in conns.items():
                    assert save(side,'prepare','Parse',pq.observe(pq.PQprepare(conn,name,sql.encode(),1,(c.c_uint*1)(param_oid))))['status']==1
                    desc=save(side,'prepare','statement-describe',pq.observe(pq.PQdescribePrepared(conn,name)))
                    assert desc['parameters']==[param_oid],desc
                    metadata(desc,result_oids)
                    assert [m[4:] for m in desc["metadata"]]==[[n,0,0] for n in ("n","s","a")],desc
                for side,w in wires.items():
                    assert save(side,'prepare','raw-Parse',w.phase(b'P',name+b'\0'+sql.encode()+b'\0'+struct.pack('!HI',1,param_oid),'1'))['tags']=='1'
                    desc=save(side,'prepare','raw-statement-describe',w.phase(b'D',b'S'+name+b'\0','Tn'))
                    assert desc['parameters']==[param_oid],desc
                    metadata(desc,result_oids)
                    assert [m[4:] for m in desc["metadata"]]==[[n,0,0] for n in ("n","s","a")],desc
            attempt('prepare-'+typ+'-'+str(zero),prepare)
            for infmt in (0,1):
              for outfmt in (0,1):
               for value in ((True,None,False) if typ=='bool' else (7,None,-9)):
                label=f'bind-{typ}-{zero}-{infmt}-{outfmt}-{value}'
                def bind():
                    encoded=None if value is None else encode(value,param_oid) if infmt else (('true' if value else 'false').encode() if typ=='bool' else str(value).encode())
                    peers={};raw={}
                    for side,conn in conns.items():
                        obs=save(side,label,'libpq-bind-execute',pq.observe(pq.PQexecPrepared(conn,name,1,(c.c_char_p*1)(encoded),(c.c_int*1)(len(encoded) if encoded else 0),(c.c_int*1)(infmt),outfmt)))
                        assert obs['status']==2,obs
                        metadata(obs,result_oids,outfmt);peers[side]=obs
                        assert len(obs['rows'])==(0 if zero else 1),obs
                        if not zero and (value is None or typ=='bool' and not value):assert obs['rows'][0][1:]==[None,None],obs
                    compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                    for side,w in wires.items():
                        body=b'\0'+name+b'\0'+struct.pack('!HHH',1,infmt,1)+struct.pack('!i',-1 if encoded is None else len(encoded))+(encoded or b'')+struct.pack('!HH',1,outfmt)
                        assert save(side,label,'Bind',w.phase(b'B',body,'2'))['tags']=='2'
                        desc=save(side,label,'portal-describe',w.phase(b'D',b'P\0','Tn'));metadata(desc,result_oids,outfmt)
                        w.send(b'E',b'\0\0\0\0\0');w.send(b'S');obs=save(side,label,'Execute-Sync',w.read('Z'));raw[side]=obs
                        assert obs['ready']=='I' and obs['tags']==('CZ' if zero else 'DCZ') and not obs['metadata'],obs
                        assert obs['rows']==peers[side]['rows'],obs
                        if not zero and (value is None or typ=='bool' and not value):assert obs['lengths'][0][1:]==[-1,-1],obs
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
        summary=dict(expected=304,attempted=len(expected),unique=len(set(expected)),passed=len(passed),failed=len(failures),failures=failures,negative_self_checks=3,**cleanup)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
        print(json.dumps(summary),flush=True)
    assert len(expected)==len(set(expected))==304,summary
    assert not failures,failures


if __name__=='__main__':
    assert self_test()==3
    if sys.argv[1:] == ['--self-test']:print('negative_self_checks=3')
    else:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
