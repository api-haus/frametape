// Packet index of an MP4's video track, for decoders fed packets instead of seeks. Null when the
// file is not such an MP4 or stores pictures out of presentation order.
const tag=(v,o)=>String.fromCharCode(v.getUint8(o),v.getUint8(o+1),v.getUint8(o+2),v.getUint8(o+3));
async function read(url,from,to){
 const response=await fetch(url,{headers:{Range:`bytes=${from}-${to-1}`}});if(!response.ok)throw Error('Video is not readable by byte range');
 const bytes=await response.arrayBuffer();return new DataView(response.status===206?bytes:bytes.slice(from,to));
}
function* boxes(view,from,to){
 while(from+8<=to){
  let size=view.getUint32(from),head=8;const type=tag(view,from+4);
  if(size===1){if(from+16>to)return;size=Number(view.getBigUint64(from+8));head=16}else if(size===0)size=to-from;
  if(size<head||from+size>to)return;
  yield {type,start:from+head,end:from+size};from+=size;
 }
}
const find=(view,box,type)=>{for(const child of boxes(view,box.start,box.end))if(child.type===type)return child};
const hex=(n,width=2)=>n.toString(16).padStart(width,'0');
function codec(format,record){
 if(format==='avc1'||format==='avc3')return `${format}.${hex(record[1])}${hex(record[2])}${hex(record[3])}`;
 let compatibility=0;for(let bit=0;bit<32;bit++)if(record[2+(bit>>3)]&(0x80>>(bit&7)))compatibility|=1<<bit;
 const constraints=[...record.subarray(6,12)];while(constraints.length>1&&!constraints.at(-1))constraints.pop();
 return `${format}.${['','A','B','C'][record[1]>>6]}${record[1]&31}.${(compatibility>>>0).toString(16)}.${record[1]&32?'H':'L'}${record[12]}.${constraints.map(n=>n.toString(16)).join('.')}`;
}
export async function videoSamples(url){
 let moov,offset=0;
 for(let i=0;i<64&&!moov;i++){
  const head=await read(url,offset,offset+16);if(head.byteLength<16)return null;
  let size=head.getUint32(0),skip=8;if(size===1){size=Number(head.getBigUint64(8));skip=16}
  if(size<skip)return null;
  if(tag(head,4)==='moov'){if(size>64*1024*1024)return null;moov=await read(url,offset+skip,offset+size)}
  offset+=size;
 }
 if(!moov)return null;
 for(const trak of boxes(moov,0,moov.byteLength)){
  const mdia=trak.type==='trak'&&find(moov,trak,'mdia'),hdlr=mdia&&find(moov,mdia,'hdlr'),minf=mdia&&find(moov,mdia,'minf'),stbl=minf&&find(moov,minf,'stbl');
  if(!hdlr||!stbl||tag(moov,hdlr.start+8)!=='vide')continue;
  const table=type=>find(moov,stbl,type),stsd=table('stsd'),stsz=table('stsz'),stsc=table('stsc'),stco=table('stco'),co64=table('co64'),stss=table('stss'),ctts=table('ctts');
  if(!stsd||!stsz||!stsc||!(stco||co64))return null;
  const entry=stsd.start+8,format=tag(moov,entry+4),record=find(moov,{start:entry+86,end:entry+moov.getUint32(entry)},format.startsWith('avc')?'avcC':'hvcC');
  if(!record||!['avc1','avc3','hvc1','hev1'].includes(format))return null;
  const expand=(box,signed)=>{const out=[];if(box)for(let i=0,n=moov.getUint32(box.start+4);i<n;i++)for(let k=moov.getUint32(box.start+8+i*8);k>0&&out.length<1000000;k--)out.push(signed?moov.getInt32(box.start+12+i*8):moov.getUint32(box.start+12+i*8));return out};
  if(ctts){const delta=expand(table('stts')),shift=expand(ctts,true);for(let i=1,decode=0,shown=shift[0];i<shift.length;i++){decode+=delta[i-1];if(decode+shift[i]<=shown)return null;shown=decode+shift[i]}}
  const count=moov.getUint32(stsz.start+8),fixed=moov.getUint32(stsz.start+4),sizes=new Uint32Array(count),offsets=new Float64Array(count);
  for(let i=0;i<count;i++)sizes[i]=fixed||moov.getUint32(stsz.start+12+i*4);
  const chunks=stco?moov.getUint32(stco.start+4):moov.getUint32(co64.start+4),runs=moov.getUint32(stsc.start+4);
  for(let chunk=1,run=0,sample=0;chunk<=chunks&&sample<count;chunk++){
   while(run+1<runs&&moov.getUint32(stsc.start+8+(run+1)*12)<=chunk)run++;
   let at=stco?moov.getUint32(stco.start+8+(chunk-1)*4):Number(moov.getBigUint64(co64.start+8+(chunk-1)*8));
   for(let i=moov.getUint32(stsc.start+12+run*12);i>0&&sample<count;i--){offsets[sample]=at;at+=sizes[sample++]}
  }
  const keys=new Uint8Array(count);
  if(stss)for(let i=0,n=moov.getUint32(stss.start+4);i<n;i++)keys[moov.getUint32(stss.start+8+i*4)-1]=1;else keys.fill(1);
  if(!count||!keys[0])return null;
  const description=new Uint8Array(moov.buffer,moov.byteOffset+record.start,record.end-record.start);
  return {count,sizes,offsets,keys,width:moov.getUint16(entry+32),height:moov.getUint16(entry+34),config:{codec:codec(format,description),description,optimizeForLatency:true}};
 }
 return null;
}
