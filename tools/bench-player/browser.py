#!/usr/bin/env python3
"""Standalone browser app in an own Chrome: same instrument, records read through CDP.
usage: browser.py play|step <recording> <out-dir>; CHROME_MODE/CHROME_ARGS override headless and window flags.
Headless Chrome rasterizes in software; run under headless-wayland with --ozone-platform=wayland for GPU numbers."""
import json,sys,time,urllib.request,websocket,subprocess,os,shutil
from pathlib import Path
mode,recording,out=sys.argv[1],str(Path(sys.argv[2]).resolve()),Path(sys.argv[3]);port=9233
import tempfile;root=Path(__file__).resolve().parents[2];profile=Path(tempfile.mkdtemp(prefix='frametape-chrome-'))
chrome=subprocess.Popen([os.environ.get('CHROME','google-chrome-stable'),*os.environ.get('CHROME_MODE','--headless=new').split(),f'--remote-debugging-port={port}',f'--user-data-dir={profile}','--no-first-run',*os.environ.get('CHROME_ARGS','--window-size=2560,1440 --force-device-scale-factor=2').split(),'--autoplay-policy=no-user-gesture-required','about:blank'],stdout=open(str(out)+'.chrome.log','w'),stderr=subprocess.STDOUT)
class CDP:
 def __init__(self,url):self.s=websocket.create_connection(url,suppress_origin=True,timeout=120);self.seq=0
 def call(self,method,params={}):
  self.seq+=1;seq=self.seq;self.s.send(json.dumps({'id':seq,'method':method,'params':params}))
  while True:
   m=json.loads(self.s.recv())
   if m.get('id')==seq:
    if 'error'in m:raise RuntimeError(m['error'])
    return m.get('result',{})
 def read(self,expression):return self.call('Runtime.evaluate',{'expression':expression,'returnByValue':True,'awaitPromise':True})['result'].get('value')
try:
 for _ in range(100):
  try:pages=json.load(urllib.request.urlopen(f'http://127.0.0.1:{port}/json'));break
  except Exception:time.sleep(.2)
 c=CDP(next(p for p in pages if p['type']=='page')['webSocketDebuggerUrl'])
 c.call('Page.enable');c.call('Runtime.enable');c.call('DOM.enable')
 flag='window.__benchPlay=true;'if mode=='play'else'window.__benchStep=true;'
 c.call('Page.addScriptToEvaluateOnNewDocument',{'source':"if(window===parent){window.__records=[];window.ipc={postMessage:s=>window.__records.push(s)}};"+flag+(root/'tools/bench-player/instrument.js').read_text()})
 c.call('Page.navigate',{'url':(root/'web/app/frametape.html').as_uri()});time.sleep(2)
 c.call('Page.setInterceptFileChooserDialog',{'enabled':True})
 r=c.read("(()=>{const b=document.querySelector('#open-file').getBoundingClientRect();return {x:b.x+b.width/2,y:b.y+b.height/2}})()")
 for t in ['mousePressed','mouseReleased']:c.call('Input.dispatchMouseEvent',{'type':t,'button':'left','clickCount':1,**r})
 node=c.call('DOM.querySelector',{'nodeId':c.call('DOM.getDocument')['root']['nodeId'],'selector':'#local-recording-files'})['nodeId']
 c.call('DOM.setFileInputFiles',{'nodeId':node,'files':[recording]})
 want='playback'if mode=='play'else'steps';end=time.monotonic()+420
 while time.monotonic()<end:
  records=[json.loads(s)for s in c.read('window.__records||[]')]
  if any(r['type']==want for r in records):break
  time.sleep(1)
 out.mkdir(parents=True,exist_ok=True)
 result={'browser':c.call('Browser.getVersion')['product'],'recording':recording,'samples':[r for r in records if r['type']=='sample'],'held':[r for r in records if r['type']=='held'],'warnings':[r for r in records if r['type']=='warning'],'playback':next((r for r in records if r['type']=='playback'),None),'ready':next((r for r in records if r['type']=='ready'),None)}
 (out/'result.json').write_text(json.dumps(result,indent=1))
 import base64;(out/'shot.png').write_bytes(base64.b64decode(c.call('Page.captureScreenshot',{'format':'png'})['data']))
 print(mode,'records',len(records),'done',any(r['type']==want for r in records),[r for r in records if r['type']=='warning'][:2])
finally:
 chrome.terminate()
 try:chrome.wait(timeout=20)
 except subprocess.TimeoutExpired:chrome.kill()
 shutil.rmtree(profile,ignore_errors=True)
