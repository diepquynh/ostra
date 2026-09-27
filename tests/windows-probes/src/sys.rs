//! Thin FFI helpers over windows-sys for the security-defining probes: process tokens, restricted
//! tokens, spawning as another user (secondary logon) and under a restricted primary token, Job
//! Objects, cross-process open, hard links, short names, and volume flags. Small SID buffers are
//! intentionally leaked; this is a short-lived throwaway program.

#![allow(clippy::missing_safety_doc)]

use std::io;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, SID_AND_ATTRIBUTES, TOKEN_ALL_ACCESS,
    WRITE_RESTRICTED,
};
use windows_sys::Win32::Storage::FileSystem::{CreateHardLinkW, GetShortPathNameW};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW,
    CreateProcessWithLogonW, GetCurrentProcess, GetExitCodeProcess, INFINITE, LOGON_WITH_PROFILE,
    OpenProcess, OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_VM_READ, STARTUPINFOW, WaitForSingleObject,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn last_error() -> u32 {
    unsafe { GetLastError() }
}

/// A handle closed on drop.
pub struct OwnedHandle(pub HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Converts an "S-1-..." string to a leaked PSID.
pub fn str_to_sid(s: &str) -> io::Result<*mut std::ffi::c_void> {
    let mut psid: *mut std::ffi::c_void = std::ptr::null_mut();
    let ok = unsafe { ConvertStringSidToSidW(wide(s).as_ptr(), &mut psid) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(psid)
}

/// The current process's primary token, opened for all access so it can be restricted and assigned.
pub fn current_primary_token() -> io::Result<OwnedHandle> {
    let mut tok: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut tok) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(tok))
}

/// A restricted version of `base`: max privilege dropped, optionally write-restricted, and each
/// SID in `restrict` added as a restricting SID (the second access-check pass). A process run with
/// this token can reach an object only if the object's DACL grants one of the restricting SIDs too.
pub fn create_restricted(
    base: HANDLE,
    restrict: &[*mut std::ffi::c_void],
    write_restricted: bool,
) -> io::Result<OwnedHandle> {
    let mut sids: Vec<SID_AND_ATTRIBUTES> = restrict
        .iter()
        .map(|&sid| SID_AND_ATTRIBUTES { Sid: sid, Attributes: 0 })
        .collect();
    let mut flags = DISABLE_MAX_PRIVILEGE;
    if write_restricted {
        flags |= WRITE_RESTRICTED;
    }
    let mut new: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        CreateRestrictedToken(
            base,
            flags,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            sids.len() as u32,
            if sids.is_empty() { std::ptr::null_mut() } else { sids.as_mut_ptr() },
            &mut new,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(new))
}

/// A medium-integrity primary token duplicated from `base`. Used to test that
/// CreateProcessWithLogonW works from a non-elevated (medium-integrity) caller, the property Ostra's
/// unelevated server relies on (the SSH caller itself runs at High integrity).
pub fn medium_il_token(base: HANDLE) -> io::Result<OwnedHandle> {
    use windows_sys::Win32::Security::{
        DuplicateTokenEx, GetLengthSid, SID_AND_ATTRIBUTES, SecurityImpersonation,
        SetTokenInformation, TOKEN_MANDATORY_LABEL, TokenIntegrityLevel, TokenPrimary,
    };
    const SE_GROUP_INTEGRITY: u32 = 0x20;
    let mut dup: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        DuplicateTokenEx(base, TOKEN_ALL_ACCESS, std::ptr::null(), SecurityImpersonation, TokenPrimary, &mut dup)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let dup = OwnedHandle(dup);
    let med = str_to_sid("S-1-16-8192")?; // SECURITY_MANDATORY_MEDIUM_RID
    let mut label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES { Sid: med, Attributes: SE_GROUP_INTEGRITY },
    };
    let size = std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + unsafe { GetLengthSid(med) };
    let ok = unsafe {
        SetTokenInformation(dup.0, TokenIntegrityLevel, (&raw mut label).cast(), size)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(dup)
}

/// A spawned process, its handles owned so it is cleaned up and can be waited on.
pub struct Proc {
    pub pid: u32,
    pub process: OwnedHandle,
    // Held so the thread handle is closed on drop.
    #[allow(dead_code)]
    pub thread: OwnedHandle,
}

impl Proc {
    pub fn wait(&self) -> u32 {
        unsafe { WaitForSingleObject(self.process.0, INFINITE) };
        let mut code = 0u32;
        unsafe { GetExitCodeProcess(self.process.0, &mut code) };
        code
    }
}

fn blank_startupinfo() -> STARTUPINFOW {
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si
}

/// Starts `cmdline` as another local user through the secondary-logon service. Does not require the
/// caller to be elevated or to hold any special privilege; that is the property verify 1 tests.
pub fn spawn_with_logon(user: &str, password: &str, cmdline: &str) -> io::Result<Proc> {
    let mut cmd = wide(cmdline);
    let cwd = wide("C:\\Windows");
    let si = blank_startupinfo();
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessWithLogonW(
            wide(user).as_ptr(),
            std::ptr::null(),
            wide(password).as_ptr(),
            LOGON_WITH_PROFILE,
            std::ptr::null(),
            cmd.as_mut_ptr(),
            CREATE_NEW_CONSOLE | CREATE_UNICODE_ENVIRONMENT,
            std::ptr::null(),
            cwd.as_ptr(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Proc {
        pid: pi.dwProcessId,
        process: OwnedHandle(pi.hProcess),
        thread: OwnedHandle(pi.hThread),
    })
}

/// Starts `cmdline` under `token` (a primary token). When `token` is a restricted version of the
/// caller's own token, this does not need SeAssignPrimaryTokenPrivilege; verify 2 tests that.
pub fn spawn_as_user(token: HANDLE, cmdline: &str, suspended: bool) -> io::Result<Proc> {
    let mut cmd = wide(cmdline);
    let cwd = wide("C:\\Windows");
    let si = blank_startupinfo();
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let flags = CREATE_NEW_CONSOLE | if suspended { CREATE_SUSPENDED } else { 0 };
    let ok = unsafe {
        CreateProcessAsUserW(
            token,
            std::ptr::null(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            flags,
            std::ptr::null(),
            cwd.as_ptr(),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Proc {
        pid: pi.dwProcessId,
        process: OwnedHandle(pi.hProcess),
        thread: OwnedHandle(pi.hThread),
    })
}

/// Tries to open another process. Returns the Win32 error on failure (5 is access denied).
pub fn try_open_process(pid: u32) -> Result<OwnedHandle, u32> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if h.is_null() {
        return Err(last_error());
    }
    Ok(OwnedHandle(h))
}

/// A Job Object that kills its processes when its last handle closes (proctree.rs's design).
pub struct Job(pub HANDLE);
impl Job {
    pub fn new() -> io::Result<Job> {
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Job(h);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    pub fn assign(&self, process: HANDLE) -> io::Result<()> {
        if unsafe { AssignProcessToJobObject(self.0, process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn terminate(&self) {
        unsafe { TerminateJobObject(self.0, 1) };
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Resumes a process created suspended (std does not expose the main thread handle).
pub fn resume_process(process: HANDLE) -> io::Result<()> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }
    let status = unsafe { NtResumeProcess(process) };
    if status < 0 {
        return Err(io::Error::other(format!("NtResumeProcess NTSTATUS {status:#x}")));
    }
    Ok(())
}

/// Whether a process is still running.
pub fn is_alive(pid: u32) -> bool {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return false;
    }
    let mut code = 0u32;
    let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
    unsafe { CloseHandle(h) };
    ok && code == windows_sys::Win32::Foundation::STILL_ACTIVE as u32
}

/// Creates a hard link. Returns the Win32 error on failure.
pub fn create_hard_link(link: &str, target: &str) -> Result<(), u32> {
    let ok = unsafe {
        CreateHardLinkW(wide(link).as_ptr(), wide(target).as_ptr(), std::ptr::null_mut())
    };
    if ok == 0 { Err(last_error()) } else { Ok(()) }
}

/// The 8.3 short name of an existing path, if the volume keeps one.
pub fn short_path(path: &str) -> Option<String> {
    let w = wide(path);
    let mut buf = vec![0u16; 260];
    let n = unsafe { GetShortPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

/// Grants BUILTIN\Users access to the current window station and its desktop, so a process started
/// as a probe user (or under a restricted token whose restricting SIDs include Users) can finish
/// process init. Over a non-interactive session (SSH) the probe user otherwise has no access to the
/// station and the child dies at load with STATUS_DLL_INIT_FAILED (0xC0000142).
pub fn grant_users_to_session_ui() -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{
        GRANT_ACCESS, GetSecurityInfo, SE_WINDOW_OBJECT, SetEntriesInAclW, SetSecurityInfo,
        TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::Authorization::{EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE};
    use windows_sys::Win32::Security::ACL;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    // GetProcessWindowStation and GetThreadDesktop live in user32. Load them dynamically so the exe
    // carries no static user32 import; otherwise every confined child (restricted-token children
    // included) would load user32 and die at init connecting to a window station it cannot reach.
    let user32 = unsafe { LoadLibraryA(c"user32.dll".as_ptr().cast()) };
    if user32.is_null() {
        return Err(io::Error::other("could not load user32"));
    }
    let get_winsta: unsafe extern "system" fn() -> HANDLE = unsafe {
        std::mem::transmute(
            GetProcAddress(user32, c"GetProcessWindowStation".as_ptr().cast())
                .ok_or_else(|| io::Error::other("GetProcessWindowStation missing"))?,
        )
    };
    let get_desktop: unsafe extern "system" fn(u32) -> HANDLE = unsafe {
        std::mem::transmute(
            GetProcAddress(user32, c"GetThreadDesktop".as_ptr().cast())
                .ok_or_else(|| io::Error::other("GetThreadDesktop missing"))?,
        )
    };

    const DACL_INFO: u32 = 0x0000_0004; // DACL_SECURITY_INFORMATION
    const GENERIC_ALL: u32 = 0x1000_0000;
    const OBJECT_INHERIT: u32 = 0x1;
    const CONTAINER_INHERIT: u32 = 0x2;
    const NO_INHERIT: u32 = 0x0;

    let users = str_to_sid(crate::defs::USERS_SID)?;

    unsafe fn add_ace(handle: HANDLE, sid: *mut std::ffi::c_void, inherit: u32) -> io::Result<()> {
        let mut old_dacl: *mut ACL = std::ptr::null_mut();
        let mut psd: *mut std::ffi::c_void = std::ptr::null_mut();
        let rc = unsafe {
            GetSecurityInfo(
                handle,
                SE_WINDOW_OBJECT,
                DACL_INFO,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut old_dacl,
                std::ptr::null_mut(),
                &mut psd,
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let mut trustee: TRUSTEE_W = unsafe { std::mem::zeroed() };
        trustee.pMultipleTrustee = std::ptr::null_mut();
        trustee.MultipleTrusteeOperation = NO_MULTIPLE_TRUSTEE;
        trustee.TrusteeForm = TRUSTEE_IS_SID;
        trustee.TrusteeType = TRUSTEE_IS_GROUP;
        trustee.ptstrName = sid.cast();
        let ea = EXPLICIT_ACCESS_W {
            grfAccessPermissions: GENERIC_ALL,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: inherit,
            Trustee: trustee,
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        let rc = unsafe { SetEntriesInAclW(1, &ea, old_dacl, &mut new_dacl) };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let rc = unsafe {
            SetSecurityInfo(
                handle,
                SE_WINDOW_OBJECT,
                DACL_INFO,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }

    unsafe {
        let winsta = get_winsta();
        let desktop = get_desktop(GetCurrentThreadId());
        if winsta.is_null() || desktop.is_null() {
            return Err(io::Error::other("no window station or desktop for this session"));
        }
        // The window station gets an inheritable grant (covers child desktops) and the desktop its own.
        add_ace(winsta as HANDLE, users, OBJECT_INHERIT | CONTAINER_INHERIT)?;
        add_ace(desktop as HANDLE, users, NO_INHERIT)?;
    }
    Ok(())
}

