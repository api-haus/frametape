// Borrowed video element; caller owns its source and cancels outstanding work on detach.
async function until(video,ready,signal){
 const deadline=performance.now()+10000;
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
export async function seekVideo(video,time,{signal}){
 const ready=()=>video.readyState>=2&&Number.isFinite(video.duration)&&video.seekable.length>0;
 await until(video,ready,signal);
 for(let attempt=0;attempt<2;attempt++){
  if(!video.seeking&&Math.abs(video.currentTime-time)<1e-5)return;
  video.currentTime=time;
  await until(video,()=>!video.seeking&&video.readyState>=2,signal);
  if(Math.abs(video.currentTime-time)<1e-5)return;
  // WebKit/GStreamer can reject a paused seek, clear seeking, and keep the old clock.
  if(!attempt){video.load();await until(video,ready,signal)}
 }
 throw Error('Video decoder reached a different position');
}
