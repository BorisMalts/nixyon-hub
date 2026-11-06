use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::artifacts::{ReadError, open_regular};

pub struct Tail {
    pub text: String,
    pub cut: bool,
}

// Из длинного журнала нужен конец: там ошибка, из-за которой шаг упал.
pub fn read_tail(path: &Path, max: u64) -> Result<Tail, ReadError> {
    let (file, len) = open_regular(path, u64::MAX)?;
    Ok(tail_of(file, len, max)?)
}

pub fn tail_of(mut file: std::fs::File, len: u64, max: u64) -> std::io::Result<Tail> {
    let cut = len > max;
    if cut {
        file.seek(SeekFrom::Start(len - max))?;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len.min(max)).unwrap_or(0));
    file.take(max).read_to_end(&mut bytes)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut && let Some(newline) = text.find('\n') {
        text.drain(..=newline);
    }
    Ok(Tail { text, cut })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_file_gives_its_end_from_a_whole_line() {
        let path = std::env::temp_dir().join(format!("nixyon-tail-{}", std::process::id()));
        std::fs::write(&path, "первая\nвторая\nтретья\n").unwrap();
        let whole = read_tail(&path, 1000).unwrap();
        assert!(!whole.cut);
        assert_eq!(whole.text, "первая\nвторая\nтретья\n");
        let tail = read_tail(&path, 20).unwrap();
        assert!(tail.cut);
        assert_eq!(tail.text, "третья\n");
    }
}
