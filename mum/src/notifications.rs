#[cfg(feature = "notifications")]
use log::*;

pub fn init() {
    #[cfg(feature = "notifications")]
    if let Err(e) = libnotify::init("mumd") {
        warn!("Unable to initialize notifications: {}", e);
    }
}

#[cfg(feature = "notifications")]
pub fn send(msg: String) -> Option<bool> {
    match libnotify::Notification::new("mumd", Some(msg.as_str()), None).show() {
        Ok(_) => Some(true),
        Err(e) => {
            warn!("Unable to send notification: {}", e);
            Some(false)
        }
    }
}

#[cfg(not(feature = "notifications"))]
pub fn send(_: String) -> Option<bool> {
    None
}
