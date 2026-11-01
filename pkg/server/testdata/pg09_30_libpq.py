# Copyright 2026 AsterSQL.
"""Set-returning libpq/raw TCP differential. Reuses existing client implementations.

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

# Explicit source-family fixtures from tasks 23–29.
CASES = [('int4-asc-from', 'SELECT g.v AS v FROM generate_series(1::int4,3::int4) AS g(v)', [23], [['1'], ['2'], ['3']]),
 ('int4-asc-projection', 'SELECT generate_series(1::int4,3::int4) AS v', [23], [['1'], ['2'], ['3']]),
 ('int4-desc-from',
  'SELECT g.v AS v FROM generate_series(3::int4,1::int4,-1::int4) AS g(v)',
  [23],
  [['3'], ['2'], ['1']]),
 ('int4-desc-projection', 'SELECT generate_series(3::int4,1::int4,-1::int4) AS v', [23], [['3'], ['2'], ['1']]),
 ('int4-empty-from', 'SELECT g.v AS v FROM generate_series(2::int4,1::int4) AS g(v)', [23], []),
 ('int4-empty-projection', 'SELECT generate_series(2::int4,1::int4) AS v', [23], []),
 ('int4-null-from', 'SELECT g.v AS v FROM generate_series(NULL::int4,2::int4) AS g(v)', [23], []),
 ('int4-null-projection', 'SELECT generate_series(NULL::int4,2::int4) AS v', [23], []),
 ('int4-null-zero-from', 'SELECT g.v AS v FROM generate_series(NULL::int4,2::int4,0::int4) AS g(v)', [23], []),
 ('int4-null-zero-projection', 'SELECT generate_series(NULL::int4,2::int4,0::int4) AS v', [23], []),
 ('int4-max-from',
  'SELECT g.v AS v FROM generate_series(2147483647::int4,2147483647::int4) AS g(v)',
  [23],
  [['2147483647']]),
 ('int4-max-projection', 'SELECT generate_series(2147483647::int4,2147483647::int4) AS v', [23], [['2147483647']]),
 ('int4-min-from',
  "SELECT g.v AS v FROM generate_series('-2147483648'::int4,'-2147483648'::int4) AS g(v)",
  [23],
  [['-2147483648']]),
 ('int4-min-projection',
  "SELECT generate_series('-2147483648'::int4,'-2147483648'::int4) AS v",
  [23],
  [['-2147483648']]),
 ('int4-overshoot-from',
  'SELECT g.v AS v FROM generate_series(2147483646::int4,2147483647::int4,2::int4) AS g(v)',
  [23],
  [['2147483646']]),
 ('int4-overshoot-projection',
  'SELECT generate_series(2147483646::int4,2147483647::int4,2::int4) AS v',
  [23],
  [['2147483646']]),
 ('int8-asc-from', 'SELECT g.v AS v FROM generate_series(1::int8,3::int8) AS g(v)', [20], [['1'], ['2'], ['3']]),
 ('int8-asc-projection', 'SELECT generate_series(1::int8,3::int8) AS v', [20], [['1'], ['2'], ['3']]),
 ('int8-desc-from',
  'SELECT g.v AS v FROM generate_series(3::int8,1::int8,-1::int8) AS g(v)',
  [20],
  [['3'], ['2'], ['1']]),
 ('int8-desc-projection', 'SELECT generate_series(3::int8,1::int8,-1::int8) AS v', [20], [['3'], ['2'], ['1']]),
 ('int8-empty-from', 'SELECT g.v AS v FROM generate_series(2::int8,1::int8) AS g(v)', [20], []),
 ('int8-empty-projection', 'SELECT generate_series(2::int8,1::int8) AS v', [20], []),
 ('int8-null-from', 'SELECT g.v AS v FROM generate_series(NULL::int8,2::int8) AS g(v)', [20], []),
 ('int8-null-projection', 'SELECT generate_series(NULL::int8,2::int8) AS v', [20], []),
 ('int8-null-zero-from', 'SELECT g.v AS v FROM generate_series(NULL::int8,2::int8,0::int8) AS g(v)', [20], []),
 ('int8-null-zero-projection', 'SELECT generate_series(NULL::int8,2::int8,0::int8) AS v', [20], []),
 ('int8-max-from',
  'SELECT g.v AS v FROM generate_series(9223372036854775807::int8,9223372036854775807::int8) AS g(v)',
  [20],
  [['9223372036854775807']]),
 ('int8-max-projection',
  'SELECT generate_series(9223372036854775807::int8,9223372036854775807::int8) AS v',
  [20],
  [['9223372036854775807']]),
 ('int8-min-from',
  "SELECT g.v AS v FROM generate_series('-9223372036854775808'::int8,'-9223372036854775808'::int8) AS g(v)",
  [20],
  [['-9223372036854775808']]),
 ('int8-min-projection',
  "SELECT generate_series('-9223372036854775808'::int8,'-9223372036854775808'::int8) AS v",
  [20],
  [['-9223372036854775808']]),
 ('int8-overshoot-from',
  'SELECT g.v AS v FROM generate_series(9223372036854775806::int8,9223372036854775807::int8,2::int8) AS g(v)',
  [20],
  [['9223372036854775806']]),
 ('int8-overshoot-projection',
  'SELECT generate_series(9223372036854775806::int8,9223372036854775807::int8,2::int8) AS v',
  [20],
  [['9223372036854775806']]),
 ('array-normal-from',
  'SELECT u.v AS v FROM unnest(ARRAY[2,NULL,2]::int4[]) AS u(v)',
  [23],
  [['2'], [None], ['2']]),
 ('array-normal-projection', 'SELECT unnest(ARRAY[2,NULL,2]::int4[]) AS v', [23], [['2'], [None], ['2']]),
 ('array-empty-from', 'SELECT u.v AS v FROM unnest(ARRAY[]::int4[]) AS u(v)', [23], []),
 ('array-empty-projection', 'SELECT unnest(ARRAY[]::int4[]) AS v', [23], []),
 ('array-null-from', 'SELECT u.v AS v FROM unnest(NULL::int4[]) AS u(v)', [23], []),
 ('array-null-projection', 'SELECT unnest(NULL::int4[]) AS v', [23], []),
 ('array-all-null-from', 'SELECT u.v AS v FROM unnest(ARRAY[NULL,NULL]::int4[]) AS u(v)', [23], [[None], [None]]),
 ('array-all-null-projection', 'SELECT unnest(ARRAY[NULL,NULL]::int4[]) AS v', [23], [[None], [None]]),
 ('array-dimensions-from',
  "SELECT u.v AS v FROM unnest('[0:1][3:4]={{1,2},{3,4}}'::int4[]) AS u(v)",
  [23],
  [['1'], ['2'], ['3'], ['4']]),
 ('array-dimensions-projection',
  "SELECT unnest('[0:1][3:4]={{1,2},{3,4}}'::int4[]) AS v",
  [23],
  [['1'], ['2'], ['3'], ['4']]),
 ('array-boundary-from',
  "SELECT u.v AS v FROM unnest(ARRAY['-2147483648'::int4,2147483647]) AS u(v)",
  [23],
  [['-2147483648'], ['2147483647']]),
 ('array-boundary-projection',
  "SELECT unnest(ARRAY['-2147483648'::int4,2147483647]) AS v",
  [23],
  [['-2147483648'], ['2147483647']]),
 ('filter-sort-limit',
  'SELECT g.v AS v FROM pg_catalog.generate_series(1::int4,5::int4) g(v) WHERE g.v>2 ORDER BY g.v DESC LIMIT 2',
  [23],
  [['5'], ['4']]),
 ('lateral-CROS',
  'SELECT t.id AS id,g.v AS v FROM (VALUES(1::int4,2::int4),(2,0),(3,NULL::int4)) t(id,n) CROSS JOIN LATERAL '
  'generate_series(1::int4,t.n) g(v) ORDER BY t.id,g.v',
  [23, 23],
  [['1', '1'], ['1', '2']]),
 ('lateral-LEFT',
  'SELECT t.id AS id,g.v AS v FROM (VALUES(1::int4,2::int4),(2,0),(3,NULL::int4)) t(id,n) LEFT JOIN LATERAL '
  'generate_series(1::int4,t.n) g(v) ON true ORDER BY t.id,g.v',
  [23, 23],
  [['1', '1'], ['1', '2'], ['2', None], ['3', None]]),
 ('correlated-projection',
  'SELECT t.id AS id,unnest(t.a) AS v FROM '
  '(VALUES(1::int4,ARRAY[2,NULL,2]::int4[]),(2,ARRAY[]::int4[]),(3,NULL::int4[]),(4,ARRAY[7]::int4[])) t(id,a)',
  [23, 23],
  [['1', '2'], ['1', None], ['1', '2'], ['4', '7']]),
 ('multiple',
  'SELECT generate_series(1::int4,2::int4) AS a,generate_series(10::int8,12::int8) AS b',
  [23, 20],
  [['1', '10'], ['2', '11'], [None, '12']]),
 ('multiple-empty', 'SELECT unnest(ARRAY[]::int4[]) AS a,generate_series(2::int8,1::int8) AS b', [23, 20], []),
 ('multiple-null',
  'SELECT unnest(ARRAY[NULL,NULL]::int4[]) AS a,generate_series(10::int8,10::int8) AS b',
  [23, 20],
  [[None, '10'], [None, None]]),
 ('multiple-padding',
  'SELECT unnest(ARRAY[]::int4[]) AS a,generate_series(12::int8,13::int8) AS b',
  [23, 20],
  [[None, '12'], [None, '13']]),
 ('limit-zero', 'SELECT generate_series(1::int4,2::int4,0::int4) AS v LIMIT 0', [23], []),
]
# Correlated arrays retain NULL elements but suppress NULL/empty arrays.
for join in ('CROSS JOIN LATERAL','LEFT JOIN LATERAL'):
    CASES.append(('array-lateral-'+join[:4],
                  'SELECT t.id AS id,u.v AS v FROM (VALUES(1::int4,ARRAY[2,NULL]::int4[]),'
                  '(2,ARRAY[]::int4[]),(3,NULL::int4[])) t(id,a) '+join+' unnest(t.a) u(v)'+
                  (' ON true' if join.startswith('LEFT') else '')+' ORDER BY t.id,u.v NULLS LAST',
                  [23,23], [['1','2'],['1',None]]+([['2',None],['3',None]] if join.startswith('LEFT') else [])))
CASES.append(('multiple-outer-restart',
              'SELECT t.id AS id,generate_series(1::int4,t.n) AS a,unnest(t.a) AS b FROM '
              '(VALUES(1::int4,2::int4,ARRAY[7]::int4[]),(2,1,ARRAY[8,NULL]::int4[])) t(id,n,a)',
              [23,23,23], [['1','1','7'],['1','2',None],['2','1','8'],['2',None,None]]))
for ty,oid in (('int4',23),('int8',20)):
    for args in ('1,NULL,1','1,2,NULL','NULL,NULL,NULL'):
        CASES.append((ty+'-null-args-'+args,
                      'SELECT g.v AS v FROM generate_series('+','.join(x+'::'+ty for x in args.split(','))+') g(v)',
                      [oid], []))
CASES.append(('cross-chunk',
              'SELECT t.id AS id,generate_series(1::int4,t.n) AS v FROM (VALUES(1::int4,1029::int4),(2,2)) t(id,n)',
              [23,23], [['1',str(i)] for i in range(1,1030)]+[['2','1'],['2','2']]))

ERRORS = [('zero-int4', 'SELECT g.v AS v FROM generate_series(1::int4,2::int4,0::int4) g(v)', '22023'),
 ('zero-int8', 'SELECT generate_series(1::int8,2::int8,0::int8) AS v', '22023'),
 ('array-syntax', "SELECT unnest('{broken}'::int4[]) AS v", '22P02'),
 ('case-srf', 'SELECT CASE WHEN false THEN generate_series(1::int4,2::int4) ELSE 1::int4 END AS v', '0A000'),
 ('coalesce-srf', 'SELECT COALESCE(1::int4,generate_series(1::int4,2::int4)) AS v', '0A000')]
WIDTH={20:8,23:4}
TOTAL=3+len(CASES)*4+4+64+len(ERRORS)*3+4

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
        for sql in ('SELECT generate_series(1,2) AS v', 'SELECT v FROM generate_series(1,2) g(v)', 'SELECT unnest(ARRAY[1,2]) AS v'):
            obs=save('mysql',label,'srf-reject',mysql.query(sql))
            assert obs['code']!=0 and obs['state'],obs
            prepared=save('mysql',label,'srf-prepare-reject',mysql.prepared(sql,[()]))
            assert prepared[-1]['code']!=0 and prepared[-1]['state'],prepared
            if prepared[0]['code']==0:
                assert len(prepared)==2 and prepared[0]['parameters']==0,prepared
        obs=save('mysql',label,'native-query',mysql.query('SELECT CAST(7 AS SIGNED) AS v'))
        assert obs['code']==0 and obs['rows']==[['37']],obs
        prepared=save('mysql',label,'native-prepared',mysql.prepared('SELECT CAST(? AS SIGNED) AS v',[b'7',None,b'9']))
        assert prepared[0]['code']==0 and prepared[0]['parameters']==1,prepared
        assert [x['rows'] for x in prepared[1:]]==[[['37']],[[None]],[['39']]],prepared
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference login failed (DSN redacted)'
        identity={s:query(admin,s) for s in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW TimeZone','SHOW extra_float_digits','SHOW lc_numeric')}
        save('reference','identity','setup',identity)
        assert identity['SHOW server_version_num']['rows']==[['180006'.encode().hex()]],'PG18.6 required; other versions are separate evidence'
        binary=Path(os.environ['PG09_REFERENCE_BIN'])/'postgres'
        save('build','identity','setup',dict(reference_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),reference_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG09_30_RUST_BINARY']).read_bytes()).hexdigest()))
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
            name=('srf_'+label).encode()
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
        for shape in ('series4','series8','array-from','array-projection'):
            array = shape.startswith('array')
            wide = shape == 'series8'
            parameter_oid = 1007 if array else 20 if wide else 23
            sql = ('SELECT u.v AS v FROM unnest($1::int4[]) u(v)' if shape == 'array-from' else
                   'SELECT unnest($1::int4[]) AS v' if array else
                   'SELECT generate_series($1::int8,$1::int8) AS v' if wide else
                   'SELECT generate_series(1::int4,$1::int4) AS v')
            name=('param_'+shape).encode();oids=[20 if wide else 23]
            def prepare_param():
                for side,conn in conns.items():
                    prepared=save(side,shape,'libpq-Parse',pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))
                    assert prepared['status']==1,prepared
                    desc=save(side,'params','statement-describe',pq.observe(pq.PQdescribePrepared(conn,name)))
                    assert desc['parameters']==[parameter_oid],desc
                    metadata(desc,oids)
                for side,w in wires.items():
                    assert w.phase(b'P',name+b'\0'+sql.encode()+b'\0\0\0','1')['tags']=='1'
                    desc=save(side,'params','raw-statement-describe',w.phase(b'D',b'S'+name+b'\0','Tn'))
                    assert desc['parameters']==[parameter_oid],desc
                    metadata(desc,oids)
            attempt('param-prepare-'+shape,prepare_param)
            for infmt in (0,1):
              for outfmt in (0,1):
               for value in (([2,None,2],None,[],[None,None],[7]) if array else (2,None,-1)):
                label=f'bind-{shape}-{infmt}-{outfmt}-{value}'
                def bind():
                    if array and value is not None:
                        encoded = ((struct.pack('!iii',1,int(None in value),23)+struct.pack('!ii',len(value),1)+
                                    b''.join(struct.pack('!i',-1) if v is None else struct.pack('!ii',4,v) for v in value))
                                   if infmt else ('{'+','.join('NULL' if v is None else str(v) for v in value)+'}').encode())
                        expected_rows = [[None if v is None else str(v)] for v in value]
                    else:
                        encoded=None if value is None else struct.pack('!q' if wide else '!i',value) if infmt else str(value).encode()
                        expected_rows=[] if value is None else [[str(value)]] if wide else [[str(v)] for v in range(1,value+1)]
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
                        assert obs['ready']=='I' and obs['tags']==('D'*len(expected_rows)+'CZ') and not obs['metadata'],obs
                        assert obs['rows']==peers[side]['rows'],obs
                        assert obs['lengths']==[[-1 if v is None else len(bytes.fromhex(v)) for v in row] for row in obs['rows']],obs
                    compare_wire(raw['reference'],raw['A']);compare_wire(raw['reference'],raw['B'])
                attempt(label,bind)
        # Explicit transactions retain named portals across Flush/Execute.
        # Complete one long result and abandon another, then reuse its name.
        for fmt in (0,1):
          for abandon in (False,True):
            def lifecycle():
                peers={}
                for side,w in wires.items():
                    label=f'portal-{fmt}-{abandon}'
                    assert w.query('BEGIN')['ready']=='T'
                    name=b'life';portal=b'cursor'
                    sql=CASES[-1][1];oids=CASES[-1][2];rows=CASES[-1][3]
                    assert w.phase(b'P',name+b'\0'+sql.encode()+b'\0\0\0','1')['tags']=='1'
                    body=portal+b'\0'+name+b'\0'+struct.pack('!HHHH',0,0,1,fmt)
                    assert w.phase(b'B',body,'2')['tags']=='2'
                    desc=save(side,label,'portal-before',w.phase(b'D',b'P'+portal+b'\0','T'))
                    metadata(desc,oids,fmt)
                    w.send(b'E',portal+b'\0'+struct.pack('!I',257));w.send(b'H')
                    first=save(side,label,'suspended',w.read('s'))
                    assert first['tags']=='D'*257+'s' and len(first['rows'])==257,first
                    assert w.phase(b'D',b'P'+portal+b'\0','T')['metadata']==desc['metadata']
                    collected=first['rows']
                    if not abandon:
                        while True:
                            w.send(b'E',portal+b'\0'+struct.pack('!I',257));w.send(b'H')
                            page=save(side,label,'resume',w.read('sC'))
                            assert not page['errors'] and not page['metadata'],page
                            collected+=page['rows']
                            if page['tags'].endswith('C'):break
                        check(dict(status=2,metadata=desc['metadata'],rows=collected),oids,rows,fmt)
                    assert w.phase(b'C',b'P'+portal+b'\0','3')['tags']=='3'
                    # Closing the pending portal must not leave a stale cursor.
                    assert w.phase(b'B',body,'2')['tags']=='2'
                    assert w.phase(b'C',b'P'+portal+b'\0','3')['tags']=='3'
                    assert w.phase(b'C',b'S'+name+b'\0','3')['tags']=='3'
                    assert save(side,label,'Sync',w.sync())['ready']=='T'
                    assert w.query('COMMIT')['ready']=='I'
                    assert w.query('SELECT 1::int4 AS v')['rows']==[['31']]
                    peers[side]=collected
                assert peers['reference']==peers['A']==peers['B'],peers
            attempt(f'lifecycle-{fmt}-{abandon}',lifecycle)
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
