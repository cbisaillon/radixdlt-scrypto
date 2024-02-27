#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(any(feature = "std", feature = "alloc")))]
compile_error!("Either feature `std` or `alloc` must be enabled for this crate.");
#[cfg(all(feature = "std", feature = "alloc"))]
compile_error!("Feature `std` and `alloc` can't be enabled at the same time.");

pub mod committable_overlay;
pub mod hash_tree;
pub mod memory_db;
#[cfg(feature = "rocksdb")]
pub mod rocks_db;
#[cfg(feature = "rocksdb")]
pub mod rocks_db_with_merkle_tree;

pub mod hash_tree_support;
