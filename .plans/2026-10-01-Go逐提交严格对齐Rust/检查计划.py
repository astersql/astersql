from pathlib import Path
from collections import defaultdict
import json, re, subprocess
p = Path(__file__).resolve().parent
repo = p.parent.parent
facts = json.loads((p / '提交事实.json').read_text())
source = subprocess.check_output(['git', 'rev-list', 'ad193e964b^1..ad193e964b^2'], cwd=repo, text=True).splitlines()
assert len(facts) == 359 and {f['提交'] for f in facts} == set(source)
files = list(p.glob('[0-9]*-提交-*-严格对齐.md'))
assert len(files) == 359
prompt = (p / 'prompt.md').read_text()
assert len(re.findall(r'^## 任务 ', prompt, re.M)) == 359
graph = json.loads((p / '并行调度.json').read_text())
assert len(graph['nodes']) == 359 and graph['max_parallel'] == 10
nodes = {n['id']: n for n in graph['nodes']}
batch_files = defaultdict(set)
for f in facts:
    n = f['编号']; sha = f['提交']; name = f"{n}-提交-{sha[:10]}-严格对齐.md"
    text = (p / name).read_text()
    assert sha in text and prompt.count('执行 `'+str(p.relative_to(repo)/name)+'`。') == 1
    assert ('## 测试计划' in text) if f['Go路径'] else ('## 验证计划' in text)
    assert re.search(r'^状态：(未开始|进行中|已完成|已阻塞)$', text, re.M)
    for h in ['## Progress', '## Decision Log', '## Outcomes', '## 文件冲突与并行执行']:
        assert h in text
    for manifest in f['现存Cargo候选']:
        assert (repo / manifest).exists(), manifest
    node = nodes[n]; footprint = set(node['footprint'])
    assert not batch_files[node['batch']].intersection(footprint), n
    batch_files[node['batch']].update(footprint)
    assert f"执行批次：【批次 {node['batch']}】" in text
    assert f"## 任务 {n}: 提交 {sha[:10]} 严格对齐 Rust【批次 {node['batch']}】" in prompt
for edge in graph['edges']:
    a, b = nodes[edge['source']], nodes[edge['target']]
    assert a['id'] < b['id'] and a['batch'] < b['batch']
    assert set(edge['files']) <= set(a['footprint']) & set(b['footprint'])
    assert a['id'] in b['predecessors']
listed = [n for batch in graph['batches'] for n in batch['tasks']]
assert sorted(listed) == list(range(1, 360))
# Ensure every successive use of each shared file has an explicit order edge.
file_uses = defaultdict(list)
for n in graph['nodes']:
    for path in n['footprint']:
        file_uses[path].append(n['id'])
edge_map = {(e['source'], e['target']): set(e['files']) for e in graph['edges']}
for path, ids in file_uses.items():
    for a, b in zip(ids, ids[1:]):
        assert path in edge_map[(a,b)]
for file in [p/'prompt.md',p/'依赖与并行批次.md',*files]:
    assert not any(x in file.read_text() for x in ['TBD', 'TODO'])
    result = subprocess.run(['git','diff','--no-index','--check','/dev/null',str(file)],capture_output=True,text=True)
    assert result.returncode in (0,1) and not result.stdout and not result.stderr, file
print(f"通过：359来源/任务/提示一一对应；{len(graph['batches'])}批次内部无候选文件交集；{len(graph['edges'])}条边保留文件顺序；独立状态及Markdown检查通过")
