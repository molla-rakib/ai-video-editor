//! Opening files and folders with the system's apps, and ending process trees.

use std::path::Path;

/// Shows `path` in the file manager (selected on Windows; its folder elsewhere).
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(not(windows))]
    open(path.parent().unwrap_or(path));
}

/// Opens a file with its default app.
pub fn open(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Opens a web link in the default browser. Only http(s) and mailto links are opened.
pub fn open_url(url: &str) {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")) {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(url.trim()).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(url.trim()).spawn();
}

/// Makes `cmd` start its process in its own process group, so a `ProcessGroup` can end
/// everything it starts. (Windows uses a job object instead, set up by `ProcessGroup::of`.)
pub fn own_process_group(cmd: &mut std::process::Command) {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    #[cfg(not(unix))]
    let _ = cmd;
}

/// A child process and everything it starts, ended together by `kill`. A `cmd /C` wrapper or
/// a launcher script would otherwise leave the real program running on its own.
///
/// On Windows this is a job object: every process in it ends at once (so none can react to
/// another's death), and they also end if this app exits or crashes. On Linux it's the
/// process group from `own_process_group`.
pub struct ProcessGroup {
    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessGroup {
    /// Call right after spawning `child` (with `own_process_group` on its command).
    pub fn of(child: &std::process::Child) -> ProcessGroup {
        #[cfg(windows)]
        return ProcessGroup { job: job::Job::with(child) };
        #[cfg(not(windows))]
        {
            let _ = child;
            ProcessGroup {}
        }
    }

    /// Ends the child and everything it started, then reaps the child.
    pub fn kill(&self, child: &mut std::process::Child) {
        // Only while it runs: an unreaped child keeps its id from being reused, a reaped one doesn't.
        if matches!(child.try_wait(), Ok(None)) {
            #[cfg(windows)]
            match &self.job {
                Some(job) => job.terminate(),
                None => {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("taskkill")
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .creation_flags(0x0800_0000)
                        .status();
                }
            }
            #[cfg(unix)]
            if let Ok(pid) = i32::try_from(child.id()) {
                // SAFETY: plain syscall; the group id is the child's pid, which is still ours (not reaped).
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        flags: u32,
        min_working_set: usize,
        max_working_set: usize,
        active_processes: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    /// JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// An owned job handle (kept as an integer so the owner stays `Send`).
    pub struct Job(isize);

    impl Job {
        /// A job holding `child`; whatever it starts from now on joins too.
        pub fn with(child: &std::process::Child) -> Option<Job> {
            // SAFETY: plain Win32 calls on handles we own; the limits struct matches the C layout.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Job(handle as isize);
                let mut limits = ExtendedLimits::default();
                limits.basic.flags = KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(handle, EXTENDED_LIMIT_INFORMATION, &limits as *const _ as *const c_void, std::mem::size_of::<ExtendedLimits>() as u32);
                (set != 0 && AssignProcessToJobObject(handle, child.as_raw_handle()) != 0).then_some(job)
            }
        }

        pub fn terminate(&self) {
            // SAFETY: our own job handle.
            unsafe { TerminateJobObject(self.0 as *mut c_void, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: our own job handle, closed once; with KILL_ON_JOB_CLOSE this also ends what's left.
            unsafe { CloseHandle(self.0 as *mut c_void) };
        }
    }
}
