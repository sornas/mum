pub mod tcp;
pub mod udp;

use futures_util::FutureExt;
use log::*;
use std::{future::Future, net::SocketAddr};
use tokio::{
    select,
    sync::{oneshot, watch},
};

use crate::state::StatePhase;

#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    socket_addr: SocketAddr,
    hostname: String,
    accept_invalid_cert: bool,
}

impl ConnectionInfo {
    pub fn new(socket_addr: SocketAddr, hostname: String, accept_invalid_cert: bool) -> Self {
        Self {
            socket_addr,
            hostname,
            accept_invalid_cert,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VoiceStreamType {
    Tcp,
    Udp,
}

async fn run_until<F, R>(
    phase_checker: impl Fn(StatePhase) -> bool,
    fut: F,
    mut phase_watcher: watch::Receiver<StatePhase>,
) -> Option<R>
where
    F: Future<Output = R>,
{
    let (tx, rx) = oneshot::channel();
    let phase_transition_block = async {
        loop {
            phase_watcher.changed().await.unwrap();
            if phase_checker(*phase_watcher.borrow()) {
                break;
            }
        }
        if tx.send(true).is_err() {
            warn!("future resolved before it could be cancelled");
        }
    };

    let main_block = async {
        let rx = rx.fuse();
        let fut = fut.fuse();
        select! {
            r = fut => Some(r),
            _ = rx => None,
        }
    };

    select! {
        m = main_block => m,
        _ = phase_transition_block => None,
    }
}
