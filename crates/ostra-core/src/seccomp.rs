//! The kernel calls a bubblewrap sandbox refuses, loaded by `ostra sandbox-init` before it starts
//! the command, and inherited by everything the command starts. It refuses new user namespaces,
//! which would hand the command capabilities over fresh namespaces and mounts, and calls that
//! builds and tests do not need but that widen the kernel's surface. Refusals return an error, so
//! programs that probe for a feature fall back instead of dying.

use libc::{
    BPF_ABS, BPF_JEQ, BPF_JGE, BPF_JMP, BPF_JSET, BPF_K, BPF_LD, BPF_RET, BPF_W, ENOSYS, EPERM,
    SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO, SECCOMP_RET_KILL_PROCESS, sock_filter, sock_fprog,
};

// From <linux/audit.h>: the ELF machine with the 64-bit and little-endian bits.
#[cfg(target_arch = "x86_64")]
const ARCH: u32 = 0xC000_003E;
#[cfg(target_arch = "aarch64")]
const ARCH: u32 = 0xC000_00B7;

// Offsets into `struct seccomp_data`. Both supported arches are little-endian, so an argument's
// low 32 bits sit at its start.
const NR: u32 = 0;
const ARCH_OFFSET: u32 = 4;
const fn arg_low(i: u32) -> u32 {
    16 + 8 * i
}

const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// Calls refused with `EPERM`.
const REFUSED: &[libc::c_long] = &[
    libc::SYS_bpf,
    libc::SYS_userfaultfd,
    libc::SYS_perf_event_open,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    libc::SYS_kexec_load,
    libc::SYS_kexec_file_load,
    libc::SYS_init_module,
    libc::SYS_finit_module,
    libc::SYS_delete_module,
    libc::SYS_open_by_handle_at,
];

/// Calls that answer `ENOSYS`, so libc and libuv fall back to an older call. `clone3` passes its
/// flags in memory a filter cannot read, so glibc falls back to `clone`, whose flags it can.
const MISSING: &[libc::c_long] = &[
    libc::SYS_clone3,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
];

/// Terminal requests refused with `EPERM`: typing into the terminal, and the console's own
/// requests. `ioctl` takes its request as an `unsigned int`, so only the low 32 bits count.
const TTY_REFUSED: &[u32] = &[libc::TIOCSTI as u32, libc::TIOCLINUX as u32];

fn stmt(code: u32, k: u32) -> sock_filter {
    sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u32, k: u32, jt: u8, jf: u8) -> sock_filter {
    sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

fn ret(action: u32) -> sock_filter {
    stmt(BPF_RET | BPF_K, action)
}

fn errno(e: i32) -> u32 {
    SECCOMP_RET_ERRNO | (e as u32 & 0xFFFF)
}

/// `call` refused with `EPERM` when its argument `arg` has any bit of `flags` set.
fn refuse_flags(p: &mut Vec<sock_filter>, call: libc::c_long, arg: u32, flags: u32) {
    p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, call as u32, 0, 4));
    p.push(stmt(BPF_LD | BPF_W | BPF_ABS, arg_low(arg)));
    p.push(jump(BPF_JMP | BPF_JSET | BPF_K, flags, 0, 1));
    p.push(ret(errno(EPERM)));
    p.push(ret(SECCOMP_RET_ALLOW));
}

/// The filter program. Each block that loads an argument ends in a return, so the accumulator
/// holds the call number wherever a later block compares it.
pub fn program() -> Vec<sock_filter> {
    let mut p = vec![
        stmt(BPF_LD | BPF_W | BPF_ABS, ARCH_OFFSET),
        // Another arch's calls (32-bit x86 on x86_64) have other numbers, so none may pass.
        jump(BPF_JMP | BPF_JEQ | BPF_K, ARCH, 1, 0),
        ret(SECCOMP_RET_KILL_PROCESS),
        stmt(BPF_LD | BPF_W | BPF_ABS, NR),
    ];
    if cfg!(target_arch = "x86_64") {
        // x32 numbers reach the same calls under other numbers.
        p.push(jump(BPF_JMP | BPF_JGE | BPF_K, X32_SYSCALL_BIT, 0, 1));
        p.push(ret(errno(ENOSYS)));
    }
    for &call in REFUSED {
        p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, call as u32, 0, 1));
        p.push(ret(errno(EPERM)));
    }
    for &call in MISSING {
        p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, call as u32, 0, 1));
        p.push(ret(errno(ENOSYS)));
    }
    let newuser = libc::CLONE_NEWUSER as u32;
    refuse_flags(&mut p, libc::SYS_unshare, 0, newuser);
    // The flags are the first argument of `clone` on both arches.
    refuse_flags(&mut p, libc::SYS_clone, 0, newuser);
    let n = TTY_REFUSED.len() as u8;
    p.push(jump(
        BPF_JMP | BPF_JEQ | BPF_K,
        libc::SYS_ioctl as u32,
        0,
        n + 3,
    ));
    p.push(stmt(BPF_LD | BPF_W | BPF_ABS, arg_low(1)));
    for (i, &req) in TTY_REFUSED.iter().enumerate() {
        p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, req, n - i as u8, 0));
    }
    p.push(ret(SECCOMP_RET_ALLOW));
    p.push(ret(errno(EPERM)));
    p.push(ret(SECCOMP_RET_ALLOW));
    p
}

/// Loads the filter into this process. Call while it has one thread, because a filter applies
/// to the calling thread and to what it starts afterwards.
pub fn install() -> Result<(), String> {
    let mut p = program();
    let prog = sock_fprog {
        len: p.len() as u16,
        filter: p.as_mut_ptr(),
    };
    // SAFETY: plain prctl calls; `prog` points at `p`, which outlives both.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
            return Err(format!(
                "cannot set no_new_privs: {}",
                std::io::Error::last_os_error()
            ));
        }
        if libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &prog as *const sock_fprog,
        ) != 0
        {
            return Err(format!(
                "cannot load the seccomp filter: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the filter over one call, as the kernel would, and returns the action.
    fn eval(arch: u32, nr: u32, args: [u64; 6]) -> u32 {
        let p = program();
        let word = |off: u32| -> u32 {
            match off {
                0 => nr,
                4 => arch,
                o if o >= 16 => args[((o - 16) / 8) as usize] as u32,
                _ => unreachable!(),
            }
        };
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let i = p[pc];
            let code = i.code as u32;
            match code {
                c if c == BPF_LD | BPF_W | BPF_ABS => acc = word(i.k),
                c if c == BPF_RET | BPF_K => return i.k,
                c if c & 0x07 == BPF_JMP => {
                    let hit = match code & 0xF0 {
                        x if x == BPF_JEQ => acc == i.k,
                        x if x == BPF_JGE => acc >= i.k,
                        x if x == BPF_JSET => acc & i.k != 0,
                        _ => unreachable!(),
                    };
                    pc += if hit { i.jt } else { i.jf } as usize;
                }
                _ => unreachable!(),
            }
            pc += 1;
        }
    }

    fn call(nr: libc::c_long, args: [u64; 6]) -> u32 {
        eval(ARCH, nr as u32, args)
    }

    #[test]
    fn refuses_new_user_namespaces_and_allows_other_clones() {
        let newuser = libc::CLONE_NEWUSER as u64;
        assert_eq!(
            call(libc::SYS_unshare, [newuser, 0, 0, 0, 0, 0]),
            errno(EPERM)
        );
        assert_eq!(
            call(
                libc::SYS_clone,
                [newuser | libc::SIGCHLD as u64, 0, 0, 0, 0, 0]
            ),
            errno(EPERM)
        );
        let thread = (libc::CLONE_VM | libc::CLONE_THREAD | libc::CLONE_SIGHAND) as u64;
        assert_eq!(
            call(libc::SYS_clone, [thread, 0, 0, 0, 0, 0]),
            SECCOMP_RET_ALLOW
        );
        assert_eq!(
            call(libc::SYS_unshare, [libc::CLONE_NEWNS as u64, 0, 0, 0, 0, 0]),
            SECCOMP_RET_ALLOW
        );
        assert_eq!(call(libc::SYS_clone3, [0; 6]), errno(ENOSYS));
    }

    #[test]
    fn refuses_the_listed_calls_and_allows_the_rest() {
        for &c in REFUSED {
            assert_eq!(call(c, [0; 6]), errno(EPERM), "call {c}");
        }
        for &c in MISSING {
            assert_eq!(call(c, [0; 6]), errno(ENOSYS), "call {c}");
        }
        for c in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_execve,
            libc::SYS_openat,
        ] {
            assert_eq!(call(c, [0; 6]), SECCOMP_RET_ALLOW, "call {c}");
        }
    }

    #[test]
    fn refuses_terminal_injection_by_the_low_32_bits() {
        let (sti, linux) = (u64::from(TTY_REFUSED[0]), u64::from(TTY_REFUSED[1]));
        let winsz = u64::from(libc::TIOCGWINSZ as u32);
        assert_eq!(call(libc::SYS_ioctl, [0, sti, 0, 0, 0, 0]), errno(EPERM));
        assert_eq!(
            call(libc::SYS_ioctl, [0, sti | 1 << 32, 0, 0, 0, 0]),
            errno(EPERM)
        );
        assert_eq!(call(libc::SYS_ioctl, [0, linux, 0, 0, 0, 0]), errno(EPERM));
        assert_eq!(
            call(libc::SYS_ioctl, [0, winsz, 0, 0, 0, 0]),
            SECCOMP_RET_ALLOW
        );
    }

    #[test]
    fn stops_other_arches_and_x32_numbers() {
        assert_eq!(
            eval(0x4000_0003, libc::SYS_read as u32, [0; 6]),
            SECCOMP_RET_KILL_PROCESS
        );
        if cfg!(target_arch = "x86_64") {
            assert_eq!(
                eval(ARCH, X32_SYSCALL_BIT | libc::SYS_read as u32, [0; 6]),
                errno(ENOSYS)
            );
        }
    }
}
