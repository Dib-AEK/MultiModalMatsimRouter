//! Reads and writes a versioned, checksummed compact timetable cache, validating its structure on load.
use crate::model::Timetable;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"MMRTR003";
pub fn write(path: &Path, timetable: &Timetable) -> Result<()> {
    timetable.validate()?;
    let payload = postcard::to_stdvec(timetable)?;
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    let mut file = std::io::BufWriter::new(temporary.as_file_mut());
    file.write_all(MAGIC)?;
    file.write_all(&(payload.len() as u64).to_le_bytes())?;
    file.write_all(blake3::hash(&payload).as_bytes())?;
    file.write_all(&payload)?;
    file.flush()?;
    drop(file);
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}
pub fn read(path: &Path, max_bytes: u64) -> Result<Timetable> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    ensure!(
        length >= 48 && length <= max_bytes,
        "invalid/oversized timetable cache"
    );
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unknown timetable cache version");
    let mut size = [0; 8];
    file.read_exact(&mut size)?;
    let declared = u64::from_le_bytes(size);
    ensure!(
        declared == length - 48,
        "truncated/trailing timetable cache payload"
    );
    let mut checksum = [0; 32];
    file.read_exact(&mut checksum)?;
    let mut payload = Vec::with_capacity(declared.try_into()?);
    file.read_to_end(&mut payload)?;
    ensure!(
        blake3::hash(&payload).as_bytes() == &checksum,
        "timetable cache checksum mismatch"
    );
    let timetable: Timetable =
        postcard::from_bytes(&payload).context("decoding timetable cache")?;
    timetable.validate()?;
    Ok(timetable)
}
