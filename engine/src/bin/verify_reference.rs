//! Cross-language guard for Clausecker's C-generated all-legal table.
//! The C validator must run first; this independently checks the Rust
//! cohort indexing, data envelope and headline WDL/DTM invariants.
use std::{env,fs,process};
use dobutsu_solver::{dobutsu_evaluate_dense,dense_lib,reference_probe_value};

fn main(){
    let file=env::args().nth(1).expect("verify-reference FILE");
    let raw=fs::read(&file).expect("read complete reference table");
    let expected=dense_lib::cohort::POSITION_COUNT as usize;
    assert_eq!(raw.len(),expected+16,"incomplete table file size");
    assert_eq!(&raw[..8],b"DOBREF01","bad format");
    assert_eq!(u64::from_le_bytes(raw[8..16].try_into().unwrap()),
        expected as u64,"wrong indexed position count");
    assert_eq!(expected,167_527_962);
    let init=dense_lib::parse("S/gle/-c-/-C-/ELG/-")
        .expect("parse independent initial");
    let initial_unfolded=dense_lib::cohort::encode_checked(&init)
        .expect("opening indexed");
    let initial_idx=dense_lib::cohort::reference_stored_offset(initial_unfolded)
        .expect("opening must lie in reference stored ownership class") as usize;
    assert!(initial_idx<expected);
    println!("Opening indexes: Rust raw={initial_unfolded} C permuted={initial_idx}");
    let initial_entry=raw[16+initial_idx] as i8;
    assert_eq!(initial_entry,-39,"opening should be loss in 78 plies");
    let board=[
        0x23,0x21,0x25,-1,0x27,-1,
        -1,0x06,-1,0x04,0x00,0x02
    ];
    let app_result=dobutsu_evaluate_dense(
        raw.as_ptr(),raw.len(),board.as_ptr(),std::ptr::null(),0,0);
    assert_eq!(app_result,-1922,"Rust FFI must read the C table exactly");

    // This 173-ply showcase position is from the independent full-board
    // scan of clausecker/dobutsu (brianhliou/dobutsu-shogi research).
    // It belongs to an OMITTED ownership class. The stored 167M entries
    // alone therefore max out at 172, even though the whole exact
    // tablebase contains a 173-ply winning position.
    let deepest=dense_lib::parse("S/cgl/--e/--L/c-G/E")
        .expect("parse published 173-ply position");
    let raw_deepest=dense_lib::cohort::encode_checked(&deepest)
        .expect("173-ply example has a legal nonterminal cohort index");
    assert!(dense_lib::cohort::reference_stored_offset(raw_deepest).is_none(),
        "the published 173-ply position belongs to an omitted C ownership class");
    let deepest_e=reference_probe_value(&raw,&deepest)
        .expect("reconstruct 173-ply position from exact stored descendants");
    assert_eq!(deepest_e,87,"173-ply win is e=+87 (2*87-1 plies)");

    let mut highest=0u16;
    let mut wins=0u64;
    let mut losses=0u64;
    let mut draws=0u64;
    for &b in &raw[16..]{
        let e=b as i8;
        assert_ne!(e,i8::MIN,"uninitialized/out-of-range signed DTM");
        let depth=if e>0{
            wins+=1;
            (e as u16)*2-1
        }else if e<0{
            losses+=1;
            (-2i32*(e as i32)) as u16
        }else{
            draws+=1;
            0
        };
        highest=highest.max(depth);
    }
    assert_eq!(wins+losses+draws,expected as u64);
    assert_eq!(highest,172,"maximum DTM in C's STORED ownership classes");
    println!("Reference table verified: stored_entries={expected} stored_win={wins} stored_loss={losses} stored_draw={draws} opening=-78 stored_max_dtm={highest} exact_omitted_example_max_dtm=173");
    process::exit(0);
}
