# Copyright 2026 AsterSQL.
"""Real TiKV catalog identity lifecycle. Run from repository root with --binary.

Owns a fresh playground tag and its SQL processes (or an explicitly selected
fresh PostgreSQL reference cluster). Evidence is written under
--evidence (default target/pg13-041-<random>); TiKV data is deleted after exit.
CARGO_TARGET_DIR must name a currently leased repository slot.
"""
import argparse
import ctypes as c
import ctypes.util
import json
import math
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import time
import urllib.request


def api(lib, name, result, *args):
    fn = getattr(lib, name)
    fn.restype, fn.argtypes = result, args
    return fn


library = os.environ.get('PG_LIBPQ_LIBRARY') or ctypes.util.find_library('pq')
if not library and Path('/opt/homebrew/opt/libpq/lib/libpq.dylib').exists():
    library = '/opt/homebrew/opt/libpq/lib/libpq.dylib'
assert library, 'libpq18 library required'
pq = c.CDLL(library)
p, s, i = c.c_void_p, c.c_char_p, c.c_int
connect = api(pq, 'PQconnectdb', p, s)
connected = api(pq, 'PQstatus', i, p)
finish = api(pq, 'PQfinish', None, p)
execute = api(pq, 'PQexec', p, p, s)
prepare = api(pq, 'PQprepare', p, p, s, s, i, c.POINTER(c.c_uint))
describe = api(pq, 'PQdescribePrepared', p, p, s)
prepared = api(pq, 'PQexecPrepared', p, p, s, i, c.POINTER(s), c.POINTER(i), c.POINTER(i), i)
status = api(pq, 'PQresultStatus', i, p)
error = api(pq, 'PQresultErrorField', s, p, i)
rows = api(pq, 'PQntuples', i, p)
cols = api(pq, 'PQnfields', i, p)
value = api(pq, 'PQgetvalue', s, p, i, i)
null = api(pq, 'PQgetisnull', i, p, i, i)
clear = api(pq, 'PQclear', None, p)
transaction = api(pq, 'PQtransactionStatus', i, p)
command = api(pq, 'PQcmdStatus', s, p)
fields = {key: api(pq, name, kind, p, i) for key, name, kind in [
    ('name', 'PQfname', s), ('oid', 'PQftype', c.c_uint), ('size', 'PQfsize', i),
    ('mod', 'PQfmod', i), ('format', 'PQfformat', i), ('table', 'PQftable', c.c_uint),
    ('attribute', 'PQftablecol', i)]}


class Client:
    def __init__(self, port, database):
        self.conn = connect(f'host=127.0.0.1 port={port} user=root dbname={database} sslmode=disable gssencmode=disable connect_timeout=3'.encode())
        if not self.conn or connected(self.conn):
            self.close()
            raise ConnectionError('libpq startup failed')
        self.serial = 0

    def close(self):
        if self.conn:
            finish(self.conn)
            self.conn = None

    def observe(self, result):
        assert result, 'libpq returned no result'
        try:
            metadata = [{k: fn(result, n) for k, fn in fields.items()} for n in range(cols(result))]
            for field in metadata:
                field['name'] = field['name'].decode()
            return dict(status=status(result), fields=metadata,
                        rows=[[None if null(result, r, n) else value(result, r, n).decode()
                               for n in range(cols(result))] for r in range(rows(result))],
                        command=(command(result) or b'').decode(),
                        sqlstate=(error(result, ord('C')) or b'').decode(),
                        message=(error(result, ord('M')) or b'').decode(),
                        transaction=transaction(self.conn))
        finally:
            clear(result)

    def run(self, sql, expected=2, state=None):
        out = self.observe(execute(self.conn, sql.encode()))
        record(dict(protocol='Q', sql=sql, result=out))
        assert out['status'] == expected and out['transaction'] == 0, (sql, out)
        if state:
            assert out['sqlstate'] == state, (sql, out)
        if expected == 2:
            assert out['command'] == f'SELECT {len(out["rows"])}', out
        elif expected == 1:
            tag = 'DEALLOCATE' if sql.startswith('DEALLOCATE ') else ' '.join(sql.upper().split()[:2])
            if sql.startswith('CREATE UNIQUE INDEX'):
                tag = 'CREATE INDEX'
            assert out['command'] == tag, (sql, out)
        return out

    def step(self, sql, expected_status, expected_tx, expected_command='', state=''):
        out = self.observe(execute(self.conn, sql.encode()))
        record(dict(phase='transaction recovery', sql=sql, result=out))
        assert (out['status'], out['transaction'], out['command'], out['sqlstate']) == (
            expected_status, expected_tx, expected_command, state), out
        return out

    def extended(self, sql, arg=None, parameter=False, state=None):
        self.serial += 1
        name = f's{self.serial}'.encode()
        types = (c.c_uint * 1)(26) if parameter else None
        parsed = self.observe(prepare(self.conn, name, sql.encode(), int(parameter), types))
        record(dict(phase='Parse', sql=sql, result=parsed))
        assert parsed['status'] == 1, parsed
        desc = self.observe(describe(self.conn, name))
        record(dict(phase='Describe', sql=sql, result=desc))
        assert desc['status'] == 1, desc
        vals = (s * 1)(None if arg is None else arg.encode()) if parameter else None
        out = self.observe(prepared(self.conn, name, int(parameter), vals, None, None, 0))
        record(dict(phase='Bind Execute Sync', sql=sql, parameter=arg, result=out))
        assert out['status'] == (7 if state else 2) and out['transaction'] == 0, out
        if state:
            assert out['sqlstate'] == state, out
        else:
            assert out['fields'] == desc['fields'], (out, desc)
            assert out['command'] == f'SELECT {len(out["rows"])}', out
        self.run('DEALLOCATE ' + name.decode(), 1)
        return out


def record(item):
    print(json.dumps(item, ensure_ascii=False), file=evidence, flush=True)


def free_ports():
    # TiUP assigns PD peer/TiKV status ports from port-offset even when the
    # client ports are explicit; reserve the whole known component port set.
    for _ in range(100):
        offset = 10000 + secrets.randbelow(20000)
        ports = [port + offset for port in (2379, 20160, 20180, 4000, 5432, 10080, 2380)]
        holders = []
        try:
            for port in ports:
                sock = socket.socket()
                holders.append(sock)
                sock.bind(('127.0.0.1', port))
            return offset, ports
        except OSError:
            pass
        finally:
            for sock in holders:
                sock.close()
    raise RuntimeError('no free isolated port set')


def wait_for(check, proc, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        assert proc.poll() is None, ('process exited', proc.args, proc.returncode)
        try:
            if check():
                return
        except (OSError, ValueError, ConnectionError):
            pass
        time.sleep(.2)
    raise TimeoutError(('readiness timeout', proc.args))


def http_json(port, path):
    with urllib.request.urlopen(f'http://127.0.0.1:{port}{path}', timeout=2) as response:
        return json.load(response)


def listening(port):
    with socket.socket() as sock:
        sock.settimeout(.2)
        return sock.connect_ex(('127.0.0.1', port)) == 0


def stop(proc):
    if proc is None:
        return
    for sig, timeout in ((signal.SIGINT, 30), (signal.SIGTERM, 10), (signal.SIGKILL, 10)):
        if proc.poll() is not None:
            break
        os.killpg(proc.pid, sig)
        try:
            proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            continue
    assert proc.poll() is not None, ('owned process survived SIGKILL', proc.pid)
    # A wrapper can exit before its PD/TiKV descendants. Signal only its own
    # freshly created process group and wait for group disappearance.
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 30
    while True:
        try:
            os.killpg(proc.pid, 0)
        except ProcessLookupError:
            break
        if time.monotonic() >= deadline:
            os.killpg(proc.pid, signal.SIGKILL)
            time.sleep(.2)
            try:
                os.killpg(proc.pid, 0)
            except ProcessLookupError:
                break
            raise TimeoutError(('owned process group survived SIGKILL', proc.pid))
        time.sleep(.1)
    record(dict(stopped_pid=proc.pid, exit_code=proc.returncode, process_group_gone=True))


def start_sql():
    global sql_process
    cmd = [str(binary), '--store=tikv', f'--path=127.0.0.1:{ports[0]}',
           '--host=127.0.0.1', f'-P={ports[3]}', f'--postgres-port={ports[4]}',
           '--report-status=true', '--status-host=127.0.0.1', f'--status={ports[5]}', '--socket=']
    if reference:
        cmd = [str(reference), '-D', str(data_dir), '-h', '127.0.0.1', '-p', str(ports[4]), '-k', '']
    log = (archive / f'sql-{len(pids)}.log').open('w')
    logs.append(log)
    sql_process = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    pids.append(sql_process.pid)
    record(dict(start_sql=cmd, pid=sql_process.pid))
    wait_for(lambda: listening(ports[4]), sql_process)


def restart():
    for conn in clients:
        conn.close()
    clients.clear()
    stop(sql_process)
    assert not listening(ports[4]), 'SQL listener survived shutdown'
    if not reference:
        assert http_json(ports[0], '/pd/api/v1/health'), 'PD must stay alive across SQL restart'
    else:
        assert data_dir.is_dir(), 'same PostgreSQL storage must survive restart'
    start_sql()
    assert len(set(pids)) == len(pids)


def open_db(db):
    conn = Client(ports[4], db)
    clients.append(conn)
    return conn


def scalar(conn, sql):
    out = conn.run(sql)
    assert len(out['rows']) == 1 and len(out['rows'][0]) == 1, out
    return out['rows'][0][0]


# Projections avoid unsupported catalog fields but cover every required endpoint.
def queries(conn, schema, table, view):
    # The catalog binder deliberately accepts bounded joins, not scalar subqueries.
    relations = f"SELECT c.oid,c.relname,c.relnamespace,c.relkind FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='{schema}' ORDER BY c.oid"
    rels = f"'{schema}.{table}'::regclass::oid,'{schema}.child'::regclass::oid,'{schema}.{view}'::regclass::oid,'{schema}.ia'::regclass::oid"
    dependency_filter = ''
    if reference:
        # PostgreSQL also stores its own bootstrap dependencies. Scope the
        # reference probe to this story's real sequence/default endpoints.
        seqs = conn.run(f"SELECT s.seqrelid FROM pg_catalog.pg_sequence s JOIN pg_catalog.pg_class c ON c.oid=s.seqrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='{schema}'")['rows']
        defs = conn.run(f"SELECT d.oid FROM pg_catalog.pg_attrdef d JOIN pg_catalog.pg_class c ON c.oid=d.adrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='{schema}'")['rows']
        def ids(rows):
            assert rows and all(int(row[0]) > 0 for row in rows)
            return ','.join(str(int(row[0])) for row in rows)
        dependency_filter = f' WHERE (classid=1259 AND objid IN ({ids(seqs)})) OR (classid=2604 AND objid IN ({ids(defs)}))'
    return {
        'database': f"SELECT oid,datname FROM pg_catalog.pg_database WHERE datname='{database}'",
        'namespace': f"SELECT oid,nspname FROM pg_catalog.pg_namespace WHERE nspname='{schema}'",
        'class': relations,
        'attribute': f'SELECT attrelid,attnum,attname,atttypid,attisdropped,atthasdef,attidentity FROM pg_catalog.pg_attribute WHERE attrelid IN ({rels}) AND attnum>0 ORDER BY attrelid,attnum',
        'index': f'SELECT indexrelid,indrelid,indkey,indisunique,indisprimary FROM pg_catalog.pg_index WHERE indrelid IN ({rels}) ORDER BY indexrelid',
        'constraint': f'SELECT oid,conname,connamespace,conrelid,confrelid,conindid,conkey,confkey,contype FROM pg_catalog.pg_constraint WHERE conrelid IN ({rels}) ORDER BY oid',
        'attrdef': f'SELECT oid,adrelid,adnum FROM pg_catalog.pg_attrdef WHERE adrelid IN ({rels}) ORDER BY oid',
        'sequence': 'SELECT seqrelid,seqtypid,seqstart,seqincrement,seqmin,seqmax,seqcache,seqcycle FROM pg_catalog.pg_sequence ORDER BY seqrelid',
        'depend': 'SELECT classid,objid,objsubid,refclassid,refobjid,refobjsubid,deptype FROM pg_catalog.pg_depend' + dependency_filter + ' ORDER BY classid,objid,objsubid,refclassid,refobjid,refobjsubid',
    }


EXPECTED_TYPES = {
    'database': [(26, 4), (19, 64)],
    'namespace': [(26, 4), (19, 64)],
    'class': [(26, 4), (19, 64), (26, 4), (18, 1)],
    'attribute': [(26, 4), (21, 2), (19, 64), (26, 4), (16, 1), (16, 1), (18, 1)],
    'index': [(26, 4), (26, 4), (22, -1), (16, 1), (16, 1)],
    'constraint': [(26, 4), (19, 64), (26, 4), (26, 4), (26, 4), (26, 4), (1005, -1), (1005, -1), (18, 1)],
    'attrdef': [(26, 4), (26, 4), (21, 2)],
    'sequence': [(26, 4), (26, 4)] + [(20, 8)] * 5 + [(16, 1)],
    'depend': [(26, 4), (26, 4), (23, 4), (26, 4), (26, 4), (23, 4), (18, 1)],
}


def snapshot(conn, schema='life', table='parent', view='v'):
    # Fresh cross-database Execute after every observed DDL/restart phase.
    observer = open_db(database + '_b')
    try:
        isolated_now = observer.run("SELECT oid,relname FROM pg_catalog.pg_class WHERE relname='parent'")
        assert isolated_now == isolated, (isolated_now, isolated)
        record(dict(cross_database=database + '_b', unchanged=True, result=isolated_now))
    finally:
        observer.close()
    if not reference:
        record(dict(shared_models=http_json(ports[5], f'/schema/{database}')))
    result = {}
    for key, query in queries(conn, schema, table, view).items():
        out = conn.run(query)
        assert out['rows'], ('missing positive endpoint', key, out)
        assert [(f['oid'], f['size']) for f in out['fields']] == EXPECTED_TYPES[key], (key, out)
        assert all(f['mod'] == -1 and f['format'] == 0 for f in out['fields']), out
        assert conn.extended(query) == out, ('Q/extended mismatch', key)
        result[key] = out
    # pg_oid uses a checked bijection of durable model IDs; record its inverse
    # explicitly as derived IDs, rather than inventing an independent allocator.
    def signed_id(encoded):
        return encoded // 2 if encoded % 2 == 0 else -(encoded + 1) // 2
    native = {}
    for oid, name, namespace, kind in ([] if reference else result['class']['rows']):
        number = int(oid)
        if kind == 'i':
            paired = number - 0x80000000
            diagonal = (math.isqrt(8 * paired + 1) - 1) // 2
            index = paired - diagonal * (diagonal + 1) // 2
            native[name] = dict(table_id=signed_id(diagonal - index), index_id=signed_id(index))
        else:
            native[name] = dict(table_id=signed_id(number - 0x40000000))
    record(dict(snapshot=result, derived_model_ids=native,
                id_source='inverse of checked pg_oid model-ID bijection; not a direct model dump'))
    validate_links(result)
    return result


def validate_links(snapshot):
    data = {k: v['rows'] for k, v in snapshot.items()}
    ns = data['namespace'][0][0]
    classes = {r[0]: r for r in data['class']}
    attrs = {(r[0], r[1]) for r in data['attribute']}
    assert all(r[2] == ns for r in classes.values())
    assert {'r', 'v', 'S', 'i'} <= {r[3] for r in classes.values()}, classes
    for index, parent, key, *_ in data['index']:
        assert index in classes and parent in classes
        assert all((parent, n) in attrs for n in key.split()), (parent, key)
    indexes = {r[0] for r in data['index']}
    index_parents = {r[0]: r[1] for r in data['index']}
    for oid, name, namespace, parent, foreign, index, key, fkey, kind in data['constraint']:
        assert int(oid) > 0 and namespace == ns and parent in classes
        if key:
            assert all((parent, n) in attrs for n in key.strip('{}').split(',') if n), (parent, key)
        if kind == 'f':
            assert foreign in classes and index in indexes and key and fkey
            assert index_parents[index] == foreign
            assert all((foreign, n) in attrs for n in fkey.strip('{}').split(',') if n), (foreign, fkey)
        if kind in ('p', 'u'):
            assert index in indexes and index_parents[index] == parent
    assert {'p', 'f', 'c', 'n'} <= {r[-1] for r in data['constraint']}, data['constraint']
    for oid, parent, number in data['attrdef']:
        assert int(oid) > 0 and (parent, number) in attrs
    seqids = {r[0] for r in data['sequence']}
    assert len(seqids) == 2 and all(classes[x][3] == 'S' for x in seqids)
    for edge in data['depend']:
        if edge[3] == '1259':
            assert edge[4] in classes
            assert edge[5] == '0' or (edge[4], edge[5]) in attrs
    for sid in seqids:
        edges = [r for r in data['depend'] if r[0] == '1259' and r[1] == sid]
        assert any(r[3:] == ['2615', ns, '0', 'n'] for r in edges), edges
        assert any(r[3] == '1259' and (r[4], r[5]) in attrs and r[6] in ('a', 'i') for r in edges), edges


def semantic_summary(snapshot):
    data = {k: v['rows'] for k, v in snapshot.items()}
    internal = {r[1] for r in data['depend'] if r[0] == '1259' and r[6] == 'i'}
    names = {r[0]: ('identity_sequence' if r[0] in internal else r[1]) for r in data['class']}
    def relation(oid):
        return None if oid == '0' else names[oid]
    return {
        'metadata': {key: value['fields'] for key, value in snapshot.items()},
        'classes': sorted((names[r[0]], r[3]) for r in data['class']),
        'attributes': sorted((names[r[0]], r[1], 'dropped' if r[4] == 't' else r[2], *r[3:]) for r in data['attribute']),
        'indexes': sorted((names[r[1]], *r[2:]) for r in data['index']),
        'constraints': sorted((r[8], relation(r[3]), relation(r[4]), r[6], r[7]) for r in data['constraint']),
        'defaults': sorted((names[r[1]], r[2]) for r in data['attrdef']),
        'sequences': sorted((names[r[0]], *r[1:]) for r in data['sequence']),
        'ownership': sorted((names[r[1]], names[r[4]], r[5], r[6]) for r in data['depend'] if r[0] == '1259' and r[3] == '1259' and r[6] in ('a', 'i')),
    }


def interrupted(signum, _frame):
    raise SystemExit(128 + signum)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--evidence', type=Path)
    parser.add_argument('--reference-postgres', type=Path, help='Run the identical story on a fresh, owned PostgreSQL 18 reference instead of TiKV')
    args = parser.parse_args()
    binary = args.binary
    reference = args.reference_postgres
    if reference:
        assert reference.is_absolute() and reference.is_file()
        assert 'PostgreSQL) 18.' in subprocess.check_output([str(reference), '--version'], text=True)
    assert binary.is_absolute() and binary.is_file(), binary
    target = Path(os.environ['CARGO_TARGET_DIR'])
    assert target.is_absolute() and target.parent == Path.cwd() / 'target'
    assert target.name in [f'rust-slot-{n}' for n in range(1, 11)]
    assert (target.parent / 'rust-slot-locks' / f'slot-{target.name.split("-")[-1]}.lock').is_dir()
    assert binary.parent == target / 'debug'
    assert api(pq, 'PQlibVersion', i)() >= 180000, 'libpq18 required'
    token = secrets.token_hex(6)
    tag = f'astersql-pg13-041-{token}'
    database = f'pg41_{token}'
    archive = args.evidence or Path('target') / f'pg13-041-{token}'
    archive.mkdir(parents=True, exist_ok=False)
    evidence = (archive / 'observations.jsonl').open('w')
    data_dir = Path(os.environ.get('TIUP_HOME', str(Path.home() / '.tiup'))) / 'data' / tag
    if reference:
        data_dir = archive.resolve() / 'reference-data'
    assert not data_dir.exists(), 'fresh storage required'
    offset, ports = free_ports()
    tiup = shutil.which('tiup') or str(Path.home() / '.tiup/bin/tiup')
    playground = sql_process = None
    pids, logs, clients = [], [], []
    count = 0
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, interrupted)
    try:
        cmd = [tiup, 'playground', 'v8.5.1', '--mode', 'tikv-slim', '--tag', tag,
               '--host', '127.0.0.1', '--port-offset', str(offset),
               '--without-monitor', '--kv.config', str(archive.resolve() / 'tikv.toml')]
        # Bound status ports and lower memory for a single isolated test store.
        (archive / 'tikv.toml').write_text(f'[server]\nstatus-addr="127.0.0.1:{ports[2]}"\n[storage]\nreserve-space="0GB"\n[rocksdb.defaultcf]\nblock-cache-size="64MB"\n')
        log = (archive / 'playground.log').open('w')
        logs.append(log)
        if reference:
            init = [str(reference.with_name('initdb')), '-D', str(data_dir), '-U', 'root', '-A', 'trust', '--no-locale', '--encoding=UTF8']
            subprocess.run(init, stdout=log, stderr=subprocess.STDOUT, check=True)
            record(dict(reference_version=subprocess.check_output([str(reference), '--version'], text=True).strip(), init=init))
        else:
            playground = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            record(dict(playground=cmd, pid=playground.pid, ports=ports, tag=tag, cargo_target_dir=str(target)))
            wait_for(lambda: http_json(ports[0], '/pd/api/v1/health') and any(
                store['store']['state_name'] == 'Up' for store in http_json(ports[0], '/pd/api/v1/stores').get('stores', [])), playground)
        start_sql()
        admin = open_db('postgres' if reference else 'mysql')
        record(dict(server_version=scalar(admin, 'SELECT version()'), libpq=api(pq, 'PQlibVersion', i)(),
                    head=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), reference=bool(reference)))
        admin.run(f'CREATE DATABASE {database}', 1)
        admin.run(f'CREATE DATABASE {database}_b', 1)
        first, second = open_db(database), open_db(database)
        other = open_db(database + '_b')
        other.run('CREATE SCHEMA life', 1)
        other.run('CREATE TABLE life.parent (id integer)', 1)
        isolated = other.run("SELECT oid,relname FROM pg_catalog.pg_class WHERE relname='parent'")
        for sql in [
            'CREATE SCHEMA life',
            'CREATE TABLE life.parent (id integer NOT NULL, gone integer, qty integer DEFAULT 7, CONSTRAINT life_pk PRIMARY KEY(id), CONSTRAINT life_ck CHECK(qty>0))',
            'CREATE INDEX life_ix ON life.parent(qty)',
            'CREATE TABLE life.child (id integer, parent_id integer, CONSTRAINT life_fk FOREIGN KEY(parent_id) REFERENCES life.parent(id))',
            'CREATE VIEW life.v AS SELECT id,qty FROM life.parent',
            'CREATE SEQUENCE life.s AS bigint START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE OWNED BY life.parent.qty',
            'CREATE TABLE life.ia (id integer GENERATED ALWAYS AS IDENTITY)',
            'ALTER TABLE life.parent DROP COLUMN gone',
            'ALTER TABLE life.parent ADD COLUMN added integer',
        ]:
            first.run(sql, 1)
        assert scalar(first, "SELECT nextval('life.s'::regclass)") == '7'
        baseline = snapshot(first)
        parent_oid = scalar(first, "SELECT 'life.parent'::regclass::oid")
        parent_attrs = [r for r in baseline['attribute']['rows'] if r[0] == parent_oid]
        assert [r[1] for r in parent_attrs] == ['1', '2', '3', '4'], parent_attrs
        assert parent_attrs[1][3:5] == ['0', 't'], parent_attrs
        assert parent_attrs[2][2] == 'qty' and parent_attrs[3][2] == 'added', parent_attrs
        assert snapshot(second) == baseline
        assert scalar(first, "SELECT currval('life.s'::regclass)") == '7'
        assert scalar(first, "SELECT nextval('life.s'::regclass)") == '10'
        assert isolated['rows'][0][0] not in {r[0] for r in baseline['class']['rows']}
        lookup = 'SELECT oid,relname FROM pg_catalog.pg_class WHERE oid=$1'
        old_table = scalar(first, "SELECT oid FROM pg_catalog.pg_class WHERE relname='parent'")
        assert len(first.extended(lookup, old_table, True)['rows']) == 1
        for arg in ('0', None):
            assert not first.extended(lookup, arg, True)['rows']
        for arg, state in [('bad', '22P02'), ('4294967296', '22003')]:
            first.extended(lookup, arg, True, state)
        first.step('BEGIN', 1, 2, 'BEGIN')
        first.step("SELECT oid FROM pg_catalog.pg_class WHERE oid='bad'::oid", 7, 3, state='22P02')
        first.step('SELECT 1', 7, 3, state='25P02')
        first.step('ROLLBACK', 1, 0, 'ROLLBACK')
        assert first.run('SELECT 1')['rows'] == [['1']]
        count += 1
        restart()
        first, second = open_db(database), open_db(database)
        other = open_db(database + '_b')
        assert snapshot(first) == snapshot(second) == baseline
        assert scalar(first, "SELECT nextval('life.s'::regclass)") == '13'
        assert other.run("SELECT oid,relname FROM pg_catalog.pg_class WHERE relname='parent'") == isolated
        count += 1
        for sql in ['ALTER TABLE life.parent RENAME TO renamed',
                    'ALTER TABLE life.renamed RENAME COLUMN qty TO amount',
                    'ALTER VIEW life.v RENAME TO renamed_v',
                    'ALTER SCHEMA life RENAME TO renamed_ns']:
            first.run(sql, 1)
        renamed = snapshot(first, 'renamed_ns', 'renamed', 'renamed_v')
        assert snapshot(second, 'renamed_ns', 'renamed', 'renamed_v') == renamed
        for key, column in [('database', 0), ('namespace', 0), ('class', 0), ('constraint', 0), ('attrdef', 0), ('attribute', 0), ('index', 0), ('sequence', 0), ('depend', 1)]:
            assert [r[column] for r in baseline[key]['rows']] == [r[column] for r in renamed[key]['rows']], key
        assert scalar(first, "SELECT 'renamed_ns.renamed'::regclass::oid") == old_table
        restart()
        first, second = open_db(database), open_db(database)
        assert snapshot(first, 'renamed_ns', 'renamed', 'renamed_v') == renamed
        assert snapshot(second, 'renamed_ns', 'renamed', 'renamed_v') == renamed
        count += 1
        for sql in ['DROP VIEW renamed_ns.renamed_v', 'DROP TABLE renamed_ns.child',
                    'DROP TABLE renamed_ns.renamed', 'DROP TABLE renamed_ns.ia',
                    'DROP SCHEMA renamed_ns', 'CREATE SCHEMA life',
                    'CREATE TABLE life.parent (id integer NOT NULL, qty integer DEFAULT 9, CONSTRAINT life_pk PRIMARY KEY(id), CONSTRAINT life_ck CHECK(qty>0))',
                    'CREATE INDEX life_ix ON life.parent(qty)',
                    'CREATE TABLE life.child (id integer, parent_id integer, CONSTRAINT life_fk FOREIGN KEY(parent_id) REFERENCES life.parent(id))',
                    'CREATE VIEW life.v AS SELECT id,qty FROM life.parent',
                    'CREATE SEQUENCE life.s AS bigint START WITH 7 INCREMENT BY 3 MINVALUE 1 MAXVALUE 100 CACHE 1 NO CYCLE OWNED BY life.parent.qty',
                    'CREATE TABLE life.ia (id integer GENERATED ALWAYS AS IDENTITY)']:
            first.run(sql, 1)
        rebuilt = snapshot(first)
        assert snapshot(second) == rebuilt
        for key in ('namespace', 'class', 'constraint', 'attrdef'):
            assert {r[0] for r in baseline[key]['rows']}.isdisjoint(r[0] for r in rebuilt[key]['rows']), key
        assert not first.extended(lookup, old_table, True)['rows']
        assert rebuilt['database'] == baseline['database']
        old_relations = {r[0] for r in baseline['class']['rows']}
        old_definitions = {r[0] for r in baseline['attrdef']['rows']}
        for edge in rebuilt['depend']['rows']:
            assert not (edge[0] == '1259' and edge[1] in old_relations)
            assert not (edge[0] == '2604' and edge[1] in old_definitions)
            assert not (edge[3] == '1259' and edge[4] in old_relations)
            assert not (edge[3] == '2615' and edge[4] == baseline['namespace']['rows'][0][0])
        restart()
        first, second = open_db(database), open_db(database)
        assert snapshot(first) == snapshot(second) == rebuilt
        assert not first.extended(lookup, old_table, True)['rows']
        count += 1
        (archive / 'semantic-summary.json').write_text(json.dumps({
            'baseline': semantic_summary(baseline), 'renamed': semantic_summary(renamed), 'rebuilt': semantic_summary(rebuilt)}, indent=2))
        record(dict(result='PASS', effective_scenarios=count, sql_pids=pids,
                    restart_count=len(pids)-1, endpoints=list(baseline)))
    except BaseException as exc:
        record(dict(result='FAIL', effective_scenarios=count, error=repr(exc)))
        raise
    finally:
        for conn in clients:
            conn.close()
        cleanup_errors = []
        for proc in (sql_process, playground):
            try:
                stop(proc)
            except Exception as exc:
                cleanup_errors.append(repr(exc))
        # Never remove data while a child is still listening.
        deadline = time.monotonic() + 30
        while any(listening(port) for port in ports) and time.monotonic() < deadline:
            time.sleep(.2)
        closed = all(not listening(port) for port in ports)
        if closed and not cleanup_errors and data_dir.exists():
            assert (reference and data_dir == archive.resolve() / 'reference-data') or (data_dir.name == tag and tag.startswith('astersql-pg13-041-'))
            shutil.rmtree(data_dir)
        record(dict(cleanup=dict(ports_closed=closed, data_removed=not data_dir.exists(), errors=cleanup_errors)))
        for log in logs:
            log.close()
        evidence.close()
        assert closed and not data_dir.exists() and not cleanup_errors, ('cleanup failed; retained owned data', cleanup_errors)
    print(json.dumps(dict(result='PASS', effective_scenarios=count, evidence=str(archive))))
