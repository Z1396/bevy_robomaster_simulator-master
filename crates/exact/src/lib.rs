//! Exact-size iterator utilities
//!
//! Provides the [`ExactExt`] trait for collecting an iterator into a fixed-size
//! array with exact-length enforcement, and the [`ExactOneExt`] trait for
//! safely extracting the single element from a 1-element array without
//! copying.
//!
//! Both traits handle dropping of partially initialized elements on error,
//! making them safe to use with types that implement `Drop`.

use std::mem::ManuallyDrop;

/// Error type for [`ExactExt::exact`]
///
/// Indicates whether the iterator produced fewer items than requested
/// ([`Insufficient`](ExactError::Insufficient)) or more items than requested
/// ([`Overflow`](ExactError::Overflow)).
#[derive(Debug)]
pub enum ExactError {
    /// The iterator produced fewer items than the requested array size `N`
    Insufficient,
    /// The iterator produced more items than the requested array size `N`
    Overflow,
}

/// Extension trait for collecting an iterator into an exact-size array
///
/// Returns `Ok([T; N])` if the iterator produces exactly `N` items,
/// or `Err(ExactError)` if the iterator produces fewer or more items.
///
/// On error, any items already consumed from the iterator are properly dropped.
pub trait ExactExt<T> {
    /// The output array type parameterized by const generic `N`
    type Output<const N: usize>;
    /// Collect the iterator into a `[T; N]` array, validating exact length
    ///
    /// # Errors
    ///
    /// Returns `Err(ExactError::Insufficient)` if the iterator yields fewer
    /// than `N` items, or `Err(ExactError::Overflow)` if it yields more
    /// than `N` items. In either case, partially consumed items are dropped.
    fn exact<const N: usize>(self) -> Result<Self::Output<N>, ExactError>;
}

/// Extension trait for extracting the single element from a 1-element array
///
/// Avoids the need to destructure or copy the array. The array is consumed
/// and the inner element is moved out via [`ManuallyDrop`] + `ptr::read`.
pub trait ExactOneExt {
    /// The output type (the unwrapped element type)
    type Output;
    /// Consume the array and return its sole element
    fn into_single(self) -> Self::Output;
}

/// Safe move-out of a single element from a 1-element array
///
/// Uses `ManuallyDrop` to prevent double-drop, then `ptr::read` to move
/// the element out without copying the array.
impl<T> ExactOneExt for [T; 1] {
    type Output = T;

    fn into_single(self) -> T {
        // Wrap in ManuallyDrop to prevent the array's Drop from running
        let array = ManuallyDrop::new(self);
        // Read the element out — safe because the array has exactly one element
        unsafe { std::ptr::read(&array[0]) }
    }
}

/// Convenience implementation for `Option<[T; 1]>`
///
/// Maps `None` to `None` and `Some(arr)` to `Some(arr.into_single())`.
impl<T> ExactOneExt for Option<[T; 1]> {
    type Output = Option<T>;

    fn into_single(self) -> Option<T> {
        self.map(|v| v.into_single())
    }
}

/// Generic implementation of [`ExactExt`] for any [`Iterator`]
///
/// Uses `MaybeUninit` to construct the array without requiring `T: Default`
/// or `T: Copy`. On error, properly drops any partially initialized elements
/// before returning.
impl<I, T> ExactExt<T> for I
where
    I: Iterator<Item = T>,
{
    type Output<const N: usize> = [T; N];

    /// Collect exactly `N` items from the iterator into a fixed-size array
    ///
    /// Algorithm:
    ///   1. Allocate a `MaybeUninit<T>` array of size `N`
    ///   2. Iterate up to `N`, writing each value into the uninit array
    ///   3. If the iterator runs dry before `N`, drop already-written values
    ///      and return `Err(Insufficient)`
    ///   4. If the iterator still has items after `N`, drop all written values
    ///      and return `Err(Overflow)`
    ///   5. Otherwise, transmute the `MaybeUninit` array to `[T; N]` and return
    ///      `Ok(...)`
    fn exact<const N: usize>(mut self) -> Result<Self::Output<N>, ExactError> {
        use std::mem::MaybeUninit;
        // Allocate uninitialized array on the stack
        let mut data: [MaybeUninit<T>; N] = [const { MaybeUninit::uninit() }; N];

        // Fill up to N elements
        for i in 0..N {
            if let Some(val) = self.next() {
                data[i].write(val);
            } else {
                // Iterator exhausted — drop what we wrote and report error
                for j in 0..i {
                    unsafe { data[j].assume_init_drop() };
                }
                return Err(ExactError::Insufficient);
            }
        }

        // Check for extra items beyond N
        if self.next().is_some() {
            // Drop all N elements and report overflow
            for j in 0..N {
                unsafe { data[j].assume_init_drop() };
            }
            return Err(ExactError::Overflow);
        }

        // Transmute the MaybeUninit array into [T; N] — safe because all N
        // elements have been initialized and no extra items remain
        Ok(unsafe { std::ptr::read(&data as *const _ as *const [T; N]) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A helper type that counts how many times it is dropped
    ///
    /// Used to verify that `ExactExt` and `ExactOneExt` properly drop
    /// elements on error paths.
    #[derive(Debug)]
    struct DropCounter<'a> {
        counter: &'a Cell<usize>,
    }

    impl<'a> Drop for DropCounter<'a> {
        fn drop(&mut self) {
            self.counter.set(self.counter.get() + 1);
        }
    }

    /// Verify that `exact::<3>()` succeeds when the iterator has exactly 3 items
    #[test]
    fn test_exact_ok() {
        let counter = Cell::new(0);
        let items = vec![
            DropCounter { counter: &counter },
            DropCounter { counter: &counter },
            DropCounter { counter: &counter },
        ];

        let result: Result<[DropCounter; 3], ExactError> = items.into_iter().exact();
        assert!(result.is_ok());
        let arr = result.unwrap();
        assert_eq!(counter.get(), 0);

        // After dropping the array, all 3 elements should be dropped
        drop(arr);
        assert_eq!(counter.get(), 3);
    }

    /// Verify that `exact::<3>()` returns `Err(Insufficient)` with 1 item
    ///
    /// The single item should be dropped as part of the cleanup.
    #[test]
    fn test_exact_insufficient() {
        let counter = Cell::new(0);
        let items = vec![DropCounter { counter: &counter }];

        let result: Result<[DropCounter; 3], ExactError> = items.into_iter().exact();
        assert!(matches!(result, Err(ExactError::Insufficient)));
        // The single item should have been dropped during cleanup
        assert_eq!(counter.get(), 1);
    }

    /// Verify that `exact::<3>()` returns `Err(Overflow)` with 4 items
    ///
    /// All 4 items should be dropped as part of the cleanup.
    #[test]
    fn test_exact_overflow() {
        let counter = Cell::new(0);
        let items = vec![
            DropCounter { counter: &counter },
            DropCounter { counter: &counter },
            DropCounter { counter: &counter },
            DropCounter { counter: &counter },
        ];

        let result: Result<[DropCounter; 3], ExactError> = items.into_iter().exact();
        assert!(matches!(result, Err(ExactError::Overflow)));
        // All 4 items (3 from the array + 1 extra) should have been dropped
        assert_eq!(counter.get(), 4);
    }

    /// Verify that `into_single()` moves the element out without dropping
    ///
    /// The drop counter should only increment after the moved value is
    /// explicitly dropped.
    #[test]
    fn test_into_single_move() {
        let counter = Cell::new(0);
        let arr = [DropCounter { counter: &counter }];

        let val = arr.into_single();
        assert_eq!(counter.get(), 0);
        drop(val);
        assert_eq!(counter.get(), 1);
    }
}
