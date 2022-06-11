use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;

use super::{
    BlockId, BlockStore, BlockStoreDeleter, BlockStoreReader, OptimizedBlockStoreWriter,
    RemoveResult, TryCreateResult,
};

use super::block_data::IBlockData;
use crate::crypto::symmetric::Cipher;
use crate::data::Data;

const FORMAT_VERSION_HEADER: &[u8; 2] = &1u16.to_ne_bytes();

pub struct EncryptedBlockStore<C: Cipher, B> {
    underlying_block_store: B,
    cipher: C,
}

impl<C: Cipher, B> EncryptedBlockStore<C, B> {
    pub fn new(underlying_block_store: B, cipher: C) -> Self {
        Self {
            underlying_block_store,
            cipher,
        }
    }
}

#[async_trait]
impl<C: Cipher + Send + Sync, B: BlockStoreReader + Send + Sync> BlockStoreReader
    for EncryptedBlockStore<C, B>
{
    async fn load(&self, id: &BlockId) -> Result<Option<Data>> {
        let loaded = self.underlying_block_store.load(id).await?;
        match loaded {
            None => Ok(None),
            Some(data) => Ok(Some(self._decrypt(data).await?)),
        }
    }

    async fn num_blocks(&self) -> Result<u64> {
        self.underlying_block_store.num_blocks().await
    }

    fn estimate_num_free_bytes(&self) -> Result<u64> {
        self.underlying_block_store.estimate_num_free_bytes()
    }

    fn block_size_from_physical_block_size(&self, block_size: u64) -> Result<u64> {
        let ciphertext_size = block_size.checked_sub(FORMAT_VERSION_HEADER.len() as u64)
            .with_context(|| anyhow!("Physical block size of {} is too small to hold even the FORMAT_VERSION_HEADER. Must be at least {}.", block_size, FORMAT_VERSION_HEADER.len()))?;
        ciphertext_size
            .checked_sub(C::CIPHERTEXT_OVERHEAD as u64)
            .with_context(|| anyhow!("Physical block size of {} is too small.", block_size))
    }

    async fn all_blocks(&self) -> Result<Pin<Box<dyn Stream<Item = Result<BlockId>> + Send>>> {
        self.underlying_block_store.all_blocks().await
    }
}

#[async_trait]
impl<C: Cipher + Send + Sync, B: BlockStoreDeleter + Send + Sync> BlockStoreDeleter
    for EncryptedBlockStore<C, B>
{
    async fn remove(&self, id: &BlockId) -> Result<RemoveResult> {
        self.underlying_block_store.remove(id).await
    }
}

create_block_data_wrapper!(BlockData);

#[async_trait]
impl<C: Cipher + Send + Sync, B: OptimizedBlockStoreWriter + Send + Sync> OptimizedBlockStoreWriter
    for EncryptedBlockStore<C, B>
{
    type BlockData = BlockData;

    fn allocate(size: usize) -> Self::BlockData {
        let mut data =
            B::allocate(FORMAT_VERSION_HEADER.len() + C::CIPHERTEXT_OVERHEAD + size).extract();
        data.shrink_to_subregion((FORMAT_VERSION_HEADER.len() + C::CIPHERTEXT_OVERHEAD)..);
        BlockData::new(data)
    }

    async fn try_create_optimized(
        &self,
        id: &BlockId,
        data: Self::BlockData,
    ) -> Result<TryCreateResult> {
        let ciphertext = self._encrypt(data.extract()).await?;
        self.underlying_block_store
            .try_create_optimized(id, B::BlockData::new(ciphertext))
            .await
    }

    async fn store_optimized(&self, id: &BlockId, data: Self::BlockData) -> Result<()> {
        let ciphertext = self._encrypt(data.extract()).await?;
        self.underlying_block_store
            .store_optimized(id, B::BlockData::new(ciphertext))
            .await
    }
}

impl<C: Cipher + Send + Sync, B: BlockStore + OptimizedBlockStoreWriter + Send + Sync> BlockStore
    for EncryptedBlockStore<C, B>
{
}

impl<C: Cipher, B> EncryptedBlockStore<C, B> {
    async fn _encrypt(&self, plaintext: Data) -> Result<Data> {
        // TODO Limit concurrency for CPU bound computations, maybe use semaphore or rayon?
        let ciphertext = tokio::task::block_in_place(move || self.cipher.encrypt(plaintext))?;
        Ok(_prepend_header(ciphertext))
    }

    async fn _decrypt(&self, ciphertext: Data) -> Result<Data> {
        // TODO Limit concurrency for CPU bound computations, maybe use semaphore or rayon?
        let ciphertext = _check_and_remove_header(ciphertext)?;
        tokio::task::block_in_place(move || self.cipher.decrypt(ciphertext)).map(|d| d.into())
    }
}

fn _check_and_remove_header(mut data: Data) -> Result<Data> {
    if !data.starts_with(FORMAT_VERSION_HEADER) {
        bail!(
            "Couldn't parse encrypted block. Expected FORMAT_VERSION_HEADER of {:?} but found {:?}",
            FORMAT_VERSION_HEADER,
            &data[..FORMAT_VERSION_HEADER.len()]
        );
    }
    data.shrink_to_subregion(FORMAT_VERSION_HEADER.len()..);
    Ok(data)
}

fn _prepend_header(mut data: Data) -> Data {
    // TODO Use binary-layout here?
    data.grow_region_fail_if_reallocation_necessary(FORMAT_VERSION_HEADER.len(), 0)
        .expect(
            "Tried to grow the data to contain the header in EncryptedBlockStore::_prepend_header",
        );
    data.as_mut()[..FORMAT_VERSION_HEADER.len()].copy_from_slice(FORMAT_VERSION_HEADER);
    data
}