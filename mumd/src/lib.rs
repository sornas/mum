#![warn(elided_lifetimes_in_paths)]
#![warn(meta_variable_misuse)]
#![warn(missing_debug_implementations)]
#![warn(single_use_lifetimes)]
#![warn(unreachable_pub)]
#![warn(unused_crate_dependencies)]
#![warn(unused_import_braces)]
#![warn(unused_lifetimes)]
#![warn(unused_qualifications)]
#![deny(macro_use_extern_crate)]
#![deny(missing_abi)]
#![deny(future_incompatible)]
#![forbid(unsafe_code)]
#![forbid(non_ascii_idents)]

pub mod audio;
pub mod client;
pub mod command;
pub mod error;
pub mod network;
pub mod notifications;
pub mod state;

use bincode as _; // used in binary
#[cfg(feature = "gtk")]
use gtk as _; // used in gui binary
