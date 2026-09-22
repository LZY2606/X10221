pub mod case;
pub mod compute;
pub mod domain;
pub mod engine;
pub mod workflow;
pub mod transfer;
pub mod db;
pub mod server;

pub type Result<T> = std::result::Result<T, String>;
