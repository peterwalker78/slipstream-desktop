//! The system calls a sandboxed plugin is refused, as a seccomp filter handed to bubblewrap
//! (`bwrap --seccomp`). The sandbox already keeps a plugin away from your files and programs;
//! this takes away the kernel's own ways round it, whole kinds at a time, much as Flatpak does:
//!
//! - keyrings, where a login can keep keys;
//! - namespaces and mounts, the stuff a sandbox is made of;
//! - typing into a terminal (`TIOCSTI`, `TIOCLINUX`);
//! - the kernel's own tools a program has no business with: loading modules, `bpf`, `kexec`,
//!   `perf_event_open`, `userfaultfd`, `ptrace` and reading other programs' memory, the kernel
//!   log, and NUMA memory placement;
//! - `io_uring`, which runs work out of the filter's sight;
//! - sockets of any family but local, internet and netlink;
//! - and any system call from another architecture's table, the 32-bit one or x32's.
//!
//! What's refused fails with an error (EPERM, or ENOSYS for `io_uring`, so programs fall back to
//! ordinary reads and writes), rather than ending the program. The filter is a classic BPF
//! program, built here by hand for x86_64 and aarch64; elsewhere there is none.

/// One instruction of a classic BPF program, as the kernel reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Instruction {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

const LD_W_ABS: u16 = 0x20; // BPF_LD | BPF_W | BPF_ABS
const JEQ_K: u16 = 0x15; // BPF_JMP | BPF_JEQ | BPF_K
const JGE_K: u16 = 0x35; // BPF_JMP | BPF_JGE | BPF_K
const RET_K: u16 = 0x06; // BPF_RET | BPF_K

const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ERRNO: u32 = 0x0005_0000;

/// Where `struct seccomp_data` keeps the call's number, its architecture, and the low half of
/// each argument (little-endian).
const NR: u32 = 0;
const ARCH: u32 = 4;
fn arg_low(n: u32) -> u32 {
    16 + 8 * n
}

#[cfg(target_arch = "x86_64")]
const AUDIT_ARCH: u32 = 0xc000_003e;
#[cfg(target_arch = "aarch64")]
const AUDIT_ARCH: u32 = 0xc000_00b7;

/// The socket families a plugin may open: local, internet (both) and netlink, which ordinary
/// programs use to find their own addresses.
const FAMILIES: [i32; 4] = [
    libc::AF_UNIX,
    libc::AF_INET,
    libc::AF_INET6,
    libc::AF_NETLINK,
];

/// The calls refused outright, with EPERM.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn refused() -> Vec<i64> {
    let mut calls = vec![
        libc::SYS_add_key,
        libc::SYS_keyctl,
        libc::SYS_request_key,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_open_tree,
        libc::SYS_move_mount,
        libc::SYS_fsopen,
        libc::SYS_fsconfig,
        libc::SYS_fsmount,
        libc::SYS_fspick,
        libc::SYS_mount_setattr,
        libc::SYS_syslog,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_perf_event_open,
        libc::SYS_bpf,
        libc::SYS_userfaultfd,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_move_pages,
        libc::SYS_mbind,
        libc::SYS_get_mempolicy,
        libc::SYS_set_mempolicy,
        libc::SYS_migrate_pages,
    ];
    #[cfg(target_arch = "x86_64")]
    calls.extend([libc::SYS_uselib, libc::SYS_iopl, libc::SYS_ioperm]);
    calls
}

/// The filter, as bubblewrap reads it from a file: the instructions back to back.
pub fn program() -> Option<Vec<u8>> {
    let instructions = instructions()?;
    let mut bytes = Vec::with_capacity(instructions.len() * 8);
    for i in instructions {
        bytes.extend(i.code.to_ne_bytes());
        bytes.push(i.jt);
        bytes.push(i.jf);
        bytes.extend(i.k.to_ne_bytes());
    }
    Some(bytes)
}

fn op(code: u16, k: u32) -> Instruction {
    Instruction {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

/// `jeq k`, going on to the next instruction when equal and skipping `skip` when not.
fn jeq(k: u32, skip: u8) -> Instruction {
    Instruction {
        code: JEQ_K,
        jt: 0,
        jf: skip,
        k,
    }
}

fn errno(errno: i32) -> Instruction {
    op(RET_K, RET_ERRNO | (errno as u32 & 0xffff))
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub fn instructions() -> Option<Vec<Instruction>> {
    let mut p = vec![
        // Another architecture's numbers mean other calls: refuse them all.
        op(LD_W_ABS, ARCH),
        Instruction {
            code: JEQ_K,
            jt: 1,
            jf: 0,
            k: AUDIT_ARCH,
        },
        errno(libc::EPERM),
        op(LD_W_ABS, NR),
    ];
    // x32's calls are x86_64's numbers with this bit set.
    #[cfg(target_arch = "x86_64")]
    p.extend([
        Instruction {
            code: JGE_K,
            jt: 0,
            jf: 1,
            k: 0x4000_0000,
        },
        errno(libc::EPERM),
    ]);
    for call in refused() {
        p.extend([jeq(call as u32, 1), errno(libc::EPERM)]);
    }
    for call in [
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ] {
        p.extend([jeq(call as u32, 1), errno(libc::ENOSYS)]);
    }
    // ioctl: no typing into a terminal.
    p.extend([
        jeq(libc::SYS_ioctl as u32, 5),
        op(LD_W_ABS, arg_low(1)),
        jeq(libc::TIOCSTI as u32, 1),
        errno(libc::EPERM),
        jeq(libc::TIOCLINUX as u32, 1),
        errno(libc::EPERM),
    ]);
    // Reloads the number, which the ioctl check replaced, for the socket check after it.
    p.push(op(LD_W_ABS, NR));
    // socket: only the families it may use.
    let families = FAMILIES.len() as u8;
    p.extend([
        jeq(libc::SYS_socket as u32, 2 + 2 * families),
        op(LD_W_ABS, arg_low(0)),
    ]);
    for family in FAMILIES {
        p.extend([jeq(family as u32, 1), op(RET_K, RET_ALLOW)]);
    }
    p.push(errno(libc::EAFNOSUPPORT));
    p.push(op(RET_K, RET_ALLOW));
    Some(p)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn instructions() -> Option<Vec<Instruction>> {
    None
}

#[cfg(all(test, any(target_arch = "x86_64", target_arch = "aarch64")))]
mod tests {
    use super::*;

    /// Runs the filter on a call, as the kernel would, and says what it returns.
    fn run(program: &[Instruction], arch: u32, nr: i64, args: [u64; 6]) -> u32 {
        let word = |at: u32| -> u32 {
            match at {
                NR => nr as u32,
                ARCH => arch,
                at => {
                    let arg = args[((at - 16) / 8) as usize];
                    if (at - 16) % 8 == 0 {
                        arg as u32
                    } else {
                        (arg >> 32) as u32
                    }
                }
            }
        };
        let (mut pc, mut a) = (0usize, 0u32);
        loop {
            let i = program[pc];
            pc += 1;
            match i.code {
                LD_W_ABS => a = word(i.k),
                JEQ_K => pc += usize::from(if a == i.k { i.jt } else { i.jf }),
                JGE_K => pc += usize::from(if a >= i.k { i.jt } else { i.jf }),
                RET_K => return i.k,
                other => panic!("unexpected instruction {other:#x}"),
            }
        }
    }

    fn call(nr: i64, args: [u64; 6]) -> u32 {
        run(&instructions().unwrap(), AUDIT_ARCH, nr, args)
    }

    const NONE: [u64; 6] = [0; 6];
    const EPERM: u32 = RET_ERRNO | libc::EPERM as u32;

    /// The promise in the guide: a sandboxed plugin can't use the kernel's ways round its
    /// sandbox, and everything an ordinary program does still works.
    #[test]
    // `Ioctl` is already a u64 with glibc, and an i32 with musl.
    #[allow(clippy::unnecessary_cast)]
    fn the_filter_refuses_the_ways_round_the_sandbox_and_nothing_ordinary() {
        for allowed in [
            libc::SYS_read,
            libc::SYS_write,
            libc::SYS_openat,
            libc::SYS_clone,
            libc::SYS_execve,
            libc::SYS_mmap,
            libc::SYS_connect,
            libc::SYS_inotify_add_watch,
        ] {
            assert_eq!(call(allowed, NONE), RET_ALLOW, "{allowed}");
        }
        for refused in refused() {
            assert_eq!(call(refused, NONE), EPERM, "{refused}");
        }
        assert_eq!(
            call(libc::SYS_io_uring_setup, NONE),
            RET_ERRNO | libc::ENOSYS as u32
        );
        // Terminals: an ioctl that types into one is refused, any other isn't.
        let ioctl = |request: u64| call(libc::SYS_ioctl, [1, request, 0, 0, 0, 0]);
        assert_eq!(ioctl(libc::TIOCSTI as u64), EPERM);
        assert_eq!(ioctl(libc::TIOCLINUX as u64), EPERM);
        assert_eq!(ioctl(libc::TIOCGWINSZ as u64), RET_ALLOW);
        // The request's high half doesn't hide it: the kernel only reads the low one.
        assert_eq!(ioctl((1 << 32) | libc::TIOCSTI as u64), EPERM);
        // Sockets: local and internet ones only.
        let socket = |family: i32| call(libc::SYS_socket, [family as u64, 1, 0, 0, 0, 0]);
        for family in FAMILIES {
            assert_eq!(socket(family), RET_ALLOW, "{family}");
        }
        for family in [
            libc::AF_PACKET,
            libc::AF_BLUETOOTH,
            libc::AF_ALG,
            libc::AF_VSOCK,
        ] {
            assert_eq!(
                socket(family),
                RET_ERRNO | libc::EAFNOSUPPORT as u32,
                "{family}"
            );
        }
        // Another architecture's calls, and x32's, are refused whatever they are.
        assert_eq!(
            run(&instructions().unwrap(), 0x4000_0003, libc::SYS_read, NONE),
            EPERM
        );
        #[cfg(target_arch = "x86_64")]
        assert_eq!(call(0x4000_0000 | libc::SYS_read, NONE), EPERM);
    }

    #[test]
    fn the_filter_is_whole_instructions() {
        let program = program().unwrap();
        assert_eq!(program.len(), instructions().unwrap().len() * 8);
        assert!(instructions().unwrap().len() < 4096, "the kernel's limit");
    }
}
