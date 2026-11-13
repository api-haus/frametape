import {seekVideo} from './video-decoder.js';
import {videoSamples} from './mp4-samples.js';

// Optional paused-frame producer. Owns its decoder and original-size pictures, bounded by bytes and count.
// Packets decode a whole neighbourhood at once; a seeking video element serves media without a packet index.
export class DecodedFrames {
 constructor({document,budget=128*1024*1024}){this.document=document;this.budget=budget;this.entries=new Map();this.waiting=[];this.generation=0;this.worker=0;this.center=null}
 reset(media){this.dispose();this.failed=false;this.media=media}
 get(picture){return this.entries.get(picture)}
 release(image){if(image.close)image.close();else image.width=image.height=0}
 limit(){return this.width?Math.min(this.samples?64:7,Math.floor(this.budget/(this.width*this.height*4))):0}
 store(picture,image,center){
  const cost=image.width*image.height*4;
  if(!cost||cost>this.budget||this.entries.has(picture)){this.release(image);return false}
  const bytes=()=>[...this.entries.values()].reduce((n,image)=>n+image.width*image.height*4,0);
  while(this.entries.size>=this.limit()||bytes()+cost>this.budget){
   const victim=[...this.entries.keys()].sort((a,b)=>Math.abs(b-center)-Math.abs(a-center))[0];
   this.release(this.entries.get(victim));this.entries.delete(victim);
  }
  this.entries.set(picture,image);return true;
 }
 capture(picture,video,center=picture){
  if(this.entries.has(picture))return true;
  if(this.failed||video.seeking||video.readyState<2||!video.videoWidth)return false;
  this.width=video.videoWidth;this.height=video.videoHeight;
  const image=this.document.createElement('canvas');image.width=this.width;image.height=this.height;
  const context=image.getContext('2d');context.drawImage(video,0,0);
  if(!context.getImageData(0,0,1,1).data[3]){image.width=image.height=0;return false}
  return this.store(picture,image,center);
 }
 warm(center,delay=this.samples?0:100){
  if(!this.media||this.failed)return;
  if(this.center===center&&this.running)return;
  this.center=center;this.generation++;
  if(!this.running){clearTimeout(this.timer);this.timer=setTimeout(()=>this.run(),delay)}
 }
 // Picture on demand, or null when only a seek can produce it.
 request(picture){
  if(!this.media||this.failed||this.samples===null)return null;
  return new Promise(resolve=>{this.waiting.push({picture,resolve});this.warm(picture,0)});
 }
 settle(all){this.waiting=this.waiting.filter(({picture,resolve})=>{const image=this.entries.get(picture);if(!image&&!all)return true;resolve(image??null);return false})}
 closeDecoder(){this.decode?.abort();if(this.video){this.video.removeAttribute('src');this.video.load();this.video.remove();this.video=null}}
 neighbors(center,count){
  const pictures=[];
  for(let distance=0;pictures.length<count&&distance<this.media.times.length;distance++)for(const picture of distance?[center+distance,center-distance]:[center])if(picture>=0&&picture<this.media.times.length&&pictures.length<count)pictures.push(picture);
  return pictures;
 }
 async index(){
  const samples=await videoSamples(this.media.url).catch(()=>null);
  if(!samples||samples.count!==this.media.times.length||!globalThis.VideoDecoder||!(await VideoDecoder.isConfigSupported(samples.config).catch(()=>null))?.supported)return null;
  return samples;
 }
 async packets(pictures,center,worker){
  const s=this.samples,last=Math.max(...pictures),wanted=new Set(pictures);let first=Math.min(...pictures);while(!s.keys[first])first--;
  const from=s.offsets[first],to=s.offsets[last]+s.sizes[last];
  const response=await fetch(this.media.url,{headers:{Range:`bytes=${from}-${to-1}`},signal:this.decode.signal});
  if(!response.ok)throw Error('Video packets are unreadable');
  const bytes=new Uint8Array(await response.arrayBuffer()),base=response.status===206?from:0,job=this.job={wanted,center,worker,stored:[]};
  // One decoder serves every request until Play or teardown: creating it costs more than a group of pictures.
  if(this.decoder?.state!=='configured'){
   this.decoder=new VideoDecoder({error:error=>{if(this.job)this.job.failure=error},output:frame=>{
    const job=this.job,picture=Math.round(frame.timestamp/1000);
    if(job?.wanted.has(picture)&&job.worker===this.worker)job.stored.push(createImageBitmap(frame).then(image=>{if(job.worker===this.worker)this.store(picture,image,job.center);else image.close()}));
    frame.close();
   }});
   this.decoder.configure(s.config);
  }
  for(let i=first;i<=last;i++)this.decoder.decode(new EncodedVideoChunk({type:s.keys[i]?'key':'delta',timestamp:i*1000,data:bytes.subarray(s.offsets[i]-base,s.offsets[i]-base+s.sizes[i])}));
  await this.decoder.flush();await Promise.all(job.stored);
  if(job.failure)throw job.failure;
  if(worker===this.worker&&job.stored.length!==wanted.size)throw Error('Packet decoder lost pictures');
 }
 closePackets(){this.job=null;if(this.decoder&&this.decoder.state!=='closed')this.decoder.close();this.decoder=null}
 async seek(picture,center,worker,generation){
  const times=this.media.times;
  if(!this.video){const v=this.document.createElement('video');v.muted=true;v.playsInline=true;v.preload='auto';this.video=v;v.setAttribute('aria-hidden','true');v.tabIndex=-1;v.style.cssText='position:fixed;width:1px;height:1px;opacity:0;pointer-events:none';this.document.body.append(v);v.src=this.media.url;v.load()}
  const v=this.video,end=times[picture+1]??times[0]+this.media.duration,at=times[picture]+Math.min(.0005,(end-times[picture])/4);
  await seekVideo(v,at,{signal:this.decode.signal,from:times[picture],to:Math.min(end,at+Math.min(.0005,(end-times[picture])/4))});
  if(worker!==this.worker||generation!==this.generation)return;
  if(v.currentTime<times[picture]-1e-7||v.currentTime>=(times[picture+1]??Infinity))throw Error('Background decoder reached a different picture');
  if(!this.capture(picture,v,center))throw Error('Background decoder supplied no drawable pixels');
 }
 async run(){
  const worker=++this.worker;this.running=true;
  try{
   if(this.samples===undefined){const samples=await this.index();if(worker!==this.worker)return;this.samples=samples;if(samples){this.width=samples.width;this.height=samples.height}else this.settle(true)}
   while(this.center!=null&&worker===this.worker){
    const generation=this.generation,center=this.center,count=this.limit();
    // Packets refill the whole window once the cursor nears its edge; seeks fetch one neighbour at a time.
    const near=this.neighbors(center,this.samples?Math.min(count,9):count);
    if(near.every(i=>this.entries.has(i)))break;
    this.decode=new AbortController();
    if(this.samples){
     // A cold cursor gets its own picture first, then the window around it.
     try{for(const pictures of this.entries.has(center)?[this.neighbors(center,count)]:[[center],this.neighbors(center,count)]){
      const missing=pictures.filter(i=>!this.entries.has(i));if(missing.length)await this.packets(missing,center,worker);
      if(worker!==this.worker)return;
      this.settle(false);if(generation!==this.generation)break;
     }}
     catch(error){if(worker!==this.worker)return;console.warn('Packet decoding unavailable:',error.message);this.closePackets();this.samples=null;this.settle(true)}
    }else await this.seek(near.find(i=>!this.entries.has(i)),center,worker,generation);
    if(worker!==this.worker)return;
   }
  }catch(error){if(worker===this.worker&&this.center!=null){this.failed=true;console.warn('Frame prefetch unavailable:',error.message)}}
  finally{if(worker===this.worker){this.running=false;this.closeDecoder();this.settle(true)}}
 }
 stop(){clearTimeout(this.timer);this.center=null;this.generation++;this.worker++;this.running=false;this.closeDecoder();this.closePackets();this.settle(true)}
 dispose(){
  this.stop();
  for(const image of this.entries.values())this.release(image);
  this.entries.clear();this.media=null;this.samples=undefined;
 }
}
