use proc_macro::TokenStream;

#[proc_macro]
pub fn resolved_name(_input: TokenStream) -> TokenStream {
  let found_crate = cargo_manifest_proc_macros::syn_utils::pretty_format_syn_path(
    &cargo_manifest_proc_macros::CargoManifest::shared()
      .resolve_crate_path("dependency-crate-real-name-b", &[]),
  );

  quote::quote! {
    #found_crate
  }
  .into()
}
