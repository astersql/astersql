#!/usr/bin/env bash
# Copyright 2026 AsterSQL.
# Owns only a unique playground and Rust server; caller owns the Cargo slot.
set -euo pipefail
cd "$(dirname "$0")/.."
: "${PG18_REFERENCE_DSN:?PG18_REFERENCE_DSN is required (no skip)}"
: "${CARGO_TARGET_DIR:?claim a repository Cargo slot first}"
case "$CARGO_TARGET_DIR" in
  "$PWD"/target/rust-slot-[1-9]|"$PWD"/target/rust-slot-10) ;;
  *) echo 'CARGO_TARGET_DIR must be an absolute repository slot' >&2; exit 1;;
esac
slot=${CARGO_TARGET_DIR##*rust-slot-}
test -s "target/rust-slot-locks/slot-$slot.lock/owner" || { echo 'caller slot lock is missing' >&2; exit 1; }
export TIUP_BIN=${TIUP_BIN:-${HOME}/.tiup/bin/tiup}
export MYSQL_BIN=${MYSQL_BIN:-$(command -v mysql || true)}
export PROTOC=${PROTOC:-/opt/homebrew/opt/protobuf@21/bin/protoc}
test -x "$TIUP_BIN"
test -x "$MYSQL_BIN"
command -v python3 >/dev/null
cargo build --locked -p astersql-cmd-tidb-server --bin astersql-cmd-tidb-server
exec python3 - <<'PY'
import hashlib, json, os, random, shutil, signal, socket, subprocess, time, urllib.request
from pathlib import Path

root = Path.cwd()
tag = f'pg22-{os.getpid()}-{time.time_ns()}'
evidence = root / 'target' / tag
evidence.mkdir()
owned_data = Path(os.environ.get('TIUP_HOME', str(Path.home() / '.tiup'))) / 'data' / tag
assert not owned_data.exists()
processes, files, ports = [], [], []
report = dict(repository_sha=subprocess.check_output(['git','rev-parse','HEAD'], text=True).strip(),
              slot=os.environ['CARGO_TARGET_DIR'], tag=tag, stages=[], cleanup=False)

# Release archives do not identify a Git checkout. Preserve their complete
# SHA256 rather than inventing a reference source commit.
if os.environ.get('PG18_BIN'):
    archive = Path(os.environ['PG18_BIN']).resolve().parents[1] / 'downloads/postgresql-18.6.tar.bz2'
    if archive.is_file():
        report['reference_archive_sha256'] = hashlib.sha256(archive.read_bytes()).hexdigest()

def launch(args, name):
    f = (evidence / (name + '.log')).open('w')
    files.append(f)
    p = subprocess.Popen(args, stdout=f, stderr=subprocess.STDOUT, start_new_session=True)
    processes.append(p)
    return p

def stop(p, sig=signal.SIGTERM):
    if p.poll() is None:
        os.killpg(p.pid, sig)
        try:
            p.wait(timeout=20)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait(timeout=10)

def interrupted(signum, frame):
    raise RuntimeError('verification interrupted')
for sig in (signal.SIGINT, signal.SIGTERM):
    signal.signal(sig, interrupted)

def free(port):
    with socket.socket() as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.bind(('127.0.0.1',port))
            return True
        except OSError:
            return False

def wait_for(p, predicate, seconds=120):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        assert p.poll() is None, f'owned process exited {p.returncode}; see {evidence}'
        try:
            if predicate():
                return
        except (OSError, ValueError):
            pass
        time.sleep(.5)
    raise RuntimeError(f'health deadline exceeded; see {evidence}')

def get(port, path):
    with urllib.request.urlopen(f'http://127.0.0.1:{port}{path}', timeout=2) as r:
        return json.load(r)

def mysql(sql):
    # Local development credentials only; reference DSN is never an argument.
    p = launch([os.environ['MYSQL_BIN'],'--no-defaults','--protocol=TCP','--host=127.0.0.1',
        f'--port={my}','--user=root','--skip-password','--execute='+sql],
        'mysql-' + str(len(processes)))
    assert p.wait(timeout=30) == 0, 'local MySQL fixture setup/cleanup failed'

def client(case, database, restart=False):
    args = ['python3','pkg/server/testdata/pg12_transaction_libpq.py','--case',case,
            '--host','127.0.0.1','--port',str(pg),'--database',database,'--storage','tikv']
    if restart:
        args.append('--restart-check')
    p = launch(args, case + ('-restart' if restart else ''))
    try:
        code = p.wait(timeout=180)
    except subprocess.TimeoutExpired:
        stop(p)
        raise
    assert code == 0, f'{case} failed ({code}); see {evidence}'
    record = json.loads((evidence / (case + ('-restart' if restart else '') + '.log')).read_text().splitlines()[-1])
    assert record['cases_per_endpoint'] > 0 and record['cleanup'] == 'passed'
    report['stages'].append(record)

try:
    # TiUP offsets all default service/status ports; probe the whole default
    # family before selecting an offset, including PD peer and TiKV status.
    for _ in range(100):
        offset = random.randrange(1000,30000)
        cluster_ports = [n+offset for n in (2379,2380,20160,20180)]
        candidates = random.sample(range(32000,60000),3)
        if len(set(cluster_ports+candidates)) == 7 and all(map(free,cluster_ports+candidates)):
            break
    else:
        raise RuntimeError('no free private port set')
    pd, _, kv, _ = cluster_ports
    my, pg, status = candidates
    ports = cluster_ports + candidates
    report['ports'] = ports
    playground = launch([os.environ['TIUP_BIN'],'playground','v8.5.8','--mode','tikv-slim',
        '--without-monitor','--tag',tag,'--host','127.0.0.1','--port-offset',str(offset)],'playground')
    wait_for(playground, lambda: bool(get(pd,'/pd/api/v1/stores').get('stores')) and
             all(x['store']['state_name']=='Up' for x in get(pd,'/pd/api/v1/stores')['stores']))
    report['pd_version'] = get(pd,'/pd/api/v1/version')
    report['stores'] = get(pd,'/pd/api/v1/stores')
    binary = str(Path(os.environ['CARGO_TARGET_DIR']) / 'debug/astersql-cmd-tidb-server')
    server_args = [binary,'--store=tikv',f'--path=127.0.0.1:{pd}','--host=127.0.0.1',
        f'-P={my}',f'--postgres-port={pg}','--report-status=true','--status-host=127.0.0.1',
        f'--status={status}','--socket=']
    server = launch(server_args,'server')
    wait_for(server, lambda: bool(get(status,'/status')))
    commit_database = None
    for case in ('rollback','disconnect','failed_commit','commit'):
        database = f'pg22_{os.getpid()}_{case}'
        mysql(f'CREATE DATABASE {database}')
        client(case,database)
        if case == 'commit':
            commit_database = database
        else:
            mysql(f'DROP DATABASE {database}')
    stop(server)
    assert all(free(p) for p in (my,pg,status)), 'server listeners not released'
    server = launch(server_args,'server-restarted')
    wait_for(server, lambda: bool(get(status,'/status')))
    client('commit',commit_database,restart=True)
    mysql(f'DROP DATABASE {commit_database}')
    report['behavior_passed'] = True
finally:
    # Reverse launch order: clients, server, then playground. Never touch the
    # independently managed PostgreSQL reference instance.
    for p in reversed(processes):
        stop(p, signal.SIGINT if p is locals().get('playground') else signal.SIGTERM)
    for f in files:
        f.close()
    deadline = time.monotonic()+15
    while not all(map(free,ports)) and time.monotonic()<deadline:
        time.sleep(.2)
    report['cleanup'] = all(map(free,ports))
    if report['cleanup'] and owned_data.exists():
        shutil.rmtree(owned_data)
    report['playground_data_removed'] = not owned_data.exists()
    (evidence / 'lifecycle.json').write_text(json.dumps(report,indent=2))
    print(json.dumps(dict(evidence=str(evidence), cleanup=report['cleanup'],
                         stages=len(report['stages']))))
    assert report['cleanup'] and report['playground_data_removed'], 'owned resources remain'
PY
