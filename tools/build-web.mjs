import {readFile,writeFile,mkdir} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {build} from 'esbuild';
const text=path=>readFile(path,'utf8');
const model=await readFile('app/.generated/frametape_web_bg.wasm');
const worker=await build({entryPoints:['node_modules/@ffmpeg/ffmpeg/dist/esm/worker.js'],bundle:true,format:'iife',write:false,minify:true});
const codecJs=await readFile('node_modules/@ffmpeg/core/dist/umd/ffmpeg-core.js');
const codecWasm=await readFile('node_modules/@ffmpeg/core/dist/umd/ffmpeg-core.wasm');
const result=await build({entryPoints:['app/browser-entry.js'],bundle:true,format:'esm',write:false,minify:true,define:{
 FRAMETAPE_MODEL:JSON.stringify(model.toString('base64')),
 FRAMETAPE_REVIEW:JSON.stringify(await text('app/annotator.html')),
 FRAMETAPE_FFMPEG_WORKER:JSON.stringify(worker.outputFiles[0].text+`
const receive=self.onmessage;
self.onmessage=event=>{
 if(event.data.type==='LOAD'){
  const config=event.data.data;
  config.coreURL=URL.createObjectURL(new Blob([config.coreSource],{type:'text/javascript'}));
  config.wasmURL=URL.createObjectURL(new Blob([config.wasmBytes],{type:'application/wasm'}));
  delete config.coreSource;delete config.wasmBytes;
 }
 return receive(event);
};`),
 FRAMETAPE_CODEC_JS:JSON.stringify(createHash('sha256').update(codecJs).digest('hex')),
 FRAMETAPE_CODEC_WASM:JSON.stringify(createHash('sha256').update(codecWasm).digest('hex')),
}});
const html=(await text('app/catalog.html')).replace('<script type="module" src="./entry.js"></script>',()=>`<script type="module">${result.outputFiles[0].text.replaceAll('</script','<\\/script')}</script>`);
await mkdir('app/dist',{recursive:true});await writeFile('app/dist/frametape.html',html);
await mkdir('web/app',{recursive:true});await writeFile('web/app/frametape.html',html);
const licenses=['node_modules/@zip.js/zip.js/LICENSE','node_modules/@ffmpeg/ffmpeg/LICENSE'];
let notices='Frametape application: MIT. Rust model dependencies retain their Cargo licenses.\n\nThe optional FFmpeg codec program is downloaded directly from its upstream npm distribution.\nVersion: @ffmpeg/core0.12.10; source/build scripts: https://github.com/ffmpegwasm/ffmpeg.wasm\nIt is not embedded in this HTML. Recordings remain local.\n';
for(const path of licenses){try{notices+='\n'+path+'\n'+await text(path)}catch{}}
await writeFile('app/dist/THIRD-PARTY-NOTICES.txt',notices);await writeFile('web/app/THIRD-PARTY-NOTICES.txt',notices);console.log('Built standalone browser app: app/dist/frametape.html');
