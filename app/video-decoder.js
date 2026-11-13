// Borrowed video element; caller owns its source and cancels outstanding work on detach.
async function until(video,ready,signal,timeout=10000){
 const deadline=performance.now()+timeout;
 while(!ready()){
  signal.throwIfAborted();
  if(video.error)throw Error(video.error.message||'Video decoder failed');
  if(performance.now()>deadline)throw Error('Video decoder did not reach selected position');
  await new Promise((resolve,reject)=>{
   const abort=()=>{clearTimeout(timer);reject(signal.reason)};
   const timer=setTimeout(()=>{signal.removeEventListener('abort',abort);resolve()},16);
   signal.addEventListener('abort',abort,{once:true});
  });
 }
 signal.throwIfAborted();
}
function reset(video){const source=video.src;video.removeAttribute('src');video.load();video.src=source;video.load()}
export async function seekVideo(video,time,{signal,from,to}){
 const reached=()=>!video.seeking&&video.readyState>=2&&video.currentTime>=from-1e-7&&video.currentTime<to;
 const ready=()=>video.readyState>=2&&Number.isFinite(video.duration)&&video.seekable.length>0;
 signal.throwIfAborted();if(video.seeking)reset(video);
 await until(video,ready,signal);
 for(let attempt=0;attempt<2;attempt++){
  signal.throwIfAborted();
  if(reached())return;
  signal.throwIfAborted();video.currentTime=time;
  try{await until(video,()=>!video.seeking&&video.readyState>=2,signal,5000)}
  catch(error){if(signal.aborted||video.error||attempt)throw error}
  signal.throwIfAborted();
  if(reached())return;
  // WebKit/GStreamer can reject a paused seek, clear seeking, and keep the old clock.
  if(!attempt){reset(video);await until(video,ready,signal)}
 }
 throw Error('Video decoder reached a different position');
}

// One writer per video. New intent replaces queued work; an active native seek finishes before the next.
export class VideoSeeker {
 constructor(video){this.video=video;this.controller=new AbortController()}
 seek(time,bounds){
  this.cancel();
  return new Promise((resolve,reject)=>{this.pending={time,bounds,resolve,reject};if(!this.running)this.run()});
 }
 cancel(){this.pending?.resolve(false);this.pending=null}
 async run(){
  this.running=true;
  while(this.pending&&!this.controller.signal.aborted){
   const request=this.pending;
   try{
    await seekVideo(this.video,request.time,{signal:this.controller.signal,...request.bounds});
    if(request===this.pending){this.pending=null;request.resolve(true)}
   }catch(error){if(request===this.pending){this.pending=null;request.reject(error)}}
  }
  this.running=false;
 }
 dispose(){this.cancel();this.controller.abort()}
}
