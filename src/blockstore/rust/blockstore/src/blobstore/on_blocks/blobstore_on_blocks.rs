use anyhow::Result;
use async_trait::async_trait;
use std::fmt;

use super::blob_on_blocks::BlobOnBlocks;
use super::data_tree_store::DataTreeStore;
use crate::blobstore::{BlobId, BlobStore, RemoveResult};
use crate::blockstore::{high_level::LockingBlockStore, low_level::BlockStore};
use crate::utils::async_drop::{AsyncDrop, AsyncDropGuard};

pub struct BlobStoreOnBlocks<B: BlockStore + Send + Sync> {
    tree_store: AsyncDropGuard<DataTreeStore<B>>,
}

impl<B: BlockStore + Send + Sync> BlobStoreOnBlocks<B> {
    pub fn new(
        blockstore: AsyncDropGuard<LockingBlockStore<B>>,
        block_size_bytes: u32,
    ) -> Result<AsyncDropGuard<Self>> {
        Ok(AsyncDropGuard::new(Self {
            tree_store: DataTreeStore::new(blockstore, block_size_bytes)?,
        }))
    }
}

#[async_trait]
impl<B: BlockStore + Send + Sync> BlobStore for BlobStoreOnBlocks<B> {
    type ConcreteBlob = BlobOnBlocks<B>;

    async fn create(&self) -> Result<AsyncDropGuard<Self::ConcreteBlob>> {
        Ok(BlobOnBlocks::new(self.tree_store.create_tree().await?))
    }

    async fn load(&self, id: &BlobId) -> Result<Option<AsyncDropGuard<Self::ConcreteBlob>>> {
        Ok(self
            .tree_store
            .load_tree(id.root)
            .await?
            .map(|tree| BlobOnBlocks::new(tree)))
    }

    async fn remove_by_id(&self, id: &BlobId) -> Result<RemoveResult> {
        Ok(self.tree_store.remove_tree_by_id(id.root).await?)
    }

    async fn num_nodes(&self) -> Result<u64> {
        self.tree_store.num_nodes().await
    }

    fn estimate_space_for_num_blocks_left(&self) -> Result<u64> {
        self.tree_store.estimate_space_for_num_blocks_left()
    }

    fn virtual_block_size_bytes(&self) -> u32 {
        self.tree_store.virtual_block_size_bytes()
    }
}

impl<B: BlockStore + Send + Sync> fmt::Debug for BlobStoreOnBlocks<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockStoreOnBlocks")
    }
}

#[async_trait]
impl<B: BlockStore + Send + Sync> AsyncDrop for BlobStoreOnBlocks<B> {
    type Error = anyhow::Error;

    async fn async_drop_impl(&mut self) -> Result<(), Self::Error> {
        self.tree_store.async_drop().await
    }
}