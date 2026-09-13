//! Checking the password that unlocks the lock screen, through PAM, in a process of its own.
//!
//! The compositor never calls PAM itself. It starts its own executable again with
//! `--authenticate`, writes the password and a newline to the child's stdin and closes it, and
//! reads the answer from the exit status: 0 authenticated, 1 refused, anything else unknown. PAM
//! modules can block for seconds (a deliberate delay after a failure, a network directory), load
//! other libraries, and fork helpers such as `unix_chkpwd`; none of that belongs on the event loop,
//! and libpam is loaded only into the helper.
//!
//! The helper checks the user it runs as, found from `getuid`, never a name from its input. A
//! non-root caller can check its own password because `pam_unix` asks its setuid `unix_chkpwd`.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use zeroize::{Zeroize, Zeroizing};

/// The flag that runs the helper instead of a compositor.
pub const FLAG: &str = "--authenticate";
/// The PAM service the helper asks: `/etc/pam.d/slipstream`.
const SERVICE: &CStr = c"slipstream";
/// The longest line the helper reads, in bytes. The lock screen itself stops at half this.
const LINE_LIMIT: usize = 1024;
/// How long a check may take before the helper is killed and the lock says it couldn't check.
pub const CHECK_LIMIT: Duration = Duration::from_secs(30);

/// The helper's exit statuses.
pub const AUTHENTICATED: i32 = 0;
pub const REFUSED: i32 = 1;
pub const UNAVAILABLE: i32 = 2;

// Linux-PAM's constants, from `security/_pam_types.h`.
const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_NEW_AUTHTOK_REQD: c_int = 12;
const PAM_AUTH_ERR: c_int = 7;
const PAM_CRED_INSUFFICIENT: c_int = 8;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_MAXTRIES: c_int = 11;
const PAM_ACCT_EXPIRED: c_int = 13;
const PAM_PERM_DENIED: c_int = 6;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type Conversation = unsafe extern "C" fn(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Conversation,
    appdata_ptr: *mut c_void,
}

/// `pam_start`'s signature, and that of the calls on a handle.
type PamStart =
    unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type PamCall = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;

/// The four functions the helper needs, looked up in `libpam.so.0` at run time.
struct Pam {
    start: PamStart,
    authenticate: PamCall,
    acct_mgmt: PamCall,
    end: PamCall,
}

impl Pam {
    /// libpam, or `None` if it isn't installed or lacks a function. The library stays loaded until
    /// the helper exits.
    fn load() -> Option<Self> {
        // SAFETY: dlopen and dlsym with NUL-terminated names; each symbol is transmuted to the
        // signature Linux-PAM declares for it.
        unsafe {
            let library = libc::dlopen(c"libpam.so.0".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
            if library.is_null() {
                return None;
            }
            let symbol = |name: &CStr| {
                let found = libc::dlsym(library, name.as_ptr());
                (!found.is_null()).then_some(found)
            };
            Some(Self {
                start: std::mem::transmute::<*mut c_void, PamStart>(symbol(c"pam_start")?),
                authenticate: std::mem::transmute::<*mut c_void, PamCall>(symbol(
                    c"pam_authenticate",
                )?),
                acct_mgmt: std::mem::transmute::<*mut c_void, PamCall>(symbol(c"pam_acct_mgmt")?),
                end: std::mem::transmute::<*mut c_void, PamCall>(symbol(c"pam_end")?),
            })
        }
    }
}

/// The helper: reads the password from stdin, asks PAM, and returns the exit status. Messages PAM
/// has for the person (a locked account, say) go to stdout, one to a line.
pub fn authenticate() -> i32 {
    // A core dump of this process could hold the password.
    // SAFETY: prctl with PR_SET_DUMPABLE and a plain integer.
    unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0) };
    let Some(password) = read_line(&mut Stdin) else {
        return UNAVAILABLE;
    };
    if password.is_empty() {
        return REFUSED;
    }
    let Some(user) = user_name() else {
        return UNAVAILABLE;
    };
    let Some(pam) = Pam::load() else {
        return UNAVAILABLE;
    };
    let status = check(&pam, &user, &password);
    let _ = std::io::stdout().flush();
    status
}

/// One line from `input`, without its newline, into memory that is wiped when dropped: `None`
/// past `LINE_LIMIT` bytes, on a read error, or with a NUL byte in it, which PAM's C strings
/// can't carry. Read a byte at a time straight into the buffer, so no reader keeps a copy.
fn read_line(input: &mut impl Read) -> Option<Zeroizing<Vec<u8>>> {
    let mut line = Zeroizing::new(Vec::with_capacity(LINE_LIMIT + 1));
    let mut byte = [0u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) if byte[0] == 0 || line.len() == LINE_LIMIT => {
                byte.zeroize();
                return None;
            }
            Ok(_) => line.push(byte[0]),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                byte.zeroize();
                return None;
            }
        }
    }
    byte.zeroize();
    Some(line)
}

/// Standard input read straight from its descriptor. `std::io::stdin` would copy what it reads
/// into a buffer of its own that nothing wipes.
struct Stdin;

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // SAFETY: reads at most `buf.len()` bytes into `buf`.
        let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

/// The name of the user this process runs as, from the password database.
fn user_name() -> Option<CString> {
    // SAFETY: getpwuid_r writes into `entry` and `buffer`, both valid for the sizes given, and
    // `found` points at `entry` only on success.
    unsafe {
        let uid = libc::getuid();
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut buffer = vec![0 as c_char; 16 * 1024];
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let failed = libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        );
        if failed != 0 || found.is_null() || entry.pw_name.is_null() {
            return None;
        }
        Some(CStr::from_ptr(entry.pw_name).to_owned())
    }
}

/// `pam_start`, `pam_authenticate`, `pam_acct_mgmt` and `pam_end`, as an exit status.
fn check(pam: &Pam, user: &CStr, password: &Zeroizing<Vec<u8>>) -> i32 {
    let conversation = PamConv {
        conv: converse,
        appdata_ptr: (password as *const Zeroizing<Vec<u8>>).cast_mut().cast(),
    };
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: the service and user are C strings, and `conversation` and the password it points
    // at outlive the handle, which `pam_end` releases before this returns.
    unsafe {
        if (pam.start)(SERVICE.as_ptr(), user.as_ptr(), &conversation, &mut handle) != PAM_SUCCESS
            || handle.is_null()
        {
            return UNAVAILABLE;
        }
        let authenticated = (pam.authenticate)(handle, 0);
        let (result, status) = if authenticated == PAM_SUCCESS {
            let account = (pam.acct_mgmt)(handle, 0);
            (account, status_for_account(account))
        } else {
            (authenticated, status_for_authenticate(authenticated))
        };
        (pam.end)(handle, result);
        status
    }
}

/// The exit status for `pam_authenticate`'s answer, when it isn't success. Only the account check
/// may say the password needs changing: a module that says so while authenticating would
/// otherwise unlock with no account check at all (an expired account, `pam_nologin`).
fn status_for_authenticate(result: c_int) -> i32 {
    match result {
        PAM_NEW_AUTHTOK_REQD => REFUSED,
        other => status_for(other),
    }
}

/// The exit status for `pam_acct_mgmt`'s answer, after a successful authentication. An expired
/// password still proves who is at the keyboard, and a lock screen has nowhere to change it, so it
/// unlocks, as other desktops' lockers do.
fn status_for_account(result: c_int) -> i32 {
    match result {
        PAM_NEW_AUTHTOK_REQD => AUTHENTICATED,
        other => status_for(other),
    }
}

/// The exit status for any other answer from PAM.
fn status_for(result: c_int) -> i32 {
    match result {
        PAM_SUCCESS => AUTHENTICATED,
        PAM_AUTH_ERR
        | PAM_CRED_INSUFFICIENT
        | PAM_USER_UNKNOWN
        | PAM_MAXTRIES
        | PAM_ACCT_EXPIRED
        | PAM_PERM_DENIED => REFUSED,
        _ => UNAVAILABLE,
    }
}

/// PAM's conversation. A password prompt (echo off) is answered with the password; a prompt that
/// would show what is typed (echo on) is refused, since nothing but the password was asked for;
/// messages are printed to stdout for the lock screen to show. Anything else is refused.
///
/// # Safety
/// Called by PAM with `num_msg` valid message pointers and `appdata_ptr` the password passed to
/// `pam_start`. Responses are allocated with `malloc`, as PAM frees them with `free`.
unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
        return PAM_CONV_ERR;
    }
    let count = num_msg as usize;
    // SAFETY: as the function's contract says.
    unsafe {
        let password = &*(appdata_ptr as *const Zeroizing<Vec<u8>>);
        let replies = libc::calloc(count, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if replies.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..count {
            let message = *msg.add(i);
            if message.is_null() {
                free_replies(replies, count);
                return PAM_CONV_ERR;
            }
            match (*message).msg_style {
                PAM_PROMPT_ECHO_OFF => {
                    let copy = libc::malloc(password.len() + 1) as *mut u8;
                    if copy.is_null() {
                        free_replies(replies, count);
                        return PAM_BUF_ERR;
                    }
                    std::ptr::copy_nonoverlapping(password.as_ptr(), copy, password.len());
                    *copy.add(password.len()) = 0;
                    (*replies.add(i)).resp = copy.cast();
                }
                PAM_ERROR_MSG | PAM_TEXT_INFO => {
                    if !(*message).msg.is_null() {
                        say(CStr::from_ptr((*message).msg));
                    }
                }
                PAM_PROMPT_ECHO_ON => {
                    free_replies(replies, count);
                    return PAM_CONV_ERR;
                }
                _ => {
                    free_replies(replies, count);
                    return PAM_CONV_ERR;
                }
            }
        }
        *resp = replies;
        PAM_SUCCESS
    }
}

/// Wipes and frees the replies made so far, when the conversation gives up part way.
///
/// # Safety
/// `replies` is a `calloc`ed array of `count` responses, each `resp` null or a `malloc`ed C
/// string.
unsafe fn free_replies(replies: *mut PamResponse, count: usize) {
    // SAFETY: as the function's contract says.
    unsafe {
        for i in 0..count {
            let reply = (*replies.add(i)).resp;
            if !reply.is_null() {
                let length = libc::strlen(reply);
                std::slice::from_raw_parts_mut(reply.cast::<u8>(), length).zeroize();
                libc::free(reply.cast());
            }
        }
        libc::free(replies.cast());
    }
}

/// A message from PAM, on one line of stdout.
fn say(message: &CStr) {
    let text = one_line(&message.to_string_lossy());
    if !text.is_empty() {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
    }
}

/// Text made safe to show as one short line: control characters become spaces, runs of space
/// collapse, and it stops at 160 characters.
pub fn one_line(text: &str) -> String {
    let spaced: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

/// What checking a password came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Unlock,
    /// Refused, with the first thing PAM said, if it said anything.
    Refused(Option<String>),
    /// The helper crashed, hung, was missing, or couldn't reach PAM.
    Failed,
}

impl Verdict {
    /// The verdict for a helper that exited with `code` (`None`: killed by a signal) after
    /// printing `output`.
    pub fn from_exit(code: Option<i32>, output: &[u8]) -> Self {
        match code {
            Some(AUTHENTICATED) => Self::Unlock,
            Some(REFUSED) => Self::Refused(
                String::from_utf8_lossy(output)
                    .lines()
                    .map(one_line)
                    .find(|line| !line.is_empty()),
            ),
            _ => Self::Failed,
        }
    }
}

/// Starts the helper on `password` and returns at once; `reply` is called from a worker thread
/// with the verdict, exactly once, whatever happens to the helper or the worker.
pub fn start_check(password: &[u8], reply: impl FnOnce(Verdict) + Send + 'static) {
    let reply = Reply(Some(Box::new(reply)));
    // The running build, even after a deploy has replaced the file on disk.
    // A clean environment: nothing the session was started with, `LD_PRELOAD` say, reaches the
    // process that handles the password. PAM's messages still come in the session's language.
    let mut command = crate::launch::command("/proc/self/exe");
    command.env_clear();
    for name in ["PATH", "LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    let spawned = command
        .arg(FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!("couldn't start the password check: {err}");
            reply.send(Verdict::Failed);
            return;
        }
    };
    // Into a pipe with room for far more than the longest password, so this never blocks.
    let written = child.stdin.take().map(|mut stdin| {
        stdin
            .write_all(password)
            .and_then(|()| stdin.write_all(b"\n"))
    });
    if !matches!(written, Some(Ok(()))) {
        tracing::warn!("couldn't hand the password to its check");
        let _ = child.kill();
        let _ = child.wait();
        reply.send(Verdict::Failed);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("lock-check".into())
        .spawn(move || {
            let verdict = wait(child);
            reply.send(verdict);
        });
    if let Err(err) = spawned {
        tracing::warn!("couldn't wait for the password check: {err}");
    }
}

/// Waits for the helper, killing it after `CHECK_LIMIT`.
fn wait(mut child: Child) -> Verdict {
    let mut stdout = child.stdout.take();
    // Read without blocking: a process the helper started may hold the pipe open after it exits.
    if let Some(out) = stdout.as_ref() {
        // SAFETY: fcntl on a descriptor this function owns.
        unsafe {
            let fd = out.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let started = Instant::now();
    let mut output = Vec::new();
    let status = loop {
        if let Some(out) = stdout.as_mut() {
            read_available(out, &mut output);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= CHECK_LIMIT => {
                tracing::warn!("the password check took too long; stopped it");
                let _ = child.kill();
                let _ = child.wait();
                return Verdict::Failed;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => {
                tracing::warn!("couldn't wait for the password check: {err}");
                return Verdict::Failed;
            }
        }
    };
    if let Some(out) = stdout.as_mut() {
        read_available(out, &mut output);
    }
    Verdict::from_exit(status.code(), &output)
}

/// Whatever is waiting in the pipe, up to 4 KiB in all.
fn read_available(out: &mut impl Read, output: &mut Vec<u8>) {
    let mut chunk = [0u8; 512];
    while output.len() < 4096 {
        match out.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => output.extend_from_slice(&chunk[..n]),
        }
    }
}

/// Sends the verdict once. Dropped without sending (the worker panicked), it sends `Failed`, so
/// the lock screen never waits on a check that will never answer.
struct Reply(Option<Box<dyn FnOnce(Verdict) + Send>>);

impl Reply {
    fn send(mut self, verdict: Verdict) {
        if let Some(reply) = self.0.take() {
            reply(verdict);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            reply(Verdict::Failed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the conversation over `styles`, returning PAM's status and each reply as text.
    fn converse_with(styles: &[c_int], password: &[u8]) -> (c_int, Vec<Option<String>>) {
        let texts: Vec<CString> = (0..styles.len())
            .map(|i| CString::new(format!("message {i}")).unwrap())
            .collect();
        let messages: Vec<PamMessage> = styles
            .iter()
            .zip(&texts)
            .map(|(style, text)| PamMessage {
                msg_style: *style,
                msg: text.as_ptr(),
            })
            .collect();
        let mut pointers: Vec<*const PamMessage> =
            messages.iter().map(|message| message as *const _).collect();
        let password = Zeroizing::new(password.to_vec());
        let mut replies: *mut PamResponse = std::ptr::null_mut();
        let status = unsafe {
            converse(
                styles.len() as c_int,
                pointers.as_mut_ptr(),
                &mut replies,
                (&password as *const Zeroizing<Vec<u8>>).cast_mut().cast(),
            )
        };
        let mut answers = Vec::new();
        if status == PAM_SUCCESS {
            unsafe {
                for i in 0..styles.len() {
                    let reply = (*replies.add(i)).resp;
                    answers.push((!reply.is_null()).then(|| {
                        let text = CStr::from_ptr(reply).to_string_lossy().into_owned();
                        libc::free(reply.cast());
                        text
                    }));
                }
                libc::free(replies.cast());
            }
        }
        (status, answers)
    }

    #[test]
    fn the_conversation_answers_only_echo_off() {
        let (status, answers) =
            converse_with(&[PAM_TEXT_INFO, PAM_PROMPT_ECHO_OFF], b"correct horse");
        assert_eq!(status, PAM_SUCCESS);
        assert_eq!(answers, [None, Some("correct horse".to_string())]);

        let (status, answers) = converse_with(&[PAM_PROMPT_ECHO_OFF, PAM_PROMPT_ECHO_ON], b"x");
        assert_eq!(status, PAM_CONV_ERR, "a visible prompt is refused");
        assert!(answers.is_empty());

        let (status, _) = converse_with(&[PAM_ERROR_MSG, 7], b"x");
        assert_eq!(status, PAM_CONV_ERR, "so is a binary prompt");
    }

    #[test]
    fn the_helper_reads_one_bounded_line() {
        let mut input: &[u8] = b"hunter2\nignored";
        assert_eq!(read_line(&mut input).unwrap().as_slice(), b"hunter2");
        let mut empty: &[u8] = b"";
        assert!(read_line(&mut empty).unwrap().is_empty());
        let long = vec![b'a'; LINE_LIMIT + 1];
        assert!(read_line(&mut long.as_slice()).is_none(), "too long");
        let fits = [vec![b'a'; LINE_LIMIT], b"\n".to_vec()].concat();
        assert_eq!(read_line(&mut fits.as_slice()).unwrap().len(), LINE_LIMIT);
        let mut nul: &[u8] = b"a\0b\n";
        assert!(read_line(&mut nul).is_none(), "PAM can't take a NUL");
    }

    #[test]
    fn exit_statuses_become_verdicts() {
        assert_eq!(Verdict::from_exit(Some(0), b"Welcome\n"), Verdict::Unlock);
        assert_eq!(
            Verdict::from_exit(Some(1), b"\n  The account is locked.\x07\nmore\n"),
            Verdict::Refused(Some("The account is locked.".into()))
        );
        assert_eq!(Verdict::from_exit(Some(1), b""), Verdict::Refused(None));
        assert_eq!(Verdict::from_exit(Some(2), b""), Verdict::Failed);
        assert_eq!(Verdict::from_exit(None, b""), Verdict::Failed, "killed");
        assert_eq!(status_for(PAM_SUCCESS), AUTHENTICATED);
        assert_eq!(status_for(PAM_AUTH_ERR), REFUSED);
        assert_eq!(status_for(PAM_CONV_ERR), UNAVAILABLE);
    }

    #[test]
    fn only_the_account_check_may_ask_for_a_new_password() {
        assert_eq!(status_for_authenticate(PAM_NEW_AUTHTOK_REQD), REFUSED);
        assert_eq!(status_for_authenticate(PAM_AUTH_ERR), REFUSED);
        assert_eq!(status_for_account(PAM_NEW_AUTHTOK_REQD), AUTHENTICATED);
        assert_eq!(status_for_account(PAM_SUCCESS), AUTHENTICATED);
        assert_eq!(status_for_account(PAM_ACCT_EXPIRED), REFUSED);
    }

    #[test]
    fn a_worker_that_never_answers_still_fails_the_check() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let reply = Reply(Some(Box::new(move |verdict| {
            sender.send(verdict).unwrap();
        })));
        drop(reply);
        assert_eq!(receiver.try_recv(), Ok(Verdict::Failed));
    }
}
