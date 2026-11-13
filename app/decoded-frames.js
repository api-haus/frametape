import {seekVideo} from './video-decoder.js';

// Optional paused-frame producer. Owns one decoder and original-size pictures, bounded by bytes and count.
export class DecodedFrames {
 constructor({document,budget=128*1024*1024}){this.document=document;this.budget=budget;this.entries=new Map();this.generation=0;this.worker=0;this.center=null}
 reset(media){this.dispose();this.failed=false;this.media=media}
 get(picture){return this.entries.get(picture)}
 capture(picture,video,center=picture){
  if(this.failed||this.entries.has(picture)||video.seeking||video.readyState<2)return;
  const cost=video.videoWidth*video.videoHeight*4;if(!cost||cost>this.budget)return;
  this.width=video.videoWidth;this.height=video.videoHeight;
  const bytes=()=>[...this.entries.values()].reduce((n,image)=>n+image.width*image.height*4,0);
  while(this.entries.size>=7||bytes()+cost>this.budget){
   const victim=[...this.entries.keys()].sort((a,b)=>Math.abs(b-center)-Math.abs(a-center))[0];
   const old=this.entries.get(victim);old.width=old.height=0;this.entries.delete(victim);
  }
  const image=this.document.createElement('canvas');image.width=this.width;image.height=this.height;
  image.getContext('2d').drawImage(video,0,0);this.entries.set(picture,image);
 }
 warm(center){
  if(!this.media||this.failed)return;
  if(this.center===center&&this.running)return;
  this.center=center;this.generation++;
  if(!this.running){clearTimeout(this.timer);this.timer=setTimeout(()=>this.run(),100)}
 }
 closeDecoder(){this.decode?.abort();if(this.video){this.video.removeAttribute('src');this.video.load();this.video=null}}
 async run(){
  const worker=++this.worker;this.running=true;
  try{
   while(this.center!=null&&worker===this.worker){
    const generation=this.generation,center=this.center,times=this.media.times;
    const count=Math.min(7,Math.floor(this.budget/(this.width*this.height*4)));
    const neighbors=[center,center+1,center-1,center+2,center-2,center+3,center-3].filter(i=>i>=0&&i<times.length).slice(0,count);
    const picture=neighbors.find(i=>!this.entries.has(i));if(picture==null)break;
    if(!this.video){const v=this.document.createElement('video');v.muted=true;v.playsInline=true;v.preload='auto';this.video=v;v.src=this.media.url;v.load()}
    const v=this.video;this.decode=new AbortController();
    const end=times[picture+1]??times[0]+this.media.duration,at=times[picture]+Math.min(.0005,(end-times[picture])/4);
    await seekVideo(v,at,{signal:this.decode.signal,from:times[picture],to:Math.min(end,at+Math.min(.0005,(end-times[picture])/4))});
    if(worker!==this.worker)return;
    if(generation!==this.generation)continue;
    if(v.currentTime<times[picture]-1e-7||v.currentTime>=(times[picture+1]??Infinity))throw Error('Background decoder reached a different picture');
    this.capture(picture,v,center);
   }
  }catch(error){if(worker===this.worker&&this.center!=null){this.failed=true;console.warn('Frame prefetch unavailable:',error.message)}}
  finally{if(worker===this.worker){this.running=false;this.closeDecoder()}}
 }
 stop(){clearTimeout(this.timer);this.center=null;this.generation++;this.worker++;this.running=false;this.closeDecoder()}
 dispose(){
  this.stop();
  for(const image of this.entries.values())image.width=image.height=0;
  this.entries.clear();this.media=null;
 }
}
