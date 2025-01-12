use proc_macro::TokenStream;

#[proc_macro]
pub fn resolved_name(_input: TokenStream) -> TokenStream {
  tracing_proc_macros_ink::proc_macro_logger_default_setup();
  let found_crate = cargo_manifest_proc_macros::syn_utils::pretty_format_syn_path(
    &cargo_manifest_proc_macros::CargoManifest::shared()
      .resolve_crate_path("my-cool-dep-real-name", &[]),
  );

  quote::quote! {
    #found_crate
  }
  .into()
}
