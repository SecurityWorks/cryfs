use async_trait::async_trait;

use super::error::FsResult;
use super::node::NodeAttrs;
use crate::utils::{Gid, Mode, NumBytes, Uid};
use std::time::SystemTime;

#[async_trait]
pub trait OpenFile {
    async fn getattr(&self) -> FsResult<NodeAttrs>;
    async fn chmod(&self, mode: Mode) -> FsResult<()>;
    async fn chown(&self, uid: Option<Uid>, gid: Option<Gid>) -> FsResult<()>;
    async fn truncate(&self, new_size: NumBytes) -> FsResult<()>;
    async fn utimens(
        &self,
        last_access: Option<SystemTime>,
        last_modification: Option<SystemTime>,
    ) -> FsResult<()>;
}
