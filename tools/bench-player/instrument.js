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
   if(!ready&&v.readyState>=2&&!v.seeking&&!player.pendingSeek&&!player.loading&&Number.isFinite(v.duration)){ready=true;send({type:'ready',dpr:devicePixelRatio,width:v.videoWidth,height:v.videoHeight,duration:v.duration,controls:Object.fromEntries(['first','fwd','back','last'].map(id=>[id,rect(document.getElementById(id))])),track:rect(track)})}
   const background=player?.prefetch?.video;if(background&&background!==observed){observed=background;for(const name of ['seeking','seeked','error'])background.addEventListener(name,()=>send({type:'decoder',event:name,time:background.currentTime,seeking:background.seeking,ready:background.readyState,duration:background.duration,seekable:Array.from({length:background.seekable.length},(_,i)=>[background.seekable.start(i),background.seekable.end(i)]),center:player.prefetch.center}))}
   const r=request,cache=document.querySelector('#decoded-frame');
   if(r&&!r.stalled&&performance.now()-r.began>5000){r.stalled=true;send({type:'stalled',id:r.id,expected:r.expected,frame:frame(),time:v.currentTime,seeking:v.seeking,readyState:v.readyState})}
   const cached=cache&&cache.style.visibility==='visible';
   if(r&&!r.pending&&frame()===r.expected&&(cached||(!v.seeking&&v.readyState>=2))){
    r.pending=true;
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
     if(request!==r)return;
     if(frame()!==r.expected){r.pending=false;return}
     sampled?.();sampled=null;send({type:'sample',id:r.id,action:r.action,frame:frame(),feedback_ms:performance.now()-r.began,seeked_ms:r.seeked_ms,cached:!!cached,media_time:v.currentTime,prefetch:player.prefetch?{keys:[...player.prefetch.entries.keys()],failed:player.prefetch.failed,running:player.prefetch.running}:null});request=null;
     if(window.__benchImages&&cached)send({type:'picture',picture:player.picture,frame:frame(),png:cache.toDataURL('image/png')});
    }));
   }
   if(ready&&window.__benchPlay&&!playback)measurePlayback();
   if(ready&&window.__benchStep&&!stepping)measureSteps();
   requestAnimationFrame(tick);
  };requestAnimationFrame(tick);
  // Paused review without an input device: scrub, walk frame by frame, then step at key-repeat pace.
  let stepping=false,sampled=null;
  const measureSteps=async()=>{
   stepping=true;
   const sample=()=>new Promise(resolve=>{sampled=resolve}),idle=ms=>new Promise(resolve=>setTimeout(resolve,ms));
   const press=id=>{const done=sample();document.getElementById(id).click();return done};
   for(const fraction of [.2,.55,.8]){
    const target=Math.round(fraction*(TAKE.frames-1)),done=sample();begin(target,'scrub');seek(target);await done;await idle(1500);
    for(const id of ['fwd','back'])for(let i=0;i<12;i++)await press(id);
    await idle(300);
    const shown=new Set(),began=performance.now(),watch=setInterval(()=>shown.add(frame()),4);
    for(let i=0;i<30;i++){document.getElementById('fwd').click();await idle(33)}
    request=null;const final=player.wanted,deadline=performance.now()+20000;
    while((frame()!==final||player.pendingSeek)&&performance.now()<deadline)await idle(4);
    clearInterval(watch);send({type:'held',presses:30,reached:frame()===final,wall_ms:performance.now()-began,frames_shown:shown.size});await idle(300);
   }
   send({type:'steps'});
  };
  // Continuous Play: display cadence, media clock progress and decoder-reported pictures until the take ends.
  let playback=null;
  const quality=()=>{const q=v.getVideoPlaybackQuality?.();return q?{total:q.totalVideoFrames,dropped:q.droppedVideoFrames}:null};
  const measurePlayback=()=>{
   const p=playback={frames:[],clock:[],presented:[],waiting:0,stalled:0,events:[]};
   for(const name of ['waiting','stalled'])v.addEventListener(name,()=>p[name]++);
   const presented=(now,meta)=>{if(p.began!=null&&!p.done)p.presented.push(meta.mediaTime);v.requestVideoFrameCallback(presented)};
   if(v.requestVideoFrameCallback)v.requestVideoFrameCallback(presented);
   new PerformanceObserver(list=>{for(const e of list.getEntries())p.events.push(e.duration)}).observe({type:'resource'});
   const finish=()=>{
    p.done=true;const gaps=p.frames.slice(1).map((t,i)=>t-p.frames[i]).sort((a,b)=>a-b),at=q=>gaps[Math.min(gaps.length-1,Math.floor(gaps.length*q))]??null;
    const interval=gaps[gaps.length>>1]??0,pictures=new Set(p.clock.map(t=>player.pictureAt(t)));
    let frozen=0,since=0;for(let i=1;i<p.clock.length;i++){if(p.clock[i]!==p.clock[i-1])since=i;frozen=Math.max(frozen,p.frames[i]-p.frames[since])}
    const q=quality(),wall=p.frames.at(-1)-p.began,media=p.clock.at(-1)-p.clock[0];
    send({type:'playback',wall_ms:wall,media_ms:media*1000,recorded_pictures:player.pictures.length,clock_pictures:pictures.size,presented_callbacks:p.presented.length,presented_pictures:new Set(p.presented.map(t=>player.pictureAt(t))).size,decoder_total:q&&q.total-p.quality.total,decoder_dropped:q&&q.dropped-p.quality.dropped,display_frames:p.frames.length,display_interval_ms:{p50:interval,p95:at(.95),p99:at(.99),max:gaps.at(-1)??null},late_display_frames:gaps.filter(g=>g>interval*1.5).length,longest_clock_freeze_ms:frozen,waiting:p.waiting,stalled:p.stalled,requests:p.events.length,slowest_request_ms:Math.max(0,...p.events)});
   };
   const frame=now=>{if(p.done)return;if(!v.paused||p.began==null){if(!v.paused&&p.began==null){p.began=now;p.quality=quality()}if(p.began!=null){p.frames.push(now);p.clock.push(v.currentTime)}}requestAnimationFrame(frame)};
   v.addEventListener('ended',finish,{once:true});requestAnimationFrame(frame);document.getElementById('play').click();
  };
 });
}
