use crate::error::UdpError;
use crate::network::ConnectionInfo;
use crate::state::{State, StatePhase};

use futures_util::future::join4;
use futures_util::stream::{SplitSink, SplitStream, Stream};
use futures_util::{FutureExt, SinkExt, StreamExt};
use log::*;
use mumble_protocol::crypt::ClientCryptState;
use mumble_protocol::ping::{PingPacket, PongPacket};
use mumble_protocol::voice::VoicePacket;
use mumble_protocol::Serverbound;
use std::collections::{hash_map::Entry, HashMap};
use std::convert::TryFrom;
use std::net::{Ipv6Addr, SocketAddr};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, RwLock,
};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::time::{interval, timeout, Duration};
use tokio::{join, net::UdpSocket};
use tokio_util::udp::UdpFramed;

use super::{run_until, VoiceStreamType};

pub(crate) type PingRequest = (u64, SocketAddr, Box<dyn FnOnce(Option<PongPacket>) + Send>);

type UdpSender = SplitSink<UdpFramed<ClientCryptState>, (VoicePacket<Serverbound>, SocketAddr)>;
type UdpReceiver = SplitStream<UdpFramed<ClientCryptState>>;

pub(crate) async fn handle(
    state: Arc<RwLock<State>>,
    mut connection_info_receiver: watch::Receiver<Option<ConnectionInfo>>,
    mut crypt_state_receiver: mpsc::Receiver<ClientCryptState>,
) -> Result<(), UdpError> {
    let receiver = state.read().unwrap().audio_input().receiver();

    loop {
        let connection_info = loop {
            if connection_info_receiver.changed().await.is_ok() {
                if let Some(data) = connection_info_receiver.borrow().clone() {
                    break data;
                }
            } else {
                return Err(UdpError::NoConnectionInfoReceived);
            }
        };
        let (sink, source) = connect(&mut crypt_state_receiver).await?;

        let sink = Arc::new(Mutex::new(sink));
        let source = Arc::new(Mutex::new(source));

        let phase_watcher = state.read().unwrap().phase_receiver();
        let last_ping_recv = AtomicU64::new(0);

        run_until(
            |phase| matches!(phase, StatePhase::Disconnected),
            join4(
                listen(Arc::clone(&state), Arc::clone(&source), &last_ping_recv),
                send_voice(
                    Arc::clone(&sink),
                    connection_info.socket_addr,
                    phase_watcher.clone(),
                    Arc::clone(&receiver),
                ),
                send_pings(
                    Arc::clone(&state),
                    Arc::clone(&sink),
                    connection_info.socket_addr,
                    &last_ping_recv,
                ),
                new_crypt_state(&mut crypt_state_receiver, sink, source),
            )
            .map(|_| ()),
            phase_watcher,
        )
        .await;

        debug!("Fully disconnected UDP stream, waiting for new connection info");
    }
}

async fn connect(
    crypt_state: &mut mpsc::Receiver<ClientCryptState>,
) -> Result<(UdpSender, UdpReceiver), UdpError> {
    // Bind UDP socket
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16)).await?;

    // Wait for initial CryptState
    let crypt_state = match crypt_state.recv().await {
        Some(crypt_state) => crypt_state,
        // disconnected before we received the CryptSetup packet, oh well
        None => return Err(UdpError::DisconnectBeforeCryptSetup),
    };
    debug!("UDP connected");

    // Wrap the raw UDP packets in Mumble's crypto and voice codec (CryptState does both)
    Ok(UdpFramed::new(udp_socket, crypt_state).split())
}

async fn new_crypt_state(
    crypt_state: &mut mpsc::Receiver<ClientCryptState>,
    sink: Arc<Mutex<UdpSender>>,
    source: Arc<Mutex<UdpReceiver>>,
) {
    loop {
        if let Some(crypt_state) = crypt_state.recv().await {
            info!("Received new crypt state");
            let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
                .await
                .expect("Failed to bind UDP socket");
            let (new_sink, new_source) = UdpFramed::new(udp_socket, crypt_state).split();
            *sink.lock().await = new_sink;
            *source.lock().await = new_source;
        }
    }
}

async fn listen(
    state: Arc<RwLock<State>>,
    source: Arc<Mutex<UdpReceiver>>,
    last_ping_recv: &AtomicU64,
) {
    loop {
        let packet = source.lock().await.next().await.unwrap();
        let (packet, _src_addr) = match packet {
            Ok(packet) => packet,
            Err(err) => {
                warn!("Got an invalid UDP packet: {}", err);
                // To be expected, considering this is the internet, just ignore it
                continue;
            }
        };
        match packet {
            VoicePacket::Ping { timestamp } => {
                state
                    .read()
                    .unwrap()
                    .broadcast_phase(StatePhase::Connected(VoiceStreamType::Udp));
                last_ping_recv.store(timestamp, Ordering::Relaxed);
            }
            VoicePacket::Audio {
                session_id,
                // seq_num,
                payload,
                // position_info,
                ..
            } => {
                state.read().unwrap().audio_output().decode_packet_payload(
                    VoiceStreamType::Udp,
                    session_id,
                    payload,
                );
            }
        }
    }
}

async fn send_pings(
    state: Arc<RwLock<State>>,
    sink: Arc<Mutex<UdpSender>>,
    server_addr: SocketAddr,
    last_ping_recv: &AtomicU64,
) {
    let mut last_send = None;
    let mut interval = interval(Duration::from_millis(1000));

    loop {
        interval.tick().await;
        let last_recv = last_ping_recv.load(Ordering::Relaxed);
        if last_send.is_some() && last_send.unwrap() != last_recv {
            debug!("Sending TCP voice");
            state
                .read()
                .unwrap()
                .broadcast_phase(StatePhase::Connected(VoiceStreamType::Tcp));
        }
        match sink
            .lock()
            .await
            .send((
                VoicePacket::Ping {
                    timestamp: last_recv + 1,
                },
                server_addr,
            ))
            .await
        {
            Ok(_) => {
                last_send = Some(last_recv + 1);
            }
            Err(e) => {
                debug!("Error sending UDP ping: {}", e);
            }
        }
    }
}

async fn send_voice(
    sink: Arc<Mutex<UdpSender>>,
    server_addr: SocketAddr,
    phase_watcher: watch::Receiver<StatePhase>,
    receiver: Arc<Mutex<Box<(dyn Stream<Item = VoicePacket<Serverbound>> + Unpin)>>>,
) {
    loop {
        let mut inner_phase_watcher = phase_watcher.clone();
        loop {
            inner_phase_watcher.changed().await.unwrap();
            if matches!(
                *inner_phase_watcher.borrow(),
                StatePhase::Connected(VoiceStreamType::Udp)
            ) {
                break;
            }
        }
        run_until(
            |phase| !matches!(phase, StatePhase::Connected(VoiceStreamType::Udp)),
            async {
                let mut receiver = receiver.lock().await;
                loop {
                    let sending = (receiver.next().await.unwrap(), server_addr);
                    sink.lock().await.send(sending).await.unwrap();
                }
            },
            phase_watcher.clone(),
        )
        .await;
    }
}

pub(crate) async fn handle_pings(mut ping_request_receiver: mpsc::UnboundedReceiver<PingRequest>) {
    let udp_socket = UdpSocket::bind((Ipv6Addr::from(0u128), 0u16))
        .await
        .expect("Failed to bind UDP socket");

    let pending = Mutex::new(HashMap::new());

    let sender = async {
        while let Some((id, socket_addr, handle)) = ping_request_receiver.recv().await {
            debug!("Sending ping with id {} to {}", id, socket_addr);
            let packet = PingPacket { id };
            let packet: [u8; 12] = packet.into();
            udp_socket.send_to(&packet, &socket_addr).await.unwrap();
            let (tx, rx) = oneshot::channel();
            match pending.lock().await.entry(id) {
                Entry::Occupied(_) => {
                    warn!("Tried to send duplicate ping with id {}", id);
                    continue;
                }
                Entry::Vacant(v) => {
                    v.insert(tx);
                }
            }

            tokio::spawn(async move {
                handle(match timeout(Duration::from_secs(1), rx).await {
                    Ok(Ok(r)) => Some(r),
                    Ok(Err(_)) => {
                        warn!(
                            "Ping response sender for server {}, ping id {} dropped",
                            socket_addr, id
                        );
                        None
                    }
                    Err(_) => {
                        debug!(
                            "Server {} timed out when sending ping id {}",
                            socket_addr, id
                        );
                        None
                    }
                });
            });
        }
    };

    let receiver = async {
        let mut buf = vec![0; 24];

        while let Ok(read) = udp_socket.recv(&mut buf).await {
            if read != 24 {
                warn!("Ping response had length {}, expected 24", read);
                continue;
            }

            let packet = PongPacket::try_from(buf.as_slice()).unwrap();

            match pending.lock().await.entry(packet.id) {
                Entry::Occupied(o) => {
                    let id = *o.key();
                    if o.remove().send(packet).is_err() {
                        debug!("Received response to ping with id {} too late", id);
                    }
                }
                Entry::Vacant(v) => {
                    warn!("Received ping with id {} that we didn't send", v.key());
                }
            }
        }
    };

    debug!("Waiting for ping requests");
    join!(sender, receiver);
}
