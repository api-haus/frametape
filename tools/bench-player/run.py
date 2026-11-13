#!/usr/bin/env python3
"""Native player workloads: XTest seek latency (isolated DISPLAY) or --play continuous playback. JSON receipts."""
import argparse,json,os,platform,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('binary');p.add_argument('recording');p.add_argument('output');p.add_argument('--idle-seconds',type=float,default=3);p.add_argument('--play',action='store_true');p.add_argument('--step',action='store_true');p.add_argument('--script',help='extra observer script');p.add_argument('--commit',default=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip());args=p.parse_args();began=time.monotonic()
base=Path(args.output).resolve();base.mkdir(parents=True,exist_ok=True);metrics=base/'events.jsonl';assert not metrics.exists(),metrics
recording=Path(args.recording).resolve();binary=Path(args.binary).resolve()
process=subprocess.Popen([str(binary),str(recording),str(metrics)]+([str(Path(args.script).resolve())]if args.script else[]),env={**os.environ,**({'FRAMETAPE_BENCH_PLAY':'1'}if args.play else{}),**({'FRAMETAPE_BENCH_STEP':'1'}if args.step else{})},stdout=open(base/'app.log','w'),stderr=subprocess.STDOUT)
try:
 def records():
  if not metrics.exists():return []
  return [json.loads(line)for line in metrics.read_text().split('\n')[:-1]if line.strip()]
 def wait(predicate,seconds=120):
  deadline=time.monotonic()+seconds
  while time.monotonic()<deadline:
   if process.poll()is not None:raise RuntimeError((process.returncode,(base/'app.log').read_text()))
   found=[r for r in records()if predicate(r)]
   if found:return found[-1]
   time.sleep(.02)
  raise RuntimeError(records()[-5:])
 ready=wait(lambda r:r['type']=='ready')
 if args.play:
  playback=wait(lambda r:r['type']=='playback',300);print(json.dumps(playback),flush=True)
  (base/'result.json').write_text(json.dumps({'commit':args.commit,'machine':platform.node(),'system':platform.platform(),'profile':'release','binary':str(binary),'recording':str(recording),'display':os.environ.get('WAYLAND_DISPLAY')or os.environ.get('DISPLAY'),'ready':ready,'playback':playback},indent=2));raise SystemExit
 if args.step:
  wait(lambda r:r['type']=='steps',300);found=records();result={'commit':args.commit,'machine':platform.node(),'system':platform.platform(),'profile':'release','binary':str(binary),'recording':str(recording),'display':os.environ.get('WAYLAND_DISPLAY')or os.environ.get('DISPLAY'),'ready':ready,'samples':[r for r in found if r['type']=='sample'],'held':[r for r in found if r['type']=='held'],'warnings':[r for r in found if r['type']=='warning']}
  (base/'result.json').write_text(json.dumps(result,indent=2));raise SystemExit
 window=subprocess.check_output(['xdotool','search','--onlyvisible','--pid',str(process.pid)],text=True).split()[0]
 samples=[]
 def click(point):subprocess.run(['xdotool','mousemove','--window',window,str(round(point['x']*ready['dpr'])),str(round(point['y']*ready['dpr'])),'click','1'],check=True,stdout=subprocess.DEVNULL)
 def action(name,fraction=None):
  old=max((r.get('id',0)for r in records()),default=0)
  if fraction is None:
   b=ready['controls'][name];point={'x':b['x']+b['width']/2,'y':b['y']+b['height']/2}
  else:
   b=ready['track'];point={'x':b['x']+10+(b['width']-20)*fraction,'y':b['y']+15}
  click(point);sample=wait(lambda r:r['type']=='sample'and r['id']>old);sample['case']=name if fraction is None else 'scrub';samples.append(sample);print(json.dumps(sample),flush=True)
 for position in [.2,.55,.8]:
  action('scrub',position);time.sleep(args.idle_seconds)
  action('fwd');time.sleep(.08);action('back');time.sleep(.08);action('fwd');time.sleep(args.idle_seconds)
 result={'commit':args.commit,'machine':platform.node(),'system':platform.platform(),'profile':'release','wall_ms':(time.monotonic()-began)*1000,'binary':str(binary),'recording':str(recording),'idle_seconds':args.idle_seconds,'display':os.environ.get('DISPLAY'),'ready':ready,'samples':samples}
 (base/'result.json').write_text(json.dumps(result,indent=2))
finally:
 process.terminate()
 try:process.wait(timeout=20)
 except subprocess.TimeoutExpired:process.kill();process.wait()
