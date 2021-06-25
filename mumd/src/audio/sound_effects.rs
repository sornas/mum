use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use mumlib::config::SoundEffect;
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt;
use std::fs::File;
#[cfg(feature = "ogg")]
use std::io::Cursor;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::audio::SAMPLE_RATE;

/// Stores unpacked audio data loaded from paths with basic caching.
///
/// Basic usage follows the following structure.
/// 1) Specify a file that should be played when a specific event occurs using
///    `SoundEffects::set_sound_effect`.
/// 2) Repeat 1 for all events that should be set.
/// 3) Call `SoundEffects::load_unloaded_files` to load any files that haven't already been loaded.
/// 4) Call `SoundEffects::get_samples` when an event occurs to get the audio data that should be
///    played.
///
/// If no file has been specified, or if the specified file couldn't be read for some reason, the
/// default sound effect is returned instead.
///
/// # Notes on caching
///
/// The caching is basic in the sense that it never checks if the data is up to date. To reload the
/// cache, clear all data using [SoundEffects::clear] and repeat the initialization process.
pub struct SoundEffects {
    /// The default sound effect that is returned if needed.
    default_sound_effect: Vec<f32>,
    /// The opened files and the data they contained when opened. None -> invalid data so use the
    /// default sound effect instead.
    opened_files: HashMap<PathBuf, Option<Vec<f32>>>,

    /// Which file should be played on an event. Event not present -> default sound effect.
    events: HashMap<NotificationEvent, PathBuf>,

    /// The amount of channels the audio data contains. Set on initialization and used when loading
    /// data.
    num_channels: usize,
}

impl fmt::Debug for SoundEffects {
    /// Custom formatting that doesn't print raw audio data.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let opened_files: Vec<_> = self.opened_files.keys().collect();
        f.debug_struct("SoundEffects")
            .field("default_sound_effect", &"[..]")
            .field("opened_files", &opened_files)
            .field("num_channels", &self.num_channels)
            .field("events", &self.events)
            .finish()
    }
}

impl SoundEffects {
    pub fn new(num_channels: usize) -> Self {
        SoundEffects {
            default_sound_effect: unpack_audio(get_default_sfx(), AudioFileKind::Wav).unwrap().0,
            opened_files: HashMap::new(),
            events: HashMap::new(),
            num_channels,
        }
    }

    /// Load a path and store the audio data it contained.
    pub fn load_file(&mut self, path: PathBuf) {
        let samples = open_and_unpack_audio(&path, self.num_channels).ok();
        self.opened_files.insert(path, samples);
    }

    /// Set a file path that should be played when a specific event occurs.
    pub fn set_sound_effect(&mut self, sound_effect: &SoundEffect) {
        if let Ok(event) = NotificationEvent::try_from(sound_effect.event.as_str()) {
            let path = PathBuf::from(&sound_effect.file);
            self.events.insert(event, path);
        }
    }

    /// Load all currently unloaded audio files. Might take some time depending on the amount and
    /// types of files.
    pub fn load_unloaded_files(&mut self) {
        // Find paths to load.
        let mut to_load = Vec::new();
        for path in self.events.values() {
            if !self.opened_files.contains_key(path) {
                to_load.push(path.to_path_buf());
            }
        }
        // Load them.
        for path in to_load {
            self.load_file(path);
        }
    }

    /// Get the samples that should be played when an event occurs.
    ///
    /// Note that this will not work as expected if you haven't called [load_unloaded_files].
    pub fn get_samples(&self, event: &NotificationEvent) -> &[f32] {
        self
            .events
            .get(event)
            .and_then(|path| self.opened_files.get(path))
            // Here we have an Option<&Option<Vec<f32>>>,
            // so we do None => None
            //          Some(&None) => None
            //          Some(&Some(v)) => Some(&v)
            .and_then(|o| o.as_ref())
            .unwrap_or(&self.default_sound_effect)
    }

    /// Clear all store data, including opened audio data.
    pub fn clear(&mut self) {
        self.events.clear();
        self.opened_files.clear();
    }
}

/// The different kinds of files we can open.
enum AudioFileKind {
    Ogg,
    Wav,
}

impl TryFrom<&str> for AudioFileKind {
    type Error = ();

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "ogg" => Ok(AudioFileKind::Ogg),
            "wav" => Ok(AudioFileKind::Wav),
            _ => Err(()),
        }
    }
}

/// A specification accompanying some audio data.
struct AudioSpec {
    channels: u32,
    sample_rate: u32,
}

/// An event where a notification is shown and a sound effect is played.
#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash)]
pub enum NotificationEvent {
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

impl TryFrom<&str> for NotificationEvent {
    type Error = ();

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "server_connect" => Ok(NotificationEvent::ServerConnect),
            "server_disconnect" => Ok(NotificationEvent::ServerDisconnect),
            "user_connected" => Ok(NotificationEvent::UserConnected),
            "user_disconnected" => Ok(NotificationEvent::UserDisconnected),
            "user_joined_channel" => Ok(NotificationEvent::UserJoinedChannel),
            "user_left_channel" => Ok(NotificationEvent::UserLeftChannel),
            "mute" => Ok(NotificationEvent::Mute),
            "unmute" => Ok(NotificationEvent::Unmute),
            "deafen" => Ok(NotificationEvent::Deafen),
            "undeafen" => Ok(NotificationEvent::Undeafen),
            _ => {
                Err(())
            }
        }
    }
}

/// Opens the audio data located in a file and returns the contained audio data.
///
/// The file kind is read from the file extension.
///
/// # Errors
///
/// Returns an error if a file extension isn't known, the file doesn't exist or something went
/// wrong when opening or unpacking the audio data.
fn open_and_unpack_audio<P: AsRef<Path>>(path: &P, num_channels: usize) -> Result<Vec<f32>, ()> {
    let kind = path
        .as_ref()
        .extension()
        .and_then(|ext| AudioFileKind::try_from(ext.to_str().unwrap()).ok())
        .ok_or(())?;
    let bytes = get_sfx(path)?;
    // Unpack the samples.
    let (samples, spec) = unpack_audio(bytes, kind)?;
    // If the audio is mono (single channel), pad every sample with
    // itself, since we later assume that audio is stored interleaved as
    // LRLRLR (or RLRLRL). Without this, mono audio would be played in
    // double speed.
    let iter: Box<dyn Iterator<Item = f32>> = match spec.channels {
        1 => Box::new(samples.into_iter().flat_map(|e| [e, e])),
        2 => Box::new(samples.into_iter()),
        _ => unimplemented!("Only mono and stereo sound is supported. See #80."),
    };
    // Create a dasp signal containing stereo sound.
    let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
    // Create a linear interpolator, in case we need to convert the sample rate.
    let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
    // Create our resulting samples.
    let samples = signal
        .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
        .until_exhausted()
        // If the source audio is stereo and is being played as mono, discard the first channel.
        .flat_map(|e| {
            if num_channels == 1 {
                vec![e[0]]
            } else {
                e.to_vec()
            }
        })
        .collect::<Vec<f32>>();
    Ok(samples)
}

/// Unpack audio data. The required audio spec is read from the file and returned as well.
fn unpack_audio(data: Cow<'_, [u8]>, kind: AudioFileKind) -> Result<(Vec<f32>, AudioSpec), ()> {
    match kind {
        AudioFileKind::Ogg => unpack_ogg(data),
        AudioFileKind::Wav => unpack_wav(data),
    }
}

#[cfg(feature = "ogg")]
/// Unpack ogg data.
fn unpack_ogg(data: Cow<'_, [u8]>) -> Result<(Vec<f32>, AudioSpec), ()> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(data.as_ref())).unwrap();
    let mut samples = Vec::new();
    while let Ok(Some(mut frame)) = reader.read_dec_packet_itl() {
        samples.append(&mut frame);
    }
    let samples = samples.iter().map(|s| cpal::Sample::to_f32(s)).collect();
    let spec = AudioSpec {
        channels: reader.ident_hdr.audio_channels as u32,
        sample_rate: reader.ident_hdr.audio_sample_rate,
    };
    Ok((samples, spec))
}

#[cfg(not(feature = "ogg"))]
/// Always errors since ogg is disabled.
fn unpack_ogg(_: Cow<'_, [u8]>) -> Result<(Vec<f32>, AudioSpec), ()> {
    warn!("Can't open .ogg without the ogg-feature enabled.");
    Err(())
}

/// Unpack wav data.
fn unpack_wav(data: Cow<'_, [u8]>) -> Result<(Vec<f32>, AudioSpec), ()> {
    let reader = hound::WavReader::new(data.as_ref()).map_err(|_| ())?;
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
    let spec = AudioSpec {
        channels: spec.channels as u32,
        sample_rate: spec.sample_rate,
    };
    Ok((samples, spec))
}

/// Open and return the data contained in a file, or the default sound effect if
/// the file couldn't be found.
// moo
fn get_sfx<P: AsRef<Path>>(file: P) -> Result<Cow<'static, [u8]>, ()> {
    let mut buf: Vec<u8> = Vec::new();
    if let Ok(mut file) = File::open(file.as_ref()) {
        file.read_to_end(&mut buf).unwrap();
        Ok(Cow::from(buf))
    } else {
        Err(())
    }
}

/// Get the default sound effect.
fn get_default_sfx() -> Cow<'static, [u8]> {
    Cow::from(include_bytes!("fallback_sfx.wav").as_ref())
}
