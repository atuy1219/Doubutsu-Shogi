//! Independent position decoder and WDL/DTM tablebase reader for Dobutsu Shogi.
//! The decoder uses observed game arrays; game-specific Try/repetition
//! semantics and on-device move-side calibration still require validation.

pub mod retrograde;
pub mod tablebase;
pub mod compact;
/// MIT-licensed deterministic cohort position index (Brian Liou / Robert Clausecker).
pub mod dense_lib;
pub mod builder;
#[cfg(feature="full-solver")]
pub mod disk_solver;

use core::ffi::c_int;

pub const INVALID: i32 = i32::MIN;
pub const CELLS: usize = 12;
pub const EMPTY: i8 = 0;
pub const LION: u8 = 1;
pub const GIRAFFE: u8 = 2;
pub const ELEPHANT: u8 = 3;
pub const CHICK: u8 = 4;
pub const HEN: u8 = 5;
pub const PLAYER: usize = 0;
pub const OPPONENT: usize = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Position {
    /// 0=empty, +1..+5 human, -1..-5 opponent. The sign is ownership.
    pub cells: [i8; CELLS],
    /// Hand inventory indexed [side][giraffe, elephant, chick].
    pub hands: [[u8; 3]; 2],
    /// Only available when obtained from a verified game-state source.
    pub side_to_move: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    BadBoard,
    InvalidPiece,
    InvalidHand,
    InvalidInventory,
    NotStarted,
    BadTurn,
}

fn owner(raw: i32) -> usize {
    if raw & 0x20 == 0 { PLAYER } else { OPPONENT }
}

/// Returns owner and type, preserving +0 as the PLAYER lion.
/// The low bit appears to be a render/flip flag; it does not alter type.
pub fn decode_piece(raw: i32) -> Result<Option<(usize, u8)>, DecodeError> {
    if raw == -1 { return Ok(None); }
    if !(0..=0x3f).contains(&raw) { return Err(DecodeError::InvalidPiece); }
    let kind = match raw & 0x1e {
        0x00 => LION,
        0x02 => GIRAFFE,
        0x04 => ELEPHANT,
        0x06 => CHICK,
        0x08 => HEN,
        _ => return Err(DecodeError::InvalidPiece),
    };
    Ok(Some((owner(raw),kind)))
}

impl Position {
    pub fn from_game(board: &[i32], hands: &[i32], side: Option<usize>)
        -> Result<Self, DecodeError>
    {
        if board.len()!=CELLS { return Err(DecodeError::BadBoard); }
        if let Some(turn)=side {
            if turn>OPPONENT { return Err(DecodeError::BadTurn); }
        }
        let mut cells=[0i8;CELLS];
        let mut inhand=[[0u8;3];2];
        let mut inventory=[0u8;5];
        let mut lions=[0u8;2];
        let mut on_board=0usize;

        for (i,&raw) in board.iter().enumerate() {
            if let Some((side,kind))=decode_piece(raw)? {
                on_board+=1;
                let signed=kind as i8;
                cells[i]=if side==PLAYER {signed} else {-signed};
                inventory[(kind.min(CHICK)-1) as usize]+=1;
                if kind==LION {lions[side]+=1;}
            }
        }
        if on_board==CELLS && cells.iter().all(|x| *x==1) {
            return Err(DecodeError::NotStarted);
        }
        // motiData contains piece codes, -1 for unused slots. Promoted
        // chick becomes an unpromoted chick upon capture.
        if hands.len()>64 { return Err(DecodeError::InvalidHand); }
        for &raw in hands {
            let Some((side,kind))=decode_piece(raw)? else {continue;};
            if kind==LION {return Err(DecodeError::InvalidHand);}
            let idx=match kind {GIRAFFE=>0,ELEPHANT=>1,CHICK|HEN=>2,_=>unreachable!()};
            inhand[side][idx]=inhand[side][idx].saturating_add(1);
            let base_kind=if kind==HEN {CHICK} else {kind};
            inventory[(base_kind-1) as usize]+=1;
        }
        // Exactly two of each piece type occur in standard Dobutsu Shogi,
        // with at most 2 on the board + both players' hands combined.
        // The last position after a lion capture can have one lion.
        let terminal_lion_capture=lions.iter().sum::<u8>()==1;
        if (lions!=[1,1] && !terminal_lion_capture) ||
            inventory[LION as usize-1] != if terminal_lion_capture {1} else {2} ||
            inventory[GIRAFFE as usize-1]!=2 ||
            inventory[ELEPHANT as usize-1]!=2 ||
            inventory[CHICK as usize-1]!=2 ||
            on_board+hands.iter().filter(|&&x|x!=-1).count()!=
                if terminal_lion_capture {7} else {8}
        {
            return Err(DecodeError::InvalidInventory);
        }
        Ok(Self{cells,hands:inhand,side_to_move:side})
    }

    pub fn board_fingerprint(&self) -> u64 {
        // Stable FNV-1a across Android, Linux and compiler versions.
        let mut hash=0xcbf29ce484222325u64;
        for b in self.cells.iter().map(|v|(*v as i16+5) as u8)
            .chain(self.hands.iter().flat_map(|h|h.iter().copied()))
            .chain(core::iter::once(self.side_to_move.unwrap_or(2) as u8))
        {
            hash=(hash^b as u64).wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Exact, lossless position key. Include side to move: a board alone
    /// cannot be used to look up a WDL/DTM result.
    pub fn key(&self) -> Option<u128> {
        let turn=self.side_to_move?;
        let mut key=turn as u128;
        for &piece in &self.cells {
            key=key*11+(piece as i16+5) as u128;
        }
        for side in 0..2 {
            for &count in &self.hands[side] {
                if count>2{return None;}
                key=key*3+count as u128;
            }
        }
        Some(key)
    }

    /// Canonical table position: normalize the player to move to PLAYER
    /// (rotate the board 180 degrees and swap colours/hands if needed), then
    /// fold the a<->c board reflection. No hashes or lossy approximations.
    pub fn canonicalized(&self)->Option<Self>{
        let turn=self.side_to_move?;
        if turn>OPPONENT{return None;}
        let mut normalized=self.clone();
        if turn==OPPONENT{
            for (index,&piece) in self.cells.iter().enumerate(){
                normalized.cells[CELLS-1-index]=-piece;
            }
            normalized.hands.swap(PLAYER,OPPONENT);
            normalized.side_to_move=Some(PLAYER);
        }
        let mut mirrored=normalized.clone();
        for row in 0..4{
            for col in 0..3{
                mirrored.cells[row*3+col]=normalized.cells[row*3+2-col];
            }
        }
        if mirrored.key_u64()?<normalized.key_u64()?{
            normalized=mirrored;
        }
        Some(normalized)
    }

    pub fn canonical_key_u64(&self)->Option<u64>{
        self.canonicalized()?.key_u64()
    }

    /// Published Tanaka (2009) +1-ply Try convention. W0: a lion is
    /// capturable on this turn; L0: an opposing lion reached our home
    /// rank and cannot be captured now. Such states must not be expanded.
    /// This is intentionally separate from visual terminal() detection.
    pub fn terminal_for_tablebase(&self)->Option<retrograde::Outcome>{
        let turn=self.side_to_move?;
        if turn>OPPONENT{return None;}
        let mine=if turn==PLAYER {LION as i8}else{-(LION as i8)};
        let enemy=-mine;
        if !self.cells.contains(&mine){
            return Some(retrograde::Outcome::Loss);
        }
        let Some(enemy_square)=self.cells.iter().position(|&p|p==enemy) else{
            return Some(retrograde::Outcome::Win);
        };
        if self.is_attacked(enemy_square,turn){
            return Some(retrograde::Outcome::Win);
        }
        let enemy_on_home=if turn==PLAYER{enemy_square>=9}else{enemy_square<3};
        if enemy_on_home{Some(retrograde::Outcome::Loss)}else{None}
    }

    /// WDL is from the **side-to-move** perspective, not always the human's.
    /// Capture is decisive. A Try succeeds only if a lion on the opposing
    /// home rank is not attackable by the opponent on the next move.
    /// The terminal outcome is always relative to the side to move.
    /// Entire game position fits in 53 bits. Dense-on-disk enumeration
    /// uses this exact encoding with no hash collision or lossy symmetry.
    pub fn key_u64(&self) -> Option<u64> {
        u64::try_from(self.key()?).ok()
    }

    /// Lossless inverse of key_u64. Invoked billions of times by the
    /// disk-backed generator, avoiding a Vec<Position> in memory.
    pub fn from_key_u64(encoded:u64)->Option<Self> {
        let mut x=encoded;
        let mut hands=[[0u8;3];2];
        for side in (0..2).rev() {
            for kind in (0..3).rev() {
                hands[side][kind]=(x%3) as u8;
                x/=3;
            }
        }
        let mut cells=[0i8;CELLS];
        for square in (0..CELLS).rev() {
            cells[square]=(x%11) as i8-5;
            x/=11;
        }
        if x>1{return None;}
        Some(Self{cells,hands,side_to_move:Some(x as usize)})
    }

    pub fn terminal(&self) -> Option<retrograde::Outcome> {
        let turn=self.side_to_move?;
        let owner_of_result=if !self.cells.iter().any(|&p|p==LION as i8) {
            Some(OPPONENT)
        } else if !self.cells.iter().any(|&p|p==-(LION as i8)) {
            Some(PLAYER)
        } else {
            let mut winner=None;
            for (index,&piece) in self.cells.iter().enumerate() {
                if piece.unsigned_abs()!=LION {continue;}
                let owner=if piece>0 {PLAYER} else {OPPONENT};
                if (if owner==PLAYER {index<3} else {index>=9}) &&
                   !self.is_attacked(index,1-owner) {
                    winner=Some(owner);
                }
            }
            winner
        };
        owner_of_result.map(|winner|if winner==turn {
            retrograde::Outcome::Win
        } else {
            retrograde::Outcome::Loss
        })
    }

    /// Does the specified side attack a square? Also detects unsafe Try.
    pub fn is_attacked(&self, target:usize, attacker:usize)->bool {
        if target>=CELLS || attacker>1 {return false;}
        let to_r=(target/3) as i32;
        let to_c=(target%3) as i32;
        for (index,&piece) in self.cells.iter().enumerate() {
            if piece==EMPTY || (piece>0)!=(attacker==PLAYER) {continue;}
            let dr=to_r-(index/3) as i32;
            let dc=to_c-(index%3) as i32;
            let fwd=if attacker==PLAYER {-1} else {1};
            let ok=match piece.unsigned_abs() {
                LION=>dr.abs()<=1 && dc.abs()<=1 && (dr!=0 || dc!=0),
                GIRAFFE=>dr.abs()+dc.abs()==1,
                ELEPHANT=>dr.abs()==1 && dc.abs()==1,
                CHICK=>dr==fwd && dc==0,
                HEN=>(dr==fwd && dc.abs()<=1) ||
                    (dr==0 && dc.abs()==1) || (dr==-fwd && dc==0),
                _=>false,
            };
            if ok {return true;}
        }
        false
    }

    /// Candidate moves according to standard 3x4 Dobutsu rules.
    /// Game-specific legality (try, illegal lion moves, repetition)
    /// must be verified before full retrograde generation.
    pub fn moves(&self)->Vec<Move> {
        let Some(turn)=self.side_to_move else{return Vec::new()};
        if self.terminal_for_tablebase().is_some(){return Vec::new();}
        let mut out=Vec::new();
        for from in 0..CELLS {
            let piece=self.cells[from];
            if piece==0 || (piece>0)!=(turn==PLAYER){continue;}
            let kind=piece.unsigned_abs();
            let row=(from/3) as i32;
            let col=(from%3) as i32;
            let forward=if turn==PLAYER {-1} else {1};
            let dirs: &[(i32,i32)]=match kind {
                LION=>&[(-1,-1),(-1,0),(-1,1),(0,-1),(0,1),(1,-1),(1,0),(1,1)],
                GIRAFFE=>&[(-1,0),(1,0),(0,-1),(0,1)],
                ELEPHANT=>&[(-1,-1),(-1,1),(1,-1),(1,1)],
                CHICK=>&[(1,0)], // transformed to forward using the special case below
                HEN=>&[(-1,-1),(-1,0),(-1,1),(0,-1),(0,1),(1,0)],
                _=>continue
            };
            for &(dr,dc) in dirs {
                let oriented_dr=if kind==CHICK {forward}
                    else if kind==HEN && turn==OPPONENT {-dr} else {dr};
                let r=row+oriented_dr;
                let c=col+dc;
                if !(0..4).contains(&r) || !(0..3).contains(&c){continue;}
                let target=(r*3+c) as usize;
                if self.cells[target]!=0 &&
                   (self.cells[target]>0)==(turn==PLAYER) {continue;}
                out.push(Move{from:Some(from as u8),to:target as u8,drop_kind:None});
            }
        }
        for (idx,&n) in self.hands[turn].iter().enumerate() {
            if n==0{continue;}
            let kind=match idx {0=>GIRAFFE,1=>ELEPHANT,2=>CHICK,_=>unreachable!()};
            for to in 0..CELLS {
                if self.cells[to]!=EMPTY{continue;}
                // Unlike orthodox Shogi, a chick can be dropped on the
                // opponent's last rank. It stays a chick and cannot move
                // farther until captured; the drop itself never promotes.
                out.push(Move{from:None,to:to as u8,drop_kind:Some(kind)});
            }
        }
        out
    }

    /// Only accept moves that the rules generator actually emitted.
    pub fn apply(&self,m:Move)->Option<Self> {
        if !self.moves().contains(&m) { return None; }
        self.apply_unchecked(m)
    }

    /// Internal transition for a move already checked by moves().
    pub(crate) fn apply_unchecked(&self,m:Move)->Option<Self> {
        let turn=self.side_to_move?;
        let mut next=self.clone();
        let to=m.to as usize;
        if to>=CELLS{return None;}
        let piece=if let Some(from)=m.from {
            let from=from as usize;
            if from>=CELLS{return None;}
            let p=next.cells[from];
            if p==0 || (p>0)!=(turn==PLAYER){return None;}
            next.cells[from]=0;
            p
        } else {
            let kind=m.drop_kind?;
            let idx=match kind {GIRAFFE=>0,ELEPHANT=>1,CHICK=>2,_=>return None};
            if next.hands[turn][idx]==0 || next.cells[to]!=0{return None;}
            next.hands[turn][idx]-=1;
            if turn==PLAYER {kind as i8}else{-(kind as i8)}
        };
        let captured=next.cells[to];
        if captured!=0 {
            if (captured>0)==(turn==PLAYER){return None;}
            let taken=captured.unsigned_abs();
            if taken!=LION {
                let index=match taken{GIRAFFE=>0,ELEPHANT=>1,CHICK|HEN=>2,_=>return None};
                next.hands[turn][index]=next.hands[turn][index].saturating_add(1);
            }
        }
        let mut kind=piece.unsigned_abs();
        // Only a moved chick promotes. A DROP on the last rank stays a
        // chick (and can be immobile); standard Dobutsu allows this.
        if m.from.is_some() && kind==CHICK &&
           (if turn==PLAYER {to<3} else{to>=9}) {
            kind=HEN;
        }
        next.cells[to]=if turn==PLAYER {kind as i8}else{-(kind as i8)};
        next.side_to_move=Some(1-turn);
        Some(next)
    }

    /// Enumerate the full set of legal immediate predecessors for retrograde
    /// analysis. Each generated parent is checked by replaying the move.
    /// No tablebase or third-party engine is used.
    ///
    /// Captures are reconstructed from the mover's current hand; for hens,
    /// the captured piece is restored on the board as a hen but removed
    /// from the hand as a chick. Captured lions are not added to a hand.
    pub fn predecessors(&self)->Vec<Self> {
        let Some(side)=self.side_to_move else{return Vec::new()};
        let mover=1-side;
        let sign:i8=if mover==PLAYER {1}else{-1};
        let enemy_sign:i8=-sign;
        let mut result=Vec::new();
        let enemy_lion_present=self.cells.iter().any(|&p|p==enemy_sign);

        for dest in 0..CELLS {
            let current=self.cells[dest];
            if current==EMPTY || current.signum()!=sign {continue;}
            let kind=current.unsigned_abs();

            // Undo a drop of a single unpromoted piece.
            if matches!(kind,GIRAFFE|ELEPHANT|CHICK) {
                let hand_index=match kind {
                    GIRAFFE=>0,ELEPHANT=>1,CHICK=>2,_=>unreachable!()
                };
                if self.hands[mover][hand_index]<2 {
                    let mut parent=self.clone();
                    parent.cells[dest]=EMPTY;
                    parent.hands[mover][hand_index]+=1;
                    parent.side_to_move=Some(mover);
                    let m=Move{from:None,to:dest as u8,drop_kind:Some(kind)};
                    if parent.terminal_for_tablebase().is_none() &&
                       parent.apply_unchecked(m).as_ref()==Some(self) {
                        result.push(parent);
                    }
                }
            }

            // Undo movement from an empty source square. An on-board hen
            // may have originated as a chick that promoted on this move.
            let possible_kinds=if kind==HEN && (if mover==PLAYER {
                dest<3
            } else {dest>=9}) {
                [HEN,CHICK]
            } else {
                [kind,0]
            };
            for src in 0..CELLS {
                if self.cells[src]!=EMPTY {continue;}
                for source_kind in possible_kinds {
                    if source_kind==0 {continue;}
                    // An inverse movement must obey the exact piece
                    // movement geometry (replaying apply alone is insufficient).
                    let dr=dest as i32/3-src as i32/3;
                    let dc=dest as i32%3-src as i32%3;
                    let fwd=if mover==PLAYER {-1}else{1};
                    let can_step=match source_kind {
                        LION=>dr.abs()<=1 && dc.abs()<=1 && (dr!=0||dc!=0),
                        GIRAFFE=>dr.abs()+dc.abs()==1,
                        ELEPHANT=>dr.abs()==1 && dc.abs()==1,
                        CHICK=>dr==fwd && dc==0,
                        HEN=>(dr==fwd && dc.abs()<=1) ||
                              (dr==0 && dc.abs()==1) ||
                              (dr==-fwd && dc==0),
                        _=>false
                    };
                    if !can_step{continue;}

                    // Nothing captured.
                    let mut parent=self.clone();
                    parent.cells[src]=sign*(source_kind as i8);
                    parent.cells[dest]=EMPTY;
                    parent.side_to_move=Some(mover);
                    let m=Move{from:Some(src as u8),to:dest as u8,drop_kind:None};
                    if parent.terminal_for_tablebase().is_none() &&
                       parent.apply_unchecked(m).as_ref()==Some(self) {
                        result.push(parent);
                    }

                    // Capturing one of giraffe/elephant/chick/hen.
                    // A captured hen contributes a CHICK to the hand.
                    for taken in [GIRAFFE,ELEPHANT,CHICK,HEN] {
                        let hi=match taken {
                            GIRAFFE=>0,ELEPHANT=>1,CHICK|HEN=>2,
                            _=>unreachable!()
                        };
                        if self.hands[mover][hi]==0 {continue;}
                        let mut parent=self.clone();
                        parent.cells[src]=sign*(source_kind as i8);
                        parent.cells[dest]=enemy_sign*(taken as i8);
                        parent.hands[mover][hi]-=1;
                        parent.side_to_move=Some(mover);
                        if parent.terminal_for_tablebase().is_none() &&
                       parent.apply_unchecked(m).as_ref()==Some(self) {
                            result.push(parent);
                        }
                    }

                    // The captured lion is not put into the hand.
                    if !enemy_lion_present {
                        let mut parent=self.clone();
                        parent.cells[src]=sign*(source_kind as i8);
                        parent.cells[dest]=enemy_sign*(LION as i8);
                        parent.side_to_move=Some(mover);
                        if parent.terminal_for_tablebase().is_none() &&
                       parent.apply_unchecked(m).as_ref()==Some(self) {
                            result.push(parent);
                        }
                    }
                }
            }
        }
        result
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct Move{
    pub from:Option<u8>,
    pub to:u8,
    pub drop_kind:Option<u8>,
}

/// Probe an externally generated, independently verified, COMPLETE
/// DOBUTB03 table. This never uses the heuristic fallback and returns INVALID
/// on unknown side-to-move, file omission, or missing/corrupt position.
#[no_mangle]
pub extern "C" fn dobutsu_evaluate_table_v3(
    data:*const u8,data_len:usize,
    board:*const c_int,hands:*const c_int,hand_len:usize,
    side_to_move:c_int
)->c_int{
    if data.is_null() || data_len<32 || data_len>(1usize<<35) ||
       board.is_null() || hand_len>64 ||
       (hand_len>0 && hands.is_null()) {return INVALID;}
    let side=match side_to_move {0=>PLAYER,1=>OPPONENT,_=>return INVALID};
    // SAFETY: C++ only supplies immutable mmap'd files and validated,
    // readable game arrays. No ownership transfer and no thread mutation.
    let (table,b,h)=unsafe{
        (core::slice::from_raw_parts(data,data_len),
         core::slice::from_raw_parts(board,CELLS),
         if hand_len==0{&[][..]}else{core::slice::from_raw_parts(hands,hand_len)})
    };
    let pos=match Position::from_game(b,h,Some(side)){
        Ok(p)=>p,Err(_)=>return INVALID,
    };
    let hit=match compact::probe_v3(table,&pos){
        Some(v)=>v,None=>return INVALID,
    };
    tablebase::display_score(hit,side,PLAYER).unwrap_or(INVALID)
}

/// Exact dense-cohort read-only probe. Two lossless formats:
/// - DOBDENS1 | u64(255280704) | 255280704 signed-byte slots (Rust solver)
/// - DOBREF01 | u64(167527962) | 167527962 signed-byte slots (BSD C solver)
/// The latter is the same indexed table, omitting unpopulated trailing slots.
/// Slot identity is computed directly without enumerating the reachable graph.
/// Negative e=-n => side-to-move loss in 2*n plies.
/// Positive e=n => side-to-move win in 2*n-1 plies. 0 => draw.
/// Terminal positions that are not encoded are evaluated by exact rules.
#[no_mangle]
pub extern "C" fn dobutsu_evaluate_dense(
    data:*const u8,data_len:usize,
    board:*const c_int,hands:*const c_int,hand_len:usize,
    side_to_move:c_int
)->c_int {
    const HEADER:usize=16;
    const DENSE:usize=dense_lib::cohort::POSITION_TOTAL_COUNT as usize;
    const REF:usize=dense_lib::cohort::POSITION_COUNT as usize;
    if data.is_null() || data_len<HEADER ||
       board.is_null() || hand_len>64 || (hand_len>0 && hands.is_null()){
        return INVALID;
    }
    let side=match side_to_move {
        0=>PLAYER,1=>OPPONENT,_=>return INVALID,
    };
    let (bytes,b,h)=unsafe {
        (core::slice::from_raw_parts(data,data_len),
         core::slice::from_raw_parts(board,CELLS),
         if hand_len==0{&[][..]}else{core::slice::from_raw_parts(hands,hand_len)})
    };
    let slots=if bytes.get(..8)==Some(&b"DOBDENS1"[..]){DENSE}
        else if bytes.get(..8)==Some(&b"DOBREF01"[..]){REF}
        else{return INVALID};
    if data_len!=HEADER+slots ||
       u64::from_le_bytes(bytes[8..16].try_into().unwrap())!=slots as u64{
        return INVALID;
    }
    let pos=match Position::from_game(b,h,Some(side)){
        Ok(p)=>p,Err(_)=>return INVALID,
    };
    let result=if let Some(terminal)=pos.terminal_for_tablebase(){
        tablebase::TableHit{
            result:terminal,
            dtm:if terminal==retrograde::Outcome::Win{1}else{0},
        }
    }else{
        let Some(dense)=as_dense_position(&pos) else{return INVALID};
        let Some(slot)=dense_lib::cohort::encode_checked(&dense)
            else{return INVALID};
        let v=if slots==REF{
            // The native C table permutes ownership blocks and omits the
            // final 22 ownership classes, which require exact reconstruction.
            let Some(value)=reference_probe_value(bytes,&dense) else{
                return INVALID;
            };
            value
        }else if slot<slots as u64{
            bytes[HEADER+slot as usize] as i8
        }else{
            return INVALID;
        };
        if v==i8::MIN{return INVALID;}
        tablebase::TableHit{
            result:if v>0{retrograde::Outcome::Win}
                else if v<0{retrograde::Outcome::Loss}
                else{retrograde::Outcome::Draw},
            dtm:if v>0{(2i32*v as i32-1) as u16}
                else if v<0{(-2i32*v as i32) as u16}
                else{0},
        }
    };
    tablebase::display_score(result,side,PLAYER).unwrap_or(INVALID)
}

/// Probe the C reference table at one legal nonterminal position, including
/// all 22 ownership classes not stored in the native file. Returning None
/// means the position or header cannot be represented, not an evaluation.
pub fn reference_probe_value(table:&[u8],p:&dense_lib::Position)->Option<i8>{
    let stored=dense_lib::cohort::POSITION_COUNT as usize;
    if table.len()!=16+stored || table.get(..8)?!=b"DOBREF01" ||
       u64::from_le_bytes(table.get(8..16)?.try_into().ok()?)!=stored as u64{
        return None;
    }
    let raw=dense_lib::cohort::encode_checked(p)?;
    if let Some(offset)=dense_lib::cohort::reference_stored_offset(raw){
        let e=table[16+offset as usize] as i8;
        (e!=i8::MIN).then_some(e)
    }else{
        probe_reference_unstored(table,p)
    }
}

/// Equivalent to clausecker/dobutsu tablebase.h wdl_compare(e,worst)<0.
fn reference_child_worse(e:i16,worst:i16)->bool{
    if worst<0{e<0 && e>worst}
    else if worst==0{e<0}
    else{e<=0 || e>worst}
}

/// The reference C table stores 42 of the 64 ownership classes. Recover the
/// other 22 classes exactly by one minimax ply, matching tbaccess.c.
/// Every child of these classes must have a table entry (fail closed otherwise).
fn probe_reference_unstored(bytes:&[u8],p:&dense_lib::Position)
    ->Option<i8>{
    const STORED:u64=dense_lib::cohort::POSITION_COUNT;
    if bytes.get(..8)?!=b"DOBREF01" ||
       bytes.len()!=16+STORED as usize{return None;}
    let mut worst=1i16;
    for mv in p.moves(){
        let child=p.make(&mv);
        // Match reference C's sente_in_check/gote_in_check predicate:
        // not only capture of our lion but also an immediate safe Try
        // by the opponent makes this move a losing continuation.
        if child.is_immediate_win(){continue;}
        let unfolded=dense_lib::cohort::encode_checked(&child)?;
        let off=dense_lib::cohort::reference_stored_offset(unfolded)?;
        let e=bytes[16+off as usize] as i8 as i16;
        if e==i8::MIN as i16{return None;}
        if reference_child_worse(e,worst){worst=e;}
    }
    i8::try_from(if worst<0{1-worst}else{-worst}).ok()
}

fn as_dense_position(pos:&Position)->Option<dense_lib::Position>{
    let turn=match pos.side_to_move?{
        PLAYER=>dense_lib::Owner::Sente,
        OPPONENT=>dense_lib::Owner::Gote,
        _=>return None,
    };
    let mut board=[None;CELLS];
    for (i,&raw) in pos.cells.iter().enumerate(){
        if raw==EMPTY{continue;}
        let piece=match raw.unsigned_abs(){
            LION=>dense_lib::Piece::Lion,
            GIRAFFE=>dense_lib::Piece::Giraffe,
            ELEPHANT=>dense_lib::Piece::Elephant,
            CHICK=>dense_lib::Piece::Chick,
            HEN=>dense_lib::Piece::Hen,
            _=>return None,
        };
        let owner=if raw>0{dense_lib::Owner::Sente}
            else{dense_lib::Owner::Gote};
        board[i]=Some((piece,owner));
    }
    Some(dense_lib::Position{
        board,
        hand_sente:pos.hands[PLAYER],
        hand_gote:pos.hands[OPPONENT],
        turn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const INITIAL:[i32;12]=[
        0x23,0x21,0x25,-1,0x27,-1,-1,0x06,-1,0x04,0x00,0x02
    ];
    #[test] fn initial_position_is_valid() {
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        assert!(p.moves().len()>0);
        assert!(p.key().is_some());
    }
    #[test] fn pre_game_all_zero_is_not_a_board(){
        assert!(Position::from_game(&[0;12],&[],None).is_err());
    }
    #[test] fn zero_is_a_lion_and_minus_one_is_empty(){
        assert_eq!(decode_piece(0).unwrap(),Some((PLAYER,LION)));
        assert_eq!(decode_piece(-1).unwrap(),None);
    }
    #[test] fn redraw_bit_does_not_change_identity(){
        assert_eq!(decode_piece(0x22).unwrap(),decode_piece(0x23).unwrap());
        assert_eq!(decode_piece(0x04).unwrap(),decode_piece(0x05).unwrap());
        assert_eq!(decode_piece(0x06).unwrap(),decode_piece(0x07).unwrap());
    }
    #[test] fn all_piece_types_are_accounted_for_after_drop(){
        let mut initial=INITIAL;
        initial[4]=-1;
        let p=Position::from_game(&initial,&[0x27],Some(PLAYER)).unwrap();
        assert_eq!(p.hands[OPPONENT][2],1);
    }
    #[test] fn terminal_lion_capture_is_decisive(){
        let mut cells=INITIAL;
        cells[1]=-1;
        let p=Position::from_game(&cells,&[],Some(PLAYER)).unwrap();
        assert_eq!(p.terminal(),Some(retrograde::Outcome::Win));
        let q=Position::from_game(&cells,&[],Some(OPPONENT)).unwrap();
        assert_eq!(q.terminal(),Some(retrograde::Outcome::Loss));
    }
    #[test] fn rejects_illegal_non_adjacent_move(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        assert_eq!(p.apply(Move{from:Some(10),to:0,drop_kind:None}),None);
    }
    #[test] fn try_requires_lion_not_attackable(){
        let mut p=Position::from_game(&INITIAL,&[],Some(OPPONENT)).unwrap();
        p.cells[10]=EMPTY;
        p.cells[2]=LION as i8; // top enemy back rank
        p.cells[3]=EMPTY; // no relevant attacker here
        // Opponent elephant at square 2 has been replaced by the lion.
        // The adjacent opponent lion still attacks square 2; not a Try.
        assert_eq!(p.terminal(),None);
        // Move the opponent lion to row 2 (not its own Try rank).
        // The player lion on top row is no longer directly attackable.
        p.cells[1]=EMPTY;
        p.cells[7]=-(LION as i8);
        assert_eq!(p.terminal(),Some(retrograde::Outcome::Loss));
    }
    #[test] fn impossible_pre_match_piece_counts_rejected(){
        let mut cells=INITIAL;
        cells[0]=0; // a second player lion, without an opponent giraffe
        assert!(Position::from_game(&cells,&[],None).is_err());
    }
    #[test] fn last_rank_chick_drop_is_legal_and_does_not_promote(){
        let mut cells=INITIAL;
        cells[4]=-1; // opponent's chick is in its hand
        cells[11]=-1; // player's giraffe is in player's hand
        let p=Position::from_game(&cells,&[0x27,0x02],Some(OPPONENT)).unwrap();
        let m=Move{from:None,to:11,drop_kind:Some(CHICK)};
        assert!(p.moves().contains(&m));
        let next=p.apply(m).unwrap();
        assert_eq!(next.cells[11],-(CHICK as i8));
        assert_eq!(next.hands[OPPONENT][2],0);
    }
    #[test] fn capturing_a_hen_returns_a_chick_to_hand(){
        let mut cells=INITIAL;
        cells[7]=0x08; // 0x08 is the actual on-board hen encoding
        let p=Position::from_game(&cells,&[],Some(OPPONENT)).unwrap();
        let m=Move{from:Some(4),to:7,drop_kind:None};
        assert!(p.moves().contains(&m));
        let next=p.apply(m).unwrap();
        assert_eq!(next.hands[OPPONENT][2],1);
        assert_eq!(next.cells[7],-(CHICK as i8));
    }
    #[test] fn compact_key_roundtrip_all_piece_encodings(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let k=p.key_u64().unwrap();
        assert_eq!(Position::from_key_u64(k),Some(p.clone()));
        let m=p.moves()[0];
        let successor=p.apply(m).unwrap();
        assert_eq!(Position::from_key_u64(successor.key_u64().unwrap()),
                   Some(successor));
    }
    #[test] fn reverse_generator_contains_actual_forward_edges(){
        let root=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let mut frontier=vec![root];
        for _ in 0..3 {
            let mut next=Vec::new();
            for p in frontier {
                for mv in p.moves(){
                    let child=p.apply(mv).unwrap();
                    assert!(child.predecessors().contains(&p),
                        "Missing reverse for move {:?} leading to {:?}",mv,child);
                    next.push(child);
                }
            }
            frontier=next;
        }
    }
    #[test] fn reverse_generator_survives_captures_drops_and_promotions(){
        // Deterministic pseudo-random traversals to stress transitions
        // which are not encountered within the first three plies.
        let initial=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let mut covered=0usize;
        let mut state=0x9e37_79b9u64;
        for iteration in 0..160 {
            let mut p=initial.clone();
            for _ in 0..35 {
                let next=p.moves();
                if next.is_empty(){break;}
                state^=state<<13;state^=state>>7;state^=state<<17;
                let mv=next[(state as usize)%next.len()];
                let child=p.apply_unchecked(mv).unwrap();
                assert!(child.predecessors().contains(&p),
                    "Reverse edge missing after iteration {}, move {:?}",iteration,mv);
                covered+=1;
                p=child;
            }
        }
        assert!(covered>=500,"Too few tested reachable transitions: {covered}");
    }
    #[test] fn canonical_inverse_edges_cover_shallow_orbits(){
        let root=Position::from_game(&INITIAL,&[],Some(PLAYER))
            .unwrap().canonicalized().unwrap();
        let mut frontier=vec![root];
        let mut seen=std::collections::HashSet::new();
        for _ in 0..3{
            let mut next=Vec::new();
            for parent in frontier{
                let parent_key=parent.canonical_key_u64();
                for mv in parent.moves(){
                    let child=parent.apply_unchecked(mv).unwrap()
                        .canonicalized().unwrap();
                    assert!(child.predecessors().iter().any(|q|
                        q.canonical_key_u64()==parent_key),
                        "Missing canonical predecessor of {child:?}");
                    if seen.insert(child.canonical_key_u64().unwrap()){
                        next.push(child);
                    }
                }
            }
            frontier=next;
        }
    }
    #[test] fn reference_ownership_index_is_permutation_of_dense_blocks(){
        use dense_lib::cohort::{
            reference_stored_offset, POSITION_TOTAL_COUNT, POSITION_COUNT, OWNERSHIP_MAP,
        };
        const BLOCK:u64=POSITION_TOTAL_COUNT/64;
        let mut seen=[false;64];
        let mut stored=0;
        for owner in 0..64usize {
            let permuted=OWNERSHIP_MAP[owner] as usize;
            assert!(!seen[permuted],"duplicate reference ownership block");
            seen[permuted]=true;
            let first=owner as u64*BLOCK;
            let expected=permuted as u64*BLOCK;
            assert_eq!(reference_stored_offset(first), 
                (expected<POSITION_COUNT).then_some(expected));
            assert_eq!(reference_stored_offset(first+BLOCK-1),
                (expected+BLOCK-1<POSITION_COUNT).then_some(expected+BLOCK-1));
            if permuted<42{stored+=1;}
        }
        assert_eq!(stored,42);
        assert!(seen.into_iter().all(|s|s));
        assert_eq!(reference_stored_offset(POSITION_TOTAL_COUNT),None);
        assert_ne!(OWNERSHIP_MAP[16],16,"mapping must not be identity");
    }
    #[test] fn reference_one_ply_minimax_preserves_optimal_dtm(){
        assert!(reference_child_worse(-1,1));
        assert!(reference_child_worse(-1,-2));
        assert!(!reference_child_worse(-2,-1));
        assert!(reference_child_worse(5,1));
        assert!(!reference_child_worse(1,5));
        assert!(reference_child_worse(0,1));
        assert!(!reference_child_worse(1,0));
    }
    #[test] fn dense_index_initial_matches_original_board(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let dense=as_dense_position(&p).unwrap();
        let ref_p=dense_lib::parse("S/gle/-c-/-C-/ELG/-").unwrap();
        assert_eq!(dense,ref_p);
        let ix=dense_lib::cohort::encode_checked(&dense).unwrap();
        assert!(ix<dense_lib::cohort::POSITION_TOTAL_COUNT);
        let recovered=dense_lib::cohort::decode_checked(ix).unwrap();
        assert_eq!(dense_lib::cohort::encode_checked(&recovered),Some(ix));
    }
    #[test] fn canonical_key_folds_turn_and_mirror(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let key=p.canonical_key_u64().unwrap();
        let mut flipped=p.clone();
        for i in 0..CELLS{flipped.cells[CELLS-1-i]=-p.cells[i];}
        flipped.hands.swap(PLAYER,OPPONENT);
        flipped.side_to_move=Some(OPPONENT);
        assert_eq!(flipped.canonical_key_u64(),Some(key));
        let mut mirrored=p.clone();
        for r in 0..4{
            for c in 0..3{
                mirrored.cells[r*3+c]=p.cells[r*3+2-c];
            }
        }
        assert_eq!(mirrored.canonical_key_u64(),Some(key));
        assert_eq!(p.canonicalized().unwrap().side_to_move,Some(PLAYER));
    }
    #[test] fn tanaka_capture_is_terminal_before_capture(){
        let mut p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        p.cells[4]=GIRAFFE as i8;
        assert_eq!(p.terminal_for_tablebase(),Some(retrograde::Outcome::Win));
        assert!(p.moves().is_empty());
    }
    #[test] fn tanaka_try_resolves_next_ply(){
        let mut p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        p.cells[1]=EMPTY;
        p.cells[10]=EMPTY;
        p.cells[6]=LION as i8;
        p.cells[11]=-(LION as i8);
        assert_eq!(p.terminal_for_tablebase(),Some(retrograde::Outcome::Loss));
        assert!(p.moves().is_empty());
    }
    #[test] fn key_includes_side_to_move(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let mut q=p.clone();
        q.side_to_move=Some(OPPONENT);
        assert_ne!(p.key(),q.key());
    }
    #[test] fn move_is_reversible_in_inventory(){
        let p=Position::from_game(&INITIAL,&[],Some(PLAYER)).unwrap();
        let m=p.moves().into_iter().find(|m|m.from==Some(7)&&m.to==4).unwrap();
        let q=p.apply(m).unwrap();
        // Human chick takes opponent chick: two chicks still in play.
        assert_eq!(q.hands[PLAYER][2],1);
    }
}
