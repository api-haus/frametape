export function mount(host){

const $=id=>document.getElementById(id);
window.frametapeHost=host;
if(host.saveMode==='draft'){document.querySelector('#empty p').textContent='Open local .ftape files to play, inspect and annotate. Drafts stay in this browser; export a new recording to share them.';$('download').textContent='Export .ftape';}
let records=[],selected=null,timer,dirty=false;
const previews=new IntersectionObserver(entries=>{for(const entry of entries)if(entry.isIntersecting){const img=entry.target;previews.unobserve(img);host.frame(img.dataset.take,0).then(url=>{if(url)img.src=url;else img.style.visibility='hidden'}).catch(()=>{img.style.visibility='hidden'})}},{root:$('library'),rootMargin:'200px'});
const size=n=>n>=1e9?(n/1e9).toFixed(1)+' GB':n>=1e6?(n/1e6).toFixed(1)+' MB':(n/1e3).toFixed(1)+' KB';
function draw(){
  const needle=$('search').value.toLowerCase(),order=$('sort').value;
  const visible=records.filter(r=>[r.name,...(r.labels||[]),...(r.manifest?.channels||[]).map(c=>c.id)].join(' ').toLowerCase().includes(needle));
  visible.sort((a,b)=>order==='name'?a.name.localeCompare(b.name):order==='duration'?(b.manifest?.duration_ns||0)-(a.manifest?.duration_ns||0):order==='size'?b.bytes-a.bytes:order==='old'?(a.manifest?.created_unix_ns||a.modified_ms*1e6)-(b.manifest?.created_unix_ns||b.modified_ms*1e6):(b.manifest?.created_unix_ns||b.modified_ms*1e6)-(a.manifest?.created_unix_ns||a.modified_ms*1e6));
  previews.disconnect();$('recordings').replaceChildren();
  for(const r of visible){
    const li=document.createElement('li'),button=document.createElement('button'),img=document.createElement('img'),text=document.createElement('span'),name=document.createElement('strong'),detail=document.createElement('small');
    button.className='take';button.setAttribute('aria-current',String(r.id===selected));button.disabled=!!r.error;
    img.alt='';img.loading='lazy';img.dataset.take=r.id;previews.observe(img);img.onerror=()=>{img.style.visibility='hidden'};
    name.textContent=r.name;detail.textContent=r.error?'Unreadable · '+r.error:`${(r.manifest.duration_ns/1e9).toFixed(1)} s · ${r.manifest.frame_count} frames · ${size(r.bytes)}`;
    text.append(name,detail);button.append(img,text);button.onclick=()=>open(r);li.append(button);$('recordings').append(li);
  }
  $('count').textContent=records.length+' recording'+(records.length===1?'':'s');
  $('notice').textContent=records.length===0?'No .ftape files in this folder. Record or import a take, then refresh.':visible.length===0?'No recordings match. Try a different search.':'';
}
async function open(r){
  if(!leave()){host.selectedId=selected;return;}
  selected=r.id;draw();$('empty').style.display='none';$('review').style.display='block';$('review').srcdoc=await host.reviewHTML(r.id);$('download').disabled=false;
  const url=new URL(location.href);url.searchParams.set('take',r.id);history.replaceState(null,'',url);
  const m=r.manifest;const data={'Host':m.metadata.host||'Custom','Duration':(m.duration_ns/1e9).toFixed(3)+' s','Frames':m.frame_count,'Events':m.event_count,'Size':size(r.bytes),'ID':m.id};
  $('metadata').replaceChildren();for(const [key,value] of Object.entries(data)){const dt=document.createElement('dt'),dd=document.createElement('dd');dt.textContent=key;dd.textContent=value;$('metadata').append(dt,dd)}
  $('channels').replaceChildren();for(const c of m.channels){const span=document.createElement('span');span.textContent=c.id;span.title=c.schema;$('channels').append(span)}
  $('events').textContent='Loading frame events…';
}
async function refresh(){
  $('refresh').disabled=true;$('notice').textContent='Reading recordings…';
  try{records=await host.list();draw();const first=host.selectedId||new URL(location.href).searchParams.get('take');if(first&&first!==selected){const r=records.find(r=>r.id===first&&!r.error);if(r)open(r)}}
  catch(e){$('notice').textContent='Cannot read folder: '+e.message}finally{$('refresh').disabled=false}
}
addEventListener('message',event=>{
  if(event.source!==$('review').contentWindow||event.data?.id!==selected)return;
  if(event.data.type==='dirty'){dirty=event.data.value;window.frametapeNative?.dirty(dirty);return}
  if(event.data.type!=='frame')return;
  const index=event.data.frame;$('frame-label').textContent=index==null?'Channel events':'Frame '+index+' · events';clearTimeout(timer);
  timer=setTimeout(async()=>{try{const events=await host.events(selected,index);$('events').textContent=events.length?JSON.stringify(events,null,2):'No channel events in this frame interval.'}catch(e){$('events').textContent=e.message}},100);
});
function leave(){if(dirty&&!confirm('Discard unsaved annotations and open another recording?'))return false;dirty=false;$('review').contentWindow?.postMessage({type:'discard'},'*');window.frametapeNative?.dirty(false);return true}
$('open-file').hidden=$('open-folder').hidden=!host.canOpen;
$('open-file').onclick=()=>host.openFiles().catch(e=>{$('notice').textContent=e.message});
$('open-folder').onclick=()=>host.openFolder().catch(e=>{$('notice').textContent=e.message});
$('download').onclick=()=>$('review').contentWindow?.postMessage({type:'export'},'*');
host.onChange(refresh);
$('search').oninput=draw;$('sort').onchange=draw;$('refresh').onclick=refresh;
$('toggle').onclick=()=>{const open=$('details').classList.toggle('open');$('toggle').setAttribute('aria-expanded',String(open))};
refresh();

}
