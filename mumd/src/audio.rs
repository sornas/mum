pub mod input;
pub mod output;

use crate::audio::output::SaturatingAdd;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate, Stream, StreamConfig};
use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use log::*;
use mumble_protocol::voice::VoicePacketPayload;
use mumlib::config::SoundEffect;
use opus::Channels;
use std::{collections::hash_map::Entry, fs::File, io};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

//TODO? move to mumlib
pub const DEFAULT_SOUND_FILES: &[(NotificationEvents, &str)] = &[
    (NotificationEvents::ServerConnect, "connect.wav"),
    (NotificationEvents::ServerDisconnect, "disconnect.wav"),
    (NotificationEvents::UserConnected, "channel_join.wav"),
    (NotificationEvents::UserDisconnected, "channel_leave.wav"),
    (NotificationEvents::UserJoinedChannel, "channel_join.wav"),
    (NotificationEvents::UserLeftChannel, "channel_leave.wav"),
    (NotificationEvents::Mute, "mute.wav"),
    (NotificationEvents::Unmute, "unmute.wav"),
    (NotificationEvents::Deafen, "deafen.wav"),
    (NotificationEvents::Undeafen, "undeafen.wav"),
];

const SAMPLE_RATE: u32 = 48000;

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub enum NotificationEvents {
    ServerConnect,
    ServerDisconnect,
    UserConnected,
    UserDisconnected,
    UserJoinedChannel,
    UserLeftChannel,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

pub struct Audio {
    output_config: StreamConfig,
    _output_stream: Stream,
    _input_stream: Stream,

    input_channel_receiver: Option<mpsc::Receiver<VoicePacketPayload>>,
    input_volume_sender: watch::Sender<f32>,

    output_volume_sender: watch::Sender<f32>,

    user_volumes: Arc<Mutex<HashMap<u32, (f32, bool)>>>,

    client_streams: Arc<Mutex<HashMap<u32, output::ClientStream>>>,

    sounds: HashMap<NotificationEvents, Vec<f32>>,
    play_sounds: Arc<Mutex<VecDeque<f32>>>,
}

impl Audio {
    pub fn new(input_volume: f32, output_volume: f32) -> Self {
        let sample_rate = SampleRate(SAMPLE_RATE);

        let host = cpal::default_host();
        let output_device = host
            .default_output_device()
            .expect("default output device not found");
        let output_supported_config = output_device
            .supported_output_configs()
            .expect("error querying output configs")
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap()
            .with_sample_rate(sample_rate);
        let output_supported_sample_format = output_supported_config.sample_format();
        let output_config: StreamConfig = output_supported_config.into();

        let input_device = host
            .default_input_device()
            .expect("default input device not found");
        let input_supported_config = input_device
            .supported_input_configs()
            .expect("error querying output configs")
            .find_map(|c| {
                if c.min_sample_rate() <= sample_rate && c.max_sample_rate() >= sample_rate {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap()
            .with_sample_rate(sample_rate);
        let input_supported_sample_format = input_supported_config.sample_format();
        let input_config: StreamConfig = input_supported_config.into();

        let err_fn = |err| error!("An error occurred on the output audio stream: {}", err);

        let user_volumes = Arc::new(Mutex::new(HashMap::new()));
        let (output_volume_sender, output_volume_receiver) = watch::channel::<f32>(output_volume);
        let play_sounds = Arc::new(Mutex::new(VecDeque::new()));

        let client_streams = Arc::new(Mutex::new(HashMap::new()));
        let output_stream = match output_supported_sample_format {
            SampleFormat::F32 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<f32>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
            SampleFormat::I16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<i16>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
            SampleFormat::U16 => output_device.build_output_stream(
                &output_config,
                output::curry_callback::<u16>(
                    Arc::clone(&play_sounds),
                    Arc::clone(&client_streams),
                    output_volume_receiver,
                    Arc::clone(&user_volumes),
                ),
                err_fn,
            ),
        }
        .unwrap();

        let input_encoder = opus::Encoder::new(
            input_config.sample_rate.0,
            match input_config.channels {
                1 => Channels::Mono,
                2 => Channels::Stereo,
                _ => unimplemented!(
                    "Only 1 or 2 channels supported, got {})",
                    input_config.channels
                ),
            },
            opus::Application::Voip,
        )
        .unwrap();
        let (input_sender, input_receiver) = mpsc::channel(100);

        let (input_volume_sender, input_volume_receiver) = watch::channel::<f32>(input_volume);

        let input_stream = match input_supported_sample_format {
            SampleFormat::F32 => input_device.build_input_stream(
                &input_config,
                input::callback::<f32>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
            SampleFormat::I16 => input_device.build_input_stream(
                &input_config,
                input::callback::<i16>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
            SampleFormat::U16 => input_device.build_input_stream(
                &input_config,
                input::callback::<u16>(
                    input_encoder,
                    input_sender,
                    input_config.sample_rate.0,
                    input_volume_receiver,
                    4, // 10 ms
                ),
                err_fn,
            ),
        }
        .unwrap();

        output_stream.play().unwrap();

        let mut res = Self {
            output_config,
            _output_stream: output_stream,
            _input_stream: input_stream,
            input_volume_sender,
            input_channel_receiver: Some(input_receiver),
            client_streams,
            sounds: HashMap::new(),
            output_volume_sender,
            user_volumes,
            play_sounds,
        };
        res.load_sound_effects(&vec![]).unwrap();
        res
    }

    pub fn load_sound_effects(&mut self, sound_effects: &Vec<SoundEffect>) -> Result<(), ()> {
        let overrides: HashMap<_, _> = sound_effects
            .iter()
            .map(|sound_effect| { (&sound_effect.event, &sound_effect.file) })
            .filter_map(|(event, file)| {
                let event = match event.as_str() {
                    "server_connect" => NotificationEvents::ServerConnect,
                    "server_disconnect" => NotificationEvents::ServerDisconnect,
                    "user_connected" => NotificationEvents::UserConnected,
                    "user_disconnected" => NotificationEvents::UserDisconnected,
                    "user_joined_channel" => NotificationEvents::UserJoinedChannel,
                    "user_left_channel" => NotificationEvents::UserLeftChannel,
                    "mute" => NotificationEvents::Mute,
                    "unmute" => NotificationEvents::Unmute,
                    "deafen" => NotificationEvents::Deafen,
                    "undeafen" => NotificationEvents::Undeafen,
                    _ => {
                        warn!("Unknown notification event '{}' in config", event);
                        return None;
                    }
                };
                Some((event, file))
            })
            .collect();

        self.sounds = DEFAULT_SOUND_FILES
            .iter()
            .map(|(event, file)| {
                let file = if let Some(file) = overrides.get(event) {
                    *file
                } else {
                    *file
                };
                let reader = hound::WavReader::new(get_resource(file).unwrap()).unwrap();
                let spec = reader.spec();
                let samples = match spec.sample_format {
                    hound::SampleFormat::Float => reader
                        .into_samples::<f32>()
                        .map(|e| e.unwrap())
                        .collect::<Vec<_>>(),
                    hound::SampleFormat::Int => reader
                        .into_samples::<i16>()
                        .map(|e| cpal::Sample::to_f32(&e.unwrap()))
                        .collect::<Vec<_>>(),
                };
                let iter: Box<dyn Iterator<Item = f32>> = match spec.channels {
                    1 => Box::new(samples.into_iter().flat_map(|e| vec![e, e])),
                    2 => Box::new(samples.into_iter()),
                    _ => unimplemented!() // TODO handle gracefully (this might not even happen)
                };
                let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
                let interp = Linear::new(signal.next(), signal.next());
                let samples = signal
                    .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                    .until_exhausted()
                    // if the source audio is stereo and is being played as mono, discard the right audio
                    .flat_map(
                        |e| if self.output_config.channels == 1 {
                            vec![e[0]]
                        } else {
                            e.to_vec()
                        }
                    )
                    .collect::<Vec<f32>>();
                (*event, samples)
            })
            .collect();

        Ok(())
    }

    pub fn decode_packet(&self, session_id: u32, payload: VoicePacketPayload) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(mut entry) => {
                entry
                    .get_mut()
                    .decode_packet(payload, self.output_config.channels as usize);
            }
            Entry::Vacant(_) => {
                warn!("Can't find session id {}", session_id);
            }
        }
    }

    pub fn add_client(&self, session_id: u32) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(_) => {
                warn!("Session id {} already exists", session_id);
            }
            Entry::Vacant(entry) => {
                entry.insert(output::ClientStream::new(
                    self.output_config.sample_rate.0,
                    self.output_config.channels,
                ));
            }
        }
    }

    pub fn remove_client(&self, session_id: u32) {
        match self.client_streams.lock().unwrap().entry(session_id) {
            Entry::Occupied(entry) => {
                entry.remove();
            }
            Entry::Vacant(_) => {
                warn!(
                    "Tried to remove session id {} that doesn't exist",
                    session_id
                );
            }
        }
    }

    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<VoicePacketPayload>> {
        self.input_channel_receiver.take()
    }

    pub fn clear_clients(&mut self) {
        self.client_streams.lock().unwrap().clear();
    }

    pub fn set_input_volume(&self, input_volume: f32) {
        self.input_volume_sender.send(input_volume).unwrap();
    }

    pub fn set_output_volume(&self, output_volume: f32) {
        self.output_volume_sender.send(output_volume).unwrap();
    }

    pub fn set_user_volume(&self, id: u32, volume: f32) {
        match self.user_volumes.lock().unwrap().entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().0 = volume;
            }
            Entry::Vacant(entry) => {
                entry.insert((volume, false));
            }
        }
    }

    pub fn set_mute(&self, id: u32, mute: bool) {
        match self.user_volumes.lock().unwrap().entry(id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().1 = mute;
            }
            Entry::Vacant(entry) => {
                entry.insert((1.0, mute));
            }
        }
    }

    pub fn play_effect(&self, effect: NotificationEvents) {
        let samples = self.sounds.get(&effect).unwrap();

        let mut play_sounds = self.play_sounds.lock().unwrap();

        for (val, e) in play_sounds.iter_mut().zip(samples.iter()) {
            *val = val.saturating_add(*e);
        }

        let l = play_sounds.len();
        play_sounds.extend(samples.iter().skip(l));
    }
}

fn get_resource(file: &str) -> io::Result<File> {
    File::open(file)
}
