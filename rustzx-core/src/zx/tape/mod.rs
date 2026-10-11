mod empty;
mod tap;

pub use empty::Empty;
pub use tap::Tap;

use crate::{
    host::{LoadableAsset, SeekableAsset},
    Result,
};

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum ZXTape<A: LoadableAsset + SeekableAsset> {
    Tap(Tap<A>),
    Empty(Empty),
}

impl<A: LoadableAsset + SeekableAsset> Default for ZXTape<A> {
    fn default() -> Self {
        Self::Empty(Empty)
    }
}

impl<A: LoadableAsset + SeekableAsset> From<Tap<A>> for ZXTape<A> {
    fn from(tap: Tap<A>) -> Self {
        Self::Tap(tap)
    }
}

impl<A: LoadableAsset + SeekableAsset> From<Empty> for ZXTape<A> {
    fn from(empty: Empty) -> Self {
        Self::Empty(empty)
    }
}

/// Calls the same method on whichever tape it is
macro_rules! dispatch {
    ($self:ident, $tape:ident => $call:expr) => {
        match $self {
            Self::Tap($tape) => $call,
            Self::Empty($tape) => $call,
        }
    };
}

impl<A: LoadableAsset + SeekableAsset> TapeImpl for ZXTape<A> {
    fn can_fast_load(&self) -> bool {
        dispatch!(self, t => t.can_fast_load())
    }
    fn next_block_byte(&mut self) -> Result<Option<u8>> {
        dispatch!(self, t => t.next_block_byte())
    }
    fn next_block(&mut self) -> Result<bool> {
        dispatch!(self, t => t.next_block())
    }
    fn current_bit(&self) -> bool {
        dispatch!(self, t => t.current_bit())
    }
    fn process_clocks(&mut self, clocks: usize) -> Result<()> {
        dispatch!(self, t => t.process_clocks(clocks))
    }
    fn stop(&mut self) {
        dispatch!(self, t => t.stop())
    }
    fn play(&mut self) {
        dispatch!(self, t => t.play())
    }
    fn rewind(&mut self) -> Result<()> {
        dispatch!(self, t => t.rewind())
    }
}

pub trait TapeImpl {
    fn can_fast_load(&self) -> bool;
    /// Returns byte of current block or `None` if block has ended
    fn next_block_byte(&mut self) -> Result<Option<u8>>;
    /// Loads next block. Returns false if end of the tape is reached
    fn next_block(&mut self) -> Result<bool>;
    /// Returns current tape (`ear`) bit
    fn current_bit(&self) -> bool;
    /// Perform tape processing emulation within `clocks` time limit
    fn process_clocks(&mut self, clocks: usize) -> Result<()>;
    fn stop(&mut self);
    fn play(&mut self);
    /// Rewinds tape content to the beginning
    fn rewind(&mut self) -> Result<()>;
}
