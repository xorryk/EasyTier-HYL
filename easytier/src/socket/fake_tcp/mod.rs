mod netfilter;
mod packet;
mod stack;

use bytes::BytesMut;
use network_interface::NetworkInterfaceConfig;
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    net::TcpStream,
};

use easytier_core::{
    socket::tcp::VirtualTcpSocket,
    tunnel::{IpVersion, TunnelError},
};

use crate::{common::netns::NetNS, tunnel::FromUrl};

use self::netfilter::create_tun;
use self::packet::MacAddr;

use futures::Future;
use tokio_util::task::AbortOnDropHandle;

use dashmap::DashMap;

struct IpToIfNameCache {
    ip_to_ifname: DashMap<IpAddr, (String, Option<MacAddr>)>,
}

fn parse_mac_addr(value: &str) -> Option<MacAddr> {
    let mut bytes = [0; 6];
    let mut octets = value.split([':', '-']);
    for byte in &mut bytes {
        *byte = u8::from_str_radix(octets.next()?, 16).ok()?;
    }
    octets.next().is_none().then(|| MacAddr::from_bytes(&bytes))
}

impl IpToIfNameCache {
    fn new() -> Self {
        Self {
            ip_to_ifname: DashMap::new(),
        }
    }

    fn reload_ip_to_ifname(&self) -> bool {
        let Ok(interfaces) = network_interface::NetworkInterface::show() else {
            tracing::warn!("failed to enumerate interfaces when reloading faketcp ip cache");
            return false;
        };
        self.ip_to_ifname.clear();
        for iface in interfaces {
            let mac = iface.mac_addr.as_deref().and_then(|mac| {
                parse_mac_addr(mac).or_else(|| {
                    tracing::debug!(iface = %iface.name, mac, "failed to parse interface mac");
                    None
                })
            });
            for ip in iface.addr.iter() {
                self.ip_to_ifname.insert(ip.ip(), (iface.name.clone(), mac));
            }
        }
        true
    }

    fn get_ifname(&self, ip: &IpAddr) -> Option<(String, Option<MacAddr>)> {
        if let Some(ifname) = self.ip_to_ifname.get(ip) {
            Some(ifname.clone())
        } else {
            self.reload_ip_to_ifname();
            self.ip_to_ifname.get(ip).map(|s| s.clone())
        }
    }
}

fn faketcp_transport_label(driver_type: &str) -> String {
    format!("faketcp_{}", driver_type)
}

async fn create_tun_off_runtime(
    interface_name: String,
    src_addr: Option<SocketAddr>,
    dst_addr: SocketAddr,
    net_ns: NetNS,
    bpf_recv_buffer_kib: Option<u32>,
) -> Result<Arc<dyn stack::Tun>, TunnelError> {
    tokio::task::spawn_blocking(move || {
        net_ns.run(|| create_tun(&interface_name, src_addr, dst_addr, bpf_recv_buffer_kib))
    })
    .await
    .map_err(|e| TunnelError::InternalError(format!("faketcp create_tun task failed: {e}")))?
    .map_err(Into::into)
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ListenerStackKey {
    interface_name: String,
    local_addr: SocketAddr,
}

impl From<&AcceptResult> for ListenerStackKey {
    fn from(result: &AcceptResult) -> Self {
        Self {
            interface_name: result.interface_name.clone(),
            local_addr: result.local_addr,
        }
    }
}

pub(crate) struct FakeTcpSocketListener {
    addr: url::Url,
    bpf_recv_buffer_kib: Option<u32>,
    os_listener: Option<tokio::net::TcpListener>,
    // Capture filters bind a concrete local address, not just an interface.
    stack_map: DashMap<ListenerStackKey, Arc<stack::Stack>>,
    // a cache from ip addr to interface name
    ip_to_ifname: IpToIfNameCache,
}

impl std::fmt::Debug for FakeTcpSocketListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FakeTcpSocketListener")
            .field("addr", &self.addr)
            .field("listening", &self.os_listener.is_some())
            .finish()
    }
}

impl FakeTcpSocketListener {
    pub(crate) fn new(addr: url::Url, bpf_recv_buffer_kib: Option<u32>) -> Self {
        FakeTcpSocketListener {
            addr,
            bpf_recv_buffer_kib,
            os_listener: None,
            stack_map: DashMap::new(),
            ip_to_ifname: IpToIfNameCache::new(),
        }
    }

    async fn do_accept(&mut self) -> Result<AcceptResult, TunnelError> {
        loop {
            match self.os_listener.as_mut().unwrap().accept().await {
                Ok((s, remote_addr)) => {
                    let Ok(local_addr) = s.local_addr() else {
                        tracing::warn!("accept fail with local_addr error");
                        continue;
                    };
                    let Some((interface_name, mac)) =
                        self.ip_to_ifname.get_ifname(&local_addr.ip())
                    else {
                        tracing::warn!("accept fail with interface_name error");
                        continue;
                    };
                    return Ok(AcceptResult {
                        socket: s,
                        local_addr,
                        remote_addr,
                        interface_name,
                        mac,
                    });
                }
                Err(e) => {
                    use std::io::ErrorKind::*;
                    if matches!(
                        e.kind(),
                        NotConnected | ConnectionAborted | ConnectionRefused | ConnectionReset
                    ) {
                        tracing::warn!(?e, "accept fail with retryable error: {:?}", e);
                        continue;
                    }
                    tracing::warn!(?e, "accept fail");
                    return Err(e.into());
                }
            }
        }
    }

    async fn get_stack(
        &self,
        accept_result: &AcceptResult,
    ) -> Result<Arc<stack::Stack>, TunnelError> {
        let key = ListenerStackKey::from(accept_result);
        if !self.stack_map.contains_key(&key) && self.ip_to_ifname.reload_ip_to_ifname() {
            self.prune_stacks();
        }
        self.get_stack_with(key, accept_result.mac, || {
            create_tun_off_runtime(
                accept_result.interface_name.clone(),
                None,
                accept_result.local_addr,
                NetNS::new(None),
                self.bpf_recv_buffer_kib,
            )
        })
        .await
    }

    fn prune_stacks(&self) {
        self.stack_map.retain(|key, stack| {
            if stack.is_closed() {
                return false;
            }
            let address_present = self
                .ip_to_ifname
                .ip_to_ifname
                .get(&key.local_addr.ip())
                .is_some_and(|binding| binding.0 == key.interface_name);
            // Keep established connections on old addresses until they close.
            address_present || stack.has_active_sockets()
        });
    }

    async fn get_stack_with<F, Fut>(
        &self,
        key: ListenerStackKey,
        mac: Option<MacAddr>,
        create_tun: F,
    ) -> Result<Arc<stack::Stack>, TunnelError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<dyn stack::Tun>, TunnelError>>,
    {
        if let Some(entry) = self.stack_map.get(&key) {
            let stack = entry.clone();
            drop(entry);

            if !stack.is_closed() {
                return Ok(stack);
            }

            tracing::warn!(
                ?key,
                "fake_tcp stack reader_task finished, recreating stack"
            );
            self.stack_map.remove(&key);
        }

        let tun = create_tun().await?;
        tracing::info!(?key, "create new address-bound fake_tcp stack");
        let stack = Arc::new(stack::Stack::new(tun, mac));
        self.stack_map.insert(key, stack.clone());

        Ok(stack)
    }
}

fn build_os_socket_reader_task(mut socket: TcpStream) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(tokio::spawn(async move {
        // read the os socket until it's closed
        let mut buf = [0u8; 1024];
        while let Ok(size) = socket.read(&mut buf).await {
            tracing::trace!("read {} bytes from os socket", size);
            if size == 0 {
                break;
            }
        }
        tracing::info!("FakeTcpSocketListener os socket closed");
    }))
}

type FakeTcpReadFuture = Pin<Box<dyn Future<Output = Option<BytesMut>> + Send + Sync + 'static>>;

enum FakeTcpReadState {
    Buffered(BytesMut),
    Receiving(FakeTcpReadFuture),
    Closed,
}

pub(crate) struct FakeTcpSocket {
    socket: Arc<stack::Socket>,
    read_state: FakeTcpReadState,
    transport_label: String,
    _lifetime_guard: Box<dyn Send + Sync>,
}

impl FakeTcpSocket {
    fn new<T>(socket: stack::Socket, transport_label: String, lifetime_guard: T) -> Self
    where
        T: Send + Sync + 'static,
    {
        Self {
            socket: Arc::new(socket),
            read_state: FakeTcpReadState::Buffered(BytesMut::new()),
            transport_label,
            _lifetime_guard: Box::new(lifetime_guard),
        }
    }
}

impl AsyncRead for FakeTcpSocket {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            let state = std::mem::replace(&mut this.read_state, FakeTcpReadState::Closed);
            match state {
                FakeTcpReadState::Buffered(mut buffer) if !buffer.is_empty() => {
                    let length = buffer.len().min(output.remaining());
                    output.put_slice(&buffer.split_to(length));
                    this.read_state = FakeTcpReadState::Buffered(buffer);
                    return Poll::Ready(Ok(()));
                }
                FakeTcpReadState::Buffered(_) => {
                    let socket = this.socket.clone();
                    this.read_state = FakeTcpReadState::Receiving(Box::pin(async move {
                        let mut buffer = BytesMut::new();
                        socket.recv(&mut buffer).await.map(|_| buffer)
                    }));
                }
                FakeTcpReadState::Receiving(mut receive) => match receive.as_mut().poll(context) {
                    Poll::Ready(Some(buffer)) => {
                        this.read_state = FakeTcpReadState::Buffered(buffer);
                    }
                    Poll::Ready(None) => {
                        this.read_state = FakeTcpReadState::Closed;
                        return Poll::Ready(Ok(()));
                    }
                    Poll::Pending => {
                        this.read_state = FakeTcpReadState::Receiving(receive);
                        return Poll::Pending;
                    }
                },
                FakeTcpReadState::Closed => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for FakeTcpSocket {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.socket.try_send(buffer).is_none() {
            // Preserve FakeTCP's existing lossy send behavior. A temporary
            // driver lock conflict is indistinguishable from a closed stack
            // here, and the former must not tear down the peer connection.
            tracing::trace!(
                len = buffer.len(),
                "FakeTCP socket dropped an outgoing frame"
            );
        }
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        self.socket.close();
        Poll::Ready(Ok(()))
    }
}

impl VirtualTcpSocket for FakeTcpSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.socket.local_addr())
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.socket.remote_addr())
    }

    fn transport_label(&self) -> Option<&str> {
        Some(&self.transport_label)
    }
}

#[derive(Debug)]
struct AcceptResult {
    socket: TcpStream,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    interface_name: String,
    mac: Option<MacAddr>,
}

impl FakeTcpSocketListener {
    pub(crate) async fn accept_socket(&mut self) -> Result<FakeTcpSocket, TunnelError> {
        tracing::debug!("FakeTcpSocketListener waiting for accept");
        let (res, stack, socket) = loop {
            let res = self.do_accept().await?;
            let stack = self.get_stack(&res).await?;
            let socket = stack.try_alloc_established_socket(
                res.local_addr,
                res.remote_addr,
                stack::State::Established,
            );
            let Some(socket) = socket else {
                tracing::warn!(
                    interface_name = res.interface_name,
                    "fake_tcp stack closed while accepting connection, dropping accepted socket"
                );
                self.stack_map.remove(&ListenerStackKey::from(&res));
                continue;
            };
            break (res, stack, socket);
        };

        tracing::info!(
            ?res,
            remote = socket.remote_addr().to_string(),
            "FakeTcpSocketListener accepted connection"
        );

        let transport_label = faketcp_transport_label(stack.driver_type());
        Ok(FakeTcpSocket::new(
            socket,
            transport_label,
            (build_os_socket_reader_task(res.socket), stack),
        ))
    }

    async fn listen_socket(&mut self) -> Result<(), TunnelError> {
        let port = self.addr.port().unwrap_or(0);
        let bind_addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let os_listener = tokio::net::TcpListener::bind(bind_addr).await?;
        tracing::info!(port, "FakeTcpSocketListener listening");
        self.os_listener = Some(os_listener);
        Ok(())
    }
}

#[async_trait::async_trait]
impl easytier_core::socket::SocketListener for FakeTcpSocketListener {
    type Accepted = FakeTcpSocket;

    async fn listen(&mut self) -> anyhow::Result<()> {
        Ok(self.listen_socket().await?)
    }

    async fn accept(&mut self) -> anyhow::Result<Self::Accepted> {
        Ok(self.accept_socket().await?)
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

fn get_local_ip_for_destination(destination: IpAddr) -> Option<IpAddr> {
    // 使用一个不可路由的、私有的、或回环地址创建一个临时的 socket，让内核自动选择源接口。
    // 对于 IPv4，使用 0.0.0.0; 对于 IPv6，使用 ::
    let bind_addr = if destination.is_ipv4() {
        IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0))
    };

    // 绑定到一个临时端口 (0)
    let socket = UdpSocket::bind((bind_addr, 0)).ok()?;

    // 尝试连接到目标地址。这不会真正发送数据包，只是让内核确定路由。
    socket.connect((destination, 80)).ok()?; // 使用一个常见的端口，例如 80

    // 获取 socket 的本地地址信息
    socket.local_addr().map(|addr| addr.ip()).ok()
}

fn bind_addr_for_remote(remote_addr: &SocketAddr) -> SocketAddr {
    match remote_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    }
}

fn new_os_socket_for_remote(remote_addr: &SocketAddr) -> io::Result<tokio::net::TcpSocket> {
    match remote_addr {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4(),
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6(),
    }
}

async fn connect_socket_with_cache(
    remote_addr: SocketAddr,
    socket_mark: Option<u32>,
    ip_to_if_name: &IpToIfNameCache,
    net_ns: NetNS,
    bpf_recv_buffer_kib: Option<u32>,
) -> Result<FakeTcpSocket, TunnelError> {
    let (local_addr, interface_name, mac, os_socket) = net_ns.run(|| {
        let local_ip = get_local_ip_for_destination(remote_addr.ip())
            .ok_or(TunnelError::InternalError("Failed to get local ip".into()))?;

        let os_socket = new_os_socket_for_remote(&remote_addr)?;
        // SO_MARK applies only to the kernel-visible "decoy" socket below.
        // The actual FakeTCP payload travels via crafted segments written
        // straight to the TUN device, which the kernel doesn't tag with
        // SO_MARK. Operators relying on fwmark for FakeTCP must mark the
        // TUN device's traffic with a separate nftables/iptables rule.
        crate::tunnel::common::apply_socket_mark(&socket2::SockRef::from(&os_socket), socket_mark)?;
        os_socket.bind(bind_addr_for_remote(&remote_addr))?;
        let local_addr = SocketAddr::new(local_ip, os_socket.local_addr()?.port());

        let (interface_name, mac) =
            ip_to_if_name
                .get_ifname(&local_ip)
                .ok_or(TunnelError::InternalError(
                    "Failed to get interface name".into(),
                ))?;
        Ok::<_, TunnelError>((local_addr, interface_name, mac, os_socket))
    })?;

    let tun = create_tun_off_runtime(
        interface_name,
        Some(remote_addr),
        local_addr,
        net_ns,
        bpf_recv_buffer_kib,
    )
    .await?;
    let stack = stack::Stack::new(tun, mac);
    let transport_label = faketcp_transport_label(stack.driver_type());

    let socket = stack
        .try_alloc_established_socket(local_addr, remote_addr, stack::State::SynSent)
        .ok_or(TunnelError::InternalError(
            "FakeTCP stack closed while allocating socket".into(),
        ))?;

    let os_stream = os_socket.connect(remote_addr).await?;

    tracing::info!(?remote_addr, "FakeTCP socket connecting");

    let mut buf = BytesMut::new();
    socket
        .recv(&mut buf)
        .await
        .ok_or(TunnelError::InternalError(
            "Failed to recv bytes to establish connection".into(),
        ))?;

    tracing::info!(local_addr = ?socket.local_addr(), "FakeTCP socket connected");

    Ok(FakeTcpSocket::new(
        socket,
        transport_label,
        (build_os_socket_reader_task(os_stream), stack),
    ))
}

pub(crate) async fn connect_socket(
    remote_addr: SocketAddr,
    socket_mark: Option<u32>,
    net_ns: NetNS,
    bpf_recv_buffer_kib: Option<u32>,
) -> Result<FakeTcpSocket, TunnelError> {
    connect_socket_with_cache(
        remote_addr,
        socket_mark,
        &IpToIfNameCache::new(),
        net_ns,
        bpf_recv_buffer_kib,
    )
    .await
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use easytier_core::socket::SocketListener;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    struct AddressCapture {
        local_addr: SocketAddr,
        sender: flume::Sender<Option<Bytes>>,
        receiver: flume::Receiver<Option<Bytes>>,
    }

    impl AddressCapture {
        fn new(local_addr: SocketAddr) -> Self {
            let (sender, receiver) = flume::unbounded();
            Self {
                local_addr,
                sender,
                receiver,
            }
        }

        fn deliver(&self, local: SocketAddr, remote: SocketAddr, payload: &[u8]) -> bool {
            // Model the address filter used by WinDivert/BPF, including the port.
            if local != self.local_addr {
                return false;
            }
            let frame = packet::build_tcp_packet(
                MacAddr::default(),
                MacAddr::default(),
                remote,
                local,
                100,
                100,
                packet::TCP_FLAG_ACK | packet::TCP_FLAG_PSH,
                Some(payload),
            );
            self.sender.send(Some(frame)).unwrap();
            true
        }
    }

    #[async_trait::async_trait]
    impl stack::Tun for AddressCapture {
        async fn recv(&self, packet: &mut BytesMut) -> io::Result<usize> {
            let frame = self
                .receiver
                .recv_async()
                .await
                .ok()
                .flatten()
                .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "test capture closed"))?;
            packet.extend_from_slice(&frame);
            Ok(frame.len())
        }

        fn try_send(&self, _packet: &Bytes) -> io::Result<()> {
            Ok(())
        }

        fn driver_type(&self) -> &'static str {
            "address-filter-test"
        }
    }

    async fn listener_test_stack(
        listener: &FakeTcpSocketListener,
        interface: &str,
        local_addr: SocketAddr,
        captures: &Mutex<Vec<Arc<AddressCapture>>>,
    ) -> Arc<stack::Stack> {
        let key = ListenerStackKey {
            interface_name: interface.to_owned(),
            local_addr,
        };
        listener
            .get_stack_with(key, None, || async {
                let capture = Arc::new(AddressCapture::new(local_addr));
                captures.lock().unwrap().push(capture.clone());
                Ok(capture as Arc<dyn stack::Tun>)
            })
            .await
            .unwrap()
    }

    async fn listener_test_receive(socket: &stack::Socket, expected: &[u8]) {
        let mut payload = BytesMut::new();
        let length = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut payload))
            .await
            .expect("address-specific capture did not deliver the reconnect payload");
        assert_eq!(length, Some(expected.len()));
        assert_eq!(&payload[..], expected);
    }

    #[tokio::test]
    async fn listener_address_change_delivers_payload_and_preserves_existing_connection() {
        let listener = FakeTcpSocketListener::new("faketcp://[::]:11014".parse().unwrap(), None);
        let captures = Mutex::new(Vec::new());
        let old_addr = "[2001:db8:1::1]:11014".parse().unwrap();
        let new_addr = "[2001:db8:2::1]:11014".parse().unwrap();
        let remote = "[2001:db8:3::1]:40000".parse().unwrap();
        let old_stack = listener_test_stack(&listener, "nic0", old_addr, &captures).await;
        let old_socket = old_stack
            .try_alloc_established_socket(old_addr, remote, stack::State::Established)
            .unwrap();
        let old_capture = captures.lock().unwrap()[0].clone();
        assert!(old_capture.deliver(old_addr, remote, b"before change"));
        listener_test_receive(&old_socket, b"before change").await;

        let new_stack = listener_test_stack(&listener, "nic0", new_addr, &captures).await;
        assert!(!Arc::ptr_eq(&old_stack, &new_stack));
        assert_eq!(captures.lock().unwrap().len(), 2);
        let new_socket = new_stack
            .try_alloc_established_socket(new_addr, remote, stack::State::Established)
            .unwrap();
        let new_capture = captures.lock().unwrap()[1].clone();
        assert!(!old_capture.deliver(new_addr, remote, b"new address"));
        assert!(new_capture.deliver(new_addr, remote, b"new address"));
        listener_test_receive(&new_socket, b"new address").await;
        assert!(old_capture.deliver(old_addr, remote, b"old connection still alive"));
        listener_test_receive(&old_socket, b"old connection still alive").await;

        drop(new_socket);
        let reconnected = listener_test_stack(&listener, "nic0", new_addr, &captures).await;
        assert!(Arc::ptr_eq(&new_stack, &reconnected));
        assert_eq!(captures.lock().unwrap().len(), 2);
        let remote = "[2001:db8:3::1]:40001".parse().unwrap();
        let socket = reconnected
            .try_alloc_established_socket(new_addr, remote, stack::State::Established)
            .unwrap();
        assert!(new_capture.deliver(new_addr, remote, b"reconnected"));
        listener_test_receive(&socket, b"reconnected").await;
    }

    #[tokio::test]
    async fn listener_address_change_separates_address_family_port_and_interface() {
        let listener = FakeTcpSocketListener::new("faketcp://[::]:11014".parse().unwrap(), None);
        let captures = Mutex::new(Vec::new());
        for (index, (interface, address)) in [
            ("nic0", "192.0.2.1:11014"),
            ("nic0", "192.0.2.2:11014"),
            ("nic0", "[2001:db8::1]:11014"),
            ("nic0", "[2001:db8::1]:11015"),
            ("nic1", "[2001:db8::1]:11014"),
        ]
        .into_iter()
        .enumerate()
        {
            let address = address.parse().unwrap();
            let stack = listener_test_stack(&listener, interface, address, &captures).await;
            let reused = listener_test_stack(&listener, interface, address, &captures).await;
            assert!(Arc::ptr_eq(&stack, &reused));
            assert_eq!(captures.lock().unwrap().len(), index + 1);
        }
    }

    #[tokio::test]
    async fn listener_address_change_recreates_closed_capture_without_discarding_other_address() {
        let listener = FakeTcpSocketListener::new("faketcp://[::]:11014".parse().unwrap(), None);
        let captures = Mutex::new(Vec::new());
        let first = "[2001:db8:1::1]:11014".parse().unwrap();
        let second = "[2001:db8:2::1]:11014".parse().unwrap();
        let closed = listener_test_stack(&listener, "nic0", first, &captures).await;
        let other = listener_test_stack(&listener, "nic0", second, &captures).await;
        captures.lock().unwrap()[0].sender.send(None).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !closed.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let replacement = listener_test_stack(&listener, "nic0", first, &captures).await;
        assert!(!Arc::ptr_eq(&closed, &replacement));
        let reused_other = listener_test_stack(&listener, "nic0", second, &captures).await;
        assert!(Arc::ptr_eq(&other, &reused_other));
        assert_eq!(captures.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn listener_address_change_reclaims_removed_address_only_after_connection_closes() {
        let listener = FakeTcpSocketListener::new("faketcp://[::]:11014".parse().unwrap(), None);
        let captures = Mutex::new(Vec::new());
        let old_addr = "[2001:db8:1::1]:11014".parse().unwrap();
        let current_addr: SocketAddr = "[2001:db8:2::1]:11014".parse().unwrap();
        let old_stack = listener_test_stack(&listener, "nic0", old_addr, &captures).await;
        let current = listener_test_stack(&listener, "nic0", current_addr, &captures).await;
        let socket = old_stack
            .try_alloc_established_socket(
                old_addr,
                "[2001:db8:3::1]:40000".parse().unwrap(),
                stack::State::Established,
            )
            .unwrap();
        listener
            .ip_to_ifname
            .ip_to_ifname
            .insert(current_addr.ip(), ("nic0".to_owned(), None));
        listener.prune_stacks();
        assert_eq!(listener.stack_map.len(), 2);
        assert!(!old_stack.is_closed());

        drop(socket);
        listener.prune_stacks();
        assert_eq!(listener.stack_map.len(), 1);
        let reused = listener_test_stack(&listener, "nic0", current_addr, &captures).await;
        assert!(Arc::ptr_eq(&current, &reused));
        assert_eq!(captures.lock().unwrap().len(), 2);
    }

    #[test]
    fn connector_uses_ipv4_socket_bind_addr_for_ipv4_remote() {
        let remote_addr: SocketAddr = "192.0.2.1:443".parse().unwrap();

        assert_eq!(
            bind_addr_for_remote(&remote_addr),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
        );

        let os_socket = new_os_socket_for_remote(&remote_addr).unwrap();
        os_socket.bind(bind_addr_for_remote(&remote_addr)).unwrap();
        assert!(os_socket.local_addr().unwrap().is_ipv4());
    }

    #[test]
    fn connector_uses_ipv6_socket_bind_addr_for_ipv6_remote() {
        let remote_addr: SocketAddr = "[2001:db8::1]:443".parse().unwrap();

        assert_eq!(
            bind_addr_for_remote(&remote_addr),
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
        );

        let os_socket = new_os_socket_for_remote(&remote_addr).unwrap();
        os_socket.bind(bind_addr_for_remote(&remote_addr)).unwrap();
        assert!(os_socket.local_addr().unwrap().is_ipv6());
    }

    #[tokio::test]
    async fn faketcp_socket_pingpong() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            #[cfg(target_family = "unix")]
            {
                if unsafe { nix::libc::geteuid() } != 0 {
                    return;
                }
            }

            let mut listener =
                FakeTcpSocketListener::new("faketcp://0.0.0.0:31011".parse().unwrap(), None);
            listener.listen().await.unwrap();
            let (server_ready_tx, server_ready_rx) = tokio::sync::oneshot::channel();

            let server = tokio::spawn(async move {
                let mut socket = listener.accept().await.unwrap();
                server_ready_tx.send(()).unwrap();
                let mut request = [0; 4];
                socket.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"ping");
                socket.write_all(b"pong").await.unwrap();
            });

            let mut socket = connect_socket(
                "127.0.0.1:31011".parse().unwrap(),
                None,
                NetNS::new(None),
                None,
            )
            .await
            .unwrap();
            server_ready_rx.await.unwrap();
            socket.write_all(b"ping").await.unwrap();
            let mut response = [0; 4];
            socket.read_exact(&mut response).await.unwrap();
            assert_eq!(&response, b"pong");

            server.await.unwrap();
        })
        .await
        .expect("FakeTCP socket ping-pong timed out");
    }
}
