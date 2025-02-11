pub use ::dependency_crate_real_name_a_macros::*;

#[test]
pub fn test_macro_expansion() {
  let resolved_name = ::dependency_crate_real_name_a_macros::resolved_name!();
  assert_eq!("::dependency_crate_real_name_a", resolved_name);
}
