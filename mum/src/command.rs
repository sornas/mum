use crate::network::{tcp::TcpEventQueue, udp::PingRequest, ConnectionInfo};
use crate::state::{ExecutionContext, State};

use log::*;
use mumble_protocol::{control::ControlPacket, Serverbound};
use mumlib::command::{Command, CommandResponse};
use std::{rc::Rc, sync::{Arc, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}}};
use tokio::sync::{mpsc, watch};

pub(crate) async fn handle(
    state: Arc<RwLock<State>>,
    mut command_receiver: mpsc::UnboundedReceiver<(
        Command,
        mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
    )>,
    tcp_event_queue: TcpEventQueue,
    ping_request_sender: mpsc::UnboundedSender<PingRequest>,
    mut packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    mut connection_info_sender: watch::Sender<Option<ConnectionInfo>>,
) {
    debug!("Begin listening for commands");
    let ping_count = AtomicU64::new(0);
    while let Some((command, mut response_sender)) = command_receiver.recv().await {
        debug!("Received command {:?}", command);
        let event = crate::state::handle_command(
            Arc::clone(&state),
            command,
            &mut packet_sender,
            &mut connection_info_sender,
        );
        match event {
            ExecutionContext::TcpEventCallback(callbacks) => {
                // A shared bool ensures that only one of the supplied callbacks is run.
                let should_handle = Rc::new(AtomicBool::new(true));
                for (event, generator) in callbacks {
                    let should_handle = Rc::clone(&should_handle);
                    let response_sender = response_sender.clone();
                    tcp_event_queue.register_callback(
                        event,
                        Box::new(move |e| {
                            // If should_handle == true no other callback has been run yet.
                            if should_handle.swap(false, Ordering::Relaxed) {
                                let response = generator(e);
                                for response in response {
                                    response_sender.send(response).unwrap();
                                }
                            }
                        }),
                    );
                }
            }
            ExecutionContext::TcpEventSubscriber(event, mut handler) => tcp_event_queue
                .register_subscriber(
                    event,
                    Box::new(move |event| handler(event, &mut response_sender)),
                ),
            ExecutionContext::Now(generator) => {
                for response in generator() {
                    response_sender.send(response).unwrap();
                }
                drop(response_sender);
            }
            ExecutionContext::Ping(generator, converter) => {
                let ret = generator();
                debug!("Ping generated: {:?}", ret);
                match ret {
                    Ok(addr) => {
                        let id = ping_count.fetch_add(1, Ordering::Relaxed);
                        let res = ping_request_sender.send((
                            id,
                            addr,
                            Box::new(move |packet| {
                                response_sender.send(converter(packet)).unwrap();
                            }),
                        ));
                        if res.is_err() {
                            panic!();
                        }
                    }
                    Err(e) => {
                        response_sender.send(Err(e)).unwrap();
                    }
                };
            }
        }
    }
}
