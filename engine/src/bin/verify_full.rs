//! Independent streaming verification of a completed tablebase.
//! Does not load 2.5 GB into memory.
use dobutsu_solver::{Position,PLAYER};
use std::{env,fs::File,io::{self,Read},process};

fn verify(path:&str)->Result<(),String>{
    let mut f=File::open(path).map_err(|e|e.to_string())?;
    let size=f.metadata().map_err(|e|e.to_string())?.len();
    let mut hdr=[0u8;16];
    f.read_exact(&mut hdr).map_err(|e|e.to_string())?;
    if &hdr[..8]!=b"DOBUTB02"{return Err("wrong magic".into());}
    let count=u64::from_le_bytes(hdr[8..16].try_into().unwrap());
    if count.checked_mul(10).and_then(|n|n.checked_add(16))!=Some(size){
        return Err("wrong file length or record count".into());
    }
    let init=Position::from_game(
        &[0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0,0x02],
        &[],Some(PLAYER)).unwrap();
    let initial_key=init.canonical_key_u64().unwrap();
    let mut last=None::<u64>;
    let mut start_score=None::<i16>;
    let mut wins=0u64;
    let mut losses=0u64;
    let mut draws=0u64;
    let mut max_distance=0u32;
    let mut row=[0u8;10];
    for index in 0..count {
        f.read_exact(&mut row).map_err(|e|e.to_string())?;
        let k=u64::from_le_bytes(row[..8].try_into().unwrap());
        let v=i16::from_le_bytes(row[8..10].try_into().unwrap());
        if last.is_some_and(|prev|k<=prev) {
            return Err(format!("non-increasing or duplicated key at record {index}"));
        }
        last=Some(k);
        if v==0 || v==i16::MIN {
            return Err(format!("invalid WDL/DTM encoding at record {index}"));
        }
        if v==i16::MAX{draws+=1;}
        else{
            if v>0{wins+=1;}else{losses+=1;}
            max_distance=max_distance.max(v.unsigned_abs() as u32-1);
        }
        if k==initial_key{start_score=Some(v);}
    }
    let result=start_score.ok_or("initial position missing")?;
    println!("Table records={count}; wins={wins}; losses={losses}; draws={draws}; max-DTM={max_distance}");
    println!("Initial position compact score: {result} (minus 1 DTM encoding)");
    // If the published reachable state count and exact start position do
    // not match, treat the file as an *experimental variant table*, not
    // the verified standard Dobutsu full solution.
    if count!=246_803_167 {
        return Err(format!("reachable-state count mismatch: expected 246803167, obtained {count}"));
    }
    if result!=-79 {return Err(format!("initial perfect-play mismatch: expected -79, got {result}"));}
    if max_distance!=173 {
        return Err(format!("maximum DTM mismatch: expected 173, got {max_distance}"));
    }
    println!("Verified against three independent published solution invariants.");
    Ok(())
}
fn main(){
    let input=env::args().nth(1).unwrap_or_else(||{
        eprintln!("Usage: verify-full <path-to-DOBUTB02>");
        process::exit(2)
    });
    if let Err(err)=verify(&input){
        eprintln!("TABLE NOT VERIFIED: {err}");
        process::exit(3)
    }
}
