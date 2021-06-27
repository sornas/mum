use mumble_protocol::control::msgs;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct User {
    channel: u32,
    comment: Option<String>,
    hash: Option<String>,
    name: String,
    priority_speaker: bool,
    recording: bool,

    suppress: bool,  // by me
    self_mute: bool, // by self
    self_deaf: bool, // by self
    mute: bool,      // by admin
    deaf: bool,      // by admin
}

impl User {
    pub fn new(mut msg: msgs::UserState) -> Self {
        Self {
            channel: msg.get_channel_id(),
            comment: if msg.has_comment() {
                Some(msg.take_comment())
            } else {
                None
            },
            hash: if msg.has_hash() {
                Some(msg.take_hash())
            } else {
                None
            },
            name: msg.take_name(),
            priority_speaker: msg.has_priority_speaker() && msg.get_priority_speaker(),
            recording: msg.has_recording() && msg.get_recording(),
            suppress: msg.has_suppress() && msg.get_suppress(),
            self_mute: msg.has_self_mute() && msg.get_self_mute(),
            self_deaf: msg.has_self_deaf() && msg.get_self_deaf(),
            mute: msg.has_mute() && msg.get_mute(),
            deaf: msg.has_deaf() && msg.get_deaf(),
        }
    }

    pub fn parse_user_state(&mut self, mut msg: msgs::UserState) {
        if msg.has_channel_id() {
            self.channel = msg.get_channel_id();
        }
        if msg.has_comment() {
            self.comment = Some(msg.take_comment());
        }
        if msg.has_hash() {
            self.hash = Some(msg.take_hash());
        }
        if msg.has_name() {
            self.name = msg.take_name();
        }
        if msg.has_priority_speaker() {
            self.priority_speaker = msg.get_priority_speaker();
        }
        if msg.has_recording() {
            self.recording = msg.get_recording();
        }
        if msg.has_suppress() {
            self.suppress = msg.get_suppress();
        }
        if msg.has_self_mute() {
            self.self_mute = msg.get_self_mute();
        }
        if msg.has_self_deaf() {
            self.self_deaf = msg.get_self_deaf();
        }
        if msg.has_mute() {
            self.mute = msg.get_mute();
        }
        if msg.has_deaf() {
            self.deaf = msg.get_deaf();
        }
    }

    pub fn apply_user_diff(&mut self, diff: &UserDiff) {
        if let Some(comment) = diff.comment.clone() {
            self.comment = Some(comment);
        }
        if let Some(hash) = diff.hash.clone() {
            self.hash = Some(hash);
        }
        if let Some(name) = diff.name.clone() {
            self.name = name;
        }
        if let Some(priority_speaker) = diff.priority_speaker {
            self.priority_speaker = priority_speaker;
        }
        if let Some(recording) = diff.recording {
            self.recording = recording;
        }
        if let Some(suppress) = diff.suppress {
            self.suppress = suppress;
        }
        if let Some(self_mute) = diff.self_mute {
            self.self_mute = self_mute;
        }
        if let Some(self_deaf) = diff.self_deaf {
            self.self_deaf = self_deaf;
        }
        if let Some(mute) = diff.mute {
            self.mute = mute;
        }
        if let Some(deaf) = diff.deaf {
            self.deaf = deaf;
        }
        if let Some(channel_id) = diff.channel_id {
            self.channel = channel_id;
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn channel(&self) -> u32 {
        self.channel
    }

    pub fn self_mute(&self) -> bool {
        self.self_mute
    }

    pub fn self_deaf(&self) -> bool {
        self.self_deaf
    }

    pub fn suppressed(&self) -> bool {
        self.suppress
    }

    pub fn set_suppressed(&mut self, value: bool) {
        self.suppress = value;
    }
}

impl From<&User> for mumlib::state::User {
    fn from(user: &User) -> Self {
        mumlib::state::User {
            comment: user.comment.clone(),
            hash: user.hash.clone(),
            name: user.name.clone(),
            priority_speaker: user.priority_speaker,
            recording: user.recording,
            suppress: user.suppress,
            self_mute: user.self_mute,
            self_deaf: user.self_deaf,
            mute: user.mute,
            deaf: user.deaf,
        }
    }
}

#[derive(Debug, Default)]
pub struct UserDiff {
    pub comment: Option<String>,
    pub hash: Option<String>,
    pub name: Option<String>,
    pub priority_speaker: Option<bool>,
    pub recording: Option<bool>,

    pub suppress: Option<bool>,  // by me
    pub self_mute: Option<bool>, // by self
    pub self_deaf: Option<bool>, // by self
    pub mute: Option<bool>,      // by admin
    pub deaf: Option<bool>,      // by admin

    pub channel_id: Option<u32>,
}

impl UserDiff {
    pub fn new() -> Self {
        UserDiff::default()
    }
}

impl From<msgs::UserState> for UserDiff {
    fn from(mut msg: msgs::UserState) -> Self {
        let mut ud = UserDiff::new();
        if msg.has_comment() {
            ud.comment = Some(msg.take_comment());
        }
        if msg.has_hash() {
            ud.hash = Some(msg.take_hash());
        }
        if msg.has_name() {
            ud.name = Some(msg.take_name());
        }
        if msg.has_priority_speaker() {
            ud.priority_speaker = Some(msg.get_priority_speaker());
        }
        if msg.has_recording() {
            ud.recording = Some(msg.get_recording());
        }
        if msg.has_suppress() {
            ud.suppress = Some(msg.get_suppress());
        }
        if msg.has_self_mute() {
            ud.self_mute = Some(msg.get_self_mute());
        }
        if msg.has_self_deaf() {
            ud.self_deaf = Some(msg.get_self_deaf());
        }
        if msg.has_mute() {
            ud.mute = Some(msg.get_mute());
        }
        if msg.has_deaf() {
            ud.deaf = Some(msg.get_deaf());
        }
        if msg.has_channel_id() {
            ud.channel_id = Some(msg.get_channel_id());
        }
        ud
    }
}
