//! Bounded processing copies, never edits to source audio. PCM WAV chunks preserve original sample coordinates.
use candid_store::uuid_v4;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
pub struct AudioChunks {
    pub path: PathBuf,
    pub fmt: Vec<u8>,
    pub offset: u64,
    pub frames: u64,
    pub rate: u32,
    pub align: u16,
    temp: Option<PathBuf>,
}
impl Drop for AudioChunks {
    fn drop(&mut self) {
        if let Some(dir) = &self.temp {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
fn inspect(path: &Path) -> Result<AudioChunks, String> {
    let mut f = File::open(path).map_err(|_| "missing_source")?;
    let size = f.metadata().map_err(|_| "missing_source")?.len();
    let mut h = [0; 12];
    f.read_exact(&mut h).map_err(|_| "invalid_audio")?;
    if &h[..4] != b"RIFF" || &h[8..] != b"WAVE" {
        return Err("not_pcm_wav".into());
    }
    let mut offset = 12;
    let mut fmt = Vec::new();
    let mut data = None;
    while offset + 8 <= size {
        f.seek(SeekFrom::Start(offset)).map_err(|_| "audio_read")?;
        let mut h = [0; 8];
        f.read_exact(&mut h).map_err(|_| "audio_read")?;
        let n = u32::from_le_bytes(h[4..].try_into().unwrap()) as u64;
        if offset + 8 + n > size {
            return Err("invalid_audio".into());
        }
        if &h[..4] == b"fmt " {
            if n > 65536 {
                return Err("invalid_audio".into());
            }
            fmt.resize(n as usize, 0);
            f.read_exact(&mut fmt).map_err(|_| "audio_read")?;
        }
        if &h[..4] == b"data" {
            data = Some((offset + 8, n));
            break;
        }
        offset += 8 + n + n % 2;
    }
    if fmt.len() < 16 {
        return Err("invalid_audio".into());
    }
    let tag = u16::from_le_bytes(fmt[..2].try_into().unwrap());
    if ![1, 3, 65534].contains(&tag) {
        return Err("not_pcm_wav".into());
    }
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
    let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap());
    let (offset, bytes) = data.ok_or("invalid_audio")?;
    if rate == 0 || align == 0 || bytes % align as u64 != 0 {
        return Err("invalid_audio".into());
    }
    Ok(AudioChunks {
        path: path.into(),
        fmt,
        offset,
        frames: bytes / align as u64,
        rate,
        align,
        temp: None,
    })
}
impl AudioChunks {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Ok(plan) = inspect(path) {
            return Ok(plan);
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::unix::fs::DirBuilderExt;
            let dir = std::env::temp_dir().join(format!("candid-processing-{}", uuid_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|_| "processing_copy_failed")?;
            let output = dir.join("processing.wav");
            let result = std::process::Command::new("/usr/bin/afconvert")
                .args(["-f", "WAVE", "-d", "LEI16@16000", "-c", "1"])
                .arg(path)
                .arg(&output)
                .output();
            if !result.is_ok_and(|r| r.status.success()) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err("audio_conversion_failed".into());
            }
            match inspect(&output) {
                Ok(mut plan) => {
                    plan.temp = Some(dir);
                    Ok(plan)
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    Err(e)
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("audio_conversion_unavailable".into())
        }
    }
    pub fn for_voice(path: &Path) -> Result<Self,String> {
        let plan=Self::open(path)?;
        if plan.rate==16000 && plan.align==2 && plan.fmt.get(..4)==Some(&[1,0,1,0]) {return Ok(plan);}
        #[cfg(target_os="macos")]
        {
            use std::os::unix::fs::DirBuilderExt;
            let dir=std::env::temp_dir().join(format!("candid-speaker-samples-{}",uuid_v4()));
            std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|_|"processing_copy_failed")?;
            let output=dir.join("mono.wav");
            let status=std::process::Command::new("/usr/bin/afconvert").args(["-f","WAVE","-d","LEI16@16000","-c","1"]).arg(path).arg(&output).status();
            if !status.is_ok_and(|s|s.success()) {let _=std::fs::remove_dir_all(&dir);return Err("audio_conversion_failed".into());}
            match inspect(&output) {Ok(mut plan)=>{plan.temp=Some(dir);Ok(plan)},Err(e)=>{let _=std::fs::remove_dir_all(&dir);Err(e)}}
        }
        #[cfg(not(target_os="macos"))]
        {Err("audio_conversion_unavailable".into())}
    }
    pub fn chunk_frames(&self) -> u64 {
        (self.rate as u64 * 240)
            .min(32 * 1024 * 1024 / self.align as u64)
            .max(1)
    }
    pub fn bytes(&self, start: u64, count: u64) -> Result<Vec<u8>, String> {
        if start + count > self.frames {
            return Err("chunk_bounds".into());
        }
        let size = count * self.align as u64;
        let mut b = Vec::with_capacity(size as usize + self.fmt.len() + 32);
        b.extend(b"RIFF");
        b.extend((20 + self.fmt.len() as u32 + size as u32).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend((self.fmt.len() as u32).to_le_bytes());
        b.extend(&self.fmt);
        b.extend(b"data");
        b.extend((size as u32).to_le_bytes());
        let n = b.len();
        b.resize(n + size as usize, 0);
        let mut f = File::open(&self.path).map_err(|_| "missing_source")?;
        f.seek(SeekFrom::Start(self.offset + start * self.align as u64))
            .map_err(|_| "audio_read")?;
        f.read_exact(&mut b[n..]).map_err(|_| "audio_read")?;
        Ok(b)
    }
}

#[cfg(all(test,target_os="macos"))]
mod tests {
    use super::*;
    #[test]
    fn native_processing_copy_decodes_public_m4a_without_touching_original() {
        let directory=std::env::temp_dir().join(format!("pw-convert-{}",uuid_v4()));std::fs::create_dir_all(&directory).unwrap();
        let public=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../data/asr/sample-zh.wav");
        if !public.exists(){eprintln!("SKIP: public sample data/asr/sample-zh.wav not downloaded");return;}let source=directory.join("PUBLIC.m4a");
        let result=std::process::Command::new("/usr/bin/afconvert").args(["-f","m4af","-d","aac@44100","-b","64000"]).arg(public).arg(&source).output().unwrap();assert!(result.status.success(),"public fixture conversion failed");
        let digest=candid_store::sha256_hex_file(&source).unwrap();
        {let plan=AudioChunks::open(&source).unwrap();assert_eq!(plan.rate,16000);assert!((4.0..4.5).contains(&(plan.frames as f64/plan.rate as f64)));assert!(plan.temp.is_some());let bytes=plan.bytes(0,plan.frames).unwrap();assert!(bytes.starts_with(b"RIFF"));}
        assert_eq!(digest,candid_store::sha256_hex_file(&source).unwrap());std::fs::remove_dir_all(directory).unwrap();
    }
}
