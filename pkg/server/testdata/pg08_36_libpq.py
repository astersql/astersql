# Copyright 2026 AsterSQL.
"""PG numeric division through independent libpq; native MySQL division contracts."""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import struct
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare

CASES = (('positive', '5::numeric / 2::numeric', '2.5000000000000000'), ('negative', "'-5'::numeric / 2::numeric", '-2.5000000000000000'), ('negative-divisor', "5::numeric / '-2'::numeric", '-2.5000000000000000'), ('both-negative', "'-5'::numeric / '-2'::numeric", '2.5000000000000000'), ('large', "'9007199254740993'::numeric / 1::numeric", '9007199254740993.0000'), ('large-fraction', "'12345678901234567890123456789012345678901234567890123456789012345678901234567890'::numeric / 2::numeric", '6172839450617283945061728394506172839450617283945061728394506172839450617283945'), ('third', '1::numeric / 3::numeric', '0.33333333333333333333'), ('round-up','2::numeric / 3::numeric','0.66666666666666666667'), ('small', "'0.00001'::numeric / 2::numeric", '0.000005000000000000000000'), ('scaled', "'5.0000000000000000000000000'::numeric / 2::numeric", '2.5000000000000000000000000'), ('zero-value', '0::numeric / 2::numeric', '0.00000000000000000000'), ('null-left', 'NULL::numeric / 0::numeric', None), ('null-right', '1::numeric / NULL::numeric', None), ('qualified', '5::numeric OPERATOR(pg_catalog./) 2::numeric', '2.5000000000000000'), ('nested', 'coalesce(NULL::numeric / 0::numeric,5::numeric / 2::numeric)', '2.5000000000000000'), ('row', 'a / b', '2.5000000000000000'), ('rowzero', 'a / b', None), ('zero', 'NULL::numeric / 0::numeric', None), ('nan', "'NaN'::numeric / 0::numeric", 'NaN'), ('infinity', "'Infinity'::numeric / 2::numeric", 'Infinity'), ('finite-infinity', "5::numeric / 'Infinity'::numeric", '0'), ('both-infinity', "'Infinity'::numeric / 'Infinity'::numeric", 'NaN'), ('negative-infinity', "'Infinity'::numeric / '-2'::numeric", '-Infinity'))
ERRORS=(("divide-zero","SELECT 1::numeric / 0::numeric AS v",'22012','division by zero'),)
MYSQL_CASES=(('slash','SELECT 5 / 2 AS v'),('negative','SELECT -5 / 2 AS v'),('div','SELECT 5 DIV 2 AS v'),('negdiv','SELECT -5 DIV 2 AS v'),('null','SELECT CAST(NULL AS SIGNED) / 0 AS v'),('zero','SELECT 1 / 0 AS v'),('health','SELECT v AS v FROM health'))
CASE_IDS=tuple(['Q-'+label for label,*_ in CASES]+[f'bind-{op}-{i}-{o}-{k}' for op in ('ordinary','qualified') for i in (0,1) for o in (0,1) for k in range(4)]+[f'bind-zero-{i}-{o}' for i in (0,1) for o in (0,1)]+[f'error-{label}-{phase}' for label,*_ in ERRORS for phase in ('Q','P','txn')]+[f'M-{label}-{mode}' for label,_ in MYSQL_CASES for mode in ('Q','P')]+['I-before','I-middle','I-after'])
assert len(CASE_IDS)==len(set(CASE_IDS))==79

def numeric(text):
    if text in ('NaN','Infinity','-Infinity'):
        return struct.pack('!HHHH',0,0,{'NaN':0xc000,'Infinity':0xd000,'-Infinity':0xf000}[text],0 if text=='NaN' else 32)
    negative=text.startswith('-');text=text.lstrip('-+')
    integer,_,fraction=text.partition('.')
    pad='0'*((-len(integer))%4)+integer+fraction+'0'*((-len(fraction))%4)
    digits=[int(pad[i:i+4]) for i in range(0,len(pad),4)];weight=(len(integer)+3)//4-1
    while digits and digits[0]==0:digits.pop(0);weight-=1
    while digits and digits[-1]==0:digits.pop()
    return struct.pack('!HhHH',len(digits),weight if digits else 0,0x4000 if negative and digits else 0,len(fraction))+b''.join(struct.pack('!H',d) for d in digits)

def run(port,database,evidence,mysql_port):
    evidence.mkdir(parents=True,exist_ok=False)
    assert database.replace('_','').isalnum()
    pq=Libpq();conns={};mysqls={};passed=[];attempted=set();failures={};cleanup={};mysql_initial={};admin=None;owned=False
    tx=pq.lib.PQtransactionStatus;tx.restype=c.c_int;tx.argtypes=[c.c_void_p]
    def query(conn,sql):return pq.observe(pq.PQexec(conn,sql.encode()))
    def save(side,case,obs,phase='query'):
        if case in CASE_IDS:attempted.add(case)
        with (evidence/(side+'.jsonl')).open('a') as f:f.write(json.dumps(dict(case_id=case,phase=phase,observation=obs))+'\n')
        return obs
    def check(obs,wanted,zero=False,fmt=0):
        assert obs['status']==2 and obs['metadata']==[[1700,-1,-1,fmt,'v',0,0]],obs
        encoded=None if wanted is None else (numeric(wanted) if fmt else wanted.encode()).hex()
        assert obs['rows']==([] if zero else [[encoded]]),obs
    def attempt(case,body):
        attempted.add(case)
        try:body();passed.append(case)
        except AssertionError as e:failures[case]=str(e)
    def paired(case,sql,wanted,zero=False):
        peers={}
        for side,conn in conns.items():
            obs=save(side,case,query(conn,sql));peers[side]=obs;check(obs,wanted,zero)
        compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
    def interleave(label):
        case='I-'+label
        paired(case,"SELECT 5::numeric / 2::numeric AS v",'2.5000000000000000')
        peers=[save('mysql-'+side,case,m.query('SELECT v FROM health')) for side,m in mysqls.items()]
        assert all(x['code']==0 and x['rows']==[['37']] for x in peers),peers
        mysql_compare(*peers)
        peers=[save('mysql-'+side,case,m.query('SELECT 5 DIV 2 AS v'),'native-behavior') for side,m in mysqls.items()]
        mysql_compare(*peers)
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode());assert admin and pq.PQstatus(admin)==0,'reference failed (DSN redacted)'
        identity={sql:query(admin,sql) for sql in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW lc_time','SHOW lc_numeric','SHOW lc_monetary','SHOW TimeZone','SHOW search_path')}
        assert identity['SHOW server_version_num']['rows']==[['313830303036']]
        save('reference','identity',identity)
        repo=Path(__file__).resolve().parents[3];bins={}
        for key,env,name in [('postgres','PG08_REFERENCE_BIN','postgres'),('mysql','PG08_MYSQL_BIN','mysqld')]:
            binary=Path(os.environ[env])/name;bins[key]=dict(version=subprocess.check_output([str(binary),'--version'],text=True).strip(),sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        tz=Path(os.environ['PG08_TZDATA_SOURCE']).read_bytes()
        save('build','identity',dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),binaries=bins,tzdata_version=tz.splitlines()[0].decode(),tzdata_sha256=hashlib.sha256(tz).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG08_36_RUST_BINARY']).read_bytes()).hexdigest()))
        assert query(admin,f'CREATE DATABASE {database}')['status']==1;owned=True
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('A',f'host=127.0.0.1 port={port} user=root dbname={database}'),('B',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
            conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)';conns[side]=conn
            if side!='B':
                assert query(conn,'CREATE TABLE source(a numeric,b numeric)')['status']==1
                assert query(conn,"INSERT INTO source VALUES(5,2)")['status']==1
        mysqls['reference']=Mysql(os.environ['MYSQL8_REFERENCE_HOST'],int(os.environ['MYSQL8_REFERENCE_PORT']),os.environ['MYSQL8_REFERENCE_USER'],os.environ['MYSQL8_REFERENCE_PASSWORD'])
        mysqls['aster']=Mysql('127.0.0.1',mysql_port,'root','',database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code']==0
        assert mysqls['reference'].query(f'USE {database}')['code']==0
        for side,m in mysqls.items():
            identity={sql:m.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection','SELECT @@div_precision_increment')}
            save('mysql-'+side,'identity',identity)
            if side=='reference':assert identity['SELECT @@div_precision_increment']['rows']==[['34']],identity['SELECT @@div_precision_increment']
            mysql_initial[side]=[bytes.fromhex(identity[sql]['rows'][0][0]).decode() for sql in ('SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection')]
            save('mysql-'+side,'client-version',m.mysql_get_client_version())
            assert m.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code']==0
            assert m.query("SET sql_mode='STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'")['code']==0
            assert m.query('CREATE TABLE health(v INT)')['code']==0
            assert m.query('INSERT INTO health VALUES(7)')['code']==0
        attempt('I-before',lambda:interleave('before'))
        for label,expr,wanted in CASES:
            sql=f'SELECT {expr} AS v'+(' FROM source' if label in ('row','rowzero') else '')+(' WHERE false' if label in ('rowzero','zero') else '')
            attempt('Q-'+label,lambda:paired('Q-'+label,sql,wanted,label in ('rowzero','zero')))
        attempt('I-middle',lambda:interleave('middle'))
        for op in ('ordinary','qualified','zero'):
            statement=('s_'+op).encode();sql='SELECT $1::numeric '+('OPERATOR(pg_catalog./)' if op=='qualified' else '/')+' $2::numeric AS v'+(' WHERE false' if op=='zero' else '')
            for side,conn in conns.items():
                assert save(side,'parse',pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)),'statement-describe')
                assert desc['parameters']==[1700,1700] and desc['metadata']==[[1700,-1,-1,0,'v',0,0]],desc
            for infmt in (0,1):
              for outfmt in (0,1):
                for k in range(1 if op=='zero' else 4):
                    case=f'bind-zero-{infmt}-{outfmt}' if op=='zero' else f'bind-{op}-{infmt}-{outfmt}-{k}'
                    def bind():
                        peers={};values=[None if k==1 else (numeric('9007199254740993' if k==3 else '-5' if k==2 else '5') if infmt else (b'9007199254740993' if k==3 else b'-5' if k==2 else b'5')),numeric('2') if infmt else b'2']
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,statement,2,(c.c_char_p*2)(*values),(c.c_int*2)(*(len(v) if v is not None else 0 for v in values)),(c.c_int*2)(infmt,infmt),outfmt)),'bind-execute');peers[side]=obs
                            check(obs,None if k==1 else '4503599627370496.5000' if k==3 else '-2.5000000000000000' if k==2 else '2.5000000000000000',op=='zero',outfmt)
                        compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                    attempt(case,bind)
        for label,sql,state,message in ERRORS:
          for phase in ('Q','P','txn'):
            case=f'error-{label}-{phase}'
            def errorcase():
                peers={}
                for side,conn in conns.items():
                    if phase=='txn':assert query(conn,'BEGIN')['status']==1
                    if phase=='P':
                        obs=save(side,case,pq.observe(pq.PQprepare(conn,('bad_'+label).encode(),sql.encode(),0,None)),'parse-analyze')
                        if obs['status']==1:obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,('bad_'+label).encode(),0,None,None,None,0)),'plan-execute')
                    else:obs=save(side,case,query(conn,sql),phase)
                    peers[side]=obs
                    assert obs['status']==7 and obs['state']==state and not obs['rows'] and not obs['metadata'],obs
                    assert all(obs['errors'][key] for key in 'SVCM'),obs
                    assert obs['errors']['M']==message,obs
                    if phase=='txn':
                        assert tx(conn)==3
                        assert query(conn,'ROLLBACK')['status']==1 and tx(conn)==0
                    assert save(side,case,query(conn,'SELECT 1::int4 AS v'),'Sync-recovery')['rows']==[['31']]
                    assert query(conn,'SELECT a,b FROM source')['rows']==[['35','32']]
                compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
            attempt(case,errorcase)
            # Restore transactions after failed assertions too.
            for conn in conns.values():
                if tx(conn) in (2,3):query(conn,'ROLLBACK')
        for label,sql in MYSQL_CASES:
          for mode in ('Q','P'):
            case=f'M-{label}-{mode}'
            def mysqlcase():
                peers=[save('mysql-'+side,case,m.query(sql) if mode=='Q' else m.prepared(sql,[()]),mode) for side,m in mysqls.items()]
                if mode=='Q':mysql_compare(*peers)
                else:assert peers[0]==peers[1],peers
            attempt(case,mysqlcase)
        attempt('I-after',lambda:interleave('after'))
    finally:
        for conn in conns.values():
            if tx(conn) in (2,3):query(conn,'ROLLBACK')
        for key,conn in conns.items():
            if key!='B':cleanup[key+'-table-dropped']=query(conn,'DROP TABLE IF EXISTS source')['status']==1
            pq.PQfinish(conn)
        if admin:
            if owned:cleanup['reference_database_dropped']=query(admin,f'DROP DATABASE {database}')['status']==1
            pq.PQfinish(admin)
        if 'reference' in mysqls:cleanup['mysql-reference-database-dropped']=mysqls['reference'].query(f'DROP DATABASE IF EXISTS {database}')['code']==0
        for side,m in mysqls.items():
            if side in mysql_initial:
                mode,charset,collation=mysql_initial[side]
                assert all(x.replace('_','').isalnum() for x in (charset,collation))
                assert all(x.replace('_','').isalnum() for x in mode.split(','))
                cleanup['mysql-'+side+'-settings-restored']=(m.query("SET sql_mode='"+mode+"'")['code']==0 and m.query(f'SET NAMES {charset} COLLATE {collation}')['code']==0)
            m.close()
        category=lambda x:'binding' if x.startswith('bind-') else 'error-recovery' if x.startswith('error-') else 'mysql' if x.startswith('M-') else 'interleaved' if x.startswith('I-') else 'zero' if x in ('Q-zero','Q-rowzero') else 'null' if x.startswith('Q-null') else 'success'
        summary=dict(expected=79,actual=len(attempted),unique=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(set(CASE_IDS)-attempted),failures=failures,negative_self_checks=3,connections_closed=True,reference_database_dropped=cleanup.get('reference_database_dropped',False),cleanup=cleanup,case_ids=passed,expected_case_ids=CASE_IDS,counts={key:sum(category(x)==key for x in passed) for key in set(map(category,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert all(cleanup.values()),cleanup
    assert len(passed)==79 and not failures,summary
    print(json.dumps(summary))
if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(143))
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
