//! Command-line driver for the correctness-first, memory-bounded solver.
//! Run on a desktop with: cargo run --release --features generator --bin generate
//!     -- --output table.tb --max-states 1000000
//! The run aborts instead of writing an incomplete/misleading table.

use dobutsu_solver::{builder, Position, PLAYER};
use std::{env, fs, process, time::Instant};

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut max_states = 1_000_000usize;
    let mut output = String::from("dobutsu.tb");
    let mut fixture = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--max-states" if i+1<args.len() => {
                i+=1;
                max_states=args[i].parse().unwrap_or_else(|_| {
                    eprintln!("invalid --max-states");process::exit(2)
                });
            }
            "--output" if i+1<args.len() => {i+=1;output=args[i].clone();}
            "--terminal-fixture" => {fixture=true;}
            "--help" => {
                println!("--max-states N --output PATH [--terminal-fixture]");
                println!("An incomplete graph is never serialized.");
                return;
            }
            other => {eprintln!("Unknown argument: {other}");process::exit(2);}
        }
        i+=1;
    }
    let mut raw = [
        0x23,0x21,0x25,
        -1,0x27,-1,
        -1,0x06,-1,
        0x04,0x00,0x02,
    ];
    if fixture {raw[1]=-1;}
    let pos=Position::from_game(&raw,&[],Some(PLAYER))
        .expect("initial fixture must be valid");
    let started=Instant::now();
    let generated=match builder::generate(pos,max_states){
        Ok(result)=>result,
        Err(error)=>{
            eprintln!("No table written; graph incomplete or invalid: {error:?}");
            process::exit(3);
        }
    };
    fs::write(&output,generated.to_bytes()).expect("cannot write table");
    eprintln!("Completed graph: {:?} ({} ms); written {output}",
        generated.stats,started.elapsed().as_millis());
}
