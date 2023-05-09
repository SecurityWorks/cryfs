use async_trait::async_trait;
use std::time::SystemTime;

use crate::utils::{Gid, Mode, NumBytes, Uid};

#[derive(Debug, Clone, Copy)]
pub struct NodeAttrs {
    pub nlink: u32,
    pub mode: Mode,
    pub uid: Uid,
    pub gid: Gid,
    pub num_bytes: NumBytes,
    pub blocks: u64,
    pub atime: SystemTime,
    pub mtime: SystemTime,
    pub ctime: SystemTime,
}

#[async_trait]
pub trait Node {
    async fn getattr(&self) -> std::io::Result<NodeAttrs>;
}
