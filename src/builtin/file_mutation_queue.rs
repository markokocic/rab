use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

use crate::builtin;

/// Per-file queue entries. Each entry is a `Notify` that the NEXT operation
/// will wait on. Operations chain through these to serialize access.
static FILE_QUEUES: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Normalize a path for use as a queue key.
fn normalize_path_key(path: &str, cwd: &Path) -> String {
    builtin::resolve_path(path, cwd)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Serialize file mutation operations targeting the same file.
///
/// Operations for different files still run in parallel. This mirrors pi's
/// `withFileMutationQueue` in file-mutation-queue.ts.
///
/// The implementation:
/// - Each file has a `Notify` stored in a global map, representing the
///   "next operation" signal.
/// - An operation registers by replacing the entry with its own `Notify`
///   (for the operation after it), and picking up the previous `Notify`
///   to wait on.
/// - When the operation finishes, it signals its own `Notify` (which the
///   next operation is waiting on) and, if it is still the latest entry,
///   cleans up.
pub async fn with_file_mutation_queue<T, E, F, Fut>(
    file_path: &str,
    cwd: &Path,
    f: F,
) -> Result<T, E>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let key = normalize_path_key(file_path, cwd);

    // ── Registration phase ─────────────────────────────────────
    // Atomically: pick up the previous Notify (if any) and store ours.
    let our_notify = Arc::new(Notify::new());
    let prev_notify = {
        let mut queues = FILE_QUEUES.lock().unwrap();
        queues.insert(key.clone(), our_notify.clone())
    };

    // ── Wait for the previous operation to finish ──────────────
    if let Some(prev) = &prev_notify {
        prev.notified().await;
    }

    // ── Run the operation ──────────────────────────────────────
    let result = f().await;

    // ── Signal the next operation ──────────────────────────────
    // Our Notify may have been picked up by the next operation as
    // its prev_notify. Signal it so the next operation can proceed.
    our_notify.notify_one();

    // ── Clean up if we're still the latest entry ───────────────
    let mut queues = FILE_QUEUES.lock().unwrap();
    if let Some(current) = queues.get(&key)
        && Arc::ptr_eq(current, &our_notify)
    {
        // No new operation registered after us — clean up.
        queues.remove(&key);
    }
    // If a new operation registered, its own Notify is now in the
    // map; we leave it there for the next cleanup cycle.

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn runs_without_previous() {
        let cwd = Path::new("/tmp");
        let mut ran = false;
        with_file_mutation_queue("/tmp/test_file_1.txt", cwd, || async {
            ran = true;
            Ok::<_, String>(42)
        })
        .await
        .unwrap();
        assert!(ran);
    }

    #[tokio::test]
    async fn serializes_concurrent_access() {
        let cwd = Path::new("/tmp");
        let counter = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let c = counter.clone();
            let m = max.clone();
            handles.push(tokio::spawn(async move {
                with_file_mutation_queue("/tmp/test_serial.txt", cwd, || async {
                    let v = c.fetch_add(1, Ordering::SeqCst) + 1;
                    // Track the maximum concurrent count
                    let prev_max = m.fetch_max(v, Ordering::SeqCst);
                    // Simulate work
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    c.fetch_sub(1, Ordering::SeqCst);
                    // If max concurrent > 1, the queue didn't work
                    if prev_max >= 1 && v > 1 {
                        panic!("concurrent access detected: v={}", v);
                    }
                    Ok::<_, String>(())
                })
                .await
                .unwrap();
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Max concurrent should be 1 (serialized)
        assert_eq!(max.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn different_files_run_in_parallel() {
        let cwd = Path::new("/tmp");
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for i in 0..5 {
            handles.push(tokio::spawn(async move {
                with_file_mutation_queue(&format!("/tmp/parallel_{}.txt", i), cwd, || async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<_, String>(i)
                })
                .await
                .unwrap()
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // All 5 ran in parallel, so total time should be ~50ms not ~250ms
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(150),
            "took too long: {:?} — files ran sequentially instead of in parallel",
            elapsed
        );
    }

    #[tokio::test]
    async fn returns_value() {
        let cwd = Path::new("/tmp");
        let result: Result<i32, String> =
            with_file_mutation_queue("/tmp/retval.txt", cwd, || async { Ok(99) }).await;
        assert_eq!(result.unwrap(), 99);
    }

    #[tokio::test]
    async fn propagates_error() {
        let cwd = Path::new("/tmp");
        let result: Result<i32, String> =
            with_file_mutation_queue("/tmp/error.txt", cwd, || async { Err("oops".to_string()) })
                .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "oops");
    }

    #[tokio::test]
    async fn chains_correctly() {
        // Test that three operations on the same file run in order
        let cwd = Path::new("/tmp");
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut handles = Vec::new();
        for i in 0..3 {
            let o = order.clone();
            handles.push(tokio::spawn(async move {
                with_file_mutation_queue("/tmp/chaining.txt", cwd, || async {
                    // Simulate variable work time
                    tokio::time::sleep(Duration::from_millis(10 * (3 - i))).await;
                    o.lock().unwrap().push(i);
                    Ok::<_, String>(())
                })
                .await
                .unwrap()
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Despite task 0 having the longest sleep (30ms),
        // task 1 (20ms) and 2 (10ms) should execute AFTER task 0
        // because they're serialized
        let order = order.lock().unwrap();
        assert_eq!(*order, vec![0, 1, 2], "operations executed out of order");
    }
}
