"""Rebuild conservative file-conflict scheduling without modifying source facts or plan.md."""
from pathlib import Path
from collections import defaultdict, Counter
import json, re, hashlib, html
ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent.parent
facts = json.loads((ROOT / '提交事实.json').read_text())
assert len(facts) == 359
plan_before = hashlib.sha256((ROOT / 'plan.md').read_bytes()).hexdigest()
last = {}
nodes = []
edges = defaultdict(set)
for f in facts:
    n = f['编号']
    footprint = set(f['现存同名Rust候选'])
    footprint.update(str(Path(g).with_suffix('.rs')) for g in f['Go路径'])
    for change in f['路径变更']:
        for path in change.split('\t')[1:]:
            if not path.endswith('.go'):
                footprint.add(path)
    for manifest in f['现存Cargo候选']:
        footprint.add(manifest)
        for entry in ('lib.rs', 'mod.rs'):
            path = Path(manifest).parent / entry
            if (REPO / path).exists():
                footprint.add(str(path))
    # Include concrete files already discovered in completed/active early tasks.
    task = ROOT / f"{n}-提交-{f['提交'][:10]}-严格对齐.md"
    text = task.read_text()
    if n <= 6:
        footprint.update(re.findall(r'(?<![\w/])((?:pkg|cmd|br)/[\w/.-]+\.(?:rs|toml))', text))
    reasons = defaultdict(set)
    for path in sorted(footprint):
        if path in last:
            reasons[last[path]].add(path)
    deps = sorted(reasons)
    batch = 1 + max((nodes[d - 1]['batch'] for d in deps), default=0)
    for d, paths in reasons.items():
        edges[(d, n)].update(paths)
    for path in footprint:
        last[path] = n
    nodes.append(dict(id=n, sha=f['提交'], title=f['原始标题'], file=task.name,
                      batch=batch, predecessors=deps, footprint=sorted(footprint),
                      status=re.search(r'^状态：(.*)$', text, re.M).group(1)))
edge_list = [dict(source=a, target=b, files=sorted(paths)) for (a,b),paths in sorted(edges.items())]
batches = defaultdict(list)
for node in nodes:
    batches[node['batch']].append(node['id'])
policy = '''本节更新并行调度，覆盖旧总plan.md及历史记录中的全串行要求；总plan.md仍只读。批次和边仅由候选写入文件交集生成，是编辑顺序约束，不代表业务能力依赖。按编号保留共享文件的先后顺序，同批次可并行，最多10个任务；已完成节点直接满足约束。阻塞节点只延后真正依赖其能力或必须接续其共享文件改动的任务，不阻断不相关分支。

执行前必须在本编号确认实际写入清单、所属crate注册文件及必要helper/调用方。图未覆盖的新写入文件、真实业务依赖或共享可变资源出现时，先向调度者报告并调整文件占用，暂停冲突任务；不能擅自写入另一活动任务占用的文件。来源Git父提交、Cargo只读依赖或共同读取文档不是默认串行理由。没有完成全部函数级依赖分析，因此此图不是“无业务依赖”的证明。

根Cargo.toml/Cargo.lock、cargo fmt --all、make lint、Bazel准备、全局failpoint开关、共用playground以及上游发布由调度者串行安排独占窗口；其他编辑在cargo fmt --all窗口暂停，之后再基于最终文件做必要验证。聚焦测试可在安全资源边界并行，共用Cargo target会由Cargo锁串行等待，不能停别人的Cargo；若隔离target会导致重复巨量编译，优先排队。每任务仍要取得独立目标证据，串行共享交付检查只可复用确实覆盖最终未再改变文件的结果。

本任务的候选写入清单和带路径原因的前驱边见本目录依赖与并行批次.md、文件依赖图.html及并行调度.json。来源候选不是保证写入项；确认某共享注册/manifest无需修改后可在调度层解除相应编辑约束，真实能力依赖必须另以源码证据记录。'''
for node in nodes:
    p = ROOT / node['file']; s = p.read_text()
    dep = '、'.join(map(str,node['predecessors'])) or '无'
    s = re.sub(r'^执行顺序：.*$', f"执行批次：【批次 {node['batch']}】；文件顺序前驱：{dep}；任务编号：{node['id']}。同批可并行，实际能力依赖另行核查。",s,count=1,flags=re.M)
    s = s.replace('按编号串行避免共享资源冲突，编号前后关系和Git父提交不是业务依赖，','按文件冲突图分批并行，编号前后关系和Git父提交不是业务依赖，')
    section = '## 文件冲突与并行执行（2026-10-02 用户更新）\n\n\n' + policy + '\n\n'
    if '## 文件冲突与并行执行（2026-10-02 用户更新）' in s:
        s = re.sub(r'## 文件冲突与并行执行（2026-10-02 用户更新）.*?(?=\n## 文件\n)',section,s,flags=re.S)
    else:
        s = s.replace('\n## 文件\n','\n'+section+'\n## 文件\n',1)
    p.write_text(s)
# Retain complete prompt bodies, regroup headings by batch.
old_prompt = (ROOT / 'prompt.md').read_text()
prompts = {}
for m in re.finditer(r'^## 任务 (\d+):[^\n]*\n\n```text\n(.*?)\n```',old_prompt,re.M|re.S):
    n=int(m.group(1)); body=m.group(2)
    body=body.replace('当前批次只有本任务，无并行任务，','同批任务可并行，遵守编号文件的文件占用及共享检查独占窗口，')
    body=body.replace('不要求前面所有任务完成。','不要求前面所有任务完成；读取本编号文件冲突前驱并核查实际写入清单，仅在文件占用和真实依赖均满足时执行。')
    prompts[n]=body
assert len(prompts)==359
out=['# Go逐提交严格对齐Rust任务提示词','',
     '每个提交独立验收，按文件冲突图分批执行，同批最多10个并行。只阻塞实际冲突或能力依赖的分支，不等待全仓回归。读取本编号最新独立验收与并行规则，优先于旧plan.md的串行要求；plan.md保持只读。已完成任务无需重跑。根格式化/lint/Cargo.lock等共享检查使用独占窗口。', '']
for b, ids in sorted(batches.items()):
    out += [f'# 批次 {b}', '']
    for n in ids:
        node=nodes[n-1]
        out += [f"## 任务 {n}: 提交 {node['sha'][:10]} 严格对齐 Rust【批次 {b}】文件前驱：{','.join(map(str,node['predecessors'])) or '无'}",'', '```text',prompts[n],'```','']
(ROOT/'prompt.md').write_text('\n'.join(out))
data=dict(schema=1,kind='conservative-file-order',max_parallel=10,nodes=nodes,edges=edge_list,
          batches=[dict(batch=b,tasks=ids) for b,ids in sorted(batches.items())],
          global_exclusive=['Cargo.toml','Cargo.lock','cargo fmt --all','make lint','Bazel prepare','shared failpoint/playground'],
          limitations='候选写入冲突图；未证明不存在业务依赖。执行前确认实际写入和能力依赖，新冲突必须重新排队。')
(ROOT/'并行调度.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
file_nodes=defaultdict(list)
for node in nodes:
    for path in node['footprint']:file_nodes[path].append(node['id'])
(ROOT/'文件任务映射.tsv').write_text('文件\t任务编号（共享文件保持编号顺序）\n'+''.join(p+'\t'+','.join(map(str,ids))+'\n' for p,ids in sorted(file_nodes.items())))
(ROOT/'任务依赖图.mmd').write_text('flowchart LR\n'+''.join(f'  T{n["id"]}["{n["id"]} · 批次{n["batch"]}"]\n' for n in nodes)+''.join(f'  T{e["source"]} --> T{e["target"]}\n' for e in edge_list))
lines=['# 文件冲突图与并行执行批次','',
       f'359个独立任务；{len(batches)}个保守批次；{len(edge_list)}条带文件原因的顺序边；每批最多10个活动任务。', '',
       '原始提交事实.json与总plan.md保持不变。这个图回答哪些候选文件可能同时被写入，不能证明全部函数之间没有业务依赖。', '',
       policy,'', '查看交互图：[文件依赖图.html](文件依赖图.html)。机器可读调度为并行调度.json，文件反查为文件任务映射.tsv，完整Mermaid图为任务依赖图.mmd。', '',
       '## 批次清单','', '| 批次 | 任务编号 |','| --- | --- |']
lines += [f'| {b} | '+', '.join(map(str,ids))+' |' for b,ids in sorted(batches.items())]
lines += ['', '## 文件冲突原因','', '| 前驱 | 后继 | 共同候选文件 |','| --- | --- | --- |']
lines += [f'| {e["source"]} | {e["target"]} | '+ '<br>'.join('`'+p+'`' for p in e['files'])+' |' for e in edge_list]
(ROOT/'依赖与并行批次.md').write_text('\n'.join(lines)+'\n')
# Self-contained interactive graph of one task and its immediate file-order neighbors.
page='''<!doctype html><html lang="zh"><meta charset="utf-8"><title>任务文件冲突图</title>
<style>body{font:15px system-ui;margin:28px;background:#f4f7fb;color:#14253b}h1{font-size:25px}input,select{padding:9px;font:inherit}section{background:white;padding:20px;margin:18px 0;border-radius:12px}svg{width:100%;min-width:900px}#diagram{overflow:auto}.node{cursor:pointer}table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:9px;border-bottom:1px solid #dae3ef}code{word-break:break-all}button{cursor:pointer}small{color:#51627a}</style>
<h1>任务文件冲突图</h1><p id="summary"></p><p>边表示共同候选写入文件的顺序约束；不是完整业务依赖图。点击节点查看文件与邻接任务。根格式化、lint、Cargo.lock等检查使用独占窗口。</p>
<section><label>任务 <select id="task"></select></label> <label>文件筛选 <input id="filter" placeholder="例如 pkg/session"></label></section>
<section><h2 id="title"></h2><div id="diagram"></div><div id="details"></div></section>
<section><h2>文件 → 任务</h2><table><thead><tr><th>文件</th><th>任务编号</th></tr></thead><tbody id="rows"></tbody></table><small>筛选后最多展示200项；可用TSV查看完整清单。</small></section>
<script>const DATA=__DATA__; const el=id=>document.getElementById(id);const esc=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const files=new Map();for(const n of DATA.nodes)for(const f of n.footprint){if(!files.has(f))files.set(f,[]);files.get(f).push(n.id)}
el('summary').textContent=`${DATA.nodes.length}个任务 · ${DATA.batches.length}个保守批次 · ${DATA.edges.length}条文件顺序边 · 最多${DATA.max_parallel}个并行`;
for(const n of DATA.nodes){let o=document.createElement('option');o.value=n.id;o.textContent=`${n.id} · 批次${n.batch} · ${n.sha.slice(0,10)} · ${n.status}`;el('task').append(o)}
function render(){const id=Number(el('task').value),n=DATA.nodes[id-1],incoming=DATA.edges.filter(e=>e.target===id),outgoing=DATA.edges.filter(e=>e.source===id);el('title').textContent=`任务${id} · 批次${n.batch} · ${n.title}`;
const count=Math.max(incoming.length,outgoing.length,1),height=80+count*60,center=height/2;let s=`<svg viewBox="0 0 960 ${height}">`;
const node=(i,x,y)=>{const a=DATA.nodes[i-1];return `<g class="node" data-id="${i}"><rect x="${x}" y="${y-20}" width="170" height="40" rx="7" fill="${i===id?'#c9e2ff':'#edf2f9'}" stroke="#526e90"/><text x="${x+12}" y="${y+5}">任务${i} · 批次${a.batch}</text></g>`};
incoming.forEach((e,i)=>{let y=50+i*60;s+=`<path d="M210 ${y} L400 ${center}" stroke="#748cac" fill="none"><title>${esc(e.files.join('\\n'))}</title></path>`;s+=node(e.source,40,y)});outgoing.forEach((e,i)=>{let y=50+i*60;s+=`<path d="M570 ${center} L750 ${y}" stroke="#748cac" fill="none"><title>${esc(e.files.join('\\n'))}</title></path>`;s+=node(e.target,750,y)});s+=node(id,400,center)+'</svg>';el('diagram').innerHTML=s;for(const g of el('diagram').querySelectorAll('[data-id]'))g.onclick=()=>{el('task').value=g.dataset.id;render()};
el('details').innerHTML=`<p>状态：${esc(n.status)}；前驱：${n.predecessors.join(', ')||'无'}。节点状态为生成时快照，执行时以编号文件为准。</p><h3>共同文件原因</h3>`+[...incoming,...outgoing].map(e=>`<p>${e.source} → ${e.target}：<code>${esc(e.files.join(' · '))}</code></p>`).join('')+`<details><summary>候选写入清单（${n.footprint.length}项）</summary><p><code>${n.footprint.map(esc).join('<br>')}</code></p></details>`;
}
function filter(){let q=el('filter').value.toLowerCase();el('rows').innerHTML=[...files.entries()].filter(([f])=>f.toLowerCase().includes(q)).slice(0,200).map(([f,ids])=>`<tr><td><code>${esc(f)}</code></td><td>${ids.map(id=>`<button data-task="${id}">${id}</button>`).join(' ')}</td></tr>`).join('');for(const b of el('rows').querySelectorAll('[data-task]'))b.onclick=()=>{el('task').value=b.dataset.task;render()}}
el('task').onchange=render;el('filter').oninput=filter;el('task').value=7;render();filter();</script></html>'''
(ROOT/'文件依赖图.html').write_text(page.replace('__DATA__',json.dumps(data,ensure_ascii=False).replace('</','<\\/')))
assert hashlib.sha256((ROOT/'plan.md').read_bytes()).hexdigest()==plan_before
print(f'Generated: {len(nodes)} tasks, {len(batches)} batches, {len(edge_list)} edges, {len(file_nodes)} file keys')
