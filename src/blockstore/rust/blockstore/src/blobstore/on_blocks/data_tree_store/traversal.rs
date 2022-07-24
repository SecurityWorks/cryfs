use anyhow::{anyhow, Error, Result};
use async_recursion::async_recursion;
use futures::stream::{FuturesUnordered, Stream, StreamExt};
use std::future::{self, Future};

use crate::blobstore::on_blocks::data_node_store::{
    DataInnerNode, DataLeafNode, DataNode, DataNodeStore,
};
use crate::blockstore::low_level::BlockStore;
use crate::utils::stream::{for_each_unordered, run_to_completion};

#[async_recursion]
pub async fn all_leaves<B, F>(
    store: &DataNodeStore<B>,
    root: &mut DataNode<B>,
    on_leaf: &(impl Sync + Fn(&mut DataLeafNode<B>) -> F),
) -> Result<()>
where
    B: BlockStore + Send + Sync,
    F: Future<Output = Result<()>> + Send,
{
    match root {
        DataNode::Leaf(leaf) => {
            on_leaf(leaf).await?;
        }
        DataNode::Inner(inner) => {
            let stream = _load_children(store, inner)?
                .map(|child| async move {
                    all_leaves(store, &mut child?, on_leaf).await?;
                    Ok(())
                })
                .buffer_unordered(usize::MAX);
            run_to_completion(stream).await?;
        }
    }
    Ok(())
}

fn _load_children<'a, 'b, B: BlockStore + Send + Sync>(
    store: &'a DataNodeStore<B>,
    inner: &'b DataInnerNode<B>,
) -> Result<FuturesUnordered<impl 'a + Future<Output = Result<DataNode<B>>>>> {
    let futures: FuturesUnordered<_> = inner
        .children()?
        .map(move |child_id| async move {
            let loaded: Result<DataNode<B>> = Ok(store.load(child_id).await?.ok_or_else(|| {
                anyhow!(
                    "Tried to load child node {:?} but couldn't find it",
                    child_id,
                )
            })?);
            loaded
        })
        .collect();
    Ok(futures)
}