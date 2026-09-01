//! Wire protocol and socket helpers shared by opusdm-hub and opusdm-lister.

pub mod frame;
pub mod proto;
pub mod sockpath;

pub use proto::{
    ColorSettings, FileMeta, ListerId, Op, Outcome, Resolution, Role, Settings, ToHub, ToLister,
};
