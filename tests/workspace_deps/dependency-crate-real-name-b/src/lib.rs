pub use ::dependency_crate_real_name_b_macros::*;

#[test]
pub fn test_macro_expansion() {
  let resolved_name = ::dependency_crate_real_name_b_macros::resolved_name!();
  assert_eq!("::dependency_crate_real_name_b", resolved_name);
}
