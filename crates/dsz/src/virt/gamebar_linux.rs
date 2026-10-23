//! Xbox Game Bar is a Windows overlay. Linux desktop/Steam overlays do not
//! expose the same ownership or Guide-button forwarding semantics.

use std::sync::Arc;
use crate::engine::Engine;

pub fn open() -> bool { false }

pub fn run(_engine: Arc<Engine>) {
    log::debug!("Xbox Game Bar watcher is not applicable on Linux");
}
