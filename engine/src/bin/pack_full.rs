//! Build lossless, memory-mappable DOBUTB03 from a *verified* DOBUTB02.
//! The conversion is checked record-for-record; nothing is quantized or
//! approximated. The original is preserved until verification completes.
use dobutsu_solver::compact::{self,BlockIndex,BLOCK_SIZE};
use memmap2::MmapOptions;
use std::{env,fs::{self,File},io::{self,Read,Write,BufReader,BufWriter},
    path::{Path,PathBuf},process};

fn invalid(s:impl Into<String>)->io::Error{
    io::Error::new(io::ErrorKind::InvalidData,s.into())
}
fn read_record<R:Read>(r:&mut R)->io::Result<(u64,i16)>{
    let mut data=[0u8;10];
    r.read_exact(&mut data)?;
    Ok((u64::from_le_bytes(data[..8].try_into().unwrap()),
        i16::from_le_bytes(data[8..].try_into().unwrap())))
}
fn pack(source:&Path,dest:&Path)->io::Result<(u64,u64,u64)>{
    let input=File::open(source)?;
    let input_size=input.metadata()?.len();
    let mut reader=BufReader::new(input);
    let mut header=[0u8;16];
    reader.read_exact(&mut header)?;
    if &header[..8]!=b"DOBUTB02" {return Err(invalid("Not DOBUTB02")); }
    let count=u64::from_le_bytes(header[8..].try_into().unwrap());
    if count==0 || count.checked_mul(10).and_then(|v|v.checked_add(16))!=Some(input_size) {
        return Err(invalid("Malformed DOBUTB02 count or length"));
    }
    let blocks=(count as usize).div_ceil(BLOCK_SIZE);
    if blocks>u32::MAX as usize {
        return Err(invalid("Too many index blocks"));
    }
    let temp_payload=dest.with_extension("payload.incomplete");
    let temp_output=dest.with_extension("tbx.incomplete");
    let mut payload=BufWriter::new(File::create(&temp_payload)?);
    let mut index=Vec::<BlockIndex>::with_capacity(blocks);
    let mut prior=None::<u64>;
    let mut payload_length=0u64;
    let mut records=Vec::<(u64,i16)>::with_capacity(BLOCK_SIZE);
    let mut total=0u64;

    while total<count {
        records.clear();
        let take=((count-total) as usize).min(BLOCK_SIZE);
        for _ in 0..take {
            let row=read_record(&mut reader)?;
            if prior.is_some_and(|x|x>=row.0){
                return Err(invalid("Unsorted or duplicated source key"));
            }
            if compact::code_from_v2(row.1).is_none() {
                return Err(invalid("WDL/DTM code exceeds 173 plies or is invalid"));
            }
            prior=Some(row.0);
            records.push(row);
        }
        let payload_bytes=compact::encode_block(&records)
            .ok_or_else(||invalid("Invalid key delta or score"))?;
        index.push(BlockIndex{
            first_key:records[0].0,
            payload_offset:payload_length,
            payload_len:u32::try_from(payload_bytes.len())
                .map_err(|_|invalid("Block too large"))?,
            count:records.len() as u16,
        });
        payload.write_all(&payload_bytes)?;
        payload_length+=payload_bytes.len() as u64;
        total+=take as u64;
        if index.len()%100_000==0 {
            eprintln!("Packed {} of {count} records",total);
        }
    }
    payload.flush()?;
    drop(payload);
    let mut writer=BufWriter::new(File::create(&temp_output)?);
    writer.write_all(b"DOBUTB03")?;
    writer.write_all(&count.to_le_bytes())?;
    writer.write_all(&(BLOCK_SIZE as u32).to_le_bytes())?;
    writer.write_all(&(index.len() as u32).to_le_bytes())?;
    writer.write_all(&[0;8])?;
    for block in &index {
        let mut buf=Vec::with_capacity(24);
        block.write_into(&mut buf);
        writer.write_all(&buf)?;
    }
    let mut payload_reader=BufReader::new(File::open(&temp_payload)?);
    io::copy(&mut payload_reader,&mut writer)?;
    writer.flush()?;
    drop(writer);

    // Independent stream equality check: read every original (key,value)
    // and compare with each decoded 9-bit entry. Not spot sampling.
    let mut verify_source=BufReader::new(File::open(source)?);
    let mut skip_header=[0u8;16];
    verify_source.read_exact(&mut skip_header)?;
    let packed_file=File::open(&temp_output)?;
    let packed=unsafe{MmapOptions::new().map(&packed_file)?};
    let data_offset=32usize
        .checked_add(index.len().checked_mul(24)
            .ok_or_else(||invalid("Index overflow"))?)
        .ok_or_else(||invalid("Header overflow"))?;
    let mut verified=0u64;
    for block in &index {
        let start=data_offset
            .checked_add(block.payload_offset as usize)
            .ok_or_else(||invalid("Data offset overflow"))?;
        let end=start.checked_add(block.payload_len as usize)
            .ok_or_else(||invalid("Block length overflow"))?;
        let data=packed.get(start..end)
            .ok_or_else(||invalid("Truncated packed block"))?;
        let decoded=compact::decode_block(data,block.first_key,block.count as usize)
            .ok_or_else(||invalid("Packed block could not be decoded"))?;
        for pair in decoded {
            let original=read_record(&mut verify_source)?;
            if pair!=original {
                return Err(invalid(format!(
                    "Lossless validation FAILED at record {verified}: {pair:?} != {original:?}")));
            }
            verified+=1;
        }
    }
    if verified!=count || packed.len() as u64 !=
        (data_offset as u64).checked_add(payload_length)
            .ok_or_else(||invalid("Length overflow"))? {
        return Err(invalid("Record count or packed length mismatch"));
    }
    drop(packed);
    fs::rename(&temp_output,dest)?;
    fs::remove_file(temp_payload)?;
    let packed_size=fs::metadata(dest)?.len();
    println!("Lossless verified: {count} records; input {input_size} bytes, packed {packed_size} bytes");
    Ok((count,input_size,packed_size))
}
fn main(){
    let args:Vec<_>=env::args().collect();
    if args.len()!=3 {
        eprintln!("Usage: pack-full path/to/full.tb path/to/full.tbx");
        process::exit(2);
    }
    match pack(Path::new(&args[1]),Path::new(&args[2])){
        Ok(_)=>(),
        Err(e)=>{eprintln!("NO VALID PACKED TABLE WRITTEN: {e}");process::exit(3);}
    }
}
