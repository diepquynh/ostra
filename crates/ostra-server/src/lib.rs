//! The `ostra` server: bootstrap, auth, REST and WebSocket API, the embedded web build, and the
//! glue that gives each workspace its engine.

pub mod api;
pub mod app;
pub mod assets;
pub mod auth;
pub mod bridge;
pub mod code;
pub mod credentials;
pub mod env;
pub mod files;
pub mod git;
pub mod harness_setup;
pub mod nav;
pub mod prices;
pub mod repo;
pub mod services;
pub mod setup;
pub mod skills;
pub mod ui_state;
pub mod workspace;
pub mod ws;
