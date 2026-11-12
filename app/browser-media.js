// Transport for the upstream codec worker; failures reject waiting UI operations.
class Codec {
 constructor(source){
  this.worker=new Worker(source);this.pending=new Map();this.sequence=0;
  this.worker.onmessage=({data:message})=>{const waiting=this.pending.get(message.id);if(!waiting)return;this.pending.delete(message.id);clearTimeout(waiting.timer);message.type==='ERROR'?waiting.reject(Error(message.data)):waiting.resolve(message.data)};
  this.worker.onerror=event=>{for(const waiting of this.pending.values()){clearTimeout(waiting.timer);waiting.reject(Error(event.message||'Video worker failed to start'))}this.pending.clear()};
 }
 request(type,data){return new Promise((resolve,reject)=>{const id=++this.sequence,timer=setTimeout(()=>{this.pending.delete(id);reject(Error('Local video decoder timed out'))},180000);this.pending.set(id,{resolve,reject,timer});this.worker.postMessage({id,type,data})})}
 async load(config){await this.request('LOAD',config);this.loaded=true}
 createDir(path){return this.request('CREATE_DIR',{path})}
 mount(fsType,options,mountPoint){return this.request('MOUNT',{fsType,options,mountPoint})}
 unmount(mountPoint){return this.request('UNMOUNT',{mountPoint})}
 exec(args,timeout){return this.request('EXEC',{args,timeout})}
 readFile(path,encoding='binary'){return this.request('READ_FILE',{path,encoding})}
 deleteFile(path){return this.request('DELETE_FILE',{path})}
 terminate(){this.worker.terminate()}
}
// The separately licensed codec program is downloaded, verified and cached; recordings never leave this device.
async function codec(name,hash){
 const url=`https://cdn.jsdelivr.net/npm/@ffmpeg/core@0.12.10/dist/umd/${name}`;
 let cache;try{cache=await caches.open('frametape-codecs-0.12.10')}catch{}
 let response=await cache?.match(url);
 if(!response){response=await fetch(url,{credentials:'omit',referrerPolicy:'no-referrer'});if(!response.ok)throw Error('Cannot download local video decoder; check connection and retry');if(cache)await cache.put(url,response.clone())}
 const bytes=await response.arrayBuffer(),digest=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(n=>n.toString(16).padStart(2,'0')).join('');
 if(digest!==hash){await cache?.delete(url);throw Error('Video decoder integrity check failed')}
 return bytes;
}

/** Serialized software media producer; WORKERFS keeps original video file reads lazy. */
export class BrowserMedia {
 constructor(){this.queue=Promise.resolve();this.current=null;this.frames=new Map();this.segments=new Map();this.bytes=0}
 async ready(){
  return this.initialized??= (async()=>{

   const [coreBytes,wasmBytes]=await Promise.all([codec('ffmpeg-core.js',FRAMETAPE_CODEC_JS),codec('ffmpeg-core.wasm',FRAMETAPE_CODEC_WASM)]);
   const worker=URL.createObjectURL(new Blob([FRAMETAPE_FFMPEG_WORKER],{type:'text/javascript'}));
   const ffmpeg=this.ffmpeg=new Codec(worker);
   await ffmpeg.load({coreSource:new TextDecoder().decode(coreBytes),wasmBytes});await ffmpeg.createDir('/input');return ffmpeg;
  })().catch(error=>{this.initialized=null;throw error});
 }
 run(row,file,work){
  const next=this.queue.catch(()=>{}).then(async()=>{
   if(!file)throw Error('No video attachment in recording');
   const ffmpeg=await this.ready();
   if(this.current!==row.id){if(this.current)await ffmpeg.unmount('/input');await ffmpeg.mount('WORKERFS',{files:[file]},'/input');this.current=row.id}
   return work(ffmpeg,'/input/'+file.name);
  });this.queue=next;return next;
 }
 async frame(row,file,picture){
  const key=row.id+':'+picture;if(this.frames.has(key))return this.frames.get(key).url;
  return this.run(row,file,async(ffmpeg,path)=>{
   const code=await ffmpeg.exec(['-hide_banner','-loglevel','error','-y','-i',path,'-vf',`select=eq(n\\,${picture})`,'-fps_mode','passthrough','-frames:v','1','/frame.png'],120000);
   if(code!==0)throw Error('Cannot decode this original frame in the browser');
   let data;try{data=await ffmpeg.readFile('/frame.png')}catch{throw Error('Video ends before its mapped picture')}
   await ffmpeg.deleteFile('/frame.png');
   const blob=new Blob([data],{type:'image/png'}),url=URL.createObjectURL(blob);this.frames.set(key,{url,bytes:blob.size});this.bytes+=blob.size;
   while(this.bytes>64*1024*1024&&this.frames.size>1){const [old,value]=this.frames.entries().next().value;this.frames.delete(old);this.bytes-=value.bytes;URL.revokeObjectURL(value.url)}
   return url;
  });
 }
 async video(row,file,picture,fps,count){
  if(!Number.isFinite(fps)||fps<=0)throw Error('Invalid declared video frame rate');
  const length=Math.max(1,Math.round(fps*30)),first=Math.floor(picture/length)*length,last=Math.min(count-1,first+length-1),key=row.id+':'+first;
  if(this.segments.has(key))return this.segments.get(key);
  return this.run(row,file,async(ffmpeg,path)=>{
   const filter=`select=between(n\\,${first}\\,${last}),setpts=N/(${fps}*TB),scale=1280:720:force_original_aspect_ratio=decrease:force_divisible_by=2`;
   const code=await ffmpeg.exec(['-hide_banner','-loglevel','error','-y','-i',path,'-vf',filter,'-frames:v',String(last-first+1),'-r',String(fps),'-fps_mode','passthrough','-c:v','libx264','-preset','ultrafast','-crf','24','-pix_fmt','yuv420p','-movflags','+faststart','-an','-progress','/progress.txt','/preview.mp4'],120000);
   if(code!==0)throw Error('Cannot prepare browser playback for this codec');
   const progress=await ffmpeg.readFile('/progress.txt','utf8'),frames=[...progress.matchAll(/^frame=(\d+)$/gm)].at(-1)?.[1];
   if(Number(frames)!==last-first+1)throw Error('Video ends before its mapped pictures');
   const data=await ffmpeg.readFile('/preview.mp4');await ffmpeg.deleteFile('/preview.mp4');await ffmpeg.deleteFile('/progress.txt');
   const segment={url:URL.createObjectURL(new Blob([data],{type:'video/mp4'})),first,last};this.segments.set(key,segment);
   while(this.segments.size>2){const [old,value]=this.segments.entries().next().value;this.segments.delete(old);URL.revokeObjectURL(value.url)}
   return segment;
  });
 }
}
