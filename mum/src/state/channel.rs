use crate::state::user::User;

use mumble_protocol::control::msgs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Channel {
    description: Option<String>,
    links: Vec<u32>,
    max_users: u32,
    name: String,
    parent: Option<u32>,
    position: i32,
}

impl Channel {
    pub fn new(mut msg: msgs::ChannelState) -> Self {
        Self {
            description: if msg.has_description() {
                Some(msg.take_description())
            } else {
                None
            },
            links: Vec::new(),
            max_users: msg.get_max_users(),
            name: msg.take_name(),
            parent: if msg.has_parent() {
                Some(msg.get_parent())
            } else {
                None
            },
            position: msg.get_position(),
        }
    }

    pub fn parse_channel_state(&mut self, mut msg: msgs::ChannelState) {
        if msg.has_description() {
            self.description = Some(msg.take_description());
        }
        self.links = msg.take_links();
        if msg.has_max_users() {
            self.max_users = msg.get_max_users();
        }
        if msg.has_name() {
            self.name = msg.take_name();
        }
        if msg.has_parent() {
            self.parent = Some(msg.get_parent());
        }
        if msg.has_position() {
            self.position = msg.get_position();
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self, channels: &HashMap<u32, Channel>) -> String {
        match &self.parent {
            Some(t) => format!("{}/{}", channels.get(t).unwrap().path(channels), self.name),
            None => self.name.clone(),
        }
    }
}

#[derive(Debug)]
struct ProtoTree<'a> {
    channel: Option<&'a Channel>,
    children: HashMap<u32, ProtoTree<'a>>,
    users: Vec<&'a User>,
}

impl<'a> ProtoTree<'a> {
    fn walk_and_add(
        &mut self,
        channel: &'a Channel,
        users: &HashMap<u32, Vec<&'a User>>,
        walk: &[u32],
    ) {
        match walk {
            [] => unreachable!("Walks should always have at least one element"),
            &[node] => {
                let pt = self.children.entry(node).or_insert(ProtoTree {
                    channel: None,
                    children: HashMap::new(),
                    users: Vec::new(),
                });
                pt.channel = Some(channel);
                pt.users = users.get(&node).cloned().unwrap_or_default();
            }
            longer => {
                self.children
                    .entry(longer[0])
                    .or_insert(ProtoTree {
                        channel: None,
                        children: HashMap::new(),
                        users: Vec::new(),
                    })
                    .walk_and_add(channel, users, &walk[1..]);
            }
        }
    }
}

impl<'a> From<&ProtoTree<'a>> for mumlib::state::Channel {
    fn from(tree: &ProtoTree<'a>) -> Self {
        let mut channel = mumlib::state::Channel::from(tree.channel.unwrap());
        let mut children = tree
            .children
            .iter()
            .map(|e| {
                (
                    e.1.channel.unwrap().position,
                    mumlib::state::Channel::from(e.1),
                )
            })
            .collect::<Vec<_>>();
        children.sort_by_key(|e| (e.0, e.1.name.clone()));
        channel.children = children.into_iter().map(|e| e.1).collect();
        channel.users = tree.users.iter().map(|e| (*e).into()).collect();
        channel
    }
}

pub fn into_channel(
    channels: &HashMap<u32, Channel>,
    users: &HashMap<u32, User>,
) -> mumlib::state::Channel {
    let mut walks = Vec::new();

    let mut channel_lookup = HashMap::new();

    for user in users.values() {
        channel_lookup
            .entry(user.channel())
            .or_insert_with(Vec::new)
            .push(user);
    }

    for (channel_id, channel) in channels {
        let mut walk = Vec::new();
        let mut current = *channel_id;
        while let Some(next) = channels.get(&current).unwrap().parent {
            walk.push(current);
            current = next;
        }
        walk.reverse();

        if !walk.is_empty() {
            walks.push((walk, channel));
        }
    }

    //root node is ignored because of how walk_and_add is implemented on ProtoTree
    let mut proto_tree = ProtoTree {
        channel: Some(channels.get(&0).unwrap()),
        children: HashMap::new(),
        users: channel_lookup.get(&0).cloned().unwrap_or_default(),
    };

    for (walk, channel) in walks {
        proto_tree.walk_and_add(channel, &channel_lookup, &walk);
    }

    (&proto_tree).into()
}

impl From<&Channel> for mumlib::state::Channel {
    fn from(channel: &Channel) -> Self {
        mumlib::state::Channel::new(
            channel.name.clone(),
            channel.description.clone(),
            channel.max_users,
        )
    }
}
