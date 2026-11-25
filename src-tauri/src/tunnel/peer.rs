//! Who is on the other end of a connection to the tunnel's local port. TablePlus's tunnel "only
//! accepts connections from TablePlus"; Tabler's does the same: every accepted connection must
//! come from a socket of this process (the database drivers) or of one of its child processes
//! (`pg_dump` / `mysqldump` of Backup / Restore). Anything else on the machine is refused before a
//! byte is forwarded.
//!
//! The listener is TCP on 127.0.0.1, where macOS has no peer credentials (`LOCAL_PEERPID` only
//! exists for Unix-domain sockets). So the check looks the connection up from the client side:
//! the accepted socket's peer address is the client socket's local address, and libproc
//! (`proc_pidinfo` / `proc_pidfdinfo`) lists the TCP sockets of this process and its descendants
//! with their addresses. A connection is ours when one of those sockets has exactly that
//! (local, remote) address pair.
//!
//! On Linux the same pair is looked up in `/proc/net/tcp` for the client socket's inode, which
//! must be an open descriptor of this process or a descendant (`/proc/<pid>/fd`).
//!
//! When the lookup itself cannot run (libproc refuses to list this very process, `/proc` is not
//! readable), the connection is let through with a warning: the check must never break the app's
//! own tunnel. On other platforms (Windows) the check is not implemented and every local
//! connection is accepted.

use std::net::SocketAddr;

/// Whose socket a local connection comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerOwner {
    ThisProcess,
    /// A descendant process (pid).
    Child(i32),
    /// Some other program: refused.
    Other,
    /// The lookup could not run (or is not implemented here): accepted.
    Unknown,
}

impl PeerOwner {
    pub fn allowed(self) -> bool {
        !matches!(self, PeerOwner::Other)
    }
}

/// Finds the owner of the client socket of an accepted connection. `peer` is the accepted
/// socket's peer address (the client's local address), `local` its local address (the
/// listener's).
pub fn owner(peer: SocketAddr, local: SocketAddr) -> PeerOwner {
    imp::owner(peer, local)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::collections::{HashSet, VecDeque};
    use std::ffi::c_void;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use super::PeerOwner;

    /// `sizeof(struct socket_fdinfo)` and the offsets used below, from `<sys/proc_info.h>`
    /// (checked against the macOS SDK with a C program; stable across macOS versions, as lsof
    /// and Activity Monitor depend on them).
    const SOCKET_FDINFO_SIZE: usize = 792;
    const OFF_SOI_FAMILY: usize = 184;
    const OFF_SOI_KIND: usize = 256;
    const OFF_INSI_FPORT: usize = 264;
    const OFF_INSI_LPORT: usize = 268;
    const OFF_INSI_VFLAG: usize = 288;
    /// `insi_faddr` / `insi_laddr` are `in4in6_addr` unions: the IPv4 address follows 12 bytes
    /// of padding.
    const OFF_INSI_FADDR4: usize = 296 + 12;
    const OFF_INSI_LADDR4: usize = 312 + 12;
    const PROX_FDTYPE_SOCKET: u32 = 2;
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    const SOCKINFO_IN: i32 = 1;
    const SOCKINFO_TCP: i32 = 2;
    const INI_IPV4: u8 = 0x1;
    const AF_INET: i32 = 2;
    const AF_INET6: i32 = 30;
    /// How far down the process tree the check looks, and how many processes at most.
    const MAX_DEPTH: usize = 4;
    const MAX_PROCESSES: usize = 256;

    /// A TCP/IPv4 socket's address pair as libproc reports it.
    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct SocketPair {
        pub local: SocketAddr,
        pub remote: SocketAddr,
    }

    fn read_i32(buf: &[u8], at: usize) -> i32 {
        i32::from_ne_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
    }

    fn read_v4(buf: &[u8], at: usize) -> Ipv4Addr {
        Ipv4Addr::new(buf[at], buf[at + 1], buf[at + 2], buf[at + 3])
    }

    /// The IPv4 address pair of a `socket_fdinfo` buffer, if it is a TCP or IP socket.
    pub(super) fn parse_socket_fdinfo(buf: &[u8]) -> Option<SocketPair> {
        if buf.len() < SOCKET_FDINFO_SIZE {
            return None;
        }
        let family = read_i32(buf, OFF_SOI_FAMILY);
        let kind = read_i32(buf, OFF_SOI_KIND);
        if !(family == AF_INET || family == AF_INET6)
            || !(kind == SOCKINFO_TCP || kind == SOCKINFO_IN)
        {
            return None;
        }
        if buf[OFF_INSI_VFLAG] & INI_IPV4 == 0 {
            return None;
        }
        // Ports are kept in network byte order in the low 16 bits.
        let port = |at| u16::from_be(read_i32(buf, at) as u16);
        Some(SocketPair {
            local: SocketAddr::new(
                IpAddr::V4(read_v4(buf, OFF_INSI_LADDR4)),
                port(OFF_INSI_LPORT),
            ),
            remote: SocketAddr::new(
                IpAddr::V4(read_v4(buf, OFF_INSI_FADDR4)),
                port(OFF_INSI_FPORT),
            ),
        })
    }

    /// The TCP/IPv4 sockets of `pid`; `None` when libproc does not list its descriptors.
    pub(super) fn sockets_of(pid: i32) -> Option<Vec<SocketPair>> {
        let entry = std::mem::size_of::<libc::proc_fdinfo>();
        // SAFETY: a size query (null buffer) as documented for PROC_PIDLISTFDS.
        let needed =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return None;
        }
        // Room for descriptors opened between the two calls.
        let capacity = needed as usize / entry + 32;
        let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(capacity);
        // SAFETY: the buffer holds `capacity` entries; libproc writes at most `buffersize` bytes
        // and returns how many it wrote.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast::<c_void>(),
                (capacity * entry) as i32,
            )
        };
        if written <= 0 {
            return None;
        }
        // SAFETY: libproc initialised `written` bytes, i.e. that many whole entries.
        unsafe { fds.set_len((written as usize / entry).min(capacity)) };
        let mut out = Vec::new();
        let mut buf = vec![0u8; SOCKET_FDINFO_SIZE];
        for fd in fds.iter().filter(|f| f.proc_fdtype == PROX_FDTYPE_SOCKET) {
            buf.fill(0);
            // SAFETY: `buf` is SOCKET_FDINFO_SIZE bytes, the size passed in.
            let got = unsafe {
                libc::proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDSOCKETINFO,
                    buf.as_mut_ptr().cast::<c_void>(),
                    SOCKET_FDINFO_SIZE as i32,
                )
            };
            if got as usize >= SOCKET_FDINFO_SIZE
                && let Some(pair) = parse_socket_fdinfo(&buf)
            {
                out.push(pair);
            }
        }
        Some(out)
    }

    /// The direct children of `pid`.
    fn children_of(pid: i32) -> Vec<i32> {
        let mut pids = vec![0i32; MAX_PROCESSES];
        // SAFETY: the buffer holds MAX_PROCESSES pids and its byte size is passed in.
        let n = unsafe {
            libc::proc_listchildpids(
                pid,
                pids.as_mut_ptr().cast::<c_void>(),
                (MAX_PROCESSES * std::mem::size_of::<i32>()) as i32,
            )
        };
        if n <= 0 {
            return Vec::new();
        }
        pids.truncate((n as usize).min(MAX_PROCESSES));
        pids.retain(|p| *p > 0 && *p != pid);
        pids
    }

    fn has_pair(sockets: &[SocketPair], peer: SocketAddr, local: SocketAddr) -> bool {
        sockets.iter().any(|s| s.local == peer && s.remote == local)
    }

    pub(super) fn owner(peer: SocketAddr, local: SocketAddr) -> PeerOwner {
        let me = std::process::id() as i32;
        match sockets_of(me) {
            Some(own) if has_pair(&own, peer, local) => return PeerOwner::ThisProcess,
            Some(_) => {}
            None => return PeerOwner::Unknown,
        }
        // Breadth-first through the descendants (Backup's dump binaries, possibly under a shell).
        let mut seen = HashSet::from([me]);
        let mut queue: VecDeque<(i32, usize)> =
            children_of(me).into_iter().map(|p| (p, 1)).collect();
        while let Some((pid, depth)) = queue.pop_front() {
            if !seen.insert(pid) || seen.len() > MAX_PROCESSES {
                continue;
            }
            if sockets_of(pid).is_some_and(|s| has_pair(&s, peer, local)) {
                return PeerOwner::Child(pid);
            }
            if depth < MAX_DEPTH {
                queue.extend(children_of(pid).into_iter().map(|p| (p, depth + 1)));
            }
        }
        PeerOwner::Other
    }
}

/// Linux: the client socket's inode comes from `/proc/net/tcp` (or `tcp6`), keyed by the
/// (local, remote) address pair; the connection is ours when that inode is among the open file
/// descriptors (`/proc/<pid>/fd`, `socket:[inode]`) of this process or a descendant.
#[cfg(target_os = "linux")]
mod imp {
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    use super::PeerOwner;

    const MAX_PROCESSES: usize = 512;
    const MAX_DEPTH: usize = 4;

    /// One `/proc/net/tcp{,6}` address, `0100007F:1F90`: the IP as the kernel's raw words in
    /// hex, the port in hex. IPv4-mapped IPv6 addresses come back as IPv4.
    pub(super) fn parse_addr(field: &str) -> Option<SocketAddr> {
        let (ip, port) = field.split_once(':')?;
        let port = u16::from_str_radix(port, 16).ok()?;
        let word = |i: usize| -> Option<[u8; 4]> {
            Some(
                u32::from_str_radix(ip.get(i * 8..i * 8 + 8)?, 16)
                    .ok()?
                    .to_ne_bytes(),
            )
        };
        let ip = match ip.len() {
            8 => IpAddr::V4(Ipv4Addr::from(word(0)?)),
            32 => {
                let mut bytes = [0u8; 16];
                for i in 0..4 {
                    bytes[i * 4..i * 4 + 4].copy_from_slice(&word(i)?);
                }
                let v6 = Ipv6Addr::from(bytes);
                v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4)
            }
            _ => return None,
        };
        Some(SocketAddr::new(ip, port))
    }

    /// The inode of the socket with exactly this (local, remote) pair in a `/proc/net/tcp{,6}`
    /// table.
    pub(super) fn find_inode(table: &str, local: SocketAddr, remote: SocketAddr) -> Option<u64> {
        table.lines().skip(1).find_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (l, r) = (parse_addr(f.get(1)?)?, parse_addr(f.get(2)?)?);
            if l == local && r == remote {
                f.get(9)?.parse().ok().filter(|inode| *inode != 0)
            } else {
                None
            }
        })
    }

    /// Whether `pid` has the socket open; `None` when its descriptors cannot be listed.
    fn holds(pid: i32, link: &str) -> Option<bool> {
        let dir = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
        Some(
            dir.flatten().any(|fd| {
                std::fs::read_link(fd.path()).is_ok_and(|target| target.as_os_str() == link)
            }),
        )
    }

    /// Parent pid of every process, from `/proc/<pid>/stat`.
    fn children_map() -> HashMap<i32, Vec<i32>> {
        let mut map: HashMap<i32, Vec<i32>> = HashMap::new();
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return map;
        };
        for entry in dir.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            // `pid (comm) state ppid ...`; comm may hold spaces and parentheses.
            let ppid = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .and_then(|p| p.parse::<i32>().ok());
            if let Some(ppid) = ppid {
                map.entry(ppid).or_default().push(pid);
            }
        }
        map
    }

    pub(super) fn owner(peer: SocketAddr, local: SocketAddr) -> PeerOwner {
        let tables: Vec<String> = ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();
        if tables.is_empty() {
            return PeerOwner::Unknown;
        }
        let Some(inode) = tables.iter().find_map(|t| find_inode(t, peer, local)) else {
            return PeerOwner::Other;
        };
        let link = format!("socket:[{inode}]");
        let me = std::process::id() as i32;
        match holds(me, &link) {
            Some(true) => return PeerOwner::ThisProcess,
            Some(false) => {}
            None => return PeerOwner::Unknown,
        }
        // Breadth-first through the descendants (Backup's dump binaries, possibly under a shell).
        let children = children_map();
        let kids = |pid: i32| children.get(&pid).cloned().unwrap_or_default();
        let mut seen = HashSet::from([me]);
        let mut queue: VecDeque<(i32, usize)> = kids(me).into_iter().map(|p| (p, 1)).collect();
        while let Some((pid, depth)) = queue.pop_front() {
            if !seen.insert(pid) || seen.len() > MAX_PROCESSES {
                continue;
            }
            if holds(pid, &link) == Some(true) {
                return PeerOwner::Child(pid);
            }
            if depth < MAX_DEPTH {
                queue.extend(kids(pid).into_iter().map(|p| (p, depth + 1)));
            }
        }
        PeerOwner::Other
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use std::net::SocketAddr;

    use super::PeerOwner;

    pub(super) fn owner(_peer: SocketAddr, _local: SocketAddr) -> PeerOwner {
        PeerOwner::Unknown
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::time::Duration;

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_a_socket_fdinfo_buffer() {
        let mut buf = vec![0u8; imp_consts::SIZE];
        buf[184..188].copy_from_slice(&2i32.to_ne_bytes()); // AF_INET
        buf[256..260].copy_from_slice(&2i32.to_ne_bytes()); // SOCKINFO_TCP
        buf[264..268].copy_from_slice(&i32::from(5432u16.to_be()).to_ne_bytes());
        buf[268..272].copy_from_slice(&i32::from(61000u16.to_be()).to_ne_bytes());
        buf[288] = 1; // INI_IPV4
        buf[308..312].copy_from_slice(&[127, 0, 0, 1]);
        buf[324..328].copy_from_slice(&[127, 0, 0, 2]);
        let pair = imp::parse_socket_fdinfo(&buf).unwrap();
        assert_eq!(pair.local, "127.0.0.2:61000".parse().unwrap());
        assert_eq!(pair.remote, "127.0.0.1:5432".parse().unwrap());
        // Not an IP socket (a Unix socket's family), or too short: skipped.
        buf[184..188].copy_from_slice(&1i32.to_ne_bytes());
        assert!(imp::parse_socket_fdinfo(&buf).is_none());
        assert!(imp::parse_socket_fdinfo(&buf[..100]).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn finds_a_socket_in_a_proc_net_tcp_table() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:EE48 0100007F:1538 01 00000000:00000000 00:00000000 00000000  1000        0 424242 1 0000000000000000 20 4 30 10 -1\n";
        let client: SocketAddr = "127.0.0.1:61000".parse().unwrap();
        let listener: SocketAddr = "127.0.0.1:5432".parse().unwrap();
        assert_eq!(imp::find_inode(table, client, listener), Some(424242));
        assert_eq!(imp::find_inode(table, listener, client), None);
        // An IPv4-mapped address in `tcp6` reads as IPv4.
        assert_eq!(
            imp::parse_addr("0000000000000000FFFF00000100007F:1538"),
            Some(listener)
        );
    }

    #[cfg(target_os = "macos")]
    mod imp_consts {
        pub const SIZE: usize = 792;
    }

    async fn accept_from(
        listener: &tokio::net::TcpListener,
    ) -> (tokio::net::TcpStream, SocketAddr, SocketAddr) {
        let (stream, peer) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .expect("a connection")
            .unwrap();
        let local = stream.local_addr().unwrap();
        (stream, peer, local)
    }

    #[tokio::test]
    async fn own_and_child_connections_are_ours_others_are_not() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // This process.
        let _client = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (_s, peer, local) = accept_from(&listener).await;
        assert_eq!(owner(peer, local), PeerOwner::ThisProcess);

        // A child process (like pg_dump), here under a shell.
        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("exec /usr/bin/nc 127.0.0.1 {port}"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (_s, peer, local) = accept_from(&listener).await;
        assert!(
            matches!(owner(peer, local), PeerOwner::Child(_)),
            "{:?}",
            owner(peer, local)
        );
        let _ = child.kill().await;

        // A process that is not ours any more: started in the background by a shell that exits
        // at once, so it is re-parented to launchd (init on Linux).
        tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "(sleep 1; /usr/bin/nc -w 3 127.0.0.1 {port} </dev/null >/dev/null) &"
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap();
        let (_s, peer, local) = accept_from(&listener).await;
        let verdict = owner(peer, local);
        assert_eq!(verdict, PeerOwner::Other);
        assert!(!verdict.allowed());
    }
}
