function floorIndex(values,time){let lo=0,hi=values.length;while(lo<hi){const mid=(lo+hi)>>1;if(values[mid]<=time)lo=mid+1;else hi=mid}return Math.max(0,lo-1)}

// Decoder-driven playback. Hosts supply one indexed video; UI supplies transport intent.
export class VideoPlayer {
 constructor({video,frames,open,onPosition,onState}) {
  this.video=video;this.frames=frames;this.open=open;this.onPosition=onPosition;this.onState=onState;
  this.media=null;this.picture=0;this.selected=null;this.wanted=0;this.disposed=false;this.compatible=false;
  this.pictures=[];let invalid=false;
  for(const frame of frames)if(frame.video_frame!=null){
   if(!Number.isSafeInteger(frame.video_frame)||frame.video_frame<0||frame.video_frame>=1000000){invalid=true;break}
   if(this.pictures[frame.video_frame]===undefined)this.pictures[frame.video_frame]=frame.index;
  }
  this.sourceTimes=frames.map(f=>f.timestamp_ns);
  this.pictureTimes=this.pictures.map(i=>frames[i].timestamp_ns);
  this.listeners=[];
  const listen=(name,fn)=>{video.addEventListener(name,fn);this.listeners.push([name,fn])};
  listen('play',()=>{this.animation=requestAnimationFrame(follow);this.state()});listen('pause',()=>{cancelAnimationFrame(this.animation);this.sync();this.state()});listen('ended',()=>{this.sync();this.state()});
  listen('seeking',()=>this.state());listen('seeked',()=>{this.sync();this.state()});
  listen('loadedmetadata',()=>this.state());
  listen('error',()=>{if(!this.loading)this.fallback()});
  const presented=(_,meta)=>{if(this.disposed)return;this.sync();this.frameRequest=video.requestVideoFrameCallback(presented)};
  if(video.requestVideoFrameCallback)this.frameRequest=video.requestVideoFrameCallback(presented);
  const follow=()=>{if(this.disposed||video.paused)return;if(!video.seeking){this.selected=null;this.sync()}this.animation=requestAnimationFrame(follow)};
  this.ready=Promise.resolve().then(()=>{this.seekSource(this.wanted);if(invalid)throw Error('Video picture index exceeds review limit');return this.load(false)});
  this.ready.catch(error=>this.state(error.message));
 }
 state(message){if(!this.disposed)this.onState({message,loading:!!this.loading,playing:!this.video.paused,seeking:this.video.seeking,ready:!!this.media&&this.video.readyState>=2})}
 async load(compatible) {
  this.loading=true;this.state(compatible?'Preparing compatible video…':'Loading video…');
  let media;
  try {
   media=await this.open(compatible);
   if(this.disposed){media.release?.();return}
   if(!Array.isArray(media.times)||!media.times.length||media.times.length!==this.pictures.length||media.times.some((t,i)=>!Number.isFinite(t)||(i>0&&t<=media.times[i-1])))throw Error('Video index differs from recorded pictures');
   this.media?.release?.();this.media=media;this.compatible=!!media.compatible;
   await new Promise((resolve,reject)=>{
    const clear=()=>{clearTimeout(timer);this.video.removeEventListener('loadeddata',loaded);this.video.removeEventListener('error',failed);this.cancelLoad=null};
    const loaded=()=>{clear();resolve()};const failed=()=>{clear();reject(Error('Video codec cannot be decoded by this player'))};
    const timer=setTimeout(()=>{clear();reject(Error('Video did not become ready'))},30000);
    this.cancelLoad=()=>{clear();reject(Error('Player closed'))};
    this.video.addEventListener('loadeddata',loaded);this.video.addEventListener('error',failed);
    this.video.src=media.url;this.video.load();
   });
   if(this.disposed)return;
   this.loading=false;this.seekSource(this.wanted);this.state();
  }catch(error){
   if(media&&media!==this.media)media.release?.();
   this.loading=false;
   if(!compatible&&!media?.compatible&&!this.disposed)return this.load(true);
   throw error;
  }
 }
 async fallback(){
  if(this.disposed||this.loading)return;
  if(this.compatible){this.state('Video cannot be decoded by this player');return}
  const playing=!this.video.paused;
  try{this.video.pause();this.ready=this.load(true);await this.ready;if(playing)await this.video.play()}catch(error){this.state(error.message)}
 }
 pictureAt(time){
  const a=this.media?.times;if(!a?.length)return 0;
  return floorIndex(a,time+1e-7);
 }
 sourceTime(time){
  const a=this.media.times,i=this.pictureAt(time),start=this.frames[this.pictures[i]].timestamp_ns;
  const end=i+1<a.length?this.frames[this.pictures[i+1]].timestamp_ns:this.frames.at(-1).timestamp_ns+this.frames.at(-1).duration_ns;
  const mediaEnd=i+1<a.length?a[i+1]:this.video.duration;
  const fraction=mediaEnd>a[i]?Math.max(0,Math.min(1,(time-a[i])/(mediaEnd-a[i]))):0;
  return start+(end-start)*fraction;
 }
 sync(){
  if(!this.media||this.loading||this.video.seeking)return;
  const ns=this.sourceTime(this.video.currentTime);let index=this.selected;
  if(index==null)index=floorIndex(this.sourceTimes,ns);
  this.picture=this.pictureAt(this.video.currentTime);
  this.wanted=index;
  this.onPosition({index,picture:this.picture,missing:this.frames[index].video_frame==null,uncovered:this.frames[index].timestamp_ns<this.pictureTimes[0],mediaTime:this.video.currentTime,sourceTimeNs:this.selected==null?ns:this.frames[index].timestamp_ns});
 }
 seekSource(index){
  if(!this.frames.length)return;
  this.wanted=Math.max(0,Math.min(this.frames.length-1,Math.round(index)));this.selected=this.wanted;
  if(!this.media||this.loading){this.onPosition({index:this.wanted,picture:null,missing:true,uncovered:true,mediaTime:null,sourceTimeNs:this.frames[this.wanted].timestamp_ns});return}
  this.video.pause();
  const source=this.frames[this.selected].timestamp_ns,a=this.media.times;
  let picture=this.frames[this.selected].video_frame;
  if(picture==null)picture=floorIndex(this.pictureTimes,source);
  const start=this.frames[this.pictures[picture]].timestamp_ns;
  const end=picture+1<a.length?this.frames[this.pictures[picture+1]].timestamp_ns:this.frames.at(-1).timestamp_ns+this.frames.at(-1).duration_ns;
  const mediaEnd=picture+1<a.length?a[picture+1]:this.video.duration;
  const fraction=end>start?Math.max(0,Math.min(1,(source-start)/(end-start))):0;
  const at=a[picture]+(mediaEnd-a[picture])*fraction;
  const epsilon=Math.min(.0005,Math.max(0,(mediaEnd-at)/4));
  this.video.currentTime=Math.min(this.video.duration,at+epsilon);this.sync();this.state();
 }
 step(delta){if(!this.pictures.length){this.seekSource(this.wanted+delta);return}const index=this.frames[this.wanted]?.video_frame??this.picture;const picture=Math.max(0,Math.min(this.pictures.length-1,index+delta));this.seekSource(this.pictures[picture])}
 async play(){try{await this.ready;if(this.disposed)return;if(this.video.ended||this.wanted<this.pictures[0])this.seekSource(this.pictures[0]);this.selected=null;await this.video.play();this.state()}catch(error){this.state(error.message)}}
 pause(){this.video.pause();this.sync();this.state()}
 dispose(){this.disposed=true;cancelAnimationFrame(this.animation);if(this.frameRequest!=null)this.video.cancelVideoFrameCallback(this.frameRequest);this.cancelLoad?.();for(const [name,fn]of this.listeners)this.video.removeEventListener(name,fn);this.video.pause();this.video.removeAttribute('src');this.video.load();this.media?.release?.()}
}
