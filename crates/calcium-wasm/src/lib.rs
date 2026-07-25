//! The wasm module is the C ABI, re-exported.
//!
//! `#[no_mangle]` symbols from a linked dependency are carried into a cdylib's
//! export table; this crate exists only to give them a wasm-only home. See
//! calcium-ffi's Cargo.toml for why the crate types must live apart.
pub use calcium_ffi::*;
