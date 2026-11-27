export const size=n=>n>=1e9?(n/1e9).toFixed(1)+' GB':n>=1e6?(n/1e6).toFixed(1)+' MB':(n/1e3).toFixed(1)+' KB';

function matches(row,needle){
  return [row.name,row.manifest?.metadata?.host,row.manifest?.metadata?.project,row.manifest?.metadata?.device,...(row.labels||[]),...(row.manifest?.channels||[]).map(c=>c.id)].join(' ').toLowerCase().includes(needle);
}
const recorded=row=>row.manifest?.created_unix_ns||row.modified_ms*1e6;
const compare={
  modified:(a,b)=>b.modified_ms-a.modified_ms,
  new:(a,b)=>recorded(b)-recorded(a),
  old:(a,b)=>recorded(a)-recorded(b),
  name:(a,b)=>a.name.localeCompare(b.name),
  duration:(a,b)=>(b.manifest?.duration_ns||0)-(a.manifest?.duration_ns||0),
  size:(a,b)=>b.bytes-a.bytes,
};
const manifestOrder=new Set(['new','old','duration']);

export function mountLibrary(host,open){
  const $=id=>document.getElementById(id);
  const viewport=$('library'),list=$('recordings'),search=$('search'),sort=$('sort'),controls=$('controls');
  function spacer(){
    const row=document.createElement('li');
    row.setAttribute('aria-hidden','true');
    return row;
  }
  const top=spacer(),bottom=spacer();
  list.replaceChildren(top,bottom);
  const nodes=new Map();
  let index=[],visible=[],records=new Map(),pending=new Map(),wanted=[];
  let selected,focused,paint=0,indexed=false,refreshing=false,reading=false,previewing=false,generation=0,scan=0,error='',folderError='';

  function schedule(){
    paint||=requestAnimationFrame(draw);
  }
  function filter(){
    const needle=search.value.toLowerCase(),order=sort.value;
    visible=index.map(row=>records.get(row.id)||row);
    if(needle)visible=visible.filter(row=>matches(row,needle));
    const canSort=!manifestOrder.has(order)||scan===index.length;
    visible.sort((a,b)=>(compare[canSort?order:'modified'](a,b)||compare.name(a,b)));
  }

  function record(id){
    if(records.has(id))return Promise.resolve(records.get(id));
    if(!pending.has(id)){
      const current=records,requests=pending;
      const request=host.record(id).then(row=>{current.set(id,row);return row}).finally(()=>requests.delete(id));
      pending.set(id,request);
    }
    return pending.get(id);
  }

  async function read(){
    if(reading||refreshing)return;
    const full=!!search.value||manifestOrder.has(sort.value);
    const before=scan;
    let next=wanted.find(row=>!records.has(row.id));
    while(!next&&full&&scan<index.length){
      const row=index[scan++];
      if(!records.has(row.id))next=row;
    }
    if(!next){
      if(before!==scan){
        if(manifestOrder.has(sort.value))filter();
        schedule();
      }
      return;
    }
    const current=records;
    reading=true;
    try{
      await record(next.id);
    }catch(e){
      current.set(next.id,{...next,error:e.message});
    }finally{
      reading=false;
      if(current===records&&((search.value&&matches(records.get(next.id),search.value.toLowerCase()))
        ||(manifestOrder.has(sort.value)&&scan===index.length)))filter();
      schedule();
    }
  }

  async function preview(){
    if(previewing||refreshing)return;
    const row=wanted.find(row=>row.manifest&&!nodes.get(row.id).preview);
    if(!row)return;
    const node=nodes.get(row.id),current=index;
    node.preview=true;
    previewing=true;
    try{
      const url=await host.frame(row.id,0);
      if(url&&index===current&&nodes.get(row.id)===node){
        node.img.src=url;
        await node.img.decode();
      }
    }catch{
      if(index===current)node.img.removeAttribute('src');
    }finally{
      previewing=false;
      schedule();
    }
  }

  const rowHeight=()=>parseFloat(getComputedStyle(list).getPropertyValue('--take-height'));

  function makeRow(id){
    const li=document.createElement('li'),button=document.createElement('button'),img=document.createElement('img');
    const text=document.createElement('span'),name=document.createElement('strong'),detail=document.createElement('small');
    button.className='take';button.onclick=()=>{if(button.getAttribute('aria-disabled')!=='true')open(id)};
    img.alt='';
    button.onfocus=()=>{focused=id};
    button.onblur=()=>{focused=null;schedule()};
    text.append(name,detail);button.append(img,text);li.append(button);
    return {li,button,img,name,detail};
  }

  function draw(){
    paint=0;
    const height=rowHeight();
    const scroll=Math.max(0,viewport.scrollTop);
    const first=Math.floor(scroll/height);
    const last=Math.ceil((scroll+Math.max(0,viewport.clientHeight-controls.offsetHeight))/height);
    const start=Math.max(0,Math.min(first-2,visible.length)),end=Math.min(visible.length,last+2);
    const rows=visible.slice(start,end);
    wanted=visible.slice(first,last).map(row=>records.get(row.id)||row);
    const ids=new Set(rows.map(row=>row.id));
    for(const [id,node] of nodes){
      if(ids.has(id))continue;
      if(id===focused&&visible.some(row=>row.id===id)){
        node.li.classList.add('parked');
      }else{
        node.li.remove();
        nodes.delete(id);
      }
    }
    top.style.height=start*height+'px';
    bottom.style.height=(visible.length-end)*height+'px';
    let previous=top,position=start;
    for(const stub of rows){
      const row=records.get(stub.id)||stub,id=row.id;
      if(!nodes.has(id))nodes.set(id,makeRow(id));
      const node=nodes.get(id),{li,button,img,name,detail}=node;
      if(node.index!==index){
        node.index=index;
        node.preview=false;
        img.removeAttribute('src');
      }
      li.classList.remove('parked');
      if(previous.nextSibling!==li)previous.after(li);
      previous=li;
      button.setAttribute('aria-current',String(id===selected));
      li.setAttribute('aria-setsize',visible.length);
      li.setAttribute('aria-posinset',++position);
      button.setAttribute('aria-disabled',String(!!row.error));
      name.textContent=name.title=row.name;
      detail.textContent=row.error?'Unreadable · '+row.error:row.manifest
        ?`${(row.manifest.duration_ns/1e9).toFixed(1)} s · ${row.manifest.frame_count} frames · ${size(row.bytes)}`
        :'Loading details…';
    }
    const full=!!search.value||manifestOrder.has(sort.value),scanning=full&&(scan<index.length||reading);
    if(indexed)$('count').textContent=index.length+' recording'+(index.length===1?'':'s');
    const notice=error||folderError||(refreshing?'Reading folder…':scanning?'Reading recording details…':!indexed?'':!index.length
      ?'No .ftape files in this folder. Record or import a take, then refresh.':!visible.length?'No recordings match. Try a different search.':'');
    if($('notice').textContent!==notice)$('notice').textContent=notice;
    read();
    preview();
  }

  function refilter(){
    viewport.scrollTop=0;
    filter();
    schedule();
  }
  list.addEventListener('keydown',event=>{
    if(event.key!=='Tab'||!focused)return;
    const next=visible.findIndex(row=>row.id===focused)+(event.shiftKey?-1:1);
    const row=visible[next];
    if(!row)return;
    event.preventDefault();
    const height=rowHeight(),top=next*height,bottom=top+height;
    const page=viewport.clientHeight-controls.offsetHeight;
    viewport.scrollTop=Math.min(top,Math.max(bottom-page,viewport.scrollTop));
    schedule();
    requestAnimationFrame(()=>nodes.get(row.id)?.button.focus({preventScroll:true}));
  });
  search.oninput=sort.onchange=refilter;
  viewport.addEventListener('scroll',schedule,{passive:true});
  const resize=new ResizeObserver(schedule);
  resize.observe(viewport);
  resize.observe(controls);

  return {
    async refresh(){
      const attempt=++generation;
      refreshing=$('refresh').disabled=true;
      folderError='';
      schedule();
      try{
        const next=await host.list();
        if(attempt!==generation)return;
        index=next;
        indexed=true;
        records=new Map(index.filter(row=>row.manifest||row.error).map(row=>[row.id,row]));
        pending=new Map();
        scan=0;
        refilter();
      }catch(e){
        if(attempt===generation)folderError='Cannot read folder: '+e.message;
      }finally{
        if(attempt===generation){refreshing=$('refresh').disabled=false;schedule()}
      }
    },
    record,
    select(id){selected=id;schedule()},
    showError(message){error=message;schedule()},
  };
}
