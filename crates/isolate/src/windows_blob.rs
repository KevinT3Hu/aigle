//! Windows transfers owned pixels through the worker pipe.
use bytemuck::Pod;
use std::{fs::File, io, ops::Deref};
pub(super) const SHARED: u64 = 1 << 63;
pub struct Payload<T: Pod>(Vec<T>);
impl<T: Pod> From<Vec<T>> for Payload<T> {
    fn from(v: Vec<T>) -> Self {
        Self(v)
    }
}
impl<T: Pod> Deref for Payload<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}
impl<T: Pod> Payload<T> {
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
    pub fn filled(len: usize, fill: impl FnOnce(&mut [T])) -> io::Result<Self> {
        len.checked_mul(size_of::<T>())
            .ok_or_else(|| io::Error::other("payload overflow"))?;
        let mut data = vec![T::zeroed(); len];
        fill(&mut data);
        Ok(Self(data))
    }
    pub fn is_mapped(&self) -> bool {
        false
    }
    pub(super) fn shared_file(&self) -> Option<&File> {
        None
    }
}
pub(super) fn send(_: &File, _: &File) -> io::Result<()> {
    Err(io::Error::other(
        "shared descriptors unsupported on Windows",
    ))
}
pub(super) fn receive<T: Pod>(_: &super::windows::PipeReader, _: usize) -> io::Result<Payload<T>> {
    Err(io::Error::other(
        "shared descriptors unsupported on Windows",
    ))
}
