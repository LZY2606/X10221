pub mod domain;
pub mod fcs;
pub mod replay;
pub mod store;
pub mod projection;
pub mod api;
pub mod server;

pub use server::{build_app, AppState, run_server};
