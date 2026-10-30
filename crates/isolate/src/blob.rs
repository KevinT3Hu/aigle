//! Direct decoder outputs use sealed anonymous files passed over the protocol socket. The
//! parent maps immutable pages instead of copying hundreds of MB through a pipe.
use bytemuck::Pod;
use rustix::net::{
    self, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};
use std::{
    fs::File,
    io::{self, IoSlice, IoSliceMut},
    marker::PhantomData,
    mem::MaybeUninit,
    ops::Deref,
    os::fd::AsFd,
};
pub(super) const SHARED: u64 = 1 << 63;
const THRESHOLD: usize = 1 << 20;

/// Owned reply pixels, either ordinary heap data or immutable shared pages.
pub struct Payload<T: Pod> {
    storage: Storage<T>,
}
enum Storage<T: Pod> {
    Heap(Vec<T>),
    Mapped(memmap2::Mmap, File, PhantomData<T>),
}
impl<T: Pod> From<Vec<T>> for Payload<T> {
    fn from(v: Vec<T>) -> Self {
        Self {
            storage: Storage::Heap(v),
        }
    }
}
impl<T: Pod> Deref for Payload<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match &self.storage {
            Storage::Heap(v) => v,
            Storage::Mapped(m, _, _) => bytemuck::cast_slice(m),
        }
    }
}
impl<T: Pod> Payload<T> {
    /// Materialize writable data; heap replies retain their original allocation.
    pub fn into_vec(self) -> Vec<T> {
        match self.storage {
            Storage::Heap(v) => v,
            Storage::Mapped(m, _, _) => bytemuck::cast_slice(&m).to_vec(),
        }
    }
    /// Fill decoder output directly in shared pages when running in an isolated
    /// worker. The descriptor is published only after all writable mappings close.
    pub fn filled(len: usize, fill: impl FnOnce(&mut [T])) -> io::Result<Self> {
        let bytes = len
            .checked_mul(size_of::<T>())
            .ok_or_else(|| io::Error::other("payload overflow"))?;
        #[cfg(target_os = "linux")]
        if bytes >= THRESHOLD
            && super::is_worker()
            && std::env::var_os("AIGLE_SHARED_BLOBS").is_none_or(|v| v != "0")
        {
            let allocation = (|| -> io::Result<(File, memmap2::MmapMut)> {
                let fd = rustix::fs::memfd_create(
                    "aigle-pixels",
                    rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
                )?;
                let file = File::from(fd);
                file.set_len(bytes as u64)?;
                rustix::fs::fcntl_add_seals(
                    &file,
                    rustix::fs::SealFlags::GROW | rustix::fs::SealFlags::SHRINK,
                )?;
                #[allow(unsafe_code)]
                // SAFETY: this new, zero-filled memfd is private to this function;
                // no descriptor or mapping has been published to another consumer.
                let map = unsafe { memmap2::MmapOptions::new().len(bytes).map_mut(&file)? };
                Ok((file, map))
            })();
            if let Ok((mut file, mut map)) = allocation {
                let pixels = bytemuck::try_cast_slice_mut::<u8, T>(&mut map)
                    .map_err(|_| io::Error::other("payload alignment"))?;
                fill(pixels);
                drop(map);
                if rustix::fs::fcntl_add_seals(&file, seals()).is_err() {
                    // A restricted kernel may reject sealing. Keep the ordinary
                    // transport rather than exposing a mutable mapping to the parent.
                    use std::io::{Read, Seek, SeekFrom};
                    let mut pixels = vec![T::zeroed(); len];
                    file.seek(SeekFrom::Start(0))?;
                    file.read_exact(bytemuck::cast_slice_mut(&mut pixels))?;
                    return Ok(pixels.into());
                }
                #[allow(unsafe_code)]
                // SAFETY: the only writable mapping was dropped and all seals set.
                let map = unsafe { memmap2::MmapOptions::new().len(bytes).map(&file)? };
                return Ok(Self {
                    storage: Storage::Mapped(map, file, PhantomData),
                });
            }
        }
        let mut pixels = vec![T::zeroed(); len];
        fill(&mut pixels);
        Ok(pixels.into())
    }
    pub(super) fn shared_file(&self) -> Option<&File> {
        match &self.storage {
            Storage::Mapped(_, f, _) => Some(f),
            _ => None,
        }
    }
    pub fn is_mapped(&self) -> bool {
        matches!(self.storage, Storage::Mapped(..))
    }
}

#[cfg(target_os = "linux")]
fn seals() -> rustix::fs::SealFlags {
    use rustix::fs::SealFlags as S;
    S::WRITE | S::SHRINK | S::GROW | S::SEAL
}

pub(super) fn send(socket: &File, file: &File) -> io::Result<()> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    let fds = [file.as_fd()];
    if !ancillary.push(SendAncillaryMessage::ScmRights(&fds)) {
        return Err(io::Error::other("missing descriptor space"));
    }
    let sent = net::sendmsg(
        socket,
        &[IoSlice::new(&[0])],
        &mut ancillary,
        SendFlags::NOSIGNAL,
    )?;
    if sent != 1 {
        return Err(io::Error::other("incomplete descriptor send"));
    }
    Ok(())
}

pub(super) fn receive<T: Pod>(socket: &impl AsFd, len: usize) -> io::Result<Payload<T>> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let mut marker = [1];
    let message = net::recvmsg(
        socket,
        &mut [IoSliceMut::new(&mut marker)],
        &mut ancillary,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    let mut descriptors = Vec::new();
    for message in ancillary.drain() {
        let RecvAncillaryMessage::ScmRights(fds) = message else {
            return Err(io::Error::other("unexpected ancillary message"));
        };
        descriptors.extend(fds);
    }
    if message.bytes != 1
        || marker != [0]
        || message.flags.contains(rustix::net::ReturnFlags::CTRUNC)
        || descriptors.len() != 1
    {
        return Err(io::Error::other("invalid shared reply descriptor"));
    }
    let file = File::from(descriptors.pop().unwrap());
    #[cfg(target_os = "linux")]
    if !rustix::fs::fcntl_get_seals(&file)?.contains(seals()) {
        return Err(io::Error::other("unsealed shared reply"));
    }
    #[cfg(not(target_os = "linux"))]
    return Err(io::Error::other("shared replies unsupported"));
    if file.metadata()?.len() != len as u64 {
        return Err(io::Error::other("shared reply length mismatch"));
    }
    #[allow(unsafe_code)]
    // SAFETY: verified memfd seals forbid writes and resizing by any process.
    // The mapping keeps the immutable object alive even after its descriptor closes.
    let mapping = unsafe { memmap2::MmapOptions::new().len(len).map(&file)? };
    bytemuck::try_cast_slice::<u8, T>(&mapping)
        .map_err(|_| io::Error::other("unaligned shared reply"))?;
    Ok(Payload {
        storage: Storage::Mapped(mapping, file, PhantomData),
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{os::fd::OwnedFd, os::unix::net::UnixStream};
    fn pair() -> (File, File) {
        let (a, b) = UnixStream::pair().unwrap();
        (File::from(OwnedFd::from(a)), File::from(OwnedFd::from(b)))
    }
    #[test]
    fn rejects_unsealed_or_wrong_length_files() {
        let (sender, receiver) = pair();
        let unsealed = File::from(
            rustix::fs::memfd_create(
                "unsealed-test",
                rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
            )
            .unwrap(),
        );
        unsealed.set_len(4096).unwrap();
        send(&sender, &unsealed).unwrap();
        assert!(receive::<u8>(&receiver, 4096).is_err());
        rustix::fs::fcntl_add_seals(&unsealed, seals()).unwrap();
        send(&sender, &unsealed).unwrap();
        assert!(receive::<u8>(&receiver, 4095).is_err());
        send(&sender, &unsealed).unwrap();
        let data = receive::<u32>(&receiver, 4096).unwrap();
        assert!(data.is_mapped());
        assert!(data.iter().all(|&v| v == 0));
        assert!(unsealed.set_len(0).is_err());
    }
}
