//! OS services required by computation: resource budgets and backing storage.
//! Window systems, monitor discovery, D-Bus and desktop credentials belong to frontends.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(unix)]
mod unix;
#[cfg(not(target_os = "linux"))]
mod unsupported;
#[cfg(target_os = "linux")]
pub use linux::physical_ram;
#[cfg(target_os = "linux")]
pub(super) use linux::{cgroup_limit, memory_pressure};
#[cfg(unix)]
pub(super) use unix::{free_space, map_file};
#[cfg(not(target_os = "linux"))]
pub use unsupported::physical_ram;
#[cfg(not(target_os = "linux"))]
pub(super) use unsupported::{cgroup_limit, memory_pressure};
#[cfg(not(unix))]
pub(super) use unsupported::{free_space, map_file};
