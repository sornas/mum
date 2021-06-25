pub mod channel;
pub mod server;
pub mod user;

use crate::audio::{AudioInput, AudioOutput, sound_effects::NotificationEvent};
use crate::error::StateError;
use crate::network::tcp::{DisconnectedReason, TcpEvent, TcpEventData};
use crate::network::{ConnectionInfo, VoiceStreamType};
use crate::notifications;
use crate::state::server::Server;
use crate::state::user::UserDiff;

use chrono::NaiveDateTime;
use log::*;
use mumble_protocol::control::{ControlPacket, msgs};
use mumble_protocol::ping::PongPacket;
use mumble_protocol::voice::Serverbound;
use mumlib::command::{ChannelTarget, Command, CommandResponse, MessageTarget, MumbleEvent, MumbleEventKind};
use mumlib::config::Config;
use mumlib::Error;
use std::fmt::Debug;
use std::iter;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, watch};

macro_rules! at {
    ( $( $event:expr => $generator:expr ),+ $(,)? ) => {
        ExecutionContext::TcpEventCallback(vec![
            $( ($event, Box::new($generator)), )+
        ])
    };
}

macro_rules! now {
    ($data:expr) => {
        ExecutionContext::Now(Box::new(move || Box::new(iter::once($data))))
    };
}

type Responses = Box<dyn Iterator<Item = mumlib::error::Result<Option<CommandResponse>>>>;

type TcpEventCallback = Box<dyn FnOnce(TcpEventData<'_>) -> Responses>;
type TcpEventSubscriberCallback = Box<
    dyn FnMut(
        TcpEventData<'_>,
        &mut mpsc::UnboundedSender<mumlib::error::Result<Option<CommandResponse>>>,
    ) -> bool
>;

//TODO give me a better name
pub enum ExecutionContext {
    TcpEventCallback(Vec<(TcpEvent, TcpEventCallback)>),
    TcpEventSubscriber(TcpEvent, TcpEventSubscriberCallback),
    Now(Box<dyn FnOnce() -> Responses>),
    Ping(
        Box<dyn FnOnce() -> mumlib::error::Result<SocketAddr>>,
        Box<
            dyn FnOnce(Option<PongPacket>) -> mumlib::error::Result<Option<CommandResponse>> + Send,
        >,
    ),
}

impl Debug for ExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple(match self {
            ExecutionContext::TcpEventCallback(_) => "TcpEventCallback",
            ExecutionContext::TcpEventSubscriber(_, _) => "TcpEventSubscriber",
            ExecutionContext::Now(_) => "Now",
            ExecutionContext::Ping(_, _) => "Ping",
        }).finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatePhase {
    Disconnected,
    Connecting,
    Connected(VoiceStreamType),
}

#[derive(Debug)]
pub struct State {
    config: Config,
    server: Option<Server>,
    audio_input: AudioInput,
    audio_output: AudioOutput,
    message_buffer: Vec<(NaiveDateTime, String, u32)>,

    phase_watcher: (watch::Sender<StatePhase>, watch::Receiver<StatePhase>),

    events: Vec<MumbleEvent>,
}

impl State {
    pub fn new() -> Result<Self, StateError> {
        let config = mumlib::config::read_cfg(&mumlib::config::default_cfg_path())?;
        let phase_watcher = watch::channel(StatePhase::Disconnected);
        let audio_input = AudioInput::new(
            config.audio.input_volume.unwrap_or(1.0),
            phase_watcher.1.clone(),
        )
        .map_err(StateError::AudioError)?;
        let audio_output = AudioOutput::new(config.audio.output_volume.unwrap_or(1.0))
            .map_err(StateError::AudioError)?;
        let mut state = Self {
            config,
            server: None,
            audio_input,
            audio_output,
            message_buffer: Vec::new(),
            phase_watcher,
            events: Vec::new(),
        };
        state.reload_config();
        Ok(state)
    }

    pub fn parse_user_state(&mut self, msg: msgs::UserState) {
        if !msg.has_session() {
            warn!("Can't parse user state without session");
            return;
        }
        let session = msg.get_session();
        // check if this is initial state
        if !self.server().unwrap().users().contains_key(&session) {
            self.create_user(msg);
        } else {
            self.update_user(msg);
        }
    }

    fn create_user(&mut self, msg: msgs::UserState) {
        if !msg.has_name() {
            warn!("Missing name in initial user state");
            return;
        }

        let session = msg.get_session();

        if msg.get_name() == self.server().unwrap().username().unwrap() {
            // this is us
            *self.server_mut().unwrap().session_id_mut() = Some(session);
        } else {
            // this is someone else
            // send notification only if we've passed the connecting phase
            if matches!(*self.phase_receiver().borrow(), StatePhase::Connected(_)) {
                let this_channel = msg.get_channel_id();
                let other_channel = self.get_users_channel(self.server().unwrap().session_id().unwrap());
                let this_channel_name = self
                    .server()
                    .unwrap()
                    .channels()
                    .get(&this_channel)
                    .map(|c| c.name())
                    .unwrap_or("<unnamed channel>")
                    .to_string();

                if this_channel == other_channel {
                    notifications::send(format!(
                        "{} connected and joined {}",
                        msg.get_name(),
                        this_channel_name,
                    ));
                    self.push_event(MumbleEventKind::UserConnected(msg.get_name().to_string(), this_channel_name));
                    self.audio_output.play_effect(NotificationEvent::UserConnected);
                }
            }
        }

        self.server_mut()
            .unwrap()
            .users_mut()
            .insert(session, user::User::new(msg));
    }

    fn update_user(&mut self, msg: msgs::UserState) {
        let session = msg.get_session();

        let from_channel = self.get_users_channel(session);

        let user = self
            .server_mut()
            .unwrap()
            .users_mut()
            .get_mut(&session)
            .unwrap();
        let username = user.name().to_string();

        let mute = if msg.has_self_mute() && user.self_mute() != msg.get_self_mute() {
            Some(msg.get_self_mute())
        } else {
            None
        };
        let deaf = if msg.has_self_deaf() && user.self_deaf() != msg.get_self_deaf() {
            Some(msg.get_self_deaf())
        } else {
            None
        };

        let diff = UserDiff::from(msg);
        user.apply_user_diff(&diff);


        if Some(session) != self.server().unwrap().session_id() {
            // Send notification if the user moved either to or from our channel
            if let Some(to_channel) = diff.channel_id {
                let this_channel =
                    self.get_users_channel(self.server().unwrap().session_id().unwrap());

                if from_channel == this_channel {
                    // User moved from our channel to somewhere else
                    if let Some(channel) = self.server().unwrap().channels().get(&to_channel) {
                        let channel = channel.name().to_string();
                        notifications::send(format!(
                            "{} moved to channel {}",
                            &username,
                            &channel,
                        ));
                        self.push_event(MumbleEventKind::UserLeftChannel(username.clone(), channel));
                    }
                    self.audio_output.play_effect(NotificationEvent::UserLeftChannel);
                } else if to_channel == this_channel {
                    // User moved from somewhere else to our channel
                    if let Some(channel) = self.server().unwrap().channels().get(&from_channel) {
                        let channel = channel.name().to_string();
                        notifications::send(format!(
                            "{} moved to your channel from {}",
                            &username,
                            &channel,
                        ));
                        self.push_event(MumbleEventKind::UserJoinedChannel(username.clone(), channel));
                    }
                    self.audio_output.play_effect(NotificationEvent::UserJoinedChannel);
                }
            }

            //send notification if a user muted/unmuted
            if mute != None || deaf != None {
                let mut s = username;
                if let Some(mute) = mute {
                    s += if mute { " muted" } else { " unmuted" };
                }
                if mute.is_some() && deaf.is_some() {
                    s += " and";
                }
                if let Some(deaf) = deaf {
                    s += if deaf { " deafened" } else { " undeafened" };
                }
                s += " themselves";
                notifications::send(s.clone());
                self.push_event(MumbleEventKind::UserMuteStateChanged(s));
            }
        }
    }

    pub fn remove_client(&mut self, msg: msgs::UserRemove) {
        if !msg.has_session() {
            warn!("Tried to remove user state without session");
            return;
        }

        let this_channel = self.get_users_channel(self.server().unwrap().session_id().unwrap());
        let other_channel = self.get_users_channel(msg.get_session());
        if this_channel == other_channel {
            let channel_name = self
                .server()
                .unwrap()
                .channels()
                .get(&this_channel)
                .map(|c| c.name())
                .unwrap_or("<unnamed channel>")
                .to_string();
            let user_name = self
                .server()
                .unwrap()
                .users()
                .get(&msg.get_session())
                .map(|u| u.name())
                .unwrap_or("<unknown user>")
                .to_string();
            notifications::send(format!("{} disconnected", &user_name));
            self.push_event(MumbleEventKind::UserDisconnected(user_name, channel_name));
            self.audio_output.play_effect(NotificationEvent::UserDisconnected);
        }

        self.server_mut()
            .unwrap()
            .users_mut()
            .remove(&msg.get_session());
        info!("User {} disconnected", msg.get_session());
    }

    pub fn reload_config(&mut self) {
        match mumlib::config::read_cfg(&mumlib::config::default_cfg_path()) {
            Ok(config) => {
                self.config = config;
            }
            Err(e) => error!("Couldn't read config: {}", e),
        }
        if let Some(input_volume) = self.config.audio.input_volume {
            self.audio_input.set_volume(input_volume);
        }
        if let Some(output_volume) = self.config.audio.output_volume {
            self.audio_output.set_volume(output_volume);
        }
        if let Some(sound_effects) = &self.config.audio.sound_effects {
            self.audio_output.set_sound_effects(sound_effects);
        }
    }

    pub fn register_message(&mut self, msg: (String, u32)) {
        self.message_buffer.push((chrono::Local::now().naive_local(), msg.0, msg.1));
    }

    pub fn broadcast_phase(&self, phase: StatePhase) {
        self.phase_watcher.0.send(phase).unwrap();
    }

    pub fn initialized(&self) {
        self.broadcast_phase(StatePhase::Connected(VoiceStreamType::Tcp));
        self.audio_output
            .play_effect(NotificationEvent::ServerConnect);
    }

    /// Store a new event
    pub fn push_event(&mut self, kind: MumbleEventKind) {
        self.events.push(MumbleEvent { timestamp: chrono::Local::now().naive_local(), kind });
    }

    pub fn audio_input(&self) -> &AudioInput {
        &self.audio_input
    }
    pub fn audio_output(&self) -> &AudioOutput {
        &self.audio_output
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
    pub fn password(&self) -> Option<&str> {
        self.server.as_ref().map(|e| e.password()).flatten()
    }
    fn get_users_channel(&self, user_id: u32) -> u32 {
        self.server()
            .unwrap()
            .users()
            .iter()
            .find(|e| *e.0 == user_id)
            .unwrap()
            .1
            .channel()
    }

    /// Gets the username of a user with id `user` connected to the same server that we are connected to.
    /// If we are connected to the server but the user with the id doesn't exist, the string "Unknown user {id}"
    /// is returned instead. If we aren't connected to a server, None is returned instead.
    fn get_user_name(&self, user: u32) -> Option<String> {
        self.server().map(|e| {
            e.users()
                .get(&user)
                .map(|e| e.name().to_string())
                .unwrap_or(format!("Unknown user {}", user))
        })
    }
}

pub fn handle_command(
    og_state: Arc<RwLock<State>>,
    command: Command,
    packet_sender: &mut mpsc::UnboundedSender<ControlPacket<Serverbound>>,
    connection_info_sender: &mut watch::Sender<Option<ConnectionInfo>>,
) -> ExecutionContext {
    let mut state = og_state.write().unwrap();
    match command {
        Command::ChannelJoin { channel_identifier } => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            let id = match state.server().unwrap().channel_name(&channel_identifier) {
                Ok((id, _)) => id,
                Err(e) => return now!(Err(Error::ChannelIdentifierError(channel_identifier, e))),
            };

            let mut msg = msgs::UserState::new();
            msg.set_session(state.server.as_ref().unwrap().session_id().unwrap());
            msg.set_channel_id(id);
            packet_sender.send(msg.into()).unwrap();
            now!(Ok(None))
        }
        Command::ChannelList => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }
            let list = channel::into_channel(
                state.server.as_ref().unwrap().channels(),
                state.server.as_ref().unwrap().users(),
            );
            now!(Ok(Some(CommandResponse::ChannelList { channels: list })))
        }
        Command::ConfigReload => {
            state.reload_config();
            now!(Ok(None))
        }
        Command::DeafenSelf(toggle) => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            let server = state.server().unwrap();
            let action = match (toggle, server.muted(), server.deafened()) {
                (Some(false), false, false) => None,
                (Some(false), false, true) => Some((false, false)),
                (Some(false), true, false) => None,
                (Some(false), true, true) => Some((true, false)),
                (Some(true), false, false) => Some((false, true)),
                (Some(true), false, true) => None,
                (Some(true), true, false) => Some((true, true)),
                (Some(true), true, true) => None,
                (None, false, false) => Some((false, true)),
                (None, false, true) => Some((false, false)),
                (None, true, false) => Some((true, true)),
                (None, true, true) => Some((true, false)),
            };

            let mut new_deaf = None;
            if let Some((mute, deafen)) = action {
                if server.deafened() != deafen {
                    state.audio_output.play_effect(if deafen {
                        NotificationEvent::Deafen
                    } else {
                        NotificationEvent::Undeafen
                    });
                } else if server.muted() != mute {
                    state.audio_output.play_effect(if mute {
                        NotificationEvent::Mute
                    } else {
                        NotificationEvent::Unmute
                    });
                }
                let mut msg = msgs::UserState::new();
                if server.muted() != mute {
                    msg.set_self_mute(mute);
                } else if !mute && !deafen && server.deafened() {
                    msg.set_self_mute(false);
                }
                if server.deafened() != deafen {
                    msg.set_self_deaf(deafen);
                    new_deaf = Some(deafen);
                }
                let server = state.server_mut().unwrap();
                server.set_muted(mute);
                server.set_deafened(deafen);
                packet_sender.send(msg.into()).unwrap();
            }

            now!(Ok(
                new_deaf.map(|b| CommandResponse::DeafenStatus { is_deafened: b })
            ))
        }
        Command::Events { block } => {
            if block {
                warn!("Blocking event list is unimplemented");
                now!(Err(Error::Unimplemented))
            } else {
                let events: Vec<_> = state
                    .events
                    .iter()
                    .map(|event| Ok(Some(CommandResponse::Event { event: event.clone() })))
                    .collect();
                ExecutionContext::Now(Box::new(move || Box::new(events.into_iter())))
            }
        }
        Command::InputVolumeSet(volume) => {
            state.audio_input.set_volume(volume);
            now!(Ok(None))
        }
        Command::MuteOther(string, toggle) => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            let id = state
                .server_mut()
                .unwrap()
                .users_mut()
                .iter_mut()
                .find(|(_, user)| user.name() == string);

            let (id, user) = match id {
                Some(id) => (*id.0, id.1),
                None => return now!(Err(Error::InvalidUsername(string))),
            };

            let action = match toggle {
                Some(state) => {
                    if user.suppressed() != state {
                        Some(state)
                    } else {
                        None
                    }
                }
                None => Some(!user.suppressed()),
            };

            if let Some(action) = action {
                user.set_suppressed(action);
                state.audio_output.set_mute(id, action);
            }

            return now!(Ok(None));
        }
        Command::MuteSelf(toggle) => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            let server = state.server().unwrap();
            let action = match (toggle, server.muted(), server.deafened()) {
                (Some(false), false, false) => None,
                (Some(false), false, true) => Some((false, false)),
                (Some(false), true, false) => Some((false, false)),
                (Some(false), true, true) => Some((false, false)),
                (Some(true), false, false) => Some((true, false)),
                (Some(true), false, true) => None,
                (Some(true), true, false) => None,
                (Some(true), true, true) => None,
                (None, false, false) => Some((true, false)),
                (None, false, true) => Some((false, false)),
                (None, true, false) => Some((false, false)),
                (None, true, true) => Some((false, false)),
            };

            let mut new_mute = None;
            if let Some((mute, deafen)) = action {
                if server.deafened() != deafen {
                    state.audio_output.play_effect(if deafen {
                        NotificationEvent::Deafen
                    } else {
                        NotificationEvent::Undeafen
                    });
                } else if server.muted() != mute {
                    state.audio_output.play_effect(if mute {
                        NotificationEvent::Mute
                    } else {
                        NotificationEvent::Unmute
                    });
                }
                let mut msg = msgs::UserState::new();
                if server.muted() != mute {
                    msg.set_self_mute(mute);
                    new_mute = Some(mute)
                } else if !mute && !deafen && server.deafened() {
                    msg.set_self_mute(false);
                    new_mute = Some(false)
                }
                if server.deafened() != deafen {
                    msg.set_self_deaf(deafen);
                }
                let server = state.server_mut().unwrap();
                server.set_muted(mute);
                server.set_deafened(deafen);
                packet_sender.send(msg.into()).unwrap();
            }

            now!(Ok(
                new_mute.map(|b| CommandResponse::MuteStatus { is_muted: b })
            ))
        }
        Command::OutputVolumeSet(volume) => {
            state.audio_output.set_volume(volume);
            now!(Ok(None))
        }
        Command::Ping => {
            now!(Ok(Some(CommandResponse::Pong)))
        }
        Command::ServerConnect {
            host,
            port,
            username,
            password,
            accept_invalid_cert,
        } => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Disconnected) {
                return now!(Err(Error::AlreadyConnected));
            }
            let mut server = Server::new();
            *server.username_mut() = Some(username);
            *server.password_mut() = password;
            *server.host_mut() = Some(format!("{}:{}", host, port));
            state.server = Some(server);
            state.phase_watcher.0.send(StatePhase::Connecting).unwrap();

            let socket_addr = match (host.as_ref(), port)
                .to_socket_addrs()
                .map(|mut e| e.next())
            {
                Ok(Some(v)) => v,
                _ => {
                    warn!("Error parsing server addr");
                    return now!(Err(Error::InvalidServerAddr(host, port)));
                }
            };
            connection_info_sender
                .send(Some(ConnectionInfo::new(
                    socket_addr,
                    host,
                    accept_invalid_cert,
                )))
                .unwrap();
            let state = Arc::clone(&og_state);
            at!(
                TcpEvent::Connected => move |res| {
                    //runs the closure when the client is connected
                    if let TcpEventData::Connected(res) = res {
                        Box::new(iter::once(res.map(|msg| {
                            Some(CommandResponse::ServerConnect {
                                welcome_message: if msg.has_welcome_text() {
                                    Some(msg.get_welcome_text().to_string())
                                } else {
                                    None
                                },
                                server_state: state.read().unwrap().server.as_ref().unwrap().into(),
                            })
                        })))
                    } else {
                        unreachable!("callback should be provided with a TcpEventData::Connected");
                    }
                },
                TcpEvent::Disconnected(DisconnectedReason::InvalidTls) => |_| {
                    Box::new(iter::once(Err(Error::ServerCertReject)))
                }
            )
        }
        Command::ServerDisconnect => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            state.server = None;

            state
                .phase_watcher
                .0
                .send(StatePhase::Disconnected)
                .unwrap();
            state
                .audio_output
                .play_effect(NotificationEvent::ServerDisconnect);
            now!(Ok(None))
        }
        Command::ServerStatus { host, port } => ExecutionContext::Ping(
            Box::new(move || {
                match (host.as_str(), port)
                    .to_socket_addrs()
                    .map(|mut e| e.next())
                {
                    Ok(Some(v)) => Ok(v),
                    _ => Err(Error::InvalidServerAddr(host, port)),
                }
            }),
            Box::new(move |pong| {
                Ok(pong.map(|pong| {
                    CommandResponse::ServerStatus {
                        version: pong.version,
                        users: pong.users,
                        max_users: pong.max_users,
                        bandwidth: pong.bandwidth,
                    }
                }))
            }),
        ),
        Command::Status => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }
            let state = state.server.as_ref().unwrap().into();
            now!(Ok(Some(CommandResponse::Status {
                server_state: state, //guaranteed not to panic because if we are connected, server is guaranteed to be Some
            })))
        }
        Command::UserVolumeSet(string, volume) => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }
            let user_id = match state
                .server()
                .unwrap()
                .users()
                .iter()
                .find(|e| e.1.name() == string)
                .map(|e| *e.0)
            {
                None => return now!(Err(Error::InvalidUsername(string))),
                Some(v) => v,
            };

            state.audio_output.set_user_volume(user_id, volume);
            now!(Ok(None))
        }
        Command::PastMessages { block } => {
            //does it make sense to wait for messages while not connected?
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }
            if block {
                let ref_state = Arc::clone(&og_state);
                ExecutionContext::TcpEventSubscriber(
                    TcpEvent::TextMessage,
                    Box::new(move |data, sender| {
                        if let TcpEventData::TextMessage(a) = data {
                            let message = (
                                chrono::Local::now().naive_local(),
                                a.get_message().to_owned(),
                                ref_state
                                    .read()
                                    .unwrap()
                                    .get_user_name(a.get_actor())
                                    .unwrap(),
                            );
                            sender
                                .send(Ok(Some(CommandResponse::PastMessage { message })))
                                .is_ok()
                        } else {
                            unreachable!("Should only receive a TextMessage data when listening to TextMessage events");
                        }
                    }),
                )
            } else {
                let messages = std::mem::take(&mut state.message_buffer);
                let messages: Vec<_> = messages
                    .into_iter()
                    .map(|(timestamp, msg, user)| (timestamp, msg, state.get_user_name(user).unwrap()))
                    .map(|e| Ok(Some(CommandResponse::PastMessage { message: e })))
                    .collect();

                ExecutionContext::Now(Box::new(move || Box::new(messages.into_iter())))
            }
        }
        Command::SendMessage { message, targets } => {
            if !matches!(*state.phase_receiver().borrow(), StatePhase::Connected(_)) {
                return now!(Err(Error::Disconnected));
            }

            let mut msg = msgs::TextMessage::new();

            msg.set_message(message);

            match targets {
                MessageTarget::Channel(channels) => for (channel, recursive) in channels {
                    let channel_id = if let ChannelTarget::Named(name) = channel {
                        let channel = state.server().unwrap().channel_name(&name);
                        match channel {
                            Ok(channel) => channel.0,
                            Err(e) => return now!(Err(Error::ChannelIdentifierError(name, e))),
                        }
                    } else {
                        match state.server().unwrap().current_channel() {
                            Some(channel) => channel.0,
                            None => return now!(Err(Error::NotConnectedToChannel)),
                        }
                    };

                    let ids = if recursive {
                        msg.mut_tree_id()
                    } else {
                        msg.mut_channel_id()
                    };
                    ids.push(channel_id);
                }
                MessageTarget::User(names) => for name in names {
                    let id = state
                        .server()
                        .unwrap()
                        .users()
                        .iter()
                        .find(|(_, user)| user.name() == name)
                        .map(|(e, _)| *e);

                    let id = match id {
                        Some(id) => id,
                        None => return now!(Err(Error::InvalidUsername(name))),
                    };

                    msg.mut_session().push(id);
                }
            }
            packet_sender.send(msg.into()).unwrap();

            now!(Ok(None))
        }
    }
}
