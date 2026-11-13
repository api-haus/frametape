import {VideoSeeker} from './video-decoder.js';

function floorIndex(values,time){let lo=0,hi=values.length;while(lo<hi){const mid=(lo+hi)>>1;if(values[mid]<=time)lo=mid+1;else hi=mid}return Math.max(0,lo-1)}

// Decoder-driven playback. Hosts supply one indexed video; UI supplies transport intent.
export class VideoPlayer {
 constructor({video,frames,open,onPosition,onState,prefetch,preview}) {
  this.video=video;this.frames=frames;this.open=open;this.onPosition=onPosition;this.onState=onState;
  this.decoder=new VideoSeeker(video);this.prefetch=prefetch;this.preview=preview;this.cachedPicture=null;this.intent=0;
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
  listen('play',()=>{this.prefetch?.stop();this.animation=requestAnimationFrame(follow);this.state()});listen('pause',()=>{cancelAnimationFrame(this.animation);this.sync();this.warm();this.state()});listen('ended',()=>{this.sync();this.state()});
  listen('seeking',()=>this.state());listen('seeked',()=>{this.sync();this.warm();this.state()});
  listen('loadedmetadata',()=>this.state());
  listen('error',()=>{if(!this.loading)this.fallback()});
  const presented=(_,meta)=>{if(this.disposed)return;this.sync();this.frameRequest=video.requestVideoFrameCallback(presented)};
  if(video.requestVideoFrameCallback)this.frameRequest=video.requestVideoFrameCallback(presented);
  const follow=()=>{if(this.disposed||video.paused)return;if(!video.seeking){this.selected=null;this.sync()}this.animation=requestAnimationFrame(follow)};
  this.ready=Promise.resolve().then(()=>{this.positionSource(this.wanted);if(invalid)throw Error('Video picture index exceeds review limit');return this.load(false)});
  this.ready.catch(error=>this.state(error.message));
 }
 state(message){if(!this.disposed)this.onState({message:message??this.seekError,loading:!!this.loading,playing:!this.video.paused,seeking:(!!this.pendingSeek||this.video.seeking)&&this.cachedPicture==null,ready:!!this.media&&(this.cachedPicture!=null||this.video.readyState>=2)})}
 async load(compatible) {
  this.loading=true;this.state(compatible?'Preparing compatible video…':'Loading video…');
  let media;
  try {
   media=await this.open(compatible);
   if(this.disposed){media.release?.();return}
   if(!Array.isArray(media.times)||!media.times.length||media.times.length!==this.pictures.length||media.times.some((t,i)=>!Number.isFinite(t)||(i>0&&t<=media.times[i-1]))||!Number.isFinite(media.duration)||media.times[0]+media.duration<=media.times.at(-1))throw Error('Video index differs from recorded pictures');
   this.seekRequest=null;this.pendingSeek=null;this.seekError=null;this.seekReady=null;this.decoder.dispose();this.decoder=new VideoSeeker(this.video);this.prefetch?.dispose();this.clearPreview();this.media?.release?.();this.media=media;this.mediaEnd=media.times[0]+media.duration;this.compatible=!!media.compatible;
   await new Promise((resolve,reject)=>{
    const clear=()=>{clearTimeout(timer);this.video.removeEventListener('loadeddata',loaded);this.video.removeEventListener('error',failed);this.cancelLoad=null};
    const loaded=()=>{clear();resolve()};const failed=()=>{clear();reject(Error('Video codec cannot be decoded by this player'))};
    const timer=setTimeout(()=>{clear();reject(Error('Video did not become ready'))},30000);
    this.cancelLoad=()=>{clear();reject(Error('Player closed'))};
    this.video.addEventListener('loadeddata',loaded);this.video.addEventListener('error',failed);
    this.video.src=media.url;this.video.load();
   });
   if(this.disposed)return;
   this.loading=false;this.prefetch?.reset(media);this.positionSource(this.wanted);this.state();
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
  const playing=!this.video.paused,intent=this.intent;
  try{this.video.pause();this.ready=this.load(true);await this.ready;if(this.seekReady&&!await this.seekReady)return;if(playing&&intent===this.intent&&!this.disposed)await this.video.play()}catch(error){this.state(error.message)}
 }
 pictureAt(time){
  const a=this.media?.times;if(!a?.length)return 0;
  return floorIndex(a,time+1e-7);
 }
 sourceTime(time){
  const a=this.media.times,i=this.pictureAt(time),start=this.frames[this.pictures[i]].timestamp_ns;
  const end=i+1<a.length?this.frames[this.pictures[i+1]].timestamp_ns:this.frames.at(-1).timestamp_ns+this.frames.at(-1).duration_ns;
  const mediaEnd=i+1<a.length?a[i+1]:this.mediaEnd;
  const fraction=mediaEnd>a[i]?Math.max(0,Math.min(1,(time-a[i])/(mediaEnd-a[i]))):0;
  return start+(end-start)*fraction;
 }
 sync(){
  if(!this.media||this.loading||this.pendingSeek||this.video.seeking||this.cachedPicture!=null)return;
  const ns=this.sourceTime(this.video.currentTime);let index=this.selected;
  if(index==null)index=floorIndex(this.sourceTimes,ns);
  const picture=this.pictureAt(this.video.currentTime);
  const expected=this.frames[index].video_frame??floorIndex(this.pictureTimes,this.frames[index].timestamp_ns);
  if(this.selected!=null&&picture!==expected){this.seekError='Video decoder did not reach selected picture';this.state();return}
  this.picture=picture;
  this.wanted=index;
  this.publish(index,this.picture,this.video.currentTime,ns);
 }
 publish(index,picture,time,ns){
  this.onPosition({index,picture,missing:this.frames[index].video_frame==null,uncovered:this.frames[index].timestamp_ns<this.pictureTimes[0],mediaTime:time,sourceTimeNs:this.selected==null?ns:this.frames[index].timestamp_ns});
 }
 showPreview(image){if(!this.preview||!image)return;this.preview.width=image.width;this.preview.height=image.height;this.preview.getContext('2d').drawImage(image,0,0);this.preview.style.visibility='visible'}
 clearPreview(){this.cachedPicture=null;if(this.preview)this.preview.style.visibility='hidden'}
 warm(){
  if(!this.prefetch||!this.media||this.loading||this.pendingSeek||this.cancelPlay||!this.video.paused||this.video.seeking)return;
  const picture=this.cachedPicture??this.pictureAt(this.video.currentTime);
  if(this.cachedPicture==null)this.prefetch.capture(picture,this.video);this.prefetch.warm(picture);
 }
 seekSource(index){this.intent++;this.cancelPlay?.();this.positionSource(index)}
 positionSource(index,useCache=true){
  this.decoder.cancel();this.seekRequest=null;this.pendingSeek=null;this.seekError=null;this.seekReady=null;
  if(!this.frames.length)return;
  this.wanted=Math.max(0,Math.min(this.frames.length-1,Math.round(index)));this.selected=this.wanted;
  if(!this.media||this.loading){this.onPosition({index:this.wanted,picture:null,missing:true,uncovered:true,mediaTime:null,sourceTimeNs:this.frames[this.wanted].timestamp_ns});return}
  this.video.pause();
  const source=this.frames[this.selected].timestamp_ns,a=this.media.times;
  let picture=this.frames[this.selected].video_frame;
  if(picture==null)picture=floorIndex(this.pictureTimes,source);
  const start=this.frames[this.pictures[picture]].timestamp_ns;
  const end=picture+1<a.length?this.frames[this.pictures[picture+1]].timestamp_ns:this.frames.at(-1).timestamp_ns+this.frames.at(-1).duration_ns;
  const mediaEnd=picture+1<a.length?a[picture+1]:this.mediaEnd;
  const fraction=end>start?Math.max(0,Math.min(1,(source-start)/(end-start))):0;
  const at=a[picture]+(mediaEnd-a[picture])*fraction;
  const epsilon=Math.min(.0005,Math.max(0,(mediaEnd-at)/4));
  this.at=Math.min(this.mediaEnd,at+epsilon);
  this.seekBounds={from:at,to:Math.min(mediaEnd,at+epsilon*2)};
  const image=useCache&&source>=this.pictureTimes[0]?this.prefetch?.get(picture):null;
  if(image&&this.preview){
   this.cachedPicture=picture;this.picture=picture;
   this.showPreview(image);
   this.publish(this.wanted,picture,this.at,source);this.prefetch.warm(picture);this.state();return;
  }
  if(this.preview?.style.visibility!=='visible')this.showPreview(this.prefetch?.get(this.picture));
  this.cachedPicture=null;this.pendingSeek=true;
  const request=this.seekRequest={},current=()=>!this.disposed&&request===this.seekRequest;
  const decoded=useCache&&this.preview&&source>=this.pictureTimes[0]?this.prefetch?.request(picture):null;
  if(!decoded)this.prefetch?.stop();
  this.seekReady=Promise.resolve(decoded).then(image=>{
   if(!current())return false;
   if(image){this.pendingSeek=null;this.cachedPicture=picture;this.picture=picture;this.showPreview(image);this.publish(this.wanted,picture,this.at,source);this.state();return true}
   if(decoded)this.prefetch.stop();
   return this.decoder.seek(this.at,this.seekBounds).then(complete=>{
    if(!complete||!current())return false;
    this.pendingSeek=null;this.clearPreview();this.sync();this.warm();this.state();return true;
   });
  }).catch(error=>{
   if(current()){this.pendingSeek=null;this.seekError=error.message;this.state()}return false;
  });
  this.state();
 }
 step(delta){if(!this.pictures.length){this.seekSource(this.wanted+delta);return}const index=this.frames[this.wanted]?.video_frame??this.picture;const picture=Math.max(0,Math.min(this.pictures.length-1,index+delta));this.seekSource(this.pictures[picture])}
 async play(){
  this.cancelPlay?.();
  const intent=++this.intent;
  try{
   await this.ready;if(this.seekReady&&!await this.seekReady)return;if(this.disposed||intent!==this.intent)return;
   this.prefetch?.stop();
   if(this.cachedPicture!=null){
    const cancel=()=>this.decoder.cancel();this.cancelPlay=cancel;
    let complete;try{complete=await this.decoder.seek(this.at,this.seekBounds)}
    finally{if(this.cancelPlay===cancel)this.cancelPlay=null}
    if(!complete||intent!==this.intent||this.disposed)return;
    this.clearPreview();
   }
   if(this.video.ended||this.wanted<this.pictures[0]){this.positionSource(this.pictures[0],false);this.prefetch?.stop();if(!await this.seekReady||intent!==this.intent||this.disposed)return}
   this.selected=null;await this.video.play();this.state();
  }catch(error){if(intent===this.intent&&!this.disposed)this.state(error.message)}
 }
 pause(){this.intent++;this.cancelPlay?.();this.video.pause();this.sync();this.warm();this.state()}
 dispose(){this.disposed=true;this.intent++;this.cancelPlay?.();this.decoder.dispose();this.prefetch?.dispose();this.clearPreview();cancelAnimationFrame(this.animation);if(this.frameRequest!=null)this.video.cancelVideoFrameCallback(this.frameRequest);this.cancelLoad?.();for(const [name,fn]of this.listeners)this.video.removeEventListener(name,fn);this.video.pause();this.video.removeAttribute('src');this.video.load();this.media?.release?.()}
}
