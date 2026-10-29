//! One-shot shell commands for the composer's bash mode.
//!
//! Each run is `$SHELL -lc '<command>'` in its own process group, with stdout
//! and stderr sharing one pipe so the output keeps its order. Only the last
//! [`SHELL_OUTPUT_CAP`] bytes are kept. A run ends when the shell exits, the
//! timeout fires or [`ShellRuns::cancel`] is called. The run never waits for
//! EOF, because a backgrounded child (`sleep 300 &`) keeps the pipe open.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::oneshot;

/// How long a command may run before its group is killed.
pub(crate) const SHELL_RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// The most output a run keeps. Past it the head is dropped.
pub(crate) const SHELL_OUTPUT_CAP: usize = 65_536;
/// Between SIGTERM and SIGKILL when a run is stopped.
pub(crate) const SHELL_KILL_GRACE: Duration = Duration::from_secs(2);
/// How long the pipe is still read after the shell exits.
const SHELL_DRAIN: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellRunResult {
    pub exit_code: Option<i32>,
    pub output: String,
    pub truncated: bool,
    pub timed_out: bool,
}

struct RunHandle {
    /// Tells this run apart from a later one that reuses its ID.
    token: u64,
    cancel: oneshot::Sender<()>,
    /// The shell's process group while it may still be signalled.
    #[cfg(unix)]
    group: Option<libc::pid_t>,
}

impl ShellRunResult {
    fn failed(output: String) -> Self {
        Self {
            exit_code: None,
            output,
            truncated: false,
            timed_out: false,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ShellRuns {
    runs: Arc<Mutex<HashMap<String, RunHandle>>>,
    timeout: Duration,
    cap: usize,
}

impl Default for ShellRuns {
    fn default() -> Self {
        Self {
            runs: Arc::default(),
            timeout: SHELL_RUN_TIMEOUT,
            cap: SHELL_OUTPUT_CAP,
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

enum Ending {
    Exited(Option<i32>),
    TimedOut,
    Cancelled,
}

impl ShellRuns {
    #[cfg(test)]
    fn with_limits(timeout: Duration, cap: usize) -> Self {
        Self {
            timeout,
            cap,
            ..Self::default()
        }
    }

    /// Runs `command` in `cwd` and returns once it exits, times out or is
    /// cancelled. A command that can't start, or whose `run_id` is already
    /// running, reports why as its output, with no exit code.
    pub(crate) async fn run(&self, run_id: String, cwd: &Path, command: &str) -> ShellRunResult {
        static TOKENS: AtomicU64 = AtomicU64::new(0);
        let token = TOKENS.fetch_add(1, Ordering::Relaxed);
        let (cancel, mut cancelled) = oneshot::channel();
        match lock(&self.runs).entry(run_id.clone()) {
            Entry::Occupied(_) => {
                return ShellRunResult::failed(format!("run {run_id} is already running"));
            }
            Entry::Vacant(slot) => {
                slot.insert(RunHandle {
                    token,
                    cancel,
                    #[cfg(unix)]
                    group: None,
                });
            }
        }
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut guard = RunGuard {
            runs: &self.runs,
            run_id,
            token,
            #[cfg(unix)]
            group: None,
        };

        let shell = crate::terminals::selected_shell();
        let (reader, mut child) = match spawn(&shell, cwd, command) {
            Ok(spawned) => spawned,
            Err(error) => return ShellRunResult::failed(format!("could not run {shell}: {error}")),
        };
        #[cfg(unix)]
        guard.set_group(
            child
                .id()
                .map(|pid| pid as libc::pid_t)
                .filter(|pid| *pid > 0),
        );
        let mut reader = Some(reader);
        let mut tail = Tail::new(self.cap);

        let ending = pump(&mut reader, &mut tail, async {
            tokio::select! {
                status = child.wait() => Ending::Exited(status.ok().and_then(|s| s.code())),
                _ = tokio::time::sleep(self.timeout) => Ending::TimedOut,
                Ok(()) = &mut cancelled => Ending::Cancelled,
            }
        })
        .await;

        let exit_code = match ending {
            Ending::Exited(code) => code,
            Ending::TimedOut | Ending::Cancelled => {
                #[cfg(unix)]
                if let Some(group) = guard.group {
                    signal_group(group, libc::SIGTERM);
                    // Reap the shell while polling: a live group keeps its ID,
                    // so the SIGKILL can't reach a reused PID.
                    let emptied = pump(
                        &mut reader,
                        &mut tail,
                        tokio::time::timeout(SHELL_KILL_GRACE, async {
                            loop {
                                let _ = child.try_wait();
                                if !group_alive(group) {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(25)).await;
                            }
                        }),
                    )
                    .await;
                    if emptied.is_err() {
                        signal_group(group, libc::SIGKILL);
                    }
                }
                let _ = child.start_kill();
                let _ = pump(&mut reader, &mut tail, child.wait()).await;
                None
            }
        };
        // The shell is gone: what it left in the group either was killed above
        // or was backgrounded on purpose and outlives the run.
        #[cfg(unix)]
        guard.set_group(None);
        let _ = pump(&mut reader, &mut tail, tokio::time::sleep(SHELL_DRAIN)).await;
        drop(guard);

        let (output, truncated) = tail.finish();
        ShellRunResult {
            exit_code,
            output,
            truncated,
            timed_out: matches!(ending, Ending::TimedOut),
        }
    }

    /// Stops the run with this ID. An unknown or finished ID is a no-op.
    pub(crate) fn cancel(&self, run_id: &str) {
        if let Some(handle) = lock(&self.runs).remove(run_id) {
            let _ = handle.cancel.send(());
        }
    }

    /// Engine stop: SIGKILLs every running group and ends its run.
    pub(crate) fn shutdown(&self) {
        let runs: Vec<_> = lock(&self.runs).drain().map(|(_, run)| run).collect();
        for run in runs {
            #[cfg(unix)]
            if let Some(group) = run.group {
                signal_group(group, libc::SIGKILL);
            }
            let _ = run.cancel.send(());
        }
    }
}

/// Removes the run's map entry on every exit path. A run dropped mid-flight
/// (the engine stopping) also SIGKILLs its group: `kill_on_drop` only reaches
/// the shell, not what the shell started.
struct RunGuard<'a> {
    runs: &'a Mutex<HashMap<String, RunHandle>>,
    run_id: String,
    token: u64,
    #[cfg(unix)]
    group: Option<libc::pid_t>,
}

impl RunGuard<'_> {
    /// Mirrors the group into the map entry so [`ShellRuns::shutdown`] sees it.
    #[cfg(unix)]
    fn set_group(&mut self, group: Option<libc::pid_t>) {
        self.group = group;
        if let Some(run) = lock(self.runs)
            .get_mut(&self.run_id)
            .filter(|run| run.token == self.token)
        {
            run.group = group;
        }
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        let mut runs = lock(self.runs);
        if runs
            .get(&self.run_id)
            .is_some_and(|run| run.token == self.token)
        {
            runs.remove(&self.run_id);
        }
        drop(runs);
        #[cfg(unix)]
        if let Some(group) = self.group {
            signal_group(group, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn signal_group(group: libc::pid_t, signal: libc::c_int) {
    // SAFETY: the shell spawned with `process_group(0)`, so its PID names its
    // own group, and `group > 0` keeps this off our own group.
    unsafe { libc::killpg(group, signal) };
}

#[cfg(unix)]
fn group_alive(group: libc::pid_t) -> bool {
    // SAFETY: signal 0 only probes whether the group still has members.
    let probed = unsafe { libc::killpg(group, 0) };
    probed == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn spawn(
    shell: &str,
    cwd: &Path,
    command: &str,
) -> std::io::Result<(impl AsyncRead + Unpin + Send, tokio::process::Child)> {
    let (reader, writer) = std::io::pipe()?;
    let mut process = Command::new(shell);
    #[cfg(unix)]
    process.arg("-lc").process_group(0);
    #[cfg(windows)]
    process.arg("/C");
    process
        .arg(command)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .kill_on_drop(true);
    if let Some(path) = zeron_harness::shell_env::login_shell_path() {
        process.env("PATH", path);
    }
    let child = process.spawn()?;
    // Drop our copies of the write end so the pipe reaches EOF once every
    // process in the group has exited.
    drop(process);
    Ok((async_reader(reader)?, child))
}

#[cfg(unix)]
fn async_reader(reader: std::io::PipeReader) -> std::io::Result<impl AsyncRead + Unpin + Send> {
    tokio::net::unix::pipe::Receiver::from_owned_fd(reader.into())
}

/// Windows has no async anonymous pipe in tokio. The blocking read lives on
/// tokio's blocking pool until the pipe closes.
#[cfg(windows)]
fn async_reader(reader: std::io::PipeReader) -> std::io::Result<impl AsyncRead + Unpin + Send> {
    let handle = std::os::windows::io::OwnedHandle::from(reader);
    Ok(tokio::fs::File::from_std(std::fs::File::from(handle)))
}

/// Awaits `future` while moving whatever the pipe yields into `tail`.
async fn pump<R, F>(reader: &mut Option<R>, tail: &mut Tail, future: F) -> F::Output
where
    R: AsyncRead + Unpin,
    F: Future,
{
    tokio::pin!(future);
    let mut buf = [0u8; 8192];
    loop {
        tokio::select! {
            output = &mut future => return output,
            read = read_chunk(reader, &mut buf) => match read {
                Some(n) => tail.push(&buf[..n]),
                None => *reader = None,
            },
        }
    }
}

/// The next chunk, `None` at EOF or on error, and never once the pipe closed.
async fn read_chunk<R: AsyncRead + Unpin>(reader: &mut Option<R>, buf: &mut [u8]) -> Option<usize> {
    match reader {
        Some(reader) => match reader.read(buf).await {
            Ok(0) | Err(_) => None,
            Ok(n) => Some(n),
        },
        None => std::future::pending().await,
    }
}

/// The last `cap` bytes seen.
struct Tail {
    bytes: VecDeque<u8>,
    cap: usize,
    truncated: bool,
}

impl Tail {
    fn new(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            cap,
            truncated: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        let kept = &chunk[chunk.len().saturating_sub(self.cap)..];
        let overflow = (self.bytes.len() + kept.len()).saturating_sub(self.cap);
        if overflow > 0 || kept.len() < chunk.len() {
            self.truncated = true;
        }
        self.bytes.drain(..overflow);
        self.bytes.extend(kept);
    }

    fn finish(mut self) -> (String, bool) {
        let output = String::from_utf8_lossy(self.bytes.make_contiguous()).into_owned();
        (output, self.truncated)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[tokio::test]
    async fn runs_in_cwd_and_reports_exit() {
        let dir = tmp();
        let result = ShellRuns::default()
            .run("a".into(), dir.path(), "pwd; exit 3")
            .await;
        let cwd = dir.path().canonicalize().unwrap();
        assert!(
            result.output.contains(cwd.to_str().unwrap()),
            "{:?}",
            result.output
        );
        assert_eq!(result.exit_code, Some(3));
        assert!(!result.truncated && !result.timed_out);
    }

    #[tokio::test]
    async fn merges_stderr_in_order() {
        let dir = tmp();
        let result = ShellRuns::default()
            .run("b".into(), dir.path(), "echo one; echo two >&2; echo three")
            .await;
        assert_eq!(result.output, "one\ntwo\nthree\n");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn caps_output_keeping_the_tail() {
        let dir = tmp();
        let result = ShellRuns::with_limits(SHELL_RUN_TIMEOUT, 16)
            .run("c".into(), dir.path(), "printf 'aaaaaaaaaaaaaaaaaaaaZ'")
            .await;
        assert!(result.truncated);
        assert!(result.output.ends_with('Z'), "{:?}", result.output);
        assert_eq!(result.output.len(), 16);
    }

    #[tokio::test]
    async fn non_utf8_output_is_lossy() {
        let dir = tmp();
        let result = ShellRuns::default()
            .run("d".into(), dir.path(), "printf '\\377\\376ok'")
            .await;
        assert!(result.output.ends_with("ok"), "{:?}", result.output);
    }

    #[tokio::test]
    async fn timeout_kills_the_group() {
        let dir = tmp();
        let started = Instant::now();
        let result = ShellRuns::with_limits(Duration::from_millis(300), SHELL_OUTPUT_CAP)
            .run("e".into(), dir.path(), "sleep 30")
            .await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
    }

    #[tokio::test]
    async fn cancel_ends_a_running_command() {
        let dir = tmp();
        let runs = ShellRuns::default();
        let running = tokio::spawn({
            let runs = runs.clone();
            let cwd = dir.path().to_path_buf();
            async move { runs.run("c".into(), &cwd, "sleep 30").await }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        runs.cancel("c");
        let result = tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .expect("cancelled run resolves")
            .unwrap();
        assert_eq!(result.exit_code, None);
        assert!(!result.timed_out);
        assert!(lock(&runs.runs).is_empty());
    }

    #[tokio::test]
    async fn background_child_does_not_hold_the_run() {
        let dir = tmp();
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            ShellRuns::default().run("f".into(), dir.path(), "sleep 5 & echo hi"),
        )
        .await
        .expect("run resolves while the background child lives");
        assert_eq!(result.output, "hi\n");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn cancel_unknown_is_noop() {
        ShellRuns::default().cancel("nope");
    }

    /// Starts `command` as run `id` on its own task.
    fn start(
        runs: &ShellRuns,
        id: &str,
        dir: &Path,
        command: &'static str,
    ) -> tokio::task::JoinHandle<ShellRunResult> {
        let (runs, id, cwd) = (runs.clone(), id.to_string(), dir.to_path_buf());
        tokio::spawn(async move { runs.run(id, &cwd, command).await })
    }

    /// The PID a command wrote to `pid` in `dir`.
    async fn written_pid(dir: &Path) -> libc::pid_t {
        loop {
            if let Some(pid) = std::fs::read_to_string(dir.join("pid"))
                .ok()
                .and_then(|s| s.trim().parse().ok())
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn assert_gone(pid: libc::pid_t) {
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: signal 0 only probes whether the PID still exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "process {pid} survived");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn dropping_a_run_kills_its_group() {
        let dir = tmp();
        let runs = ShellRuns::default();
        let running = start(&runs, "g", dir.path(), "sleep 30 & echo $! > pid; wait");
        let pid = written_pid(dir.path()).await;
        running.abort();
        let _ = running.await;
        assert_gone(pid).await;
        assert!(lock(&runs.runs).is_empty());
    }

    #[tokio::test]
    async fn shutdown_kills_a_running_command() {
        let dir = tmp();
        let runs = ShellRuns::default();
        let running = start(&runs, "h", dir.path(), "sleep 30 & echo $! > pid; wait");
        let pid = written_pid(dir.path()).await;
        runs.shutdown();
        let result = tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .expect("run resolves after shutdown")
            .unwrap();
        assert_eq!(result.exit_code, None);
        assert_gone(pid).await;
        assert!(lock(&runs.runs).is_empty());
    }

    #[tokio::test]
    async fn duplicate_run_id_is_refused() {
        let dir = tmp();
        let runs = ShellRuns::default();
        let first = start(&runs, "dup", dir.path(), "sleep 30");
        tokio::time::sleep(Duration::from_millis(200)).await;
        let second = runs.run("dup".into(), dir.path(), "echo second").await;
        assert_eq!(second.exit_code, None);
        assert_eq!(second.output, "run dup is already running");
        runs.cancel("dup");
        let first = tokio::time::timeout(Duration::from_secs(3), first)
            .await
            .expect("the first run stays cancellable")
            .unwrap();
        assert_eq!(first.exit_code, None);
        assert!(lock(&runs.runs).is_empty());
    }

    #[tokio::test]
    async fn sigkill_waits_the_grace_for_a_group_ignoring_sigterm() {
        let dir = tmp();
        let started = Instant::now();
        let result = ShellRuns::with_limits(Duration::from_millis(300), SHELL_OUTPUT_CAP)
            .run(
                "i".into(),
                dir.path(),
                "(trap '' TERM; sleep 30) & echo $! > pid; wait",
            )
            .await;
        let elapsed = started.elapsed();
        assert!(result.timed_out);
        assert!(elapsed >= SHELL_KILL_GRACE, "{elapsed:?}");
        assert!(
            elapsed < SHELL_KILL_GRACE + Duration::from_secs(2),
            "{elapsed:?}"
        );
        assert_gone(written_pid(dir.path()).await).await;
    }
}
