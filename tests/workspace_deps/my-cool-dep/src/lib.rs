use proc_macro::TokenStream;

#[proc_macro]
pub fn do_something(input: TokenStream) -> TokenStream {
  tracing_proc_macros_ink::proc_macro_logger_default_setup();
  let found_crate = cargo_manifest_proc_macros::CargoManifest::shared()
    .resolve_crate_path("my-cool-dep-real-name", &[]);

  assert_eq!(
    "::my_cool_dep",
    cargo_manifest_proc_macros::syn_utils::pretty_format_syn_path(&found_crate)
  );

  input
}
