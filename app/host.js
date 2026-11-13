// Server-backed storage/media adapter. Native shell supplies file dialogs; review remains shared.
const base=new URL('./',location.href);
async function json(path,options){const response=await fetch(new URL(path,base),options);if(!response.ok)throw new Error(await response.text());return response.json()}
export const host={
  saveMode:'recording',
  canOpen:!!window.frametapeNative,
  list:()=>json('catalog'),
  frame:(id,index)=>Promise.resolve(new URL(`frame/${id}/${index}`,base).href),
  reviewHTML:async id=>{const response=await fetch(new URL('review/'+id,base));if(!response.ok)throw Error(await response.text());return response.text()},
  video:async(id,compatible=false)=>{const media=await json(`media/${id}/${compatible?1:0}`);return {...media,url:new URL(media.url,base).href}},
  save:(id,annotations,revision)=>json('annotations/'+id,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({revision,annotations})}),
  export:async(id,annotations,revision)=>{const response=await fetch(new URL('copy/'+id,base),{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({annotations,revision})});if(!response.ok)throw new Error(await response.text());const blob=await response.blob(),url=URL.createObjectURL(blob),link=document.createElement('a');link.href=url;link.download='recording-annotated.ftape';link.click();setTimeout(()=>URL.revokeObjectURL(url),60000);return {state:'download'}},
  events:(id,index)=>json(`events/${id}/${index==null?'':index}`),
  openFiles:async()=>window.frametapeNative?.open('file'),
  openFolder:async()=>window.frametapeNative?.open('folder'),
  download:async id=>{const link=document.createElement('a');link.href=new URL('download/'+id,base);link.download='';link.click()},
  onChange:()=>{},
};
