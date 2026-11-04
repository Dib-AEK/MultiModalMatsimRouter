//! Selects the optional application allocator; this module is never included in the routing library.
#[cfg(feature = "fast-allocator")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
