//! Procedural macro backend for the `guarden` workspace.
//!
//! This crate exposes the internal `__guarded` proc macro that powers the public
//! macros re-exported by `guarden`.

use proc_macro::TokenStream;

mod guarded;

#[doc(hidden)]
#[proc_macro]
pub fn __guarded(input: TokenStream) -> TokenStream {
    guarded::proc(input.into())
        .map(Into::into)
        .unwrap_or_else(|e| e.to_compile_error().into())
}
