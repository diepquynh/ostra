//! The `ostra` server: bootstrap, auth, REST and WebSocket API, the embedded web build, and the
//! glue that gives each workspace its engine.

pub mod api;
pub mod app;
pub mod assets;
pub mod auth;
pub mod bridge;
pub mod env;
pub mod services;
pub mod workspace;
pub mod ws;
