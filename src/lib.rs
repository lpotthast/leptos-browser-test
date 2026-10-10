#![doc = include_str!("../README.md")]

mod app;
mod cargo_leptos;
mod config;
mod error;
mod ports;
mod site;
mod startup;

pub use app::LeptosTestApp;
pub use cargo_leptos::{BuildProfile, CargoLeptosMode};
pub use config::LeptosTestAppConfig;
pub use error::LeptosBrowserTestError;
pub use rootcause::{Report, bail, prelude::ResultExt, report};
pub use site::SiteScheme;
pub use startup::CapturedOutput;
pub use tokio_process_tools::UnixGracefulSignal;
