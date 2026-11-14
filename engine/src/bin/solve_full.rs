//! Full offline WDL/DTM generation. Requires many hours of CPU for the
//! correctness-first mmap/iterative implementation and >10 GB scratch disk.
//! It is deliberately *not* run in the fast Android build workflow.
use dobutsu_solver::{Position,PLAYER,disk_solver::{generate_full,SolveSettings}};
use std::{env,path::PathBuf,process};

fn main() {
    let mut cfg=SolveSettings::default();
    let mut out=PathBuf::from("dobutsu-full.tb");
    let mut scratch=PathBuf::from("dobutsu-scratch");
    let args:Vec<_>=env::args().collect();
    let mut i=1;
    while i<args.len(){
        match args[i].as_str() {
            "--output" if i+1<args.len()=>{
                i+=1;out=PathBuf::from(&args[i]);
            }
            "--scratch" if i+1<args.len()=>{
                i+=1;scratch=PathBuf::from(&args[i]);
            }
            "--max-states" if i+1<args.len()=>{
                i+=1;cfg.max_positions=args[i].parse().unwrap();
            }
            "--hash-slots" if i+1<args.len()=>{
                i+=1;cfg.hash_capacity=args[i].parse().unwrap();
            }
            "--expect-states" if i+1<args.len()=>{
                i+=1;cfg.expected_positions=Some(args[i].parse().unwrap());
            }
            "--max-passes" if i+1<args.len()=>{
                i+=1;cfg.max_passes=args[i].parse().unwrap();
            }
            "--help" => {
                println!("--output PATH --scratch DIR --max-states N --hash-slots POWER_OF_TWO [--expect-states N]");
                return;
            }
            x=>{eprintln!("Unknown argument: {x}");process::exit(2);}
        }
        i+=1;
    }
    let board=[
        0x23,0x21,0x25,
        -1,0x27,-1,
        -1,0x06,-1,
        0x04,0x00,0x02
    ];
    let initial=Position::from_game(&board,&[],Some(PLAYER)).unwrap();
    match generate_full(initial,&scratch,&out,cfg) {
        Ok(stats)=>println!("FULL TABLE SUCCESSFULLY GENERATED: {stats:?}"),
        Err(e)=>{
            eprintln!("NO COMPLETE TABLE GENERATED: {e:?}");
            process::exit(3);
        }
    }
}
