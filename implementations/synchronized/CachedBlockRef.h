#pragma once
#ifndef BLOCKSTORE_IMPLEMENTATIONS_SYNCHRONIZED_CACHEDBLOCKREF_H_
#define BLOCKSTORE_IMPLEMENTATIONS_SYNCHRONIZED_CACHEDBLOCKREF_H_

#include "../../interface/Block.h"

#include "messmer/cpp-utils/macros.h"
#include <memory>

namespace blockstore {
namespace synchronized {
class SynchronizedBlockStore;

class CachedBlockRef: public Block {
public:
  CachedBlockRef(Block *baseBlock, SynchronizedBlockStore *blockStore);
  virtual ~CachedBlockRef();

  const void *data() const override;
  void write(const void *source, uint64_t offset, uint64_t size) override;

  void flush() override;

  size_t size() const override;

private:
  Block *_baseBlock;
  SynchronizedBlockStore *_blockStore;

  DISALLOW_COPY_AND_ASSIGN(CachedBlockRef);
};

}
}

#endif
