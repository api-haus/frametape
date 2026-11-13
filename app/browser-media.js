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
 ffprobe(args){return this.request('FFPROBE',{args})}
 writeFile(path,data){return this.request('WRITE_FILE',{path,data})}
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
 constructor(){this.queue=Promise.resolve();this.current=null;this.frames=new Map();this.videos=new Map();this.bytes=0}
 async ready(){
  return this.initialized??= (async()=>{

   const [coreBytes,wasmBytes]=await Promise.all([codec('ffmpeg-core.js',FRAMETAPE_CODEC_JS),codec('ffmpeg-core.wasm',FRAMETAPE_CODEC_WASM)]);
   const worker=URL.createObjectURL(new Blob([FRAMETAPE_FFMPEG_WORKER],{type:'text/javascript'}));
   const ffmpeg=this.ffmpeg=new Codec(worker);
   let coreSource=new TextDecoder().decode(coreBytes);
   // Pinned core0.12.10 discards ffprobe's returned status, leaving Module.ret=-1
   // after a successful probe. Preserve the actual C return value in its JS wrapper.
   const probeCall='Module["_ffprobe"](args.length,stringsToPtr(args))';
   if(!coreSource.includes(probeCall))throw Error('Unsupported local video probe wrapper');
   coreSource=coreSource.replace(probeCall,'Module["ret"]='+probeCall);
   await ffmpeg.load({coreSource,wasmBytes});await ffmpeg.createDir('/input');URL.revokeObjectURL(worker);return ffmpeg;
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
 async video(row,file,compatible=false){
  const key=row.id+':'+compatible;
  if(!this.videos.has(key)){
   const prepared=this.run(row,file,async(ffmpeg,path)=>{
    const count=row.info.frames.reduce((max,f)=>Math.max(max,f.video_frame??-1),-1)+1;
    if(!count||count>1000000)throw Error('Video picture index is absent or exceeds the review limit');
    const probe=async(input,decoded=false)=>{
     const entries=decoded?'stream=codec_name,has_b_frames,time_base,width,height,start_time,duration:format=format_name,duration:frame=best_effort_timestamp_time,duration_time,pkt_duration_time:frame_side_data=':'stream=codec_name,has_b_frames,time_base,width,height,start_time,duration:format=format_name,duration:packet=pts_time,duration_time:packet_side_data=';
     const args=['-v','fatal','-select_streams','v:0'];
     if(!decoded)args.push('-read_intervals',`%+#${count+1}`);
     const result=await ffmpeg.ffprobe([...args,'-show_entries',entries,'-of','json','-o','/probe.json',input]);
     if(result!==0)throw Error('Cannot index recording video');
     const text=await ffmpeg.readFile('/probe.json','utf8');await ffmpeg.deleteFile('/probe.json');return JSON.parse(text);
    };
    let info=await probe(path);
    const copy=['h264','hevc'].includes(info.streams?.[0]?.codec_name);
    if(!copy)info=await probe(path,true);
    const planText=row.state.media_plan(JSON.stringify(info)),plan=JSON.parse(planText);
    if(plan.original&&!compatible){
     const times=(copy?info.packets.map(p=>Number(p.pts_time)):info.frames.map(f=>Number(f.best_effort_timestamp_time))).sort((a,b)=>a-b);
     return {blob:new Blob([file],{type:plan.original_mime}),times,source_offset_ns:plan.source_offset_ns,duration:plan.duration,compatible:false};
    }
    const encode=compatible||!plan.copy;
    const args=['-hide_banner','-loglevel','error','-y'];if(encode)args.push('-r',String(count/plan.duration));args.push('-i',path,'-map','0:v:0','-an');
    if(encode)args.push('-vf',plan.filter,'-c:v','libx264','-pix_fmt','yuv420p','-preset','ultrafast','-crf','16','-bf','0','-fps_mode','passthrough','-enc_time_base','1:1000000');
    else args.push('-c:v','copy');
    args.push('-bsf:v',encode?plan.encode_bsf:plan.bsf,'-video_track_timescale','1000000','-movflags','+faststart','/playback.mp4');
    if(await ffmpeg.exec(args,180000)!==0)throw Error('Cannot prepare seekable recording video');
    const outputProbe=await probe('/playback.mp4');
    const times=JSON.parse(row.state.verify_media(planText,JSON.stringify(outputProbe)));
    const data=await ffmpeg.readFile('/playback.mp4');await ffmpeg.deleteFile('/playback.mp4');
    return {blob:new Blob([data],{type:'video/mp4'}),times,source_offset_ns:plan.source_offset_ns,duration:plan.duration,compatible:encode};
   }).catch(error=>{this.videos.delete(key);throw error});
   this.videos.set(key,prepared);
   while(this.videos.size>2)this.videos.delete(this.videos.keys().next().value);
  }
  const {blob,...media}=await this.videos.get(key),url=URL.createObjectURL(blob);
  return {...media,url,release:()=>URL.revokeObjectURL(url)};
 }
}
