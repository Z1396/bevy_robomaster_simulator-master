//! Lock-free triple-buffer implementation for single-producer single-consumer IPC
//!
//! Provides a wait-free, single-producer single-consumer triple buffer that
//! uses atomics for synchronization. The producer writes to the current write
//! slot, then atomically swaps the state to signal availability. The consumer
//! reads the latest ready slot and clears the new-data flag.
//!
//! The state encoding uses the highest bit as a FLAG_NEW indicator and the
//! lower 2 bits as the index of the ready slot (0, 1, 2).

use crate::layout::{FLAG_NEW, INDEX_MASK};
use std::sync::atomic::Ordering;

/// Lock-free triple-buffer producer
///
/// Writes data to the current write slot and publishes it by atomically
/// swapping the shared state byte. The producer holds a mutable reference to
/// the write index, so only one producer may exist at a time.
///
/// # Safety
///
/// The caller must ensure that at most one `TripleBufferProducer` exists for
/// a given triple buffer at any time.
pub struct TripleBufferProducer<'a, S> {
    /// Shared atomic state byte (FLAG_NEW | ready_index)
    state: &'a std::sync::atomic::AtomicU8,
    /// Index of the slot the producer should write to next
    write_idx: &'a mut u8,
    /// Three data slots
    slots: &'a mut [S; 3],
}

impl<'a, S> TripleBufferProducer<'a, S> {
    /// Create a new producer
    ///
    /// # Safety
    ///
    /// The caller must ensure that only one producer exists for this buffer.
    /// Concurrent producers will cause data races on `write_idx`.
    pub unsafe fn new(
        state: &'a std::sync::atomic::AtomicU8,
        write_idx: &'a mut u8,
        slots: &'a mut [S; 3],
    ) -> Self {
        Self {
            state,
            write_idx,
            slots,
        }
    }

    /// Get a mutable reference to the current write slot
    pub fn borrow_mut(&mut self) -> &mut S {
        &mut self.slots[*self.write_idx as usize]
    }

    /// Publish the current write slot
    ///
    /// Atomically swaps the state byte to `write_idx | FLAG_NEW`, which
    /// simultaneously marks the data as new and records which slot is ready.
    /// The old state's index (lower 2 bits) becomes the next write index,
    /// ensuring the producer never overwrites the slot the consumer is reading.
    pub fn publish(&mut self) {
        // Swap: write_idx|FLAG_NEW into state, old state value becomes new write_idx
        let old = self
            .state
            .swap(*self.write_idx | FLAG_NEW, Ordering::AcqRel);
        // The old state's slot index (lower 2 bits) is now safe to write to
        *self.write_idx = old & INDEX_MASK;
    }
}

/// Lock-free triple-buffer consumer
///
/// Reads the latest published slot by attempting a CAS on the shared state
/// byte to clear the FLAG_NEW flag. If the CAS succeeds, the consumer has
/// exclusive access to the slot until the next publish.
///
/// # Safety
///
/// The caller must ensure that at most one `TripleBufferConsumer` exists for
/// a given triple buffer at any time.
pub struct TripleBufferConsumer<'a, S> {
    /// Shared atomic state byte (FLAG_NEW | ready_index)
    state: &'a std::sync::atomic::AtomicU8,
    /// Index of the slot the consumer last read
    read_idx: &'a mut u8,
    /// Three data slots (immutable references for reading)
    slots: &'a [S; 3],
}

impl<'a, S> TripleBufferConsumer<'a, S> {
    /// Create a new consumer
    ///
    /// # Safety
    ///
    /// The caller must ensure that only one consumer exists for this buffer.
    /// Concurrent consumers will cause data races on `read_idx`.
    pub unsafe fn new(
        state: &'a std::sync::atomic::AtomicU8,
        read_idx: &'a mut u8,
        slots: &'a [S; 3],
    ) -> Self {
        Self {
            state,
            read_idx,
            slots,
        }
    }

    /// Try to borrow the latest published slot
    ///
    /// If new data is available (FLAG_NEW is set), attempts to CAS the state
    /// from `(ready_idx | FLAG_NEW)` to `read_idx` (clearing the flag). On
    /// success, updates `read_idx` and returns `Some(&S)`. If the CAS fails
    /// (e.g., the producer just published a newer frame), retries once.
    ///
    /// Returns `None` if no new data is available or if the CAS fails twice.
    pub fn borrow(&mut self) -> Option<&S> {
        let mut expected = self.state.load(Ordering::Acquire);

        // No new data available
        if (expected & FLAG_NEW) == 0 {
            return None;
        }

        let mut ready_idx = expected & INDEX_MASK;
        let mut desired = *self.read_idx;

        // First CAS attempt: clear FLAG_NEW, set state to old read_idx
        match self.state.compare_exchange_weak(
            expected,
            desired,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                *self.read_idx = ready_idx;
                Some(&self.slots[ready_idx as usize])
            }
            Err(new_expected) => {
                // Producer published another frame between our load and CAS
                expected = new_expected;
                if (expected & FLAG_NEW) == 0 {
                    return None;
                }
                ready_idx = expected & INDEX_MASK;
                desired = *self.read_idx;

                // Second (and final) CAS attempt
                match self.state.compare_exchange_weak(
                    expected,
                    desired,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        *self.read_idx = ready_idx;
                        Some(&self.slots[ready_idx as usize])
                    }
                    Err(_) => None,
                }
            }
        }
    }

    /// Check if new data is available without consuming it
    ///
    /// This is useful for non-blocking polling of data availability.
    /// Returns `true` if a call to `borrow()` would return `Some(_)`.
    #[must_use]
    #[allow(dead_code)]
    pub fn has_new_data(&self) -> bool {
        (self.state.load(Ordering::Acquire) & FLAG_NEW) != 0
    }
}
