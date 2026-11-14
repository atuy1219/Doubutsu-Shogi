//! Generic WDL+DTM retrograde propagation used by the future full solver.
//! A precomputed graph is required. Unknown cyclic states are draws.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {Win,Loss,Draw}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResultState {pub outcome:Outcome,pub dtm:u32}

/// Exact minimax distance for all determined nodes in a finite graph.
/// A Win chooses a move to Loss with minimum DTM; a Loss maximizes DTM.
/// Unresolved directed cycles are Draw.
pub fn solve(successors:&[Vec<usize>],terminals:&[Option<Outcome>])
    -> Result<Vec<ResultState>,&'static str>
{
    let n=successors.len();
    if terminals.len()!=n {return Err("terminal count differs from nodes");}
    let mut predecessors=vec![Vec::<usize>::new();n];
    let mut remaining=vec![0usize;n];
    let mut max_win_successor=vec![0u32;n];
    let mut result=vec![None::<ResultState>;n];
    let mut heap=BinaryHeap::<Reverse<(u32,usize)>>::new();
    for (node,next) in successors.iter().enumerate(){
        for &to in next{
            if to>=n{return Err("successor out of range");}
            predecessors[to].push(node);
        }
        remaining[node]=next.len();
    }
    for i in 0..n {
        if let Some(outcome)=terminals[i] {
            result[i]=Some(ResultState{outcome,dtm:0});
            heap.push(Reverse((0,i)));
        } else if successors[i].is_empty() {
            result[i]=Some(ResultState{outcome:Outcome::Loss,dtm:0});
            heap.push(Reverse((0,i)));
        }
    }
    while let Some(Reverse((dist,node)))=heap.pop(){
        let Some(current)=result[node] else {continue;};
        if current.dtm!=dist {continue;}
        for &pred in &predecessors[node]{
            if result[pred].is_some(){continue;}
            match current.outcome {
                Outcome::Loss=>{
                    let value=ResultState{outcome:Outcome::Win,dtm:dist+1};
                    result[pred]=Some(value);
                    heap.push(Reverse((value.dtm,pred)));
                }
                Outcome::Win=>{
                    remaining[pred]-=1;
                    max_win_successor[pred]=max_win_successor[pred].max(dist);
                    if remaining[pred]==0{
                        let value=ResultState{outcome:Outcome::Loss,
                            dtm:max_win_successor[pred]+1};
                        result[pred]=Some(value);
                        heap.push(Reverse((value.dtm,pred)));
                    }
                }
                Outcome::Draw=>{}
            }
        }
    }
    Ok(result.into_iter().map(|x|x.unwrap_or(ResultState{
        outcome:Outcome::Draw,dtm:0,
    })).collect())
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test] fn forced_mate_line(){
        let g=vec![vec![1],vec![2],vec![]];
        let r=solve(&g,&[None,None,Some(Outcome::Loss)]).unwrap();
        assert_eq!(r[0],ResultState{outcome:Outcome::Loss,dtm:2});
        assert_eq!(r[1],ResultState{outcome:Outcome::Win,dtm:1});
    }
    #[test] fn unresolved_cycle_draw(){
        let g=vec![vec![1],vec![0]];
        let r=solve(&g,&[None,None]).unwrap();
        assert!(r.iter().all(|x|x.outcome==Outcome::Draw));
    }
    #[test] fn winning_player_prefers_fast_line(){
        let g=vec![vec![1,3],vec![2],vec![],vec![]];
        let r=solve(&g,&[None,None,Some(Outcome::Loss),Some(Outcome::Loss)]).unwrap();
        assert_eq!(r[0],ResultState{outcome:Outcome::Win,dtm:1});
    }
}
