use anyhow::Result;
use async_trait::async_trait;
use futures::{
    future,
    stream::{self, Stream, StreamExt, TryStreamExt},
};
use std::collections::HashSet;
use std::fmt::{self, Debug};
use std::pin::Pin;
use std::sync::Arc;

use crate::blockstore::BlockId;
use crate::data::Data;
use crate::utils::async_drop::{AsyncDrop, AsyncDropGuard};

mod cache;
use cache::{BlockBaseStoreState, BlockCache, BlockCacheEntryGuard, CacheEntryState};

pub struct Block<B: super::low_level::BlockStore + Send + Sync + 'static> {
    cache_entry: BlockCacheEntryGuard<B>,
}

impl<B: super::low_level::BlockStore + Send + Sync> Block<B> {
    #[inline]
    pub fn block_id(&self) -> &BlockId {
        &self.cache_entry.key()
    }

    #[inline]
    pub fn data(&self) -> &Data {
        self.cache_entry
            .value()
            .expect("An existing block cannot have a None cache entry")
            .data()
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut Data {
        self.cache_entry
            .value_mut()
            .expect("An existing block cannot have a None cache entry")
            .data_mut()
    }

    pub async fn flush(&mut self) -> Result<()> {
        let block_id = *self.block_id();
        self.cache_entry
            .value_mut()
            .expect("An existing block cannot have a None cache entry")
            .flush(&block_id)
            .await
    }

    pub async fn resize(&mut self, new_size: usize) {
        self.cache_entry
            .value_mut()
            .expect("An existing block cannot have a None cache entry")
            .resize(new_size)
            .await;
    }
}

impl<B: super::low_level::BlockStore + Send + Sync + 'static> fmt::Debug for Block<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("block_id", self.block_id())
            .field("cache_entry", &self.cache_entry)
            .finish()
    }
}

// TODO Should we require B: OptimizedBlockStoreWriter and use its methods?
pub struct LockingBlockStore<B: super::low_level::BlockStore + Send + Sync + 'static> {
    base_store: Arc<B>,

    // cache doubles as a cache for blocks that are being returned and might be
    // re-requested, and as a set of mutexes making sure we don't concurrently
    // do multiple actions on the same block (e.g. remove it while it is loaded).
    cache: AsyncDropGuard<BlockCache<B>>,
}

impl<B: super::low_level::BlockStore + Send + Sync + 'static> LockingBlockStore<B> {
    pub fn new(base_store: B) -> AsyncDropGuard<Self> {
        AsyncDropGuard::new(Self {
            base_store: Arc::new(base_store),
            cache: BlockCache::new(),
        })
    }

    pub async fn load(&self, block_id: BlockId) -> Result<Option<Block<B>>> {
        // TODO Cache non-existence?
        let mut cache_entry = self.cache.async_lock(block_id).await;
        if cache_entry.value().is_none() {
            let loaded = self.base_store.load(&block_id).await?;
            if let Some(loaded) = loaded {
                self.cache.set_entry(
                    &self.base_store,
                    &mut cache_entry,
                    loaded,
                    CacheEntryState::Clean,
                    BlockBaseStoreState::ExistsInBaseStore,
                );
            }
        }
        if cache_entry.value().is_some() {
            Ok(Some(Block { cache_entry }))
        } else {
            Ok(None)
        }
    }

    pub async fn try_create(&self, block_id: &BlockId, data: &Data) -> Result<TryCreateResult> {
        let mut cache_entry = self.cache.async_lock(*block_id).await;
        if cache_entry.value().is_some() {
            // Block already exists in the cache
            return Ok(TryCreateResult::NotCreatedBecauseBlockIdAlreadyExists);
        }
        if self.base_store.exists(block_id).await? {
            return Ok(TryCreateResult::NotCreatedBecauseBlockIdAlreadyExists);
        }
        self.cache.set_entry(
            &self.base_store,
            &mut cache_entry,
            data.clone(),
            CacheEntryState::Dirty,
            BlockBaseStoreState::DoesntExistInBaseStore,
        );
        Ok(TryCreateResult::SuccessfullyCreated)
    }

    pub async fn overwrite(&self, block_id: &BlockId, data: &Data) -> Result<()> {
        let mut cache_entry = self.cache.async_lock(*block_id).await;

        let exists_in_base_store = || async {
            if self.base_store.exists(block_id).await? {
                Ok(BlockBaseStoreState::ExistsInBaseStore)
            } else {
                Ok(BlockBaseStoreState::DoesntExistInBaseStore)
            }
        };

        // Add the new value to the cache.
        self.cache
            .set_or_overwrite_entry_even_if_dirty(
                &self.base_store,
                &mut cache_entry,
                data.clone(),
                CacheEntryState::Dirty,
                exists_in_base_store,
            )
            .await?;

        Ok(())
    }

    pub async fn remove(&self, block_id: &BlockId) -> Result<RemoveResult> {
        // TODO Don't write-through but cache remove operations?

        let mut cache_entry_guard = self.cache.async_lock(*block_id).await;

        // Remove from cache
        // TODO This is dangerous, we could accidentally drop the cache entry lock if we put it into the let binding by value but it needs to be held while we remove from the base store. Instead make removed_from_base_store a lambda and invoke it from in here?
        let (removed_from_cache, should_remove_from_base_store) =
            if let Some(cache_entry) = cache_entry_guard.value() {
                let should_remove_from_base_store = cache_entry.block_exists_in_base_store()
                    == BlockBaseStoreState::ExistsInBaseStore;
                self.cache
                    .delete_entry_from_cache_even_if_dirty(&mut cache_entry_guard);
                (true, should_remove_from_base_store)
            } else {
                (false, true)
            };

        let removed_from_base_store = if should_remove_from_base_store {
            match self.base_store.remove(block_id).await? {
                crate::blockstore::low_level::RemoveResult::SuccessfullyRemoved => true,
                crate::blockstore::low_level::RemoveResult::NotRemovedBecauseItDoesntExist => false,
            }
        } else {
            false
        };

        if removed_from_cache || removed_from_base_store {
            Ok(RemoveResult::SuccessfullyRemoved)
        } else {
            Ok(RemoveResult::NotRemovedBecauseItDoesntExist)
        }
    }

    // Note: for any blocks that are created or removed while the returned stream is running,
    // we don't give any guarantees for whether they're counted or not.
    pub async fn num_blocks(&self) -> Result<u64> {
        Ok(self.base_store.num_blocks().await?
            + self.cache.num_blocks_in_cache_but_not_in_base_store())
    }

    pub fn estimate_num_free_bytes(&self) -> Result<u64> {
        self.base_store.estimate_num_free_bytes()
    }

    pub fn block_size_from_physical_block_size(&self, block_size: u64) -> Result<u64> {
        self.base_store
            .block_size_from_physical_block_size(block_size)
    }

    // Note: for any blocks that are created or removed while the returned stream is running,
    // we don't give any guarantees for whether they'll be part of the stream or not.
    // TODO Make sure we have tests that have some blocks in the cache and some in the base store
    pub async fn all_blocks(&self) -> Result<Pin<Box<dyn Stream<Item = Result<BlockId>> + Send>>> {
        let blocks_in_cache = self.cache.keys();
        let blocks_in_base_store = self.base_store.all_blocks().await?;

        let blocks_in_cache_set: HashSet<_> = blocks_in_cache.iter().copied().collect();
        let blocks_in_base_store_and_not_in_cache = blocks_in_base_store
            .try_filter(move |block_id| future::ready(!blocks_in_cache_set.contains(block_id)));

        Ok(Box::pin(
            stream::iter(blocks_in_cache.into_iter().map(Ok))
                .chain(blocks_in_base_store_and_not_in_cache),
        ))
    }

    pub async fn create(&self, data: &Data) -> Result<()> {
        loop {
            let created = self.try_create(&BlockId::new_random(), data).await?;
            match created {
                TryCreateResult::NotCreatedBecauseBlockIdAlreadyExists => {
                    /* just continue */
                    ()
                }
                TryCreateResult::SuccessfullyCreated => {
                    return Ok(());
                }
            }
        }
    }
}

#[async_trait]
impl<B: crate::blockstore::low_level::BlockStore + Send + Sync + 'static> AsyncDrop
    for LockingBlockStore<B>
{
    type Error = anyhow::Error;

    async fn async_drop_impl(mut self) -> Result<()> {
        self.cache.async_drop().await?;
        Ok(())
    }
}

impl<B: crate::blockstore::low_level::BlockStore + Send + Sync + 'static> Debug
    for LockingBlockStore<B>
{
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("LockingBlockStore").finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum TryCreateResult {
    SuccessfullyCreated,
    NotCreatedBecauseBlockIdAlreadyExists,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RemoveResult {
    SuccessfullyRemoved,
    NotRemovedBecauseItDoesntExist,
}
