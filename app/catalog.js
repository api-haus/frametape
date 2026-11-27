import {VideoPlayer} from './player.js';
import {DecodedFrames} from './decoded-frames.js';
import {mountLibrary,size} from './library.js';
export function mount(host){

const $=id=>document.getElementById(id);
window.frametapeHost=host;
window.FrametapeVideoPlayer=VideoPlayer;
window.FrametapeDecodedFrames=DecodedFrames;
if(host.saveMode==='draft'){document.querySelector('#empty p').textContent='Open local .ftape files to play, inspect and annotate. Drafts stay in this browser; export a new recording to share them.';$('download').textContent='Export .ftape';}
let selected=null,opening=0,timer,dirty=false,eventFrame=null,eventBusy=false;
const library=mountLibrary(host,open);
async function open(id){
  const attempt=++opening,review=$('review');
  library.showError('');
  try{
    const html=await host.reviewHTML(id);
    if(attempt!==opening)return;
    if(!leave()){host.selectedId=selected;return;}
    selected=id;
    library.select(id);
    $('empty').style.display='none';review.style.display='block';$('download').disabled=false;$('share').disabled=false;
    const loaded=new Promise(resolve=>review.addEventListener('load',resolve,{once:true}));
    review.srcdoc=html;
    const url=new URL(location.href);url.searchParams.set('take',id);history.replaceState(null,'',url);
    $('metadata').replaceChildren();$('channels').replaceChildren();$('events').textContent='Loading frame events…';
    library.record(id).then(r=>{
      if(selected!==id)return;
      if(r.error)throw Error(r.error);
      const m=r.manifest;
      const data={'Project':m.metadata.project||m.metadata.host||'Custom','Device':m.metadata.device||'Unknown','Duration':(m.duration_ns/1e9).toFixed(3)+' s','Frames':m.frame_count,'Events':m.event_count,'Size':size(r.bytes),'ID':m.id};
      for(const [key,value] of Object.entries(data)){const dt=document.createElement('dt'),dd=document.createElement('dd');dt.textContent=key;dd.textContent=value;$('metadata').append(dt,dd)}
      for(const c of m.channels){const span=document.createElement('span');span.textContent=c.id;span.title=c.schema;$('channels').append(span)}
    }).catch(e=>{if(selected===id)library.showError(e.message)});
    await loaded;
  }catch(e){
    if(attempt===opening)library.showError('Cannot open recording: '+e.message);
  }
}
async function refresh(){
  const first=host.selectedId;
  if(first&&first!==selected)await open(first);
  await library.refresh();
}
addEventListener('message',event=>{
  if(event.source!==$('review').contentWindow||event.data?.id!==selected)return;
  if(event.data.type==='dirty'){dirty=event.data.value;window.frametapeNative?.dirty(dirty);return}
  if(event.data.type!=='frame')return;
  eventFrame=event.data.frame;
  refreshEvents();
});
function refreshEvents(){
  if(timer||eventBusy)return;
  timer=setTimeout(async()=>{
    timer=null;eventBusy=true;const id=selected,index=eventFrame;
    try{
      const events=await host.events(id,index);
      if(selected===id){$('frame-label').textContent=index==null?'Channel events':'Frame '+index+' · events';$('events').textContent=events.length?JSON.stringify(events,null,2):'No channel events in this frame interval.'}
    }catch(error){if(selected===id)$('events').textContent=error.message}
    finally{eventBusy=false;if(selected!==id||eventFrame!==index)refreshEvents()}
  },100);
}
function leave(){if(dirty&&!confirm('Discard unsaved annotations and open another recording?'))return false;dirty=false;$('review').contentWindow?.postMessage({type:'discard'},'*');window.frametapeNative?.dirty(false);return true}
$('open-file').hidden=$('open-folder').hidden=!host.canOpen;
$('open-file').onclick=()=>host.openFiles().catch(e=>library.showError(e.message));
$('open-folder').onclick=()=>host.openFolder().catch(e=>library.showError(e.message));
$('download').onclick=()=>$('review').contentWindow?.postMessage({type:'export'},'*');
if(host.portal){
  $('share').hidden=false;
  $('refresh').textContent='Refresh recordings';
  $('share').onclick=async()=>{
    try{await navigator.clipboard.writeText(location.href);$('share').textContent='Copied';setTimeout(()=>$('share').textContent='Copy link',1500)}
    catch(error){library.showError(error.message)}
  };
  const uploads=$('uploads');
  let revision;
  const poll=async()=>{
    try{
      const {active:rows,revision:next}=await host.uploads();
      uploads.hidden=!rows.length;
      uploads.textContent=rows.length===1?'1 recording uploading':rows.length+' recordings uploading';
      if(revision!==undefined&&revision!==next)await library.refresh();
      revision=next;
    }catch(error){uploads.hidden=false;uploads.textContent='Upload status unavailable'}
    setTimeout(poll,2000);
  };
  poll();
}
host.onChange(refresh);
$('refresh').onclick=()=>library.refresh();
$('toggle').onclick=()=>{const open=$('details').classList.toggle('open');$('toggle').setAttribute('aria-expanded',String(open))};
refresh();

}
