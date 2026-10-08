#[cfg(test)]
use std::fs;
use std::path::Path;
// MoveFileW atomically refuses to replace an existing destination.
#[cfg(windows)]
pub(crate) fn publish(source: &Path, output: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileW(source: *const u16, target: *const u16) -> i32;
    }
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = output.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileW(source.as_ptr(), target.as_ptr()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn publish(source: &Path, output: &Path) -> std::io::Result<()> {
    // Use the existing tempfile implementation's exclusive native rename on
    // macOS/Linux (link-and-unlink fallback elsewhere). A retained hard link
    // would let a reused staging path modify the already published output.
    let mut staged = tempfile::TempPath::try_from_path(source)?;
    // The caller owns staging cleanup, including when publication fails.
    staged.disable_cleanup(true);
    staged
        .persist_noclobber(output)
        .map_err(|error| error.error)
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_publications_have_one_winner_and_never_replace_it() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().to_path_buf();
        let output = folder.join("final.mp4");
        let sources = [folder.join("first.mp4"), folder.join("second.mp4")];
        for (source, bytes) in sources.iter().zip([b"first", b"other"]) {
            fs::write(source, bytes).unwrap();
        }
        let barrier = Arc::new(Barrier::new(2));
        let handles = sources.map(|source| {
            let barrier = barrier.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                barrier.wait();
                publish(&source, &output)
            })
        });
        let outcomes = handles.map(|handle| handle.join().unwrap());
        assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
        let published = fs::read(&output).unwrap();
        assert!(published == b"first" || published == b"other");
        let third = folder.join("third.mp4");
        fs::write(&third, b"replacement").unwrap();
        assert!(publish(&third, &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), published);
        assert_eq!(fs::read(third).unwrap(), b"replacement");
        drop(temp);
        assert!(!folder.exists());
    }
}
