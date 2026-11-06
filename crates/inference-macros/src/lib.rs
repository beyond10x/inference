//! The attribute macros of the `inference` SDK, re-exported as `inference::tool` and
//! `inference::main` (`docs/design/sdk.md` §3.2, §5).
//!
//! Each macro emits only `::inference::…` paths, so an application with the one `inference`
//! dependency compiles what they generate. Until their expansion is written, each attribute
//! expands to a `compile_error!` that names it, so no program builds on an empty expansion.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote_spanned;

/// `#[inference::tool]`: an `async fn` becomes an `inference::tools::Tool` and its executor.
/// Not expanded yet: it emits a `compile_error!`.
#[proc_macro_attribute]
pub fn tool(_attribute: TokenStream, _item: TokenStream) -> TokenStream {
    not_yet("#[inference::tool]")
}

/// `#[inference::main]`: an `async fn main(host: impl AudioHost, stack: Stack)` becomes the
/// program and its `run` seam. Not expanded yet: it emits a `compile_error!`.
#[proc_macro_attribute]
pub fn main(_attribute: TokenStream, _item: TokenStream) -> TokenStream {
    not_yet("#[inference::main]")
}

fn not_yet(attribute: &str) -> TokenStream {
    let message = format!("{attribute} is not implemented yet");
    let message = syn::LitStr::new(&message, Span::call_site());
    quote_spanned!(Span::call_site()=> ::core::compile_error!(#message);).into()
}
