//! Networking: interface addresses, the server's listening socket, and stopping
//! whatever holds a port (docs/windows-port.md §1.K).

use std::net::{IpAddr, SocketAddr};

/// Addresses of interfaces that are up, loopback included, as `(interface, address)` in
/// the system's order. On Windows the interface is the adapter's friendly name ("Wi-Fi",
/// "Ethernet 2", "vEthernet (WSL)").
pub fn interfaces() -> Vec<(String, IpAddr)> {
    sys::interfaces()
}

/// Name prefixes of this system's virtual interfaces (VM and container switches), on top
/// of the Linux names `platform::netif` knows. Empty on Unix.
pub const VIRTUAL_INTERFACES: &[&str] = sys::VIRTUAL_INTERFACES;

/// Bind the server's listener. A `[::]` listener takes IPv4 as well wherever
/// `v6_any_takes_v4` says so; Windows makes `[::]` IPv6-only unless asked, so there it
/// is asked, like Linux's default.
pub async fn bind(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    sys::bind(addr).await
}

/// Whether a `[::]` listener made by `bind` also accepts IPv4 connections.
pub fn v6_any_takes_v4() -> bool {
    sys::v6_any_takes_v4()
}

/// Stop the processes that hold TCP `port`, as `fuser -k PORT/tcp` does; on Windows the
/// listener's owner and what it started after binding, only the current user's, never
/// Workbench itself. `Ok` says what was done, for a "still in use after …" message; `Err`
/// when nothing could be done.
pub async fn kill_port_holders(port: u16) -> Result<String, String> {
    sys::kill_port_holders(port).await
}

#[cfg(unix)]
mod sys {
    use std::ffi::CStr;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::time::Duration;

    pub const VIRTUAL_INTERFACES: &[&str] = &[];

    pub fn interfaces() -> Vec<(String, IpAddr)> {
        let mut out = vec![];
        // SAFETY: getifaddrs allocates a linked list that we only read and then free
        // with freeifaddrs; every pointer is checked for null before it is dereferenced,
        // and sockaddr casts follow the address family reported by the kernel.
        unsafe {
            let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
            if libc::getifaddrs(&mut head) != 0 {
                return out;
            }
            let mut cur = head;
            while !cur.is_null() {
                let ifa = &*cur;
                cur = ifa.ifa_next;
                if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() || ifa.ifa_flags & (libc::IFF_UP as libc::c_uint) == 0 {
                    continue;
                }
                let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                let addr = match i32::from((*ifa.ifa_addr).sa_family) {
                    libc::AF_INET => {
                        let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                        IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
                    }
                    libc::AF_INET6 => {
                        let sa = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                        IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))
                    }
                    _ => continue,
                };
                out.push((name, addr));
            }
            libc::freeifaddrs(head);
        }
        out
    }

    pub async fn bind(addr: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
        tokio::net::TcpListener::bind(addr).await
    }

    /// [::] also takes IPv4 unless the kernel is set to v6-only sockets.
    pub fn v6_any_takes_v4() -> bool {
        std::fs::read_to_string("/proc/sys/net/ipv6/bindv6only").map(|s| s.trim() != "1").unwrap_or(true)
    }

    pub async fn kill_port_holders(port: u16) -> Result<String, String> {
        if !crate::util::which("fuser") {
            return Err("fuser is not installed (package psmisc); free the port yourself".into());
        }
        let out = crate::util::proc::run("fuser", &["-k", &format!("{port}/tcp")], std::path::Path::new("/"), Duration::from_secs(10))
            .await
            .map_err(|e| e.message)?;
        Ok(format!("fuser -k ({})", out.message()))
    }
}

#[cfg(windows)]
mod sys {
    use std::collections::{HashSet, VecDeque};
    use std::io;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::os::windows::io::AsRawSocket;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BUFFER_OVERFLOW, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_SUCCESS, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, GetExtendedTcpTable,
        IP_ADAPTER_ADDRESSES_LH, MIB_TCP6ROW_OWNER_MODULE, MIB_TCP6TABLE_OWNER_MODULE, MIB_TCPROW_OWNER_MODULE,
        MIB_TCPTABLE_OWNER_MODULE, TCP_TABLE_OWNER_MODULE_LISTENER,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, IPPROTO_IPV6, IPV6_V6ONLY, SOCKADDR_IN, SOCKADDR_IN6, SOCKET, WSAGetLastError, setsockopt,
    };
    use windows_sys::Win32::Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
        WaitForSingleObject,
    };

    /// Hyper-V switches (WSL, Docker Desktop, Windows Sandbox), VMware and VirtualBox
    /// host adapters, by their default friendly names.
    pub const VIRTUAL_INTERFACES: &[&str] = &["vEthernet", "VMware", "VirtualBox"];

    /// Processes that forward ports for WSL and Docker Desktop. They run as the user, but
    /// stopping one breaks every forwarded port, so they are left alone (on Linux the
    /// equivalent, docker-proxy, runs as root and `fuser -k` cannot stop it either).
    const FORWARDERS: &[&str] = &["wslrelay.exe", "com.docker.backend.exe", "com.docker.proxy.exe", "vpnkit.exe"];

    /// Closes a kernel handle when dropped.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful Open* or CreateToolhelp32Snapshot call
            // and is closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A zeroed buffer of at least `bytes` bytes, 8-byte aligned as the Win32 structures
    /// read from it need.
    fn aligned(bytes: usize) -> Vec<u64> {
        vec![0u64; bytes.div_ceil(8).max(1)]
    }

    /// A NUL-terminated UTF-16 string.
    ///
    /// # Safety
    /// `p` must point at a NUL-terminated UTF-16 string.
    unsafe fn wide_str(p: *const u16) -> String {
        // SAFETY: the caller guarantees a terminating NUL, so every read is in bounds.
        unsafe {
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }

    pub fn interfaces() -> Vec<(String, IpAddr)> {
        let mut out = vec![];
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        // 15 KB as documented; the call reports the size it needs when that is too small.
        let mut size: u32 = 15 * 1024;
        let mut buf = aligned(size as usize);
        let mut tries = 0;
        let rc = loop {
            // SAFETY: buf is writable for at least `size` bytes and 8-byte aligned.
            let rc = unsafe { GetAdaptersAddresses(u32::from(AF_UNSPEC), flags, ptr::null(), buf.as_mut_ptr().cast(), &mut size) };
            tries += 1;
            if rc != ERROR_BUFFER_OVERFLOW || tries == 4 {
                break rc;
            }
            buf = aligned(size as usize);
        };
        if rc != ERROR_SUCCESS {
            return out;
        }
        // SAFETY: on success buf holds a linked list of adapters whose pointers stay inside
        // buf, which outlives the loop; every pointer is checked for null before it is
        // dereferenced, and sockaddr casts follow the reported family and length.
        unsafe {
            let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
            while !cur.is_null() {
                let adapter = &*cur;
                cur = adapter.Next;
                if adapter.OperStatus != IfOperStatusUp || adapter.FriendlyName.is_null() {
                    continue;
                }
                let name = wide_str(adapter.FriendlyName);
                let mut ua = adapter.FirstUnicastAddress;
                while !ua.is_null() {
                    let unicast = &*ua;
                    ua = unicast.Next;
                    let sa = unicast.Address.lpSockaddr;
                    let len = usize::try_from(unicast.Address.iSockaddrLength).unwrap_or(0);
                    if sa.is_null() {
                        continue;
                    }
                    let addr = match (*sa).sa_family {
                        AF_INET if len >= size_of::<SOCKADDR_IN>() => {
                            let sin = &*(sa as *const SOCKADDR_IN);
                            IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.S_un.S_addr)))
                        }
                        AF_INET6 if len >= size_of::<SOCKADDR_IN6>() => {
                            let sin6 = &*(sa as *const SOCKADDR_IN6);
                            IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.u.Byte))
                        }
                        _ => continue,
                    };
                    out.push((name.clone(), addr));
                }
            }
        }
        out
    }

    pub async fn bind(addr: SocketAddr) -> io::Result<tokio::net::TcpListener> {
        if !(addr.is_ipv6() && addr.ip().is_unspecified()) {
            return tokio::net::TcpListener::bind(addr).await;
        }
        let socket = tokio::net::TcpSocket::new_v6()?;
        let off: i32 = 0;
        // SAFETY: the socket is open for the whole call, and optval points at an i32 of
        // the length given.
        let rc = unsafe {
            setsockopt(socket.as_raw_socket() as SOCKET, IPPROTO_IPV6, IPV6_V6ONLY, (&off as *const i32).cast(), size_of::<i32>() as i32)
        };
        if rc != 0 {
            // SAFETY: reads the calling thread's last Winsock error; no preconditions.
            return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
        }
        socket.bind(addr)?;
        // The backlog tokio's `TcpListener::bind` uses.
        socket.listen(128)
    }

    /// `bind` turns IPV6_V6ONLY off for `[::]`.
    pub fn v6_any_takes_v4() -> bool {
        true
    }

    /// How much later than a socket's bind time a process may seem to start and still count
    /// as started before it (100 ns units): Windows stamps the two from clocks that advance
    /// in different steps, the coarser every 15.6 ms.
    const CLOCK_SLACK: u64 = 50 * 10_000;

    /// `fuser -k` stops every process that has the socket open. Windows names only the
    /// process that bound it, so this stops that owner and the processes it started after
    /// binding, which are the ones that can hold an inherited or shared copy (a reloader
    /// binds and hands the socket to its worker, as `flask run --debug` and
    /// `uvicorn --reload` do). Parents go first, so none starts another worker meanwhile.
    pub async fn kill_port_holders(port: u16) -> Result<String, String> {
        let owners = listeners(port).map_err(|e| format!("cannot list the listeners on port {port}: {e}"))?;
        if owners.is_empty() {
            return Err(format!("no Windows process listens on port {port} (WSL or a VM may hold it); free the port yourself"));
        }
        let (procs, listed) = processes().map_err(|e| format!("cannot list the processes on port {port}: {e}"))?;
        // SAFETY: the pseudo-handle of the current process needs no closing.
        let mine = token_user(unsafe { GetCurrentProcess() }).map_err(|e| format!("cannot read Workbench's user ({e})"))?;
        // SAFETY: no preconditions.
        let me = unsafe { GetCurrentProcessId() };
        let (mut stopped, mut notes, mut gone) = (vec![], vec![], false);
        // Handled pids. Workbench is one, so no walk stops it or enters its tree.
        let mut seen = HashSet::from([me]);
        for Listener { pid: owner, bound } in owners {
            if owner == me {
                notes.push(format!("pid {owner} is Workbench itself"));
                continue;
            }
            if !seen.insert(owner) {
                continue;
            }
            // Processes naming the owner as parent that started after `until` are the children
            // of another process: one that got the pid after the owner exited, or started
            // after the listing.
            let mut until = listed;
            let exited = match open(owner) {
                Err(why) => {
                    notes.push(format!("pid {owner}: {why}"));
                    continue;
                }
                // The owner bound the socket, so it started before `bound`: a younger process
                // has its pid now.
                Ok(Some(p)) if p.created > bound.saturating_add(CLOCK_SLACK) => {
                    until = listed.min(p.created - 1);
                    true
                }
                Ok(Some(p)) if !p.exited => match stop(&p, &mine) {
                    Ok(()) => {
                        stopped.push(format!("{} (pid {owner})", p.name));
                        false
                    }
                    Err(why) => {
                        notes.push(format!("pid {owner}: {why}"));
                        continue;
                    }
                },
                Ok(_) => true,
            };
            if exited {
                gone = true;
                notes.push(format!("pid {owner} had exited"));
            }
            // (pid, earliest and latest start of its children that belong to this tree)
            let mut queue = VecDeque::from([(owner, bound.saturating_sub(CLOCK_SLACK), until)]);
            while let Some((parent, from, until)) = queue.pop_front() {
                let children: Vec<u32> = procs.iter().filter(|&&(_, ppid)| ppid == parent).map(|&(pid, _)| pid).collect();
                for pid in children {
                    if seen.contains(&pid) {
                        continue;
                    }
                    match open(pid) {
                        Err(why) => notes.push(format!("pid {pid}: {why}")),
                        // Exited since the listing (its pid maybe taken again): its children
                        // can still hold the socket.
                        Ok(None) => queue.push_back((pid, from, listed)),
                        Ok(Some(p)) if p.created > listed => queue.push_back((pid, from, listed)),
                        // Started before the socket existed, or not a child of `parent`'s process.
                        Ok(Some(p)) if p.created < from || p.created > until => continue,
                        Ok(Some(p)) if p.exited => queue.push_back((pid, p.created, listed)),
                        Ok(Some(p)) => match stop(&p, &mine) {
                            Ok(()) => {
                                stopped.push(format!("{} (pid {pid})", p.name));
                                queue.push_back((pid, p.created, listed));
                            }
                            Err(why) => notes.push(format!("pid {pid}: {why}")),
                        },
                    }
                    seen.insert(pid);
                }
            }
        }
        if stopped.is_empty() {
            if !gone {
                return Err(format!("cannot free port {port} ({}); free the port yourself", notes.join("; ")));
            }
            // Something may still hold the socket: the caller's wait for the port decides.
            return Ok(notes.join("; "));
        }
        let mut done = format!("stopping {}", stopped.join(", "));
        if !notes.is_empty() {
            done.push_str(&format!(" ({})", notes.join("; ")));
        }
        Ok(done)
    }

    /// A listening socket's owning pid and when it was bound (FILETIME ticks).
    struct Listener {
        pid: u32,
        bound: u64,
    }

    /// The IPv4 and IPv6 listeners on `port`, one per owning pid (its earliest bind).
    fn listeners(port: u16) -> io::Result<Vec<Listener>> {
        // Windows leaves the time 0 when it does not know it; "never" stops the owner alone.
        let bound = |t: i64| u64::try_from(t).ok().filter(|&t| t > 0).unwrap_or(u64::MAX);
        let mut out = vec![];
        let v4 = tcp_listeners(AF_INET)?;
        // SAFETY: tcp_listeners returned a table of this family; its rows are counted by
        // dwNumEntries, checked against the buffer's length before they are read.
        unsafe {
            let table = v4.as_ptr() as *const MIB_TCPTABLE_OWNER_MODULE;
            let rows = ptr::addr_of!((*table).table) as *const MIB_TCPROW_OWNER_MODULE;
            for i in 0..row_count(&v4, (*table).dwNumEntries, rows.cast(), size_of::<MIB_TCPROW_OWNER_MODULE>()) {
                let row = &*rows.add(i);
                // The port is in network byte order in the low 16 bits.
                if u16::from_be(row.dwLocalPort as u16) == port {
                    out.push(Listener { pid: row.dwOwningPid, bound: bound(row.liCreateTimestamp) });
                }
            }
        }
        let v6 = tcp_listeners(AF_INET6)?;
        // SAFETY: as above, for the IPv6 table.
        unsafe {
            let table = v6.as_ptr() as *const MIB_TCP6TABLE_OWNER_MODULE;
            let rows = ptr::addr_of!((*table).table) as *const MIB_TCP6ROW_OWNER_MODULE;
            for i in 0..row_count(&v6, (*table).dwNumEntries, rows.cast(), size_of::<MIB_TCP6ROW_OWNER_MODULE>()) {
                let row = &*rows.add(i);
                if u16::from_be(row.dwLocalPort as u16) == port {
                    out.push(Listener { pid: row.dwOwningPid, bound: bound(row.liCreateTimestamp) });
                }
            }
        }
        out.sort_unstable_by_key(|l| (l.pid, l.bound));
        out.dedup_by_key(|l| l.pid);
        Ok(out)
    }

    /// How many of `claimed` rows of `row_size` bytes starting at `rows` fit in `buf`.
    fn row_count(buf: &[u64], claimed: u32, rows: *const u8, row_size: usize) -> usize {
        let offset = rows as usize - buf.as_ptr() as usize;
        let room = (buf.len() * 8).saturating_sub(offset) / row_size;
        (claimed as usize).min(room)
    }

    /// The listening-socket table of one address family, with owning pids and bind times.
    fn tcp_listeners(family: u16) -> io::Result<Vec<u64>> {
        let mut size: u32 = 0;
        let mut buf = aligned(0);
        // The table can grow between the size query and the read: retry a few times.
        for _ in 0..4 {
            // SAFETY: buf is writable for at least `size` bytes and 8-byte aligned; with
            // size 0 the call only reports the size it needs.
            let rc = unsafe {
                GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, u32::from(family), TCP_TABLE_OWNER_MODULE_LISTENER, 0)
            };
            match rc {
                ERROR_SUCCESS => return Ok(buf),
                ERROR_INSUFFICIENT_BUFFER => buf = aligned(size as usize),
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
        }
        Err(io::Error::other("the TCP table kept growing"))
    }

    /// Every process as `(pid, parent pid)`, and a time after the listing (FILETIME ticks):
    /// a process that starts later under a listed pid is another one.
    fn processes() -> io::Result<(Vec<(u32, u32)>, u64)> {
        // SAFETY: no memory preconditions; the snapshot is closed by the guard.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut now = FILETIME::default();
        // SAFETY: `now` is a valid FILETIME to write.
        unsafe { GetSystemTimePreciseAsFileTime(&mut now) };
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut out = vec![];
        // SAFETY: the snapshot is open and `entry` is a PROCESSENTRY32W with dwSize set.
        let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
        while more {
            out.push((entry.th32ProcessID, entry.th32ParentProcessID));
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
        }
        Ok((out, ticks(now)))
    }

    fn ticks(t: FILETIME) -> u64 {
        (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
    }

    /// A process opened to be stopped.
    struct Process {
        handle: Handle,
        /// Its start time (FILETIME ticks).
        created: u64,
        /// It has exited, but a handle keeps it (and its pid) around.
        exited: bool,
        /// Its executable's file name.
        name: String,
    }

    /// Open `pid`; `None` when no process has that pid.
    fn open(pid: u32) -> Result<Option<Process>, String> {
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
        // SAFETY: OpenProcess has no memory preconditions; the handle is closed by the guard.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                return Ok(None);
            }
            return Err(format!("cannot open it ({e})"));
        }
        let handle = Handle(handle);
        let [mut created, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        // SAFETY: the handle is open with PROCESS_QUERY_LIMITED_INFORMATION and all four
        // pointers are valid FILETIMEs to write.
        if unsafe { GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) } == 0 {
            return Err(format!("cannot read its start time ({})", io::Error::last_os_error()));
        }
        // SAFETY: the handle is open with SYNCHRONIZE; a zero timeout only polls.
        let exited = unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0;
        let name = image_name(handle.0).unwrap_or_else(|| "process".into());
        Ok(Some(Process { handle, created: ticks(created), exited, name }))
    }

    /// Terminate `p` when it runs as `mine`'s user (a TOKEN_USER buffer from `token_user`)
    /// and is not a port forwarder.
    fn stop(p: &Process, mine: &[u64]) -> Result<(), String> {
        let name = &p.name;
        let theirs = token_user(p.handle.0).map_err(|e| format!("cannot read the user of {name} ({e})"))?;
        // SAFETY: both buffers hold a TOKEN_USER written by GetTokenInformation, whose Sid
        // points into the same buffer.
        let same = unsafe {
            EqualSid((*(mine.as_ptr() as *const TOKEN_USER)).User.Sid, (*(theirs.as_ptr() as *const TOKEN_USER)).User.Sid) != 0
        };
        if !same {
            return Err(format!("{name} belongs to another user"));
        }
        if FORWARDERS.iter().any(|f| name.eq_ignore_ascii_case(f)) {
            return Err(format!("{name} forwards the port from WSL or Docker; stop what listens there"));
        }
        // SAFETY: the handle is open with PROCESS_TERMINATE.
        if unsafe { TerminateProcess(p.handle.0, 1) } == 0 {
            return Err(format!("cannot stop {name} ({})", io::Error::last_os_error()));
        }
        Ok(())
    }

    /// The file name of a process's executable.
    fn image_name(process: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: buf is writable for `len` UTF-16 units; on success len is the length
        // written, without the NUL.
        if unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())]);
        Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned())
    }

    /// The TOKEN_USER of a process (its user's SID), in a buffer it points into.
    fn token_user(process: HANDLE) -> io::Result<Vec<u64>> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: `process` is open with at least PROCESS_QUERY_LIMITED_INFORMATION (or is
        // the current process's pseudo-handle); the token is closed by the guard.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Handle(token);
        let mut len: u32 = 0;
        // SAFETY: a null buffer of length 0 only asks for the size needed.
        unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = aligned(len as usize);
        // SAFETY: buf is writable for at least `len` bytes and 8-byte aligned.
        if unsafe { GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(buf)
    }

    #[cfg(test)]
    mod tests {
        use std::io::{BufRead, BufReader};
        use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
        use std::os::windows::io::AsRawSocket;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};

        fn pids(port: u16) -> Vec<u32> {
            super::listeners(port).unwrap().iter().map(|l| l.pid).collect()
        }

        fn listening(port: u16) -> bool {
            TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(500)).is_ok()
        }

        /// A listener of this process shows up under its own pid, over IPv4 and IPv6, bound
        /// after this process started.
        #[test]
        fn finds_the_owner_of_a_listener() {
            let me = std::process::id();
            let v4 = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = v4.local_addr().unwrap().port();
            assert_eq!(pids(port), vec![me]);
            let bound = super::listeners(port).unwrap()[0].bound;
            assert_ne!(bound, u64::MAX, "Windows gives no bind time");
            let started = super::open(me).unwrap().unwrap().created;
            assert!(started <= bound.saturating_add(super::CLOCK_SLACK), "started {started}, bound {bound}");
            if let Ok(v6) = TcpListener::bind("[::1]:0") {
                let port = v6.local_addr().unwrap().port();
                assert_eq!(pids(port), vec![me]);
            }
        }

        /// Workbench never stops itself, and says so.
        #[tokio::test]
        async fn never_stops_itself() {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            let e = super::kill_port_holders(port).await.unwrap_err();
            assert!(e.contains("Workbench itself"), "{e}");
        }

        /// A process of the same user that listens on the port is stopped.
        #[tokio::test]
        async fn stops_a_listener_of_the_same_user() {
            let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let script = format!(
                "$l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, {port}); $l.Start(); Start-Sleep -Seconds 60"
            );
            let mut child = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            while pids(port).is_empty() {
                assert!(Instant::now() < deadline, "the listener did not start");
                std::thread::sleep(Duration::from_millis(100));
            }
            let done = super::kill_port_holders(port).await.unwrap();
            assert!(done.contains("powershell.exe"), "{done}");
            let status = child.wait().unwrap();
            assert!(!status.success());
        }

        /// Not a test: `stops_the_children_that_share_a_listener` runs this test binary as a
        /// server that binds a port and starts a child that inherits the socket, as a
        /// reloader does, and then waits (or exits with `WB_PORT_HOLDER_EXIT`).
        #[test]
        fn port_holder_shim() {
            let Some(port) = std::env::var_os("WB_PORT_HOLDER_SHIM") else {
                return;
            };
            let l = TcpListener::bind(("127.0.0.1", port.to_str().unwrap().parse::<u16>().unwrap())).unwrap();
            // SAFETY: the socket is open for the whole call; this only marks it inheritable.
            let ok = unsafe { SetHandleInformation(l.as_raw_socket() as HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
            let child = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 60"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            println!("port-holder-child={}", child.id());
            if std::env::var_os("WB_PORT_HOLDER_EXIT").is_some() {
                // Exit without closing the socket, as a server that dies does.
                std::mem::forget(l);
                return;
            }
            std::thread::sleep(Duration::from_secs(60));
        }

        /// A server whose socket lives on in a child it started is stopped with that child,
        /// whether the server still runs or has exited (Windows then names the dead server).
        #[tokio::test]
        async fn stops_the_children_that_share_a_listener() {
            for exits in [false, true] {
                let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
                let mut shim = Command::new(std::env::current_exe().unwrap());
                shim.args(["--exact", "--quiet", "--nocapture", "--test-threads=1", "util::os::net::sys::tests::port_holder_shim"])
                    .env("WB_PORT_HOLDER_SHIM", port.to_string())
                    .stdout(Stdio::piped());
                if exits {
                    shim.env("WB_PORT_HOLDER_EXIT", "1");
                }
                let mut server = shim.spawn().unwrap();
                // Kept open to the end: the shim writes to it after the marker.
                let mut out = BufReader::new(server.stdout.take().unwrap());
                let mut line = String::new();
                let child: u32 = loop {
                    line.clear();
                    assert_ne!(out.read_line(&mut line).unwrap(), 0, "the shim did not start its child");
                    if let Some((_, pid)) = line.split_once("port-holder-child=") {
                        break pid.trim().parse().unwrap();
                    }
                };
                if exits {
                    assert!(server.wait().unwrap().success());
                    assert!(listening(port), "the child did not inherit the socket");
                }
                assert_eq!(pids(port), vec![server.id()]);
                let done = super::kill_port_holders(port).await.unwrap();
                assert!(done.contains(&format!("powershell.exe (pid {child})")), "{done}");
                if exits {
                    assert!(done.contains(&format!("pid {} had exited", server.id())), "{done}");
                } else {
                    assert!(!server.wait().unwrap().success(), "{done}");
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                while listening(port) {
                    assert!(Instant::now() < deadline, "port {port} is still in use after {done}");
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// `v6_any_takes_v4` tells the truth about the listener `bind` makes.
    #[tokio::test]
    async fn unspecified_v6_listener_matches_its_description() {
        let Ok(l) = super::bind("[::]:0".parse().unwrap()).await else {
            eprintln!("no IPv6; skipping");
            return;
        };
        let port = l.local_addr().unwrap().port();
        let v4 = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok();
        assert_eq!(v4, super::v6_any_takes_v4());
    }
}
