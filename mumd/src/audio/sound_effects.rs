use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use std::borrow::Cow;
use std::convert::TryFrom;
use std::fmt;
use std::fs::File;
#[cfg(feature = "ogg")]
use std::io::Cursor;
use std::io::Read;
use std::ops::Index;
use std::path::Path;

use crate::audio::SAMPLE_RATE;

#[derive(Debug, Clone, Copy)]
pub struct SoundEffectId(usize);

pub struct SoundEffects {
    data: Vec<Vec<f32>>,

    num_channels: usize,
}

impl fmt::Debug for SoundEffects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoundEffects")
            .field("num_channels", &self.num_channels)
            .finish_non_exhaustive()
    }
}

impl SoundEffects {
    pub fn new(num_channels: usize) -> Self {
        SoundEffects {
            data: vec![unpack_audio(get_default_sfx(), AudioFileKind::Wav).unwrap().0],
            num_channels,
        }
    }

    pub fn open<P: AsRef<Path>>(&mut self, path: &P) -> Result<SoundEffectId, ()> {
        open_and_unpack_audio(&path.as_ref(), self.num_channels)
            .map(|samples| {
                let idx = SoundEffectId(self.data.len());
                self.data.push(samples);
                idx
            })
    }

    pub fn default_sound_effect() -> SoundEffectId {
        SoundEffectId(0)
    }
}

impl Index<SoundEffectId> for SoundEffects {
    type Output = [f32];

    fn index(&self, index: SoundEffectId) -> &Self::Output {
        &self.data[index.0]
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
