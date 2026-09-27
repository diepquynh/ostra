//! A child process and everything it starts, stopped as one. On Unix the child leads a process
//! group of its own and is signalled through it. On Windows it runs in a Job Object created with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and no breakaway, so closing the job's handle, or Ostra
//! exiting, ends the whole tree.
//!
//! A Windows child must be in its job before it starts anything, or its first children escape the
//! job. [`prepare_tokio`] and [`prepare_std`] create it suspended, and [`Tree::of_tokio`] assigns it
//! and then resumes it.

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

/// Sets what [`Tree::of_tokio`] needs on a command before it spawns.
pub fn prepare_tokio(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATION_FLAGS);
}

/// [`prepare_tokio`] for a `std` command.
pub fn prepare_std(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATION_FLAGS);
    }
}

/// Suspended, so the job holds the child before its first instruction, and without a console
/// window of its own, which a server started without a console would otherwise open per command.
#[cfg(windows)]
const CREATION_FLAGS: u32 = windows_sys::Win32::System::Threading::CREATE_SUSPENDED
    | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// The process tree of one child.
#[derive(Debug)]
pub struct Tree {
    #[cfg(unix)]
    pgid: i32,
    #[cfg(windows)]
    job: Job,
}

impl Tree {
    /// Takes over a child spawned from a command given to [`prepare_tokio`]. On Windows this
    /// assigns it to a new job and resumes it; on failure the child is killed.
    pub fn of_tokio(child: &tokio::process::Child) -> std::io::Result<Tree> {
        let pid = child
            .id()
            .ok_or_else(|| std::io::Error::other("the child has already exited"))?;
        #[cfg(unix)]
        return Ok(Tree { pgid: pid as i32 });
        #[cfg(windows)]
        {
            let h = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("the child has already exited"))?;
            Self::adopt(pid, h)
        }
    }

    /// [`Tree::of_tokio`] for a child spawned from a command given to [`prepare_std`].
    pub fn of_std(child: &std::process::Child) -> std::io::Result<Tree> {
        #[cfg(unix)]
        return Ok(Tree {
            pgid: child.id() as i32,
        });
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            Self::adopt(child.id(), child.as_raw_handle())
        }
    }

    #[cfg(windows)]
    fn adopt(_pid: u32, process: std::os::windows::io::RawHandle) -> std::io::Result<Tree> {
        let process = process as HANDLE;
        let joined = Job::new().and_then(|job| job.assign(process).map(|()| job));
        let resumed = resume(process);
        match (joined, resumed) {
            (Ok(job), Ok(())) => Ok(Tree { job }),
            (Err(e), _) | (_, Err(e)) => {
                // A process that never ran cannot finish terminating, so it is resumed above
                // before it is killed here.
                // SAFETY: `process` is the live child's handle, owned by the caller.
                unsafe { windows_sys::Win32::System::Threading::TerminateProcess(process, 1) };
                Err(e)
            }
        }
    }

    /// Takes over a running child that was started outside Ostra's control of its creation flags,
    /// such as a PTY child. On Windows, what it started before this call is not in the job.
    pub fn adopt_running(pid: u32) -> std::io::Result<Tree> {
        #[cfg(unix)]
        return Ok(Tree { pgid: pid as i32 });
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
            };
            let job = Job::new()?;
            // SAFETY: plain call; the handle is checked and closed below.
            let h = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if h.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let joined = job.assign(h);
            // SAFETY: `h` was opened above and is not used again.
            unsafe { CloseHandle(h) };
            joined?;
            Ok(Tree { job })
        }
    }

    /// Asks the tree to stop: SIGTERM to the group on Unix. Windows has no such request for
    /// console programs outside a console of their own, so it does nothing there.
    pub fn terminate(&self) {
        #[cfg(unix)]
        // SAFETY: plain kill(2) on the group this tree's child leads.
        unsafe {
            libc::kill(-self.pgid, libc::SIGTERM);
            libc::kill(self.pgid, libc::SIGTERM);
        }
    }

    /// Lets go of the tree. On Windows this closes its job, which kills what is still in it; a
    /// Unix process group holds nothing to close.
    pub fn end(self) {}

    /// Kills every process of the tree.
    pub fn kill(&self) {
        #[cfg(unix)]
        // SAFETY: plain kill(2) on the group this tree's child leads.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
            libc::kill(self.pgid, libc::SIGKILL);
        }
        #[cfg(windows)]
        self.job.terminate();
    }
}

/// A Job Object that kills its processes when its last handle closes.
#[cfg(windows)]
#[derive(Debug)]
struct Job(HANDLE);

// SAFETY: a job handle may be used and closed from any thread.
#[cfg(windows)]
unsafe impl Send for Job {}
// SAFETY: every method takes `&self` and calls thread-safe kernel functions.
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    fn new() -> std::io::Result<Job> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // SAFETY: an unnamed job with default security; the handle is checked.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let job = Job(h);
        // SAFETY: zeroed is a valid value of this plain C struct.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // Breakaway stays off, so no process of the tree can leave the job.
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is the struct this information class names, passed with its size.
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign(&self, process: HANDLE) -> std::io::Result<()> {
        // SAFETY: both handles are live for the call.
        if unsafe { windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(self.0, process) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn terminate(&self) {
        // SAFETY: the job handle is live until drop.
        unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(self.0, 1) };
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed once; closing it kills the job's processes.
        unsafe { CloseHandle(self.0) };
    }
}

/// Resumes every thread of a process created suspended, through its handle. std does not return
/// the main thread's handle, and a thread snapshot taken right after creation can miss the new
/// thread, which would leave the process suspended and unable even to terminate.
/// `NtResumeProcess` is an ntdll export, stable since Windows XP, that resumes the threads the
/// kernel holds for the process.
#[cfg(windows)]
fn resume(process: HANDLE) -> std::io::Result<()> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }
    // SAFETY: `process` is a live handle with PROCESS_SUSPEND_RESUME, as std's child handles have
    // full access.
    let status = unsafe { NtResumeProcess(process) };
    if status < 0 {
        return Err(std::io::Error::other(format!(
            "could not resume the process after placing it in its job (NTSTATUS {status:#x})"
        )));
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn killing_the_tree_ends_a_grandchild() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("pid");
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-Command",
            &format!(
                "$p = Start-Process -PassThru -WindowStyle Hidden powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep 60'; Set-Content -LiteralPath '{}' $p.Id; Start-Sleep 60",
                marker.display()
            ),
        ]);
        prepare_std(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = Tree::of_std(&child).unwrap();
        let started = Instant::now();
        while !marker.exists() && started.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let grandchild: u32 = std::fs::read_to_string(&marker)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(tree);
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive(grandchild), "the grandchild outlived its job");
    }

    fn alive(pid: u32) -> bool {
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        // SAFETY: plain calls; the handle is checked and closed.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            ok && code == windows_sys::Win32::Foundation::STILL_ACTIVE as u32
        }
    }

    /// Every child created suspended starts once adopted, so none is left waiting to run.
    #[test]
    fn adopted_children_always_start() {
        let programs: [(std::path::PathBuf, &[&str]); 2] = [
            (crate::shells::cmd(), &["/d", "/c", "exit 7"]),
            (
                crate::shells::powershell(),
                &["-NoProfile", "-NonInteractive", "-Command", "exit 7"],
            ),
        ];
        for (program, args) in programs {
            for i in 0..10 {
                let mut cmd = std::process::Command::new(&program);
                cmd.args(args)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                prepare_std(&mut cmd);
                let child = cmd.spawn().unwrap();
                let tree = Tree::of_std(&child).unwrap();
                let out = child.wait_with_output().unwrap();
                assert_eq!(out.status.code(), Some(7), "{} spawn {i}", program.display());
                drop(tree);
            }
        }
    }
}
