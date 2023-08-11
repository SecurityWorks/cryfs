use async_trait::async_trait;
use cryfs_rustfs::{
    object_based_api::{File, OpenFile},
    Data, FsError, FsResult, Gid, Mode, NodeAttrs, NumBytes, OpenFlags, Uid,
};
use cryfs_utils::async_drop::{AsyncDrop, AsyncDropGuard};
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use super::device::InMemoryDevice;
use super::inode_metadata::setattr;
use super::node::InMemoryNodeRef;

// Inode is in separate module so we can ensure class invariant through public/private boundaries
mod inode {
    use super::*;

    pub struct FileInode {
        // Invariant: metadata.num_bytes == data.len()
        metadata: NodeAttrs,
        data: Vec<u8>,
    }

    impl FileInode {
        pub fn new(mode: Mode, uid: Uid, gid: Gid) -> Self {
            Self {
                metadata: NodeAttrs {
                    // TODO What are the right file attributes here?
                    nlink: 1,
                    mode,
                    uid,
                    gid,
                    num_bytes: NumBytes::from(0),
                    num_blocks: None,
                    atime: SystemTime::now(),
                    mtime: SystemTime::now(),
                    ctime: SystemTime::now(),
                },
                data: vec![],
            }
        }

        pub fn metadata(&self) -> &NodeAttrs {
            &self.metadata
        }

        pub fn resize(&mut self, new_size: NumBytes) {
            // TODO No unwrap
            let new_size_usize = usize::try_from(u64::from(new_size)).unwrap();
            self.data.resize(new_size_usize, 0u8);
            self.metadata.num_bytes = new_size;
        }

        pub fn len(&self) -> NumBytes {
            self.metadata.num_bytes
        }

        pub fn data(&self) -> &[u8] {
            &self.data
        }

        pub fn data_mut(&mut self) -> &mut [u8] {
            &mut self.data
        }

        pub fn setattr(
            &mut self,
            mode: Option<Mode>,
            uid: Option<Uid>,
            gid: Option<Gid>,
            atime: Option<SystemTime>,
            mtime: Option<SystemTime>,
            ctime: Option<SystemTime>,
        ) -> FsResult<NodeAttrs> {
            setattr(&mut self.metadata, mode, uid, gid, atime, mtime, ctime)
        }
    }
}

use inode::FileInode;

pub struct InMemoryFileRef {
    // TODO Here (and also in InMemoryDir/Symlink), can we avoid the Mutex by using Rust's `&mut` for functions that modify data?
    inode: Arc<Mutex<FileInode>>,
}

impl InMemoryFileRef {
    pub fn new(mode: Mode, uid: Uid, gid: Gid) -> Self {
        Self {
            inode: Arc::new(Mutex::new(FileInode::new(mode, uid, gid))),
        }
    }

    pub fn as_node(&self) -> AsyncDropGuard<InMemoryNodeRef> {
        AsyncDropGuard::new(InMemoryNodeRef::File(self.clone_ref()))
    }

    pub fn clone_ref(&self) -> Self {
        Self {
            inode: Arc::clone(&self.inode),
        }
    }

    pub fn open_sync(&self, openflags: OpenFlags) -> AsyncDropGuard<InMemoryOpenFileRef> {
        AsyncDropGuard::new(InMemoryOpenFileRef {
            openflags,
            inode: Arc::clone(&self.inode),
        })
    }

    pub fn metadata(&self) -> NodeAttrs {
        let inode = self.inode.lock().unwrap();
        *inode.metadata()
    }

    pub fn setattr(
        &self,
        mode: Option<Mode>,
        uid: Option<Uid>,
        gid: Option<Gid>,
        size: Option<NumBytes>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
        ctime: Option<SystemTime>,
    ) -> FsResult<NodeAttrs> {
        let mut inode = self.inode.lock().unwrap();
        if let Some(size) = size {
            inode.resize(size);
        }
        inode.setattr(mode, uid, gid, atime, mtime, ctime)
    }
}

#[async_trait]
impl File for InMemoryFileRef {
    type Device = InMemoryDevice;

    async fn open(&self, openflags: OpenFlags) -> FsResult<AsyncDropGuard<InMemoryOpenFileRef>> {
        Ok(self.open_sync(openflags))
    }
}

impl Debug for InMemoryFileRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryFileRef").finish()
    }
}

#[async_trait]
impl AsyncDrop for InMemoryFileRef {
    type Error = FsError;

    async fn async_drop_impl(&mut self) -> Result<(), FsError> {
        // Nothing to do
        Ok(())
    }
}

pub struct InMemoryOpenFileRef {
    openflags: OpenFlags,
    inode: Arc<Mutex<FileInode>>,
}

#[async_trait]
impl OpenFile for InMemoryOpenFileRef {
    async fn getattr(&self) -> FsResult<NodeAttrs> {
        // TODO Deduplicate with implementation in InMemoryNode
        // TODO Is getattr allowed when openflags are writeonly?
        Ok(*self.inode.lock().unwrap().metadata())
    }

    async fn setattr(
        &self,
        mode: Option<Mode>,
        uid: Option<Uid>,
        gid: Option<Gid>,
        size: Option<NumBytes>,
        atime: Option<SystemTime>,
        mtime: Option<SystemTime>,
        ctime: Option<SystemTime>,
    ) -> FsResult<NodeAttrs> {
        // TODO Is setattr allowed when openflags are readonly?
        let mut inode = self.inode.lock().unwrap();
        if let Some(size) = size {
            match self.openflags {
                OpenFlags::Read => return Err(FsError::WriteOnReadOnlyFileDescriptor),
                OpenFlags::Write | OpenFlags::ReadWrite => {
                    inode.resize(size);
                }
            }
        }
        inode.setattr(mode, uid, gid, atime, mtime, ctime)
    }

    async fn read(&self, offset: NumBytes, size: NumBytes) -> FsResult<Data> {
        match self.openflags {
            OpenFlags::Write => Err(FsError::ReadOnWriteOnlyFileDescriptor),
            OpenFlags::Read | OpenFlags::ReadWrite => {
                let offset = usize::try_from(u64::from(offset)).unwrap();
                let size = usize::try_from(u64::from(size)).unwrap();
                let inode = self.inode.lock().unwrap();
                let data = inode.data();
                let actually_read = std::cmp::min(size, data.len() - offset);
                Ok(data[offset..offset + actually_read].to_vec().into())
            }
        }
    }

    async fn write(&self, offset: NumBytes, data: Data) -> FsResult<()> {
        match self.openflags {
            OpenFlags::Read => Err(FsError::WriteOnReadOnlyFileDescriptor),
            OpenFlags::Write | OpenFlags::ReadWrite => {
                // TODO No unwrap
                let data_len = NumBytes::from(u64::try_from(data.len()).unwrap());
                let inode = &mut self.inode.lock().unwrap();
                if offset + data_len > inode.len() {
                    inode.resize(offset + data_len);
                }
                let offset = usize::try_from(u64::from(offset)).unwrap();
                inode.data_mut()[offset..offset + data.len()].copy_from_slice(&data);
                Ok(())
            }
        }
    }

    async fn flush(&self) -> FsResult<()> {
        // TODO Is flush allowed when openflags are readonly?
        // No need to flush because we're in-memory
        Ok(())
    }

    async fn fsync(&self, _datasync: bool) -> FsResult<()> {
        // TODO Is fsync allowed when openflags are readonly?
        // No need to fsync because we're in-memory
        Ok(())
    }
}

impl Debug for InMemoryOpenFileRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InMemoryOpenFileRef")
            .field("openflags", &self.openflags)
            .finish()
    }
}

#[async_trait]
impl AsyncDrop for InMemoryOpenFileRef {
    type Error = FsError;

    async fn async_drop_impl(&mut self) -> Result<(), FsError> {
        // Nothing to do
        Ok(())
    }
}
