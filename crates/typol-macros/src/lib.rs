mod attributes;
mod expand;
mod model;

#[cfg(test)]
mod tests;

use expand::Expander;
use model::Model;
use proc_macro::TokenStream;
use syn::{DeriveInput, parse};

/// Generates descriptors and bidirectional conversions from owned Rust declarations without reading Schema files.
#[proc_macro_derive(Typol, attributes(typol))]
pub fn derive_typol(input: TokenStream) -> TokenStream {
    parse::<DeriveInput>(input)
        .and_then(Model::parse)
        .and_then(|model| Expander::expand(&model))
        .unwrap_or_else(|error| error.into_compile_error())
        .into()
}
