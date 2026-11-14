//! Exact, block-seekable lossless compression of the complete WDL/DTM table.
//!
//! DOBUTB03 stores 256 sorted positions per block. Each block contains a
//! full 64-bit anchor, positive ULEB128 key deltas, then losslessly packed
//! 9-bit WDL/DTM values. No floating point, sampling, neural network,
//! probability approximation, or quantization occurs.
//!
//! 9 bits are sufficient for the published 173-ply maximum forced win:
//! code 0 = Draw, 1..=174 = Win(DTM=code-1),
//! 175..=348 = Loss(DTM=code-175).
//! Unlike a monolithic zstd archive, blocks can be searched in place with
//! a memory mapped file; decoding requires <=255 deltas per random probe.

use crate::{Position,retrograde::Outcome,tablebase::TableHit};

const MAGIC:&[u8;8]=b"DOBUTB03";
const HEADER:usize=32;
const INDEX:usize=24;
pub const BLOCK_SIZE:usize=256;

fn get_u32(b:&[u8],i:usize)->Option<u32>{
    Some(u32::from_le_bytes(b.get(i..i+4)?.try_into().ok()?))
}
fn get_u64(b:&[u8],i:usize)->Option<u64>{
    Some(u64::from_le_bytes(b.get(i..i+8)?.try_into().ok()?))
}

#[derive(Clone,Copy,Debug)]
pub struct BlockIndex {
    pub first_key:u64,
    pub payload_offset:u64,
    pub payload_len:u32,
    pub count:u16,
}
impl BlockIndex {
    pub fn write_into(&self,out:&mut Vec<u8>){
        out.extend_from_slice(&self.first_key.to_le_bytes());
        out.extend_from_slice(&self.payload_offset.to_le_bytes());
        out.extend_from_slice(&self.payload_len.to_le_bytes());
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&[0,0]);
    }
    fn from_at(buf:&[u8],offset:usize)->Option<Self>{
        Some(Self{
            first_key:get_u64(buf,offset)?,
            payload_offset:get_u64(buf,offset+8)?,
            payload_len:get_u32(buf,offset+16)?,
            count:u16::from_le_bytes(
                buf.get(offset+20..offset+22)?.try_into().ok()?),
        })
    }
}

fn decode_uleb(data:&[u8],pos:&mut usize)->Option<u64>{
    let mut v=0u64;
    for shift in (0..=63).step_by(7) {
        let b=*data.get(*pos)?;
        *pos+=1;
        let limb=(b&0x7f) as u64;
        if limb>(u64::MAX>>shift){return None;}
        v|=limb<<shift;
        if b&0x80==0{return Some(v);}
    }
    None
}
fn encode_uleb(mut delta:u64,out:&mut Vec<u8>) {
    loop{
        let mut b=(delta&127) as u8;
        delta>>=7;
        if delta!=0{b|=0x80;}
        out.push(b);
        if delta==0{break;}
    }
}

/// Exact compact score conversion; returns None on invalid/overflow.
pub fn code_from_v2(v:i16)->Option<u16>{
    if v==i16::MAX{return Some(0);}
    if v==0 || v==i16::MIN{return None;}
    let dist=v.unsigned_abs();
    if dist>174 {return None;}
    Some(if v>0 {dist}else{174+dist})
}
pub fn hit_from_code(code:u16)->Option<TableHit>{
    match code {
        0=>Some(TableHit{result:Outcome::Draw,dtm:0}),
        1..=174=>Some(TableHit{result:Outcome::Win,dtm:code-1}),
        175..=348=>Some(TableHit{result:Outcome::Loss,dtm:code-175}),
        _=>None,
    }
}
pub fn v2_from_code(code:u16)->Option<i16>{
    match code{
        0=>Some(i16::MAX),
        1..=174=>Some(code as i16),
        175..=348=>Some(-((code-174) as i16)),
        _=>None
    }
}
fn get_nine_bits(bytes:&[u8],index:usize)->Option<u16>{
    let bitpos=index.checked_mul(9)?;
    let at=bitpos/8;
    let shift=bitpos%8;
    let lo=*bytes.get(at)? as u16;
    let hi=*bytes.get(at+1).unwrap_or(&0) as u16;
    Some(((lo|(hi<<8))>>shift)&0x1ff)
}

pub fn encode_block(records:&[(u64,i16)]) -> Option<Vec<u8>> {
    if records.is_empty() || records.len()>BLOCK_SIZE{return None;}
    let mut out=Vec::with_capacity(records.len()*7);
    let mut previous=records[0].0;
    for &(key,_) in records.iter().skip(1){
        let delta=key.checked_sub(previous)?;
        if delta==0 {return None;}
        encode_uleb(delta,&mut out);
        previous=key;
    }
    let mut bits=vec![0u8;(records.len()*9+7)/8];
    for (i,(_,v)) in records.iter().enumerate(){
        let code=code_from_v2(*v)? as u32;
        let at=i*9;
        for j in 0..9 {
            if (code>>j)&1!=0{
                bits[(at+j)/8]|=1<<((at+j)%8);
            }
        }
    }
    out.extend(bits);
    Some(out)
}

/// Look up one position from a complete DOBUTB03 mapped slice.
/// A miss returns None, never a fabricated 0/draw/heuristic value.
pub fn probe_v3(bytes:&[u8],position:&Position)->Option<TableHit>{
    probe_key_v3(bytes,position.canonical_key_u64()?)
}
pub fn probe_key_v3(bytes:&[u8],key:u64)->Option<TableHit>{
    if bytes.get(0..8)?!=MAGIC {return None;}
    let total=get_u64(bytes,8)? as usize;
    let block_size=get_u32(bytes,16)? as usize;
    let blocks=get_u32(bytes,20)? as usize;
    if block_size!=BLOCK_SIZE ||
       blocks!=total.checked_add(BLOCK_SIZE-1)?/BLOCK_SIZE {return None;}
    let payload_base=HEADER.checked_add(blocks.checked_mul(INDEX)?)?;
    if payload_base>bytes.len() || blocks==0{return None;}
    let mut lo=0usize;
    let mut hi=blocks;
    while lo<hi {
        let mid=lo+(hi-lo)/2;
        let at=HEADER+mid*INDEX;
        if get_u64(bytes,at)?<=key{lo=mid+1;}else{hi=mid;}
    }
    if lo==0{return None;}
    let blk=BlockIndex::from_at(bytes,HEADER+(lo-1)*INDEX)?;
    if blk.count==0 || blk.count as usize>BLOCK_SIZE{return None;}
    let from=payload_base.checked_add(usize::try_from(blk.payload_offset).ok()?)?;
    let end=from.checked_add(blk.payload_len as usize)?;
    let payload=bytes.get(from..end)?;
    let mut offset=0usize;
    let mut target_index=if blk.first_key==key{Some(0)}else{None};
    let mut cur=blk.first_key;
    for i in 1..blk.count as usize {
        let delta=decode_uleb(payload,&mut offset)?;
        if delta==0{return None;}
        cur=cur.checked_add(delta)?;
        if cur==key {target_index=Some(i);}
    }
    let scores=payload.get(offset..)?;
    if scores.len()!=(blk.count as usize*9+7)/8{return None;}
    hit_from_code(get_nine_bits(scores,target_index?)?)
}

/// Decode one entire block for a streamed, exact equality check against
/// the original sorted table. This is used by the offline compressor only.
#[cfg(feature="full-solver")]
pub fn decode_block(payload:&[u8],first:u64,count:usize)->Option<Vec<(u64,i16)>> {
    if count==0 || count>BLOCK_SIZE{return None;}
    let mut keys=Vec::with_capacity(count);
    keys.push(first);
    let mut cursor=0usize;
    for _ in 1..count{
        let d=decode_uleb(payload,&mut cursor)?;
        if d==0{return None;}
        keys.push(keys.last()?.checked_add(d)?);
    }
    let bits=payload.get(cursor..)?;
    if bits.len()!=(count*9+7)/8{return None;}
    keys.into_iter().enumerate().map(|(i,k)|{
        Some((k,v2_from_code(get_nine_bits(bits,i)?)?))
    }).collect()
}

/// In-memory independent codec verification for test fixtures. The
/// production packer streams sorted records and a bounded block buffer.
#[cfg(test)]
pub fn encode_small(entries:&[(u64,i16)])->Option<Vec<u8>>{
    let blocks=entries.len().div_ceil(BLOCK_SIZE);
    let mut idx=Vec::with_capacity(blocks*INDEX);
    let mut payload=Vec::new();
    for chunk in entries.chunks(BLOCK_SIZE) {
        let encoded=encode_block(chunk)?;
        BlockIndex {
            first_key:chunk.first()?.0,
            payload_offset:payload.len() as u64,
            payload_len:encoded.len().try_into().ok()?,
            count:chunk.len().try_into().ok()?,
        }.write_into(&mut idx);
        payload.extend_from_slice(&encoded);
    }
    let mut out=Vec::new();
    out.extend(MAGIC);
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    out.extend_from_slice(&(BLOCK_SIZE as u32).to_le_bytes());
    out.extend_from_slice(&(blocks as u32).to_le_bytes());
    out.extend_from_slice(&[0u8;8]);
    out.extend(idx);
    out.extend(payload);
    Some(out)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test] fn exact_all_supported_distances(){
        for dtm in 0..=173u16{
            for (sign,outcome) in [(1i16,Outcome::Win),(-1,Outcome::Loss)]{
                let v=sign*(dtm as i16+1);
                let got=hit_from_code(code_from_v2(v).unwrap()).unwrap();
                assert_eq!(got.result,outcome);
                assert_eq!(got.dtm,dtm);
                assert_eq!(v2_from_code(code_from_v2(v).unwrap()),Some(v));
            }
        }
        assert_eq!(v2_from_code(code_from_v2(i16::MAX).unwrap()),Some(i16::MAX));
        assert!(code_from_v2(175).is_none());
    }
    #[test] fn exact_seek_across_blocks_and_gaps(){
        let mut entries=Vec::new();
        let mut key=0u64;
        for i in 0..830 {
            key+=if i%67==0{1_000_001}else{3+(i%7) as u64};
            let val=if i%11==0{i16::MAX}
                else if i%2==0{(i%174+1) as i16}
                else{-((i%174+1) as i16)};
            entries.push((key,val));
        }
        let bytes=encode_small(&entries).unwrap();
        assert!(bytes.len()<entries.len()*10);
        for &(k,v) in &entries{
            let got=probe_key_v3(&bytes,k).unwrap();
            assert_eq!(got,hit_from_code(code_from_v2(v).unwrap()).unwrap());
            assert_eq!(probe_key_v3(&bytes,k-1),None);
        }
        let mut corrupt=bytes.clone();
        corrupt[0]=0;
        assert_eq!(probe_key_v3(&corrupt,entries[1].0),None);
    }
}
