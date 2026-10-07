//! Inputs for judge calls, built from session state, and the YOLO answer logic.

mod completion;
mod facts;
mod inputs;
mod yolo;

use completion::*;
use facts::*;
pub use inputs::*;
pub use yolo::*;
