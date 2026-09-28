//! Minimal proc macro built in the exec configuration by
//! `exec_proc_macro_macho_test` to check host-tool Mach-O output.

use proc_macro::TokenStream;

#[proc_macro]
pub fn exec_proc_macro_probe(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
