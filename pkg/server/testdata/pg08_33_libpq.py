# Copyright 2026 AsterSQL.
"""PG ILIKE/ESCAPE through independent libpq; native MySQL retains LIKE semantics."""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare

CASES = (
 ('base',"'Ab'::text ILIKE 'a%'::text",'t'),
 ('not',"'Ab'::text NOT ILIKE 'a%'::text",'f'),
 ('escape',"'A_b'::text ILIKE 'a!_b'::text ESCAPE '!'::text",'t'),
 ('default',"'A_b'::text ILIKE 'a\\_b'::text",'t'),
 ('empty',"'A_b'::text ILIKE 'a_b'::text ESCAPE ''::text",'t'),
 ('nullvalue',"NULL::text ILIKE 'a%'::text",None),
 ('nullpattern',"'Ab'::text ILIKE NULL::text",None),
 ('nullescape',"'Ab'::text ILIKE 'a%'::text ESCAPE NULL::text",None),
 ('C',"'Ä'::text COLLATE \"C\" ILIKE 'ä'::text",'f'),
 ('unicode',"'Ä'::text COLLATE \"en_US.UTF-8\" ILIKE 'ä'::text",'t'),
 ('utfescape',"'A_b'::text ILIKE 'aé_b'::text ESCAPE 'é'::text",'t'),
 ('letterescape',"'AB'::text ILIKE 'aXB'::text ESCAPE 'X'::text",'t'),
 ('percent',"'A%b'::text ILIKE 'a!%b'::text ESCAPE '!'::text",'t'),
 ('nested',"COALESCE('Ab'::text ILIKE 'a%'::text,false)",'t'),
 ('row',"v::text ILIKE 'a%'::text",'t'),
 ('rowzero',"v::text NOT ILIKE 'a%'::text",None),
 ('zero',"NULL::text ILIKE 'a%'::text",None),
 ('priority',"NOT 'Ab'::text NOT ILIKE 'a%'::text AND ('x'::text = 'x'::text)",'t'),
)
ERRORS=(('multi',"SELECT 'Ab'::text ILIKE 'a%'::text ESCAPE '!!'::text AS v"),('tail',"SELECT 'Abc'::text ILIKE 'Ab!'::text ESCAPE '!'::text AS v"))
CASE_IDS=tuple(['Q-'+label for label,*_ in CASES]+[f'bind-{op}-{i}-{o}-{k}' for op in ('ilike','not') for i in (0,1) for o in (0,1) for k in range(3)]+[f'bind-zero-{i}-{o}' for i in (0,1) for o in (0,1)]+[f'error-{label}-{phase}' for label,_ in ERRORS for phase in ('Q','P','txn')]+[f'M-{label}-{mode}' for label in ('like','escape','null','health') for mode in ('Q','P')]+['I-before','I-middle','I-after'])
assert len(CASE_IDS)==len(set(CASE_IDS))==63

def run(port,database,evidence,mysql_port):
    evidence.mkdir(parents=True,exist_ok=False)
    assert database.replace('_','').isalnum()
    pq=Libpq();conns={};mysqls={};passed=[];attempted=set();failures={};cleanup={};admin=None;owned=False
    tx=pq.lib.PQtransactionStatus;tx.restype=c.c_int;tx.argtypes=[c.c_void_p]
    def query(conn,sql):return pq.observe(pq.PQexec(conn,sql.encode()))
    def save(side,case,obs,phase='query'):
        if case in CASE_IDS:attempted.add(case)
        with (evidence/(side+'.jsonl')).open('a') as f:f.write(json.dumps(dict(case_id=case,phase=phase,observation=obs))+'\n')
        return obs
    def check(obs,wanted,zero=False,fmt=0):
        assert obs['status']==2 and obs['metadata']==[[16,1,-1,fmt,'v',0,0]],obs
        encoded=None if wanted is None else ((b'\x01' if wanted=='t' else b'\x00') if fmt else wanted.encode()).hex()
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
        paired(case,"SELECT 'Ab'::text ILIKE 'a%'::text AS v",'t')
        peers=[save('mysql-'+side,case,m.query('SELECT v FROM health')) for side,m in mysqls.items()]
        assert all(x['code']==0 and x['rows']==[['37']] for x in peers),peers
        mysql_compare(*peers)
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode());assert admin and pq.PQstatus(admin)==0,'reference failed (DSN redacted)'
        identity={sql:query(admin,sql) for sql in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW lc_time','SHOW lc_numeric','SHOW lc_monetary','SHOW TimeZone','SHOW search_path',"SELECT collname,collprovider,collisdeterministic,collcollate FROM pg_collation WHERE collname='en_US.UTF-8'")}
        assert identity['SHOW server_version_num']['rows']==[['313830303036']]
        assert identity["SELECT collname,collprovider,collisdeterministic,collcollate FROM pg_collation WHERE collname='en_US.UTF-8'"]['rows']==[['656e5f55532e5554462d38','63','74','656e5f55532e5554462d38']]
        save('reference','identity',identity)
        repo=Path(__file__).resolve().parents[3];bins={}
        for key,env,name in [('postgres','PG08_REFERENCE_BIN','postgres'),('mysql','PG08_MYSQL_BIN','mysqld')]:
            binary=Path(os.environ[env])/name;bins[key]=dict(version=subprocess.check_output([str(binary),'--version'],text=True).strip(),sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        tz=Path(os.environ['PG08_TZDATA_SOURCE']).read_bytes()
        save('build','identity',dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),binaries=bins,tzdata_version=tz.splitlines()[0].decode(),tzdata_sha256=hashlib.sha256(tz).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG08_33_RUST_BINARY']).read_bytes()).hexdigest()))
        assert query(admin,f'CREATE DATABASE {database}')['status']==1;owned=True
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={database}'),('A',f'host=127.0.0.1 port={port} user=root dbname={database}'),('B',f'host=127.0.0.1 port={port} user=root dbname={database}')]:
            conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)';conns[side]=conn
            if side!='B':
                assert query(conn,'CREATE TABLE source(v text)')['status']==1
                assert query(conn,"INSERT INTO source VALUES('Ab')")['status']==1
        mysqls['reference']=Mysql(os.environ['MYSQL8_REFERENCE_HOST'],int(os.environ['MYSQL8_REFERENCE_PORT']),os.environ['MYSQL8_REFERENCE_USER'],os.environ['MYSQL8_REFERENCE_PASSWORD'])
        mysqls['aster']=Mysql('127.0.0.1',mysql_port,'root','',database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code']==0
        assert mysqls['reference'].query(f'USE {database}')['code']==0
        for side,m in mysqls.items():
            save('mysql-'+side,'identity',{sql:m.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection')})
            save('mysql-'+side,'client-version',m.mysql_get_client_version())
            assert m.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code']==0
            assert m.query('CREATE TABLE health(v INT)')['code']==0
            assert m.query('INSERT INTO health VALUES(7)')['code']==0
        attempt('I-before',lambda:interleave('before'))
        for label,expr,wanted in CASES:
            sql=f'SELECT {expr} AS v'+(' FROM source' if label in ('row','rowzero') else '')+(' WHERE false' if label in ('rowzero','zero') else '')
            attempt('Q-'+label,lambda:paired('Q-'+label,sql,wanted,label in ('rowzero','zero')))
        attempt('I-middle',lambda:interleave('middle'))
        for op in ('ilike','not','zero'):
            statement=('s_'+op).encode();sql='SELECT $1::text '+('NOT ' if op=='not' else '')+'ILIKE $2::text ESCAPE $3::text AS v'+(' WHERE false' if op=='zero' else '')
            for side,conn in conns.items():
                assert save(side,'parse',pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)),'statement-describe')
                assert desc['parameters']==[25,25,25] and desc['metadata']==[[16,1,-1,0,'v',0,0]],desc
            for infmt in (0,1):
              for outfmt in (0,1):
                for k in range(1 if op=='zero' else 3):
                    case=f'bind-zero-{infmt}-{outfmt}' if op=='zero' else f'bind-{op}-{infmt}-{outfmt}-{k}'
                    def bind():
                        peers={};values=[None if k==1 else b'A_b',b'a!_b',b'!']
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,statement,3,(c.c_char_p*3)(*values),(c.c_int*3)(*(len(v) if v is not None else 0 for v in values)),(c.c_int*3)(infmt,infmt,infmt),outfmt)),'bind-execute');peers[side]=obs
                            check(obs,None if k==1 else 'f' if op=='not' else 't',op=='zero',outfmt)
                        compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
                    attempt(case,bind)
        for label,sql in ERRORS:
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
                    assert obs['status']==7 and obs['state']=='22025' and not obs['rows'] and not obs['metadata'],obs
                    assert all(obs['errors'][key] for key in 'SVCM'),obs
                    assert obs['errors']['M']==('invalid escape string' if label=='multi' else 'LIKE pattern must not end with escape character'),obs
                    if phase=='txn':
                        assert tx(conn)==3
                        assert query(conn,'ROLLBACK')['status']==1 and tx(conn)==0
                    assert save(side,case,query(conn,'SELECT 1::int4 AS v'),'Sync-recovery')['rows']==[['31']]
                    assert query(conn,'SELECT v FROM source')['rows']==[['4162']]
                compare(peers['reference'],peers['A']);compare(peers['reference'],peers['B'])
            attempt(case,errorcase)
            # Restore transactions after failed assertions too.
            for conn in conns.values():
                if tx(conn) in (2,3):query(conn,'ROLLBACK')
        for label,sql in [('like',"SELECT 'Ab' LIKE 'a%' AS v"),('escape',"SELECT 'A_b' LIKE 'A!_b' ESCAPE '!' AS v"),('null',"SELECT NULL LIKE 'a%' AS v"),('health','SELECT v AS v FROM health')]:
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
        for m in mysqls.values():m.close()
        category=lambda x:'binding' if x.startswith('bind-') else 'error-recovery' if x.startswith('error-') else 'mysql' if x.startswith('M-') else 'interleaved' if x.startswith('I-') else 'zero' if x in ('Q-zero','Q-rowzero') else 'null' if x.startswith('Q-null') else 'success'
        summary=dict(expected=63,actual=len(attempted),unique=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(set(CASE_IDS)-attempted),failures=failures,negative_self_checks=3,connections_closed=True,reference_database_dropped=cleanup.get('reference_database_dropped',False),cleanup=cleanup,case_ids=passed,expected_case_ids=CASE_IDS,counts={key:sum(category(x)==key for x in passed) for key in set(map(category,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert all(cleanup.values()),cleanup
    assert len(passed)==63 and not failures,summary
    print(json.dumps(summary))
if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(143))
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
