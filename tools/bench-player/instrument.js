const warn=console.warn;console.warn=(...args)=>{window===parent?window.ipc?.postMessage(JSON.stringify({type:'warning',message:args.join(' ')})):parent.ipc.postMessage(JSON.stringify({type:'warning',message:args.join(' ')}));warn.apply(console,args)};
// Read-only timing observer. The external driver sends real XTest input.
if(window!==parent){
 addEventListener('DOMContentLoaded',()=>{
  const v=document.querySelector('#v'),clock=document.querySelector('#clock'),track=document.querySelector('#track');
  if(!v||!clock)return;
  let request=null,sequence=0,ready=false,observed=null;
  const send=value=>parent.ipc.postMessage(JSON.stringify(value));
  const frame=()=>Number(clock.textContent.match(/Frame (\d+)/)?.[1]??-1);
  const rect=element=>{const a=frameElement.getBoundingClientRect(),r=element.getBoundingClientRect();return {x:a.x+r.x,y:a.y+r.y,width:r.width,height:r.height}};
  const adjacent=delta=>{const current=frame(),picture=TAKE.timeline[current]?.video_frame??Math.max(0,player.media.times.findLastIndex(t=>t<=v.currentTime+1e-7));return player.pictures[Math.max(0,Math.min(player.pictures.length-1,picture+delta))]};
  const begin=(expected,action)=>{request={id:++sequence,action,expected,began:performance.now(),seeked_ms:null};send({type:'request',id:sequence,action,expected})};
  addEventListener('click',event=>{
   const id=event.target.closest('button')?.id,n=Number(clock.textContent.match(/\/ (\d+)/)?.[1]??0);
   if(id==='fwd')begin(adjacent(1),'next');
   if(id==='back')begin(adjacent(-1),'previous');
   if(id==='first')begin(0,'first');if(id==='last')begin(n,'last');
  },true);
  track.addEventListener('pointerdown',event=>{const n=Number(clock.textContent.match(/\/ (\d+)/)?.[1]??0);begin(Math.round((event.offsetX-10)/(track.clientWidth-20)*n),'scrub')},true);
  v.addEventListener('seeked',()=>{if(request)request.seeked_ms=performance.now()-request.began});
  const tick=()=>{
   if(!ready&&v.readyState>=2&&!v.seeking&&Number.isFinite(v.duration)){ready=true;send({type:'ready',dpr:devicePixelRatio,width:v.videoWidth,height:v.videoHeight,duration:v.duration,controls:Object.fromEntries(['first','fwd','back','last'].map(id=>[id,rect(document.getElementById(id))])),track:rect(track)})}
   const background=player?.prefetch?.video;if(background&&background!==observed){observed=background;for(const name of ['seeking','seeked','error'])background.addEventListener(name,()=>send({type:'decoder',event:name,time:background.currentTime,seeking:background.seeking,ready:background.readyState,duration:background.duration,seekable:Array.from({length:background.seekable.length},(_,i)=>[background.seekable.start(i),background.seekable.end(i)]),center:player.prefetch.center}))}
   const r=request,cache=document.querySelector('#decoded-frame');
   if(r&&!r.stalled&&performance.now()-r.began>5000){r.stalled=true;send({type:'stalled',id:r.id,expected:r.expected,frame:frame(),time:v.currentTime,seeking:v.seeking,readyState:v.readyState})}
   const cached=cache&&cache.style.visibility==='visible';
   if(r&&!r.pending&&frame()===r.expected&&(cached||(!v.seeking&&v.readyState>=2))){
    r.pending=true;
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
     if(request!==r)return;
     if(frame()!==r.expected){r.pending=false;return}
     send({type:'sample',id:r.id,action:r.action,frame:frame(),feedback_ms:performance.now()-r.began,seeked_ms:r.seeked_ms,cached:!!cached,media_time:v.currentTime,prefetch:player.prefetch?{keys:[...player.prefetch.entries.keys()],failed:player.prefetch.failed,running:player.prefetch.running}:null});request=null;
     if(window.__benchImages&&cached)send({type:'picture',picture:player.picture,frame:frame(),png:cache.toDataURL('image/png')});
    }));
   }
   requestAnimationFrame(tick);
  };requestAnimationFrame(tick);
 });
}
