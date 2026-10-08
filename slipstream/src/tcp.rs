//! How many bytes each TCP connection on the machine has carried, and which connections a
//! process holds.
//!
//! The kernel answers the first for any user over its socket diagnostics (the netlink family `ss`
//! reads), naming each connection by its socket's inode; a process's own `/proc/<pid>/fd` gives
//! the inodes it holds. Put together they are an app's network traffic with no privileges and no
//! packet capture. Only TCP is counted this way: the kernel keeps no such totals for UDP.

use std::{
    collections::HashMap,
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_DUMP: u16 = 0x300;
/// The attribute carrying the kernel's `struct tcp_info`.
const INET_DIAG_INFO: u16 = 2;

/// Sizes of `struct nlmsghdr`, `struct inet_diag_req_v2` and `struct inet_diag_msg`.
const HEADER: usize = 16;
const REQUEST: usize = 56;
const MESSAGE: usize = 72;
/// Where `idiag_inode` sits in `inet_diag_msg`, and the two totals in `tcp_info`.
const INODE_AT: usize = 68;
const BYTES_ACKED_AT: usize = 120;
const BYTES_RECEIVED_AT: usize = 128;

/// Connections that can be carrying anything: not listening, closed, or waiting out their last
/// packets. The bits are the kernel's TCP states.
const CARRYING: u32 = 0xfff & !(1 << 10 | 1 << 7 | 1 << 6 | 1 << 3);

/// Bytes sent and received so far by every TCP connection, by socket inode. Empty where the
/// kernel won't say (a container without the diagnostics module, say), and empty rather than
/// partial if it stops part way: a connection missing from one answer and back in the next
/// would look as if it had carried its whole life's traffic in between.
pub fn bytes_by_inode() -> HashMap<u64, u64> {
    let mut table = HashMap::new();
    let Ok(socket) = open() else {
        return table;
    };
    for family in [libc::AF_INET, libc::AF_INET6] {
        if dump(&socket, family as u8, &mut table).is_err() {
            return HashMap::new();
        }
    }
    table
}

/// The inodes of the sockets `pid` holds open. Nothing for a process that can't be looked into.
pub fn sockets_of(pid: u32) -> impl Iterator<Item = u64> {
    fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| socket_inode(fs::read_link(entry.path()).ok()?.to_str()?))
}

/// The inode in an open socket's `/proc` link, `socket:[12345]`.
fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

fn open() -> io::Result<OwnedFd> {
    // SAFETY: plain socket calls; the descriptor is owned from the moment it exists.
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_SOCK_DIAG,
        );
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let socket = OwnedFd::from_raw_fd(fd);
        // The kernel answers at once or not at all; never wait on it.
        let wait = libc::timeval {
            tv_sec: 0,
            tv_usec: 200_000,
        };
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&raw const wait).cast(),
            size_of::<libc::timeval>() as libc::socklen_t,
        );
        Ok(socket)
    }
}

/// Asks for every carrying connection of one address family and adds their totals to `table`.
fn dump(socket: &OwnedFd, family: u8, table: &mut HashMap<u64, u64>) -> io::Result<()> {
    let mut request = [0u8; HEADER + REQUEST];
    request[0..4].copy_from_slice(&((HEADER + REQUEST) as u32).to_ne_bytes());
    request[4..6].copy_from_slice(&SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    request[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_DUMP).to_ne_bytes());
    request[HEADER] = family;
    request[HEADER + 1] = libc::IPPROTO_TCP as u8;
    request[HEADER + 2] = 1 << (INET_DIAG_INFO - 1);
    request[HEADER + 4..HEADER + 8].copy_from_slice(&CARRYING.to_ne_bytes());
    // SAFETY: the buffer is ours and its length is passed with it.
    let sent = unsafe {
        libc::send(
            socket.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            0,
        )
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut buffer = vec![0u8; 32 * 1024];
    // A machine has thousands of connections at the very most; this many answers is a kernel
    // that isn't going to finish.
    for _ in 0..1024 {
        // SAFETY: as above.
        let got = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        if got < 0 {
            return Err(io::Error::last_os_error());
        }
        if read_answer(&buffer[..got as usize], table)? {
            return Ok(());
        }
    }
    Err(io::ErrorKind::TimedOut.into())
}

/// Reads one datagram of answers. True once the kernel says the list is finished.
fn read_answer(mut data: &[u8], table: &mut HashMap<u64, u64>) -> io::Result<bool> {
    while data.len() >= HEADER {
        let length = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let kind = u16::from_ne_bytes([data[4], data[5]]);
        if length < HEADER || length > data.len() {
            break;
        }
        match kind {
            NLMSG_DONE => return Ok(true),
            NLMSG_ERROR => return Err(io::ErrorKind::Unsupported.into()),
            SOCK_DIAG_BY_FAMILY => {
                if let Some((inode, bytes)) = connection(&data[HEADER..length]) {
                    table.insert(inode, bytes);
                }
            }
            _ => {}
        }
        data = &data[align(length).min(data.len())..];
    }
    Ok(false)
}

/// One connection's inode and the bytes it has carried both ways, from its `inet_diag_msg` and
/// the attributes after it.
fn connection(message: &[u8]) -> Option<(u64, u64)> {
    let inode = u32::from_ne_bytes(message.get(INODE_AT..INODE_AT + 4)?.try_into().ok()?);
    let mut attributes = message.get(MESSAGE..)?;
    while attributes.len() >= 4 {
        let length = u16::from_ne_bytes([attributes[0], attributes[1]]) as usize;
        let kind = u16::from_ne_bytes([attributes[2], attributes[3]]);
        if length < 4 || length > attributes.len() {
            break;
        }
        if kind == INET_DIAG_INFO {
            let info = &attributes[4..length];
            let total = |at: usize| -> Option<u64> {
                Some(u64::from_ne_bytes(info.get(at..at + 8)?.try_into().ok()?))
            };
            let bytes = total(BYTES_ACKED_AT)?.saturating_add(total(BYTES_RECEIVED_AT)?);
            // A socket no process holds any more has no inode to find it by.
            return (inode != 0).then_some((u64::from(inode), bytes));
        }
        attributes = &attributes[align(length).min(attributes.len())..];
    }
    None
}

/// Netlink pads every message and attribute to four bytes.
fn align(length: usize) -> usize {
    (length + 3) & !3
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
    };

    #[test]
    fn a_socket_link_gives_its_inode() {
        assert_eq!(socket_inode("socket:[48213]"), Some(48213));
        assert_eq!(socket_inode("pipe:[48213]"), None);
        assert_eq!(socket_inode("/dev/pts/3"), None);
        assert_eq!(socket_inode("socket:[]"), None);
    }

    #[test]
    fn an_answer_is_read_up_to_its_end() {
        // One connection with a tcp_info long enough to hold both totals, then the end marker.
        let mut info = vec![0u8; 136];
        info[BYTES_ACKED_AT..BYTES_ACKED_AT + 8].copy_from_slice(&700u64.to_ne_bytes());
        info[BYTES_RECEIVED_AT..BYTES_RECEIVED_AT + 8].copy_from_slice(&300u64.to_ne_bytes());
        let mut message = vec![0u8; MESSAGE];
        message[INODE_AT..INODE_AT + 4].copy_from_slice(&4242u32.to_ne_bytes());
        message.extend(((4 + info.len()) as u16).to_ne_bytes());
        message.extend(INET_DIAG_INFO.to_ne_bytes());
        message.extend(&info);
        let mut data = Vec::new();
        data.extend(((HEADER + message.len()) as u32).to_ne_bytes());
        data.extend(SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        data.extend([0u8; 10]);
        data.extend(&message);
        let mut table = HashMap::new();
        assert!(!read_answer(&data, &mut table).unwrap());
        assert_eq!(table.get(&4242), Some(&1000));

        data.extend((HEADER as u32).to_ne_bytes());
        data.extend(NLMSG_DONE.to_ne_bytes());
        data.extend([0u8; 10]);
        assert!(read_answer(&data, &mut HashMap::new()).unwrap());
    }

    #[test]
    fn a_truncated_answer_is_left_alone() {
        let mut table = HashMap::new();
        assert!(!read_answer(&[0u8; 7], &mut table).unwrap());
        // A length that runs past the data.
        let mut data = Vec::new();
        data.extend(4096u32.to_ne_bytes());
        data.extend(SOCK_DIAG_BY_FAMILY.to_ne_bytes());
        data.extend([0u8; 10]);
        assert!(!read_answer(&data, &mut table).unwrap());
        assert!(table.is_empty());
    }

    #[test]
    fn a_connection_of_our_own_is_counted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut sending = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut receiving, _) = listener.accept().unwrap();
        sending.write_all(&[7u8; 10_000]).unwrap();
        let mut taken = [0u8; 10_000];
        receiving.read_exact(&mut taken).unwrap();

        let inode = fs::read_link(format!("/proc/self/fd/{}", receiving.as_raw_fd()))
            .ok()
            .and_then(|link| socket_inode(link.to_str()?))
            .unwrap();
        assert!(sockets_of(std::process::id()).any(|held| held == inode));
        // Where the kernel won't list connections there is nothing to check the total against.
        if let Some(bytes) = bytes_by_inode().get(&inode) {
            assert!(*bytes >= 10_000, "{bytes} bytes for 10,000 received");
        }
    }
}
