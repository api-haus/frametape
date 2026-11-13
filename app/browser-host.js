import * as zip from '@zip.js/zip.js/index-native.js';
import init,{Recording} from './.generated/frametape_web.js';
import {BrowserMedia} from './browser-media.js';
zip.configure({useWebWorkers:false,useCompressionStream:true});
let initialized;
const model=()=>initialized??=init({module_or_path:Uint8Array.from(atob(FRAMETAPE_MODEL),c=>c.charCodeAt(0))});
const safe=name=>typeof name==='string'&&name.length&&!/[\\:\0]/.test(name)&&name.split('/').every(part=>part&&part!=='.'&&part!=='..');
const signature=entries=>entries.map(e=>[e.filename,e.signature,e.uncompressedSize]).sort((a,b)=>a[0].localeCompare(b[0])).map(x=>x.join(':')).join('|');
async function text(entry,limit){
 if(!entry)return '';
 if(entry.uncompressedSize>limit)throw Error(`${entry.filename} exceeds browser review memory limit`);
 const chunks=[];let length=0;
 await entry.getData(new WritableStream({write(chunk){length+=chunk.length;if(length>limit)throw Error('Expanded ZIP member exceeds memory limit');chunks.push(chunk)}}),{checkSignature:true});
 return new TextDecoder('utf-8',{fatal:true}).decode(await new Blob(chunks).arrayBuffer());
}
async function read(file){
 const reader=new zip.ZipReader(new zip.BlobReader(file),{useWebWorkers:false});
 try{
  const entries=await reader.getEntries(),names=new Set();
  if(entries.length>10000)throw Error('Recording contains too many ZIP members');
  for(const e of entries){if(!safe(e.filename)||names.has(e.filename)||e.encrypted||((e.externalFileAttributes>>>16)&0o170000)===0o120000)throw Error('Unsafe, duplicate, encrypted or symlink ZIP member');names.add(e.filename)}
  const byName=new Map(entries.map(e=>[e.filename,e]));
  for(const name of ['manifest.json','frames.jsonl','events.jsonl'])if(!byName.has(name))throw Error('Missing '+name);
  const manifest=await text(byName.get('manifest.json'),64*1024*1024),annotations=await text(byName.get('annotations.json'),8*1024*1024);
  const m=JSON.parse(manifest);
  for(const a of m.attachments||[])if(!byName.has(a.path)||byName.get(a.path).uncompressedSize!==a.bytes)throw Error('Missing or truncated attachment');
  await model();
  const state=new Recording(manifest,await text(byName.get('frames.jsonl'),256*1024*1024),await text(byName.get('events.jsonl'),256*1024*1024),annotations);
  return {reader,entries,byName,state,info:JSON.parse(state.metadata()),revision:signature(entries)};
 }catch(error){await reader.close();throw error}
}
let database;
function db(){return database??=new Promise((resolve,reject)=>{const request=indexedDB.open('frametape-review',1);request.onupgradeneeded=()=>request.result.createObjectStore('drafts');request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)})}
async function draft(key,value,expected=0){
 const database=await db();
 return new Promise((resolve,reject)=>{
  const tx=database.transaction('drafts',value===undefined?'readonly':'readwrite'),store=tx.objectStore('drafts');let result,failure;
  const request=store.get(key);
  request.onsuccess=()=>{
   const raw=request.result,current=typeof raw==='string'?{generation:0,annotations:raw}:raw;
   if(value===undefined){result=current;return}
   if((current?.generation||0)!==expected){failure=Error('Draft changed in another tab. Export your current edits, then reload this app and open the recording to load the saved draft.');tx.abort();return}
   result={generation:expected+1,annotations:value};store.put(result,key);
  };
  tx.oncomplete=()=>resolve(result);tx.onerror=()=>reject(failure||tx.error);tx.onabort=()=>reject(failure||tx.error);
 });
}

function choose(directory=false){return new Promise(resolve=>{const input=document.createElement('input');input.type='file';input.hidden=true;input.id='local-recording-files';document.body.append(input);input.accept='.ftape';input.multiple=true;if(directory)input.webkitdirectory=true;input.onchange=()=>{resolve([...input.files]);input.remove()};input.oncancel=()=>{resolve([]);input.remove()};input.click()})}
function saveBlob(blob,name){const url=URL.createObjectURL(blob),link=document.createElement('a');link.href=url;link.download=name;link.click();setTimeout(()=>URL.revokeObjectURL(url),60000)}
function inline(value){return JSON.stringify(value).replaceAll('<','\\u003c').replaceAll('&','\\u0026')}

/** Browser filesystem/media host. Original attachments are retained byte-for-byte in saved copies. */
export class BrowserReviewHost {
 constructor(){this.rows=new Map();this.canOpen=true;this.saveMode='draft';this.selectedId=null;this.changed=()=>{};this.media=new BrowserMedia()}
 onChange(callback){this.changed=callback}
 async add(file,handle=null){
  const id=crypto.randomUUID(),row={id,name:file.name,bytes:file.size,modified_ms:file.lastModified,file,handle};
  try{Object.assign(row,await read(file));row.manifest=row.info.manifest;
   try{const saved=await draft(row.revision);if(saved){row.info.annotations=JSON.parse(row.state.annotations(saved.annotations));row.draft=saved.annotations;row.draftGeneration=saved.generation}}catch(error){row.draftWarning='Browser draft storage unavailable: '+error.message}
   row.labels=row.info.annotations.containers.flatMap(c=>c.marks.filter(m=>m.mark==='text').map(m=>m.text))}catch(error){row.error=error.message}
  const existing=[...this.rows.values()].find(old=>!old.error&&old.revision===row.revision);
  if(existing){row.state?.free();await row.reader?.close();this.selectedId=existing.id;return}
  this.rows.set(id,row);this.selectedId=id;
 }
 async openFiles(){for(const file of await choose())await this.add(file);this.changed();}

 async openFolder(){
  if(window.showDirectoryPicker){try{const folder=await showDirectoryPicker();for await(const handle of folder.values())if(handle.kind==='file'&&handle.name.endsWith('.ftape'))await this.add(await handle.getFile(),handle)}catch(e){if(e.name!=='AbortError')throw e}}
  else for(const file of await choose(true))if(file.name.endsWith('.ftape')&&file.webkitRelativePath.split('/').length===2)await this.add(file);
  this.changed();
 }
 async list(){return [...this.rows.values()]}
 row(id){const row=this.rows.get(id);if(!row||row.error)throw Error(row?.error||'Recording no longer available');return row}
 async reviewHTML(id){
  const row=this.row(id),{manifest,frames,annotations}=row.info;
  const data={id,stem:row.name.replace(/\.ftape$/i,''),frames:frames.length,timeline:frames,revision:row.revision};
  const html=FRAMETAPE_REVIEW.replace(/__DATA__|__MARKS__/g,token=>token==='__DATA__'?inline(data):inline(annotations));
  return html;
 }
 async source(row){
  if(row.source)return row.source;
  const a=row.manifest.attachments.find(a=>a.media_type==='video/mp4')||row.manifest.attachments.find(a=>a.media_type.startsWith('video/'));
  if(!a)return null;
  const e=row.byName.get(a.path);
  // WORKERFS reads this Blob lazily; stored video never needs a full in-memory copy.
  if(e.compressionMethod===0){
   const header=new DataView(await row.file.slice(e.offset,e.offset+30).arrayBuffer());
   if(header.getUint32(0,true)!==0x04034b50)throw Error('Invalid video ZIP header');
   const offset=e.offset+30+header.getUint16(26,true)+header.getUint16(28,true);
   if(offset+e.uncompressedSize>row.file.size)throw Error('Truncated video');
   await e.getData(new WritableStream({write(){}}),{checkSignature:true});
   row.source=new File([row.file.slice(offset,offset+e.uncompressedSize)],'source.'+(a.path.split('.').pop()||'mp4'),{type:a.media_type});
  }else row.source=new File([await e.getData(new zip.BlobWriter(a.media_type),{checkSignature:true})],'source.'+(a.path.split('.').pop()||'mp4'),{type:a.media_type});
  return row.source;
 }
 async frame(id,index){const row=this.row(id),picture=row.info.frames[index]?.video_frame;if(picture==null)return null;return this.media.frame(row,await this.source(row),picture)}
 async video(id,compatible=false){const row=this.row(id);return this.media.video(row,await this.source(row),compatible)}
 async events(id,index){return JSON.parse(this.row(id).state.events_at(index??-1))}
 async bundle(row,annotations){
  const writer=new zip.ZipWriter(new zip.BlobWriter('application/x-frametape'),{useWebWorkers:false});
  for(const entry of row.entries){if(entry.filename==='annotations.json')continue;
   const stream=new TransformStream();const transfer=entry.getData(stream.writable,{checkSignature:true});
   await writer.add(entry.filename,stream.readable,{compressionMethod:0});await transfer;
  }
  await writer.add('annotations.json',new zip.TextReader(annotations),{compressionMethod:8});
  return writer.close();
 }
 async save(id,annotations,revision){
  const row=this.row(id);if(revision!==row.revision)throw Error('Recording changed; reopen before saving');
  const canonical=row.state.annotations(JSON.stringify(annotations));
  const saved=await draft(row.revision,canonical,row.draftGeneration||0);row.draftGeneration=saved.generation;row.draft=canonical;row.info.annotations=JSON.parse(canonical);
  row.labels=row.info.annotations.containers.flatMap(c=>c.marks.filter(m=>m.mark==='text').map(m=>m.text));
  return {revision:row.revision};
 }
 async export(id,annotations){
  const row=this.row(id),canonical=row.state.annotations(JSON.stringify(annotations)),name=row.name.replace(/\.ftape$/i,'')+'-annotated.ftape';
  const blob=await this.bundle(row,canonical);
  saveBlob(blob,name);return {state:'download'};
 }

 async download(id){const row=this.row(id);return this.export(id,row.info.annotations)}
}
export const host=new BrowserReviewHost();
