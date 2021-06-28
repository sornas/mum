use crate::error::ClientError;
use crate::network::{tcp, udp, ConnectionInfo};
use crate::state::State;
use crate::{command, network::tcp::TcpEventQueue};

use futures_util::{select, FutureExt};
use mumble_protocol::{control::ControlPacket, crypt::ClientCryptState, Serverbound};
use mumlib::command::{Command, CommandResponse};
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, watch};

pub(crate) async fn handle(
    state: State,
    command_receiver: mpsc::UnboundedReceiver<(
        Command,
        mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
) -> Result<(), ClientError> {
    let (connection_info_sender, connection_info_receiver) =
        watch::channel::<Option<ConnectionInfo>>(None);
    let (crypt_state_sender, crypt_state_receiver) = mpsc::channel::<ClientCryptState>(1);
    let (packet_sender, packet_receiver) = mpsc::unbounded_channel::<ControlPacket<Serverbound>>();
    let (ping_request_sender, ping_request_receiver) = mpsc::unbounded_channel();
    let event_queue = TcpEventQueue::new();

    let state = Arc::new(RwLock::new(state));

    select! {
        r = tcp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_sender,
            packet_sender.clone(),
            packet_receiver,
            event_queue.clone(),
        ).fuse() => r.map_err(ClientError::TcpError),
        _ = udp::handle(
            Arc::clone(&state),
            connection_info_receiver.clone(),
            crypt_state_receiver,
        ).fuse() => Ok(()),
        _ = command::handle(
            state,
            command_receiver,
            event_queue,
            ping_request_sender,
            packet_sender,
            connection_info_sender,
        ).fuse() => Ok(()),
        _ = udp::handle_pings(ping_request_receiver).fuse() => Ok(()),
    }
}
