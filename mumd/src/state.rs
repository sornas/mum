pub mod server;
pub mod channel;
pub mod user;

use crate::audio::Audio;
use crate::network::ConnectionInfo;
use crate::state::server::Server;

use log::*;
use mumble_protocol::control::msgs;
use mumble_protocol::control::ControlPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{Command, CommandResponse};
use mumlib::config::Config;
use mumlib::error::{ChannelIdentifierError, Error};
use std::net::ToSocketAddrs;
use tokio::sync::{mpsc, watch};
use crate::network::tcp::{TcpEvent, TcpEventData};

macro_rules! at {
    ($event:expr, $generator:expr) => {
        (Some($event), Box::new($generator))
    };
}

macro_rules! now {
    ($data:expr) => {
        (None, Box::new(move |_| $data))
    };
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected,
}

pub struct State {
    config: Option<Config>,
    server: Option<Server>,
    audio: Audio,

    packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    connection_info_sender: watch::Sender<Option<ConnectionInfo>>,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),
}

impl State {
    pub fn new(
        packet_sender: mpsc::UnboundedSender<ControlPacket<Serverbound>>,
        connection_info_sender: watch::Sender<Option<ConnectionInfo>>,
    ) -> Self {
        let audio = Audio::new();
        let mut state = Self {
            config: mumlib::config::read_default_cfg(),
            server: None,
            audio,
            packet_sender,
            connection_info_sender,
            phase_watcher: watch::channel(StatePhase::Disconnected),
        };
        state.reload_config();
        state
    }

    //TODO? move bool inside Result
    pub fn handle_command(
        &mut self,
        command: Command,
    ) -> (Option<TcpEvent>, Box<dyn FnOnce(Option<&TcpEventData>) -> mumlib::error::Result<Option<CommandResponse>>>) {
        match command {
            Command::ChannelJoin { channel_identifier } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                let channels = self.server()
                    .unwrap()
                    .channels();

                let matches = channels.iter()
                    .map(|e| (e.0, e.1.path(channels)))
                    .filter(|e| e.1.ends_with(&channel_identifier))
                    .collect::<Vec<_>>();
                let id = match matches.len() {
                    0 => {
                        let soft_matches = channels.iter()
                            .map(|e| (e.0, e.1.path(channels).to_lowercase()))
                            .filter(|e| e.1.ends_with(&channel_identifier.to_lowercase()))
                            .collect::<Vec<_>>();
                        match soft_matches.len() {
                            0 => return now!(Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Invalid))),
                            1 => *soft_matches.get(0).unwrap().0,
                            _ => return now!(Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Invalid))),
                        }
                    },
                    1 => *matches.get(0).unwrap().0,
                    _ => return now!(Err(Error::ChannelIdentifierError(channel_identifier, ChannelIdentifierError::Ambiguous))),
                };

                let mut msg = msgs::UserState::new();
                msg.set_session(self.server.as_ref().unwrap().session_id().unwrap());
                msg.set_channel_id(id);
                self.packet_sender.send(msg.into()).unwrap();
                now!(Ok(None))
            }
            Command::ChannelList => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return (None, Box::new(|_| Err(Error::DisconnectedError)));
                }
                let list = channel::into_channel(
                    self.server.as_ref().unwrap().channels(),
                    self.server.as_ref().unwrap().users(),
                );
                now!(
                    Ok(Some(CommandResponse::ChannelList {
                        channels: list,
                    }))
                )
            }
            Command::ServerConnect {
                host,
                port,
                username,
                accept_invalid_cert,
            } => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Disconnected) {
                    return now!(Err(Error::AlreadyConnectedError));
                }
                let mut server = Server::new();
                *server.username_mut() = Some(username);
                *server.host_mut() = Some(format!("{}:{}", host, port));
                self.server = Some(server);
                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Connecting)
                    .unwrap();

                let socket_addr = match (host.as_ref(), port)
                    .to_socket_addrs()
                    .map(|mut e| e.next())
                {
                    Ok(Some(v)) => v,
                    _ => {
                        warn!("Error parsing server addr");
                        return now!(Err(Error::InvalidServerAddrError(host, port)));
                    }
                };
                self.connection_info_sender
                    .broadcast(Some(ConnectionInfo::new(
                        socket_addr,
                        host,
                        accept_invalid_cert,
                    )))
                    .unwrap();
                at!(TcpEvent::Connected, |e| { //runs the closure when the client is connected
                    if let Some(TcpEventData::Connected(msg)) = e {
                        Ok(Some(CommandResponse::ServerConnect {
                            welcome_message: if msg.has_welcome_text() {
                                Some(msg.get_welcome_text().to_string())
                            } else {
                                None
                            }
                        }))
                    } else {
                        unreachable!("callback should be provided with a TcpEventData::Connected");
                    }
                })
            }
            Command::Status => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }
                let state = self.server.as_ref().unwrap().into();
                now!(
                    Ok(Some(CommandResponse::Status {
                        server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
                    }))
                )
            }
            Command::ServerDisconnect => {
                if !matches!(*self.phase_receiver().borrow(), StatePhase::Connected) {
                    return now!(Err(Error::DisconnectedError));
                }

                self.server = None;
                self.audio.clear_clients();

                self.phase_watcher
                    .0
                    .broadcast(StatePhase::Disconnected)
                    .unwrap();
                now!(Ok(None))
            }
            Command::InputVolumeSet(volume) => {
                self.audio.set_input_volume(volume);
                now!(Ok(None))
            }
            Command::ConfigReload => {
                self.reload_config();
                now!(Ok(None))
            }
        }
    }

    pub fn parse_user_state(&mut self, msg: msgs::UserState) -> Option<mumlib::state::UserDiff> {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return None;
        }
        let session = msg.get_session();
        // check if this is initial state
        if !self.server().unwrap().users().contains_key(&session) {
            self.parse_initial_user_state(session, msg);
            None
        } else {
            Some(self.parse_updated_user_state(session, msg))
        }
    }

    fn parse_initial_user_state(&mut self, session: u32, msg: msgs::UserState) {
        if !msg.has_name() {
            warn!("Missing name in initial user state");
        } else if msg.get_name() == self.server().unwrap().username().unwrap() {
            // this is us
            *self.server_mut().unwrap().session_id_mut() = Some(session);
        } else {
            // this is someone else
            self.audio_mut().add_client(session);

            // send notification only if we've passed the connecting phase
            if *self.phase_receiver().borrow() == StatePhase::Connected {
                let channel_id = if msg.has_channel_id() {
                    msg.get_channel_id()
                } else {
                    0
                };
                if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                    libnotify::Notification::new("mumd",
                                                 Some(format!("{} connected and joined {}",
                                                              &msg.get_name(),
                                                              channel.name()).as_str()),
                                                 None)
                        .show().unwrap();
                }
            }
        }
        self.server_mut().unwrap().users_mut().insert(session, user::User::new(msg));
    }

    fn parse_updated_user_state(&mut self, session: u32, msg: msgs::UserState) -> mumlib::state::UserDiff {
        let user = self.server_mut().unwrap().users_mut().get_mut(&session).unwrap();
        let diff = mumlib::state::UserDiff::from(msg);
        user.apply_user_diff(&diff);
        let user = self.server().unwrap().users().get(&session).unwrap();

        // send notification
        if let Some(channel_id) = diff.channel_id {
            if let Some(channel) = self.server().unwrap().channels().get(&channel_id) {
                libnotify::Notification::new("mumd",
                                                Some(format!("{} moved to channel {}",
                                                            &user.name(),
                                                            channel.name()).as_str()),
                                                None)
                    .show().unwrap();
            } else {
                warn!("{} moved to invalid channel {}", &user.name(), channel_id);
            }
        }

        diff
    }

    pub fn remove_client(&mut self, msg: msgs::UserRemove) {
        if !msg.has_session() {
            warn!("Tried to remove user state without session");
            return;
        }
        if let Some(user) = self.server().unwrap().users().get(&msg.get_session()) {
            libnotify::Notification::new("mumd",
                                         Some(format!("{} disconnected",
                                                      &user.name()).as_str()),
                                         None)
                .show().unwrap();
        }

        self.audio().remove_client(msg.get_session());
        self.server_mut().unwrap().users_mut().remove(&msg.get_session());
        info!("User {} disconnected", msg.get_session());
    }

    pub fn reload_config(&mut self) {
        if let Some(config) = mumlib::config::read_default_cfg() {
            self.config = Some(config);
            let config = &self.config.as_ref().unwrap();
            if let Some(audio_config) = &config.audio {
                if let Some(input_volume) = audio_config.input_volume {
                    self.audio.set_input_volume(input_volume);
                }
            }
        } else {
            warn!("config file not found");
        }
    }

    pub fn initialized(&self) {
        self.phase_watcher
            .0
            .broadcast(StatePhase::Connected)
            .unwrap();
    }

    pub fn audio(&self) -> &Audio {
        &self.audio
    }
    pub fn audio_mut(&mut self) -> &mut Audio {
        &mut self.audio
    }
    pub fn packet_sender(&self) -> mpsc::UnboundedSender<ControlPacket<Serverbound>> {
        self.packet_sender.clone()
    }
    pub fn phase_receiver(&self) -> watch::Receiver<StatePhase> {
        self.phase_watcher.1.clone()
    }
    pub fn server(&self) -> Option<&Server> {
        self.server.as_ref()
    }
    pub fn server_mut(&mut self) -> Option<&mut Server> {
        self.server.as_mut()
    }
    pub fn username(&self) -> Option<&str> {
        self.server.as_ref().map(|e| e.username()).flatten()
    }
}
