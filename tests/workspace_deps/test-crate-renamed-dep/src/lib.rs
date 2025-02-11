#[test]
pub fn test_macro_expansion() {
  let resolved_name = dependency_crate_renamed_a::resolved_name!();
  assert_eq!("::dependency_crate_renamed_a", resolved_name);
}
