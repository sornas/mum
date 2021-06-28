use crate::state::channel::{into_channel, Channel};
use crate::state::user::User;

use log::*;
use mumble_protocol::control::msgs;
use mumlib::error::ChannelIdentifierError;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum Server {
    Connected(ConnectedServer),
    Connecting(ConnectingServer),
    Disconnected,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ConnectingServer {}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ConnectedServer {
    channels: HashMap<u32, Channel>,
    users: HashMap<u32, User>,
    pub(crate) welcome_text: Option<String>,

    username: Option<String>,
    password: Option<String>,
    session_id: Option<u32>,
    muted: bool,
    deafened: bool,

    host: Option<String>,
}

impl ConnectedServer {
    pub(crate) fn new() -> Self {
        Self {
            channels: HashMap::new(),
            users: HashMap::new(),
            welcome_text: None,
            username: None,
            password: None,
            session_id: None,
            muted: false,
            deafened: false,
            host: None,
        }
    }

    pub(crate) fn parse_server_sync(&mut self, mut msg: msgs::ServerSync) {
        if msg.has_welcome_text() {
            self.welcome_text = Some(msg.take_welcome_text());
        }
    }

    pub(crate) fn parse_channel_state(&mut self, msg: msgs::ChannelState) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel state without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(e) => {
                e.insert(Channel::new(msg));
            }
            Entry::Occupied(mut e) => e.get_mut().parse_channel_state(msg),
        }
    }

    pub(crate) fn parse_channel_remove(&mut self, msg: msgs::ChannelRemove) {
        if !msg.has_channel_id() {
            warn!("Can't parse channel remove without channel id");
            return;
        }
        match self.channels.entry(msg.get_channel_id()) {
            Entry::Vacant(_) => {
                warn!("Attempted to remove channel that doesn't exist");
            }
            Entry::Occupied(e) => {
                e.remove();
            }
        }
    }

    pub(crate) fn channels(&self) -> &HashMap<u32, Channel> {
        &self.channels
    }

    /// Takes a channel name and returns either a tuple with the channel id and a reference to the
    /// channel struct if the channel name unambiguosly refers to a channel, or an error describing
    /// if the channel identifier was ambigous or invalid.
    /// note that doctests currently aren't run in binary crates yet (see #50784)
    /// ```
    /// use crate::state::channel::Channel;
    /// let mut server = ConnectedServer::new();
    /// let channel = Channel {
    ///     name: "Foobar".to_owned(),
    ///     ..Default::default(),
    /// };
    /// server.channels.insert(0, channel.clone);
    /// assert_eq!(server.channel_name("Foobar"), Ok((0, &channel)));
    /// ```
    pub(crate) fn channel_name(
        &self,
        channel_name: &str,
    ) -> Result<(u32, &Channel), ChannelIdentifierError> {
        let matches = self
            .channels
            .iter()
            .map(|e| ((*e.0, e.1), e.1.path(&self.channels)))
            .filter(|e| e.1.ends_with(channel_name))
            .collect::<Vec<_>>();
        Ok(match matches.len() {
            0 => {
                let soft_matches = self
                    .channels
                    .iter()
                    .map(|e| ((*e.0, e.1), e.1.path(&self.channels).to_lowercase()))
                    .filter(|e| e.1.ends_with(&channel_name.to_lowercase()))
                    .collect::<Vec<_>>();
                match soft_matches.len() {
                    0 => return Err(ChannelIdentifierError::Invalid),
                    1 => soft_matches.get(0).unwrap().0,
                    _ => return Err(ChannelIdentifierError::Ambiguous),
                }
            }
            1 => matches.get(0).unwrap().0,
            _ => return Err(ChannelIdentifierError::Ambiguous),
        })
    }

    /// Returns the currenctly connected channel.
    ///
    /// Returns None if not connected.
    pub(crate) fn current_channel(&self) -> Option<(u32, &Channel)> {
        let channel_id = self.users().get(&self.session_id()?)?.channel();
        let channel = self.channels().get(&channel_id)?;
        Some((channel_id, channel))
    }

    pub(crate) fn host_mut(&mut self) -> &mut Option<String> {
        &mut self.host
    }

    pub(crate) fn session_id(&self) -> Option<u32> {
        self.session_id
    }

    pub(crate) fn session_id_mut(&mut self) -> &mut Option<u32> {
        &mut self.session_id
    }

    pub(crate) fn users(&self) -> &HashMap<u32, User> {
        &self.users
    }

    pub(crate) fn users_mut(&mut self) -> &mut HashMap<u32, User> {
        &mut self.users
    }

    pub(crate) fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub(crate) fn username_mut(&mut self) -> &mut Option<String> {
        &mut self.username
    }

    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub(crate) fn password_mut(&mut self) -> &mut Option<String> {
        &mut self.password
    }

    pub(crate) fn muted(&self) -> bool {
        self.muted
    }

    pub(crate) fn deafened(&self) -> bool {
        self.deafened
    }

    pub(crate) fn set_muted(&mut self, value: bool) {
        self.muted = value;
    }

    pub(crate) fn set_deafened(&mut self, value: bool) {
        self.deafened = value;
    }
}

impl From<&ConnectedServer> for mumlib::state::Server {
    fn from(server: &ConnectedServer) -> Self {
        mumlib::state::Server {
            channels: into_channel(server.channels(), server.users()),
            welcome_text: server.welcome_text.clone(),
            username: server.username.clone().unwrap(),
            host: server.host.as_ref().unwrap().clone(),
        }
    }
}
