//! On-disk exact WDL/DTM lookup format for generated full tables.
//! No tablebase is bundled until all rules and canonicalization are verified.
//! A lookup miss must never silently fall back to a "perfect play" score.

use crate::{Position,retrograde::Outcome};

const MAGIC:&[u8;8]=b"DOBUTB01";
const HEADER:usize=16;
const RECORD:usize=24;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct TableHit{pub result:Outcome,pub dtm:u16}

/// Convert exact side-to-move WDL/DTM into the UI's fixed ±2000 scale.
/// Never call this on a heuristic score or an unverified tablebase result.
pub fn display_score(hit:TableHit,side_to_move:usize,player_side:usize)->Option<i32>{
    if side_to_move>1 || player_side>1{return None;}
    let value=2000i32.saturating_sub(hit.dtm as i32).max(1);
    let my_turn=side_to_move==player_side;
    Some(match hit.result {
        Outcome::Draw=>0,
        Outcome::Win=>if my_turn{value}else{-value},
        Outcome::Loss=>if my_turn{-value}else{value},
    })
}

pub fn probe(bytes:&[u8],position:&Position)->Option<TableHit>{
    let key=position.key()?;
    if bytes.len()<HEADER || &bytes[..8]!=MAGIC{return None;}
    let count=u64::from_le_bytes(bytes[8..16].try_into().ok()?) as usize;
    if bytes.len()!=HEADER.checked_add(count.checked_mul(RECORD)?)? {return None;}
    let mut lo=0usize;
    let mut hi=count;
    while lo<hi {
        let m=lo+(hi-lo)/2;
        let start=HEADER+m*RECORD;
        let probe_key=u128::from_le_bytes(bytes[start..start+16].try_into().ok()?);
        if probe_key<key{lo=m+1;}else{hi=m;}
    }
    if lo>=count{return None;}
    let pos=HEADER+lo*RECORD;
    if u128::from_le_bytes(bytes[pos..pos+16].try_into().ok()?)!=key {
        return None;
    }
    let outcome=match bytes[pos+16] {
        1=>Outcome::Win,
        2=>Outcome::Loss,
        3=>Outcome::Draw,
        _=>return None
    };
    let dtm=u16::from_le_bytes(bytes[pos+17..pos+19].try_into().ok()?);
    Some(TableHit{result:outcome,dtm})
}

/// Disk-backed generator's compact sorted 10-byte records.
/// Header: "DOBUTB02" + u64 record count.
/// Record: u64 exact position key + i16 signed (DTM+1) from side to move.
/// Positive: win, negative: loss, i16::MAX: draw. No omitted states or
/// ambiguous cache fallback is allowed.
pub fn probe_v2(bytes:&[u8],position:&Position)->Option<TableHit>{
    let key=position.key_u64()?;
    if bytes.len()<16 || &bytes[..8]!=b"DOBUTB02" {return None;}
    let count=u64::from_le_bytes(bytes[8..16].try_into().ok()?) as usize;
    if 16usize.checked_add(count.checked_mul(10)?)?!=bytes.len() {
        return None;
    }
    let mut lo=0usize;
    let mut hi=count;
    while lo<hi {
        let mid=lo+(hi-lo)/2;
        let offset=16+mid*10;
        let recorded=u64::from_le_bytes(
            bytes[offset..offset+8].try_into().ok()?);
        if recorded<key{lo=mid+1;}else{hi=mid;}
    }
    if lo==count{return None;}
    let offset=16+lo*10;
    let found=u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
    if found!=key{return None;}
    let v=i16::from_le_bytes(bytes[offset+8..offset+10].try_into().ok()?);
    if v==i16::MAX{return Some(TableHit{result:Outcome::Draw,dtm:0});}
    if v==0 || v==i16::MIN{return None;}
    Some(TableHit{
        result:if v>0 {Outcome::Win}else{Outcome::Loss},
        dtm:v.unsigned_abs()-1,
    })
}

/// Used only to pack *generated and verified* results, not to invent them.
pub fn encode_sorted(entries:&[(u128,TableHit)])->Vec<u8>{
    let mut bytes=Vec::with_capacity(HEADER+entries.len()*RECORD);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (index,(key,val)) in entries.iter().enumerate(){
        assert!(index==0 || entries[index-1].0 < *key);
        bytes.extend_from_slice(&key.to_le_bytes());
        bytes.push(match val.result {
            Outcome::Win=>1,Outcome::Loss=>2,Outcome::Draw=>3
        });
        bytes.extend_from_slice(&val.dtm.to_le_bytes());
        bytes.extend_from_slice(&[0;5]);
    }
    bytes
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::{Position,PLAYER};
    #[test] fn correct_player_perspective(){
        let win=TableHit{result:Outcome::Win,dtm:12};
        assert_eq!(display_score(win,0,0),Some(1988));
        assert_eq!(display_score(win,1,0),Some(-1988));
        let loss=TableHit{result:Outcome::Loss,dtm:8};
        assert_eq!(display_score(loss,0,0),Some(-1992));
        assert_eq!(display_score(loss,1,0),Some(1992));
    }
    #[test] fn compact_v2_perspective_and_no_fake_hit(){
        let p=Position::from_game(
            &[0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0,0x02],
            &[],Some(PLAYER)).unwrap();
        let mut bytes=Vec::new();
        bytes.extend_from_slice(b"DOBUTB02");
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&p.key_u64().unwrap().to_le_bytes());
        bytes.extend_from_slice(&(-79i16).to_le_bytes());
        assert_eq!(probe_v2(&bytes,&p),
            Some(TableHit{result:Outcome::Loss,dtm:78}));
        let mut other=p.clone();
        other.side_to_move=Some(1-PLAYER);
        assert_eq!(probe_v2(&bytes,&other),None);
        bytes.push(0);
        assert_eq!(probe_v2(&bytes,&p),None);
    }
    #[test] fn roundtrip(){
        let pos=Position::from_game(
            &[0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0,0x02],
            &[],Some(PLAYER)).unwrap();
        let data=encode_sorted(&[(pos.key().unwrap(),TableHit{
            result:Outcome::Loss,dtm:78
        })]);
        assert_eq!(probe(&data,&pos),Some(TableHit{
            result:Outcome::Loss,dtm:78
        }));
    }
}
