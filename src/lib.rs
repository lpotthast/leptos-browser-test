#![doc = include_str!("../README.md")]

mod app;
mod cargo_leptos;
mod config;
mod error;
mod ports;
mod site;
mod startup;

pub use app::LeptosTestApp;
pub use cargo_leptos::CargoLeptosMode;
pub use config::LeptosTestAppConfig;
pub use error::LeptosBrowserTestError;
pub use site::SiteScheme;

pub use rootcause::prelude::ResultExt;
pub use rootcause::{Report, bail, report};
