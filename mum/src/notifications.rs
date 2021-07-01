/// Try to initialize the notification daemon.
///
/// Does nothing if compiled without notification support.
pub fn init() {
    #[cfg(feature = "notifications")]
    if let Err(e) = libnotify::init("mumd") {
        log::warn!("Unable to initialize notifications: {}", e);
    }
}

#[cfg(feature = "notifications")]
/// Send a notification non-blocking, returning a JoinHandle that resolves to
/// whether the notification was sent or not.
///
/// The notification is sent on its own thread since a timeout otherwise might
/// block for several seconds.
///
/// None is returned iff the program was compiled without notification support.
pub fn send(msg: String) -> Option<std::thread::JoinHandle<bool>> {
    Some(std::thread::spawn(move || {
        let status = libnotify::Notification::new("mumd", Some(msg.as_str()), None).show();
        if let Err(e) = &status {
            log::warn!("Unable to send notification: {}", e);
        }
        status.is_ok()
    }))
}

#[cfg(not(feature = "notifications"))]
/// Send a notification. Without the `notifications`-feature, this will never do
/// anything and always return None.
pub fn send(_: String) -> Option<std::thread::JoinHandle<bool>> {
    None
}
