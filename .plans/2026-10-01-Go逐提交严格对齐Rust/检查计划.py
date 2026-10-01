from pathlib import Path
import json, re, subprocess
p=Path(__file__).resolve().parent
facts=json.loads((p/'提交事实.json').read_text())
source=subprocess.check_output(['git','rev-list','ad193e964b^1..ad193e964b^2'],text=True).splitlines()
assert len(facts)==359 and {f['提交'] for f in facts}==set(source)
files=list(p.glob('[0-9]*-提交-*-严格对齐.md'))
assert len(files)==359
prompt=(p/'prompt.md').read_text()
assert len(re.findall(r'^## 任务 ',prompt,re.M))==359
seen=set()
for f in facts:
 n=f['编号'];sha=f['提交'];name=f"{n}-提交-{sha[:10]}-严格对齐.md"
 text=(p/name).read_text()
 assert sha in text and name in prompt
 assert ('## 测试计划' in text) if f['Go路径'] else ('## 验证计划' in text)
 for h in ['状态：未开始','## Progress','## Decision Log','## Outcomes']:
  assert h in text
 for m in f['现存Cargo候选']:assert Path(m).exists(),m
 for parent in f['父提交'].split():
  assert parent not in source or parent in seen
 seen.add(sha)
for file in [p/'plan.md',p/'prompt.md',*files]:
 assert not any(x in file.read_text() for x in ['TBD','TODO'])
 result=subprocess.run(['git','diff','--no-index','--check','/dev/null',str(file)],capture_output=True,text=True)
 assert result.returncode in (0,1) and not result.stdout and not result.stderr,(file,result.stdout,result.stderr)
print('通过：359提交、359任务、359提示一一对应；拓扑顺序、现存manifest及Markdown空白检查正确')
