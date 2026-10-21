//! Each bounded operation owns its Tokio runtime; no runtime is driven on GPUI's thread.
use futures_channel::oneshot;
use std::{
    future::Future,
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub fn spawn<T, F>(cancel: Arc<AtomicBool>, future: F) -> oneshot::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    // If thread creation fails, dropping tx still wakes the UI receiver with a terminal error.
    let _=std::thread::Builder::new().name("dbide-tools-readonly".into()).spawn(move||{
        let result=(||{
            if cancel.load(Ordering::Acquire){return Err("Operation cancelled".into());}
            let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_|"Could not start read-only tools runtime".to_string())?;
            runtime.block_on(async move{
                tokio::select!{
                    biased;
                    _=async{loop{if cancel.load(Ordering::Acquire){break;}tokio::time::sleep(Duration::from_millis(50)).await;}}=>Err("Operation cancelled; dedicated read-only session retired".into()),
                    result=tokio::time::timeout(Duration::from_secs(135),future)=>result.unwrap_or_else(|_|Err("Tools operation timed out; dedicated read-only session retired".into()))
                }
            })
        })();let _=tx.send(result);
    });
    rx
}
pub fn read_text(path: &Path) -> Result<String, String> {
    let file =
        std::fs::File::open(path).map_err(|_| "Could not open selected input".to_string())?;
    let mut bytes = vec![];
    file.take(16_000_001)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read selected input".to_string())?;
    if bytes.len() > 16_000_000 {
        return Err("Schema input exceeds 16 MB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "Schema input must be UTF-8".into())
}
/// New-path only; atomic no-clobber persistence protects existing destinations and symlinks.
pub fn write_new_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path).map_err(|e| e.error)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_preserve_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.json");
        write_new_atomic(&path, b"first").unwrap();
        assert!(write_new_atomic(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn cancelled_worker_never_evaluates_payload() {
        let cancel = Arc::new(AtomicBool::new(true));
        let receiver = spawn(cancel, async {
            panic!("cancelled job ran");
            #[allow(unreachable_code)]
            Ok::<(), String>(())
        });
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(receiver)
            .unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
    }
}
