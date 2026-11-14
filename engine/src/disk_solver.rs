//! Full-state retrograde solver using disk-backed memory-mapped storage.
//!
//! This is an independent, exact (not approximate) graph algorithm. It
//! exhaustively enumerates positions reachable from the supplied initial
//! position, then solves Win/Loss/Draw and optimal DTM by fixed-point
//! propagation. It does NOT output a table when it reaches the state cap,
//! fails to converge, or fails verification. The full standard game has
//! >200 million positions and needs substantial CPU and disk I/O.
//!
//! MEMORY: open-addressed 64-bit keys, four-byte slot ids, one outcome byte
//! and one DTM byte per hash slot are memory-mapped files. Disk files
//! require roughly 8–15 GB at full scale, depending on the output format.
//!
//! Limitations before interpreting a full run as the reference solution:
//! game-specific Try, repetition and move legality must be cross-checked;
//! no identical count/initial result is assumed until it is measured.

use crate::{Position,retrograde::Outcome};
use memmap2::{MmapMut, MmapOptions};
use std::{collections::BinaryHeap, cmp::Reverse,fs::{self,File,OpenOptions},
    io::{self,BufReader,BufWriter,Read,Write},path::{Path,PathBuf}};

const EMPTY_SLOT:u64=0;
const WIN:u8=1;
const LOSS:u8=2;
const DRAW:u8=3;
const PENDING:u8=0;

#[derive(Debug)]
pub enum SolveError {
    Io(io::Error),
    InvalidCapacity,
    MissingTurn,
    ExceededMaxPositions(usize),
    MissingSuccessor(u64),
    InvalidPosition(u64),
    NonConvergent,
    DistanceOverflow,
    CorruptRecord,
    IncorrectStateCount{expected:u64,actual:u64},
}
impl From<io::Error> for SolveError {
    fn from(value:io::Error)->Self{Self::Io(value)}
}

#[derive(Clone,Copy,Debug)]
pub struct SolveSettings {
    pub max_positions:usize,
    pub hash_capacity:usize, // must be power of two and <= u32::MAX
    pub max_passes:usize,
    pub chunk_records:usize,
    pub expected_positions:Option<u64>, // reject an incorrect orbit count early
}
impl Default for SolveSettings {
    fn default()->Self{
        Self{max_positions:300_000_000,hash_capacity:1usize<<29,
            max_passes:512,chunk_records:3_000_000,expected_positions:None}
    }
}

#[derive(Clone,Copy,Debug,Default)]
pub struct SolveStats {
    pub positions:u64,
    pub legal_edges:u64,
    pub wins:u64,
    pub losses:u64,
    pub draws:u64,
    pub wdl_passes:u32,
    pub dtm_passes:u32,
    pub deepest_dtm:u8,
}

fn create_map(root:&Path,name:&str,length:usize)->Result<MmapMut,SolveError>{
    let f=OpenOptions::new().create(true).read(true).write(true)
        .truncate(true).open(root.join(name))?;
    f.set_len(length as u64)?;
    let mmap=unsafe {MmapOptions::new().len(length).map_mut(&f)?};
    Ok(mmap)
}
fn load_u64(map:&MmapMut,index:usize)->u64 {
    let off=index*8;
    u64::from_le_bytes(map[off..off+8].try_into().unwrap())
}
fn store_u64(map:&mut MmapMut,index:usize,value:u64){
    let off=index*8;
    map[off..off+8].copy_from_slice(&value.to_le_bytes());
}
fn load_u32(map:&MmapMut,index:usize)->u32 {
    let off=index*4;
    u32::from_le_bytes(map[off..off+4].try_into().unwrap())
}
fn store_u32(map:&mut MmapMut,index:usize,value:u32){
    let off=index*4;
    map[off..off+4].copy_from_slice(&value.to_le_bytes());
}
fn mix64(mut x:u64)->u64 {
    x=x.wrapping_add(0x9e3779b97f4a7c15);
    x=(x^(x>>30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x=(x^(x>>27)).wrapping_mul(0x94d049bb133111eb);
    x^(x>>31)
}

struct DiskStates {
    hash:MmapMut,
    order:MmapMut,
    wdl:MmapMut,
    depth:MmapMut,
    // Remaining winning successors / longest known child DTM, one byte
    // per hash slot. These counters are also memory-mapped to bound RSS.
    remaining:MmapMut,
    longest:MmapMut,
    mask:usize,
    used:usize,
    max_states:usize,
}
impl DiskStates {
    fn new(root:&Path,cfg:SolveSettings)->Result<Self,SolveError>{
        if cfg.hash_capacity<16 || !cfg.hash_capacity.is_power_of_two() ||
           cfg.hash_capacity>u32::MAX as usize ||
           cfg.max_positions<1 ||
           cfg.max_positions*4 > cfg.hash_capacity*3 ||
           cfg.chunk_records==0 { return Err(SolveError::InvalidCapacity); }
        fs::create_dir_all(root)?;
        Ok(Self{
            hash:create_map(root,"hash.bin",cfg.hash_capacity*8)?,
            order:create_map(root,"enumeration.bin",cfg.max_positions*4)?,
            wdl:create_map(root,"wdl.bin",cfg.hash_capacity)?,
            depth:create_map(root,"dtm.bin",cfg.hash_capacity)?,
            remaining:create_map(root,"remaining.bin",cfg.hash_capacity)?,
            longest:create_map(root,"longest.bin",cfg.hash_capacity)?,
            mask:cfg.hash_capacity-1,
            used:0,
            max_states:cfg.max_positions,
        })
    }
    /// All legal keys are >0. The +1 maps the zero sentinel away from
    /// any possible key, with checked overflow.
    fn find(&self,key:u64)->Option<usize>{
        let target=key.checked_add(1)?;
        let mut slot=mix64(key) as usize & self.mask;
        loop {
            let item=load_u64(&self.hash,slot);
            if item==EMPTY_SLOT {return None;}
            if item==target {return Some(slot);}
            slot=(slot+1)&self.mask;
        }
    }
    fn insert(&mut self,key:u64)->Result<(usize,bool),SolveError>{
        let target=key.checked_add(1).ok_or(SolveError::InvalidCapacity)?;
        let mut slot=mix64(key) as usize & self.mask;
        loop {
            let old=load_u64(&self.hash,slot);
            if old==target{return Ok((slot,false));}
            if old==EMPTY_SLOT{
                if self.used>=self.max_states{
                    return Err(SolveError::ExceededMaxPositions(self.used));
                }
                store_u64(&mut self.hash,slot,target);
                store_u32(&mut self.order,self.used,slot as u32);
                self.used+=1;
                return Ok((slot,true));
            }
            slot=(slot+1)&self.mask;
        }
    }
    fn nth(&self,index:usize)->Result<(usize,Position),SolveError>{
        let slot=load_u32(&self.order,index) as usize;
        let value=load_u64(&self.hash,slot);
        if value==0{return Err(SolveError::CorruptRecord);}
        let key=value-1;
        let pos=Position::from_key_u64(key)
            .ok_or(SolveError::InvalidPosition(key))?;
        Ok((slot,pos))
    }
    fn child_slots(&self,pos:&Position)->Result<Vec<usize>,SolveError>{
        let moves=pos.moves();
        let mut nodes=Vec::with_capacity(moves.len());
        for m in moves {
            let q=pos.apply_unchecked(m).ok_or(SolveError::CorruptRecord)?;
            let key=q.canonical_key_u64().ok_or(SolveError::MissingTurn)?;
            let slot=self.find(key).ok_or(SolveError::MissingSuccessor(key))?;
            nodes.push(slot);
        }
        Ok(nodes)
    }
}

/// Exhaustive enumeration+fixed-point solution, with a *fail-closed* output.
/// Memory-mapped files are scratch artifacts; only the complete validated
/// DOBUTB02 output is an actual tablebase.
pub fn generate_full(
    initial:Position, scratch:&Path, output:&Path,
    cfg:SolveSettings,
)->Result<SolveStats,SolveError>{
    let mut db=DiskStates::new(scratch,cfg)?;
    let first=initial.canonical_key_u64().ok_or(SolveError::MissingTurn)?;
    db.insert(first)?;
    let mut cursor=0usize;
    let mut stats=SolveStats::default();

    // 1. COMPLETE graph closure, breadth-first. No status computation yet.
    while cursor<db.used {
        let (_,pos)=db.nth(cursor)?;
        cursor+=1;
        if pos.terminal_for_tablebase().is_some() {continue;}
        let moves=pos.moves();
        stats.legal_edges+=moves.len() as u64;
        for m in moves{
            let child=pos.apply_unchecked(m).ok_or(SolveError::CorruptRecord)?;
            let key=child.canonical_key_u64().ok_or(SolveError::MissingTurn)?;
            db.insert(key)?;
        }
        if cursor%1_000_000==0 {
            eprintln!("enumeration {cursor} / {} reachable",db.used);
        }
    }
    stats.positions=db.used as u64;
    if let Some(expected)=cfg.expected_positions {
        if stats.positions!=expected {
            return Err(SolveError::IncorrectStateCount {
                expected,actual:stats.positions
            });
        }
    }
    eprintln!("Enumerated {} positions, {} edges; WDL propagation",
        stats.positions,stats.legal_edges);

    // 2. One-time outdegree calculation, terminal labeling and priority
    // buckets for shortest Win / longest Loss distance. No repeated
    // million-state scans, no in-memory graph of all predecessor edges.
    //
    // At a terminal or zero-move node the DTM is 0 => stored depth=1.
    // Each discovered result enters the bucket of its depth+1.
    // The predecessor callback reconstructs all reverse legal moves
    // on demand and joins only states present in this *closed* graph.
    let levels=256usize;
    let mut buckets:Vec<Option<BufWriter<File>>>=
        (0..levels).map(|_|None).collect();
    let bucket_path=|d:usize|{
        scratch.join(format!("retro_{d:03}.bin"))
    };
    let mut pending_count=0usize;
    for i in 0..db.used {
        let (slot,pos)=db.nth(i)?;
        if let Some(outcome)=pos.terminal_for_tablebase() {
            db.wdl[slot]=if outcome==Outcome::Win {WIN}else{LOSS};
            db.depth[slot]=1;
        } else {
            // Count successor ORBITS: two different moves may lead to
            // positions related by reflection. They must only decrement
            // their parent's remaining counter once during retrograde.
            let moves=pos.moves();
            let mut children=Vec::with_capacity(moves.len());
            for mv in moves{
                let child=pos.apply_unchecked(mv).ok_or(SolveError::CorruptRecord)?;
                children.push(child.canonical_key_u64()
                    .ok_or(SolveError::MissingTurn)?);
            }
            children.sort_unstable();
            children.dedup();
            let count=children.len();
            if count>u8::MAX as usize {
                return Err(SolveError::CorruptRecord);
            }
            db.remaining[slot]=count as u8;
            if count==0 {
                db.wdl[slot]=LOSS;
                db.depth[slot]=1;
            } else {
                pending_count+=1;
            }
        }
        if db.depth[slot]!=0 {
            if buckets[1].is_none() {
                buckets[1]=Some(BufWriter::new(File::create(bucket_path(1))?));
            }
            buckets[1].as_mut().unwrap().write_all(&(slot as u32).to_le_bytes())?;
        }
    }

    // 3. Retrograde sweep in strictly increasing DTM order.
    // A node with a losing child is Win (the first losing child gives
    // shortest DTM). A node whose *all* children are Win is Loss, with
    // longest child DTM. Unresolved cycles remain draws.
    let mut processed=0u64;
    let mut max_depth=0usize;
    for level in 1..levels {
        if level>cfg.max_passes {
            break;
        }
        if buckets[level].is_none(){continue;}
        buckets[level].as_mut().unwrap().flush()?;
        // Once flushed, close the writer, then sequentially read every
        // slot at this depth. Any generated parent is necessarily at a
        // strictly higher depth, so no bucket is read while appended.
        buckets[level].take();
        let path=bucket_path(level);
        let mut reader=BufReader::new(File::open(&path)?);
        while let Some(slot)=read_slot(&mut reader)? {
            let slot=slot as usize;
            if slot>=db.hash.len()/8{return Err(SolveError::CorruptRecord);}
            let value=load_u64(&db.hash,slot);
            if value==0 || db.depth[slot] as usize!=level ||
               (db.wdl[slot]!=WIN && db.wdl[slot]!=LOSS) {
                return Err(SolveError::CorruptRecord);
            }
            let node=Position::from_key_u64(value-1)
                .ok_or(SolveError::InvalidPosition(value-1))?;
            processed+=1;
            max_depth=max_depth.max(level-1);

            // No other reverse-engineering library is used: the
            // standalone Rust rules engine reconstructs predecessors
            // and validates them by forward move replay.
            let mut seen_parent_orbits=Vec::new();
            for parent in node.predecessors(){
                let key=parent.canonical_key_u64()
                    .ok_or(SolveError::MissingTurn)?;
                let Some(pslot)=db.find(key) else{continue};
                if seen_parent_orbits.contains(&pslot){continue;}
                seen_parent_orbits.push(pslot);
                if db.wdl[pslot]!=PENDING {continue;}
                if db.wdl[slot]==LOSS {
                    // Earliest losing child provides shortest winning DTM.
                    db.wdl[pslot]=WIN;
                    db.depth[pslot]=(level as u8).checked_add(1)
                        .ok_or(SolveError::DistanceOverflow)?;
                }else{
                    // A player is lost only when every legal reply
                    // leads to an already known winning child.
                    if db.remaining[pslot]==0 {
                        return Err(SolveError::CorruptRecord);
                    }
                    db.remaining[pslot]-=1;
                    db.longest[pslot]=db.longest[pslot].max(level as u8);
                    if db.remaining[pslot]!=0 {continue;}
                    db.wdl[pslot]=LOSS;
                    db.depth[pslot]=db.longest[pslot].checked_add(1)
                        .ok_or(SolveError::DistanceOverflow)?;
                }
                pending_count-=1;
                let next=db.depth[pslot] as usize;
                if next>=levels || next>cfg.max_passes{
                    return Err(SolveError::NonConvergent);
                }
                if buckets[next].is_none(){
                    buckets[next]=Some(BufWriter::new(File::create(bucket_path(next))?));
                }
                buckets[next].as_mut().unwrap()
                    .write_all(&(pslot as u32).to_le_bytes())?;
            }
        }
        fs::remove_file(path)?;
        if level%10==0 || processed%1_000_000==0 {
            eprintln!("Retrograde distance {}: {} solved, {} unsettled",
                level-1,processed,pending_count);
        }
    }
    // After all depth buckets have been consumed, positions that
    // never entered the attractor are exact Draw under the selected
    // repetition convention. Do not label draws after an early cap.
    if buckets.iter().any(Option::is_some){
        return Err(SolveError::NonConvergent);
    }
    stats.wdl_passes=1;
    stats.dtm_passes=max_depth as u32;
    for i in 0..db.used {
        let slot=load_u32(&db.order,i) as usize;
        if db.wdl[slot]==PENDING {db.wdl[slot]=DRAW;}
    }

    // 5. No unresolved Win/Loss entries may be serialized. DTM is u8,
    // allowing depths up to 254 plies (published maximum ~173).
    for i in 0..db.used {
        let slot=load_u32(&db.order,i) as usize;
        match db.wdl[slot] {
            WIN => {stats.wins+=1;},
            LOSS=> {stats.losses+=1;},
            DRAW=> {stats.draws+=1;},
            _=>return Err(SolveError::CorruptRecord),
        }
        if db.wdl[slot]!=DRAW {
            if db.depth[slot]==0{return Err(SolveError::NonConvergent);}
            stats.deepest_dtm=stats.deepest_dtm.max(db.depth[slot]-1);
        }
    }

    // 6. External sorted-run construction and k-way merging. No vector
    // of all hundreds of millions of (key,score) records in RAM.
    let mut part_paths=Vec::<PathBuf>::new();
    for start in (0..db.used).step_by(cfg.chunk_records) {
        let stop=(start+cfg.chunk_records).min(db.used);
        let mut batch=Vec::<(u64,i16)>::with_capacity(stop-start);
        for idx in start..stop {
            let slot=load_u32(&db.order,idx) as usize;
            let key=load_u64(&db.hash,slot)-1;
            let score=match db.wdl[slot] {
                WIN=>db.depth[slot] as i16,
                LOSS=>-(db.depth[slot] as i16),
                DRAW=>i16::MAX,
                _=>return Err(SolveError::CorruptRecord)
            };
            batch.push((key,score));
        }
        batch.sort_unstable_by_key(|x|x.0);
        let file_path=scratch.join(format!("sorted_{:05}.bin",part_paths.len()));
        let mut writer=BufWriter::new(File::create(&file_path)?);
        for (k,v) in batch {
            writer.write_all(&k.to_le_bytes())?;
            writer.write_all(&v.to_le_bytes())?;
        }
        writer.flush()?;
        part_paths.push(file_path);
    }
    let tmp=output.with_extension("incomplete");
    let out=File::create(&tmp)?;
    let mut writer=BufWriter::new(out);
    writer.write_all(b"DOBUTB02")?;
    writer.write_all(&stats.positions.to_le_bytes())?;
    let mut readers:Vec<BufReader<File>>=part_paths.iter()
        .map(|p|File::open(p).map(BufReader::new))
        .collect::<Result<_,_>>()?;
    let mut heap=BinaryHeap::<Reverse<(u64,i16,usize)>>::new();
    for (id,r) in readers.iter_mut().enumerate() {
        if let Some((k,v))=read_record(r)?{heap.push(Reverse((k,v,id)));}
    }
    let mut written=0u64;
    let mut previous=None::<u64>;
    while let Some(Reverse((key,val,id)))=heap.pop(){
        if let Some(last)=previous {
            if key<=last{return Err(SolveError::CorruptRecord);}
        }
        previous=Some(key);
        writer.write_all(&key.to_le_bytes())?;
        writer.write_all(&val.to_le_bytes())?;
        written+=1;
        if let Some((k,v))=read_record(&mut readers[id])?{
            heap.push(Reverse((k,v,id)));
        }
    }
    writer.flush()?;
    drop(writer);
    if written!=stats.positions{return Err(SolveError::CorruptRecord);}
    // The table header and sorted key uniqueness have been validated.
    fs::rename(&tmp,output)?;
    eprintln!("Finished complete WDL/DTM table: {} records; stats={stats:?}",
        written);
    Ok(stats)
}
fn read_slot<R:Read>(r:&mut R)->Result<Option<u32>,SolveError>{
    let mut bytes=[0u8;4];
    match r.read_exact(&mut bytes){
        Ok(())=>Ok(Some(u32::from_le_bytes(bytes))),
        Err(e) if e.kind()==io::ErrorKind::UnexpectedEof=>Ok(None),
        Err(e)=>Err(e.into())
    }
}

fn read_record<R:Read>(r:&mut R)->Result<Option<(u64,i16)>,SolveError>{
    let mut key=[0u8;8];
    match r.read_exact(&mut key){
        Ok(())=>{}
        Err(e) if e.kind()==io::ErrorKind::UnexpectedEof=>return Ok(None),
        Err(e)=>return Err(e.into())
    }
    let mut val=[0u8;2];
    r.read_exact(&mut val)?;
    Ok(Some((u64::from_le_bytes(key),i16::from_le_bytes(val))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PLAYER,OPPONENT};
    fn fixture(terminal:bool)->Position{
        let mut raw=[0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0x00,0x02];
        if terminal{raw[1]=-1;}
        Position::from_game(&raw,&[],Some(if terminal{OPPONENT}else{PLAYER}))
            .unwrap()
    }
    #[test] fn hash_lookup_survives_linear_collisions() {
        let work=std::env::temp_dir().join(format!("dobutsu_disk_hash_{}",std::process::id()));
        let cfg=SolveSettings{max_positions:50,hash_capacity:128,max_passes:8,chunk_records:5,expected_positions:None};
        let mut d=DiskStates::new(&work,cfg).unwrap();
        for i in 0..45 {
            assert!(d.insert((i*71+37) as u64).unwrap().1);
        }
        for i in 0..45{
            assert!(d.find((i*71+37) as u64).is_some());
            assert!(!d.insert((i*71+37) as u64).unwrap().1);
        }
        drop(d);
        fs::remove_dir_all(&work).unwrap();
    }
    #[test] fn can_solve_terminal_and_merge_table(){
        let work=std::env::temp_dir().join(format!("dobutsu_disk_test_{}",std::process::id()));
        let out=work.join("tiny.tb");
        let cfg=SolveSettings{max_positions:30,hash_capacity:64,max_passes:8,chunk_records:1,expected_positions:None};
        let p=fixture(true);
        let stats=generate_full(p.clone(),&work,&out,cfg).unwrap();
        assert_eq!(stats.positions,1);
        assert_eq!(stats.losses,1);
        let bytes=fs::read(out).unwrap();
        assert_eq!(&bytes[..8],b"DOBUTB02");
        assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()),1);
        assert_eq!(u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
                   p.canonical_key_u64().unwrap());
        assert_eq!(i16::from_le_bytes(bytes[24..26].try_into().unwrap()),-1);
        fs::remove_dir_all(&work).unwrap();
    }
    #[test] fn refuses_frontier_truncation(){
        let work=std::env::temp_dir().join(format!("dobutsu_disk_limit_{}",std::process::id()));
        let out=work.join("partial.tb");
        let cfg=SolveSettings{max_positions:1,hash_capacity:64,max_passes:8,chunk_records:1,expected_positions:None};
        assert!(matches!(generate_full(fixture(false),&work,&out,cfg),
                         Err(SolveError::ExceededMaxPositions(1))));
        assert!(!out.exists());
        fs::remove_dir_all(&work).unwrap();
    }
}
