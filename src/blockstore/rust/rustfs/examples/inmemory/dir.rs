use async_trait::async_trait;
use cryfs_rustfs::{
    object_based_api::Dir, DirEntry, FsError, FsResult, Gid, Mode, NodeAttrs, NodeKind, NumBytes,
    OpenFlags, Uid,
};
use cryfs_utils::async_drop::AsyncDropGuard;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use super::device::{InMemoryDevice, RootDir};
use super::file::InMemoryFileRef;
use super::file::InMemoryOpenFileRef;
use super::inode_metadata::{chmod, chown, utimens};
use super::node::InMemoryNodeRef;
use super::symlink::InMemorySymlinkRef;
use crate::utils::lock_in_ptr_order;

// Inode is in separate module so we can ensure class invariant through public/private boundaries
mod inode {
    use super::*;

    pub struct DirInode {
        metadata: NodeAttrs,
        entries: HashMap<String, InMemoryNodeRef>,
    }

    impl DirInode {
        pub fn new(mode: Mode, uid: Uid, gid: Gid) -> Self {
            Self {
                metadata: NodeAttrs {
                    // TODO What are the right dir attributes here for directories?
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
                entries: HashMap::new(),
            }
        }

        pub fn metadata(&self) -> &NodeAttrs {
            &self.metadata
        }

        pub fn chmod(&mut self, mode: Mode) {
            chmod(&mut self.metadata, mode);
        }

        pub fn chown(&mut self, uid: Option<Uid>, gid: Option<Gid>) {
            chown(&mut self.metadata, uid, gid);
        }

        pub fn utimens(
            &mut self,
            last_access: Option<SystemTime>,
            last_modification: Option<SystemTime>,
        ) {
            utimens(&mut self.metadata, last_access, last_modification);
        }

        pub fn entries(&self) -> &HashMap<String, InMemoryNodeRef> {
            &self.entries
        }

        // TODO Once we have an invariant that depends on the number of entries
        //      (e.g. metadata.num_bytes),
        //      we can't offer `entries_mut` as a function anymore because it could
        //      violate that invariant.
        pub fn entries_mut(&mut self) -> &mut HashMap<String, InMemoryNodeRef> {
            &mut self.entries
        }
    }
}
pub use inode::DirInode;

pub struct InMemoryDirRef {
    inode: Arc<Mutex<DirInode>>,

    // weak to avoid reference cycles
    rootdir: Weak<RootDir>,
}

impl InMemoryDirRef {
    pub fn new(rootdir: Weak<RootDir>, mode: Mode, uid: Uid, gid: Gid) -> Self {
        Self {
            inode: Arc::new(Mutex::new(DirInode::new(mode, uid, gid))),
            rootdir,
        }
    }

    pub fn from_inode(rootdir: Weak<RootDir>, inode: Arc<Mutex<DirInode>>) -> Self {
        Self { inode, rootdir }
    }

    pub fn clone_ref(&self) -> Self {
        Self {
            inode: Arc::clone(&self.inode),
            rootdir: Weak::clone(&self.rootdir),
        }
    }

    pub fn metadata(&self) -> NodeAttrs {
        let inode = self.inode.lock().unwrap();
        *inode.metadata()
    }

    pub fn get_child(&self, name: &str) -> FsResult<InMemoryNodeRef> {
        let inode = self.inode.lock().unwrap();
        match inode.entries().get(name) {
            Some(node) => Ok(node.clone_ref()),
            None => Err(FsError::NodeDoesNotExist),
        }
    }

    pub fn chmod(&self, mode: Mode) {
        self.inode.lock().unwrap().chmod(mode);
    }

    pub fn chown(&self, uid: Option<Uid>, gid: Option<Gid>) {
        self.inode.lock().unwrap().chown(uid, gid);
    }

    pub fn utimens(&self, last_access: Option<SystemTime>, last_modification: Option<SystemTime>) {
        self.inode
            .lock()
            .unwrap()
            .utimens(last_access, last_modification);
    }
}

#[async_trait]
impl Dir for InMemoryDirRef {
    type Device = InMemoryDevice;

    async fn entries(&self) -> FsResult<Vec<DirEntry>> {
        let inode = self.inode.lock().unwrap();
        let basic_entries = [
            DirEntry {
                name: ".".to_string(),
                kind: NodeKind::Dir,
            },
            DirEntry {
                name: "..".to_string(),
                kind: NodeKind::Dir,
            },
        ];
        let real_entries = inode.entries().iter().map(|(name, node)| {
            let kind: NodeKind = match node {
                InMemoryNodeRef::File(_) => NodeKind::File,
                InMemoryNodeRef::Dir(_) => NodeKind::Dir,
                InMemoryNodeRef::Symlink(_) => NodeKind::Symlink,
            };
            DirEntry {
                name: name.clone(),
                kind,
            }
        });
        Ok(basic_entries.into_iter().chain(real_entries).collect())
    }

    async fn create_child_dir(
        &self,
        name: &str,
        mode: Mode,
        uid: Uid,
        gid: Gid,
    ) -> FsResult<NodeAttrs> {
        let mut inode = self.inode.lock().unwrap();
        let dir = InMemoryDirRef::new(Weak::clone(&self.rootdir), mode, uid, gid);
        let metadata = dir.metadata();
        // TODO Use try_insert once that is stable
        match inode.entries_mut().entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(FsError::NodeAlreadyExists);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(InMemoryNodeRef::Dir(dir));
            }
        }
        Ok(metadata)
    }

    async fn remove_child_dir(&self, name: &str) -> FsResult<()> {
        let mut inode = self.inode.lock().unwrap();
        // TODO Use try_insert once that is stable
        match inode.entries_mut().entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(entry) => match entry.get() {
                InMemoryNodeRef::File(_) | InMemoryNodeRef::Symlink(_) => {
                    return Err(FsError::NodeIsNotADirectory);
                }
                InMemoryNodeRef::Dir(_) => {
                    entry.remove();
                    Ok(())
                }
            },
            std::collections::hash_map::Entry::Vacant(_) => Err(FsError::NodeDoesNotExist),
        }
    }

    async fn create_child_symlink(
        &self,
        name: &str,
        target: &Path,
        uid: Uid,
        gid: Gid,
    ) -> FsResult<NodeAttrs> {
        let mut inode = self.inode.lock().unwrap();
        let symlink = InMemorySymlinkRef::new(target.to_owned(), uid, gid);
        let metadata = symlink.metadata();
        // TODO Use try_insert once that is stable
        match inode.entries_mut().entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(FsError::NodeAlreadyExists);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(InMemoryNodeRef::Symlink(symlink));
            }
        }
        Ok(metadata)
    }

    async fn remove_child_file_or_symlink(&self, name: &str) -> FsResult<()> {
        let mut inode = self.inode.lock().unwrap();
        // TODO Use try_insert once that is stable
        match inode.entries_mut().entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(entry) => match entry.get() {
                InMemoryNodeRef::File(_) | InMemoryNodeRef::Symlink(_) => {
                    entry.remove();
                    Ok(())
                }
                InMemoryNodeRef::Dir(_) => return Err(FsError::NodeIsADirectory),
            },
            std::collections::hash_map::Entry::Vacant(_) => Err(FsError::NodeDoesNotExist),
        }
    }

    async fn create_and_open_file(
        &self,
        name: &str,
        mode: Mode,
        uid: Uid,
        gid: Gid,
    ) -> FsResult<(NodeAttrs, AsyncDropGuard<InMemoryOpenFileRef>)> {
        let mut inode = self.inode.lock().unwrap();
        let file = InMemoryFileRef::new(mode, uid, gid);
        let openfile = file.open_sync(OpenFlags::ReadWrite);
        let metadata = file.metadata();
        // TODO Use try_insert once that is stable
        match inode.entries_mut().entry(name.to_string()) {
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err(FsError::NodeAlreadyExists);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(InMemoryNodeRef::File(file));
            }
        }
        Ok((metadata, openfile))
    }

    async fn rename_child(&self, old_name: &str, new_path: &Path) -> FsResult<()> {
        // TODO Go through CryNode assertions (C++) and check if we should do them here too,
        //      - moving a directory into a subdirectory of itself
        //      - overwriting a directory with a non-directory
        //      - overwriten a non-empty dir (special case: making a directory into its own ancestor)
        // TODO No unwrap
        let new_parent_path = new_path.parent().unwrap();
        // TODO Is to_string_lossy the right way to convert from OsStr to &str?
        let new_name: &str = &new_path.file_name().unwrap().to_string_lossy();
        let new_parent = self
            .rootdir
            .upgrade()
            .expect("RootDir cannot be gone when we still have `self` as a InMemoryDir instance")
            .load_dir(new_parent_path)?;
        if Arc::as_ptr(&self.inode) == Arc::as_ptr(&new_parent.inode) {
            // TODO Deduplicate logic in the then-branch and the else-branch of this if statement
            // We're just renaming it within one directory
            // TODO Check if this actually works or if there is some deadlock because we're loading the same directory twice
            let mut inode = self.inode.lock().unwrap();
            let entries = inode.entries_mut();
            if entries.contains_key(new_name) {
                // TODO Some forms of overwriting are actually ok, we don't need to block them all
                Err(FsError::NodeAlreadyExists)
            } else {
                let old_entry = match entries.remove(old_name) {
                    Some(node) => node,
                    None => {
                        return Err(FsError::NodeDoesNotExist);
                    }
                };
                // TODO Use try_insert once stable
                let insert_result = entries.insert(new_name.to_owned(), old_entry);
                assert!(insert_result.is_none(), "We checked above that `new_name` doesn't exist in the map. Inserting it shouldn't fail.");
                Ok(())
            }
        } else {
            // We're moving it to another directory
            let (mut source_inode, mut target_inode) =
                lock_in_ptr_order(&self.inode, &new_parent.inode);
            let source_entries = source_inode.entries_mut();
            let target_entries = target_inode.entries_mut();
            if target_entries.contains_key(new_name) {
                // TODO Some forms of overwriting are actually ok, we don't need to block them all
                Err(FsError::NodeAlreadyExists)
            } else {
                let old_entry = match source_entries.remove(old_name) {
                    Some(node) => node,
                    None => {
                        return Err(FsError::NodeDoesNotExist);
                    }
                };
                // TODO Use try_insert once stable
                let insert_result = target_entries.insert(new_name.to_owned(), old_entry);
                assert!(insert_result.is_none(), "We checked above that `new_name` doesn't exist in the map. Inserting it shouldn't fail.");
                Ok(())
            }
        }
    }
}