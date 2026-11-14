//! Offline, bounded enumeration and generation of a verified WDL/DTM table.
//! This is a correctness-first reference builder. It refuses to serialize
//! anything when a configured state cap is reached: partial graphs have
//! unresolved successors and treating them as draws would be incorrect.
//! Full >200-million-position solving requires a compact/disk-backed graph.

use crate::{retrograde::{self,Outcome},tablebase::{self,TableHit},Position};
use std::collections::HashMap;

#[derive(Clone,Debug,Eq,PartialEq)]
pub enum BuildError {
    MissingTurn,
    TooManyStates {limit:usize},
    InvalidTransition,
    Graph(&'static str),
    DistanceOverflow,
}

#[derive(Clone,Debug,Default,Eq,PartialEq)]
pub struct BuildStats {
    pub positions:usize,
    pub edges:usize,
    pub wins:usize,
    pub losses:usize,
    pub draws:usize,
}

#[derive(Clone,Debug)]
pub struct CompletedTable {
    pub stats:BuildStats,
    pub entries:Vec<(u128,TableHit)>,
}

impl CompletedTable {
    pub fn to_bytes(&self)->Vec<u8> {
        tablebase::encode_sorted(&self.entries)
    }
}

/// Explore every position reachable from an initial position, then compute
/// true WDL + optimal DTM for the resulting CLOSED successor graph.
/// Aborts before solving if the full closure exceeds max_states. Never
/// mislabels unknown frontier nodes as draws or publishes a partial table.
pub fn generate(initial:Position,max_states:usize)->Result<CompletedTable,BuildError> {
    if max_states==0 {return Err(BuildError::TooManyStates{limit:0});}
    let first=initial.key().ok_or(BuildError::MissingTurn)?;
    let mut positions=vec![initial];
    let mut ids=HashMap::<u128,usize>::new();
    ids.insert(first,0);
    let mut graph:Vec<Vec<usize>>=Vec::new();
    let mut terminals:Vec<Option<Outcome>>=Vec::new();
    let mut cursor=0usize;
    while cursor<positions.len() {
        // Own the small 3x4 position while the frontier vector grows.
        // Avoid borrowing positions immutably across positions.push().
        let current=positions[cursor].clone();
        let terminal=current.terminal();
        let moves=if terminal.is_none(){current.moves()}else{Vec::new()};
        let mut edges=Vec::with_capacity(moves.len());
        for mv in moves {
            let child=current.apply(mv).ok_or(BuildError::InvalidTransition)?;
            let key=child.key().ok_or(BuildError::MissingTurn)?;
            let to=if let Some(&existing)=ids.get(&key){
                existing
            }else{
                if positions.len()>=max_states {
                    return Err(BuildError::TooManyStates{limit:max_states});
                }
                let next=positions.len();
                ids.insert(key,next);
                positions.push(child);
                next
            };
            edges.push(to);
        }
        graph.push(edges);
        terminals.push(terminal);
        cursor+=1;
    }
    let results=retrograde::solve(&graph,&terminals).map_err(BuildError::Graph)?;
    let mut stats=BuildStats{positions:positions.len(),
        edges:graph.iter().map(Vec::len).sum(),..Default::default()};
    let mut entries=Vec::with_capacity(positions.len());
    for (pos,result) in positions.iter().zip(results.iter()) {
        match result.outcome {
            Outcome::Win=>stats.wins+=1,
            Outcome::Loss=>stats.losses+=1,
            Outcome::Draw=>stats.draws+=1,
        }
        let dtm=u16::try_from(result.dtm).map_err(|_|BuildError::DistanceOverflow)?;
        entries.push((
            pos.key().ok_or(BuildError::MissingTurn)?,
            TableHit{result:result.outcome,dtm},
        ));
    }
    entries.sort_unstable_by_key(|item|item.0);
    Ok(CompletedTable{stats,entries})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PLAYER,OPPONENT};
    const INIT:[i32;12]=[
        0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0x00,0x02
    ];
    #[test] fn refuses_partial_initial_graph(){
        let p=Position::from_game(&INIT,&[],Some(PLAYER)).unwrap();
        assert!(matches!(generate(p,1),Err(BuildError::TooManyStates{limit:1})));
    }
    #[test] fn complete_one_terminal_graph_is_exact(){
        let mut board=INIT;
        board[1]=-1;
        let p=Position::from_game(&board,&[],Some(OPPONENT)).unwrap();
        let key=p.key().unwrap();
        let t=generate(p.clone(),1).unwrap();
        assert_eq!(t.stats.positions,1);
        assert_eq!(t.stats.losses,1);
        let hit=tablebase::probe(&t.to_bytes(),&p).unwrap();
        assert_eq!(hit,TableHit{result:Outcome::Loss,dtm:0});
        assert_eq!(t.entries[0].0,key);
    }
    #[test] fn cap_zero_must_abort(){
        let p=Position::from_game(&INIT,&[],Some(PLAYER)).unwrap();
        assert!(matches!(generate(p,0),Err(BuildError::TooManyStates{limit:0})));
    }
}
